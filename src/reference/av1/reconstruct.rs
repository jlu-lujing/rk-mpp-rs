//! AV1 reconstruction: dequantize a coefficient block, inverse-transform it and
//! add the residual to the prediction.

use crate::reference::av1::idct::{inverse_transform_2d, TxKind};
use crate::reference::av1::intra::{IntraMode, UvMode};
use crate::reference::av1::intra_sink::IntraSyntaxBlock;
use crate::reference::av1::predict::{intra_filter_type, predict_intra, IntraEdges};
use crate::reference::av1::quant::{ac_q, dc_q, dequantize};
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

/// Convert AV1's column-major entropy buffer to the row-major transform input.
///
/// AV1 caps transform coefficients at 32x32; 64-sample transforms retain that
/// coefficient grid and reconstruct with zero high-frequency rows/columns.
pub fn quant_to_plane_order(quant: &[i32], tx: TxSize) -> Vec<i32> {
    let src_w = (tx.width() as usize).min(32);
    let src_h = (tx.height() as usize).min(32);
    let dst_w = tx.width() as usize;
    let dst_h = tx.height() as usize;
    let src_log2_h = src_h.trailing_zeros() as usize;
    let mut out = vec![0i32; dst_w * dst_h];
    for pos in 0..src_w * src_h {
        let row = pos & (src_h - 1);
        let col = pos >> src_log2_h;
        out[row * dst_w + col] = quant[pos];
    }
    out
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaneQDelta {
    pub dc: i32,
    pub ac: i32,
}

fn uv_intra_mode(mode: UvMode) -> Option<IntraMode> {
    Some(match mode {
        UvMode::Dc => IntraMode::Dc,
        UvMode::V => IntraMode::V,
        UvMode::H => IntraMode::H,
        UvMode::D45 => IntraMode::D45,
        UvMode::D135 => IntraMode::D135,
        UvMode::D113 => IntraMode::D113,
        UvMode::D157 => IntraMode::D157,
        UvMode::D203 => IntraMode::D203,
        UvMode::D67 => IntraMode::D67,
        UvMode::Smooth => IntraMode::Smooth,
        UvMode::SmoothV => IntraMode::SmoothV,
        UvMode::SmoothH => IntraMode::SmoothH,
        UvMode::Paeth => IntraMode::Paeth,
        UvMode::Cfl => return None,
    })
}

/// Reconstruct one plane of an intra syntax block when it uses ordinary intra
/// prediction. Palette, CFL and filter-intra are explicit later layers.
#[allow(clippy::too_many_arguments)]
pub fn reconstruct_standard_block_plane(
    frame: &mut [u16],
    stride: usize,
    plane_width: usize,
    plane_height: usize,
    plane: usize,
    syntax: &IntraSyntaxBlock,
    q_delta: PlaneQDelta,
    bit_depth: u32,
    enable_intra_edge_filter: bool,
    above_mode: Option<IntraMode>,
    left_mode: Option<IntraMode>,
) -> Result<(), &'static str> {
    if (plane == 0 && syntax.palette.y_size > 0) || (plane > 0 && syntax.palette.uv_size > 0) {
        return Err("palette prediction is not wired into plane reconstruction");
    }
    if syntax.filter_intra.is_some() {
        return Err("filter-intra prediction is not wired into plane reconstruction");
    }
    let (mode, angle_delta) = if plane == 0 {
        (syntax.y_mode, syntax.angle_y)
    } else {
        let mode = syntax
            .uv_mode
            .and_then(uv_intra_mode)
            .ok_or("CFL prediction is not wired into plane reconstruction")?;
        (mode, syntax.angle_uv)
    };
    let qindex = (i32::from(syntax.qindex) + q_delta.dc).clamp(0, 255);
    let dc_q = dc_q(bit_depth, qindex);
    let ac_q = ac_q(bit_depth, qindex + q_delta.ac - q_delta.dc);
    let filter_type = intra_filter_type(above_mode, left_mode);

    for tx in &syntax.transforms {
        if tx.plane != plane {
            continue;
        }
        let plane_quant = quant_to_plane_order(&tx.quant, tx.tx);
        let w = tx.tx.width() as usize;
        let h = tx.tx.height() as usize;
        if tx.x + w > plane_width || tx.y + h > plane_height {
            return Err("transform extends beyond the reconstructed plane");
        }
        reconstruct_intra_tx(
            frame,
            stride,
            plane_width,
            plane_height,
            &IntraTxBlock {
                coeffs: &plane_quant,
                x: tx.x,
                y: tx.y,
                tx: tx.tx,
                tx_type: tx.tx_type,
                mode,
                angle_delta,
                enable_edge_filter: enable_intra_edge_filter && !syntax.lossless,
                filter_type,
                have_above: tx.y > 0,
                have_left: tx.x > 0,
                have_above_right: tx.x + w < plane_width,
                have_below_left: tx.y + h < plane_height,
                dc_q,
                ac_q,
                bit_depth,
                lossless: syntax.lossless,
            },
        );
    }
    Ok(())
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
    fn entropy_quant_order_is_transposed_for_the_transform() {
        let quant: Vec<i32> = (0..16).collect();
        assert_eq!(
            quant_to_plane_order(&quant, TxSize::Tx4x4),
            [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15]
        );
    }

    #[test]
    fn capped_sixty_four_transforms_zero_pad_their_plane_grid() {
        let quant: Vec<i32> = (0..32 * 32).collect();
        let plane = quant_to_plane_order(&quant, TxSize::Tx64x64);
        assert_eq!(plane.len(), 64 * 64);
        assert_eq!(plane[63], 0);
        assert_eq!(plane[31], 31 * 32);
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

    #[test]
    fn syntax_quant_reconstructs_a_standard_intra_plane() {
        use crate::reference::av1::cfl::CflAlphas;
        use crate::reference::av1::intra_sink::IntraSyntaxTx;
        use crate::reference::av1::palette::{PaletteModeInfo, PaletteTokens};
        use crate::reference::av1::partition::BLOCK_SIZES;
        use crate::reference::av1::tile::DecodedBlock;

        let syntax = IntraSyntaxBlock {
            block: DecodedBlock {
                row: 0,
                col: 0,
                size: BLOCK_SIZES[0],
            },
            segment_id: 0,
            skip: false,
            lossless: false,
            qindex: 40,
            y_mode: IntraMode::Dc,
            uv_mode: None,
            cfl_alphas: CflAlphas::default(),
            angle_y: 0,
            angle_uv: 0,
            filter_intra: None,
            palette: PaletteModeInfo::default(),
            palette_tokens: PaletteTokens { y: None, uv: None },
            tx: TxSize::Tx4x4,
            transforms: vec![IntraSyntaxTx {
                plane: 0,
                x: 0,
                y: 0,
                tx: TxSize::Tx4x4,
                tx_type: TxType::DctDct,
                all_zero: true,
                eob: 0,
                quant: vec![0; 16],
            }],
        };
        let mut plane = vec![0u16; 8 * 8];
        reconstruct_standard_block_plane(
            &mut plane,
            8,
            8,
            8,
            0,
            &syntax,
            PlaneQDelta::default(),
            8,
            true,
            None,
            None,
        )
        .unwrap();
        assert_eq!(&plane[..4], &[128; 4]);
        assert_eq!(plane[4], 0);
    }
}
