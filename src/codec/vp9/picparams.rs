//! `DXVA_PicParams_VP9` construction for the rkvdec VP9 front-end.
//!
//! The byte offsets and ordering mirror
//! `mpp/common/vp9d_syntax.h::DXVA_PicParams_VP9` and
//! `vp9d_parser.c::vp9d_fill_picparams`. Keeping the representation explicit
//! avoids accidentally treating the parser's syntax order as the hardware
//! order.

use super::compressed::{CompoundPredictionMode, CompressedHeader, TxMode};
use super::parser::{FrameKind, Header};

pub const PIC_PARAMS_SIZE: usize = 19564;
pub const INVALID_SLOT: u8 = 0x7f;

const PROFILE_OFFSET: usize = 1;
const FLAGS_OFFSET: usize = 2;
const WIDTH_OFFSET: usize = 4;
const HEIGHT_OFFSET: usize = 8;
const BIT_DEPTH_LUMA_OFFSET: usize = 12;
const BIT_DEPTH_CHROMA_OFFSET: usize = 13;
const INTERP_FILTER_OFFSET: usize = 14;
const REF_FRAME_MAP_OFFSET: usize = 16;
const REF_WIDTH_OFFSET: usize = 24;
const REF_HEIGHT_OFFSET: usize = 56;
const FRAME_REFS_OFFSET: usize = 88;
const SIGN_BIAS_OFFSET: usize = 91;
const FILTER_LEVEL_OFFSET: usize = 95;
const SHARPNESS_OFFSET: usize = 96;
const CONTROL_FLAGS_OFFSET: usize = 97;
const REF_DELTAS_OFFSET: usize = 98;
const MODE_DELTAS_OFFSET: usize = 102;
const BASE_QINDEX_OFFSET: usize = 104;
const Y_DC_DELTA_OFFSET: usize = 106;
const UV_DC_DELTA_OFFSET: usize = 107;
const UV_AC_DELTA_OFFSET: usize = 108;
const SEGMENTATION_OFFSET: usize = 110;
const SEGMENTATION_SIZE: usize = 84;
const TILE_COLS_OFFSET: usize = 194;
const TILE_ROWS_OFFSET: usize = 195;
const HEADER_SIZE_OFFSET: usize = 196;
const FIRST_PARTITION_OFFSET: usize = 198;
const PROB_OFFSET: usize = 208;
const PROB_SIZE: usize = 2039;
const PROB_UPDATES_OFFSET: usize = 15472;
const PROB_UPDATES_SIZE: usize = 4078;
const MV_SCALE_OFFSET: usize = 19550;
const TX_MODE_OFFSET: usize = 19562;
const REF_MODE_OFFSET: usize = 19563;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceSlot {
    pub slot: u8,
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
}

impl ReferenceSlot {
    pub const INVALID: Self = Self {
        slot: INVALID_SLOT,
        width: 0,
        height: 0,
        bit_depth: 0,
    };

    pub fn new(slot: u8, width: u32, height: u32, bit_depth: u8) -> Self {
        Self {
            slot,
            width,
            height,
            bit_depth,
        }
    }
}

/// Build the C MPP HAL input for a picture that will be decoded.
pub fn build(
    header: &Header,
    compressed: &CompressedHeader,
    current_slot: u8,
    reference_slots: &[ReferenceSlot; 8],
    use_prev_in_find_mv_refs: bool,
) -> [u8; PIC_PARAMS_SIZE] {
    let mut bytes = [0u8; PIC_PARAMS_SIZE];
    bytes[0] = current_slot;
    bytes[PROFILE_OFFSET] = profile_number(header) as u8;

    let mut flags = 0u16;
    flags |= (!matches!(header.frame_kind, FrameKind::Key) as u16) << 0;
    flags |= (header.show_frame as u16) << 1;
    flags |= (header.error_resilient_mode as u16) << 2;
    // Profile 0/2 are both 4:2:0 in the Rockchip parser.
    flags |= 1 << 3;
    flags |= 1 << 4;
    flags |= (header.refresh_frame_context as u16) << 6;
    flags |= (header.intra_only as u16) << 7;
    flags |= (header.frame_context_idx as u16 & 0x3) << 8;
    flags |= (header.reset_frame_context as u16 & 0x3) << 10;
    flags |= (header.allow_high_precision_mv as u16) << 12;
    flags |= (header.frame_parallel_mode as u16) << 13;
    put_u16(&mut bytes, FLAGS_OFFSET, flags);

    put_u32(&mut bytes, WIDTH_OFFSET, header.width);
    put_u32(&mut bytes, HEIGHT_OFFSET, header.height);
    bytes[BIT_DEPTH_LUMA_OFFSET] = header.bit_depth - 8;
    bytes[BIT_DEPTH_CHROMA_OFFSET] = header.bit_depth - 8;
    bytes[INTERP_FILTER_OFFSET] = header.interp_filter;

    for (index, reference) in reference_slots.iter().enumerate() {
        bytes[REF_FRAME_MAP_OFFSET + index] = reference.slot;
        put_u32(&mut bytes, REF_WIDTH_OFFSET + index * 4, reference.width);
        put_u32(&mut bytes, REF_HEIGHT_OFFSET + index * 4, reference.height);
    }
    bytes[FRAME_REFS_OFFSET..FRAME_REFS_OFFSET + 3].copy_from_slice(&header.ref_frame_idx);
    for index in 0..3 {
        bytes[SIGN_BIAS_OFFSET + 1 + index] = header.ref_frame_sign_bias[index] as u8;
    }

    bytes[FILTER_LEVEL_OFFSET] = header.loop_filter.level;
    bytes[SHARPNESS_OFFSET] = header.loop_filter.sharpness;
    let mut control = 0u8;
    control |= header.loop_filter.mode_ref_delta_enabled as u8;
    control |= (header.loop_filter.mode_ref_delta_update as u8) << 1;
    control |= (use_prev_in_find_mv_refs as u8) << 2;
    bytes[CONTROL_FLAGS_OFFSET] = control;
    for index in 0..4 {
        bytes[REF_DELTAS_OFFSET + index] = header.loop_filter.ref_deltas[index] as u8;
    }
    for index in 0..2 {
        bytes[MODE_DELTAS_OFFSET + index] = header.loop_filter.mode_deltas[index] as u8;
    }
    put_u16(
        &mut bytes,
        BASE_QINDEX_OFFSET,
        u16::from(header.quantization.base_q_idx),
    );
    bytes[Y_DC_DELTA_OFFSET] = header.quantization.y_dc_delta_q as u8;
    bytes[UV_DC_DELTA_OFFSET] = header.quantization.uv_dc_delta_q as u8;
    bytes[UV_AC_DELTA_OFFSET] = header.quantization.uv_ac_delta_q as u8;
    write_segmentation(&mut bytes, header);

    bytes[TILE_COLS_OFFSET] = header.log2_tile_cols;
    bytes[TILE_ROWS_OFFSET] = header.log2_tile_rows;
    put_u16(&mut bytes, HEADER_SIZE_OFFSET, header.header_size as u16);
    put_u16(
        &mut bytes,
        FIRST_PARTITION_OFFSET,
        header.first_partition_size,
    );

    bytes[PROB_OFFSET..PROB_OFFSET + PROB_SIZE]
        .copy_from_slice(&compressed.probabilities.hardware_snapshot_bytes());
    bytes[PROB_UPDATES_OFFSET..PROB_UPDATES_OFFSET + PROB_UPDATES_SIZE]
        .copy_from_slice(&compressed.updates.hardware_snapshot_bytes());

    for (index, scale) in header.mvscale.iter().enumerate() {
        put_u16(&mut bytes, MV_SCALE_OFFSET + index * 4, scale[0]);
        put_u16(&mut bytes, MV_SCALE_OFFSET + index * 4 + 2, scale[1]);
    }
    bytes[TX_MODE_OFFSET] = tx_mode_number(compressed.tx_mode) as u8;
    bytes[REF_MODE_OFFSET] = compound_mode_number(compressed.compound_prediction_mode) as u8;
    bytes
}

fn write_segmentation(bytes: &mut [u8], header: &Header) {
    let segmentation = &header.segmentation;
    let base = SEGMENTATION_OFFSET;
    let flags = (segmentation.enabled as u8)
        | ((segmentation.update_map as u8) << 1)
        | ((segmentation.temporal_update as u8) << 2)
        | ((segmentation.abs_delta as u8) << 3);
    bytes[base] = flags;
    bytes[base + 1..base + 8].copy_from_slice(&segmentation.tree_probs);
    bytes[base + 8..base + 11].copy_from_slice(&segmentation.pred_probs);
    for segment in 0..8 {
        for feature in 0..4 {
            put_i16(
                bytes,
                base + 12 + (segment * 4 + feature) * 2,
                segmentation.feature_data[segment][feature],
            );
        }
    }
    bytes[base + 76..base + SEGMENTATION_SIZE].copy_from_slice(&segmentation.feature_mask);
}

fn profile_number(header: &Header) -> u8 {
    match header.profile {
        super::parser::Profile::Profile0 => 0,
        super::parser::Profile::Profile2 => 2,
    }
}

fn tx_mode_number(mode: TxMode) -> u8 {
    match mode {
        TxMode::Only4x4 => 0,
        TxMode::Only8x8 => 1,
        TxMode::Only16x16 => 2,
        TxMode::Only32x32 => 3,
        TxMode::Select => 4,
    }
}

fn compound_mode_number(mode: CompoundPredictionMode) -> u8 {
    match mode {
        CompoundPredictionMode::SingleReference => 0,
        CompoundPredictionMode::CompoundReference => 1,
        CompoundPredictionMode::Select => 2,
    }
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_i16(bytes: &mut [u8], offset: usize, value: i16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
