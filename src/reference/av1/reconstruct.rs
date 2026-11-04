//! AV1 reconstruction: dequantize a coefficient block, inverse-transform it and
//! add the residual to the prediction.

use crate::reference::av1::idct::{inverse_transform_2d, TxKind};
use crate::reference::av1::intra::IntraMode;
use crate::reference::av1::predict::{predict_intra, IntraEdges};
use crate::reference::av1::quant::dequantize;
use crate::reference::av1::tx::row_shift;
use crate::reference::av1::tx::TxSize;
use crate::reference::av1::tx_type::TxType;

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

/// Parameters for rebuilding one intra transform block in place.
#[derive(Debug, Clone, Copy)]
pub struct IntraTxBlock<'a> {
    pub coeffs: &'a [i32],
    pub x: usize,
    pub y: usize,
    pub tx: TxSize,
    pub tx_type: TxType,
    pub mode: IntraMode,
    pub angle_delta: i32,
    pub enable_edge_filter: bool,
    pub filter_type: u8,
    pub have_above: bool,
    pub have_left: bool,
    pub have_above_right: bool,
    pub have_below_left: bool,
    pub dc_q: i32,
    pub ac_q: i32,
    pub bit_depth: u32,
    pub lossless: bool,
}

/// Predict, inverse-transform, reconstruct and write one intra transform block.
pub fn reconstruct_intra_tx(
    frame: &mut [u16],
    stride: usize,
    plane_width: usize,
    plane_height: usize,
    block: &IntraTxBlock<'_>,
) -> Vec<u16> {
    let w = block.tx.width() as usize;
    let h = block.tx.height() as usize;
    assert!(block.coeffs.len() >= w * h);
    assert!(block.x + w <= plane_width && block.y + h <= plane_height);
    let edges = IntraEdges::from_plane(
        frame,
        stride,
        block.x,
        block.y,
        w,
        h,
        block.have_above,
        block.have_left,
        block.have_above_right,
        block.have_below_left,
        plane_width - 1,
        plane_height - 1,
        block.bit_depth,
    );
    let prediction = predict_intra(
        block.mode,
        &edges,
        (w as u32).trailing_zeros(),
        (h as u32).trailing_zeros(),
        block.angle_delta,
        block.enable_edge_filter,
        block.filter_type,
        block.bit_depth,
    );
    let out = reconstruct_block(
        &block.coeffs[..w * h],
        w,
        h,
        block.tx,
        block.tx_type.row_kind(),
        block.tx_type.col_kind(),
        block.dc_q,
        block.ac_q,
        &prediction,
        block.bit_depth,
        block.lossless,
    );
    for row in 0..h {
        let start = (block.y + row) * stride + block.x;
        frame[start..start + w].copy_from_slice(&out[row * w..(row + 1) * w]);
    }
    out
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

    #[test]
    fn first_dc_intra_block_uses_midpoint_and_zero_residual() {
        let mut frame = vec![0u16; 16 * 16];
        let out = reconstruct_intra_tx(
            &mut frame,
            16,
            16,
            16,
            &IntraTxBlock {
                coeffs: &[0; 16],
                x: 0,
                y: 0,
                tx: TxSize::Tx4x4,
                tx_type: TxType::DctDct,
                mode: IntraMode::Dc,
                angle_delta: 0,
                enable_edge_filter: true,
                filter_type: 0,
                have_above: false,
                have_left: false,
                have_above_right: false,
                have_below_left: false,
                dc_q: 4,
                ac_q: 4,
                bit_depth: 8,
                lossless: false,
            },
        );
        assert_eq!(out, [128; 16]);
        assert_eq!(&frame[..4], &[128; 4]);
    }

    #[test]
    fn reconstructed_neighbour_feeds_vertical_prediction() {
        let mut frame = vec![128u16; 16 * 16];
        frame[3 * 16 + 4..3 * 16 + 8].copy_from_slice(&[10, 20, 30, 40]);
        let out = reconstruct_intra_tx(
            &mut frame,
            16,
            16,
            16,
            &IntraTxBlock {
                coeffs: &[0; 16],
                x: 4,
                y: 4,
                tx: TxSize::Tx4x4,
                tx_type: TxType::DctDct,
                mode: IntraMode::V,
                angle_delta: 0,
                enable_edge_filter: false,
                filter_type: 0,
                have_above: true,
                have_left: true,
                have_above_right: true,
                have_below_left: true,
                dc_q: 4,
                ac_q: 4,
                bit_depth: 8,
                lossless: false,
            },
        );
        assert_eq!(out, [10, 20, 30, 40].repeat(4));
        assert_eq!(&frame[4 * 16 + 4..4 * 16 + 8], &[10, 20, 30, 40]);
    }
}
