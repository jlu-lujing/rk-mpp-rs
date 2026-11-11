//! Pixel reconstruction for AV1 inter frames.
//!
//! The tile sink has already decoded the references, modes and MVs. This module
//! applies normative motion-vector scaling, filtered prediction, residual
//! reconstruction and average compound blending, then maintains the eight AV1
//! reference-frame slots after post-processing.

use crate::reference::av1::cdef::CdefIndexGrid;
use crate::reference::av1::cdf_state::CdfState;
use crate::reference::av1::coeff::coeff_q_context;
use crate::reference::av1::deblock::{
    DeblockCell, DeblockEdgeInfo, DeblockGrid, SEG_LVL_ALT_LF_Y_V,
};
use crate::reference::av1::frame_header::{
    get_relative_dist, FilmGrainParams, FrameHeader, PRIMARY_REF_NONE,
};
use crate::reference::av1::inter::{
    CompoundType, InterBlock, InterIntraMode, InterMode, MotionMode, MotionVector, ALTREF2_FRAME,
    ALTREF_FRAME, BWDREF_FRAME, GOLDEN_FRAME, INTRA_FRAME, LAST2_FRAME, LAST_FRAME, NONE_FRAME,
};
use crate::reference::av1::inter_sink::{
    InterNeighbourState, InterSyntaxBlock, InterTileBlockSink, InterTileConfig,
};
use crate::reference::av1::intra_frame::{
    apply_cdef_with_skip_grid, apply_deblock_grids, chroma_neighbor_modes, fill_modes,
    merge_lr_params, neighbor_modes, tile_of, uv_mode, IntraFrame, PlaneBuffer,
};
use crate::reference::av1::lr_frame::LrFrameParams;
use crate::reference::av1::partition::{block_size_index, BlockSize};
use crate::reference::av1::predict::{intra_filter_type, predict_intra, IntraEdges};
use crate::reference::av1::quant::{ac_q, dc_q};
use crate::reference::av1::reconstruct::{
    quant_to_plane_order, reconstruct_block, reconstruct_intra_block_plane_with_grid, DecodedPlane,
    PlaneQDelta, ReconstructedLuma,
};
use crate::reference::av1::residual::{plane_tx_size, residual_size};
use crate::reference::av1::segment::SegmentMap;
use crate::reference::av1::seq_header::SequenceHeader;
use crate::reference::av1::temporal::{
    block_position, project_mv, FrameMotionField, FrameMv, TemporalMotionField, TemporalMv,
    REFMVS_LIMIT,
};
use crate::reference::av1::tile::DecodedBlock;
use crate::reference::av1::tile_entropy::{
    decode_tile_partitions_with_cdfs, tile_bounds, TileEntropyError,
};
use crate::reference::av1::tile_group::ParsedTileGroup;
use crate::reference::av1::warped::{warp_block_res, warp_block_samples, WarpParams};

const REF_SCALE_SHIFT: u32 = 14;
const SCALE_SUBPEL_BITS: u32 = 10;
const SUBPEL_BITS: u32 = 4;
const FILTER_BITS: u32 = 7;
const ROUND0_BITS: u32 = 3;
const COMPOUND_ROUND1_BITS: u32 = 7;
const SINGLE_ROUND1_BITS: u32 = 2 * FILTER_BITS - ROUND0_BITS;
const II_WEIGHTS_1D: [u8; 128] = [
    60, 58, 56, 54, 52, 50, 48, 47, 45, 44, 42, 41, 39, 38, 37, 35, 34, 33, 32, 31, 30, 29, 28, 27,
    26, 25, 24, 23, 22, 22, 21, 20, 19, 19, 18, 18, 17, 16, 16, 15, 15, 14, 14, 13, 13, 12, 12, 12,
    11, 11, 10, 10, 10, 9, 9, 9, 8, 8, 8, 8, 7, 7, 7, 7, 6, 6, 6, 6, 6, 5, 5, 5, 5, 5, 4, 4, 4, 4,
    4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];
const II_SIZE_SCALES: [u8; 22] = [
    32, 16, 16, 16, 8, 8, 8, 4, 4, 4, 2, 2, 2, 1, 1, 1, 8, 8, 4, 4, 2, 2,
];

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

/// `av1_sub_pel_filters_8smooth`.
const SMOOTH_8TAP: [[i16; 8]; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 2, 28, 62, 34, 2, 0, 0],
    [0, 0, 26, 62, 36, 4, 0, 0],
    [0, 0, 22, 62, 40, 4, 0, 0],
    [0, 0, 20, 60, 42, 6, 0, 0],
    [0, 0, 18, 58, 44, 8, 0, 0],
    [0, 0, 16, 56, 46, 10, 0, 0],
    [0, -2, 16, 54, 48, 12, 0, 0],
    [0, -2, 14, 52, 52, 14, -2, 0],
    [0, 0, 12, 48, 54, 16, -2, 0],
    [0, 0, 10, 46, 56, 16, 0, 0],
    [0, 0, 8, 44, 58, 18, 0, 0],
    [0, 0, 6, 42, 60, 20, 0, 0],
    [0, 0, 4, 40, 62, 22, 0, 0],
    [0, 0, 4, 36, 62, 26, 0, 0],
    [0, 0, 2, 34, 62, 28, 2, 0],
];

/// `av1_sub_pel_filters_8sharp`.
const SHARP_8TAP: [[i16; 8]; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [-2, 2, -6, 126, 8, -2, 2, 0],
    [-2, 6, -12, 124, 16, -6, 4, -2],
    [-2, 8, -18, 120, 26, -10, 6, -2],
    [-4, 10, -22, 116, 38, -14, 6, -2],
    [-4, 10, -22, 108, 48, -18, 8, -2],
    [-4, 10, -24, 100, 60, -20, 8, -2],
    [-4, 10, -24, 90, 70, -22, 10, -2],
    [-4, 12, -24, 80, 80, -24, 12, -4],
    [-2, 10, -22, 70, 90, -24, 10, -4],
    [-2, 8, -20, 60, 100, -24, 10, -4],
    [-2, 8, -18, 48, 108, -22, 10, -4],
    [-2, 6, -14, 38, 116, -22, 10, -4],
    [-2, 6, -10, 26, 120, -18, 8, -2],
    [-2, 4, -6, 16, 124, -12, 6, -2],
    [0, 2, -2, 8, 126, -6, 2, -2],
];

/// `av1_bilinear_filters`.
const BILINEAR_8TAP: [[i16; 8]; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, 0, 120, 8, 0, 0, 0],
    [0, 0, 0, 112, 16, 0, 0, 0],
    [0, 0, 0, 104, 24, 0, 0, 0],
    [0, 0, 0, 96, 32, 0, 0, 0],
    [0, 0, 0, 88, 40, 0, 0, 0],
    [0, 0, 0, 80, 48, 0, 0, 0],
    [0, 0, 0, 72, 56, 0, 0, 0],
    [0, 0, 0, 64, 64, 0, 0, 0],
    [0, 0, 0, 56, 72, 0, 0, 0],
    [0, 0, 0, 48, 80, 0, 0, 0],
    [0, 0, 0, 40, 88, 0, 0, 0],
    [0, 0, 0, 32, 96, 0, 0, 0],
    [0, 0, 0, 24, 104, 0, 0, 0],
    [0, 0, 0, 16, 112, 0, 0, 0],
    [0, 0, 0, 8, 120, 0, 0, 0],
];

/// `av1_sub_pel_filters_4smooth`.
const SMOOTH_4TAP: [[i16; 8]; 16] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, 30, 62, 34, 2, 0, 0],
    [0, 0, 26, 62, 36, 4, 0, 0],
    [0, 0, 22, 62, 40, 4, 0, 0],
    [0, 0, 20, 60, 42, 6, 0, 0],
    [0, 0, 18, 58, 44, 8, 0, 0],
    [0, 0, 16, 56, 46, 10, 0, 0],
    [0, 0, 14, 54, 48, 12, 0, 0],
    [0, 0, 12, 52, 52, 12, 0, 0],
    [0, 0, 12, 48, 54, 14, 0, 0],
    [0, 0, 10, 46, 56, 16, 0, 0],
    [0, 0, 8, 44, 58, 18, 0, 0],
    [0, 0, 6, 42, 60, 20, 0, 0],
    [0, 0, 4, 40, 62, 22, 0, 0],
    [0, 0, 4, 36, 62, 26, 0, 0],
    [0, 0, 2, 34, 62, 30, 0, 0],
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
    order_hint: u8,
    motion_field: Option<FrameMotionField>,
    segment_map: SegmentMap,
    film_grain: FilmGrainParams,
}

fn build_frame_motion_field(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    blocks: &[InterSyntaxBlock],
    reference_order_hints: [u8; 7],
) -> FrameMotionField {
    let mvs_cols = (header.mi_cols + 1) >> 1;
    let mvs_rows = (header.mi_rows + 1) >> 1;
    let mut cells = vec![None; mvs_cols * mvs_rows];
    for syntax in blocks {
        let width = syntax.block.size.num_4x4_wide() as usize;
        let height = syntax.block.size.num_4x4_high() as usize;
        let start_row = syntax.block.row >> 1;
        let start_col = syntax.block.col >> 1;
        let cell_rows = (height + 1) >> 1;
        let cell_cols = (width + 1) >> 1;
        // `av1_copy_frame_mvs()` clears every motion-field cell covered by a
        // syntax block before deciding whether that block contributes an MV.
        // This matters when a later intra block shares a half-resolution cell
        // with an earlier inter block: leaving the earlier MV behind fabricates
        // an invalid temporal candidate and changes `mode_context`.
        for row in start_row..(start_row + cell_rows).min(mvs_rows) {
            for col in start_col..(start_col + cell_cols).min(mvs_cols) {
                cells[row * mvs_cols + col] = None;
            }
        }
        let Some(inter) = syntax.inter else {
            continue;
        };
        let mut selected = None;
        for (list, reference) in [inter.ref_frame, inter.ref_frame1].into_iter().enumerate() {
            if reference <= INTRA_FRAME {
                continue;
            }
            let side = if sequence.enable_order_hint {
                get_relative_dist(
                    i32::from(reference_order_hints[(reference - 1) as usize]),
                    i32::from(header.order_hint),
                    true,
                    sequence.order_hint_bits,
                )
            } else {
                0
            };
            if side > 0
                || (sequence.enable_order_hint
                    && reference_order_hints[(reference - 1) as usize] == header.order_hint)
            {
                continue;
            }
            let mv = if list == 0 { inter.mv } else { inter.mv1 };
            if mv.row.abs() > REFMVS_LIMIT || mv.col.abs() > REFMVS_LIMIT {
                continue;
            }
            selected = Some(FrameMv {
                ref_frame: reference,
                mv,
            });
        }
        if let Some(selected) = selected {
            for row in start_row..(start_row + cell_rows).min(mvs_rows) {
                for col in start_col..(start_col + cell_cols).min(mvs_cols) {
                    cells[row * mvs_cols + col] = Some(selected);
                }
            }
        }
    }
    FrameMotionField {
        mi_rows: header.mi_rows,
        mi_cols: header.mi_cols,
        ref_order_hints: reference_order_hints,
        cells,
    }
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

    pub fn get_order_hint(&self, slot: usize) -> Option<u8> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|slot| slot.order_hint)
    }

    pub fn get_film_grain(&self, slot: usize) -> Option<&FilmGrainParams> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|slot| &slot.film_grain)
    }

    pub fn load_cdfs(&self, slot: usize) -> Option<CdfState> {
        let mut cdfs = self
            .slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|slot| slot.cdfs.clone())?;
        cdfs.reset_symbol_counters();
        Some(cdfs)
    }

    /// The primary-reference segment map used by inter temporal prediction.
    pub fn primary_segment_map(&self, header: &FrameHeader) -> Option<SegmentMap> {
        if !header.segmentation_enabled || header.primary_ref_frame == PRIMARY_REF_NONE {
            return None;
        }
        let slot = usize::from(header.ref_frame_idx[header.primary_ref_frame as usize]);
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|slot| slot.segment_map.clone())
            .filter(|map| map.mi_cols == header.mi_cols && map.mi_rows == header.mi_rows)
    }

    pub fn temporal_motion(
        &self,
        sequence: &SequenceHeader,
        header: &FrameHeader,
    ) -> TemporalMotionField {
        let mvs_cols = (header.mi_cols + 1) >> 1;
        let mvs_rows = ((header.mi_rows + 16) >> 1).max((header.mi_rows + 1) >> 1);
        let mut field = TemporalMotionField {
            mi_rows: header.mi_rows,
            mi_cols: header.mi_cols,
            mvs_rows,
            mvs_cols,
            current_order_hint: header.order_hint,
            enable_order_hint: sequence.enable_order_hint,
            order_hint_bits: sequence.order_hint_bits,
            ref_order_hints: [0; 7],
            cells: vec![None; mvs_rows * mvs_cols],
        };
        for (index, slot) in header.ref_frame_idx.iter().enumerate() {
            field.ref_order_hints[index] = self.get_order_hint(usize::from(*slot)).unwrap_or(0);
        }
        if !header.use_ref_frame_mvs || !sequence.enable_order_hint {
            return field;
        }

        let current_ref_order_hints = field.ref_order_hints;
        let mut ref_stamp = 2;
        let last_slot = usize::from(header.ref_frame_idx[(LAST_FRAME - 1) as usize]);
        if let Some(start) = self.slots.get(last_slot).and_then(Option::as_ref) {
            let is_lst_overlay = start
                .motion_field
                .as_ref()
                .map(|motion| {
                    motion.ref_order_hints[(ALTREF_FRAME - 1) as usize]
                        == current_ref_order_hints[(GOLDEN_FRAME - 1) as usize]
                })
                .unwrap_or(false);
            if !is_lst_overlay {
                let _ = self.project_motion_field(sequence, header, LAST_FRAME, 2, &mut field);
            }
            ref_stamp -= 1;
        }
        for (reference, direction) in [(BWDREF_FRAME, 0), (ALTREF2_FRAME, 0), (ALTREF_FRAME, 0)] {
            let reference_hint = current_ref_order_hints[(reference - 1) as usize];
            if ref_stamp < 0 {
                break;
            }
            if get_relative_dist(
                i32::from(reference_hint),
                i32::from(header.order_hint),
                true,
                sequence.order_hint_bits,
            ) > 0
                && self.project_motion_field(sequence, header, reference, direction, &mut field)
            {
                ref_stamp -= 1;
            }
        }
        if ref_stamp >= 0 {
            let _ = self.project_motion_field(sequence, header, LAST2_FRAME, 2, &mut field);
        }
        field
    }

    fn project_motion_field(
        &self,
        sequence: &SequenceHeader,
        header: &FrameHeader,
        start_frame: i8,
        direction: i32,
        output: &mut TemporalMotionField,
    ) -> bool {
        let slot = usize::from(header.ref_frame_idx[(start_frame - 1) as usize]);
        let Some(start) = self.slots.get(slot).and_then(Option::as_ref) else {
            return false;
        };
        let Some(motion) = start.motion_field.as_ref() else {
            return false;
        };
        if motion.mi_rows != header.mi_rows || motion.mi_cols != header.mi_cols {
            return false;
        }
        let mut start_to_current = get_relative_dist(
            i32::from(start.order_hint),
            i32::from(header.order_hint),
            true,
            sequence.order_hint_bits,
        );
        if direction == 2 {
            start_to_current = -start_to_current;
        }
        if start_to_current.abs() > 31 {
            return false;
        }
        let start_cols = (motion.mi_cols + 1) >> 1;
        let start_rows = (motion.mi_rows + 1) >> 1;
        for block_row in 0..start_rows {
            for block_col in 0..start_cols {
                let Some(candidate) = motion.cells[block_row * start_cols + block_col] else {
                    continue;
                };
                let reference_hint = motion.ref_order_hints[(candidate.ref_frame - 1) as usize];
                let reference_offset = get_relative_dist(
                    i32::from(start.order_hint),
                    i32::from(reference_hint),
                    true,
                    sequence.order_hint_bits,
                );
                if reference_offset <= 0 || reference_offset > 31 {
                    continue;
                }
                let projected = project_mv(candidate.mv, start_to_current, reference_offset);
                let Some((row, col)) = block_position(
                    block_row,
                    block_col,
                    projected,
                    direction >> 1,
                    header.mi_rows,
                    header.mi_cols,
                ) else {
                    continue;
                };
                let index = row * output.mvs_cols + col;
                output.cells[index] = Some(TemporalMv {
                    mv: candidate.mv,
                    ref_frame_offset: reference_offset,
                });
            }
        }
        true
    }

    pub fn refresh(
        &mut self,
        sequence: &SequenceHeader,
        header: &FrameHeader,
        blocks: &[InterSyntaxBlock],
        frame: &IntraFrame,
        cdfs: &CdfState,
        segment_map: SegmentMap,
    ) {
        let mut reference_order_hints = [0u8; 7];
        for (index, slot) in header.ref_frame_idx.iter().enumerate() {
            reference_order_hints[index] = self.get_order_hint(usize::from(*slot)).unwrap_or(0);
        }
        let motion_field = (!header.frame_is_intra)
            .then(|| build_frame_motion_field(sequence, header, blocks, reference_order_hints));
        let mut saved_cdfs = cdfs.clone();
        saved_cdfs.normalize_coeff_q_context(coeff_q_context(header.base_q_idx));
        saved_cdfs.reset_symbol_counters();
        for slot in 0..self.slots.len() {
            if header.refresh_frame_flags & (1 << slot) != 0 {
                self.slots[slot] = Some(ReferenceSlot {
                    frame: frame.clone(),
                    cdfs: saved_cdfs.clone(),
                    order_hint: header.order_hint,
                    motion_field: motion_field.clone(),
                    segment_map: segment_map.clone(),
                    film_grain: header.film_grain.clone(),
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

fn interpolation_filter(
    filter: u8,
    length: usize,
) -> Result<&'static [[i16; 8]; 16], InterFrameError> {
    match (filter, length <= 4) {
        (0, true) | (2, true) => Ok(&REGULAR_4TAP),
        (1, true) => Ok(&SMOOTH_4TAP),
        (0, false) => Ok(&REGULAR_8TAP),
        (1, false) => Ok(&SMOOTH_8TAP),
        (2, false) => Ok(&SHARP_8TAP),
        (3, _) => Ok(&BILINEAR_8TAP),
        _ => Err(InterFrameError::InvalidState("interpolation filter")),
    }
}

fn inter_block_res(
    reference: &IntraFrame,
    plane: usize,
    current_width: usize,
    current_height: usize,
    block: InterBlock,
    block_col: usize,
    block_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
    round1: u32,
) -> Result<Vec<i64>, InterFrameError> {
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
    let scale = source_start(
        block_col,
        block_row,
        block.mv,
        sub_x,
        sub_y,
        current_width as u32,
        current_height as u32,
        reference_plane.width as u32,
        reference_plane.height as u32,
    );
    let last_x = reference_plane.width.saturating_sub(1);
    let last_y = reference_plane.height.saturating_sub(1);
    let intermediate_height =
        (((height.saturating_sub(1) as i64 * i64::from(scale.step_y)) + 1023) >> 10) as usize + 8;
    let mut intermediate = vec![0i64; intermediate_height * width];
    let horizontal_filter = interpolation_filter(block.interp_filter[1], width)?;
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
            let horizontal_bias = 1i64 << (bit_depth + FILTER_BITS - 1);
            intermediate[row * width + col] = round2(sum + horizontal_bias, ROUND0_BITS);
        }
    }
    let vertical_filter = interpolation_filter(block.interp_filter[0], height)?;
    let mut out = vec![0i64; width * height];
    for row in 0..height {
        let p = i64::from(scale.start_y & 1023) + i64::from(scale.step_y) * row as i64;
        let filter = vertical_filter[((p >> 6) & 15) as usize];
        for col in 0..width {
            let offset_bits = bit_depth + 2 * FILTER_BITS - ROUND0_BITS;
            let mut sum = 1i64 << offset_bits;
            for (tap, coefficient) in filter.iter().enumerate() {
                sum += i64::from(*coefficient)
                    * intermediate[((p >> 10) as usize + tap) * width + col];
            }
            out[row * width + col] = round2(sum, round1);
        }
    }
    Ok(out)
}

fn convolution_round_offset(bit_depth: u32, round1: u32) -> i64 {
    let offset_bits = bit_depth + 2 * FILTER_BITS - ROUND0_BITS;
    (1i64 << (offset_bits - round1)) + (1i64 << (offset_bits - round1 - 1))
}

fn inter_block_samples(
    reference: &IntraFrame,
    plane: usize,
    current_width: usize,
    current_height: usize,
    block: InterBlock,
    block_col: usize,
    block_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
) -> Result<Vec<u16>, InterFrameError> {
    let max = (1i64 << bit_depth) - 1;
    let round_offset = convolution_round_offset(bit_depth, SINGLE_ROUND1_BITS);
    Ok(inter_block_res(
        reference,
        plane,
        current_width,
        current_height,
        block,
        block_col,
        block_row,
        width,
        height,
        sub_x,
        sub_y,
        bit_depth,
        SINGLE_ROUND1_BITS,
    )?
    .into_iter()
    .map(|res| (res - round_offset).clamp(0, max) as u16)
    .collect())
}

fn difference_weighted_mask(
    res0: &[i64],
    res1: &[i64],
    bit_depth: u32,
    mask_type: u8,
) -> Result<Vec<u8>, InterFrameError> {
    if !matches!(mask_type, 0 | 1) {
        return Err(InterFrameError::InvalidState(
            "difference-weighted mask type",
        ));
    }
    let round = 2 * FILTER_BITS - ROUND0_BITS - COMPOUND_ROUND1_BITS + bit_depth.saturating_sub(8);
    Ok(res0
        .iter()
        .zip(res1)
        .map(|(left, right)| {
            let difference = round2((left - right).abs(), round);
            let mask = (38 + difference / 16).clamp(0, 64) as u8;
            if mask_type == 0 {
                mask
            } else {
                64 - mask
            }
        })
        .collect())
}

fn sample_compound_mask(
    mask: &[u8],
    stride: usize,
    row: usize,
    col: usize,
    sub_x: u32,
    sub_y: u32,
) -> u8 {
    match (sub_x != 0, sub_y != 0) {
        (false, false) => mask[row * stride + col],
        (true, false) => round2(
            i64::from(mask[row * stride + 2 * col]) + i64::from(mask[row * stride + 2 * col + 1]),
            1,
        ) as u8,
        (false, true) => round2(
            i64::from(mask[(2 * row) * stride + col])
                + i64::from(mask[(2 * row + 1) * stride + col]),
            1,
        ) as u8,
        (true, true) => round2(
            i64::from(mask[(2 * row) * stride + 2 * col])
                + i64::from(mask[(2 * row + 1) * stride + 2 * col])
                + i64::from(mask[(2 * row) * stride + 2 * col + 1])
                + i64::from(mask[(2 * row + 1) * stride + 2 * col + 1]),
            2,
        ) as u8,
    }
}

fn neighbor_modes_at(
    grid: &[Option<crate::reference::av1::intra::IntraMode>],
    cols: usize,
    row: usize,
    col: usize,
) -> (
    Option<crate::reference::av1::intra::IntraMode>,
    Option<crate::reference::av1::intra::IntraMode>,
) {
    let above = (row > 0).then(|| grid[(row - 1) * cols + col]).flatten();
    let left = (col > 0).then(|| grid[row * cols + col - 1]).flatten();
    (above, left)
}

fn chroma_neighbor_modes_at(
    grid: &[Option<crate::reference::av1::intra::IntraMode>],
    cols: usize,
    rows: usize,
    row: usize,
    col: usize,
    sub_x: u32,
    sub_y: u32,
) -> (
    Option<crate::reference::av1::intra::IntraMode>,
    Option<crate::reference::av1::intra::IntraMode>,
) {
    let mut above_row = row as isize - 1;
    let mut above_col = col as isize;
    if sub_x != 0 && col & 1 == 0 {
        above_col += 1;
    }
    if sub_y != 0 && row & 1 != 0 {
        above_row -= 1;
    }

    let mut left_row = row as isize;
    let mut left_col = col as isize - 1;
    if sub_x != 0 && col & 1 != 0 {
        left_col -= 1;
    }
    if sub_y != 0 && row & 1 == 0 {
        left_row += 1;
    }

    let at = |row: isize, col: isize| {
        (row >= 0 && col >= 0 && row < rows as isize && col < cols as isize)
            .then(|| grid[row as usize * cols + col as usize])
            .flatten()
    };
    (at(above_row, above_col), at(left_row, left_col))
}

fn smooth_interintra_mask(
    mode: InterIntraMode,
    size: BlockSize,
    width: usize,
    height: usize,
) -> Vec<u8> {
    let scale = usize::from(II_SIZE_SCALES[block_size_index(size)]);
    let mut mask = vec![0u8; width * height];
    for row in 0..height {
        for col in 0..width {
            let index = match mode {
                InterIntraMode::V => row * scale,
                InterIntraMode::H => col * scale,
                InterIntraMode::Smooth => row.min(col) * scale,
                InterIntraMode::Dc => {
                    mask[row * width + col] = 32;
                    continue;
                }
            };
            mask[row * width + col] = II_WEIGHTS_1D[index];
        }
    }
    mask
}

#[allow(clippy::too_many_arguments)]
fn blend_interintra(
    current: &mut IntraFrame,
    plane: usize,
    interintra: crate::reference::av1::inter::InterIntra,
    block_size: BlockSize,
    plane_size: BlockSize,
    block_mi_row: usize,
    block_mi_col: usize,
    mi_cols: usize,
    mi_rows: usize,
    block_col: usize,
    block_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
    enable_intra_edge_filter: bool,
    filter_type: u8,
) -> Result<(), InterFrameError> {
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
    let have_above = block_mi_row > 0;
    let have_left = block_mi_col > 0;
    let have_above_right = block_mi_col + (block_size.num_4x4_wide() as usize) < mi_cols;
    let have_below_left = block_mi_row + (block_size.num_4x4_high() as usize) < mi_rows;
    let edges = IntraEdges::from_plane(
        &current_plane.data,
        current_plane.stride,
        block_col,
        block_row,
        width,
        height,
        have_above,
        have_left,
        have_above_right,
        have_below_left,
        current_plane.width - 1,
        current_plane.height - 1,
        bit_depth,
    );
    debug_assert!(width.is_power_of_two() && height.is_power_of_two());
    let intra = predict_intra(
        interintra.mode.intra_mode(),
        &edges,
        width.trailing_zeros(),
        height.trailing_zeros(),
        0,
        enable_intra_edge_filter,
        filter_type,
        bit_depth,
    );
    let wedge = interintra
        .wedge_index
        .map(|index| {
            crate::reference::av1::wedge::wedge_mask(block_size, index, 0)
                .map(|mask| (mask, block_size.width as usize))
        })
        .transpose()
        .map_err(InterFrameError::InvalidState)?;
    let smooth = wedge
        .is_none()
        .then(|| smooth_interintra_mask(interintra.mode, plane_size, width, height));
    for row in 0..height {
        for col in 0..width {
            let weight = if let Some((mask, stride)) = &wedge {
                i64::from(sample_compound_mask(mask, *stride, row, col, sub_x, sub_y))
            } else {
                i64::from(smooth.as_ref().expect("smooth mask")[row * width + col])
            };
            let dst = (block_row + row) * current_plane.stride + block_col + col;
            let inter = i64::from(current_plane.data[dst]);
            let prediction = round2(
                weight * i64::from(intra[row * width + col]) + (64 - weight) * inter,
                6,
            );
            current_plane.data[dst] = prediction as u16;
        }
    }
    Ok(())
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
    let current_width = match plane {
        0 => current.y.width,
        1 => {
            current
                .u
                .as_ref()
                .ok_or(InterFrameError::MissingPlane(plane))?
                .width
        }
        2 => {
            current
                .v
                .as_ref()
                .ok_or(InterFrameError::MissingPlane(plane))?
                .width
        }
        _ => return Err(InterFrameError::InvalidState("plane index")),
    };
    let current_height = match plane {
        0 => current.y.height,
        1 => {
            current
                .u
                .as_ref()
                .ok_or(InterFrameError::MissingPlane(plane))?
                .height
        }
        2 => {
            current
                .v
                .as_ref()
                .ok_or(InterFrameError::MissingPlane(plane))?
                .height
        }
        _ => return Err(InterFrameError::InvalidState("plane index")),
    };
    let samples = inter_block_samples(
        reference,
        plane,
        current_width,
        current_height,
        block,
        block_col,
        block_row,
        width,
        height,
        sub_x,
        sub_y,
        bit_depth,
    )?;
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
    for row in 0..height {
        let start = (block_row + row) * current_plane.stride + block_col;
        current_plane.data[start..start + width]
            .copy_from_slice(&samples[row * width..(row + 1) * width]);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn predict_warped_block(
    reference: &IntraFrame,
    current: &mut IntraFrame,
    plane: usize,
    params: WarpParams,
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
    let samples = warp_block_samples(
        &reference_plane.data,
        reference_plane.stride,
        reference_plane.width,
        reference_plane.height,
        params,
        block_col,
        block_row,
        width,
        height,
        sub_x,
        sub_y,
        bit_depth,
    )
    .map_err(InterFrameError::InvalidState)?;
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
    for row in 0..height {
        let start = (block_row + row) * current_plane.stride + block_col;
        current_plane.data[start..start + width]
            .copy_from_slice(&samples[row * width..(row + 1) * width]);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn inter_or_warp_res(
    reference: &IntraFrame,
    plane: usize,
    current_width: usize,
    current_height: usize,
    block: InterBlock,
    block_col: usize,
    block_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
    round1: u32,
) -> Result<Vec<i64>, InterFrameError> {
    if width >= 8 && height >= 8 {
        if let Some(params) = block.warp {
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
            return warp_block_res(
                &reference_plane.data,
                reference_plane.stride,
                reference_plane.width,
                reference_plane.height,
                params,
                block_col,
                block_row,
                width,
                height,
                sub_x,
                sub_y,
                bit_depth,
                Some(round1),
            )
            .map_err(InterFrameError::InvalidState);
        }
    }
    inter_block_res(
        reference,
        plane,
        current_width,
        current_height,
        block,
        block_col,
        block_row,
        width,
        height,
        sub_x,
        sub_y,
        bit_depth,
        round1,
    )
}

fn predict_compound_block(
    reference0: &IntraFrame,
    reference1: &IntraFrame,
    current: &mut IntraFrame,
    plane: usize,
    first: InterBlock,
    second: InterBlock,
    block_col: usize,
    block_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
    compound_type: CompoundType,
) -> Result<(), InterFrameError> {
    let (current_width, current_height) = match plane {
        0 => (current.y.width, current.y.height),
        1 => {
            let current_plane = current
                .u
                .as_ref()
                .ok_or(InterFrameError::MissingPlane(plane))?;
            (current_plane.width, current_plane.height)
        }
        2 => {
            let current_plane = current
                .v
                .as_ref()
                .ok_or(InterFrameError::MissingPlane(plane))?;
            (current_plane.width, current_plane.height)
        }
        _ => return Err(InterFrameError::InvalidState("plane index")),
    };
    let res0 = inter_or_warp_res(
        reference0,
        plane,
        current_width,
        current_height,
        first,
        block_col,
        block_row,
        width,
        height,
        sub_x,
        sub_y,
        bit_depth,
        COMPOUND_ROUND1_BITS,
    )?;
    let res1 = inter_or_warp_res(
        reference1,
        plane,
        current_width,
        current_height,
        second,
        block_col,
        block_row,
        width,
        height,
        sub_x,
        sub_y,
        bit_depth,
        COMPOUND_ROUND1_BITS,
    )?;
    let distance_weights = if compound_type == CompoundType::DistanceWeighted {
        let weights = first
            .distance_weights
            .ok_or(InterFrameError::InvalidState("distance weights"))?;
        if u16::from(weights[0]) + u16::from(weights[1]) != 16 {
            return Err(InterFrameError::InvalidState("distance weights"));
        }
        Some(weights)
    } else {
        None
    };
    let mask = match compound_type {
        CompoundType::Average => None,
        CompoundType::DifferenceWeighted { mask_type } => {
            if plane == 0 {
                Some((
                    difference_weighted_mask(&res0, &res1, bit_depth, mask_type)?,
                    width,
                ))
            } else {
                let luma_col = block_col << sub_x;
                let luma_row = block_row << sub_y;
                let luma_width = (width << sub_x).min(current.y.width.saturating_sub(luma_col));
                let luma_height = (height << sub_y).min(current.y.height.saturating_sub(luma_row));
                let luma_res0 = inter_or_warp_res(
                    reference0,
                    0,
                    current.y.width,
                    current.y.height,
                    first,
                    luma_col,
                    luma_row,
                    luma_width,
                    luma_height,
                    0,
                    0,
                    bit_depth,
                    COMPOUND_ROUND1_BITS,
                )?;
                let luma_res1 = inter_or_warp_res(
                    reference1,
                    0,
                    current.y.width,
                    current.y.height,
                    second,
                    luma_col,
                    luma_row,
                    luma_width,
                    luma_height,
                    0,
                    0,
                    bit_depth,
                    COMPOUND_ROUND1_BITS,
                )?;
                Some((
                    difference_weighted_mask(&luma_res0, &luma_res1, bit_depth, mask_type)?,
                    luma_width,
                ))
            }
        }
        CompoundType::DistanceWeighted => None,
        CompoundType::Wedge { index, sign } => Some((
            crate::reference::av1::wedge::wedge_mask(first.size, index, sign)
                .map_err(InterFrameError::InvalidState)?,
            first.size.width as usize,
        )),
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
    let round_offset = convolution_round_offset(bit_depth, COMPOUND_ROUND1_BITS);
    let round_bits = 2 * FILTER_BITS - ROUND0_BITS - COMPOUND_ROUND1_BITS;
    for row in 0..height {
        for col in 0..width {
            let index = row * width + col;
            let max = (1i64 << bit_depth) - 1;
            let blended = if let Some(weights) = distance_weights {
                (i64::from(weights[0]) * res0[index] + i64::from(weights[1]) * res1[index]) >> 4
            } else {
                match &mask {
                    Some((mask, mask_stride)) => {
                        let weight = i64::from(sample_compound_mask(
                            mask,
                            *mask_stride,
                            row,
                            col,
                            sub_x,
                            sub_y,
                        ));
                        (weight * res0[index] + (64 - weight) * res1[index]) >> 6
                    }
                    None => (res0[index] + res1[index]) >> 1,
                }
            };
            let value = round2(blended - round_offset, round_bits).clamp(0, max) as u16;
            let dst = (block_row + row) * current_plane.stride + block_col + col;
            current_plane.data[dst] = value;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn predict_inter_or_compound_block(
    reference0: &IntraFrame,
    reference1: Option<&IntraFrame>,
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
    enable_intra_edge_filter: bool,
    filter_type: u8,
    block_mi_row: usize,
    block_mi_col: usize,
    mi_cols: usize,
    mi_rows: usize,
) -> Result<(), InterFrameError> {
    if let Some(interintra) = block.interintra {
        predict_inter_block(
            reference0, current, plane, block, block_col, block_row, width, height, sub_x, sub_y,
            bit_depth,
        )?;
        let plane_size = if plane == 0 {
            block.size
        } else {
            residual_size(block.size, sub_x, sub_y)
                .ok_or(InterFrameError::InvalidState("invalid inter-intra size"))?
        };
        blend_interintra(
            current,
            plane,
            interintra,
            block.size,
            plane_size,
            block_mi_row,
            block_mi_col,
            mi_cols,
            mi_rows,
            block_col,
            block_row,
            width,
            height,
            sub_x,
            sub_y,
            bit_depth,
            enable_intra_edge_filter,
            filter_type,
        )
    } else if let Some(reference1) = reference1 {
        let second = InterBlock {
            ref_frame: block.ref_frame1,
            mv: block.mv1,
            warp: block.warp1,
            warp1: None,
            ..block
        };
        predict_compound_block(
            reference0,
            reference1,
            current,
            plane,
            block,
            second,
            block_col,
            block_row,
            width,
            height,
            sub_x,
            sub_y,
            bit_depth,
            block
                .compound_type
                .ok_or(InterFrameError::InvalidState("compound type"))?,
        )
    } else {
        if width >= 8 && height >= 8 {
            if let Some(params) = block.warp {
                return predict_warped_block(
                    reference0, current, plane, params, block_col, block_row, width, height, sub_x,
                    sub_y, bit_depth,
                );
            }
        }
        predict_inter_block(
            reference0, current, plane, block, block_col, block_row, width, height, sub_x, sub_y,
            bit_depth,
        )
    }
}

#[derive(Clone, Copy)]
struct ObmcNeighbor {
    rel_row: usize,
    rel_col: usize,
    width_mi: usize,
    height_mi: usize,
    inter: InterBlock,
}

fn obmc_mask(length: usize) -> Option<&'static [u8]> {
    const MASK_1: [u8; 1] = [64];
    const MASK_2: [u8; 2] = [45, 64];
    const MASK_4: [u8; 4] = [39, 50, 59, 64];
    const MASK_8: [u8; 8] = [36, 42, 48, 53, 57, 61, 64, 64];
    const MASK_16: [u8; 16] = [
        34, 37, 40, 43, 46, 49, 52, 54, 56, 58, 60, 61, 64, 64, 64, 64,
    ];
    const MASK_32: [u8; 32] = [
        33, 35, 36, 38, 40, 41, 43, 44, 45, 47, 48, 50, 51, 52, 53, 55, 56, 57, 58, 59, 60, 60, 61,
        62, 64, 64, 64, 64, 64, 64, 64, 64,
    ];
    const MASK_64: [u8; 64] = [
        33, 34, 35, 35, 36, 37, 38, 39, 40, 40, 41, 42, 43, 44, 44, 44, 45, 46, 47, 47, 48, 49, 50,
        51, 51, 51, 52, 52, 53, 54, 55, 56, 56, 56, 57, 57, 58, 58, 59, 60, 60, 60, 60, 60, 61, 62,
        62, 62, 62, 62, 63, 63, 63, 63, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
    ];
    Some(match length {
        1 => &MASK_1,
        2 => &MASK_2,
        4 => &MASK_4,
        8 => &MASK_8,
        16 => &MASK_16,
        32 => &MASK_32,
        64 => &MASK_64,
        _ => return None,
    })
}

fn obmc_neighbors_above(
    block: DecodedBlock,
    motion_grid: &[Option<InterBlock>],
    size_grid: &[Option<BlockSize>],
    mi_cols: usize,
) -> Vec<ObmcNeighbor> {
    if block.row == 0 {
        return Vec::new();
    }
    let width = block.size.num_4x4_wide() as usize;
    let end_col = (block.col + width).min(mi_cols);
    let mut col = block.col;
    let mut result = Vec::new();
    while col < end_col {
        let mut lookup_col = col;
        let initial = (block.row - 1) * mi_cols + lookup_col;
        let mut step = size_grid[initial]
            .map_or(1, |size| size.num_4x4_wide() as usize)
            .min(16);
        if step == 1 {
            col &= !1;
            lookup_col = col + 1;
            if lookup_col >= mi_cols {
                break;
            }
            step = 2;
        }
        let index = (block.row - 1) * mi_cols + lookup_col;
        if let Some(inter) = motion_grid[index] {
            if col >= block.col {
                result.push(ObmcNeighbor {
                    rel_row: 0,
                    rel_col: col - block.col,
                    width_mi: width.min(step),
                    height_mi: step,
                    inter,
                });
            }
        }
        col += step;
    }
    result
}

fn obmc_neighbors_left(
    block: DecodedBlock,
    motion_grid: &[Option<InterBlock>],
    size_grid: &[Option<BlockSize>],
    mi_cols: usize,
    mi_rows: usize,
) -> Vec<ObmcNeighbor> {
    if block.col == 0 {
        return Vec::new();
    }
    let height = block.size.num_4x4_high() as usize;
    let end_row = (block.row + height).min(mi_rows);
    let mut row = block.row;
    let mut result = Vec::new();
    while row < end_row {
        let mut lookup_row = row;
        let initial = lookup_row * mi_cols + block.col - 1;
        let mut step = size_grid[initial]
            .map_or(1, |size| size.num_4x4_high() as usize)
            .min(16);
        if step == 1 {
            row &= !1;
            lookup_row = row + 1;
            if lookup_row >= mi_rows {
                break;
            }
            step = 2;
        }
        let index = lookup_row * mi_cols + block.col - 1;
        if let Some(inter) = motion_grid[index] {
            if row >= block.row {
                result.push(ObmcNeighbor {
                    rel_row: row - block.row,
                    rel_col: 0,
                    width_mi: step,
                    height_mi: height.min(step),
                    inter,
                });
            }
        }
        row += step;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn blend_obmc_region(
    current: &mut IntraFrame,
    plane: usize,
    reference_store: &ReferenceFrameStore,
    header: &FrameHeader,
    neighbor: InterBlock,
    block_col: usize,
    block_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
    vertical: bool,
) -> Result<(), InterFrameError> {
    if width == 0 || height == 0 || neighbor.ref_frame <= INTRA_FRAME {
        return Ok(());
    }
    let slot = header.ref_frame_idx[(neighbor.ref_frame - 1) as usize] as usize;
    let reference = reference_store
        .get(slot)
        .ok_or(InterFrameError::MissingReference(neighbor.ref_frame))?;
    let first = InterBlock {
        ref_frame1: NONE_FRAME,
        mv1: MotionVector::default(),
        compound_mode: None,
        compound_type: None,
        distance_weights: None,
        interintra: None,
        motion_mode: MotionMode::Simple,
        warp: None,
        warp1: None,
        ..neighbor
    };
    let (current_width, current_height) = match plane {
        0 => (current.y.width, current.y.height),
        1 => {
            let plane = current
                .u
                .as_ref()
                .ok_or(InterFrameError::MissingPlane(plane))?;
            (plane.width, plane.height)
        }
        2 => {
            let plane = current
                .v
                .as_ref()
                .ok_or(InterFrameError::MissingPlane(plane))?;
            (plane.width, plane.height)
        }
        _ => return Err(InterFrameError::InvalidState("plane index")),
    };
    let width = width.min(current_width.saturating_sub(block_col));
    let height = height.min(current_height.saturating_sub(block_row));
    let prediction = inter_block_samples(
        reference,
        plane,
        current_width,
        current_height,
        first,
        block_col,
        block_row,
        width,
        height,
        sub_x,
        sub_y,
        bit_depth,
    )?;
    let mask = obmc_mask(if vertical { height } else { width })
        .ok_or(InterFrameError::InvalidState("OBMC mask length"))?;
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
    for row in 0..height {
        for col in 0..width {
            let weight = i64::from(if vertical { mask[row] } else { mask[col] });
            let dst = (block_row + row) * current_plane.stride + block_col + col;
            let base = i64::from(current_plane.data[dst]);
            let adjacent = i64::from(prediction[row * width + col]);
            current_plane.data[dst] = round2(weight * base + (64 - weight) * adjacent, 6) as u16;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_obmc_block(
    block: DecodedBlock,
    current: &mut IntraFrame,
    motion_grid: &[Option<InterBlock>],
    size_grid: &[Option<BlockSize>],
    references: &ReferenceFrameStore,
    header: &FrameHeader,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
    planes: usize,
) -> Result<(), InterFrameError> {
    let above = obmc_neighbors_above(block, motion_grid, size_grid, header.mi_cols);
    let left = obmc_neighbors_left(
        block,
        motion_grid,
        size_grid,
        header.mi_cols,
        header.mi_rows,
    );
    for plane in 0..planes {
        let plane_sub_x = if plane == 0 { 0 } else { sub_x };
        let plane_sub_y = if plane == 0 { 0 } else { sub_y };
        let plane_size = if plane == 0 {
            block.size
        } else {
            let Some(size) = residual_size(block.size, sub_x, sub_y) else {
                continue;
            };
            size
        };
        let small_plane = matches!(
            (plane_size.width, plane_size.height),
            (4, 4) | (8, 4) | (4, 8)
        );
        let block_col = (block.col >> plane_sub_x) * 4;
        let block_row = (block.row >> plane_sub_y) * 4;
        let overlap_height = (block.size.height.min(64) as usize / 2) >> plane_sub_y;
        let overlap_width = (block.size.width.min(64) as usize / 2) >> plane_sub_x;

        if !small_plane {
            for neighbor in &above {
                blend_obmc_region(
                    current,
                    plane,
                    references,
                    header,
                    neighbor.inter,
                    block_col + ((neighbor.rel_col * 4) >> plane_sub_x),
                    block_row,
                    (neighbor.width_mi * 4) >> plane_sub_x,
                    overlap_height,
                    plane_sub_x,
                    plane_sub_y,
                    bit_depth,
                    true,
                )?;
            }
        }
        for neighbor in &left {
            blend_obmc_region(
                current,
                plane,
                references,
                header,
                neighbor.inter,
                block_col,
                block_row + ((neighbor.rel_row * 4) >> plane_sub_y),
                overlap_width,
                (neighbor.height_mi * 4) >> plane_sub_y,
                plane_sub_x,
                plane_sub_y,
                bit_depth,
                false,
            )?;
        }
    }
    Ok(())
}

/// Reconstruct an inter frame with single and compound prediction.
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
    let mut size_grid = vec![None; header.mi_cols * header.mi_rows];
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
        for row in block.block.row
            ..(block.block.row + block.block.size.num_4x4_high() as usize).min(header.mi_rows)
        {
            for col in block.block.col
                ..(block.block.col + block.block.size.num_4x4_wide() as usize).min(header.mi_cols)
            {
                size_grid[row * header.mi_cols + col] = Some(block.block.size);
            }
        }
        if let Some(inter) = block.inter {
            let (y_above, y_left) =
                neighbor_modes_at(&y_modes, header.mi_cols, block.block.row, block.block.col);
            let (uv_above, uv_left) = chroma_neighbor_modes_at(
                &uv_modes,
                header.mi_cols,
                header.mi_rows,
                block.block.row,
                block.block.col,
                sub_x,
                sub_y,
            );
            let y_filter_type = intra_filter_type(y_above, y_left);
            let uv_filter_type = intra_filter_type(uv_above, uv_left);
            let ref_slot = header.ref_frame_idx[(inter.ref_frame - 1) as usize] as usize;
            let reference = references
                .get(ref_slot)
                .ok_or(InterFrameError::MissingReference(inter.ref_frame))?;
            let reference1 = if inter.ref_frame1 > INTRA_FRAME {
                let slot = header.ref_frame_idx[(inter.ref_frame1 - 1) as usize] as usize;
                Some(
                    references
                        .get(slot)
                        .ok_or(InterFrameError::MissingReference(inter.ref_frame1))?,
                )
            } else {
                None
            };
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
                    predict_inter_or_compound_block(
                        reference,
                        reference1,
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
                        sequence.enable_intra_edge_filter,
                        y_filter_type,
                        block.block.row,
                        block.block.col,
                        header.mi_cols,
                        header.mi_rows,
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
                    predict_inter_or_compound_block(
                        reference,
                        reference1,
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
                        sequence.enable_intra_edge_filter,
                        uv_filter_type,
                        block.block.row,
                        block.block.col,
                        header.mi_cols,
                        header.mi_rows,
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
                        let candidate_reference1 =
                            if candidate.ref_frame1 > INTRA_FRAME {
                                let cut = header.ref_frame_idx[(candidate.ref_frame1 - 1) as usize]
                                    as usize;
                                Some(references.get(cut).ok_or(
                                    InterFrameError::MissingReference(candidate.ref_frame1),
                                )?)
                            } else {
                                None
                            };
                        predict_inter_or_compound_block(
                            candidate_reference,
                            candidate_reference1,
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
                            sequence.enable_intra_edge_filter,
                            uv_filter_type,
                            block.block.row,
                            block.block.col,
                            header.mi_cols,
                            header.mi_rows,
                        )?;
                    }
                }
            }
            if inter.motion_mode == MotionMode::Obmc {
                apply_obmc_block(
                    block.block,
                    &mut frame,
                    &motion_grid,
                    &size_grid,
                    references,
                    header,
                    sub_x,
                    sub_y,
                    bit_depth,
                    planes,
                )?;
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
            let intra_tile = tile_of(header, intra.block.row, intra.block.col);
            let (y_above, y_left) = neighbor_modes(&y_modes, header.mi_cols, intra, intra_tile);
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
                intra_tile,
            )?;
            let (uv_above, uv_left) = chroma_neighbor_modes(
                &uv_modes,
                header.mi_cols,
                header.mi_rows,
                intra,
                sub_x,
                sub_y,
                intra_tile,
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
                    intra_tile,
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
                    intra_tile,
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
    let vertical = (0..sequence.color.num_planes)
        .map(|plane| deblock_grid(header, blocks, true, plane, sub_x, sub_y))
        .collect::<Vec<_>>();
    let horizontal = (0..sequence.color.num_planes)
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

/// Frame-level syntax of one inter tile group.
///
/// Tiles are independent entropy streams, but they share the frame's motion
/// state: libaom keeps modes, references and MVs in the frame mi grid, so a
/// tile sees the blocks an earlier tile decoded (`scan_row_mbmi` reads
/// `xd->mi[...]` with no tile clipping, and cells a later tile has not written
/// yet fail `is_inter_block`), while every *neighbour context* is still clipped
/// at the tile boundary by `set_mi_row_col`.
pub struct InterTileGroup {
    /// Leaf blocks of every tile, in tile decode (raster) order.
    pub blocks: Vec<InterSyntaxBlock>,
    /// Union of the CDEF indices transmitted by the tiles.
    pub cdef: CdefIndexGrid,
    /// Union of the loop-restoration parameters read by the tiles.
    pub lr: LrFrameParams,
    /// Segment ids of the whole frame.
    pub segment_map: SegmentMap,
    /// Final CDF state of `header.context_update_tile_id`.
    pub cdfs: CdfState,
}

/// Decode every tile of an inter frame's tile group.
pub fn decode_inter_tile_group(
    header: &FrameHeader,
    layout: &ParsedTileGroup,
    payload: &[u8],
    config: InterTileConfig,
    initial_cdfs: CdfState,
) -> Result<InterTileGroup, TileEntropyError> {
    let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
    let mut blocks = Vec::new();
    let mut cdef = CdefIndexGrid::new(header.mi_cols, header.mi_rows);
    let mut lr = LrFrameParams::for_geometry(&config.lr_geometry);
    let mut cdfs = initial_cdfs.clone();
    let mut neighbour: Option<InterNeighbourState> = None;
    let mut segment_map: Option<SegmentMap> = None;
    for range in &layout.tiles {
        let bounds = tile_bounds(header, range.tile)?;
        let sink = InterTileBlockSink::new(config.clone().with_tile_origin(
            bounds.mi_row_start,
            bounds.mi_row_end,
            bounds.mi_col_start,
            bounds.mi_col_end,
        ));
        let sink = match neighbour.take() {
            Some(state) => sink.with_neighbour_state(state),
            None => sink,
        };
        let decoded = decode_tile_partitions_with_cdfs(
            &payload[range.bytes.clone()],
            header,
            range.tile,
            sb_size,
            header.disable_cdf_update,
            true,
            initial_cdfs.clone(),
            sink,
        )?;
        cdef.absorb_tile(
            decoded.sink.cdef_grid(),
            bounds.mi_row_start,
            bounds.mi_row_end,
            bounds.mi_col_start,
            bounds.mi_col_end,
        );
        merge_lr_params(&mut lr, decoded.sink.lr_params());
        neighbour = Some(decoded.sink.neighbour_state());
        segment_map = Some(decoded.sink.segment_map());
        blocks.extend(decoded.sink.blocks);
        if range.tile == header.context_update_tile_id as usize {
            cdfs = decoded.cdfs;
        }
    }
    let segment_map =
        segment_map.unwrap_or_else(|| SegmentMap::new(header.mi_cols, header.mi_rows));
    Ok(InterTileGroup {
        blocks,
        cdef,
        lr,
        segment_map,
        cdfs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::cdf_state::CdfState;
    use crate::reference::av1::film_grain::apply_film_grain_params;
    use crate::reference::av1::frame_header::PRIMARY_REF_NONE;
    use crate::reference::av1::inter::{CompoundMode, InterMode};
    use crate::reference::av1::intra_frame::{
        apply_intra_cdef, apply_intra_deblock, apply_intra_loop_restoration,
        decode_intra_tile_group, reconstruct_intra_frame, upscale_frame,
    };
    use crate::reference::av1::obu::parse_ivf;
    use crate::reference::av1::partition::BLOCK_SIZES;
    use crate::reference::av1::stream::{ReferenceStream, StreamEvent};

    #[test]
    fn real_two_by_two_tile_inter_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_tiles_320x240_10.ivf");
        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 10, "one picture per aomdec frame");
        assert_eq!(inter_frames.len(), 10);
        assert!(
            inter_frames
                .iter()
                .all(|blocks| blocks.iter().any(|block| block.inter.is_some())),
            "every non-key frame must carry inter blocks",
        );

        // Locked against `aomdec --rawvideo`: all ten visible frames, each
        // decoded from four independent tiles that share the frame motion state.
        let expected: [(u64, u64, u64); 10] = [
            (0xe4c15c7e09f4c973, 0x7f75e286318ce612, 0xb3d6a9d8663b3bc1),
            (0xb00762c1a12e1707, 0xb1e3848cea781f74, 0xdb841ec2077e7b3d),
            (0x682789ba575cb6a3, 0xecce1176fc86c1bc, 0x46f646d998ca7f0c),
            (0x26b01c23a0ece2e8, 0x299adb14adf501ef, 0xe538705f3a64d453),
            (0x65855b518c137e07, 0x33237f4c8dd6995c, 0x1ce4cefbcb2ea12e),
            (0x86866abe1013fce0, 0x7201e9c886fcf6d0, 0xf9b558c8932f33ae),
            (0x1ef014be4aaab150, 0x1c99dd5896a4a189, 0xf302868cd1539a42),
            (0x1f40b5c0f477f2dd, 0xd5e43a2cc3de7d13, 0x27eab2fcdd888f8b),
            (0x23b4527112ec34de, 0x1ada25836fa0b94c, 0xbaff89390d9abeee),
            (0x48ca494c917f6e9f, 0x410bd711b62dd0fb, 0xccdf5f938278f0ef),
        ];
        for (index, (picture, want)) in pictures.iter().zip(expected).enumerate() {
            assert_eq!(
                (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap()),
                ),
                want,
                "frame {index} diverges from aomdec",
            );
        }
    }

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
                size: BLOCK_SIZES[0],
                ref_frame: 1,
                mode: crate::reference::av1::inter::InterMode::Nearest,
                mv: MotionVector::default(),
                ref_frame1: crate::reference::av1::inter::NONE_FRAME,
                mv1: MotionVector::default(),
                compound_mode: None,
                compound_type: None,
                distance_weights: None,
                interintra: None,
                motion_mode: crate::reference::av1::inter::MotionMode::Simple,
                warp: None,
                warp1: None,
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

    #[test]
    fn compound_average_rounds_the_two_predictions() {
        let mut first = IntraFrame {
            coded_width: 4,
            coded_height: 4,
            viewport_width: 4,
            viewport_height: 4,
            bit_depth: 8,
            y: PlaneBuffer {
                data: vec![10; 32],
                width: 4,
                height: 4,
                stride: 8,
            },
            u: None,
            v: None,
        };
        let second = IntraFrame {
            y: PlaneBuffer {
                data: vec![21; 32],
                ..first.y.clone()
            },
            ..first.clone()
        };
        first.y.data.fill(0);
        let block = |ref_frame| InterBlock {
            size: BLOCK_SIZES[0],
            ref_frame,
            mode: InterMode::Nearest,
            mv: MotionVector::default(),
            ref_frame1: 2,
            mv1: MotionVector::default(),
            compound_mode: None,
            compound_type: Some(CompoundType::Average),
            distance_weights: None,
            interintra: None,
            motion_mode: crate::reference::av1::inter::MotionMode::Simple,
            warp: None,
            warp1: None,
            interp_filter: [0, 0],
            ref_mv_idx: 0,
        };
        predict_compound_block(
            &IntraFrame {
                y: PlaneBuffer {
                    data: vec![10; 32],
                    ..first.y.clone()
                },
                ..first.clone()
            },
            &second,
            &mut first,
            0,
            block(1),
            block(2),
            0,
            0,
            4,
            4,
            0,
            0,
            8,
            CompoundType::Average,
        )
        .unwrap();
        assert_eq!(first.y.data[0], 16);
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

    fn visible_frame_hash(frame: &IntraFrame) -> u64 {
        [Some(&frame.y), frame.u.as_ref(), frame.v.as_ref()]
            .into_iter()
            .flatten()
            .flat_map(|plane| {
                plane
                    .data
                    .chunks_exact(plane.stride)
                    .take(plane.height)
                    .flat_map(|row| &row[..plane.width])
            })
            .fold(0xcbf29ce484222325u64, |hash, sample| {
                (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
            })
    }

    fn decode_picture_stream(data: &[u8]) -> (Vec<IntraFrame>, Vec<Vec<InterSyntaxBlock>>) {
        decode_picture_stream_with_grain(data, true)
    }

    fn decode_picture_stream_with_grain(
        data: &[u8],
        apply_grain: bool,
    ) -> (Vec<IntraFrame>, Vec<Vec<InterSyntaxBlock>>) {
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        let mut references = ReferenceFrameStore::new();
        let mut pictures = Vec::new();
        let mut inter_frames = Vec::new();

        for event in events.iter().flatten() {
            if let StreamEvent::ShowExisting(header) = event {
                let slot = header.frame_to_show_map_idx as usize;
                let mut picture = references.get(slot).unwrap().clone();
                if apply_grain {
                    let grain = references.get_film_grain(slot).unwrap();
                    apply_film_grain_params(sequence, grain, &mut picture).unwrap();
                }
                pictures.push(picture);
                continue;
            }
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
            let final_cdfs;
            let current_blocks;
            let segment_map;
            let picture = if frame.header.frame_is_intra {
                let group = decode_intra_tile_group(
                    sequence,
                    &frame.header,
                    &frame.layout,
                    frame.payload,
                    initial_cdfs,
                )
                .unwrap();
                final_cdfs = group.cdfs;
                let mut picture =
                    reconstruct_intra_frame(sequence, &frame.header, &group.blocks).unwrap();
                if frame
                    .header
                    .loop_filter_level
                    .iter()
                    .any(|level| *level != 0)
                {
                    apply_intra_deblock(sequence, &frame.header, &group.blocks, &mut picture);
                }
                let deblocked = picture.clone();
                if sequence.enable_cdef && !frame.header.coded_lossless {
                    apply_intra_cdef(
                        sequence,
                        &frame.header,
                        &group.blocks,
                        &group.cdef,
                        &mut picture,
                    );
                }
                if frame.header.use_superres {
                    picture = upscale_frame(sequence, &frame.header, &picture).unwrap();
                }
                apply_intra_loop_restoration(
                    sequence,
                    &frame.header,
                    &group.lr,
                    &deblocked,
                    &mut picture,
                )
                .unwrap();
                current_blocks = Vec::new();
                segment_map = group.segment_map;
                picture
            } else {
                let config = InterTileConfig::from_headers(sequence, &frame.header)
                    .with_reference_order_hints(frame.reference_order_hints)
                    .with_temporal_motion(references.temporal_motion(sequence, &frame.header))
                    .with_previous_segment_map(references.primary_segment_map(&frame.header));
                let group = decode_inter_tile_group(
                    &frame.header,
                    &frame.layout,
                    frame.payload,
                    config,
                    initial_cdfs,
                )
                .unwrap();
                current_blocks = group.blocks.clone();
                segment_map = group.segment_map;
                inter_frames.push(group.blocks.clone());
                final_cdfs = group.cdfs;
                let mut picture =
                    reconstruct_inter_frame(sequence, &frame.header, &group.blocks, &references)
                        .unwrap();
                if frame
                    .header
                    .loop_filter_level
                    .iter()
                    .any(|level| *level != 0)
                {
                    apply_inter_deblock(sequence, &frame.header, &group.blocks, &mut picture);
                }
                let deblocked = picture.clone();
                if sequence.enable_cdef && !frame.header.coded_lossless {
                    apply_inter_cdef(
                        sequence,
                        &frame.header,
                        &group.blocks,
                        &group.cdef,
                        &mut picture,
                    );
                }
                if frame.header.use_superres {
                    picture = upscale_frame(sequence, &frame.header, &picture).unwrap();
                }
                apply_intra_loop_restoration(
                    sequence,
                    &frame.header,
                    &group.lr,
                    &deblocked,
                    &mut picture,
                )
                .unwrap();
                picture
            };
            references.refresh(
                sequence,
                &frame.header,
                &current_blocks,
                &picture,
                &final_cdfs,
                segment_map,
            );
            if frame.header.show_frame {
                let mut display = picture;
                if apply_grain {
                    apply_film_grain_params(sequence, &frame.header.film_grain, &mut display)
                        .unwrap();
                }
                pictures.push(display);
            }
        }
        (pictures, inter_frames)
    }

    fn decode_two_picture_stream(data: &[u8]) -> (Vec<IntraFrame>, Vec<InterSyntaxBlock>) {
        let (pictures, mut inter_frames) = decode_picture_stream(data);
        (pictures, inter_frames.pop().unwrap_or_default())
    }

    #[test]
    fn real_film_grain_stream_matches_libaom_after_synthesis() {
        let data = include_bytes!("../../../tests/data/av1_film_grain_320x240_10.ivf");
        let (pictures, _) = decode_picture_stream(data);

        assert_eq!(
            pictures.iter().map(visible_frame_hash).collect::<Vec<_>>(),
            [
                0xc11cbbcd3cab2a08,
                0xd6a00b8ee3a161c6,
                0x7dd2615ec74bbb91,
                0xc7ee24f4e3d93cfb,
                0x18fc4f20f58afd59,
                0xc428a5a58dbe6808,
                0x06c118bda2aa313b,
                0x1471e33d7202356b,
                0xa3cdff289fba8e06,
                0x30d83d47c2b9ff3b,
            ]
        );
    }

    #[test]
    fn real_ten_bit_film_grain_stream_matches_libaom_after_synthesis() {
        let data = include_bytes!("../../../tests/data/film_grain_10bit.ivf");
        let (pictures, _) = decode_picture_stream(data);

        assert_eq!(
            pictures.iter().map(visible_frame_hash).collect::<Vec<_>>(),
            [
                0xd50fddb69d68df95,
                0x3841f3ebb492f93a,
                0x1c8dddba91732a6d,
                0x5c741c416524381e,
                0xc2fa474fe5d5f8ea,
                0xb8a45eb3859694a8,
                0x6c191cf34666f170,
                0xf9f43fced83b6e96,
                0x9d7367a93ae385af,
                0x75c23e775c8e5319,
            ]
        );
    }

    #[test]
    fn real_ten_bit_film_grain_stream_clean_pictures_match_libaom() {
        let data = include_bytes!("../../../tests/data/film_grain_10bit.ivf");
        let (pictures, _) = decode_picture_stream_with_grain(data, false);

        assert_eq!(
            pictures.iter().map(visible_frame_hash).collect::<Vec<_>>(),
            [
                0xf97d03bc3cb41196,
                0xf31c77c0cd3d7859,
                0xe0a583a7d3ba9445,
                0xe7495692d7cc4891,
                0x72c0773fc61ac58e,
                0x786f7d0d0b8e9511,
                0xced198a7300732ac,
                0x1ad73db5c4bbaea6,
                0x2dfe30ef60cc7873,
                0x77e080d062826eec,
            ]
        );
    }

    #[test]
    fn real_96x96_inter_stream_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_96x96_10.ivf");
        let (pictures, _) = decode_picture_stream(data);

        assert_eq!(
            pictures.iter().map(visible_frame_hash).collect::<Vec<_>>(),
            [
                0x63de3f982309679b,
                0x3a1a282fa21e27da,
                0x2f6c7e3ed716c511,
                0x0c8e36a6e931ac52,
                0xf0bb5e474f4b183b,
                0x11f8dd95f5ee0be3,
                0xd241569402e3bede,
                0x0b1399e78addc217,
                0x626809202a51421a,
                0xf35d91c347ed6bd1,
            ]
        );
    }

    #[test]
    fn real_128x128_inter_stream_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_128x128_10.ivf");
        let (pictures, _) = decode_picture_stream(data);

        assert_eq!(
            pictures.iter().map(visible_frame_hash).collect::<Vec<_>>(),
            [
                0x8ddad484f4e84da3,
                0xa1414cbe4ea9453f,
                0x0686738f6aeca0c8,
                0xfc2659638f077c66,
                0x8c452dd0f4fa8f47,
                0xb8fcbe236a33e0fe,
                0x7e7f07d81301fb18,
                0x099af4bd069c1767,
                0xa724024718ecc7da,
                0x368c37e6089d4282,
            ]
        );
    }

    #[test]
    fn real_192x192_inter_stream_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_192x192_10.ivf");
        let (pictures, _) = decode_picture_stream(data);

        assert_eq!(
            pictures.iter().map(visible_frame_hash).collect::<Vec<_>>(),
            [
                0x79605eecc0b57d16,
                0xd96c3b89dcfc7a97,
                0xb77c8a16f23d21ef,
                0x9fd5aca5c85b9a46,
                0xe23fb8684a0ef20c,
                0x20293ba7bf7e80a9,
                0x8835e5d9152f7838,
                0x19558c729f4e265a,
                0x0ea18aba26154a8c,
                0x4afa509decc5ddec,
            ]
        );
    }

    #[test]
    fn real_444_inter_stream_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_444_320x240.ivf");
        let (pictures, _) = decode_picture_stream(data);

        assert_eq!(
            pictures.iter().map(visible_frame_hash).collect::<Vec<_>>(),
            [0x3a3ac8702504f818, 0xfeb795c9af578d86, 0x3fa247cf03083531]
        );
    }

    #[test]
    fn real_one_column_two_tile_row_stream_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_tile_rows_320x320.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let frames = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(frames.len(), 3);
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame.header.tile_cols, 1);
            assert_eq!(frame.header.tile_rows, 2);
            assert_eq!(
                frame.header.context_update_tile_id,
                [0, 1, 1][index],
                "context update tile for frame {index}"
            );
            assert_eq!(frame.layout.tiles.len(), 2);
        }

        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 3);
        assert_eq!(inter_frames.len(), 2);
        assert!(!inter_frames[0].is_empty());
        assert!(!inter_frames[1].is_empty());

        // Locked against stock aomdec at 80% OPP on t6.
        let expected: [(u64, u64, u64); 3] = [
            (0x6eca3ac0d4243aad, 0x14e1c277af328064, 0x4c069b480fd0eb6b),
            (0xec5884cb134c1dd8, 0x00676cb66148b3f7, 0xcbf13b2bd14a4669),
            (0x5fe57016af73bb4e, 0x57b09062271f10a9, 0x5ac2276fd3fb418c),
        ];
        for (index, (picture, want)) in pictures.iter().zip(expected).enumerate() {
            assert_eq!(
                (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap()),
                ),
                want,
                "frame {index} diverges from aomdec",
            );
        }
    }

    #[test]
    fn real_intrabc_and_palette_stream_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_intrabc_palette_320x320.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let frames = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(frames.len(), 3);
        assert!(frames[0].header.allow_intrabc);

        let (pictures, _) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 3);
        let expected: [(u64, u64, u64); 3] = [
            (0x996b85d532070894, 0x6259ac5235216584, 0x2f1b2f8d4387861c),
            (0xbeee2dc6561fce58, 0x1770358636fc0196, 0xf3a48d8c4bfcab5c),
            (0x3942d84cd4654ccf, 0x770a06024e21a8e3, 0x73bebeea68714a5d),
        ];
        for (index, (picture, want)) in pictures.iter().zip(expected).enumerate() {
            assert_eq!(
                (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap()),
                ),
                want,
                "frame {index} diverges from aomdec",
            );
        }
    }

    #[test]
    fn real_b_pyramid_reordered_stream_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_b_pyramid_320x240_20.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let coded = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(coded.iter().any(|frame| !frame.header.show_frame));
        assert!(events
            .iter()
            .flatten()
            .any(|event| matches!(event, StreamEvent::ShowExisting(_))));

        let (pictures, _) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 20);
        let expected = [
            0x1af1341dc4d99ee3,
            0xb3a63bb9859a3a78,
            0x5de20c45fd22c8f9,
            0x8c4f5d082030f577,
            0x95f82f32d140094b,
            0x30e53bb6bb8d0d11,
            0x79b4a8d1a7a6653c,
            0x4374564e02353309,
            0xe0eb4740e50acf10,
            0x206ad386a8c1c130,
            0xb24992c1b530b679,
            0xaaeea6a7090a2dba,
            0x3ce3d9838df98b49,
            0x8ddd6895dba8415c,
            0xa54d70aba2b9583a,
            0x674eac5d10d66f56,
            0x757dacde17be27eb,
            0xe2390c614cdbfb9c,
            0x90f1e78c92765cc3,
            0x5444a1b825c4a678,
        ];
        for (index, (picture, want)) in pictures.iter().zip(expected).enumerate() {
            assert_eq!(
                visible_frame_hash(picture),
                want,
                "displayed frame {index} diverges from aomdec",
            );
        }
    }

    #[test]
    fn real_advanced_b_pyramid_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_b_pyramid_advanced_320x240_20.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let coded = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(coded.iter().any(|frame| !frame.header.show_frame));
        assert!(events
            .iter()
            .flatten()
            .any(|event| matches!(event, StreamEvent::ShowExisting(_))));
        assert!(coded.iter().any(|frame| {
            frame.header.reference_select
                && frame
                    .reference_order_hints
                    .iter()
                    .filter(|hint| **hint != 0)
                    .count()
                    >= 3
        }));

        let (pictures, _) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 20);
        let expected = [
            0x1af1341dc4d99ee3,
            0xd86ff29764a36eb0,
            0xc3a8194a4f645916,
            0xc5ade2b0a441bf85,
            0xe926467ce106d3f1,
            0x3241570c2a99c050,
            0x21d2916164b43210,
            0x9955dbbe18378da6,
            0xd724c16342595d97,
            0xec2f1fdce9b05dc2,
            0xaad90720b86fe1f4,
            0xa354dc426b752b9c,
            0x1fc66bb4357949ce,
            0x40e15a02609b9872,
            0xfaa22a09bf747b70,
            0x12ecc550d33e40d4,
            0xd9414d6c111f70fa,
            0xc485b2dd53b56ba6,
            0x16f1bae0d045693a,
            0xadd6a7361ee6dc75,
        ];
        for (index, (picture, want)) in pictures.iter().zip(expected).enumerate() {
            assert_eq!(
                visible_frame_hash(picture),
                want,
                "advanced displayed frame {index} diverges from aomdec",
            );
        }
    }

    #[test]
    fn real_ten_bit_b_pyramid_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_b_pyramid_10bit_320x240_12.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let coded = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(coded.len(), 12);
        assert!(coded.iter().all(|frame| frame.header.show_frame));
        assert!(coded.iter().all(|frame| !frame.header.reference_select));
        let max_order_hints = coded
            .iter()
            .map(|frame| {
                frame
                    .reference_order_hints
                    .iter()
                    .filter(|hint| **hint != 0)
                    .count()
            })
            .max()
            .unwrap_or(0);
        assert_eq!(max_order_hints, 7);

        let (pictures, _) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 12);
        assert!(pictures.iter().all(|picture| picture.bit_depth == 10));
        let expected = [
            0x8ba70f4f40f0b596,
            0x06ae7e7870e4ff00,
            0x26c756d4b3267738,
            0x1174442a703ff716,
            0xcba566d33caee8cb,
            0x39f860af62962cf9,
            0x0606d5627ca3ede6,
            0x1abd2b921beef8b6,
            0x9de73f8cac3ec0cd,
            0x1833f502f09341b9,
            0xd03d07b88ab0699e,
            0x27f49b4d058db9d0,
        ];
        for (index, (picture, want)) in pictures.iter().zip(expected).enumerate() {
            assert_eq!(
                visible_frame_hash(picture),
                want,
                "10-bit displayed frame {index} diverges from aomdec",
            );
        }
    }

    #[test]
    fn real_advanced_ten_bit_b_pyramid_matches_libaom() {
        let data =
            include_bytes!("../../../tests/data/av1_b_pyramid_10bit_advanced_320x240_20.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let coded = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(coded.iter().any(|frame| !frame.header.show_frame));
        assert!(events
            .iter()
            .flatten()
            .any(|event| matches!(event, StreamEvent::ShowExisting(_))));
        assert!(coded.iter().any(|frame| {
            frame.header.reference_select
                && frame
                    .reference_order_hints
                    .iter()
                    .filter(|hint| **hint != 0)
                    .count()
                    >= 3
        }));

        let (pictures, _) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 20);
        assert!(pictures.iter().all(|picture| picture.bit_depth == 10));
        let expected = [
            0x292c05997b32ce54,
            0x73e91c224e0477f6,
            0x7bbdfcff8449cad2,
            0x70ff8ceac810a2cb,
            0x1c9aaabca0e8d855,
            0xf68f606822324f85,
            0x0c412221b4ac7500,
            0x7168fc65c68fb23f,
            0x2061318aed5bd14b,
            0x747622594197e071,
            0x9b77da11c914dcb2,
            0xb02679f293721674,
            0x5ecdd4e85a11baa3,
            0x0081737e88b1b835,
            0x8ba93c350fc005b3,
            0x61e6437d62ec397b,
            0x97af5e8173850436,
            0xa6db4ee1e6415384,
            0x9c371c0c38428734,
            0xf7c9b5c25d528b7a,
        ];
        for (index, (picture, want)) in pictures.iter().zip(expected).enumerate() {
            assert_eq!(
                visible_frame_hash(picture),
                want,
                "advanced 10-bit displayed frame {index} diverges from aomdec",
            );
        }
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

    #[test]
    fn real_mixed_intra_inter_tx_context_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_lr_320x240.ivf");
        let (pictures, blocks) = decode_two_picture_stream(data);
        assert_eq!(pictures.len(), 2);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x9a98c493231c827b, 0x21132b144973900c, 0x5b3443d30cb8abd7),
                (0x02aa93cebd88f224, 0xd65506d1606b7ba7, 0xf81fd49b7783f352),
            ]
        );
        assert_eq!(blocks.len(), 458);
        assert_eq!(
            blocks.iter().filter(|block| block.inter.is_some()).count(),
            356
        );
    }

    #[test]
    fn real_inter_frames_with_delta_q_match_libaom_after_context_fix() {
        let data = include_bytes!("../../../tests/data/av1_inter_delta_q_320x240.ivf");
        let (pictures, inter_blocks) = decode_two_picture_stream(data);
        assert_eq!(pictures.len(), 3);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x4e58fc2c9de78af0, 0x7484f6164de4aacc, 0xc7f4a3e4acedc952),
                (0xee7ab437b69011b8, 0xbea5591ae873e737, 0x2cb839cf8fccbd69),
                (0x4a1bffa62af555fd, 0xc6e15cbd3a97dbea, 0x0c6771f50cfb1f1e),
            ]
        );

        assert_eq!(inter_blocks.len(), 40);
    }

    #[test]
    fn smooth_interintra_masks_match_the_normative_weights() {
        let size = BLOCK_SIZES[3];
        let dc = smooth_interintra_mask(InterIntraMode::Dc, size, 8, 8);
        assert!(dc.iter().all(|value| *value == 32));

        let vertical = smooth_interintra_mask(InterIntraMode::V, size, 8, 8);
        let horizontal = smooth_interintra_mask(InterIntraMode::H, size, 8, 8);
        let smooth = smooth_interintra_mask(InterIntraMode::Smooth, size, 8, 8);
        for row in 0..8 {
            for col in 0..8 {
                assert_eq!(vertical[row * 8 + col], II_WEIGHTS_1D[row * 16]);
                assert_eq!(horizontal[row * 8 + col], II_WEIGHTS_1D[col * 16]);
                assert_eq!(smooth[row * 8 + col], II_WEIGHTS_1D[row.min(col) * 16]);
            }
        }
    }

    #[test]
    fn real_smooth_interintra_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_interintra_smooth_320x240.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        assert!(sequence.enable_interintra_compound);
        assert!(!sequence.enable_masked_compound);

        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 5);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x65820d2149447476, 0x8d8d4cf76702b654, 0xe868959f13276360),
                (0xde44dc6bd617a3eb, 0xe5147a4ba1e0accb, 0xd71b3e5cbb57147d),
                (0x398b260ca400a637, 0x5f01766f59a52bde, 0xbcb425a3e849dc64),
                (0xcd107160e7e6f57d, 0xf662f2a93c0da2d7, 0x91d0e12436a5854b),
                (0x2a9156967e567bdf, 0x9c4017c7b2921557, 0xd64e6ae153c17f37),
            ]
        );

        let blocks = inter_frames
            .iter()
            .flatten()
            .filter_map(|syntax| syntax.inter)
            .filter_map(|inter| inter.interintra.map(|interintra| (inter, interintra)))
            .collect::<Vec<_>>();
        assert_eq!(blocks.len(), 15);
        assert!(blocks.iter().all(|(inter, interintra)| {
            inter.ref_frame1 == INTRA_FRAME && interintra.wedge_index.is_none()
        }));
        assert!(blocks
            .iter()
            .any(|(_, interintra)| interintra.mode == InterIntraMode::V));
        assert!(blocks
            .iter()
            .any(|(_, interintra)| interintra.mode == InterIntraMode::H));
        assert!(blocks
            .iter()
            .any(|(_, interintra)| interintra.mode == InterIntraMode::Smooth));
    }

    #[test]
    fn real_wedge_interintra_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_interintra_wedge_320x240.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        assert!(sequence.enable_interintra_compound);
        assert!(!sequence.enable_masked_compound);

        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 5);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0xc19ba2000554501b, 0x2a846b4a7ec85873, 0x934d3f2be4138428),
                (0xf922eadddf4c3ae7, 0x1b26c72f0df03a0c, 0xae3de71079e72d8c),
                (0x454a0a97f2b3b1b1, 0x589d51fc7354fb85, 0x39b049e1b2d7d099),
                (0x1fbb5438d5686089, 0xcaf63827a132e836, 0xad8d167a53fc3932),
                (0x2dad928218620daf, 0xcc66d8ba3bd64405, 0xec1f44eedee8c1a3),
            ]
        );

        let blocks = inter_frames
            .iter()
            .flatten()
            .filter_map(|syntax| syntax.inter)
            .filter_map(|inter| inter.interintra.map(|interintra| (inter, interintra)))
            .collect::<Vec<_>>();
        assert_eq!(blocks.len(), 9);
        assert!(blocks.iter().all(|(inter, interintra)| {
            inter.ref_frame1 == INTRA_FRAME && interintra.wedge_index.is_some()
        }));
        let dc = blocks
            .iter()
            .find(|(_, interintra)| interintra.mode == InterIntraMode::Dc)
            .unwrap();
        assert_eq!(dc.1.mode, InterIntraMode::Dc);
        assert_eq!(dc.1.wedge_index, Some(6));
    }

    #[test]
    fn real_obmc_inter_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_obmc_320x240.ivf");
        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 5);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0xc19ba2000554501b, 0x2a846b4a7ec85873, 0x934d3f2be4138428),
                (0x5f5ce0ef87c74696, 0xcbbd90d8c635ec12, 0xb55695910e9b4721),
                (0x956cce0de7cd0e2c, 0x4733e5d99e76e697, 0x9292bed5c52e85de),
                (0x3cea5dbe44754fc3, 0xb7700a49b016afbf, 0xba93574ee2bf9412),
                (0x19a04e55312c0029, 0x87ef465b0d79ad77, 0x14cb302639a6acc7),
            ]
        );
        let obmc_blocks = inter_frames
            .iter()
            .flatten()
            .filter_map(|syntax| syntax.inter)
            .filter(|inter| inter.motion_mode == MotionMode::Obmc)
            .collect::<Vec<_>>();
        assert_eq!(obmc_blocks.len(), 32);
        assert!(obmc_blocks.iter().any(|inter| inter.ref_frame == 7));
        assert!(obmc_blocks.iter().any(|inter| inter.mv.row != 0));
    }

    #[test]
    fn real_local_warped_motion_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_warped_320x240.ivf");
        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 5);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x918d0530d1feecd9, 0x1d85883c86fcbdbe, 0x0e550e0877a7e742),
                (0x7090b3e808a470a8, 0xc66fc6f9f2e14c43, 0x3620d76cca27251b),
                (0x74408035b023950c, 0x3e5891fb9f4c2b9e, 0x650928c86b2cef32),
                (0xb2ee8e53ef7d9357, 0x726f53df51c75981, 0xb29bfa04526a802b),
                (0x6f5b99c30a67500e, 0xc883f8bec7a33157, 0xbd31fbb0390a6a75),
            ]
        );
        let warped = inter_frames
            .iter()
            .flatten()
            .filter_map(|syntax| syntax.inter)
            .filter(|inter| inter.motion_mode == MotionMode::LocalWarp)
            .collect::<Vec<_>>();
        assert_eq!(warped.len(), 10);
        assert!(warped.iter().all(|inter| inter.warp.is_some()));
        assert!(warped.iter().any(|inter| {
            let matrix = inter.warp.unwrap().matrix;
            matrix != [0, 0, 1 << 16, 0, 0, 1 << 16]
        }));
    }

    #[test]
    fn real_global_warped_motion_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_global_warped_320x240.ivf");
        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 2);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x406d0805c7eb56b4, 0x94e9b79e9c8373ff, 0x936fcf6f42619786),
                (0x03df613f1188494, 0xc1d0b4eb09a7076b, 0xb10a346b120d1a8e),
            ]
        );
        let global = inter_frames
            .iter()
            .flatten()
            .filter_map(|syntax| syntax.inter)
            .filter(|inter| {
                inter.mode == InterMode::Global
                    && inter.motion_mode == MotionMode::Simple
                    && inter.warp.is_some()
            })
            .collect::<Vec<_>>();
        assert_eq!(global.len(), 756);
        assert!(global
            .iter()
            .all(|inter| inter.ref_frame == crate::reference::av1::inter::LAST_FRAME));
    }

    #[test]
    fn real_global_warp_compound_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_global_warp_compound_320x240.ivf");
        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 4);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x203491d03c8ac843, 0xdc35b7ebcd4eecdd, 0xf1f5baa1c32e4472),
                (0x24b240c72352849d, 0xc3ef74b23898f939, 0x943da4164cbddac4),
                (0x5635f7c99f72d884, 0x478360d6c54309f5, 0x325d143fa2cf92ca),
                (0x8bb9fa9fddfe4fe2, 0x947b6dc789d7e5ac, 0x3dc67ca226742fc9),
            ]
        );
        let global_compound = inter_frames
            .iter()
            .flatten()
            .filter_map(|syntax| syntax.inter)
            .filter(|inter| {
                inter.compound_mode == Some(CompoundMode::GlobalGlobal)
                    && (inter.warp.is_some() || inter.warp1.is_some())
            })
            .collect::<Vec<_>>();
        assert_eq!(global_compound.len(), 3);
        assert!(global_compound
            .iter()
            .any(|inter| matches!(inter.compound_type, Some(CompoundType::DistanceWeighted))));
        assert!(global_compound
            .iter()
            .any(|inter| matches!(inter.compound_type, Some(CompoundType::Wedge { .. }))));
    }

    #[test]
    fn real_temporal_motion_field_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_temporal_mv_320x240.ivf");
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
        assert_eq!(headers.len(), 3);
        assert!(headers[1].use_ref_frame_mvs);
        assert!(headers[2].use_ref_frame_mvs);

        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 3);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x61b96b84e4bdc0f8, 0xdb9392766abafc2a, 0x067de1b4a2fc42ac),
                (0x156730fec968b729, 0x567bb50a75473b19, 0x57ee9c438bde8948),
                (0x9c178429ea526093, 0x72f7a2ebfa2aef6c, 0x2be2987011717aa1),
            ]
        );
        assert_eq!(inter_frames.iter().flatten().count(), 128);
    }

    #[test]
    fn real_segment_maps_alt_q_and_temporal_prediction_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_segment_320x240.ivf");
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
        assert_eq!(headers.len(), 3);
        assert!(headers[0].segmentation_enabled);
        assert_eq!(headers[0].last_active_seg_id, 7);
        assert!(headers[0]
            .feature_enabled
            .iter()
            .all(|features| features[0]));
        assert!(headers[2].segmentation_enabled);
        assert!(headers[2].segmentation_update_map);
        assert!(headers[2].segmentation_temporal_update);
        assert_eq!(headers[2].last_active_seg_id, 7);

        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 3);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x42c4af21492b57a, 0x930bef4f94807f25, 0x930bef4f94807f25),
                (0x7a1c3a043269c142, 0x930bef4f94807f25, 0x930bef4f94807f25),
                (0x157e20c668b537db, 0x930bef4f94807f25, 0x930bef4f94807f25),
            ]
        );
        assert_eq!(
            inter_frames.iter().map(Vec::len).collect::<Vec<_>>(),
            [168, 1286]
        );
        let mut segment_counts = [0usize; 8];
        for block in inter_frames[1].iter().filter(|block| block.inter.is_some()) {
            segment_counts[block.segment_id] += 1;
        }
        assert_eq!(segment_counts, [1, 0, 2, 0, 253, 4, 791, 0]);
    }

    #[test]
    fn real_average_compound_inter_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_compound_320x240.ivf");
        let (pictures, inter_blocks) = decode_two_picture_stream(data);

        assert_eq!(pictures.len(), 4);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x4e58fc2c9de78af0, 0x7484f6164de4aacc, 0xc7f4a3e4acedc952),
                (0xee7ab437b69011b8, 0xbea5591ae873e737, 0x2cb839cf8fccbd69),
                (0x4a1bffa62af555fd, 0xc6e15cbd3a97dbea, 0x0c6771f50cfb1f1e),
                (0x39b67a234a3172ad, 0x4191e824f1fb0a98, 0xde088d10e6016818),
            ]
        );

        assert_eq!(inter_blocks.len(), 38);
        let block = inter_blocks
            .iter()
            .find(|block| block.block.row == 32 && block.block.col == 16)
            .unwrap();
        let inter = block.inter.unwrap();
        assert_eq!(
            inter.compound_mode,
            Some(crate::reference::av1::inter::CompoundMode::NewNew)
        );
        assert_eq!((inter.ref_frame, inter.ref_frame1), (1, 4));
        assert_eq!((inter.mv.row, inter.mv.col), (0, -18));
        assert_eq!((inter.mv1.row, inter.mv1.col), (4, -12));
    }

    #[test]
    fn real_difference_weighted_compound_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_diffwtd_compound_320x240.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        assert!(sequence.enable_masked_compound);
        assert!(!sequence.enable_interintra_compound);
        assert!(!sequence.enable_order_hint);

        let (pictures, inter_blocks) = decode_two_picture_stream(data);
        assert_eq!(pictures.len(), 5);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0xfe5a72f3b8fcb0c0, 0x1d850b528c8704cc, 0x9edc3b88f9fe2e69),
                (0x37802176b74c4ca3, 0x20611c93be42d21c, 0x09077d5409d14227),
                (0x3fc460a4337801a2, 0x78aeee20ef3533cd, 0xbb57d25fe05cd369),
                (0x8366f509fa718675, 0xb2323978a827835d, 0x44d96a63350ded41),
                (0x89a893342f663d21, 0xda7358359f0f4adb, 0x91cff8e2e3d4f404),
            ]
        );
        assert_eq!(inter_blocks.len(), 20);
    }

    #[test]
    fn real_distance_weighted_compound_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_distance_weighted_320x240.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        assert!(sequence.enable_order_hint);
        assert!(sequence.enable_jnt_comp);
        assert!(!sequence.enable_ref_frame_mvs);

        let (pictures, inter_frames) = decode_picture_stream(data);
        assert_eq!(pictures.len(), 5);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0x98a09f65a8531836, 0x6ae74b5d387bd11e, 0x5d54ab813a34334b),
                (0x22a99dcee877eb7d, 0x5fe7584e9e1e37c6, 0x9392a532aa09f88b),
                (0x17cc27fd11ea1af6, 0x43e5401099ed2c59, 0xb301ae5256567552),
                (0xaf81ec791640e3a0, 0xc5d5aa4e09283f06, 0x05975faacc2956ac),
                (0xddde28e4ea1b96bf, 0x8653ff333eec5e3f, 0xf440be7760baf7f9),
            ]
        );

        let distance_blocks = inter_frames
            .iter()
            .flatten()
            .filter_map(|syntax| syntax.inter)
            .filter(|inter| inter.compound_type == Some(CompoundType::DistanceWeighted))
            .collect::<Vec<_>>();
        assert_eq!(distance_blocks.len(), 2);

        let first = distance_blocks[0];
        assert_eq!(
            (first.size.name, first.ref_frame, first.ref_frame1),
            ("BLOCK_32X16", 1, 7)
        );
        assert_eq!(first.compound_mode, Some(CompoundMode::NearestNearest));
        assert_eq!((first.mv.row, first.mv.col), (0, 0));
        assert_eq!((first.mv1.row, first.mv1.col), (0, 0));
        assert_eq!(first.distance_weights, Some([11, 5]));

        let second = distance_blocks[1];
        assert_eq!(
            (second.size.name, second.ref_frame, second.ref_frame1),
            ("BLOCK_8X8", 1, 7)
        );
        assert_eq!(second.compound_mode, Some(CompoundMode::NearestNearest));
        assert_eq!((second.mv.row, second.mv.col), (-4, 0));
        assert_eq!((second.mv1.row, second.mv1.col), (-4, 0));
        assert_eq!(second.distance_weights, Some([13, 3]));
    }

    #[test]
    fn real_wedge_compound_frames_match_libaom() {
        let data = include_bytes!("../../../tests/data/av1_inter_wedge_compound_320x240.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        assert!(sequence.enable_masked_compound);
        assert!(!sequence.enable_interintra_compound);
        assert!(!sequence.enable_order_hint);

        let (pictures, inter_blocks) = decode_two_picture_stream(data);
        assert_eq!(pictures.len(), 5);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| (
                    visible_hash(&picture.y),
                    visible_hash(picture.u.as_ref().unwrap()),
                    visible_hash(picture.v.as_ref().unwrap())
                ))
                .collect::<Vec<_>>(),
            [
                (0xe191eac24026e71c, 0x8db5e9c0b091c76e, 0x5647612e67ea65bb),
                (0x20dc365b67c6047d, 0x5d568d2a04691cb9, 0xbd6d0e3c6f40f286),
                (0x60be86be4fceb27e, 0xfb059eef9bc6e8e4, 0xaf42fb75230725bd),
                (0x5cfd3e1bb14dfd02, 0x3235fcddb17d33fe, 0xcb0352c4175f5b5f),
                (0xed64d6041f846d81, 0xf37e53c3eccf0002, 0x5b0ab73fc940b883),
            ]
        );
        assert_eq!(inter_blocks.len(), 20);
    }
}
