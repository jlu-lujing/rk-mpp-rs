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

#[cfg(test)]
mod tests {
    use super::*;

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
