//! AV1 reconstruction: dequantize a coefficient block, inverse-transform it and
//! add the residual to the prediction.

use crate::reference::av1::filter_intra::predict_filter_intra;
use crate::reference::av1::idct::{inverse_transform_2d, TxKind};
use crate::reference::av1::intra::{IntraMode, UvMode};
use crate::reference::av1::intra_sink::IntraSyntaxBlock;
use crate::reference::av1::palette::predict_palette_plane;
use crate::reference::av1::predict::{cfl_predict, intra_filter_type, predict_intra, IntraEdges};
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
    assert!(block.x + w <= stride);
    assert!(frame.len() >= (block.y + h) * stride);
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

#[derive(Debug, Clone, Copy)]
pub struct ReconstructedLuma<'a> {
    pub samples: &'a [u16],
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

/// `BlockDecoded[plane]` at one 4x4 sample per cell.
#[derive(Debug, Clone)]
pub(crate) struct DecodedPlane {
    cols: usize,
    rows: usize,
    cells: Vec<bool>,
}

impl DecodedPlane {
    pub(crate) fn new(mi_cols: usize, mi_rows: usize, sub_x: u32, sub_y: u32) -> Self {
        let cols = mi_cols >> sub_x;
        let rows = mi_rows >> sub_y;
        Self {
            cols,
            rows,
            cells: vec![false; cols * rows],
        }
    }

    fn at(&self, col: isize, row: isize) -> bool {
        col >= 0
            && row >= 0
            && (col as usize) < self.cols
            && (row as usize) < self.rows
            && self.cells[row as usize * self.cols + col as usize]
    }

    fn mark(&mut self, x: usize, y: usize, width: usize, height: usize) {
        let x4 = x >> 2;
        let y4 = y >> 2;
        for row in y4..(y4 + (height >> 2)).min(self.rows) {
            for col in x4..(x4 + (width >> 2)).min(self.cols) {
                self.cells[row * self.cols + col] = true;
            }
        }
    }
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

/// Reconstruct one plane of an intra syntax block. Palette and filter-intra
/// predictions are wired here; CFL is the remaining explicit layer.
#[allow(clippy::too_many_arguments)]
pub fn reconstruct_intra_block_plane(
    frame: &mut [u16],
    stride: usize,
    plane_width: usize,
    plane_height: usize,
    plane: usize,
    sub_x: u32,
    sub_y: u32,
    syntax: &IntraSyntaxBlock,
    q_delta: PlaneQDelta,
    bit_depth: u32,
    enable_intra_edge_filter: bool,
    above_mode: Option<IntraMode>,
    left_mode: Option<IntraMode>,
    luma: Option<ReconstructedLuma<'_>>,
) -> Result<(), &'static str> {
    reconstruct_intra_block_plane_with_grid(
        frame,
        stride,
        plane_width,
        plane_height,
        plane,
        sub_x,
        sub_y,
        syntax,
        q_delta,
        bit_depth,
        enable_intra_edge_filter,
        above_mode,
        left_mode,
        luma,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn reconstruct_intra_block_plane_with_grid(
    frame: &mut [u16],
    stride: usize,
    plane_width: usize,
    plane_height: usize,
    plane: usize,
    sub_x: u32,
    sub_y: u32,
    syntax: &IntraSyntaxBlock,
    q_delta: PlaneQDelta,
    bit_depth: u32,
    enable_intra_edge_filter: bool,
    above_mode: Option<IntraMode>,
    left_mode: Option<IntraMode>,
    luma: Option<ReconstructedLuma<'_>>,
    mut decoded: Option<&mut DecodedPlane>,
) -> Result<(), &'static str> {
    let is_cfl = plane > 0 && syntax.uv_mode == Some(UvMode::Cfl);
    let (mode, angle_delta) = if plane == 0 {
        (syntax.y_mode, syntax.angle_y)
    } else if is_cfl {
        (IntraMode::Dc, 0)
    } else {
        let mode = syntax
            .uv_mode
            .and_then(uv_intra_mode)
            .ok_or("invalid chroma intra mode")?;
        (mode, syntax.angle_uv)
    };
    let qindex = (i32::from(syntax.qindex) + q_delta.dc).clamp(0, 255);
    let dc_q = dc_q(bit_depth, qindex);
    let ac_q = ac_q(bit_depth, qindex + q_delta.ac - q_delta.dc);
    let filter_type = intra_filter_type(above_mode, left_mode);
    let block_x = (syntax.block.col * 4) >> sub_x;
    let block_y = (syntax.block.row * 4) >> sub_y;

    for tx in &syntax.transforms {
        if tx.plane != plane {
            continue;
        }
        let plane_quant = quant_to_plane_order(&tx.quant, tx.tx);
        let w = tx.tx.width() as usize;
        let h = tx.tx.height() as usize;
        if tx.x + w > stride || frame.len() < (tx.y + h) * stride {
            return Err("transform extends beyond the reconstructed plane buffer");
        }
        let (have_above, have_left, have_above_right, have_below_left) =
            if let Some(grid) = decoded.as_deref() {
                let x4 = (tx.x >> 2) as isize;
                let y4 = (tx.y >> 2) as isize;
                (
                    grid.at(x4, y4 - 1),
                    grid.at(x4 - 1, y4),
                    grid.at(x4 + (w >> 2) as isize, y4 - 1),
                    grid.at(x4 - 1, y4 + (h >> 2) as isize),
                )
            } else {
                (
                    tx.y > 0,
                    tx.x > 0,
                    tx.x + w < plane_width,
                    tx.y + h < plane_height,
                )
            };
        let palette_prediction = predict_palette_plane(
            &syntax.palette,
            &syntax.palette_tokens,
            plane,
            block_x,
            block_y,
            tx.x,
            tx.y,
            w,
            h,
        )
        .map_err(|_| "invalid palette prediction")?;
        let prediction = if let Some(prediction) = palette_prediction {
            Some(prediction)
        } else if is_cfl {
            let luma = luma.ok_or("CFL prediction needs the reconstructed luma plane")?;
            let edges = IntraEdges::from_plane(
                frame,
                stride,
                tx.x,
                tx.y,
                w,
                h,
                have_above,
                have_left,
                have_above_right,
                have_below_left,
                plane_width - 1,
                plane_height - 1,
                bit_depth,
            );
            let dc = predict_intra(
                IntraMode::Dc,
                &edges,
                (w as u32).trailing_zeros(),
                (h as u32).trailing_zeros(),
                0,
                false,
                0,
                bit_depth,
            );
            let alpha = if plane == 1 {
                i32::from(syntax.cfl_alphas.alpha_u)
            } else {
                i32::from(syntax.cfl_alphas.alpha_v)
            };
            Some(cfl_predict(
                luma.samples,
                luma.stride,
                tx.x,
                tx.y,
                w,
                h,
                sub_x,
                sub_y,
                luma.width,
                luma.height,
                alpha,
                &dc,
                bit_depth,
            ))
        } else if plane == 0 {
            syntax.filter_intra.map(|filter| {
                let edges = IntraEdges::from_plane(
                    frame,
                    stride,
                    tx.x,
                    tx.y,
                    w,
                    h,
                    have_above,
                    have_left,
                    have_above_right,
                    have_below_left,
                    plane_width - 1,
                    plane_height - 1,
                    bit_depth,
                );
                predict_filter_intra(filter, &edges, w, h, bit_depth)
            })
        } else {
            None
        };
        if let Some(prediction) = prediction {
            let out = reconstruct_block(
                &plane_quant,
                w,
                h,
                tx.tx,
                tx.tx_type.row_kind(),
                tx.tx_type.col_kind(),
                dc_q,
                ac_q,
                &prediction,
                bit_depth,
                syntax.lossless,
            );
            for row in 0..h {
                let start = (tx.y + row) * stride + tx.x;
                frame[start..start + w].copy_from_slice(&out[row * w..(row + 1) * w]);
            }
            if let Some(grid) = decoded.as_deref_mut() {
                grid.mark(tx.x, tx.y, w, h);
            }
            continue;
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
                have_above,
                have_left,
                have_above_right,
                have_below_left,
                dc_q,
                ac_q,
                bit_depth,
                lossless: syntax.lossless,
            },
        );
        if let Some(grid) = decoded.as_deref_mut() {
            grid.mark(tx.x, tx.y, w, h);
        }
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
    fn decoded_grid_exposes_only_reconstructed_neighbours() {
        let mut luma = DecodedPlane::new(8, 8, 0, 0);
        assert!(!luma.at(0, 0));
        luma.mark(0, 0, 4, 4);
        assert!(luma.at(0, 0));
        assert!(!luma.at(1, 0));
        assert!(!luma.at(0, 1));
        luma.mark(4, 0, 4, 4);
        assert!(luma.at(1, 0));

        let chroma = DecodedPlane::new(8, 8, 1, 1);
        assert_eq!((chroma.cols, chroma.rows), (4, 4));
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
            delta_lf: [0; 4],
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
        reconstruct_intra_block_plane(
            &mut plane,
            8,
            8,
            8,
            0,
            1,
            1,
            &syntax,
            PlaneQDelta::default(),
            8,
            true,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(&plane[..4], &[128; 4]);
        assert_eq!(plane[4], 0);
    }

    #[test]
    fn syntax_palette_prediction_expands_to_plane_samples() {
        use crate::reference::av1::cfl::CflAlphas;
        use crate::reference::av1::intra_sink::IntraSyntaxTx;
        use crate::reference::av1::palette::{PaletteMap, PaletteModeInfo, PaletteTokens};
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
            delta_lf: [0; 4],
            y_mode: IntraMode::Dc,
            uv_mode: None,
            cfl_alphas: CflAlphas::default(),
            angle_y: 0,
            angle_uv: 0,
            filter_intra: None,
            palette: PaletteModeInfo {
                y_size: 2,
                y_colors: [10, 20, 0, 0, 0, 0, 0, 0],
                ..PaletteModeInfo::default()
            },
            palette_tokens: PaletteTokens {
                y: Some(PaletteMap {
                    width: 2,
                    height: 2,
                    indices: vec![0, 1, 1, 0],
                }),
                uv: None,
            },
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
        let mut plane = vec![0u16; 16];
        reconstruct_intra_block_plane(
            &mut plane,
            4,
            4,
            4,
            0,
            1,
            1,
            &syntax,
            PlaneQDelta::default(),
            8,
            false,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            plane,
            [10, 20, 20, 20, 20, 10, 10, 10, 20, 10, 10, 10, 20, 10, 10, 10]
        );
    }

    #[test]
    fn syntax_filter_intra_feeds_the_residual_reconstructor() {
        use crate::reference::av1::cfl::CflAlphas;
        use crate::reference::av1::filter_intra::FilterIntraMode;
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
            delta_lf: [0; 4],
            y_mode: IntraMode::Dc,
            uv_mode: None,
            cfl_alphas: CflAlphas::default(),
            angle_y: 0,
            angle_uv: 0,
            filter_intra: Some(FilterIntraMode::Paeth),
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
        let mut plane = vec![0u16; 64];
        reconstruct_intra_block_plane(
            &mut plane,
            8,
            8,
            8,
            0,
            1,
            1,
            &syntax,
            PlaneQDelta::default(),
            8,
            false,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(&plane[..4], &[128; 4]);
    }

    #[test]
    fn syntax_cfl_uses_the_reconstructed_luma_plane() {
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
            delta_lf: [0; 4],
            y_mode: IntraMode::Dc,
            uv_mode: Some(UvMode::Cfl),
            cfl_alphas: CflAlphas {
                joint_sign: 7,
                alpha_u: 2,
                alpha_v: -2,
                index: 0,
            },
            angle_y: 0,
            angle_uv: 0,
            filter_intra: None,
            palette: PaletteModeInfo::default(),
            palette_tokens: PaletteTokens { y: None, uv: None },
            tx: TxSize::Tx4x4,
            transforms: vec![IntraSyntaxTx {
                plane: 1,
                x: 0,
                y: 0,
                tx: TxSize::Tx4x4,
                tx_type: TxType::DctDct,
                all_zero: true,
                eob: 0,
                quant: vec![0; 16],
            }],
        };
        let luma = vec![100u16; 8 * 8];
        let mut chroma = vec![0u16; 16];
        reconstruct_intra_block_plane(
            &mut chroma,
            4,
            4,
            4,
            1,
            1,
            1,
            &syntax,
            PlaneQDelta::default(),
            8,
            false,
            None,
            None,
            Some(ReconstructedLuma {
                samples: &luma,
                stride: 8,
                width: 8,
                height: 8,
            }),
        )
        .unwrap();
        assert_eq!(chroma, [128; 16]);
    }
}
