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

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;
use std::rc::Rc;

use serde_json::Value;

use crate::int8::{self, Weights};
use crate::pool::Pool;

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
///
/// Written so that the compiler can use vector instructions: every step
/// (max, division, floor, addition) is exact or correctly rounded, so the
/// vector code gives the scalar results; `quantize` picks AVX-512 when the
/// CPU has it.
#[inline(always)]
fn quantize_body(x: &[f32], q: &mut [i8]) -> f32 {
    let mut lanes = [0.0f32; 16];
    let mut chunks = x.chunks_exact(16);
    for c in &mut chunks {
        for (l, &v) in lanes.iter_mut().zip(c) {
            *l = l.max(v.abs());
        }
    }
    let m = chunks.remainder().iter().fold(lanes.iter().fold(0.0f32, |a, &v| a.max(v)), |a, v| a.max(v.abs()));
    let scale = (m as f64 / 127.0) as f32;
    let safe = if scale == 0.0 { 1.0 } else { scale as f64 };
    for (q, &v) in q.iter_mut().zip(x) {
        let v = v as f64 / safe;
        // round_half_away: 0.5 away from zero (the sign of a zero does not matter).
        let r = (v.abs() + 0.5).floor().copysign(v);
        *q = r.clamp(-127.0, 127.0) as i8;
    }
    scale
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
unsafe fn quantize_avx512(x: &[f32], q: &mut [i8]) -> f32 {
    quantize_body(x, q)
}

fn has_avx512() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *HAS.get_or_init(|| {
            std::arch::is_x86_feature_detected!("avx512f")
                && std::arch::is_x86_feature_detected!("avx512bw")
                && std::arch::is_x86_feature_detected!("avx512vl")
                && std::arch::is_x86_feature_detected!("avx512dq")
        })
    }
    #[cfg(not(target_arch = "x86_64"))]
    false
}

fn quantize(x: &[f32], q: &mut [i8]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    if has_avx512() {
        // SAFETY: the CPU has these features.
        return unsafe { quantize_avx512(x, q) };
    }
    quantize_body(x, q)
}

/// `out[j] = f32(acc[j] · (s · scale[j]) + bias[j])`, in f64 (each step
/// correctly rounded, so vector code gives the same).
#[inline(always)]
fn dequantize_body(acc: &[i32], s: f64, scale: &[f32], bias: &[f32], out: &mut [f32]) {
    for (((o, &a), &sc), &b) in out.iter_mut().zip(acc).zip(scale).zip(bias) {
        *o = (a as f64 * (s * sc as f64) + b as f64) as f32;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
unsafe fn dequantize_avx512(acc: &[i32], s: f64, scale: &[f32], bias: &[f32], out: &mut [f32]) {
    dequantize_body(acc, s, scale, bias, out)
}

fn dequantize(acc: &[i32], s: f64, scale: &[f32], bias: &[f32], out: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    if has_avx512() {
        // SAFETY: the CPU has these features.
        return unsafe { dequantize_avx512(acc, s, scale, bias, out) };
    }
    dequantize_body(acc, s, scale, bias, out)
}

// -- quantized linear layers ------------------------------------------------------

/// `y = W x + b` with W as 8-bit rows and a scale per row.
struct QLinear {
    w: Weights,
    scale: Vec<f32>,
    bias: Vec<f32>,
    n_in: usize,
    n_out: usize,
}

impl QLinear {
    /// Quantize every row of `x` (rows of `n_in`): the 8-bit rows into `qx`,
    /// one scale per row into `sx`.
    fn quantize_rows(&self, x: &[f32], qx: &mut Vec<i8>, sx: &mut Vec<f64>) {
        let m = x.len() / self.n_in;
        qx.resize(m * self.n_in, 0);
        sx.clear();
        for (xs, q) in x.chunks_exact(self.n_in).zip(qx.chunks_exact_mut(self.n_in)) {
            sx.push(quantize(xs, q) as f64);
        }
    }

    /// One output per row of `x` (rows of `n_in`), into `out`. All rows go
    /// through the integer product together: each weight row is read once.
    fn apply(&self, x: &[f32], out: &mut Vec<f32>) {
        self.apply_then(x, out, |_| {});
    }

    /// `apply`, then `then` on each row's outputs (in parts: it must work
    /// value by value, like GELU). With two threads each computes half of the
    /// outputs; every output is computed the same way either way.
    fn apply_then(&self, x: &[f32], out: &mut Vec<f32>, then: impl Fn(&mut [f32]) + Sync) {
        let mut scratch = Scratch::take();
        let Scratch { qx, sx, acc, .. } = &mut *scratch;
        self.quantize_rows(x, qx, sx);
        let (m, n) = (sx.len(), self.n_out);
        // Every value is written below; no need to clear.
        acc.resize(m * n, 0);
        out.resize(m * n, 0.0);
        let (qx, sx) = (&*qx, &*sx);
        let (acc_p, out_p) = (Shared(acc.as_mut_ptr()), Shared(out.as_mut_ptr()));
        let kernel = int8::best();
        let part = |cols: std::ops::Range<usize>| {
            let (acc_p, out_p) = (&acc_p, &out_p);
            // SAFETY: the parts write disjoint columns of buffers of m · n.
            unsafe {
                int8::matmul_raw(kernel, qx, m, &self.w, cols.clone(), acc_p.0);
                for (i, &s) in sx.iter().enumerate() {
                    let a = std::slice::from_raw_parts(acc_p.0.add(i * n + cols.start), cols.len());
                    let y = std::slice::from_raw_parts_mut(out_p.0.add(i * n + cols.start), cols.len());
                    dequantize(a, s, &self.scale[cols.clone()], &self.bias[cols.clone()], y);
                    then(y);
                }
            }
        };
        // Two blocks of 16 outputs at a time in the VNNI kernel: split at a multiple of 32.
        let mid = n / 2 / 32 * 32;
        match Pool::get() {
            Some(pool) if m * n * self.n_in >= PARALLEL_WORK && mid > 0 => pool.halves(mid, n, part),
            _ => part(0..n),
        }
    }

    /// ln Σ exp(y) of every output row (`top + ln Σ exp(y − top)`, the sum in
    /// numpy's pairwise order), for the output layer. With two threads each
    /// does half of the outputs: half the integer products, then the half of
    /// the sum that numpy's pairwise order adds up on its own (its first
    /// split), so the result is the same.
    fn log_sum_exp(&self, x: &[f32]) -> Vec<f64> {
        let mut scratch = Scratch::take();
        let Scratch { qx, sx, acc, logits } = &mut *scratch;
        self.quantize_rows(x, qx, sx);
        let (m, n) = (sx.len(), self.n_out);
        // Every value is written below; no need to clear.
        acc.resize(m * n, 0);
        logits.resize(m * n, 0.0);
        let (qx, sx) = (&*qx, &*sx);
        let (acc_p, logits_p) = (Shared(acc.as_mut_ptr()), Shared(logits.as_mut_ptr()));
        let logits = &*logits;
        let kernel = int8::best();
        // Outputs `cols` of every row; the largest of them per row.
        let part = |cols: std::ops::Range<usize>| -> Vec<f32> {
            let (acc_p, logits_p) = (&acc_p, &logits_p);
            let mut tops = vec![f32::NEG_INFINITY; m];
            // SAFETY: the two parts write disjoint columns of buffers of m · n.
            unsafe {
                int8::matmul_raw(kernel, qx, m, &self.w, cols.clone(), acc_p.0);
                for (i, top) in tops.iter_mut().enumerate() {
                    let a = std::slice::from_raw_parts(acc_p.0.add(i * n + cols.start), cols.len());
                    let y = std::slice::from_raw_parts_mut(logits_p.0.add(i * n + cols.start), cols.len());
                    dequantize(a, sx[i], &self.scale[cols.clone()], &self.bias[cols.clone()], y);
                    *top = y.iter().fold(f32::NEG_INFINITY, |t, &v| t.max(v));
                }
            }
            tops
        };
        // numpy's pairwise sum of more than 128 values adds its two halves
        // (split at a multiple of 8) on their own.
        let split = if n > 128 { n / 2 - n / 2 % 8 } else { n };
        let pool = if split < n { Pool::get() } else { None };
        let tops: Vec<f64> = match pool {
            Some(pool) => {
                let (mut left, mut right) = (Vec::new(), Vec::new());
                pool.join(|| left = part(0..split), || right = part(split..n));
                left.iter().zip(&right).map(|(&a, &b)| a.max(b) as f64).collect()
            }
            None => part(0..n).into_iter().map(f64::from).collect(),
        };
        let sum = |cols: std::ops::Range<usize>| -> Vec<f64> {
            let mut exps = vec![0.0f64; cols.len()];
            (0..m)
                .map(|i| {
                    for (e, &l) in exps.iter_mut().zip(&logits[i * n + cols.start..i * n + cols.end]) {
                        *e = (l as f64 - tops[i]).exp();
                    }
                    pairwise_sum(&exps)
                })
                .collect()
        };
        let sums: Vec<f64> = match pool {
            Some(pool) => {
                let (mut left, mut right) = (Vec::new(), Vec::new());
                pool.join(|| left = sum(0..split), || right = sum(split..n));
                left.iter().zip(&right).map(|(a, b)| a + b).collect()
            }
            None => sum(0..n),
        };
        sums.iter().zip(&tops).map(|(s, t)| s.ln() + t).collect()
    }
}

/// Working buffers of the linear layers, kept between calls (no allocation
/// on the hot path); one set per thread.
#[derive(Default)]
struct Scratch {
    qx: Vec<i8>,
    sx: Vec<f64>,
    acc: Vec<i32>,
    logits: Vec<f32>,
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Vec<Scratch>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A set of buffers on loan; handed back when dropped.
struct Loan(Option<Scratch>);

impl Scratch {
    fn take() -> Loan {
        Loan(Some(SCRATCH.with(|s| s.borrow_mut().pop()).unwrap_or_default()))
    }
}

impl std::ops::Deref for Loan {
    type Target = Scratch;
    fn deref(&self) -> &Scratch {
        self.0.as_ref().expect("on loan")
    }
}

impl std::ops::DerefMut for Loan {
    fn deref_mut(&mut self) -> &mut Scratch {
        self.0.as_mut().expect("on loan")
    }
}

impl Drop for Loan {
    fn drop(&mut self) {
        if let Some(s) = self.0.take() {
            // While the thread ends there is nowhere to keep them: they are freed.
            let _ = SCRATCH.try_with(|keep| keep.borrow_mut().push(s));
        }
    }
}

/// Products (inputs × outputs × rows) below which one thread does the work.
const PARALLEL_WORK: usize = 300_000;

/// A buffer two threads write disjoint parts of.
struct Shared<T>(*mut T);
// SAFETY: used only for disjoint writes while the owner waits (see log_sum_exp).
unsafe impl<T> Send for Shared<T> {}
unsafe impl<T> Sync for Shared<T> {}

struct Block {
    norm1: (Vec<f32>, Vec<f32>),
    norm2: (Vec<f32>, Vec<f32>),
    qkv: QLinear,
    proj: QLinear,
    ff1: QLinear,
    ff2: QLinear,
}

/// One position of a sequence: its keys and values in every layer, the final
/// normalized output, and the output's log-sum-exp once computed. It depends
/// only on the tokens up to it, so sequences with the same start share it.
struct Pos {
    /// Layer after layer, `d` each.
    k: Box<[f32]>,
    v: Box<[f32]>,
    x: Box<[f32]>,
    lse: Cell<Option<f64>>,
}

/// A sequence kept for its prefix: the reference keeps the last 32 sequences
/// and starts a new one from the closest; positions are shared, not copied.
struct State {
    seq: Vec<u32>,
    pos: Vec<Rc<Pos>>,
}

/// A position while a batch is computed: done before, or new row `r` of the batch.
#[derive(Clone)]
enum Slot {
    Done(Rc<Pos>),
    New(usize),
}

/// Where a position's keys and values are while a batch is computed: in a
/// kept position (`layers` · d values each), or new row `r` of the batch.
#[derive(Clone, Copy)]
enum KeyRef {
    Done(*const f32, *const f32),
    New(usize),
}

/// The positions of every planned sequence, readable from both threads.
struct KeyTable(Vec<Vec<KeyRef>>);
// SAFETY: read only, while the positions pointed to are kept alive by the
// batch's plans (see `positions`).
unsafe impl Sync for KeyTable {}

/// A kept sequence as the batch plan sees it: in the cache, or planned in this batch.
#[derive(Clone, Copy)]
enum Kept {
    Cache(usize),
    Plan(usize),
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
                w: Weights::new(i8s(&format!("{name}.q"), &[n_out, n_in])?, n_out, n_in),
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
            w: Weights::new(i8s("emb.q", &[n_chars, d])?, n_chars, d),
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

    fn embed(&self, id: u32, at: usize, x: &mut Vec<f32>) {
        let d = self.d;
        let id = id as usize;
        let scale = self.out.scale[id];
        let row = self.out.w.row(id);
        x.extend(row.iter().zip(&self.pos[at * d..(at + 1) * d]).map(|(&q, &p)| q as f32 * scale + p));
    }

    /// The positions of every sequence of `seqs`, computing what no kept
    /// sequence has, all sequences together layer by layer.
    ///
    /// The result is what computing the sequences one after another would
    /// give (the reference): each starts from the kept sequence sharing the
    /// longest prefix with it, among those kept before it, including the ones
    /// earlier in the batch, and is kept itself afterwards (the last 32).
    fn positions(&mut self, seqs: &[&[u32]]) -> Vec<Vec<Rc<Pos>>> {
        let (d, h) = (self.d, self.h);
        let dh = d / h;
        // -- plan: what each sequence starts from and which rows are new.
        let mut kept: Vec<Kept> = (0..self.cache.len()).map(Kept::Cache).collect();
        let mut plans: Vec<(usize, Vec<Slot>)> = Vec::new(); // (sequence, slots)
        let mut result: Vec<Kept> = Vec::with_capacity(seqs.len());
        let mut rows: Vec<(usize, usize)> = Vec::new(); // (plan, position) of each new row
        for (q, seq) in seqs.iter().enumerate() {
            let seq_of = |k: Kept| -> &[u32] {
                match k {
                    Kept::Cache(i) => &self.cache[i].seq,
                    Kept::Plan(j) => seqs[plans[j].0],
                }
            };
            let (mut best, mut n) = (None, 0);
            for &k in &kept {
                let shared = seq_of(k).iter().zip(seq.iter()).take_while(|(a, b)| a == b).count();
                if shared > n {
                    best = Some(k);
                    n = shared;
                }
            }
            let n = n.min(seq.len());
            if let Some(k) = best {
                if n == seq.len() && seq_of(k).len() == n {
                    result.push(k);
                    continue;
                }
            }
            let mut slots: Vec<Slot> = match best {
                Some(Kept::Cache(i)) => self.cache[i].pos[..n].iter().map(|p| Slot::Done(Rc::clone(p))).collect(),
                Some(Kept::Plan(j)) => plans[j].1[..n].to_vec(),
                None => Vec::new(),
            };
            let plan = plans.len();
            for at in n..seq.len() {
                slots.push(Slot::New(rows.len()));
                rows.push((plan, at));
            }
            plans.push((q, slots));
            kept.push(Kept::Plan(plan));
            if kept.len() > PREFIX_CACHE {
                kept.remove(0);
            }
            result.push(Kept::Plan(plan));
        }

        // -- compute the new rows, all sequences together.
        let m = rows.len();
        let mut x = Vec::with_capacity(m * d);
        for &(plan, at) in &rows {
            self.embed(seqs[plans[plan].0][at], at, &mut x);
        }
        let layers = self.blocks.len();
        let mut new_k = vec![0.0f32; m * layers * d];
        let mut new_v = vec![0.0f32; m * layers * d];
        let (mut ln, mut qkv, mut tmp, mut ff) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut a = vec![0.0f32; m * d];
        // Where each position's keys and values are, for the attention below,
        // which may run on two threads (an Rc cannot be shared between
        // threads; the plans keep every position alive meanwhile).
        let keys = KeyTable(
            plans
                .iter()
                .map(|(_, slots)| {
                    slots
                        .iter()
                        .map(|s| match s {
                            Slot::Done(p) => KeyRef::Done(p.k.as_ptr(), p.v.as_ptr()),
                            Slot::New(r) => KeyRef::New(*r),
                        })
                        .collect()
                })
                .collect(),
        );
        for (li, blk) in self.blocks.iter().enumerate() {
            layer_norm(&x, &blk.norm1.0, &blk.norm1.1, self.eps, &mut ln);
            blk.qkv.apply(&ln, &mut qkv);
            for (r, row) in qkv.chunks_exact(3 * d).enumerate() {
                let at = (r * layers + li) * d;
                new_k[at..at + d].copy_from_slice(&row[d..2 * d]);
                new_v[at..at + d].copy_from_slice(&row[2 * d..]);
            }
            let (new_k, new_v, qkv, keys, rows) = (&new_k, &new_v, &qkv, &keys, &rows);
            let a_p = Shared(a.as_mut_ptr());
            // Rows `range` of the batch; each row on its own, so the rows can be
            // split between threads.
            let attend = |range: std::ops::Range<usize>| {
                let a_p = &a_p;
                let kv = |key: &KeyRef| -> (*const f32, *const f32) {
                    match *key {
                        // SAFETY: k and v hold `layers` · d values (see Pos).
                        KeyRef::Done(k, v) => unsafe { (k.add(li * d), v.add(li * d)) },
                        KeyRef::New(r) => {
                            let at = (r * layers + li) * d;
                            (new_k[at..at + d].as_ptr(), new_v[at..at + d].as_ptr())
                        }
                    }
                };
                let longest = range.clone().map(|r| keys.0[rows[r].0].len()).max().unwrap_or(0);
                let (mut scores, mut e, mut acc) = (vec![0.0f64; longest], vec![0.0f64; longest], vec![0.0f64; dh]);
                for r in range {
                    let (plan, at) = rows[r];
                    // Row `at` of its sequence sees positions 0 ..= at; the softmax
                    // sums over the whole sequence (zeros after `at`), as the reference.
                    let slots = &keys.0[plan];
                    let len = slots.len();
                    let q = &qkv[r * 3 * d..r * 3 * d + d];
                    // SAFETY: each row writes only its own d values of `a`.
                    let out_row = unsafe { std::slice::from_raw_parts_mut(a_p.0.add(r * d), d) };
                    for hd in 0..h {
                        let qh = &q[hd * dh..(hd + 1) * dh];
                        let mut top = f64::NEG_INFINITY;
                        for (j, slot) in slots[..=at].iter().enumerate() {
                            // SAFETY: dh values inside the slot's d.
                            let kh = unsafe { std::slice::from_raw_parts(kv(slot).0.add(hd * dh), dh) };
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
                        let total = pairwise_sum(&e[..len]);
                        acc.fill(0.0);
                        for (j, slot) in slots[..=at].iter().enumerate() {
                            let p = (e[j] / total) as f32 as f64;
                            // SAFETY: as above.
                            let vh = unsafe { std::slice::from_raw_parts(kv(slot).1.add(hd * dh), dh) };
                            for t in 0..dh {
                                acc[t] += p * vh[t] as f64;
                            }
                        }
                        for t in 0..dh {
                            out_row[hd * dh + t] = acc[t] as f32;
                        }
                    }
                }
            };
            match Pool::get() {
                Some(pool) if m >= 2 => pool.halves(m / 2, m, attend),
                _ => attend(0..m),
            }
            blk.proj.apply(&a, &mut tmp);
            for (xv, &y) in x.iter_mut().zip(&tmp) {
                *xv += y;
            }
            layer_norm(&x, &blk.norm2.0, &blk.norm2.1, self.eps, &mut ln);
            blk.ff1.apply_then(&ln, &mut ff, gelu);
            blk.ff2.apply(&ff, &mut tmp);
            for (xv, &y) in x.iter_mut().zip(&tmp) {
                *xv += y;
            }
            if let Some(t) = self.trace_layers.as_mut() {
                t.push(x.clone());
            }
        }
        layer_norm(&x, &self.norm.0, &self.norm.1, self.eps, &mut ln);
        let done: Vec<Rc<Pos>> = (0..m)
            .map(|r| {
                let span = r * layers * d..(r + 1) * layers * d;
                Rc::new(Pos {
                    k: new_k[span.clone()].into(),
                    v: new_v[span].into(),
                    x: ln[r * d..(r + 1) * d].into(),
                    lse: Cell::new(None),
                })
            })
            .collect();
        let finish = |slots: &[Slot]| -> Vec<Rc<Pos>> {
            slots
                .iter()
                .map(|s| match s {
                    Slot::Done(p) => Rc::clone(p),
                    Slot::New(r) => Rc::clone(&done[*r]),
                })
                .collect()
        };
        let planned: Vec<Vec<Rc<Pos>>> = plans.iter().map(|(_, slots)| finish(slots)).collect();
        let out = result
            .iter()
            .map(|&k| match k {
                Kept::Cache(i) => self.cache[i].pos.clone(),
                Kept::Plan(j) => planned[j].clone(),
            })
            .collect();
        // Keep the new sequences, as the reference does after each.
        for ((q, _), pos) in plans.iter().zip(planned) {
            self.cache.push(State { seq: seqs[*q].to_vec(), pos });
            if self.cache.len() > PREFIX_CACHE {
                self.cache.remove(0);
            }
        }
        out
    }

    /// Sum of ln P(seq[i + 1] | seq[..= i]) for i = first .. len - 2, for
    /// each (seq, first), computed together.
    pub fn logp_batch(&mut self, items: &[(&[u32], usize)]) -> Vec<f64> {
        let seqs: Vec<&[u32]> = items.iter().map(|(s, _)| *s).collect();
        let all = self.positions(&seqs);
        let d = self.d;
        let out = &self.out;
        // The output layer for every position whose log-sum-exp is not known
        // yet, all at once; a position shared by several sequences once.
        let mut todo: Vec<Rc<Pos>> = Vec::new();
        for ((seq, first), pos) in items.iter().zip(&all) {
            for p in &pos[*first..seq.len().saturating_sub(1)] {
                if p.lse.get().is_none() && !todo.iter().any(|t| Rc::ptr_eq(t, p)) {
                    todo.push(Rc::clone(p));
                }
            }
        }
        if !todo.is_empty() {
            let rows: Vec<f32> = todo.iter().flat_map(|p| p.x.iter().copied()).collect();
            for (p, lse) in todo.iter().zip(out.log_sum_exp(&rows)) {
                p.lse.set(Some(lse));
            }
        }
        let mut qx = vec![0i8; d];
        items
            .iter()
            .zip(&all)
            .map(|((seq, first), pos)| {
                let mut total = 0.0;
                for i in *first..seq.len().saturating_sub(1) {
                    let target = seq[i + 1] as usize;
                    // One output of the same row: the same arithmetic as the full row.
                    let sx = quantize(&pos[i].x, &mut qx) as f64;
                    let acc = int8::dot_plain(&qx, out.w.row(target));
                    let logit = (acc as f64 * (sx * out.scale[target] as f64) + out.bias[target] as f64) as f32;
                    // Added in order, as the reference does.
                    total += logit as f64 - pos[i].lse.get().unwrap_or(f64::NAN);
                }
                total
            })
            .collect()
    }

    /// Sum of ln P(seq[i + 1] | seq[..= i]) for i = first .. len - 2.
    pub fn logp(&mut self, seq: &[u32], first: usize) -> f64 {
        self.logp_batch(&[(seq, first)])[0]
    }

    /// The hidden vectors of `seq` after the embedding and after each block,
    /// computed from scratch (the test vectors' "trace").
    pub fn trace(&mut self, seq: &[u32]) -> Vec<Vec<f32>> {
        self.clear_cache();
        let mut x = Vec::with_capacity(seq.len() * self.d);
        for (at, &id) in seq.iter().enumerate() {
            self.embed(id, at, &mut x);
        }
        let mut out = vec![x];
        self.trace_layers = Some(Vec::new());
        self.positions(&[seq]);
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
        let mut lm = CharLM {
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
        };
        lm.warm_up();
        Ok(lm)
    }

    /// One made-up re-ranking right after loading, so that the first real key
    /// does not pay for page faults, cold caches and starting the helper
    /// thread. What it leaves in the caches is forgotten: results stay
    /// exactly as without it.
    fn warm_up(&mut self) {
        let texts: Vec<String> = ["我在家", "我再加", "臥在家裡。"].iter().map(|s| s.to_string()).collect();
        self.score("今天", &texts);
        self.cache.clear();
        self.order.clear();
        self.model.clear_cache();
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
        let built: Vec<(Vec<u32>, usize)> = missing.iter().map(|t| self.sequence(&context, t)).collect();
        let items: Vec<(&[u32], usize)> = built.iter().map(|(s, f)| (s.as_slice(), *f)).collect();
        let scores = if items.is_empty() { Vec::new() } else { self.model.logp_batch(&items) };
        for (t, score) in missing.into_iter().zip(scores) {
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
    }

    /// The reference's quantization, step by step (charlm_native.py).
    fn quantize_reference(x: &[f32]) -> (f32, Vec<i8>) {
        let m = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let scale = (m as f64 / 127.0) as f32;
        let safe = if scale == 0.0 { 1.0 } else { scale as f64 };
        let round_half_away = |v: f64| if v == 0.0 { 0.0 } else { v.signum() * (v.abs() + 0.5).floor() };
        (scale, x.iter().map(|&v| round_half_away(v as f64 / safe).clamp(-127.0, 127.0) as i8).collect())
    }

    #[test]
    fn quantization_matches_the_reference() {
        let mut seed = 7u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
        };
        let mut rows: Vec<Vec<f32>> = vec![vec![0.0; 40], vec![-0.0, 0.0, 1.0, -1.0, 0.5], vec![2.5, -2.5, 127.0, 0.25, -0.75, 63.5]];
        for len in [1, 15, 16, 17, 384, 1536] {
            rows.push((0..len).map(|_| next() * 10.0).collect());
        }
        for x in rows {
            let (want_s, want_q) = quantize_reference(&x);
            let mut q = vec![0i8; x.len()];
            assert_eq!(quantize_body(&x, &mut q).to_bits(), want_s.to_bits());
            assert_eq!(q, want_q);
            let mut q = vec![0i8; x.len()];
            assert_eq!(quantize(&x, &mut q).to_bits(), want_s.to_bits());
            assert_eq!(q, want_q);
        }
    }
}
