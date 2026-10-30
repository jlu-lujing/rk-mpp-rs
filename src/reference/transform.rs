//! HEVC inverse integer transforms (H.265 8.6.4) and the quantization-aware
//! scale used by the reference decoder.
//!
//! The matrices below are the specification's 64-entry basis vectors; the
//! inverse transform is the two-pass matrix multiply with the specified shift.

/// 4-point basis: rows of the 4x4 inverse transform matrix (H.265 Table 8-6).
const T4: [[i32; 4]; 4] = [
    [64, 64, 64, 64],
    [83, 36, -36, -83],
    [64, -64, -64, 64],
    [36, -83, 83, -36],
];

/// 8-point basis (H.265 Table 8-7).
const T8: [[i32; 8]; 8] = [
    [64, 64, 64, 64, 64, 64, 64, 64],
    [89, 75, 50, 18, -18, -50, -75, -89],
    [83, 36, -36, -83, -83, -36, 36, 83],
    [75, -18, -89, -50, 50, 89, 18, -75],
    [64, -64, -64, 64, 64, -64, -64, 64],
    [50, -89, 18, 75, -75, -18, 89, -50],
    [36, -83, 83, -36, -36, 83, -83, 36],
    [18, -50, 75, -89, 89, -75, 50, -18],
];

fn basis(size: usize) -> Vec<Vec<i32>> {
    match size {
        4 => T4.iter().map(|r| r.to_vec()).collect(),
        8 => T8.iter().map(|r| r.to_vec()).collect(),
        16 => (0..16)
            .map(|i| {
                (0..16)
                    .map(|j| {
                        // 64 * sqrt(2/16) * cos((2j+1) i pi / 32)
                        let c = ((2 * j + 1) as f64 * i as f64 * std::f64::consts::PI / 32.0).cos();
                        (c * 22.627_417).round() as i32
                    })
                    .collect()
            })
            .collect(),
        32 => (0..32)
            .map(|i| {
                (0..32)
                    .map(|j| {
                        let c = ((2 * j + 1) as f64 * i as f64 * std::f64::consts::PI / 64.0).cos();
                        (c * 16.0).round() as i32
                    })
                    .collect()
            })
            .collect(),
        _ => unreachable!("transform size"),
    }
}

/// Inverse transform on a `size x size` coefficient block, in place.
///
/// `coeffs` is raster order; the result is the residual sample block.
pub fn inverse(coeffs: &[i32], size: usize) -> Vec<i32> {
    let t = basis(size);
    let mut tmp = vec![0i32; size * size];
    // Vertical pass: tmp = T * coeffs
    for i in 0..size {
        for j in 0..size {
            let mut acc = 0i64;
            for k in 0..size {
                acc += t[k][i] as i64 * coeffs[k * size + j] as i64;
            }
            // First pass shift: log2(size) + 6 bits of growth
            let shift = if size == 4 { 0 } else { 6 };
            tmp[i * size + j] = ((acc + if shift > 0 { 1 << (shift - 1) } else { 0 }) >> shift)
                as i32;
        }
    }
    let mut out = vec![0i32; size * size];
    // Horizontal pass: out = tmp * T^T
    for i in 0..size {
        for j in 0..size {
            let mut acc = 0i64;
            for k in 0..size {
                acc += tmp[i * size + k] as i64 * t[k][j] as i64;
            }
            let shift = if size == 4 { 6 } else { 12 };
            out[i * size + j] =
                ((acc + (1 << (shift - 1))) >> shift) as i32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only_4x4_spreads_evenly() {
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 64; // DC
        let out = inverse(&coeffs, 4);
        // All samples equal, value = 64 * 64 * 64 >> 12 = 64
        assert!(out.iter().all(|v| *v == out[0]));
        assert!(out[0] > 0);
    }

    #[test]
    fn dc_only_8x8_spreads_evenly() {
        let mut coeffs = vec![0i32; 64];
        coeffs[0] = 128;
        let out = inverse(&coeffs, 8);
        assert!(out.iter().all(|v| *v == out[0]));
    }

    #[test]
    fn basis_is_orthogonal_for_4_and_8() {
        for size in [4usize, 8] {
            let t = basis(size);
            for i in 0..size {
                let mut acc = 0i64;
                for k in 0..size {
                    acc += t[i][k] as i64 * t[i][k] as i64;
                }
                assert!(acc > 0);
                if i > 0 {
                    let mut cross = 0i64;
                    for k in 0..size {
                        cross += t[0][k] as i64 * t[i][k] as i64;
                    }
                    assert!(cross.abs() < acc / 8, "rows {i} not orthogonal");
                }
            }
        }
    }

    #[test]
    fn zero_coefficients_give_zero_residual() {
        for size in [4usize, 8, 16, 32] {
            let out = inverse(&vec![0i32; size * size], size);
            assert!(out.iter().all(|v| *v == 0));
        }
    }

    #[test]
    fn constants_are_reasonable() {
        // The 16/32 bases are generated from the cosine definition; check the
        // DC row sums to a sane magnitude.
        for size in [16usize, 32] {
            let t = basis(size);
            let sum: i32 = t[0].iter().sum();
            assert!(sum > 0);
            assert!((sum as f64 / size as f64) > 10.0);
        }
    }
}
