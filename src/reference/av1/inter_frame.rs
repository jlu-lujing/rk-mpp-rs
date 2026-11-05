//! Pixel reconstruction for the first single-reference inter slice.
//!
//! The tile sink has already decoded the reference, mode and integer MV. This
//! module applies the normative motion-vector scaling and sample copy to the
//! reference frame and maintains the eight AV1 reference-frame slots after
//! post-processing.

use crate::reference::av1::cdf_state::CdfState;
use crate::reference::av1::coeff::coeff_q_context;
use crate::reference::av1::deblock::{
    DeblockCell, DeblockEdgeInfo, DeblockGrid, SEG_LVL_ALT_LF_Y_V,
};
use crate::reference::av1::frame_header::FrameHeader;
use crate::reference::av1::inter::{InterBlock, InterMode, MotionVector, INTRA_FRAME};
use crate::reference::av1::inter_sink::InterSyntaxBlock;
use crate::reference::av1::intra_frame::{
    apply_cdef_with_skip_grid, apply_deblock_grids, chroma_neighbor_modes, fill_modes,
    neighbor_modes, uv_mode, IntraFrame, PlaneBuffer,
};
use crate::reference::av1::quant::{ac_q, dc_q};
use crate::reference::av1::reconstruct::{
    quant_to_plane_order, reconstruct_block, reconstruct_intra_block_plane_with_grid, DecodedPlane,
    PlaneQDelta, ReconstructedLuma,
};
use crate::reference::av1::residual::{plane_tx_size, residual_size};
use crate::reference::av1::seq_header::SequenceHeader;

const REF_SCALE_SHIFT: u32 = 14;
const SCALE_SUBPEL_BITS: u32 = 10;
const SUBPEL_BITS: u32 = 4;

/// `Subpel_Filters[EIGHTTAP_REGULAR]`, 16 phases x 8 taps.
const REGULAR_8TAP: [[i16; 8]; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 2, -6, 126, 8, -2, 0, 0],
    [0, 2, -10, 122, 18, -4, 0, 0],
    [0, 2, -12, 116, 28, -8, 2, 0],
    [0, 2, -14, 110, 38, -10, 2, 0],
    [0, 2, -14, 102, 48, -12, 2, 0],
    [0, 2, -16, 94, 58, -12, 2, 0],
    [0, 2, -14, 84, 66, -12, 2, 0],
    [0, 2, -14, 76, 76, -14, 2, 0],
    [0, 2, -12, 66, 84, -14, 2, 0],
    [0, 2, -12, 58, 94, -16, 2, 0],
    [0, 2, -12, 48, 102, -14, 2, 0],
    [0, 2, -10, 38, 110, -14, 2, 0],
    [0, 2, -8, 28, 116, -12, 2, 0],
    [0, 0, -4, 18, 122, -10, 2, 0],
    [0, 0, -2, 8, 126, -6, 2, 0],
];

/// `Subpel_Filters[4]`, the four-tap regular filter used when a dimension is
/// at most four samples.
const REGULAR_4TAP: [[i16; 8]; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, -4, 126, 8, -2, 0, 0],
    [0, 0, -8, 122, 18, -4, 0, 0],
    [0, 0, -10, 116, 28, -6, 0, 0],
    [0, 0, -12, 110, 38, -8, 0, 0],
    [0, 0, -12, 102, 48, -10, 0, 0],
    [0, 0, -14, 94, 58, -10, 0, 0],
    [0, 0, -12, 84, 66, -10, 0, 0],
    [0, 0, -12, 76, 76, -12, 0, 0],
    [0, 0, -10, 66, 84, -12, 0, 0],
    [0, 0, -10, 58, 94, -14, 0, 0],
    [0, 0, -10, 48, 102, -12, 0, 0],
    [0, 0, -8, 38, 110, -12, 0, 0],
    [0, 0, -6, 28, 116, -10, 0, 0],
    [0, 0, -4, 18, 122, -8, 0, 0],
    [0, 0, -2, 8, 126, -4, 0, 0],
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterFrameError {
    MissingReference(i8),
    InvalidState(&'static str),
    FractionalMotionVector(MotionVector),
    MissingPlane(usize),
}

impl core::fmt::Display for InterFrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<&'static str> for InterFrameError {
    fn from(error: &'static str) -> Self {
        Self::InvalidState(error)
    }
}

/// Final post-filtered frames indexed by AV1's eight reference slots.
#[derive(Debug, Clone, Default)]
pub struct ReferenceFrameStore {
    slots: [Option<ReferenceSlot>; 8],
}

#[derive(Debug, Clone)]
struct ReferenceSlot {
    frame: IntraFrame,
    cdfs: CdfState,
}

impl ReferenceFrameStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, slot: usize) -> Option<&IntraFrame> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|slot| &slot.frame)
    }

    pub fn load_cdfs(&self, slot: usize) -> Option<CdfState> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|slot| slot.cdfs.clone())
    }

    pub fn refresh(&mut self, flags: u8, frame: &IntraFrame, base_q_idx: u8, cdfs: &CdfState) {
        let mut saved_cdfs = cdfs.clone();
        saved_cdfs.normalize_coeff_q_context(coeff_q_context(base_q_idx));
        saved_cdfs.reset_symbol_counters();
        for slot in 0..self.slots.len() {
            if flags & (1 << slot) != 0 {
                self.slots[slot] = Some(ReferenceSlot {
                    frame: frame.clone(),
                    cdfs: saved_cdfs.clone(),
                });
            }
        }
    }
}

fn round2_signed(value: i64, shift: u32) -> i64 {
    if shift == 0 {
        return value;
    }
    let offset = 1i64 << (shift - 1);
    if value >= 0 {
        (value + offset) >> shift
    } else {
        -((-value + offset) >> shift)
    }
}

fn round2(value: i64, shift: u32) -> i64 {
    if shift == 0 {
        value
    } else {
        (value + (1i64 << (shift - 1))) >> shift
    }
}

fn scale_for(current: u32, reference: u32) -> i64 {
    ((i64::from(reference) << REF_SCALE_SHIFT) + i64::from(current / 2)) / i64::from(current)
}

#[derive(Debug, Clone, Copy)]
struct MotionScale {
    start_x: i32,
    start_y: i32,
    step_x: i32,
    step_y: i32,
}

fn source_start(
    x: usize,
    y: usize,
    mv: MotionVector,
    sub_x: u32,
    sub_y: u32,
    current_width: u32,
    current_height: u32,
    reference_width: u32,
    reference_height: u32,
) -> MotionScale {
    let x_scale = scale_for(current_width, reference_width);
    let y_scale = scale_for(current_height, reference_height);
    let half_sample = 1i64 << (SUBPEL_BITS - 1);
    let orig_x = (x as i64) * (1 << SUBPEL_BITS) + ((2 * i64::from(mv.col)) >> sub_x) + half_sample;
    let orig_y = (y as i64) * (1 << SUBPEL_BITS) + ((2 * i64::from(mv.row)) >> sub_y) + half_sample;
    let base_x = orig_x * x_scale - (half_sample << REF_SCALE_SHIFT);
    let base_y = orig_y * y_scale - (half_sample << REF_SCALE_SHIFT);
    let off = 1i64 << (SCALE_SUBPEL_BITS - SUBPEL_BITS - 1);
    let start_x = round2_signed(base_x, REF_SCALE_SHIFT + SUBPEL_BITS - SCALE_SUBPEL_BITS) + off;
    let start_y = round2_signed(base_y, REF_SCALE_SHIFT + SUBPEL_BITS - SCALE_SUBPEL_BITS) + off;
    let step_x = round2_signed(x_scale, REF_SCALE_SHIFT - SCALE_SUBPEL_BITS) as i32;
    let step_y = round2_signed(y_scale, REF_SCALE_SHIFT - SCALE_SUBPEL_BITS) as i32;
    MotionScale {
        start_x: start_x as i32,
        start_y: start_y as i32,
        step_x,
        step_y,
    }
}

fn predict_inter_block(
    reference: &IntraFrame,
    current: &mut IntraFrame,
    plane: usize,
    block: InterBlock,
    block_col: usize,
    block_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
) -> Result<(), InterFrameError> {
    let reference_plane = match plane {
        0 => &reference.y,
        1 => reference
            .u
            .as_ref()
            .ok_or(InterFrameError::MissingPlane(plane))?,
        2 => reference
            .v
            .as_ref()
            .ok_or(InterFrameError::MissingPlane(plane))?,
        _ => return Err(InterFrameError::InvalidState("plane index")),
    };
    let current_plane = match plane {
        0 => &mut current.y,
        1 => current
            .u
            .as_mut()
            .ok_or(InterFrameError::MissingPlane(plane))?,
        2 => current
            .v
            .as_mut()
            .ok_or(InterFrameError::MissingPlane(plane))?,
        _ => return Err(InterFrameError::InvalidState("plane index")),
    };
    let scale = source_start(
        block_col,
        block_row,
        block.mv,
        sub_x,
        sub_y,
        current_plane.width as u32,
        current_plane.height as u32,
        reference_plane.width as u32,
        reference_plane.height as u32,
    );
    if block.interp_filter != [0, 0] {
        return Err(InterFrameError::InvalidState(
            "only EIGHTTAP_REGULAR is implemented",
        ));
    }
    let last_x = reference_plane.width.saturating_sub(1);
    let last_y = reference_plane.height.saturating_sub(1);
    let intermediate_height =
        (((height.saturating_sub(1) as i64 * i64::from(scale.step_y)) + 1023) >> 10) as usize + 8;
    let mut intermediate = vec![0i64; intermediate_height * width];
    let horizontal_filter = if width <= 4 {
        &REGULAR_4TAP
    } else {
        &REGULAR_8TAP
    };
    for row in 0..intermediate_height {
        for col in 0..width {
            let p = i64::from(scale.start_x) + i64::from(scale.step_x) * col as i64;
            let filter = horizontal_filter[((p >> 6) & 15) as usize];
            let src_y =
                (i64::from(scale.start_y >> 10) + row as i64 - 3).clamp(0, last_y as i64) as usize;
            let mut sum = 0i64;
            for (tap, coefficient) in filter.iter().enumerate() {
                let src_x = ((p >> 10) + tap as i64 - 3).clamp(0, last_x as i64) as usize;
                sum += i64::from(*coefficient)
                    * i64::from(reference_plane.data[src_y * reference_plane.stride + src_x]);
            }
            intermediate[row * width + col] = round2(sum, 3);
        }
    }
    let max = (1i32 << bit_depth) - 1;
    let vertical_filter = if height <= 4 {
        &REGULAR_4TAP
    } else {
        &REGULAR_8TAP
    };
    for row in 0..height {
        let p = i64::from(scale.start_y & 1023) + i64::from(scale.step_y) * row as i64;
        let filter = vertical_filter[((p >> 6) & 15) as usize];
        for col in 0..width {
            let mut sum = 0i64;
            for (tap, coefficient) in filter.iter().enumerate() {
                sum += i64::from(*coefficient)
                    * intermediate[((p >> 10) as usize + tap) * width + col];
            }
            let value = round2(sum, 11).clamp(0, i64::from(max)) as u16;
            let dst_x = block_col + col;
            let dst_y = block_row + row;
            current_plane.data[dst_y * current_plane.stride + dst_x] = value;
        }
    }
    Ok(())
}

/// Reconstruct an inter frame made entirely of single-reference blocks.
pub fn reconstruct_inter_frame(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    blocks: &[InterSyntaxBlock],
    references: &ReferenceFrameStore,
) -> Result<IntraFrame, InterFrameError> {
    if header.frame_is_intra {
        return Err(InterFrameError::InvalidState(
            "inter driver received intra frame",
        ));
    }
    let width = header.mi_cols * 4;
    let height = header.mi_rows * 4;
    let sub_x = u32::from(sequence.color.subsampling_x);
    let sub_y = u32::from(sequence.color.subsampling_y);
    let chroma_width = width >> sub_x;
    let chroma_height = height >> sub_y;
    let bit_depth = u32::from(sequence.color.bit_depth);
    let mut frame = IntraFrame {
        coded_width: width,
        coded_height: height,
        viewport_width: header.upscaled_width as usize,
        viewport_height: header.frame_height as usize,
        bit_depth,
        y: PlaneBuffer::new(width, height),
        u: (!sequence.color.mono_chrome).then(|| PlaneBuffer::new(chroma_width, chroma_height)),
        v: (!sequence.color.mono_chrome).then(|| PlaneBuffer::new(chroma_width, chroma_height)),
    };
    let mut y_modes = vec![None; header.mi_cols * header.mi_rows];
    let mut uv_modes = vec![None; header.mi_cols * header.mi_rows];
    let mut motion_grid = vec![None; header.mi_cols * header.mi_rows];
    let mut y_decoded = DecodedPlane::new(header.mi_cols, header.mi_rows, 0, 0);
    let mut u_decoded = DecodedPlane::new(header.mi_cols, header.mi_rows, sub_x, sub_y);
    let mut v_decoded = DecodedPlane::new(header.mi_cols, header.mi_rows, sub_x, sub_y);
    let y_delta = PlaneQDelta {
        dc: header.delta_q_y_dc,
        ac: 0,
    };
    let u_delta = PlaneQDelta {
        dc: header.delta_q_u_dc,
        ac: header.delta_q_u_ac,
    };
    let v_delta = PlaneQDelta {
        dc: header.delta_q_v_dc,
        ac: header.delta_q_v_ac,
    };

    for block in blocks {
        if let Some(inter) = block.inter {
            let ref_slot = header.ref_frame_idx[(inter.ref_frame - 1) as usize] as usize;
            let reference = references
                .get(ref_slot)
                .ok_or(InterFrameError::MissingReference(inter.ref_frame))?;
            let plane_widths = [width, chroma_width, chroma_width];
            let plane_heights = [height, chroma_height, chroma_height];
            let planes = if sequence.color.mono_chrome { 1 } else { 3 };
            for row in block.block.row
                ..(block.block.row + block.block.size.num_4x4_high() as usize).min(header.mi_rows)
            {
                for col in block.block.col
                    ..(block.block.col + block.block.size.num_4x4_wide() as usize)
                        .min(header.mi_cols)
                {
                    motion_grid[row * header.mi_cols + col] = Some(inter);
                }
            }
            for plane in 0..planes {
                let plane_sub_x = if plane == 0 { 0 } else { sub_x };
                let plane_sub_y = if plane == 0 { 0 } else { sub_y };
                let plane_size = if plane == 0 {
                    block.block.size
                } else {
                    let Some(size) = residual_size(block.block.size, sub_x, sub_y) else {
                        continue;
                    };
                    size
                };
                let block_col = (block.block.col >> plane_sub_x) * 4;
                let block_row = (block.block.row >> plane_sub_y) * 4;
                let block_width =
                    (plane_size.width as usize).min(plane_widths[plane].saturating_sub(block_col));
                let block_height = (plane_size.height as usize)
                    .min(plane_heights[plane].saturating_sub(block_row));
                if plane == 0 {
                    predict_inter_block(
                        reference,
                        &mut frame,
                        plane,
                        inter,
                        block_col,
                        block_row,
                        block_width,
                        block_height,
                        plane_sub_x,
                        plane_sub_y,
                        bit_depth,
                    )?;
                    continue;
                }

                let pred_width = (block.block.size.width as usize >> plane_sub_x).max(1);
                let pred_height = (block.block.size.height as usize >> plane_sub_y).max(1);
                let cand_row = (block.block.row >> plane_sub_y) << plane_sub_y;
                let cand_col = (block.block.col >> plane_sub_x) << plane_sub_x;
                let some_use_intra =
                    (0..plane_size.height as usize)
                        .step_by(pred_height)
                        .any(|y| {
                            let row = cand_row + y / pred_height;
                            (0..plane_size.width as usize).step_by(pred_width).any(|x| {
                                let col = cand_col + x / pred_width;
                                motion_grid[row * header.mi_cols + col].is_none()
                            })
                        });
                if some_use_intra {
                    predict_inter_block(
                        reference,
                        &mut frame,
                        plane,
                        inter,
                        block_col,
                        block_row,
                        block_width,
                        block_height,
                        plane_sub_x,
                        plane_sub_y,
                        bit_depth,
                    )?;
                    continue;
                }
                for (r, y) in (0..plane_size.height as usize)
                    .step_by(pred_height)
                    .enumerate()
                {
                    for (c, x) in (0..plane_size.width as usize)
                        .step_by(pred_width)
                        .enumerate()
                    {
                        let candidate = motion_grid[(cand_row + r) * header.mi_cols + cand_col + c]
                            .ok_or(InterFrameError::InvalidState("missing chroma MV"))?;
                        let candidate_ref =
                            header.ref_frame_idx[(candidate.ref_frame - 1) as usize] as usize;
                        let candidate_reference = references
                            .get(candidate_ref)
                            .ok_or(InterFrameError::MissingReference(candidate.ref_frame))?;
                        predict_inter_block(
                            candidate_reference,
                            &mut frame,
                            plane,
                            candidate,
                            block_col + x,
                            block_row + y,
                            pred_width.min(block_width.saturating_sub(x)),
                            pred_height.min(block_height.saturating_sub(y)),
                            plane_sub_x,
                            plane_sub_y,
                            bit_depth,
                        )?;
                    }
                }
            }
            for tx in &block.transforms {
                let (plane, dc_delta, ac_delta) = match tx.plane {
                    0 => (&mut frame.y, header.delta_q_y_dc, 0),
                    1 => (
                        frame.u.as_mut().ok_or(InterFrameError::MissingPlane(1))?,
                        header.delta_q_u_dc,
                        header.delta_q_u_ac,
                    ),
                    2 => (
                        frame.v.as_mut().ok_or(InterFrameError::MissingPlane(2))?,
                        header.delta_q_v_dc,
                        header.delta_q_v_ac,
                    ),
                    _ => return Err(InterFrameError::InvalidState("plane index")),
                };
                let qindex = (i32::from(block.qindex) + dc_delta).clamp(0, 255);
                let dc_q_value = dc_q(bit_depth, qindex);
                let ac_q_value = ac_q(bit_depth, qindex + ac_delta - dc_delta);
                let tx_width = tx.tx.width() as usize;
                let tx_height = tx.tx.height() as usize;
                let mut prediction = Vec::with_capacity(tx_width * tx_height);
                for row in 0..tx_height {
                    let start = (tx.y + row) * plane.stride + tx.x;
                    prediction.extend_from_slice(&plane.data[start..start + tx_width]);
                }
                let quant = quant_to_plane_order(&tx.quant, tx.tx);
                let reconstructed = reconstruct_block(
                    &quant,
                    tx_width,
                    tx_height,
                    tx.tx,
                    tx.tx_type.row_kind(),
                    tx.tx_type.col_kind(),
                    dc_q_value,
                    ac_q_value,
                    &prediction,
                    bit_depth,
                    block.lossless,
                );
                for row in 0..tx_height {
                    let start = (tx.y + row) * plane.stride + tx.x;
                    plane.data[start..start + tx_width]
                        .copy_from_slice(&reconstructed[row * tx_width..(row + 1) * tx_width]);
                }
            }
            let plane_widths = [width, chroma_width, chroma_width];
            let plane_heights = [height, chroma_height, chroma_height];
            for plane in 0..if sequence.color.mono_chrome { 1 } else { 3 } {
                let plane_sub_x = if plane == 0 { 0 } else { sub_x };
                let plane_sub_y = if plane == 0 { 0 } else { sub_y };
                let plane_size = if plane == 0 {
                    block.block.size
                } else {
                    let Some(size) = residual_size(block.block.size, sub_x, sub_y) else {
                        continue;
                    };
                    size
                };
                let x = (block.block.col >> plane_sub_x) * 4;
                let y = (block.block.row >> plane_sub_y) * 4;
                let w = (plane_size.width as usize).min(plane_widths[plane].saturating_sub(x));
                let h = (plane_size.height as usize).min(plane_heights[plane].saturating_sub(y));
                match plane {
                    0 => y_decoded.mark(x, y, w, h),
                    1 => u_decoded.mark(x, y, w, h),
                    _ => v_decoded.mark(x, y, w, h),
                }
                for r in block.block.row
                    ..(block.block.row + block.block.size.num_4x4_high() as usize)
                        .min(header.mi_rows)
                {
                    for c in block.block.col
                        ..(block.block.col + block.block.size.num_4x4_wide() as usize)
                            .min(header.mi_cols)
                    {
                        y_modes[r * header.mi_cols + c] = None;
                        uv_modes[r * header.mi_cols + c] = None;
                    }
                }
            }
        } else if let Some(intra) = &block.intra {
            fill_modes(&mut y_modes, header.mi_cols, intra, Some(intra.y_mode));
            fill_modes(
                &mut uv_modes,
                header.mi_cols,
                intra,
                intra.uv_mode.and_then(uv_mode),
            );
            let (y_above, y_left) = neighbor_modes(&y_modes, header.mi_cols, intra);
            reconstruct_intra_block_plane_with_grid(
                &mut frame.y.data,
                frame.y.stride,
                frame.y.width,
                frame.y.height,
                0,
                sub_x,
                sub_y,
                intra,
                y_delta,
                bit_depth,
                sequence.enable_intra_edge_filter,
                y_above,
                y_left,
                None,
                Some(&mut y_decoded),
            )?;
            let (uv_above, uv_left) = chroma_neighbor_modes(
                &uv_modes,
                header.mi_cols,
                header.mi_rows,
                intra,
                sub_x,
                sub_y,
            );
            if intra.transforms.iter().any(|tx| tx.plane == 1) {
                let u = frame.u.as_mut().ok_or(InterFrameError::MissingPlane(1))?;
                reconstruct_intra_block_plane_with_grid(
                    &mut u.data,
                    u.stride,
                    u.width,
                    u.height,
                    1,
                    sub_x,
                    sub_y,
                    intra,
                    u_delta,
                    bit_depth,
                    sequence.enable_intra_edge_filter,
                    uv_above,
                    uv_left,
                    Some(ReconstructedLuma {
                        samples: &frame.y.data,
                        stride: frame.y.stride,
                        width: frame.y.width,
                        height: frame.y.height,
                    }),
                    Some(&mut u_decoded),
                )?;
            }
            if intra.transforms.iter().any(|tx| tx.plane == 2) {
                let v = frame.v.as_mut().ok_or(InterFrameError::MissingPlane(2))?;
                reconstruct_intra_block_plane_with_grid(
                    &mut v.data,
                    v.stride,
                    v.width,
                    v.height,
                    2,
                    sub_x,
                    sub_y,
                    intra,
                    v_delta,
                    bit_depth,
                    sequence.enable_intra_edge_filter,
                    uv_above,
                    uv_left,
                    Some(ReconstructedLuma {
                        samples: &frame.y.data,
                        stride: frame.y.stride,
                        width: frame.y.width,
                        height: frame.y.height,
                    }),
                    Some(&mut v_decoded),
                )?;
            }
        } else {
            return Err(InterFrameError::InvalidState("block has no prediction"));
        }
    }
    Ok(frame)
}

fn deblock_grid(
    header: &FrameHeader,
    blocks: &[InterSyntaxBlock],
    vertical: bool,
    plane: usize,
    sub_x: u32,
    sub_y: u32,
) -> DeblockGrid {
    let (plane_sub_x, plane_sub_y) = if plane == 0 { (0, 0) } else { (sub_x, sub_y) };
    let cell = DeblockCell {
        info: DeblockEdgeInfo::default(),
        apply_filter: false,
        filter_size: 4,
    };
    let mut grid = DeblockGrid::uniform(header.mi_cols, header.mi_rows, cell);
    let mut tx_extents = vec![0usize; header.mi_cols * header.mi_rows];
    for block in blocks {
        let w = block.block.size.num_4x4_wide() as usize;
        let h = block.block.size.num_4x4_high() as usize;
        let fallback_tx =
            plane_tx_size(block.block.size, plane, block.tx, plane_sub_x, plane_sub_y)
                .unwrap_or(block.tx);
        let extent = if vertical {
            fallback_tx.width() as usize
        } else {
            fallback_tx.height() as usize
        };
        for row in block.block.row..(block.block.row + h).min(header.mi_rows) {
            for col in block.block.col..(block.block.col + w).min(header.mi_cols) {
                tx_extents[row * header.mi_cols + col] = extent;
            }
        }
        if plane == 0 {
            for tx in block.transforms.iter().filter(|tx| tx.plane == 0) {
                let extent = if vertical {
                    tx.tx.width() as usize
                } else {
                    tx.tx.height() as usize
                };
                let w = tx.tx.width() as usize / 4;
                let h = tx.tx.height() as usize / 4;
                let base_row = tx.y / 4;
                let base_col = tx.x / 4;
                for row in base_row..(base_row + h).min(header.mi_rows) {
                    for col in base_col..(base_col + w).min(header.mi_cols) {
                        tx_extents[row * header.mi_cols + col] = extent;
                    }
                }
            }
        }
    }
    for block in blocks {
        let w = block.block.size.num_4x4_wide() as usize;
        let h = block.block.size.num_4x4_high() as usize;
        let block_extent = if vertical {
            (block.block.size.width as usize) >> plane_sub_x
        } else {
            (block.block.size.height as usize) >> plane_sub_y
        };
        let mut seg_lvl_alt_lf = [0i32; 4];
        let mut has_seg_lvl_alt_lf = false;
        for (feature, value) in seg_lvl_alt_lf.iter_mut().enumerate() {
            let feature = SEG_LVL_ALT_LF_Y_V + feature;
            if header.feature_enabled[block.segment_id][feature] {
                *value = header.feature_data[block.segment_id][feature];
                has_seg_lvl_alt_lf = true;
            }
        }
        let info = DeblockEdgeInfo {
            ref_frame: block
                .inter
                .map_or(INTRA_FRAME as usize, |inter| inter.ref_frame as usize),
            mode_type: block
                .inter
                .map_or(0, |inter| usize::from(inter.mode != InterMode::Global)),
            delta_lf: block.delta_lf,
            seg_lvl_alt_lf: has_seg_lvl_alt_lf.then_some(seg_lvl_alt_lf),
        };
        for row in block.block.row..(block.block.row + h).min(header.mi_rows) {
            for col in block.block.col..(block.block.col + w).min(header.mi_cols) {
                let coordinate = if vertical {
                    (((col & !((1 << plane_sub_x) - 1))
                        - (block.block.col & !((1 << plane_sub_x) - 1)))
                        * 4)
                        >> plane_sub_x
                } else {
                    (((row & !((1 << plane_sub_y) - 1))
                        - (block.block.row & !((1 << plane_sub_y) - 1)))
                        * 4)
                        >> plane_sub_y
                };
                let current_tx_extent = tx_extents[row * header.mi_cols + col];
                let col_step = 1usize << plane_sub_x;
                let row_step = 1usize << plane_sub_y;
                let prev_tx_extent =
                    if (vertical && col < col_step) || (!vertical && row < row_step) {
                        current_tx_extent
                    } else {
                        let (prev_row, prev_col) = if vertical {
                            (row, col - col_step)
                        } else {
                            (row - row_step, col)
                        };
                        tx_extents[prev_row * header.mi_cols + prev_col]
                    };
                grid.cells[row * header.mi_cols + col] = DeblockCell::from_geometry(
                    coordinate,
                    plane,
                    block_extent,
                    current_tx_extent,
                    prev_tx_extent,
                    block.skip,
                    true,
                    info,
                );
            }
        }
    }
    grid
}

pub fn apply_inter_deblock(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    blocks: &[InterSyntaxBlock],
    frame: &mut IntraFrame,
) {
    let sub_x = u32::from(sequence.color.subsampling_x);
    let sub_y = u32::from(sequence.color.subsampling_y);
    let vertical = (0..3)
        .map(|plane| deblock_grid(header, blocks, true, plane, sub_x, sub_y))
        .collect::<Vec<_>>();
    let horizontal = (0..3)
        .map(|plane| deblock_grid(header, blocks, false, plane, sub_x, sub_y))
        .collect::<Vec<_>>();
    apply_deblock_grids(header, &vertical, &horizontal, sub_x, sub_y, frame);
}

pub fn apply_inter_cdef(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    blocks: &[InterSyntaxBlock],
    cdef_grid: &crate::reference::av1::cdef::CdefIndexGrid,
    frame: &mut IntraFrame,
) {
    let mut skip_grid = vec![false; header.mi_cols * header.mi_rows];
    for block in blocks {
        let width = block.block.size.num_4x4_wide() as usize;
        let height = block.block.size.num_4x4_high() as usize;
        for row in block.block.row..(block.block.row + height).min(header.mi_rows) {
            for col in block.block.col..(block.block.col + width).min(header.mi_cols) {
                skip_grid[row * header.mi_cols + col] = block.skip;
            }
        }
    }
    apply_cdef_with_skip_grid(sequence, header, &skip_grid, cdef_grid, frame);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::cdf_state::CdfState;
    use crate::reference::av1::frame_header::PRIMARY_REF_NONE;
    use crate::reference::av1::inter::InterMode;
    use crate::reference::av1::inter_sink::{InterTileBlockSink, InterTileConfig};
    use crate::reference::av1::intra_frame::{
        apply_intra_cdef, apply_intra_deblock, apply_intra_loop_restoration,
        reconstruct_intra_frame, upscale_frame,
    };
    use crate::reference::av1::intra_sink::{IntraTileBlockSink, IntraTileConfig};
    use crate::reference::av1::obu::parse_ivf;
    use crate::reference::av1::stream::{ReferenceStream, StreamEvent};
    use crate::reference::av1::tile_entropy::decode_tile_partitions_with_cdfs;

    #[test]
    fn signed_rounding_matches_the_spec() {
        assert_eq!(round2_signed(15, 4), 1);
        assert_eq!(round2_signed(16, 4), 1);
        assert_eq!(round2_signed(-15, 4), -1);
        assert_eq!(round2_signed(-16, 4), -1);
    }

    #[test]
    fn zero_motion_copies_the_same_sample() {
        let frame = IntraFrame {
            coded_width: 4,
            coded_height: 4,
            viewport_width: 4,
            viewport_height: 4,
            bit_depth: 8,
            y: PlaneBuffer {
                data: (0..64).collect(),
                width: 4,
                height: 4,
                stride: 8,
            },
            u: None,
            v: None,
        };
        let mut out = frame.clone();
        predict_inter_block(
            &frame,
            &mut out,
            0,
            InterBlock {
                ref_frame: 1,
                mode: crate::reference::av1::inter::InterMode::Nearest,
                mv: MotionVector::default(),
                motion_mode: crate::reference::av1::inter::MotionMode::Simple,
                interp_filter: [0, 0],
                ref_mv_idx: 0,
            },
            0,
            0,
            4,
            4,
            0,
            0,
            8,
        )
        .unwrap();
        assert_eq!(out.y.data, frame.y.data);
    }

    fn visible_hash(plane: &PlaneBuffer) -> u64 {
        plane
            .data
            .chunks_exact(plane.stride)
            .take(plane.height)
            .flat_map(|row| &row[..plane.width])
            .fold(0xcbf29ce484222325u64, |hash, sample| {
                (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
            })
    }

    fn decode_two_picture_stream(data: &[u8]) -> (Vec<IntraFrame>, Vec<InterSyntaxBlock>) {
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        let mut references = ReferenceFrameStore::new();
        let mut pictures = Vec::new();
        let mut inter_blocks = Vec::new();

        for event in events.iter().flatten() {
            let StreamEvent::CodedFrame(frame) = event else {
                continue;
            };
            let initial_cdfs = if frame.header.primary_ref_frame == PRIMARY_REF_NONE {
                CdfState::new()
            } else {
                let slot =
                    frame.header.ref_frame_idx[frame.header.primary_ref_frame as usize] as usize;
                references.load_cdfs(slot).unwrap()
            };
            let tile = &frame.layout.tiles[0];
            let final_cdfs;
            let picture = if frame.header.frame_is_intra {
                let config = IntraTileConfig::from_headers(sequence, &frame.header);
                let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
                let decoded = decode_tile_partitions_with_cdfs(
                    &frame.payload[tile.bytes.clone()],
                    &frame.header,
                    tile.tile,
                    sb_size,
                    false,
                    true,
                    initial_cdfs,
                    IntraTileBlockSink::new(config),
                )
                .unwrap();
                final_cdfs = decoded.cdfs;
                let mut picture =
                    reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks).unwrap();
                if frame
                    .header
                    .loop_filter_level
                    .iter()
                    .any(|level| *level != 0)
                {
                    apply_intra_deblock(
                        sequence,
                        &frame.header,
                        &decoded.sink.blocks,
                        &mut picture,
                    );
                }
                let deblocked = picture.clone();
                if sequence.enable_cdef && !frame.header.coded_lossless {
                    apply_intra_cdef(
                        sequence,
                        &frame.header,
                        &decoded.sink.blocks,
                        decoded.sink.cdef_grid(),
                        &mut picture,
                    );
                }
                if frame.header.use_superres {
                    picture = upscale_frame(sequence, &frame.header, &picture).unwrap();
                }
                apply_intra_loop_restoration(
                    sequence,
                    &frame.header,
                    decoded.sink.lr_params(),
                    &deblocked,
                    &mut picture,
                )
                .unwrap();
                picture
            } else {
                let config = InterTileConfig::from_headers(sequence, &frame.header);
                let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
                let decoded = decode_tile_partitions_with_cdfs(
                    &frame.payload[tile.bytes.clone()],
                    &frame.header,
                    tile.tile,
                    sb_size,
                    false,
                    true,
                    initial_cdfs,
                    InterTileBlockSink::new(config),
                )
                .unwrap();
                inter_blocks = decoded.sink.blocks.clone();
                final_cdfs = decoded.cdfs;
                let mut picture = reconstruct_inter_frame(
                    sequence,
                    &frame.header,
                    &decoded.sink.blocks,
                    &references,
                )
                .unwrap();
                if frame
                    .header
                    .loop_filter_level
                    .iter()
                    .any(|level| *level != 0)
                {
                    apply_inter_deblock(
                        sequence,
                        &frame.header,
                        &decoded.sink.blocks,
                        &mut picture,
                    );
                }
                let deblocked = picture.clone();
                if sequence.enable_cdef && !frame.header.coded_lossless {
                    apply_inter_cdef(
                        sequence,
                        &frame.header,
                        &decoded.sink.blocks,
                        decoded.sink.cdef_grid(),
                        &mut picture,
                    );
                }
                if frame.header.use_superres {
                    picture = upscale_frame(sequence, &frame.header, &picture).unwrap();
                }
                apply_intra_loop_restoration(
                    sequence,
                    &frame.header,
                    decoded.sink.lr_params(),
                    &deblocked,
                    &mut picture,
                )
                .unwrap();
                picture
            };
            references.refresh(
                frame.header.refresh_frame_flags,
                &picture,
                frame.header.base_q_idx,
                &final_cdfs,
            );
            pictures.push(picture);
        }
        (pictures, inter_blocks)
    }

    #[test]
    fn real_integer_inter_frame_matches_libaom_and_locks_the_block_modes() {
        let data = include_bytes!("../../../tests/data/av1_inter_box_320x240.ivf");
        let (pictures, inter_blocks) = decode_two_picture_stream(data);

        assert_eq!(pictures.len(), 2);
        assert_eq!(
            (
                visible_hash(&pictures[0].y),
                visible_hash(pictures[0].u.as_ref().unwrap()),
                visible_hash(pictures[0].v.as_ref().unwrap())
            ),
            (0xc30180323dd09b41, 0x930bef4f94807f25, 0x930bef4f94807f25)
        );
        assert_eq!(
            (
                visible_hash(&pictures[1].y),
                visible_hash(pictures[1].u.as_ref().unwrap()),
                visible_hash(pictures[1].v.as_ref().unwrap())
            ),
            (0xdd5a7af1044fd871, 0x930bef4f94807f25, 0x930bef4f94807f25)
        );

        assert_eq!(inter_blocks.len(), 20);
        assert_eq!(
            inter_blocks
                .iter()
                .filter_map(|block| {
                    let inter = block.inter?;
                    Some((
                        block.block.row,
                        block.block.col,
                        inter.mode,
                        inter.mv.row,
                        inter.mv.col,
                    ))
                })
                .collect::<Vec<_>>(),
            [
                (0, 0, InterMode::Nearest, 0, 0),
                (0, 16, InterMode::Nearest, 0, 0),
                (0, 32, InterMode::Nearest, 0, 0),
                (0, 48, InterMode::Nearest, 0, 0),
                (0, 64, InterMode::Nearest, 0, 0),
                (16, 0, InterMode::New, 0, -64),
                (16, 16, InterMode::New, 0, -64),
                (16, 32, InterMode::Nearest, 0, 0),
                (16, 48, InterMode::Nearest, 0, 0),
                (16, 64, InterMode::Nearest, 0, 0),
                (32, 0, InterMode::Nearest, 0, -64),
                (32, 16, InterMode::Nearest, 0, -64),
                (32, 32, InterMode::Nearest, 0, 0),
                (32, 48, InterMode::Nearest, 0, 0),
                (32, 64, InterMode::Nearest, 0, 0),
                (48, 0, InterMode::Nearest, 0, -64),
                (48, 16, InterMode::Nearest, 0, -64),
                (48, 32, InterMode::Nearest, 0, 0),
                (48, 48, InterMode::Nearest, 0, 0),
                (48, 64, InterMode::Nearest, 0, 0),
            ]
        );
    }

    #[test]
    fn real_fractional_inter_and_residuals_match_libaom_pixels() {
        let data = include_bytes!("../../../tests/data/av1_inter_fractional_residual_320x240.ivf");
        let (pictures, inter_blocks) = decode_two_picture_stream(data);

        assert_eq!(pictures.len(), 2);
        assert_eq!(
            (
                visible_hash(&pictures[0].y),
                visible_hash(pictures[0].u.as_ref().unwrap()),
                visible_hash(pictures[0].v.as_ref().unwrap())
            ),
            (0xa7dd92b772324c1f, 0x34bf8b29c9daf66a, 0x9f3b0aa82b1ca27)
        );
        assert_eq!(
            (
                visible_hash(&pictures[1].y),
                visible_hash(pictures[1].u.as_ref().unwrap()),
                visible_hash(pictures[1].v.as_ref().unwrap())
            ),
            (0x76b3e62fbd628b74, 0x36abc1779df2b070, 0x1d942f4aa8d82eb5)
        );

        assert_eq!(inter_blocks.len(), 327);
        let block = inter_blocks
            .iter()
            .find(|block| block.block.row == 16 && block.block.col == 32)
            .unwrap();
        let inter = block.inter.unwrap();
        assert_eq!(inter.mode, InterMode::New);
        assert_eq!(inter.ref_mv_idx, 2);
        assert_eq!((inter.mv.row, inter.mv.col), (192, 0));
        assert!(block.transforms.is_empty());
    }

    #[test]
    fn real_inter_frame_matches_libaom_after_deblock_and_cdef() {
        let data = include_bytes!("../../../tests/data/av1_inter_filters_320x240.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        let headers = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(&frame.header),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(headers.len(), 2);
        assert!(headers
            .iter()
            .any(|header| { header.loop_filter_level.iter().any(|level| *level != 0) }));
        assert!(sequence.enable_cdef);
        assert!(!headers[1].coded_lossless);
        assert!(headers[1]
            .cdef_y_pri_strength
            .iter()
            .chain(headers[1].cdef_uv_pri_strength.iter())
            .any(|strength| *strength != 0));
        assert!(!headers[1].uses_lr);

        let (pictures, inter_blocks) = decode_two_picture_stream(data);
        assert_eq!(pictures.len(), 2);
        assert_eq!(
            (
                visible_hash(&pictures[0].y),
                visible_hash(pictures[0].u.as_ref().unwrap()),
                visible_hash(pictures[0].v.as_ref().unwrap())
            ),
            (0xa81ca8f8720415ba, 0x493bd4ca00ff198a, 0x29fd6cd8d60687c4)
        );
        assert_eq!(
            (
                visible_hash(&pictures[1].y),
                visible_hash(pictures[1].u.as_ref().unwrap()),
                visible_hash(pictures[1].v.as_ref().unwrap())
            ),
            (0x7e12b58bee2e77e1, 0x254c13003bd29764, 0x5c6c680c955ff66b)
        );
        assert_eq!(inter_blocks.len(), 255);
    }

    #[test]
    fn real_inter_frame_applies_loop_restoration_and_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_lr_320x240.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let headers = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(&frame.header),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(headers.len(), 2);
        assert!(headers[1].uses_lr);

        let (pictures, inter_blocks) = decode_two_picture_stream(data);
        assert_eq!(pictures.len(), 2);
        assert_eq!(
            (
                visible_hash(&pictures[0].y),
                visible_hash(pictures[0].u.as_ref().unwrap()),
                visible_hash(pictures[0].v.as_ref().unwrap())
            ),
            (0x136f825011c9e4c1, 0xdd42c406c78ee0fd, 0xdc7484640931f714)
        );
        assert_eq!(
            (
                visible_hash(&pictures[1].y),
                visible_hash(pictures[1].u.as_ref().unwrap()),
                visible_hash(pictures[1].v.as_ref().unwrap())
            ),
            (0x793843d54eb6ac5f, 0xd52b385eb4c9d513, 0x338cc0e91d047c1e)
        );
        assert_eq!(inter_blocks.len(), 242);
    }
}
