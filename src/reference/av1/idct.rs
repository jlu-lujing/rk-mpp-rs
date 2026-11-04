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

#[cfg(test)]
mod tests {
    use super::*;

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
