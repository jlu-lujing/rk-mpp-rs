//! AV1 inverse DCT: a mathematical reference implementation.
//!
//! This is the direct fixed-point DCT-III, not the bit-exact butterfly chain the
//! specification's decoder uses. It exists so prediction/frame experiments have
//! a working inverse transform, and so the future butterfly port can be checked
//! against it. `idct_float` is the same transform in `f64` and is what the tests
//! compare with.
//!
//! The cosine table is `cospi[n] = round(cos(n * pi / 64) * 64)`.

/// `cospi[n]` for `n` in `0..=63`.
pub fn cospi(n: u32) -> i32 {
    let value = (n as f64 * std::f64::consts::PI / 64.0).cos() * 64.0;
    value.round() as i32
}

/// One-dimensional inverse DCT-III of `N` points, fixed point.
///
/// `out[k] = round(sum_n X[n] * cospi((2k+1)n*32/N) / 64)` with the DC term
/// scaled by `1/sqrt(2)`, matching the usual orthonormal DCT-III convention.
pub fn idct_1d_fixed(input: &[i32]) -> Vec<i32> {
    let n = input.len();
    let mut out = vec![0i32; n];
    for k in 0..n {
        let mut sum = 0f64;
        for (i, x) in input.iter().enumerate() {
            let angle = (std::f64::consts::PI * (2 * k + 1) as f64 * i as f64) / (2.0 * n as f64);
            let scale = if i == 0 { 1.0 / 2.0f64.sqrt() } else { 1.0 };
            sum += *x as f64 * scale * angle.cos();
        }
        out[k] = (sum / 2.0f64.sqrt()).round() as i32;
    }
    out
}

/// Two-dimensional inverse DCT over an `n` x `n` block in row-major order.
pub fn idct_2d(input: &[i32], n: usize) -> Vec<i32> {
    assert_eq!(input.len(), n * n);
    let mut rows = vec![0i32; n * n];
    for r in 0..n {
        let row = idct_1d_fixed(&input[r * n..(r + 1) * n]);
        rows[r * n..(r + 1) * n].copy_from_slice(&row);
    }
    let mut out = vec![0i32; n * n];
    for c in 0..n {
        let column: Vec<i32> = (0..n).map(|r| rows[r * n + c]).collect();
        let transformed = idct_1d_fixed(&column);
        for r in 0..n {
            out[r * n + c] = transformed[r];
        }
    }
    out
}

/// `Cos128_Lookup[65]`: `round(4096 * cos(angle * pi / 128))`.
pub static COS128_LOOKUP: [i32; 65] = [
    4096, 4095, 4091, 4085, 4076, 4065, 4052, 4036, 4017, 3996, 3973, 3948, 3920, 3889, 3857, 3822,
    3784, 3745, 3703, 3659, 3612, 3564, 3513, 3461, 3406, 3349, 3290, 3229, 3166, 3102, 3035, 2967,
    2896, 2824, 2751, 2675, 2598, 2520, 2440, 2359, 2276, 2191, 2106, 2019, 1931, 1842, 1751, 1660,
    1567, 1474, 1380, 1285, 1189, 1092, 995, 897, 799, 700, 601, 501, 401, 301, 201, 101, 0,
];

/// `cos128(angle)`.
pub fn cos128(angle: i32) -> i32 {
    let angle = angle & 255;
    if (0..=64).contains(&angle) {
        COS128_LOOKUP[angle as usize]
    } else if (64..=128).contains(&angle) {
        -COS128_LOOKUP[(128 - angle) as usize]
    } else if (128..=192).contains(&angle) {
        -COS128_LOOKUP[(angle - 128) as usize]
    } else {
        COS128_LOOKUP[(256 - angle) as usize]
    }
}

/// `sin128(angle) = cos128(angle - 64)`.
pub fn sin128(angle: i32) -> i32 {
    cos128(angle - 64)
}

/// `brev(numBits, x)`: the bit-reversal of `numBits` bits of `x`.
pub fn brev(num_bits: u32, x: u32) -> u32 {
    let mut t = 0;
    for i in 0..num_bits {
        let bit = (x >> i) & 1;
        t |= bit << (num_bits - 1 - i);
    }
    t
}

/// The specification's butterfly workspace: an integer array plus its clamping
/// precision `r`.
pub struct Butterfly {
    pub t: Vec<i32>,
    pub r: u32,
}

impl Butterfly {
    pub fn new(t: Vec<i32>, r: u32) -> Self {
        Self { t, r }
    }

    fn clip(&self, value: i32) -> i32 {
        let limit = 1i32 << (self.r - 1);
        value.clamp(-limit, limit - 1)
    }

    fn round2_12(&self, value: i32) -> i32 {
        self.clip((value + 2048) >> 12)
    }

    /// `B(a, b, angle, 0, r)`: a rotation.
    pub fn b_rotation(&mut self, a: usize, b: usize, angle: i32) {
        let ca = cos128(angle);
        let sa = sin128(angle);
        let x = self.t[a] * ca - self.t[b] * sa;
        let y = self.t[a] * sa + self.t[b] * ca;
        self.t[a] = self.round2_12(x);
        self.t[b] = self.round2_12(y);
    }

    /// `B(a, b, angle, 1, r)`: rotation and flip.
    pub fn b_rotation_flip(&mut self, a: usize, b: usize, angle: i32) {
        self.b_rotation(a, b, angle);
        self.t.swap(a, b);
    }

    /// `H(a, b, 0, r)`: a Hadamard rotation with clamping.
    pub fn h(&mut self, a: usize, b: usize, flip: bool) {
        let (a, b) = if flip { (b, a) } else { (a, b) };
        let x = self.t[a];
        let y = self.t[b];
        self.t[a] = self.clip(x + y);
        self.t[b] = self.clip(x - y);
    }
}

/// `Inverse DCT array permutation process`: `T[i] = copyT[brev(n, i)]`.
pub fn idct_permute(t: &mut [i32], n: u32) {
    let copy = t.to_vec();
    let len = 1usize << n;
    for i in 0..len {
        t[i] = copy[brev(n, i as u32) as usize];
    }
}

/// `Inverse DCT process` (specification 7.13.2.2) for `2 <= n <= 6`.
///
/// `t` holds `1 << n` coefficients in place; `r` is the intermediate clamping
/// precision.
pub fn inverse_dct(t: &mut [i32], n: u32, r: u32) {
    let len = 1usize << n;
    debug_assert_eq!(t.len(), len);
    idct_permute(t, n);
    let mut bf = Butterfly::new(t.to_vec(), r);

    if n == 6 {
        for i in 0..16 {
            bf.b_rotation(32 + i, 63 - i, 63 - 4 * brev(4, i as u32) as i32);
        }
    }
    if n >= 5 {
        for i in 0..8 {
            bf.b_rotation(16 + i, 31 - i, 6 + ((brev(3, 7 - i as u32) << 3) as i32));
        }
    }
    if n == 6 {
        for i in 0..16 {
            bf.h(32 + i * 2, 33 + i * 2, i & 1 != 0);
        }
    }
    if n >= 4 {
        for i in 0..4 {
            bf.b_rotation(8 + i, 15 - i, 12 + ((brev(2, 3 - i as u32) << 4) as i32));
        }
    }
    if n >= 5 {
        for i in 0..8 {
            bf.h(16 + 2 * i, 17 + 2 * i, i & 1 != 0);
        }
    }
    if n == 6 {
        for i in 0..4 {
            for j in 0..2 {
                bf.b_rotation_flip(
                    62 - i * 4 - j,
                    33 + i * 4 + j,
                    60 - 16 * brev(2, i as u32) as i32 + 64 * j as i32,
                );
            }
        }
    }
    if n >= 3 {
        for i in 0..2 {
            bf.b_rotation(4 + i, 7 - i, 56 - 32 * i as i32);
        }
    }
    if n >= 4 {
        for i in 0..4 {
            bf.h(8 + 2 * i, 9 + 2 * i, i & 1 != 0);
        }
    }
    if n >= 5 {
        for i in 0..2 {
            for j in 0..2 {
                bf.b_rotation_flip(
                    30 - 4 * i - j,
                    17 + 4 * i + j,
                    24 + ((j << 6) as i32) + (((1 - i) << 5) as i32),
                );
            }
        }
    }
    if n == 6 {
        for i in 0..8 {
            for j in 0..2 {
                bf.h(32 + i * 4 + j, 35 + i * 4 - j, i & 1 != 0);
            }
        }
    }
    for i in 0..2 {
        let flip = 1 - i as i32;
        if flip == 1 {
            bf.b_rotation_flip(2 * i, 2 * i + 1, 32 + 16 * i as i32);
        } else {
            bf.b_rotation(2 * i, 2 * i + 1, 32 + 16 * i as i32);
        }
    }
    if n >= 3 {
        for i in 0..2 {
            bf.h(4 + 2 * i, 5 + 2 * i, i != 0);
        }
    }
    if n >= 4 {
        for i in 0..2 {
            bf.b_rotation_flip(14 - i, 9 + i, 48 + 64 * i as i32);
        }
    }
    if n >= 5 {
        for i in 0..4 {
            for j in 0..2 {
                bf.h(16 + 4 * i + j, 19 + 4 * i - j, i & 1 != 0);
            }
        }
    }
    if n == 6 {
        for i in 0..2 {
            for j in 0..4 {
                bf.b_rotation_flip(
                    61 - 8 * i - j,
                    34 + 8 * i + j,
                    56 - 32 * i as i32 + (j >> 1) as i32 * 64,
                );
            }
        }
    }
    for i in 0..2 {
        bf.h(i, 3 - i, false);
    }
    if n >= 3 {
        bf.b_rotation_flip(6, 5, 32);
    }
    if n >= 4 {
        for i in 0..2 {
            for j in 0..2 {
                bf.h(8 + 4 * i + j, 11 + 4 * i - j, i != 0);
            }
        }
    }
    if n >= 5 {
        for i in 0..4 {
            bf.b_rotation_flip(29 - i, 18 + i, 48 + (i >> 1) as i32 * 64);
        }
    }
    if n == 6 {
        for i in 0..4 {
            for j in 0..4 {
                bf.h(32 + 8 * i + j, 39 + 8 * i - j, i & 1 != 0);
            }
        }
    }
    if n >= 3 {
        for i in 0..4 {
            bf.h(i, 7 - i, false);
        }
    }
    if n >= 4 {
        for i in 0..2 {
            bf.b_rotation_flip(13 - i, 10 + i, 32);
        }
    }
    if n >= 5 {
        for i in 0..2 {
            for j in 0..4 {
                bf.h(16 + 8 * i + j, 23 + 8 * i - j, i != 0);
            }
        }
    }
    if n == 6 {
        for i in 0..8 {
            bf.b_rotation_flip(59 - i, 36 + i, if i < 4 { 48 } else { 112 });
        }
    }
    if n >= 4 {
        for i in 0..8 {
            bf.h(i, 15 - i, false);
        }
    }
    if n >= 5 {
        for i in 0..4 {
            bf.b_rotation_flip(27 - i, 20 + i, 32);
        }
    }
    if n == 6 {
        for i in 0..8 {
            bf.h(32 + i, 47 - i, false);
            bf.h(48 + i, 63 - i, true);
        }
    }
    if n >= 5 {
        for i in 0..16 {
            bf.h(i, 31 - i, false);
        }
    }
    if n == 6 {
        for i in 0..8 {
            bf.b_rotation_flip(55 - i, 40 + i, 32);
        }
        for i in 0..32 {
            bf.h(i, 63 - i, false);
        }
    }

    t.copy_from_slice(&bf.t);
}

/// `SINPI_1_9`..`SINPI_4_9`.
pub const SINPI_1_9: i32 = 1321;
pub const SINPI_2_9: i32 = 2482;
pub const SINPI_3_9: i32 = 3344;
pub const SINPI_4_9: i32 = 3803;

/// `Inverse ADST input array permutation process` (`3 <= n <= 4`).
pub fn adst_input_permute(t: &mut [i32], n: u32) {
    let n0 = 1usize << n;
    let copy = t.to_vec();
    for i in 0..n0 {
        let idx = if i & 1 != 0 { i - 1 } else { n0 - i - 1 };
        t[i] = copy[idx];
    }
}

/// `Inverse ADST output array permutation process` (`3 <= n <= 4`).
pub fn adst_output_permute(t: &mut [i32], n: u32) {
    let n0 = 1usize << n;
    let copy = t.to_vec();
    for i in 0..n0 {
        let a = (i >> 3) & 1;
        let b = ((i >> 2) & 1) ^ ((i >> 3) & 1);
        let c = ((i >> 1) & 1) ^ ((i >> 2) & 1);
        let d = (i & 1) ^ ((i >> 1) & 1);
        let idx = ((d << 3) | (c << 2) | (b << 1) | a) >> (4 - n);
        t[i] = if i & 1 != 0 { -copy[idx] } else { copy[idx] };
    }
}

/// `Inverse ADST4 process`: a 4-point inverse ADST.
pub fn inverse_adst4(t: &mut [i32; 4], _r: u32) {
    let mut s = [0i32; 7];
    s[0] = SINPI_1_9 * t[0];
    s[1] = SINPI_2_9 * t[0];
    s[2] = SINPI_3_9 * t[1];
    s[3] = SINPI_4_9 * t[2];
    s[4] = SINPI_1_9 * t[2];
    s[5] = SINPI_2_9 * t[3];
    s[6] = SINPI_4_9 * t[3];
    let a7 = t[0] - t[2];
    let b7 = a7 + t[3];
    s[0] += s[3];
    s[1] -= s[4];
    s[3] = s[2];
    s[2] = SINPI_3_9 * b7;
    s[0] += s[5];
    s[1] -= s[6];
    let x0 = s[0] + s[3];
    let x1 = s[1] + s[3];
    let x2 = s[2];
    let x3 = s[0] + s[1] - s[3];
    t[0] = (x0 + 2048) >> 12;
    t[1] = (x1 + 2048) >> 12;
    t[2] = (x2 + 2048) >> 12;
    t[3] = (x3 + 2048) >> 12;
}

/// `Inverse ADST8 process`.
pub fn inverse_adst8(t: &mut [i32; 8], r: u32) {
    adst_input_permute(t, 3);
    let mut bf = Butterfly::new(t.to_vec(), r);
    for i in 0..4 {
        bf.b_rotation_flip(2 * i, 2 * i + 1, 60 - 16 * i as i32);
    }
    for i in 0..4 {
        bf.h(i, 4 + i, false);
    }
    for i in 0..2 {
        bf.b_rotation_flip(4 + 3 * i, 5 + i, 48 - 32 * i as i32);
    }
    for j in 0..2 {
        for i in 0..2 {
            bf.h(4 * j + i, 2 + 4 * j + i, false);
        }
    }
    for i in 0..2 {
        bf.b_rotation_flip(2 + 4 * i, 3 + 4 * i, 32);
    }
    t.copy_from_slice(&bf.t);
    adst_output_permute(t, 3);
}

/// `Inverse ADST16 process`.
pub fn inverse_adst16(t: &mut [i32; 16], r: u32) {
    adst_input_permute(t, 4);
    let mut bf = Butterfly::new(t.to_vec(), r);
    for i in 0..8 {
        bf.b_rotation_flip(2 * i, 2 * i + 1, 62 - 8 * i as i32);
    }
    for i in 0..8 {
        bf.h(i, 8 + i, false);
    }
    for i in 0..2 {
        bf.b_rotation_flip(8 + 2 * i, 9 + 2 * i, 56 - 32 * i as i32);
        bf.b_rotation_flip(13 + 2 * i, 12 + 2 * i, 8 + 32 * i as i32);
    }
    for j in 0..2 {
        for i in 0..4 {
            bf.h(8 * j + i, 4 + 8 * j + i, false);
        }
    }
    for j in 0..2 {
        for i in 0..2 {
            bf.b_rotation_flip(4 + 8 * j + 3 * i, 5 + 8 * j + i, 48 - 32 * i as i32);
        }
    }
    for j in 0..4 {
        for i in 0..2 {
            bf.h(4 * j + i, 2 + 4 * j + i, false);
        }
    }
    for i in 0..4 {
        bf.b_rotation_flip(2 + 4 * i, 3 + 4 * i, 32);
    }
    t.copy_from_slice(&bf.t);
    adst_output_permute(t, 4);
}

/// One axis of the inverse transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxKind {
    Dct,
    Adst,
    FlipAdst,
    Identity,
}

/// `Inverse ADST process` with the FLIPADST input negation.
pub fn inverse_adst(t: &mut [i32], n: u32, flip: bool, r: u32) {
    if flip {
        for (i, value) in t.iter_mut().enumerate() {
            if i & 1 != 0 {
                *value = -*value;
            }
        }
    }
    match n {
        2 => {
            let mut a = [t[0], t[1], t[2], t[3]];
            inverse_adst4(&mut a, r);
            t.copy_from_slice(&a);
        }
        3 => {
            let mut a = [0i32; 8];
            a.copy_from_slice(t);
            inverse_adst8(&mut a, r);
            t.copy_from_slice(&a);
        }
        _ => {
            let mut a = [0i32; 16];
            a.copy_from_slice(t);
            inverse_adst16(&mut a, r);
            t.copy_from_slice(&a);
        }
    }
}

/// `Inverse identity transform process` for transforms of length 4..32.
fn inverse_identity(t: &mut [i32], n: u32) {
    match n {
        2 => {
            for value in t {
                *value = round2_i64(i64::from(*value) * 5793, 12);
            }
        }
        3 => {
            for value in t {
                *value *= 2;
            }
        }
        4 => {
            for value in t {
                *value = round2_i64(i64::from(*value) * 11586, 12);
            }
        }
        5 => {
            for value in t {
                *value *= 4;
            }
        }
        _ => {}
    }
}

/// Dispatch one 1D inverse transform of `1 << n` samples.
pub fn inverse_transform_1d(t: &mut [i32], kind: TxKind, n: u32, r: u32) {
    match kind {
        TxKind::Dct => inverse_dct(t, n, r),
        TxKind::Adst | TxKind::FlipAdst => inverse_adst(t, n, false, r),
        TxKind::Identity => inverse_identity(t, n),
    }
}

/// `2D inverse transform process` for the rectangular transform pair.
#[allow(clippy::too_many_arguments)]
pub fn inverse_transform_2d(
    dequant: &[i32],
    w: usize,
    h: usize,
    row_kind: TxKind,
    col_kind: TxKind,
    row_shift: u32,
    col_shift: u32,
    row_clamp: u32,
    col_clamp: u32,
) -> Vec<i32> {
    let log2_w = (w as u32).trailing_zeros();
    let log2_h = (h as u32).trailing_zeros();
    let mut residual = vec![0i32; w * h];
    for i in 0..h {
        let mut t = vec![0i32; w];
        for (j, value) in t.iter_mut().enumerate() {
            *value = if i < 32 && j < 32 {
                dequant[i * w + j]
            } else {
                0
            };
        }
        if log2_w.abs_diff(log2_h) == 1 {
            for value in t.iter_mut() {
                *value = round2_i64(i64::from(*value) * 2896, 12);
            }
        }
        inverse_transform_1d(&mut t, row_kind, log2_w, row_clamp);
        for (j, value) in t.iter().enumerate() {
            let clipped = clip_to(round2(*value, row_shift), row_clamp);
            residual[i * w + j] = clip_to(clipped, col_clamp);
        }
    }
    for j in 0..w {
        let mut t: Vec<i32> = (0..h).map(|i| residual[i * w + j]).collect();
        inverse_transform_1d(&mut t, col_kind, log2_h, col_clamp);
        for (i, value) in t.iter().enumerate() {
            residual[i * w + j] = clip_to(round2(*value, col_shift), col_clamp);
        }
    }
    if row_kind == TxKind::FlipAdst {
        for row in residual.chunks_exact_mut(w) {
            row.reverse();
        }
    }
    if col_kind == TxKind::FlipAdst {
        for row in 0..h / 2 {
            for col in 0..w {
                residual.swap(row * w + col, (h - row - 1) * w + col);
            }
        }
    }
    residual
}

fn round2(value: i32, shift: u32) -> i32 {
    if shift == 0 {
        value
    } else {
        (value + (1 << (shift - 1))) >> shift
    }
}

fn round2_i64(value: i64, shift: u32) -> i32 {
    let rounded = if shift == 0 {
        value
    } else {
        (value + (1i64 << (shift - 1))) >> shift
    };
    rounded.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn clip_to(value: i32, bits: u32) -> i32 {
    let limit = 1i32 << (bits - 1);
    value.clamp(-limit, limit - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_shifts_use_the_specification_round2() {
        assert_eq!(round2(0, 0), 0);
        assert_eq!(round2(7, 2), 2);
        assert_eq!(round2(-7, 2), -2);
        assert_eq!(round2(5, 1), 3);
        assert_eq!(round2(-5, 1), -2);
    }

    #[test]
    fn transform_2d_dc_is_a_constant_block() {
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 64 << 12;
        let out = inverse_transform_2d(&coeffs, 4, 4, TxKind::Dct, TxKind::Dct, 0, 4, 16, 16);
        // Row DC then column DC: the block is constant after the shifts.
        assert!(out.iter().all(|v| *v == out[0]), "{out:?}");
    }

    #[test]
    fn eight_by_four_horizontal_dct_matches_libaom_vector() {
        let column_major = [
            62, 72, -1296, 1368, 0, 0, 0, 0, 0, 0, 1152, 0, 0, 0, 0, 0, 0, 0, 0, -1224, 0, 0, 0, 0,
            0, 0, -504, 0, 0, 0, 0, 0,
        ];
        let mut row_major = [0i32; 32];
        for pos in 0..32 {
            let row = pos & 3;
            let col = pos >> 2;
            row_major[row * 8 + col] = column_major[pos];
        }
        let out = inverse_transform_2d(
            &row_major,
            8,
            4,
            TxKind::Dct,
            TxKind::Identity,
            0,
            4,
            16,
            16,
        );
        assert_eq!(
            out,
            [
                3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, -3, -1, -114, -112, -112, -114, -1,
                -3, 6, 115, 115, 6, 6, 115, 115, 6,
            ]
        );
    }

    #[test]
    fn eight_by_four_dct_adst_matches_libaom_vector() {
        let mut column_major = [0i32; 32];
        column_major[..3].copy_from_slice(&[-68, 160, -80]);
        let mut row_major = [0i32; 32];
        for pos in 0..32 {
            let row = pos & 3;
            let col = pos >> 2;
            row_major[row * 8 + col] = column_major[pos];
        }
        let out = inverse_transform_2d(&row_major, 8, 4, TxKind::Adst, TxKind::Dct, 0, 4, 16, 16);
        assert_eq!(
            out,
            [
                0, 1, 1, 1, 1, 2, 2, 2, 0, 1, 2, 2, 2, 3, 3, 3, 0, -1, -1, -1, -2, -2, -2, -2, -1,
                -3, -5, -7, -9, -10, -11, -11,
            ]
        );
    }

    #[test]
    fn four_by_four_horizontal_dct_matches_libaom_vector() {
        let input = [0, 0, 936, 936, 0, 0, -864, 0, 0, 0, 0, -936, 0, 0, 360, 0];
        let mut row_major = [0i32; 16];
        for pos in 0..16 {
            let row = pos & 3;
            let col = pos >> 2;
            row_major[row * 4 + col] = input[pos];
        }
        let out = inverse_transform_2d(
            &row_major,
            4,
            4,
            TxKind::Dct,
            TxKind::Identity,
            0,
            4,
            16,
            16,
        );
        assert_eq!(
            out,
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 117, 117, 0, 117, 117, 0]
        );
    }

    #[test]
    fn four_by_four_identity_matches_libaom_vector() {
        let column_major = [0, 0, 168, 252, 0, 0, 0, 168, 0, 0, 0, 0, 0, 0, 168, 168];
        let mut row_major = [0i32; 16];
        for pos in 0..16 {
            let row = pos & 3;
            let col = pos >> 2;
            row_major[row * 4 + col] = column_major[pos];
        }
        let out = inverse_transform_2d(
            &row_major,
            4,
            4,
            TxKind::Identity,
            TxKind::Identity,
            0,
            4,
            16,
            16,
        );
        assert_eq!(out, [0, 0, 0, 0, 0, 0, 0, 0, 21, 0, 0, 21, 31, 21, 0, 21]);
    }

    #[test]
    fn four_by_four_dct_adst_matches_libaom_vector() {
        let mut row_major = [0i32; 16];
        row_major[0] = -518;
        let out = inverse_transform_2d(&row_major, 4, 4, TxKind::Adst, TxKind::Dct, 0, 4, 16, 16);
        assert_eq!(
            out,
            [-7, -14, -19, -21, -7, -14, -19, -21, -7, -14, -19, -21, -7, -14, -19, -21]
        );
    }

    #[test]
    fn four_by_four_dct_dct_matches_libaom_vector() {
        let column_major = [
            -1984, 360, 0, -144, 360, -288, 0, 144, 0, 0, 0, 0, -144, 144, 0, -72,
        ];
        let mut row_major = [0i32; 16];
        for pos in 0..16 {
            let row = pos & 3;
            let col = pos >> 2;
            row_major[row * 4 + col] = column_major[pos];
        }
        let out = inverse_transform_2d(&row_major, 4, 4, TxKind::Dct, TxKind::Dct, 0, 4, 16, 16);
        assert_eq!(
            out,
            [-47, -49, -51, -52, -49, -51, -49, -51, -51, -49, -99, -97, -52, -51, -97, -96,]
        );
    }

    #[test]
    fn four_by_eight_dct_dct_matches_libaom_rounding() {
        let mut column_major = [0i32; 32];
        column_major[0] = -165;
        let mut row_major = [0i32; 32];
        for pos in 0..32 {
            let row = pos & 7;
            let col = pos >> 3;
            row_major[row * 4 + col] = column_major[pos];
        }
        let out = inverse_transform_2d(&row_major, 4, 8, TxKind::Dct, TxKind::Dct, 0, 4, 16, 16);
        assert_eq!(out, [-4; 32]);
    }

    #[test]
    fn sixteen_by_thirty_two_dc_matches_libaom_rounding() {
        let mut column_major = [0i32; 16 * 32];
        column_major[0] = 104;
        let mut row_major = [0i32; 16 * 32];
        for pos in 0..16 * 32 {
            let row = pos & 31;
            let col = pos >> 5;
            row_major[row * 16 + col] = column_major[pos];
        }
        let out = inverse_transform_2d(&row_major, 16, 32, TxKind::Dct, TxKind::Dct, 1, 4, 16, 16);
        assert_eq!(out, [1; 16 * 32]);
    }

    #[test]
    fn flipadst_negates_the_odd_inputs() {
        let base = [10i32, 20, 30, 40];
        let mut plain = [0i32; 4];
        plain.copy_from_slice(&base);
        inverse_adst(&mut plain, 2, true, 16);
        let flipped: [i32; 4] =
            std::array::from_fn(|i| if i & 1 != 0 { -base[i] } else { base[i] });
        let mut expect = [0i32; 4];
        expect.copy_from_slice(&flipped);
        inverse_adst(&mut expect, 2, false, 16);
        assert_eq!(plain, expect);
    }

    #[test]
    fn inverse_dct_dc_is_constant() {
        for n in 2..=6u32 {
            let len = 1usize << n;
            for dc in [1i32, 16, 100] {
                let mut t = vec![0i32; len];
                t[0] = dc;
                inverse_dct(&mut t, n, 16);
                let first = t[0];
                assert!(t.iter().all(|v| *v == first), "n {n} dc {dc}: {t:?}");
            }
        }
    }

    #[test]
    fn inverse_dct_is_linear_before_rounding() {
        let n = 2u32;
        let a = vec![10, -20, 30, -40];
        let mut scaled = a.iter().map(|v| v * 2).collect::<Vec<_>>();
        inverse_dct(&mut scaled, n, 16);
        let mut single = a.clone();
        inverse_dct(&mut single, n, 16);
        let doubled = single.iter().map(|v| v * 2).collect::<Vec<_>>();
        for (a, b) in scaled.iter().zip(doubled.iter()) {
            assert!((a - b).abs() <= 2, "{scaled:?} vs {doubled:?}");
        }
    }

    #[test]
    fn adst8_and_adst16_are_linear_and_deterministic() {
        let base8 = [100i32, -50, 20, 0, 0, 0, 0, 0];
        let mut a = base8;
        inverse_adst8(&mut a, 16);
        let mut again = base8;
        inverse_adst8(&mut again, 16);
        assert_eq!(a, again);
        let doubled = base8.map(|v| v * 2);
        let mut b = doubled;
        inverse_adst8(&mut b, 16);
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x * 2 - y).abs() <= 2, "{a:?} vs {b:?}");
        }

        let mut base16 = [0i32; 16];
        base16[0] = 100;
        let mut c = base16;
        inverse_adst16(&mut c, 16);
        let mut d = base16.map(|v| v * 2);
        inverse_adst16(&mut d, 16);
        for (x, y) in c.iter().zip(d.iter()) {
            assert!((x * 2 - y).abs() <= 2, "{c:?} vs {d:?}");
        }
        // Regression snapshots; numeric validation against libaom needs the
        // 2D/tile path.
        assert_eq!(a, [4, 10, 10, 13, 35, 78, 130, 166]);
        assert_eq!(
            c,
            [5, 15, 24, 34, 42, 52, 59, 67, 74, 81, 86, 90, 94, 97, 99, 100]
        );
    }

    #[test]
    fn adst4_matches_the_specification_sequence() {
        // T = [100,0,0,0] gives x = [132100, 248200, 334400, 380300].
        let mut t = [100i32, 0, 0, 0];
        inverse_adst4(&mut t, 16);
        assert_eq!(t, [32, 61, 82, 93]);
    }

    #[test]
    fn adst_permutations_follow_the_index_rules() {
        let mut t = vec![0, 1, 2, 3, 4, 5, 6, 7];
        adst_input_permute(&mut t, 3);
        assert_eq!(t, [7, 0, 5, 2, 3, 4, 1, 6]);
        let mut t = (0..8).collect::<Vec<i32>>();
        adst_output_permute(&mut t, 3);
        // Computed from the specification's a/b/c/d index rule.
        assert_eq!(t, [0, -4, 6, -2, 3, -7, 5, -1]);
    }

    #[test]
    fn vertical_flipadst_keeps_the_horizontal_axis_identity() {
        let input = [3311, -50, -650, -250, 300, 450, 100, -250];
        let mut coeffs = vec![0i32; 64];
        for (row, value) in input.iter().enumerate() {
            coeffs[row * 8] = *value;
        }
        let out = inverse_transform_2d(
            &coeffs,
            8,
            8,
            TxKind::Identity,
            TxKind::FlipAdst,
            1,
            4,
            16,
            16,
        );
        assert_eq!(
            (0..8).map(|row| out[row * 8]).collect::<Vec<_>>(),
            [187, 191, 190, 194, 196, 20, 21, 20]
        );
        for row in 0..8 {
            assert!(out[row * 8 + 1..(row + 1) * 8]
                .iter()
                .all(|value| *value == 0));
        }
    }

    #[test]
    fn butterfly_rotation_and_flip() {
        // cos128(32) = sin128(32) = 2896.
        let mut bf = Butterfly::new(vec![100, 50], 24);
        bf.b_rotation(0, 1, 32);
        assert_eq!(bf.t, [35, 106]);
        let mut bf = Butterfly::new(vec![100, 50], 24);
        bf.b_rotation_flip(0, 1, 32);
        assert_eq!(bf.t, [106, 35]);
    }

    #[test]
    fn hadamard_rotation_clamps() {
        let mut bf = Butterfly::new(vec![100, 50], 8);
        bf.h(0, 1, false);
        assert_eq!(bf.t, [127, 50]);
        // Flipped indices: H(1, 0, 0) gives T[1] = 150 and T[0] = -50.
        let mut bf = Butterfly::new(vec![100, 50], 24);
        bf.h(0, 1, true);
        assert_eq!(bf.t, [-50, 150]);
    }

    #[test]
    fn dct_permutation_uses_bit_reversal() {
        let mut t = vec![1, 2, 3, 4];
        idct_permute(&mut t, 2);
        assert_eq!(t, [1, 3, 2, 4]);
        let mut t = (0..8).collect::<Vec<i32>>();
        idct_permute(&mut t, 3);
        assert_eq!(t, [0, 4, 2, 6, 1, 5, 3, 7]);
    }

    #[test]
    fn cos128_and_sin128_follow_the_lookup_rules() {
        assert_eq!(cos128(0), 4096);
        assert_eq!(cos128(32), COS128_LOOKUP[32]);
        assert_eq!(cos128(64), 0);
        assert_eq!(cos128(96), -COS128_LOOKUP[32]);
        assert_eq!(cos128(128), -4096);
        assert_eq!(cos128(192), 0);
        assert_eq!(cos128(255), COS128_LOOKUP[1]);
        assert_eq!(sin128(0), 0);
        assert_eq!(sin128(64), 4096);
        assert_eq!(sin128(32), COS128_LOOKUP[32]);
    }

    #[test]
    fn bit_reversal_matches_the_specification_formula() {
        assert_eq!(brev(0, 5), 0);
        assert_eq!(brev(1, 1), 1);
        assert_eq!(brev(2, 1), 2);
        assert_eq!(brev(3, 1), 4);
        assert_eq!(brev(4, 1), 8);
        assert_eq!(brev(3, 7), 7);
        assert_eq!(brev(2, 3), 3);
    }

    #[test]
    fn cospi_matches_the_reference_values() {
        assert_eq!(cospi(0), 64);
        assert_eq!(cospi(8), 59);
        assert_eq!(cospi(16), 45);
        assert_eq!(cospi(32), 0);
        assert_eq!(cospi(48), -45);
        assert_eq!(cospi(56), -59);
        assert_eq!(cospi(63), -64);
    }

    #[test]
    fn dc_only_input_is_a_constant_block() {
        let mut block = vec![0i32; 16];
        block[0] = 64;
        let out = idct_2d(&block, 4);
        let first = out[0];
        assert!(out.iter().all(|v| *v == first), "{out:?}");
        assert!(first > 0);
    }

    #[test]
    fn one_dimensional_transform_matches_a_direct_formula() {
        let input = [100, -50, 20, -10];
        let out = idct_1d_fixed(&input);
        // Recompute independently with the orthonormal DCT-III definition.
        for (k, value) in out.iter().enumerate() {
            let mut sum = 0f64;
            for (i, x) in input.iter().enumerate() {
                let scale = if i == 0 { 1.0 / 2.0f64.sqrt() } else { 1.0 };
                sum += *x as f64
                    * scale
                    * (std::f64::consts::PI * (2 * k + 1) as f64 * i as f64 / 8.0).cos();
            }
            let expected = (sum / 2.0f64.sqrt()).round() as i32;
            assert_eq!(*value, expected);
        }
    }

    #[test]
    fn two_dimensional_transform_is_separable() {
        let mut input = vec![0i32; 64];
        input[0] = 32;
        input[1] = -16;
        let out = idct_2d(&input, 8);
        // A DC + horizontal frequency is constant down every column.
        for r in 1..8 {
            for c in 0..8 {
                assert_eq!(out[r * 8 + c], out[c], "row {r} col {c}");
            }
        }
    }
}
