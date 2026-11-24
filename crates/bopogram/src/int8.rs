//! Exact 8-bit integer matrix products for the character model
//! (`prototype/docs/charlm-optimization.md`, section 3).
//!
//! `out[i][j] = Σ_t x[i][t] · w[j][t]` with x and w signed 8-bit and the sum
//! in i32: exact, so the order of additions is free and every kernel gives the
//! same integers. Kernels, chosen once from what the CPU has:
//!
//! - AVX-512 VNNI (`vpdpbusd`, not the saturating `vpdpbusds`): one side must
//!   be unsigned, so x is shifted to `u = x + 128` (0..=254) and
//!   `128 · Σ w[j]` (kept per row) is taken off again.
//! - AVX2: both sides sign-extended to i16 and multiplied with `vpmaddwd`
//!   (`vpmaddubsw` would saturate its i16 sums).
//! - scalar, everywhere else.

use std::ops::Range;
use std::sync::OnceLock;

/// A weight matrix: `n` rows of `k`.
pub struct Weights {
    pub q: Vec<i8>,
    pub k: usize,
    pub n: usize,
    /// For the VNNI kernel, when the CPU has it: rearranged at load.
    packed: Option<Packed>,
}

/// The weights in the order the VNNI kernel reads them: blocks of 16 output
/// rows; in each, groups of 4 inputs, the 4 bytes of each of the 16 rows one
/// after another (64 bytes: one vector). Rows and inputs past the end are 0.
struct Packed {
    data: Vec<i8>,
    /// Groups of 4 inputs (k rounded up).
    k4: usize,
    /// 128 · Σ w[j] per row (rounded up to whole blocks), taken off again
    /// after the products with `x + 128`.
    corr: Vec<i32>,
}

impl Packed {
    fn new(q: &[i8], n: usize, k: usize) -> Packed {
        let k4 = k.div_ceil(4);
        let blocks = n.div_ceil(16);
        let mut data = vec![0i8; blocks * k4 * 64];
        let mut corr = vec![0i32; blocks * 16];
        for j in 0..n {
            let (jb, jj) = (j / 16, j % 16);
            let row = &q[j * k..(j + 1) * k];
            corr[j] = 128 * row.iter().map(|&v| v as i32).sum::<i32>();
            for (t, &v) in row.iter().enumerate() {
                data[(jb * k4 + t / 4) * 64 + jj * 4 + t % 4] = v;
            }
        }
        Packed { data, k4, corr }
    }
}

impl Weights {
    pub fn new(q: Vec<i8>, n: usize, k: usize) -> Weights {
        assert_eq!(q.len(), n * k);
        let packed = available().contains(&Kernel::Avx512Vnni).then(|| Packed::new(&q, n, k));
        Weights { q, k, n, packed }
    }

    pub fn row(&self, j: usize) -> &[i8] {
        &self.q[j * self.k..(j + 1) * self.k]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kernel {
    Scalar,
    Avx2,
    Avx512Vnni,
}

/// The kernels this CPU can run, fastest last.
pub fn available() -> Vec<Kernel> {
    let mut out = vec![Kernel::Scalar];
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            out.push(Kernel::Avx2);
        }
        if std::arch::is_x86_feature_detected!("avx512f")
            && std::arch::is_x86_feature_detected!("avx512bw")
            && std::arch::is_x86_feature_detected!("avx512vnni")
        {
            out.push(Kernel::Avx512Vnni);
        }
    }
    out
}

/// The fastest kernel this CPU has.
pub fn best() -> Kernel {
    static BEST: OnceLock<Kernel> = OnceLock::new();
    *BEST.get_or_init(|| *available().last().unwrap())
}

#[inline(always)]
pub fn dot_plain(a: &[i8], b: &[i8]) -> i32 {
    a.iter().zip(b).map(|(&x, &y)| x as i32 * y as i32).sum()
}

/// `out[i * w.n + j]` for the `m` rows of `x` and the rows `cols` of `w`;
/// other columns of `out` are left alone.
pub fn matmul(kernel: Kernel, x: &[i8], m: usize, w: &Weights, cols: Range<usize>, out: &mut [i32]) {
    assert!(x.len() >= m * w.k && out.len() >= m * w.n && cols.end <= w.n);
    // SAFETY: the slices are large enough (checked above) and the kernel was
    // chosen from what this CPU reports.
    unsafe { matmul_raw(kernel, x, m, w, cols, out.as_mut_ptr()) }
}

/// # Safety
/// `out` must hold `m * w.n` values; `x` at least `m * w.k`; `cols` within `w.n`;
/// `kernel` supported by this CPU. Writes only the columns `cols`.
pub unsafe fn matmul_raw(kernel: Kernel, x: &[i8], m: usize, w: &Weights, cols: Range<usize>, out: *mut i32) {
    match kernel {
        #[cfg(target_arch = "x86_64")]
        Kernel::Avx512Vnni => vnni::matmul(x, m, w, cols, out),
        #[cfg(target_arch = "x86_64")]
        Kernel::Avx2 => avx2::matmul(x, m, w, cols, out),
        _ => {
            let k = w.k;
            for j in cols {
                let row = w.row(j);
                for i in 0..m {
                    *out.add(i * w.n + j) = dot_plain(&x[i * k..(i + 1) * k], row);
                }
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod avx2 {
    use super::{dot_plain, Weights};
    use std::arch::x86_64::*;
    use std::ops::Range;

    #[target_feature(enable = "avx2")]
    unsafe fn hsum(s: __m256i) -> i32 {
        let half = _mm_add_epi32(_mm256_castsi256_si128(s), _mm256_extracti128_si256(s, 1));
        let half = _mm_add_epi32(half, _mm_shuffle_epi32(half, 0b01_00_11_10));
        let half = _mm_add_epi32(half, _mm_shuffle_epi32(half, 0b10_11_00_01));
        _mm_cvtsi128_si32(half)
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn matmul(x: &[i8], m: usize, w: &Weights, cols: Range<usize>, out: *mut i32) {
        let k = w.k;
        let blocks = k / 16;
        let tail = blocks * 16;
        for j in cols {
            let row = w.q.as_ptr().add(j * k);
            for i in 0..m {
                let a = x.as_ptr().add(i * k);
                let mut s0 = _mm256_setzero_si256();
                let mut s1 = _mm256_setzero_si256();
                let mut b = 0;
                while b + 1 < blocks {
                    let a0 = _mm256_cvtepi8_epi16(_mm_loadu_si128(a.add(b * 16) as *const __m128i));
                    let w0 = _mm256_cvtepi8_epi16(_mm_loadu_si128(row.add(b * 16) as *const __m128i));
                    let a1 = _mm256_cvtepi8_epi16(_mm_loadu_si128(a.add(b * 16 + 16) as *const __m128i));
                    let w1 = _mm256_cvtepi8_epi16(_mm_loadu_si128(row.add(b * 16 + 16) as *const __m128i));
                    s0 = _mm256_add_epi32(s0, _mm256_madd_epi16(a0, w0));
                    s1 = _mm256_add_epi32(s1, _mm256_madd_epi16(a1, w1));
                    b += 2;
                }
                if b < blocks {
                    let a0 = _mm256_cvtepi8_epi16(_mm_loadu_si128(a.add(b * 16) as *const __m128i));
                    let w0 = _mm256_cvtepi8_epi16(_mm_loadu_si128(row.add(b * 16) as *const __m128i));
                    s0 = _mm256_add_epi32(s0, _mm256_madd_epi16(a0, w0));
                }
                let rest = dot_plain(&x[i * k + tail..(i + 1) * k], &w.row(j)[tail..]);
                *out.add(i * w.n + j) = hsum(_mm256_add_epi32(s0, s1)) + rest;
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod vnni {
    use super::Weights;
    use std::arch::x86_64::*;
    use std::ops::Range;

    /// `R` input rows (starting at row `i`) against `B` blocks of 16 output
    /// rows (block `jb` on): each weight vector serves `R` rows, each input
    /// group `B` blocks.
    #[inline]
    #[target_feature(enable = "avx512f,avx512bw,avx512vnni")]
    unsafe fn block<const R: usize, const B: usize>(
        u: *const u8,
        kp: usize,
        i: usize,
        p: &super::Packed,
        jb: usize,
    ) -> [[__m512i; B]; R] {
        let k4 = p.k4;
        let w: [*const i8; B] = std::array::from_fn(|b| p.data.as_ptr().add((jb + b) * k4 * 64));
        let mut acc = [[_mm512_setzero_si512(); B]; R];
        for g in 0..k4 {
            let wv: [__m512i; B] = std::array::from_fn(|b| _mm512_loadu_si512(w[b].add(g * 64) as *const __m512i));
            for (r, acc) in acc.iter_mut().enumerate() {
                let a = _mm512_set1_epi32((u.add((i + r) * kp + g * 4) as *const i32).read_unaligned());
                for b in 0..B {
                    acc[b] = _mm512_dpbusd_epi32(acc[b], a, wv[b]);
                }
            }
        }
        for b in 0..B {
            let corr = _mm512_loadu_si512(p.corr.as_ptr().add((jb + b) * 16) as *const __m512i);
            for acc in acc.iter_mut() {
                acc[b] = _mm512_sub_epi32(acc[b], corr);
            }
        }
        acc
    }

    /// All rows against blocks `jb .. jb + B`.
    #[inline]
    #[target_feature(enable = "avx512f,avx512bw,avx512vnni")]
    unsafe fn rows<const B: usize>(u: *const u8, kp: usize, m: usize, p: &super::Packed, jb: usize, store: &mut impl FnMut(usize, usize, __m512i)) {
        let mut i = 0;
        let mut put = |i: usize, acc: &[[__m512i; B]]| {
            for (r, acc) in acc.iter().enumerate() {
                for (b, &v) in acc.iter().enumerate() {
                    store(i + r, jb + b, v);
                }
            }
        };
        while i + 4 <= m {
            put(i, &block::<4, B>(u, kp, i, p, jb));
            i += 4;
        }
        match m - i {
            3 => put(i, &block::<3, B>(u, kp, i, p, jb)),
            2 => put(i, &block::<2, B>(u, kp, i, p, jb)),
            1 => put(i, &block::<1, B>(u, kp, i, p, jb)),
            _ => {}
        }
    }

    #[target_feature(enable = "avx512f,avx512bw,avx512vnni")]
    pub unsafe fn matmul(x: &[i8], m: usize, w: &Weights, cols: Range<usize>, out: *mut i32) {
        let p = w.packed.as_ref().expect("packed weights");
        let (k, k4) = (w.k, p.k4);
        let kp = k4 * 4;
        // u = x + 128, as unsigned bytes (x is never -128, so u fits 0..=254);
        // the padding meets zero weights.
        let mut u = vec![128u8; m * kp];
        for i in 0..m {
            for (u, &v) in u[i * kp..i * kp + k].iter_mut().zip(&x[i * k..(i + 1) * k]) {
                *u = (v as i16 + 128) as u8;
            }
        }
        let u = u.as_ptr();
        let mut tmp = [0i32; 16];
        let mut store = |i: usize, jb: usize, v: __m512i| {
            _mm512_storeu_si512(tmp.as_mut_ptr() as *mut __m512i, v);
            for (jj, &value) in tmp.iter().enumerate() {
                let j = jb * 16 + jj;
                if cols.contains(&j) {
                    *out.add(i * w.n + j) = value;
                }
            }
        };
        if cols.is_empty() {
            return;
        }
        let (first, end) = (cols.start / 16, cols.end.div_ceil(16));
        let mut jb = first;
        while jb + 2 <= end {
            rows::<2>(u, kp, m, p, jb, &mut store);
            jb += 2;
        }
        if jb < end {
            rows::<1>(u, kp, m, p, jb, &mut store);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kernel this CPU has gives the scalar kernel's integers, for
    /// sizes with and without a remainder, including the extreme values.
    #[test]
    fn kernels_agree() {
        let mut seed = 12345u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) % 255) as i32 - 127
        };
        for (m, n, k) in [
            (1, 7, 16),
            (3, 9, 64),
            (5, 13, 384),
            (2, 6, 400),
            (4, 5, 1536),
            (1, 3, 70),
            (7, 40, 384),
            (6, 33, 68),
            (9, 64, 128),
            (2, 49, 1536),
        ] {
            let mut x: Vec<i8> = (0..m * k).map(|_| next() as i8).collect();
            let mut q: Vec<i8> = (0..n * k).map(|_| next() as i8).collect();
            x[0] = 127;
            q[0] = -127;
            x[m * k - 1] = -127;
            q[n * k - 1] = 127;
            let w = Weights::new(q, n, k);
            let mut want = vec![0; m * n];
            matmul(Kernel::Scalar, &x, m, &w, 0..n, &mut want);
            for kernel in available() {
                let mut got = vec![0; m * n];
                matmul(kernel, &x, m, &w, 0..n, &mut got);
                assert_eq!(got, want, "{kernel:?} m {m} n {n} k {k}");
                // A part of the columns only.
                let mut part = vec![i32::MIN; m * n];
                matmul(kernel, &x, m, &w, 1..n, &mut part);
                for i in 0..m {
                    assert_eq!(part[i * n], i32::MIN);
                    assert_eq!(&part[i * n + 1..(i + 1) * n], &want[i * n + 1..(i + 1) * n], "{kernel:?} part");
                }
            }
        }
    }
}
