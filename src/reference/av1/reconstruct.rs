//! AV1 reconstruction: dequantize a coefficient block, inverse-transform it and
//! add the residual to the prediction.

use crate::reference::av1::idct::{inverse_transform_2d, TxKind};
use crate::reference::av1::quant::dequantize;
use crate::reference::av1::tx::row_shift;
use crate::reference::av1::tx::TxSize;

/// Dequantize a `w` x `h` coefficient block: DC uses `dc_q`, everything else
/// `ac_q`.
pub fn dequantize_block(coeffs: &[i32], dc_q: i32, ac_q: i32) -> Vec<i32> {
    coeffs
        .iter()
        .enumerate()
        .map(|(i, c)| dequantize(*c, if i == 0 { dc_q } else { ac_q }))
        .collect()
}

/// Rebuild a block: dequantize, inverse 2D transform, add to `prediction` and
/// clip to the sample range.
#[allow(clippy::too_many_arguments)]
pub fn reconstruct_block(
    coeffs: &[i32],
    w: usize,
    h: usize,
    tx: TxSize,
    row_kind: TxKind,
    col_kind: TxKind,
    dc_q: i32,
    ac_q: i32,
    prediction: &[u16],
    bit_depth: u32,
    lossless: bool,
) -> Vec<u16> {
    let dequant = dequantize_block(coeffs, dc_q, ac_q);
    let residual = inverse_transform_2d(
        &dequant,
        w,
        h,
        row_kind,
        col_kind,
        if lossless { 0 } else { row_shift(tx) },
        if lossless { 0 } else { 4 },
        bit_depth + 8,
        (bit_depth + 6).max(16),
    );
    let max = (1i32 << bit_depth) - 1;
    prediction
        .iter()
        .zip(residual.iter())
        .map(|(p, r)| (i32::from(*p) + r).clamp(0, max) as u16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx4() -> TxSize {
        TxSize::Tx4x4
    }

    #[test]
    fn zero_coefficients_leave_the_prediction() {
        let coeffs = vec![0i32; 16];
        let pred = vec![100u16; 16];
        let out = reconstruct_block(
            &coeffs,
            4,
            4,
            tx4(),
            TxKind::Dct,
            TxKind::Dct,
            8,
            8,
            &pred,
            8,
            false,
        );
        assert_eq!(out, pred);
    }

    #[test]
    fn dc_coefficient_adds_a_constant_offset() {
        let mut coeffs = vec![0i32; 16];
        // Dequant DC = 1 * dc_q(8, 0) = 4; the inverse DCT spreads it evenly.
        coeffs[0] = 1;
        let pred = vec![100u16; 16];
        let out = reconstruct_block(
            &coeffs,
            4,
            4,
            tx4(),
            TxKind::Dct,
            TxKind::Dct,
            4,
            4,
            &pred,
            8,
            false,
        );
        assert!(out.iter().all(|v| *v == out[0]), "{out:?}");
    }

    #[test]
    fn dequantize_uses_dc_and_ac_quantizers() {
        let coeffs = [3, 3, 3, 3];
        assert_eq!(dequantize_block(&coeffs, 10, 20), [30, 60, 60, 60]);
    }
}
