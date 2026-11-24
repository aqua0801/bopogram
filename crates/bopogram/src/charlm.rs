//! Character language model in the engine's own format, run without ONNX
//! Runtime (`prototype/docs/charlm-native.md`; reference implementation
//! `prototype/bopogram/charlm_native.py`, scorer `prototype/bopogram/charlm.py`).
//!
//! The model re-ranks the decoder's best few results: it reads the text before
//! the cursor and each candidate, and says how natural the candidate reads,
//! including how it ends. Nothing here uses the network.
//!
//! The arithmetic follows the reference step by step so that both give the same
//! numbers: tensors are f32 between steps; every step computes in f64 and rounds
//! to f32 once; matrix products quantize the input row to 8 bits and sum the
//! integer products exactly. Sums of f64 that numpy reduces (means, softmax and
//! log-sum-exp denominators, the final total) use numpy's pairwise order.
//!
//! A causal model's state at a position depends only on the text up to it, so
//! the keys, values and output of the sequences scored recently are kept and a
//! new sequence starts where it parts from the closest of them.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

use serde_json::Value;

const MAGIC: &[u8; 8] = b"BPCLM\x00\x00\x01";
const ALIGN: usize = 64;
/// Sequences whose state is kept.
const PREFIX_CACHE: usize = 32;
/// Characters of context the model reads.
pub const MAX_CONTEXT: usize = 40;
/// (context, text) scores kept.
const CACHE_SIZE: usize = 2000;

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

// -- numpy-compatible arithmetic ----------------------------------------------

/// Sum in the order numpy's `add.reduce` uses along a contiguous axis.
pub fn pairwise_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        let mut res = 0.0;
        for &v in a {
            res += v;
        }
        res
    } else if n <= 128 {
        let mut r = [0.0f64; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i < n - n % 8 {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

fn round_half_away(x: f64) -> f64 {
    if x == 0.0 {
        return 0.0;
    }
    x.signum() * (x.abs() + 0.5).floor()
}

/// Abramowitz & Stegun 7.1.26; part of the format.
fn erf(x: f64) -> f64 {
    let a = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let y = ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t;
    let s = if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    };
    s * (1.0 - y * (-a * a).exp())
}

fn gelu(x: &mut [f32]) {
    for v in x {
        let x = *v as f64;
        // The same double as the reference's literal 0.7071067811865476.
        *v = (0.5 * x * (1.0 + erf(x * std::f64::consts::FRAC_1_SQRT_2))) as f32;
    }
}

/// Layer norm of each row of `x` (rows of `w.len()`), into `out`.
fn layer_norm(x: &[f32], w: &[f32], b: &[f32], eps: f64, out: &mut Vec<f32>) {
    let d = w.len();
    out.clear();
    let mut row = vec![0.0f64; d];
    let mut sq = vec![0.0f64; d];
    for xs in x.chunks_exact(d) {
        for (r, &v) in row.iter_mut().zip(xs) {
            *r = v as f64;
        }
        let mean = pairwise_sum(&row) / d as f64;
        for (s, &r) in sq.iter_mut().zip(&row) {
            let c = r - mean;
            *s = c * c;
        }
        let var = pairwise_sum(&sq) / d as f64;
        let div = (var + eps).sqrt();
        for t in 0..d {
            out.push(((row[t] - mean) / div * w[t] as f64 + b[t] as f64) as f32);
        }
    }
}

/// 8-bit quantization of one row: (scale, q).
fn quantize(x: &[f32], q: &mut [i8]) -> f32 {
    let m = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let scale = (m as f64 / 127.0) as f32;
    let safe = if scale == 0.0 { 1.0 } else { scale as f64 };
    for (q, &v) in q.iter_mut().zip(x) {
        *q = round_half_away(v as f64 / safe).clamp(-127.0, 127.0) as i8;
    }
    scale
}

// -- integer dot products --------------------------------------------------------

#[inline(always)]
fn dot_plain(a: &[i8], b: &[i8]) -> i32 {
    a.iter().zip(b).map(|(&x, &y)| x as i32 * y as i32).sum()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn dots_avx2(qx: &[i8], w: &[i8], n_in: usize, acc: &mut [i32]) {
    use std::arch::x86_64::*;
    let blocks = n_in / 16;
    for (j, out) in acc.iter_mut().enumerate() {
        let row = &w[j * n_in..(j + 1) * n_in];
        let mut s0 = _mm256_setzero_si256();
        let mut s1 = _mm256_setzero_si256();
        let mut k = 0;
        while k + 1 < blocks {
            let a0 = _mm256_cvtepi8_epi16(_mm_loadu_si128(qx.as_ptr().add(k * 16) as *const __m128i));
            let b0 = _mm256_cvtepi8_epi16(_mm_loadu_si128(row.as_ptr().add(k * 16) as *const __m128i));
            let a1 = _mm256_cvtepi8_epi16(_mm_loadu_si128(qx.as_ptr().add(k * 16 + 16) as *const __m128i));
            let b1 = _mm256_cvtepi8_epi16(_mm_loadu_si128(row.as_ptr().add(k * 16 + 16) as *const __m128i));
            s0 = _mm256_add_epi32(s0, _mm256_madd_epi16(a0, b0));
            s1 = _mm256_add_epi32(s1, _mm256_madd_epi16(a1, b1));
            k += 2;
        }
        if k < blocks {
            let a0 = _mm256_cvtepi8_epi16(_mm_loadu_si128(qx.as_ptr().add(k * 16) as *const __m128i));
            let b0 = _mm256_cvtepi8_epi16(_mm_loadu_si128(row.as_ptr().add(k * 16) as *const __m128i));
            s0 = _mm256_add_epi32(s0, _mm256_madd_epi16(a0, b0));
        }
        let s = _mm256_add_epi32(s0, s1);
        let half = _mm_add_epi32(_mm256_castsi256_si128(s), _mm256_extracti128_si256(s, 1));
        let half = _mm_add_epi32(half, _mm_shuffle_epi32(half, 0b01_00_11_10));
        let half = _mm_add_epi32(half, _mm_shuffle_epi32(half, 0b10_11_00_01));
        let tail = blocks * 16;
        *out = _mm_cvtsi128_si32(half) + dot_plain(&qx[tail..], &row[tail..]);
    }
}

/// `acc[j] = Σ qx · w[j]` for every row j of `w` (rows of `n_in`). Exact.
fn dots(qx: &[i8], w: &[i8], n_in: usize, acc: &mut [i32]) {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            // SAFETY: the CPU has AVX2; all reads stay inside the slices.
            unsafe { dots_avx2(qx, w, n_in, acc) };
            return;
        }
    }
    for (j, out) in acc.iter_mut().enumerate() {
        *out = dot_plain(qx, &w[j * n_in..(j + 1) * n_in]);
    }
}

/// `y = W x + b` with W as 8-bit rows and a scale per row.
struct QLinear {
    q: Vec<i8>,
    scale: Vec<f32>,
    bias: Vec<f32>,
    n_in: usize,
    n_out: usize,
}

impl QLinear {
    /// One output per row of `x` (rows of `n_in`), appended to `out`.
    fn apply(&self, x: &[f32], out: &mut Vec<f32>) {
        out.clear();
        let mut qx = vec![0i8; self.n_in];
        let mut acc = vec![0i32; self.n_out];
        for xs in x.chunks_exact(self.n_in) {
            let sx = quantize(xs, &mut qx) as f64;
            dots(&qx, &self.q, self.n_in, &mut acc);
            for ((&a, &scale), &bias) in acc.iter().zip(&self.scale).zip(&self.bias) {
                out.push((a as f64 * (sx * scale as f64) + bias as f64) as f32);
            }
        }
    }
}

struct Block {
    norm1: (Vec<f32>, Vec<f32>),
    norm2: (Vec<f32>, Vec<f32>),
    qkv: QLinear,
    proj: QLinear,
    ff1: QLinear,
    ff2: QLinear,
}

/// Keys and values of every layer and the final normalized output, per
/// position of one sequence; log-sum-exp of the output once computed.
struct State {
    seq: Vec<u32>,
    k: Vec<Vec<f32>>,
    v: Vec<Vec<f32>>,
    x: Vec<f32>,
    lse: Vec<Option<f64>>,
}

/// The network: log-probabilities of token sequences (`NativeCharLM`).
pub struct Model {
    d: usize,
    h: usize,
    eps: f64,
    pub max_len: usize,
    pub n_chars: usize,
    /// Output layer; its weights are also the input embedding.
    out: QLinear,
    pos: Vec<f32>,
    blocks: Vec<Block>,
    norm: (Vec<f32>, Vec<f32>),
    cache: Vec<State>,
    /// Testing (`trace`): the hidden vectors after each block.
    trace_layers: Option<Vec<Vec<f32>>>,
}

impl Model {
    pub fn load(path: &Path) -> io::Result<Model> {
        let raw = std::fs::read(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        let bad = |what: &str| invalid(format!("{}: {what}", path.display()));
        if raw.len() < 12 || &raw[..8] != MAGIC {
            return Err(bad("not a character model in the engine's format"));
        }
        let n = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
        let head: Value = raw
            .get(12..12 + n)
            .and_then(|h| serde_json::from_slice(h).ok())
            .ok_or_else(|| bad("unreadable header"))?;
        let start = (12 + n).div_ceil(ALIGN) * ALIGN;
        let int = |name: &str| head.get(name).and_then(Value::as_u64).map(|v| v as usize).ok_or_else(|| bad(name));
        let (d, h, n_layers, d_ff, n_chars, max_len) =
            (int("d_model")?, int("n_heads")?, int("n_layers")?, int("d_ff")?, int("n_chars")?, int("max_len")?);
        let eps = head.get("ln_eps").and_then(Value::as_f64).ok_or_else(|| bad("ln_eps"))?;
        if h == 0 || d % h != 0 {
            return Err(bad("d_model is not a multiple of n_heads"));
        }
        let mut tensors: HashMap<String, (&str, Vec<usize>, usize)> = HashMap::new();
        for e in head.get("tensors").and_then(Value::as_array).ok_or_else(|| bad("tensors"))? {
            let name = e.get("name").and_then(Value::as_str).ok_or_else(|| bad("tensor name"))?;
            let dtype = match e.get("dtype").and_then(Value::as_str) {
                Some("i8") => "i8",
                Some("f32") => "f32",
                _ => return Err(bad("tensor dtype")),
            };
            let shape: Vec<usize> = e
                .get("shape")
                .and_then(Value::as_array)
                .map(|s| s.iter().filter_map(|v| v.as_u64().map(|v| v as usize)).collect())
                .ok_or_else(|| bad("tensor shape"))?;
            let offset = e.get("offset").and_then(Value::as_u64).ok_or_else(|| bad("tensor offset"))? as usize;
            tensors.insert(name.to_owned(), (dtype, shape, offset));
        }
        let span = |name: &str, dtype: &str, shape: &[usize]| -> io::Result<&[u8]> {
            let (dt, sh, offset) = tensors.get(name).ok_or_else(|| bad(&format!("no tensor {name}")))?;
            if *dt != dtype || sh != shape {
                return Err(bad(&format!("tensor {name}: {dt} {sh:?}, expected {dtype} {shape:?}")));
            }
            let size = shape.iter().product::<usize>() * if dtype == "i8" { 1 } else { 4 };
            raw.get(start + offset..start + offset + size).ok_or_else(|| bad(&format!("tensor {name} is cut short")))
        };
        let i8s = |name: &str, shape: &[usize]| -> io::Result<Vec<i8>> {
            Ok(span(name, "i8", shape)?.iter().map(|&b| b as i8).collect())
        };
        let f32s = |name: &str, shape: &[usize]| -> io::Result<Vec<f32>> {
            Ok(span(name, "f32", shape)?.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
        };
        let linear = |name: &str, n_out: usize, n_in: usize| -> io::Result<QLinear> {
            Ok(QLinear {
                q: i8s(&format!("{name}.q"), &[n_out, n_in])?,
                scale: f32s(&format!("{name}.scale"), &[n_out])?,
                bias: f32s(&format!("{name}.b"), &[n_out])?,
                n_in,
                n_out,
            })
        };
        let norm = |name: &str| -> io::Result<(Vec<f32>, Vec<f32>)> {
            Ok((f32s(&format!("{name}.w"), &[d])?, f32s(&format!("{name}.b"), &[d])?))
        };
        let mut blocks = Vec::with_capacity(n_layers);
        for i in 0..n_layers {
            blocks.push(Block {
                norm1: norm(&format!("{i}.norm1"))?,
                norm2: norm(&format!("{i}.norm2"))?,
                qkv: linear(&format!("{i}.qkv"), 3 * d, d)?,
                proj: linear(&format!("{i}.proj"), d, d)?,
                ff1: linear(&format!("{i}.ff1"), d_ff, d)?,
                ff2: linear(&format!("{i}.ff2"), d, d_ff)?,
            });
        }
        let out = QLinear {
            q: i8s("emb.q", &[n_chars, d])?,
            scale: f32s("emb.scale", &[n_chars])?,
            bias: f32s("out_bias", &[n_chars])?,
            n_in: d,
            n_out: n_chars,
        };
        Ok(Model {
            d,
            h,
            eps,
            max_len,
            n_chars,
            out,
            pos: f32s("pos", &[max_len, d])?,
            blocks,
            norm: norm("norm")?,
            cache: Vec::new(),
            trace_layers: None,
        })
    }

    /// Forget the kept states (results do not depend on them).
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// The kept state sharing the longest prefix with `seq`, and that length.
    fn closest(&self, seq: &[u32]) -> (Option<usize>, usize) {
        let (mut best, mut n) = (None, 0);
        for (i, st) in self.cache.iter().enumerate() {
            let k = st.seq.iter().zip(seq).take_while(|(a, b)| a == b).count();
            if k > n {
                best = Some(i);
                n = k;
            }
        }
        (best, n)
    }

    /// Index in the cache of the state for every position of `seq`.
    fn extend(&mut self, seq: &[u32]) -> usize {
        let (old, n) = self.closest(seq);
        let n = n.min(seq.len());
        if let Some(i) = old {
            if n == seq.len() && self.cache[i].seq.len() == n {
                return i;
            }
        }
        let (d, h) = (self.d, self.h);
        let dh = d / h;
        let len = seq.len();
        let m = len - n;
        let mut x = Vec::with_capacity(m * d);
        for (i, &id) in seq[n..].iter().enumerate() {
            let id = id as usize;
            let scale = self.out.scale[id];
            let row = &self.out.q[id * d..(id + 1) * d];
            let pos = &self.pos[(n + i) * d..(n + i + 1) * d];
            x.extend(row.iter().zip(pos).map(|(&q, &p)| q as f32 * scale + p));
        }
        let mut ks = Vec::with_capacity(self.blocks.len());
        let mut vs = Vec::with_capacity(self.blocks.len());
        let (mut ln, mut qkv, mut tmp, mut ff) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut a = vec![0.0f32; m * d];
        let mut scores = vec![0.0f64; len];
        let mut e = vec![0.0f64; len];
        let mut acc = vec![0.0f64; dh];
        for (li, blk) in self.blocks.iter().enumerate() {
            layer_norm(&x, &blk.norm1.0, &blk.norm1.1, self.eps, &mut ln);
            blk.qkv.apply(&ln, &mut qkv);
            let mut k = Vec::with_capacity(len * d);
            let mut v = Vec::with_capacity(len * d);
            if let Some(o) = old {
                k.extend_from_slice(&self.cache[o].k[li][..n * d]);
                v.extend_from_slice(&self.cache[o].v[li][..n * d]);
            }
            for row in qkv.chunks_exact(3 * d) {
                k.extend_from_slice(&row[d..2 * d]);
                v.extend_from_slice(&row[2 * d..]);
            }
            for i in 0..m {
                let at = n + i; // sees keys 0 ..= at
                let q = &qkv[i * 3 * d..i * 3 * d + d];
                for hd in 0..h {
                    let sl = hd * dh..(hd + 1) * dh;
                    let qh = &q[sl.clone()];
                    let mut top = f64::NEG_INFINITY;
                    for j in 0..=at {
                        let kh = &k[j * d + hd * dh..j * d + (hd + 1) * dh];
                        let mut dot = 0.0f64;
                        for t in 0..dh {
                            dot += qh[t] as f64 * kh[t] as f64;
                        }
                        let s = ((dot as f32) as f64 / (dh as f64).sqrt()) as f32 as f64;
                        scores[j] = s;
                        top = top.max(s);
                    }
                    for j in 0..len {
                        e[j] = if j <= at { (scores[j] - top).exp() as f32 as f64 } else { 0.0 };
                    }
                    let total = pairwise_sum(&e);
                    let out = &mut a[i * d + hd * dh..i * d + (hd + 1) * dh];
                    acc.fill(0.0);
                    for j in 0..=at {
                        let p = (e[j] / total) as f32 as f64;
                        let vh = &v[j * d + hd * dh..j * d + (hd + 1) * dh];
                        for t in 0..acc.len() {
                            acc[t] += p * vh[t] as f64;
                        }
                    }
                    for t in 0..acc.len() {
                        out[t] = acc[t] as f32;
                    }
                }
            }
            blk.proj.apply(&a, &mut tmp);
            for (xv, &y) in x.iter_mut().zip(&tmp) {
                *xv += y;
            }
            layer_norm(&x, &blk.norm2.0, &blk.norm2.1, self.eps, &mut ln);
            blk.ff1.apply(&ln, &mut ff);
            gelu(&mut ff);
            blk.ff2.apply(&ff, &mut tmp);
            for (xv, &y) in x.iter_mut().zip(&tmp) {
                *xv += y;
            }
            ks.push(k);
            vs.push(v);
            if let Some(t) = self.trace_layers.as_mut() {
                t.push(x.clone());
            }
        }
        layer_norm(&x, &self.norm.0, &self.norm.1, self.eps, &mut ln);
        let (mut xs, mut lse) = (Vec::with_capacity(len * d), Vec::with_capacity(len));
        if let Some(o) = old {
            xs.extend_from_slice(&self.cache[o].x[..n * d]);
            lse.extend_from_slice(&self.cache[o].lse[..n]);
        }
        xs.extend_from_slice(&ln);
        lse.resize(len, None);
        self.cache.push(State { seq: seq.to_vec(), k: ks, v: vs, x: xs, lse });
        if self.cache.len() > PREFIX_CACHE {
            self.cache.remove(0);
        }
        self.cache.len() - 1
    }

    /// Sum of ln P(seq[i + 1] | seq[..= i]) for i = first .. len - 2.
    pub fn logp(&mut self, seq: &[u32], first: usize) -> f64 {
        let at = self.extend(seq);
        let end = seq.len().saturating_sub(1);
        let d = self.d;
        let (out, cache) = (&self.out, &mut self.cache);
        let st = &mut cache[at];
        let mut qx = vec![0i8; d];
        let mut logits = Vec::new();
        let mut exps = vec![0.0f64; out.n_out];
        let mut parts = Vec::with_capacity(end.saturating_sub(first));
        for i in first..end {
            let row = &st.x[i * d..(i + 1) * d];
            let target = seq[i + 1] as usize;
            let lse = match st.lse[i] {
                Some(v) => v,
                None => {
                    out.apply(row, &mut logits);
                    let top = logits.iter().fold(f64::NEG_INFINITY, |a, &l| a.max(l as f64));
                    for (e, &l) in exps.iter_mut().zip(&logits) {
                        *e = (l as f64 - top).exp();
                    }
                    let v = pairwise_sum(&exps).ln() + top;
                    st.lse[i] = Some(v);
                    v
                }
            };
            // One output of the same row: the same arithmetic as the full row.
            let sx = quantize(row, &mut qx) as f64;
            let acc = dot_plain(&qx, &out.q[target * d..(target + 1) * d]);
            let logit = (acc as f64 * (sx * out.scale[target] as f64) + out.bias[target] as f64) as f32;
            parts.push(logit as f64 - lse);
        }
        // Added in order, as the reference does.
        parts.iter().fold(0.0, |total, &p| total + p)
    }

    /// The hidden vectors of `seq` after the embedding and after each block,
    /// computed from scratch (the test vectors' "trace").
    pub fn trace(&mut self, seq: &[u32]) -> Vec<Vec<f32>> {
        self.clear_cache();
        let mut out = Vec::new();
        let d = self.d;
        let mut x: Vec<f32> = Vec::with_capacity(seq.len() * d);
        for (i, &id) in seq.iter().enumerate() {
            let id = id as usize;
            let scale = self.out.scale[id];
            let row = &self.out.q[id * d..(id + 1) * d];
            x.extend(row.iter().zip(&self.pos[i * d..(i + 1) * d]).map(|(&q, &p)| q as f32 * scale + p));
        }
        out.push(x);
        self.trace_layers = Some(Vec::new());
        self.extend(seq);
        out.extend(self.trace_layers.take().unwrap_or_default());
        self.clear_cache();
        out
    }
}

// -- the scorer -------------------------------------------------------------------

/// Python's `str.isdigit` (Unicode 15): Numeric_Type Decimal or Digit.
const DIGITS: &[(u32, u32)] = &[
    (0x30, 0x39), (0xB2, 0xB3), (0xB9, 0xB9), (0x660, 0x669), (0x6F0, 0x6F9), (0x7C0, 0x7C9), (0x966, 0x96F),
    (0x9E6, 0x9EF), (0xA66, 0xA6F), (0xAE6, 0xAEF), (0xB66, 0xB6F), (0xBE6, 0xBEF), (0xC66, 0xC6F), (0xCE6, 0xCEF),
    (0xD66, 0xD6F), (0xDE6, 0xDEF), (0xE50, 0xE59), (0xED0, 0xED9), (0xF20, 0xF29), (0x1040, 0x1049),
    (0x1090, 0x1099), (0x1369, 0x1371), (0x17E0, 0x17E9), (0x1810, 0x1819), (0x1946, 0x194F), (0x19D0, 0x19DA),
    (0x1A80, 0x1A89), (0x1A90, 0x1A99), (0x1B50, 0x1B59), (0x1BB0, 0x1BB9), (0x1C40, 0x1C49), (0x1C50, 0x1C59),
    (0x2070, 0x2070), (0x2074, 0x2079), (0x2080, 0x2089), (0x2460, 0x2468), (0x2474, 0x247C), (0x2488, 0x2490),
    (0x24EA, 0x24EA), (0x24F5, 0x24FD), (0x24FF, 0x24FF), (0x2776, 0x277E), (0x2780, 0x2788), (0x278A, 0x2792),
    (0xA620, 0xA629), (0xA8D0, 0xA8D9), (0xA900, 0xA909), (0xA9D0, 0xA9D9), (0xA9F0, 0xA9F9), (0xAA50, 0xAA59),
    (0xABF0, 0xABF9), (0xFF10, 0xFF19), (0x104A0, 0x104A9), (0x10A40, 0x10A43), (0x10D30, 0x10D39),
    (0x10E60, 0x10E68), (0x11052, 0x1105A), (0x11066, 0x1106F), (0x110F0, 0x110F9), (0x11136, 0x1113F),
    (0x111D0, 0x111D9), (0x112F0, 0x112F9), (0x11450, 0x11459), (0x114D0, 0x114D9), (0x11650, 0x11659),
    (0x116C0, 0x116C9), (0x11730, 0x11739), (0x118E0, 0x118E9), (0x11950, 0x11959), (0x11C50, 0x11C59),
    (0x11D50, 0x11D59), (0x11DA0, 0x11DA9), (0x11F50, 0x11F59), (0x16A60, 0x16A69), (0x16AC0, 0x16AC9),
    (0x16B50, 0x16B59), (0x1D7CE, 0x1D7FF), (0x1E140, 0x1E149), (0x1E2F0, 0x1E2F9), (0x1E4F0, 0x1E4F9),
    (0x1E950, 0x1E959), (0x1F100, 0x1F10A), (0x1FBF0, 0x1FBF9),
];

/// Python's `str.isspace`.
const SPACES: &[(u32, u32)] = &[
    (0x9, 0xD), (0x1C, 0x20), (0x85, 0x85), (0xA0, 0xA0), (0x1680, 0x1680), (0x2000, 0x200A), (0x2028, 0x2029),
    (0x202F, 0x202F), (0x205F, 0x205F), (0x3000, 0x3000),
];

fn in_ranges(ranges: &[(u32, u32)], ch: char) -> bool {
    let c = ch as u32;
    ranges.iter().any(|&(a, b)| a <= c && c <= b)
}

/// Re-ranking scores: ln P(text, and the end of the line after it | context)
/// (`CharLMScorer` in charlm.py).
pub struct CharLM {
    pub model: Model,
    char_id: HashMap<char, u32>,
    num: u32,
    eng: u32,
    unk_han: u32,
    other: u32,
    bos: u32,
    eos: bool,
    /// (context, text) -> (score, last use); `order` maps last use -> key.
    cache: HashMap<(String, String), (f64, u64)>,
    order: BTreeMap<u64, (String, String)>,
    clock: u64,
}

impl CharLM {
    /// `meta` is the model's `charlm.json` (character list, `bos`, `eos`).
    pub fn load(model: &Path, meta: &Path) -> io::Result<CharLM> {
        let model = Model::load(model)?;
        let text = std::fs::read_to_string(meta).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", meta.display())))?;
        let meta_v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| invalid(format!("{}: {e}", meta.display())))?;
        let bad = |what: &str| invalid(format!("{}: {what}", meta.display()));
        let chars = meta_v.get("chars").and_then(Value::as_array).ok_or_else(|| bad("no \"chars\""))?;
        if chars.len() != model.n_chars {
            return Err(bad("the character list does not match the model"));
        }
        let mut char_id = HashMap::new();
        let mut special = HashMap::new();
        for (i, c) in chars.iter().enumerate() {
            let s = c.as_str().ok_or_else(|| bad("a character is not text"))?;
            let mut it = s.chars();
            match (it.next(), it.next()) {
                (Some(ch), None) => {
                    char_id.entry(ch).or_insert(i as u32);
                }
                _ => {
                    special.insert(s.to_owned(), i as u32);
                }
            }
        }
        let sp = |name: &str| special.get(name).copied().ok_or_else(|| bad(&format!("no {name}")));
        let bos = meta_v.get("bos").and_then(Value::as_u64).ok_or_else(|| bad("no \"bos\""))? as u32;
        let eos = meta_v.get("eos").is_some_and(|v| v.as_bool().unwrap_or(!v.is_null()));
        if let Some(n) = meta_v.get("max_len").and_then(Value::as_u64) {
            if n as usize != model.max_len {
                return Err(bad("max_len does not match the model"));
            }
        }
        Ok(CharLM {
            num: sp("<n>")?,
            eng: sp("<e>")?,
            unk_han: sp("<x>")?,
            other: sp("<p>")?,
            model,
            char_id,
            bos,
            eos,
            cache: HashMap::new(),
            order: BTreeMap::new(),
            clock: 0,
        })
    }

    /// Same encoding as training/neural/make_data.py.
    pub fn encode(&self, text: &str, out: &mut Vec<u32>) {
        let mut run = None;
        for ch in text.chars() {
            let kind = if in_ranges(DIGITS, ch) {
                Some(self.num)
            } else if ch.is_ascii_alphabetic() {
                Some(self.eng)
            } else {
                None
            };
            if let Some(k) = kind {
                if run != kind {
                    out.push(k);
                }
                run = kind;
                continue;
            }
            run = None;
            if !in_ranges(SPACES, ch) {
                let fallback = if ('\u{4e00}'..='\u{9fff}').contains(&ch) { self.unk_han } else { self.other };
                out.push(self.char_id.get(&ch).copied().unwrap_or(fallback));
            }
        }
    }

    /// (sequence, first scored position) for one text after `context`.
    pub fn sequence(&self, context: &str, text: &str) -> (Vec<u32>, usize) {
        let mut seq = vec![self.bos];
        self.encode(&last_chars(context, MAX_CONTEXT), &mut seq);
        let mut body = Vec::new();
        self.encode(text, &mut body);
        if self.eos {
            body.push(self.bos);
        }
        seq.extend_from_slice(&body);
        if seq.len() > self.model.max_len {
            seq.drain(..seq.len() - self.model.max_len);
        }
        let first = seq.len().saturating_sub(body.len() + 1);
        (seq, first)
    }

    fn touch(&mut self, key: (String, String), score: f64) {
        self.clock += 1;
        if let Some((_, old)) = self.cache.insert(key.clone(), (score, self.clock)) {
            self.order.remove(&old);
        }
        self.order.insert(self.clock, key);
    }

    /// ln P(text, and the end of the line after it | context) for each text.
    pub fn score(&mut self, context: &str, texts: &[String]) -> Vec<f64> {
        let context = last_chars(context, MAX_CONTEXT);
        let mut missing: Vec<&String> = Vec::new();
        for t in texts {
            if !missing.contains(&t) && !self.cache.contains_key(&(context.clone(), t.clone())) {
                missing.push(t);
            }
        }
        for t in missing {
            let (seq, first) = self.sequence(&context, t);
            let score = self.model.logp(&seq, first);
            self.touch((context.clone(), t.clone()), score);
        }
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            let key = (context.clone(), t.clone());
            let score = self.cache[&key].0;
            self.touch(key, score);
            out.push(score);
        }
        while self.cache.len() > CACHE_SIZE {
            let (_, key) = self.order.pop_first().unwrap();
            self.cache.remove(&key);
        }
        out
    }
}

/// The last `max` characters of `text`.
fn last_chars(text: &str, max: usize) -> String {
    let n = text.chars().count();
    text.chars().skip(n.saturating_sub(max)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numpy_order() {
        let a: Vec<f64> = (0..300).map(|i| 1.0 / (i as f64 + 1.0)).collect();
        // Small sums are sequential.
        assert_eq!(pairwise_sum(&a[..5]), a[..5].iter().fold(0.0, |s, v| s + v));
        assert!((pairwise_sum(&a) - a.iter().sum::<f64>()).abs() < 1e-12);
        assert_eq!(round_half_away(2.5), 3.0);
        assert_eq!(round_half_away(-2.5), -3.0);
        assert_eq!(round_half_away(-0.4), 0.0);
    }

    #[test]
    fn dot_products_agree() {
        let qx: Vec<i8> = (0..400).map(|i| ((i * 37) % 255 - 127) as i8).collect();
        let w: Vec<i8> = (0..400 * 3).map(|i| ((i * 91) % 255 - 127) as i8).collect();
        for n_in in [16, 48, 384, 400] {
            let mut acc = vec![0; 3];
            dots(&qx[..n_in], &w[..3 * n_in], n_in, &mut acc);
            for j in 0..3 {
                assert_eq!(acc[j], dot_plain(&qx[..n_in], &w[j * n_in..(j + 1) * n_in]));
            }
        }
    }
}
