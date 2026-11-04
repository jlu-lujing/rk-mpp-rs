//! AV1 frame header syntax (§5.9 `uncompressed_header()`).
//!
//! This is the first layer that turns **frame** bitstream bits into the
//! parameters the already-implemented post-processing components need:
//! [`LoopFilterParams`] for `deblock.rs`, the CDEF strength arrays, and the
//! loop-restoration geometry for `lr_frame.rs` / `lr_syntax.rs`.
//!
//! Everything that the specification reads from decoder state rather than
//! from the bitstream (reference-frame dimensions, order hints, previous
//! global-motion parameters, saved loop-filter deltas, the CDF/segmentation
//! state loaded by `load_cdfs()` / `load_previous()`) is supplied through
//! [`FrameHeaderState`]. The default is `setup_past_independence()`, i.e. a
//! fresh key frame; a full decoder updates it after each frame.
//!
//! `OBU_FRAME_HEADER` and `OBU_FRAME` are handled by
//! [`parse_frame_header_obu`] and [`parse_frame_obu`]. The latter performs
//! the `byte_alignment()` in `frame_obu(sz)` and returns the remaining bytes
//! as the tile-group payload. `frame_header_copy()` (the `SeenFrameHeader`
//! case) belongs to the OBU scheduler and is deliberately not implemented
//! here: it is a copy of a previous header, not a syntax parse.
//!
//! Reference: AV1 specification §5.9, §7.13, §7.14, §7.17 and §8.3.

use crate::reference::av1::bitstream::{BitReader, BitstreamError};
use crate::reference::av1::deblock::LoopFilterParams;
use crate::reference::av1::lr_frame::LrPlaneGeometry;
use crate::reference::av1::obu::{Obu, ObuError, ObuType};
use crate::reference::av1::restoration::RestorationType;
use crate::reference::av1::seq_header::SequenceHeader;
use crate::reference::av1::subexp::inverse_recenter;

pub const REFS_PER_FRAME: usize = 7;
pub const TOTAL_REFS_PER_FRAME: usize = 8;
pub const NUM_REF_FRAMES: usize = 8;
pub const MAX_SEGMENTS: usize = 8;
pub const SEG_LVL_MAX: usize = 8;
pub const SEG_LVL_REF_FRAME: usize = 5;
pub const SEG_LVL_ALT_LF_Y_V: usize = 1;
pub const MAX_LOOP_FILTER: i32 = 63;
pub const PRIMARY_REF_NONE: u8 = 7;

pub const SUPERRES_NUM: u32 = 8;
pub const SUPERRES_DENOM_MIN: u32 = 9;
pub const SUPERRES_DENOM_BITS: usize = 3;
pub const MAX_TILE_WIDTH: usize = 4096;
/// `MAX_TILE_AREA = 4096 * 2304` luma samples (§3 constants table).
pub const MAX_TILE_AREA: usize = 4096 * 2304;
pub const MAX_TILE_COLS: usize = 64;
pub const MAX_TILE_ROWS: usize = 64;
pub const RESTORATION_TILESIZE_MAX: usize = 256;

pub const WARPEDMODEL_PREC_BITS: u32 = 16;
pub const GM_ABS_ALPHA_BITS: u32 = 12;
pub const GM_ALPHA_PREC_BITS: u32 = 15;
pub const GM_ABS_TRANS_BITS: u32 = 12;
pub const GM_TRANS_PREC_BITS: u32 = 6;
pub const GM_ABS_TRANS_ONLY_BITS: u32 = 9;
pub const GM_TRANS_ONLY_PREC_BITS: u32 = 3;

/// `loop_filter_ref_deltas` / `mode_deltas` after `setup_past_independence()`.
/// Indexed by the reference-frame enum (INTRA, LAST, LAST2, LAST3, GOLDEN,
/// BWDREF, ALTREF2, ALTREF), matching [`LoopFilterParams::lossless_defaults`].
pub const DEFAULT_LF_REF_DELTAS: [i32; TOTAL_REFS_PER_FRAME] = [1, 0, 0, 0, -1, 0, -1, -1];
pub const DEFAULT_LF_MODE_DELTAS: [i32; 2] = [0, 0];

const SEG_FEATURE_BITS: [usize; SEG_LVL_MAX] = [8, 6, 6, 6, 6, 3, 0, 0];
const SEG_FEATURE_SIGNED: [bool; SEG_LVL_MAX] = [true, true, true, true, true, false, false, false];
const SEG_FEATURE_MAX: [i32; SEG_LVL_MAX] = [255, 63, 63, 63, 63, 7, 0, 0];

/// `frame_type` as transmitted by `f(2)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Key = 0,
    Inter = 1,
    IntraOnly = 2,
    Switch = 3,
}

impl FrameType {
    pub const fn from_bits(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Key),
            1 => Some(Self::Inter),
            2 => Some(Self::IntraOnly),
            3 => Some(Self::Switch),
            _ => None,
        }
    }
    pub const fn is_intra(self) -> bool {
        matches!(self, Self::Key | Self::IntraOnly)
    }
}

/// `TxMode` values used by `read_tx_mode()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxMode {
    Only4x4 = 0,
    Largest = 1,
    Select = 2,
}

/// `GmType[ref]`, ordered so that the numeric comparisons in the
/// specification (`type >= ROTZOOM`, `type >= TRANSLATION`) stay valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalMotionType {
    Identity = 0,
    Translation = 1,
    RotZoom = 2,
    Affine = 3,
}

impl GlobalMotionType {
    pub const fn from_bits(value: u8) -> Self {
        match value {
            1 => Self::Translation,
            2 => Self::RotZoom,
            3 => Self::Affine,
            _ => Self::Identity,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameHeaderError {
    Bitstream(BitstreamError),
    Obu(ObuError),
    WrongObuType(ObuType),
    BadFrameType(u32),
    BadLrUnitShift(u32),
    BadRestorationType(u32),
    /// A state-dependent field was requested but no state was supplied.
    MissingState(&'static str),
}

impl From<BitstreamError> for FrameHeaderError {
    fn from(value: BitstreamError) -> Self {
        Self::Bitstream(value)
    }
}

impl From<ObuError> for FrameHeaderError {
    fn from(value: ObuError) -> Self {
        Self::Obu(value)
    }
}

/// Decoder state that the frame header reads but does not transmit.
#[derive(Debug, Clone)]
pub struct FrameHeaderState {
    pub ref_order_hints: [u8; NUM_REF_FRAMES],
    pub ref_frame_id: [u32; NUM_REF_FRAMES],
    pub ref_upscaled_width: [u32; NUM_REF_FRAMES],
    pub ref_frame_height: [u32; NUM_REF_FRAMES],
    pub ref_render_width: [u32; NUM_REF_FRAMES],
    pub ref_render_height: [u32; NUM_REF_FRAMES],
    pub ref_frame_type: [u8; NUM_REF_FRAMES],
    /// `PrevGmParams[ref][idx]`; the past-independence default is
    /// `1 << WARPEDMODEL_PREC_BITS` for `idx % 3 == 2` and zero elsewhere.
    pub prev_gm_params: [[i32; 6]; REFS_PER_FRAME],
    /// Saved by `load_previous()` when `primary_ref_frame != PRIMARY_REF_NONE`.
    pub prev_lf_ref_deltas: [i32; TOTAL_REFS_PER_FRAME],
    pub prev_lf_mode_deltas: [i32; 2],
    pub prev_frame_id: u32,
    pub temporal_id: u8,
    pub spatial_id: u8,
    /// Override `skipModeAllowed`. `None` derives it from `ref_order_hints`,
    /// `ref_frame_idx` and `order_hint` as §7.13 specifies.
    pub skip_mode_allowed: Option<bool>,
}

impl Default for FrameHeaderState {
    fn default() -> Self {
        let mut prev_gm_params = [[0i32; 6]; REFS_PER_FRAME];
        for params in &mut prev_gm_params {
            for (idx, value) in params.iter_mut().enumerate() {
                if idx % 3 == 2 {
                    *value = 1i32 << WARPEDMODEL_PREC_BITS;
                }
            }
        }
        Self {
            ref_order_hints: [0; NUM_REF_FRAMES],
            ref_frame_id: [0; NUM_REF_FRAMES],
            ref_upscaled_width: [0; NUM_REF_FRAMES],
            ref_frame_height: [0; NUM_REF_FRAMES],
            ref_render_width: [0; NUM_REF_FRAMES],
            ref_render_height: [0; NUM_REF_FRAMES],
            ref_frame_type: [0; NUM_REF_FRAMES],
            prev_gm_params,
            prev_lf_ref_deltas: DEFAULT_LF_REF_DELTAS,
            prev_lf_mode_deltas: DEFAULT_LF_MODE_DELTAS,
            prev_frame_id: 0,
            temporal_id: 0,
            spatial_id: 0,
            skip_mode_allowed: None,
        }
    }
}

/// `film_grain_params()` fields. The point/coefficient arrays are sized to
/// the largest value the syntax can produce (`num_y_points <= 15`,
/// `ar_coeff_lag <= 3` gives `numPosChroma <= 25`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilmGrainParams {
    pub apply_grain: bool,
    pub grain_seed: u16,
    pub update_grain: bool,
    pub film_grain_params_ref_idx: u8,
    pub num_y_points: usize,
    pub point_y_value: [u8; 16],
    pub point_y_scaling: [u8; 16],
    pub chroma_scaling_from_luma: bool,
    pub num_cb_points: usize,
    pub point_cb_value: [u8; 16],
    pub point_cb_scaling: [u8; 16],
    pub num_cr_points: usize,
    pub point_cr_value: [u8; 16],
    pub point_cr_scaling: [u8; 16],
    pub grain_scaling_minus_8: u8,
    pub ar_coeff_lag: u8,
    pub ar_coeffs_y: [u8; 25],
    pub ar_coeffs_cb: [u8; 25],
    pub ar_coeffs_cr: [u8; 25],
    pub ar_coeff_shift_minus_6: u8,
    pub grain_scale_shift: u8,
    pub cb_mult: u8,
    pub cb_luma_mult: u8,
    pub cb_offset: u16,
    pub cr_mult: u8,
    pub cr_luma_mult: u8,
    pub cr_offset: u16,
    pub overlap_flag: bool,
    pub clip_to_restricted_range: bool,
}

/// Every field `uncompressed_header()` produces, in specification order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrameHeader {
    pub show_existing_frame: bool,
    pub frame_to_show_map_idx: u8,
    pub frame_type: u8,
    pub frame_is_intra: bool,
    pub show_frame: bool,
    pub showable_frame: bool,
    pub error_resilient_mode: bool,
    pub disable_cdf_update: bool,
    pub allow_screen_content_tools: u8,
    pub force_integer_mv: u8,
    pub current_frame_id: u32,
    pub prev_frame_id: u32,
    pub frame_size_override_flag: bool,
    pub order_hint: u8,
    pub primary_ref_frame: u8,
    pub buffer_removal_time_present: bool,
    pub buffer_removal_time: [u32; 8],
    pub refresh_frame_flags: u8,
    pub allow_high_precision_mv: bool,
    pub use_ref_frame_mvs: bool,
    pub allow_intrabc: bool,
    pub frame_refs_short_signaling: bool,
    pub last_frame_idx: u8,
    pub gold_frame_idx: u8,
    pub ref_frame_idx: [u8; REFS_PER_FRAME],
    pub frame_width: u32,
    pub frame_height: u32,
    pub upscaled_width: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub superres_denom: u32,
    pub use_superres: bool,
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub is_filter_switchable: bool,
    pub interpolation_filter: u8,
    pub is_motion_mode_switchable: bool,
    pub disable_frame_end_update_cdf: bool,
    pub uniform_tile_spacing_flag: bool,
    pub tile_cols: usize,
    pub tile_rows: usize,
    pub tile_cols_log2: usize,
    pub tile_rows_log2: usize,
    pub mi_col_starts: Vec<usize>,
    pub mi_row_starts: Vec<usize>,
    pub context_update_tile_id: u32,
    pub tile_size_bytes: usize,
    pub base_q_idx: u8,
    pub delta_q_y_dc: i32,
    pub delta_q_u_dc: i32,
    pub delta_q_u_ac: i32,
    pub delta_q_v_dc: i32,
    pub delta_q_v_ac: i32,
    pub diff_uv_delta: bool,
    pub using_qmatrix: bool,
    pub qm_y: u8,
    pub qm_u: u8,
    pub qm_v: u8,
    pub segmentation_enabled: bool,
    pub segmentation_update_map: bool,
    pub segmentation_temporal_update: bool,
    pub segmentation_update_data: bool,
    pub feature_enabled: [[bool; SEG_LVL_MAX]; MAX_SEGMENTS],
    pub feature_data: [[i32; SEG_LVL_MAX]; MAX_SEGMENTS],
    pub seg_id_pre_skip: bool,
    pub last_active_seg_id: u8,
    pub delta_q_present: bool,
    pub delta_q_res: u8,
    pub delta_lf_present: bool,
    pub delta_lf_res: u8,
    pub delta_lf_multi: bool,
    pub lossless_array: [bool; MAX_SEGMENTS],
    pub coded_lossless: bool,
    pub all_lossless: bool,
    pub seg_qm_level: [[u8; MAX_SEGMENTS]; 3],
    pub loop_filter_level: [u8; 4],
    pub loop_filter_sharpness: u8,
    pub loop_filter_delta_enabled: bool,
    pub loop_filter_delta_update: bool,
    pub loop_filter_ref_deltas: [i32; TOTAL_REFS_PER_FRAME],
    pub loop_filter_mode_deltas: [i32; 2],
    pub cdef_bits: u8,
    pub cdef_damping: u8,
    pub cdef_y_pri_strength: [u8; 8],
    pub cdef_y_sec_strength: [u8; 8],
    pub cdef_uv_pri_strength: [u8; 8],
    pub cdef_uv_sec_strength: [u8; 8],
    /// Raw `lr_type` values; use [`FrameHeader::restoration_type`].
    pub frame_restoration_type: [u8; 3],
    pub uses_lr: bool,
    pub uses_chroma_lr: bool,
    pub lr_unit_shift: u8,
    pub lr_uv_shift: u8,
    pub loop_restoration_size: [usize; 3],
    pub tx_mode: u8,
    pub reference_select: bool,
    pub skip_mode_allowed: bool,
    pub skip_mode_present: bool,
    pub skip_mode_frame: [u8; 2],
    pub allow_warped_motion: bool,
    pub reduced_tx_set: bool,
    pub gm_types: [u8; REFS_PER_FRAME],
    pub gm_params: [[i32; 6]; REFS_PER_FRAME],
    pub film_grain: FilmGrainParams,
    /// Bit position just after the last syntax element.
    pub bits_consumed: usize,
}

impl FrameHeader {
    pub fn frame_type(&self) -> FrameType {
        FrameType::from_bits(self.frame_type as u32).unwrap_or(FrameType::Key)
    }
    pub fn tx_mode(&self) -> TxMode {
        match self.tx_mode {
            0 => TxMode::Only4x4,
            1 => TxMode::Largest,
            _ => TxMode::Select,
        }
    }
    pub fn restoration_type(&self, plane: usize) -> RestorationType {
        RestorationType::from_header_bits(self.frame_restoration_type[plane])
    }
    pub fn global_motion_type(&self, ref_idx: usize) -> GlobalMotionType {
        GlobalMotionType::from_bits(self.gm_types[ref_idx])
    }

    /// Feed `deblock.rs` directly with the frame-header values.
    pub fn loop_filter_params(&self) -> LoopFilterParams {
        LoopFilterParams {
            level: self.loop_filter_level.map(|v| v as i32),
            sharpness: self.loop_filter_sharpness as u32,
            delta_enabled: self.loop_filter_delta_enabled,
            ref_deltas: self.loop_filter_ref_deltas,
            mode_deltas: self.loop_filter_mode_deltas,
            delta_lf_multi: self.delta_lf_multi,
        }
    }

    /// Build the §7.17 geometry `lr_frame.rs` / `lr_syntax.rs` consume.
    /// Only meaningful when [`FrameHeader::uses_lr`] is set.
    pub fn lr_geometry(&self, plane: usize, seq: &SequenceHeader) -> LrPlaneGeometry {
        let (sub_x, sub_y) = if plane == 0 {
            (0, 0)
        } else {
            (
                seq.color.subsampling_x as u32,
                seq.color.subsampling_y as u32,
            )
        };
        LrPlaneGeometry::new(
            self.frame_height as usize,
            self.upscaled_width as usize,
            self.loop_restoration_size[plane],
            sub_x,
            sub_y,
            self.restoration_type(plane),
        )
    }
}

/// `tile_log2(blkSize, target)`: smallest `k` with `blkSize << k >= target`.
pub const fn tile_log2(blk_size: usize, target: usize) -> usize {
    let mut k = 0;
    while blk_size << k < target {
        k += 1;
    }
    k
}

/// `get_relative_dist(a, b)` (§7.13): sign-extended difference of order hints.
pub fn get_relative_dist(a: i32, b: i32, enable_order_hint: bool, order_hint_bits: u8) -> i32 {
    if !enable_order_hint {
        return 0;
    }
    let diff = a - b;
    let m = 1i32 << (order_hint_bits - 1);
    (diff & (m - 1)) - (diff & m)
}

/// `decode_subexp(numSyms)` — the **bitstream** (`f`/`ns`) form used by
/// `read_global_param()`. The arithmetic-coded `NS(n)`/`decode_subexp_bool`
/// form for loop-restoration parameters lives in `subexp.rs`.
pub fn decode_subexp(r: &mut BitReader<'_>, num_syms: u32) -> Result<u32, BitstreamError> {
    let mut i = 0u32;
    let mut mk = 0u32;
    let k = 3u32;
    loop {
        let b2 = if i != 0 { k + i - 1 } else { k };
        let a = 1u32 << b2;
        if num_syms <= mk + 3 * a {
            if num_syms <= mk {
                return Err(BitstreamError::InvalidDescriptorRange(num_syms));
            }
            return Ok(r.read_ns(num_syms - mk)? + mk);
        }
        if r.read_bit()? != 0 {
            i += 1;
            mk += a;
        } else {
            return Ok(r.read_f(b2 as usize)? + mk);
        }
    }
}

/// `decode_unsigned_subexp_with_ref(mx, r)` — bitstream form.
pub fn decode_unsigned_subexp_with_ref(
    r: &mut BitReader<'_>,
    mx: i32,
    reference: i32,
) -> Result<i32, BitstreamError> {
    let v = decode_subexp(r, mx as u32)? as i32;
    if (reference << 1) <= mx {
        Ok(inverse_recenter(reference, v))
    } else {
        Ok(mx - 1 - inverse_recenter(mx - 1 - reference, v))
    }
}

/// `decode_signed_subexp_with_ref(low, high, r)` — bitstream form.
pub fn decode_signed_subexp_with_ref(
    r: &mut BitReader<'_>,
    low: i32,
    high: i32,
    reference: i32,
) -> Result<i32, BitstreamError> {
    let x = decode_unsigned_subexp_with_ref(r, high - low, reference - low)?;
    Ok(x + low)
}

struct Parser<'a, 'b> {
    r: BitReader<'a>,
    seq: &'b SequenceHeader,
    state: &'b FrameHeaderState,
    h: FrameHeader,
}

impl<'a, 'b> Parser<'a, 'b> {
    fn new(payload: &'a [u8], seq: &'b SequenceHeader, state: &'b FrameHeaderState) -> Self {
        Self {
            r: BitReader::new(payload),
            seq,
            state,
            h: FrameHeader::default(),
        }
    }

    fn f(&mut self, n: usize) -> Result<u32, FrameHeaderError> {
        Ok(self.r.read_f(n)?)
    }
    fn flag(&mut self) -> Result<bool, FrameHeaderError> {
        Ok(self.f(1)? != 0)
    }
    fn su(&mut self, n: usize) -> Result<i32, FrameHeaderError> {
        Ok(self.r.read_su(n)?)
    }
    fn ns(&mut self, n: u32) -> Result<u32, FrameHeaderError> {
        Ok(self.r.read_ns(n)?)
    }

    fn parse(&mut self) -> Result<(), FrameHeaderError> {
        let seq = self.seq;
        let id_len = if seq.frame_id_numbers_present_flag {
            seq.delta_frame_id_length as usize + seq.additional_frame_id_length as usize + 3
        } else {
            0
        };
        let all_frames = (1u32 << NUM_REF_FRAMES) - 1;

        if seq.reduced_still_picture_header {
            self.h.frame_type = FrameType::Key as u8;
            self.h.frame_is_intra = true;
            self.h.show_frame = true;
        } else {
            self.h.show_existing_frame = self.flag()?;
            if self.h.show_existing_frame {
                self.h.frame_to_show_map_idx = self.f(3)? as u8;
                if seq.decoder_model_info.is_some() && !self.equal_picture_interval() {
                    self.temporal_point_info()?;
                }
                if seq.frame_id_numbers_present_flag {
                    self.f(id_len)?;
                }
                let idx = self.h.frame_to_show_map_idx as usize;
                self.h.frame_type = *self
                    .state
                    .ref_frame_type
                    .get(idx)
                    .ok_or(FrameHeaderError::MissingState("RefFrameType"))?;
                self.h.refresh_frame_flags = if self.h.frame_type == FrameType::Key as u8 {
                    all_frames as u8
                } else {
                    0
                };
                self.h.bits_consumed = self.r.position();
                return Ok(());
            }
            let raw_type = self.f(2)?;
            let frame_type =
                FrameType::from_bits(raw_type).ok_or(FrameHeaderError::BadFrameType(raw_type))?;
            self.h.frame_type = raw_type as u8;
            self.h.frame_is_intra = frame_type.is_intra();
            self.h.show_frame = self.flag()?;
            if self.h.show_frame
                && seq.decoder_model_info.is_some()
                && !self.equal_picture_interval()
            {
                self.temporal_point_info()?;
            }
            self.h.showable_frame = if self.h.show_frame {
                frame_type != FrameType::Key
            } else {
                self.flag()?
            };
            self.h.error_resilient_mode = if frame_type == FrameType::Switch
                || (frame_type == FrameType::Key && self.h.show_frame)
            {
                true
            } else {
                self.flag()?
            };
        }

        self.h.disable_cdf_update = self.flag()?;
        self.h.allow_screen_content_tools = if seq.seq_force_screen_content_tools
            == crate::reference::av1::seq_header::SELECT_SCREEN_CONTENT_TOOLS
        {
            self.f(1)? as u8
        } else {
            seq.seq_force_screen_content_tools
        };
        self.h.force_integer_mv = if self.h.allow_screen_content_tools != 0 {
            if seq.seq_force_integer_mv == crate::reference::av1::seq_header::SELECT_INTEGER_MV {
                self.f(1)? as u8
            } else {
                seq.seq_force_integer_mv
            }
        } else {
            0
        };
        if self.h.frame_is_intra {
            self.h.force_integer_mv = 1;
        }
        self.h.prev_frame_id = self.state.prev_frame_id;
        if seq.frame_id_numbers_present_flag {
            self.h.current_frame_id = self.f(id_len)?;
        }

        let frame_type = self.frame_type();
        self.h.frame_size_override_flag = if frame_type == FrameType::Switch {
            true
        } else if seq.reduced_still_picture_header {
            false
        } else {
            self.flag()?
        };
        self.h.order_hint = self.f(seq.order_hint_bits as usize)? as u8;
        self.h.primary_ref_frame = if self.h.frame_is_intra || self.h.error_resilient_mode {
            PRIMARY_REF_NONE
        } else {
            self.f(3)? as u8
        };
        self.read_buffer_removal_times()?;

        self.h.refresh_frame_flags = if frame_type == FrameType::Switch
            || (frame_type == FrameType::Key && self.h.show_frame)
        {
            all_frames as u8
        } else {
            self.f(8)? as u8
        };
        if (!self.h.frame_is_intra || self.h.refresh_frame_flags != all_frames as u8)
            && self.h.error_resilient_mode
            && seq.enable_order_hint
        {
            for _ in 0..NUM_REF_FRAMES {
                self.f(seq.order_hint_bits as usize)?;
            }
        }

        self.read_frame_geometry()?;

        self.h.disable_frame_end_update_cdf =
            if seq.reduced_still_picture_header || self.h.disable_cdf_update {
                true
            } else {
                self.flag()?
            };

        self.tile_info()?;
        self.quantization_params()?;
        self.segmentation_params()?;
        self.delta_q_params()?;
        self.delta_lf_params()?;
        self.compute_lossless()?;
        self.loop_filter_params()?;
        self.cdef_params()?;
        self.lr_params()?;
        self.read_tx_mode()?;
        self.frame_reference_mode()?;
        self.skip_mode_params()?;
        self.h.allow_warped_motion =
            if self.h.frame_is_intra || self.h.error_resilient_mode || !seq.enable_warped_motion {
                false
            } else {
                self.flag()?
            };
        self.h.reduced_tx_set = self.flag()?;
        self.global_motion_params()?;
        self.film_grain_params()?;
        self.h.bits_consumed = self.r.position();
        Ok(())
    }

    fn frame_type(&self) -> FrameType {
        FrameType::from_bits(self.h.frame_type as u32).unwrap_or(FrameType::Key)
    }

    fn equal_picture_interval(&self) -> bool {
        self.seq
            .timing_info
            .map(|t| t.equal_picture_interval)
            .unwrap_or(false)
    }

    /// `temporal_point_info()` reads `frame_presentation_time_length_minus_1 + 1` bits.
    fn temporal_point_info(&mut self) -> Result<(), FrameHeaderError> {
        let model = self
            .seq
            .decoder_model_info
            .ok_or(FrameHeaderError::MissingState("decoder_model_info"))?;
        let n = model.frame_presentation_time_length_minus_1 as usize + 1;
        self.f(n)?;
        Ok(())
    }

    fn read_buffer_removal_times(&mut self) -> Result<(), FrameHeaderError> {
        let Some(model) = self.seq.decoder_model_info else {
            return Ok(());
        };
        if !self.flag()? {
            return Ok(());
        }
        self.h.buffer_removal_time_present = true;
        let n = model.buffer_removal_time_length_minus_1 as usize + 1;
        for (op_num, op) in self.seq.operating_points.iter().enumerate() {
            if !op.decoder_model_present {
                continue;
            }
            let idc = op.idc;
            let in_temporal = (idc >> self.state.temporal_id) & 1 != 0;
            let in_spatial = (idc >> (self.state.spatial_id + 8)) & 1 != 0;
            if idc == 0 || (in_temporal && in_spatial) {
                let value = self.f(n)?;
                if op_num < self.h.buffer_removal_time.len() {
                    self.h.buffer_removal_time[op_num] = value;
                }
            }
        }
        Ok(())
    }

    fn frame_size(&mut self) -> Result<(), FrameHeaderError> {
        let (width, height) = if self.h.frame_size_override_flag {
            (
                self.f(self.seq.frame_width_bits_minus_1 as usize + 1)? + 1,
                self.f(self.seq.frame_height_bits_minus_1 as usize + 1)? + 1,
            )
        } else {
            (self.seq.max_frame_width, self.seq.max_frame_height)
        };
        self.superres_params(width)?;
        self.h.frame_height = height;
        self.compute_image_size();
        Ok(())
    }

    fn superres_params(&mut self, upscaled_width: u32) -> Result<(), FrameHeaderError> {
        if self.seq.enable_superres {
            self.h.use_superres = self.flag()?;
        }
        self.h.superres_denom = if self.h.use_superres {
            self.f(SUPERRES_DENOM_BITS)? + SUPERRES_DENOM_MIN
        } else {
            SUPERRES_NUM
        };
        self.h.upscaled_width = upscaled_width;
        self.h.frame_width =
            (upscaled_width * SUPERRES_NUM + self.h.superres_denom / 2) / self.h.superres_denom;
        Ok(())
    }

    fn compute_image_size(&mut self) {
        self.h.mi_cols = 2 * ((self.h.frame_width + 7) >> 3) as usize;
        self.h.mi_rows = 2 * ((self.h.frame_height + 7) >> 3) as usize;
    }

    fn render_size(&mut self) -> Result<(), FrameHeaderError> {
        if self.flag()? {
            self.h.render_width = self.f(16)? + 1;
            self.h.render_height = self.f(16)? + 1;
        } else {
            self.h.render_width = self.h.upscaled_width;
            self.h.render_height = self.h.frame_height;
        }
        Ok(())
    }

    fn read_frame_geometry(&mut self) -> Result<(), FrameHeaderError> {
        if self.h.frame_is_intra {
            self.frame_size()?;
            self.render_size()?;
            if self.h.allow_screen_content_tools != 0 && self.h.upscaled_width == self.h.frame_width
            {
                self.h.allow_intrabc = self.flag()?;
            }
            return Ok(());
        }

        if self.seq.enable_order_hint {
            self.h.frame_refs_short_signaling = self.flag()?;
        }
        if self.h.frame_refs_short_signaling {
            self.h.last_frame_idx = self.f(3)? as u8;
            self.h.gold_frame_idx = self.f(3)? as u8;
        }
        let delta_bits = self.seq.delta_frame_id_length as usize;
        for i in 0..REFS_PER_FRAME {
            if !self.h.frame_refs_short_signaling {
                self.h.ref_frame_idx[i] = self.f(3)? as u8;
            }
            if self.seq.frame_id_numbers_present_flag {
                self.f(delta_bits)?;
            }
        }

        let with_refs = self.h.frame_size_override_flag && !self.h.error_resilient_mode;
        let mut found_ref = false;
        if with_refs {
            for _ in 0..REFS_PER_FRAME {
                if self.flag()? {
                    found_ref = true;
                    break;
                }
            }
            if found_ref {
                let idx = self.h.ref_frame_idx[0] as usize;
                let width = *self
                    .state
                    .ref_upscaled_width
                    .get(idx)
                    .ok_or(FrameHeaderError::MissingState("RefUpscaledWidth"))?;
                self.h.frame_height = *self
                    .state
                    .ref_frame_height
                    .get(idx)
                    .ok_or(FrameHeaderError::MissingState("RefFrameHeight"))?;
                self.h.render_width = *self
                    .state
                    .ref_render_width
                    .get(idx)
                    .ok_or(FrameHeaderError::MissingState("RefRenderWidth"))?;
                self.h.render_height = *self
                    .state
                    .ref_render_height
                    .get(idx)
                    .ok_or(FrameHeaderError::MissingState("RefRenderHeight"))?;
                self.superres_params(width)?;
                self.compute_image_size();
            } else {
                self.frame_size()?;
                self.render_size()?;
            }
        } else {
            self.frame_size()?;
            self.render_size()?;
        }

        self.h.allow_high_precision_mv = if self.h.force_integer_mv != 0 {
            false
        } else {
            self.flag()?
        };
        self.h.is_filter_switchable = self.flag()?;
        if !self.h.is_filter_switchable {
            self.h.interpolation_filter = self.f(2)? as u8;
        }
        self.h.is_motion_mode_switchable = self.flag()?;
        self.h.use_ref_frame_mvs = if self.h.error_resilient_mode || !self.seq.enable_ref_frame_mvs
        {
            false
        } else {
            self.flag()?
        };
        Ok(())
    }

    fn tile_info(&mut self) -> Result<(), FrameHeaderError> {
        let sb_shift = if self.seq.use_128x128_superblock {
            5
        } else {
            4
        };
        let sb_cols = (self.h.mi_cols + (1 << sb_shift) - 1) >> sb_shift;
        let sb_rows = (self.h.mi_rows + (1 << sb_shift) - 1) >> sb_shift;
        let sb_size = sb_shift + 2;
        let max_tile_width_sb = MAX_TILE_WIDTH >> sb_size;
        let max_tile_area_sb = MAX_TILE_AREA >> (2 * sb_size);
        let min_log2_tile_cols = tile_log2(max_tile_width_sb, sb_cols);
        let max_log2_tile_cols = tile_log2(1, sb_cols.min(MAX_TILE_COLS));
        let max_log2_tile_rows = tile_log2(1, sb_rows.min(MAX_TILE_ROWS));
        let min_log2_tiles = min_log2_tile_cols.max(tile_log2(max_tile_area_sb, sb_rows * sb_cols));

        self.h.uniform_tile_spacing_flag = self.flag()?;
        if self.h.uniform_tile_spacing_flag {
            let mut tile_cols_log2 = min_log2_tile_cols;
            while tile_cols_log2 < max_log2_tile_cols {
                if self.flag()? {
                    tile_cols_log2 += 1;
                } else {
                    break;
                }
            }
            let tile_width_sb = (sb_cols + (1 << tile_cols_log2) - 1) >> tile_cols_log2;
            let mut start_sb = 0;
            while start_sb < sb_cols {
                self.h.mi_col_starts.push(start_sb << sb_shift);
                start_sb += tile_width_sb;
            }
            self.h.mi_col_starts.push(self.h.mi_cols);
            self.h.tile_cols = self.h.mi_col_starts.len() - 1;
            self.h.tile_cols_log2 = tile_cols_log2;

            let min_log2_tile_rows = min_log2_tiles.saturating_sub(tile_cols_log2);
            let mut tile_rows_log2 = min_log2_tile_rows;
            while tile_rows_log2 < max_log2_tile_rows {
                if self.flag()? {
                    tile_rows_log2 += 1;
                } else {
                    break;
                }
            }
            let tile_height_sb = (sb_rows + (1 << tile_rows_log2) - 1) >> tile_rows_log2;
            let mut start_sb = 0;
            while start_sb < sb_rows {
                self.h.mi_row_starts.push(start_sb << sb_shift);
                start_sb += tile_height_sb;
            }
            self.h.mi_row_starts.push(self.h.mi_rows);
            self.h.tile_rows = self.h.mi_row_starts.len() - 1;
            self.h.tile_rows_log2 = tile_rows_log2;
        } else {
            let mut widest_tile_sb = 0;
            let mut start_sb = 0;
            while start_sb < sb_cols {
                self.h.mi_col_starts.push(start_sb << sb_shift);
                let max_width = (sb_cols - start_sb).min(max_tile_width_sb);
                let size_sb = self.ns(max_width as u32)? as usize + 1;
                widest_tile_sb = widest_tile_sb.max(size_sb);
                start_sb += size_sb;
            }
            self.h.mi_col_starts.push(self.h.mi_cols);
            self.h.tile_cols = self.h.mi_col_starts.len() - 1;
            self.h.tile_cols_log2 = tile_log2(1, self.h.tile_cols);

            let max_tile_area_sb = if min_log2_tiles > 0 {
                (sb_rows * sb_cols) >> (min_log2_tiles + 1)
            } else {
                sb_rows * sb_cols
            };
            let max_tile_height_sb = (max_tile_area_sb / widest_tile_sb.max(1)).max(1);
            let mut start_sb = 0;
            while start_sb < sb_rows {
                self.h.mi_row_starts.push(start_sb << sb_shift);
                let max_height = (sb_rows - start_sb).min(max_tile_height_sb);
                let size_sb = self.ns(max_height as u32)? as usize + 1;
                start_sb += size_sb;
            }
            self.h.mi_row_starts.push(self.h.mi_rows);
            self.h.tile_rows = self.h.mi_row_starts.len() - 1;
            self.h.tile_rows_log2 = tile_log2(1, self.h.tile_rows);
        }

        if self.h.tile_cols_log2 > 0 || self.h.tile_rows_log2 > 0 {
            self.h.context_update_tile_id = self.f(self.h.tile_rows_log2 + self.h.tile_cols_log2)?;
            self.h.tile_size_bytes = self.f(2)? as usize + 1;
        }
        Ok(())
    }

    fn read_delta_q(&mut self) -> Result<i32, FrameHeaderError> {
        if self.flag()? {
            self.su(7)
        } else {
            Ok(0)
        }
    }

    fn quantization_params(&mut self) -> Result<(), FrameHeaderError> {
        self.h.base_q_idx = self.f(8)? as u8;
        self.h.delta_q_y_dc = self.read_delta_q()?;
        let num_planes = self.seq.color.num_planes;
        if num_planes > 1 {
            self.h.diff_uv_delta = if self.seq.color.separate_uv_delta_q {
                self.flag()?
            } else {
                false
            };
            self.h.delta_q_u_dc = self.read_delta_q()?;
            self.h.delta_q_u_ac = self.read_delta_q()?;
            if self.h.diff_uv_delta {
                self.h.delta_q_v_dc = self.read_delta_q()?;
                self.h.delta_q_v_ac = self.read_delta_q()?;
            } else {
                self.h.delta_q_v_dc = self.h.delta_q_u_dc;
                self.h.delta_q_v_ac = self.h.delta_q_u_ac;
            }
        }
        self.h.using_qmatrix = self.flag()?;
        if self.h.using_qmatrix {
            self.h.qm_y = self.f(4)? as u8;
            self.h.qm_u = self.f(4)? as u8;
            self.h.qm_v = if self.seq.color.separate_uv_delta_q {
                self.f(4)? as u8
            } else {
                self.h.qm_u
            };
        }
        Ok(())
    }

    fn segmentation_params(&mut self) -> Result<(), FrameHeaderError> {
        self.h.segmentation_enabled = self.flag()?;
        if self.h.segmentation_enabled {
            if self.h.primary_ref_frame == PRIMARY_REF_NONE {
                self.h.segmentation_update_map = true;
                self.h.segmentation_temporal_update = false;
                self.h.segmentation_update_data = true;
            } else {
                self.h.segmentation_update_map = self.flag()?;
                if self.h.segmentation_update_map {
                    self.h.segmentation_temporal_update = self.flag()?;
                }
                self.h.segmentation_update_data = self.flag()?;
            }
            if self.h.segmentation_update_data {
                for i in 0..MAX_SEGMENTS {
                    for j in 0..SEG_LVL_MAX {
                        let enabled = self.flag()?;
                        self.h.feature_enabled[i][j] = enabled;
                        if enabled {
                            let value = if SEG_FEATURE_SIGNED[j] {
                                self.su(1 + SEG_FEATURE_BITS[j])?
                                    .clamp(-SEG_FEATURE_MAX[j], SEG_FEATURE_MAX[j])
                            } else {
                                (self.f(SEG_FEATURE_BITS[j])? as i32).min(SEG_FEATURE_MAX[j])
                            };
                            self.h.feature_data[i][j] = value;
                        }
                    }
                }
            }
        }
        for i in 0..MAX_SEGMENTS {
            for j in 0..SEG_LVL_MAX {
                if self.h.feature_enabled[i][j] {
                    self.h.last_active_seg_id = i as u8;
                    if j >= SEG_LVL_REF_FRAME {
                        self.h.seg_id_pre_skip = true;
                    }
                }
            }
        }
        Ok(())
    }

    fn delta_q_params(&mut self) -> Result<(), FrameHeaderError> {
        if self.h.base_q_idx > 0 {
            self.h.delta_q_present = self.flag()?;
        }
        if self.h.delta_q_present {
            self.h.delta_q_res = self.f(2)? as u8;
        }
        Ok(())
    }

    fn delta_lf_params(&mut self) -> Result<(), FrameHeaderError> {
        if self.h.delta_q_present && !self.h.allow_intrabc {
            self.h.delta_lf_present = self.flag()?;
        }
        if self.h.delta_lf_present {
            self.h.delta_lf_res = self.f(2)? as u8;
            self.h.delta_lf_multi = self.flag()?;
        }
        Ok(())
    }

    fn compute_lossless(&mut self) -> Result<(), FrameHeaderError> {
        // get_qindex(1, segmentId) ignores SEG_LVL_ALT_Q, so every segment
        // sees base_q_idx.
        let qindex = self.h.base_q_idx;
        let deltas_zero = self.h.delta_q_y_dc == 0
            && self.h.delta_q_u_dc == 0
            && self.h.delta_q_u_ac == 0
            && self.h.delta_q_v_dc == 0
            && self.h.delta_q_v_ac == 0;
        for segment in 0..MAX_SEGMENTS {
            self.h.lossless_array[segment] = qindex == 0 && deltas_zero;
            if self.h.using_qmatrix {
                for plane in 0..3 {
                    self.h.seg_qm_level[plane][segment] = if self.h.lossless_array[segment] {
                        15
                    } else {
                        [self.h.qm_y, self.h.qm_u, self.h.qm_v][plane]
                    };
                }
            }
        }
        self.h.coded_lossless = self.h.lossless_array.iter().all(|&v| v);
        self.h.all_lossless = self.h.coded_lossless && self.h.frame_width == self.h.upscaled_width;
        Ok(())
    }

    fn loop_filter_params(&mut self) -> Result<(), FrameHeaderError> {
        if self.h.coded_lossless || self.h.allow_intrabc {
            self.h.loop_filter_level = [0; 4];
            self.h.loop_filter_sharpness = 0;
            self.h.loop_filter_delta_enabled = false;
            self.h.loop_filter_ref_deltas = DEFAULT_LF_REF_DELTAS;
            self.h.loop_filter_mode_deltas = DEFAULT_LF_MODE_DELTAS;
            return Ok(());
        }
        let l0 = self.f(6)? as u8;
        let l1 = self.f(6)? as u8;
        let mut level = [l0, l1, 0, 0];
        if self.seq.color.num_planes > 1 && (l0 != 0 || l1 != 0) {
            level[2] = self.f(6)? as u8;
            level[3] = self.f(6)? as u8;
        }
        self.h.loop_filter_level = level;
        self.h.loop_filter_sharpness = self.f(3)? as u8;
        self.h.loop_filter_delta_enabled = self.flag()?;

        if self.h.primary_ref_frame == PRIMARY_REF_NONE {
            self.h.loop_filter_ref_deltas = DEFAULT_LF_REF_DELTAS;
            self.h.loop_filter_mode_deltas = DEFAULT_LF_MODE_DELTAS;
        } else {
            self.h.loop_filter_ref_deltas = self.state.prev_lf_ref_deltas;
            self.h.loop_filter_mode_deltas = self.state.prev_lf_mode_deltas;
        }
        if self.h.loop_filter_delta_enabled {
            self.h.loop_filter_delta_update = self.flag()?;
            if self.h.loop_filter_delta_update {
                for i in 0..TOTAL_REFS_PER_FRAME {
                    if self.flag()? {
                        self.h.loop_filter_ref_deltas[i] = self.su(7)?;
                    }
                }
                for i in 0..2 {
                    if self.flag()? {
                        self.h.loop_filter_mode_deltas[i] = self.su(7)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn cdef_params(&mut self) -> Result<(), FrameHeaderError> {
        if self.h.coded_lossless || self.h.allow_intrabc || !self.seq.enable_cdef {
            self.h.cdef_bits = 0;
            self.h.cdef_damping = 3;
            return Ok(());
        }
        self.h.cdef_damping = self.f(2)? as u8 + 3;
        self.h.cdef_bits = self.f(2)? as u8;
        let count = 1usize << self.h.cdef_bits;
        for i in 0..count {
            self.h.cdef_y_pri_strength[i] = self.f(4)? as u8;
            let mut y_sec = self.f(2)? as u8;
            if y_sec == 3 {
                y_sec += 1;
            }
            self.h.cdef_y_sec_strength[i] = y_sec;
            if self.seq.color.num_planes > 1 {
                self.h.cdef_uv_pri_strength[i] = self.f(4)? as u8;
                let mut uv_sec = self.f(2)? as u8;
                if uv_sec == 3 {
                    uv_sec += 1;
                }
                self.h.cdef_uv_sec_strength[i] = uv_sec;
            }
        }
        Ok(())
    }

    fn lr_params(&mut self) -> Result<(), FrameHeaderError> {
        if self.h.all_lossless || self.h.allow_intrabc || !self.seq.enable_restoration {
            return Ok(());
        }
        for plane in 0..self.seq.color.num_planes {
            let value = self.f(2)?;
            if value > 3 {
                return Err(FrameHeaderError::BadRestorationType(value));
            }
            self.h.frame_restoration_type[plane] = value as u8;
            if value != 0 {
                self.h.uses_lr = true;
                if plane > 0 {
                    self.h.uses_chroma_lr = true;
                }
            }
        }
        if !self.h.uses_lr {
            return Ok(());
        }
        self.h.lr_unit_shift = if self.seq.use_128x128_superblock {
            self.f(1)? as u8 + 1
        } else {
            let base = self.f(1)? as u8;
            if base != 0 {
                base + self.f(1)? as u8
            } else {
                0
            }
        };
        if self.h.lr_unit_shift > 2 {
            return Err(FrameHeaderError::BadLrUnitShift(
                self.h.lr_unit_shift as u32,
            ));
        }
        let unit_size = RESTORATION_TILESIZE_MAX >> (2 - self.h.lr_unit_shift);
        self.h.loop_restoration_size[0] = unit_size;
        self.h.lr_uv_shift = if self.seq.color.subsampling_x
            && self.seq.color.subsampling_y
            && self.h.uses_chroma_lr
        {
            self.f(1)? as u8
        } else {
            0
        };
        self.h.loop_restoration_size[1] = unit_size >> self.h.lr_uv_shift;
        self.h.loop_restoration_size[2] = unit_size >> self.h.lr_uv_shift;
        Ok(())
    }

    fn read_tx_mode(&mut self) -> Result<(), FrameHeaderError> {
        self.h.tx_mode = if self.h.coded_lossless {
            TxMode::Only4x4 as u8
        } else if self.flag()? {
            TxMode::Select as u8
        } else {
            TxMode::Largest as u8
        };
        Ok(())
    }

    fn frame_reference_mode(&mut self) -> Result<(), FrameHeaderError> {
        self.h.reference_select = !self.h.frame_is_intra && self.flag()?;
        Ok(())
    }

    fn relative_dist(&self, a: i32, b: i32) -> i32 {
        get_relative_dist(a, b, self.seq.enable_order_hint, self.seq.order_hint_bits)
    }

    /// §7.13 `skip_mode_params()`'s `skipModeAllowed` derivation.
    fn derive_skip_mode_allowed(&mut self) -> bool {
        if self.h.frame_is_intra || !self.h.reference_select || !self.seq.enable_order_hint {
            return false;
        }
        let order_hint = self.h.order_hint as i32;
        let mut forward: Option<(usize, i32)> = None;
        let mut backward: Option<(usize, i32)> = None;
        for i in 0..REFS_PER_FRAME {
            let hint = self.state.ref_order_hints[self.h.ref_frame_idx[i] as usize] as i32;
            let dist = self.relative_dist(hint, order_hint);
            if dist < 0 {
                if forward
                    .map(|(_, h)| self.relative_dist(hint, h) > 0)
                    .unwrap_or(true)
                {
                    forward = Some((i, hint));
                }
            } else if dist > 0
                && backward
                    .map(|(_, h)| self.relative_dist(hint, h) < 0)
                    .unwrap_or(true)
            {
                backward = Some((i, hint));
            }
        }
        let Some((forward_idx, forward_hint)) = forward else {
            return false;
        };
        if let Some((backward_idx, _)) = backward {
            self.h.skip_mode_frame[0] = forward_idx.min(backward_idx) as u8;
            self.h.skip_mode_frame[1] = forward_idx.max(backward_idx) as u8;
            true
        } else {
            let mut second: Option<(usize, i32)> = None;
            for i in 0..REFS_PER_FRAME {
                let hint = self.state.ref_order_hints[self.h.ref_frame_idx[i] as usize] as i32;
                if self.relative_dist(hint, forward_hint) < 0
                    && second
                        .map(|(_, h)| self.relative_dist(hint, h) > 0)
                        .unwrap_or(true)
                {
                    second = Some((i, hint));
                }
            }
            match second {
                Some((idx, _)) => {
                    self.h.skip_mode_frame[0] = forward_idx.min(idx) as u8;
                    self.h.skip_mode_frame[1] = forward_idx.max(idx) as u8;
                    true
                }
                None => false,
            }
        }
    }

    fn skip_mode_params(&mut self) -> Result<(), FrameHeaderError> {
        // The specification gates skip mode on these three conditions before
        // even deriving skipModeAllowed; an injected override must not bypass
        // them (intra frames have no reference order hints at all).
        let allowed =
            if self.h.frame_is_intra || !self.h.reference_select || !self.seq.enable_order_hint {
                false
            } else {
                match self.state.skip_mode_allowed {
                    Some(value) => value,
                    None => self.derive_skip_mode_allowed(),
                }
            };
        self.h.skip_mode_allowed = allowed;
        if allowed {
            self.h.skip_mode_present = self.flag()?;
        }
        Ok(())
    }

    fn global_motion_params(&mut self) -> Result<(), FrameHeaderError> {
        for params in &mut self.h.gm_params {
            for (idx, value) in params.iter_mut().enumerate() {
                *value = if idx % 3 == 2 {
                    1i32 << WARPEDMODEL_PREC_BITS
                } else {
                    0
                };
            }
        }
        if self.h.frame_is_intra {
            return Ok(());
        }
        for ref_idx in 0..REFS_PER_FRAME {
            let is_global = self.flag()?;
            let typ = if !is_global {
                GlobalMotionType::Identity
            } else if self.flag()? {
                GlobalMotionType::RotZoom
            } else if self.flag()? {
                GlobalMotionType::Translation
            } else {
                GlobalMotionType::Affine
            };
            self.h.gm_types[ref_idx] = typ as u8;
            if (typ as u8) >= GlobalMotionType::RotZoom as u8 {
                self.read_global_param(ref_idx, typ, 2)?;
                self.read_global_param(ref_idx, typ, 3)?;
                if typ == GlobalMotionType::Affine {
                    self.read_global_param(ref_idx, typ, 4)?;
                    self.read_global_param(ref_idx, typ, 5)?;
                } else {
                    self.h.gm_params[ref_idx][4] = -self.h.gm_params[ref_idx][3];
                    self.h.gm_params[ref_idx][5] = self.h.gm_params[ref_idx][2];
                }
            }
            if (typ as u8) >= GlobalMotionType::Translation as u8 {
                self.read_global_param(ref_idx, typ, 0)?;
                self.read_global_param(ref_idx, typ, 1)?;
            }
        }
        Ok(())
    }

    fn read_global_param(
        &mut self,
        ref_idx: usize,
        typ: GlobalMotionType,
        idx: usize,
    ) -> Result<(), FrameHeaderError> {
        let (mut abs_bits, mut prec_bits) = (GM_ABS_ALPHA_BITS, GM_ALPHA_PREC_BITS);
        if idx < 2 {
            if typ == GlobalMotionType::Translation {
                let penalty = u32::from(!self.h.allow_high_precision_mv);
                abs_bits = GM_ABS_TRANS_ONLY_BITS - penalty;
                prec_bits = GM_TRANS_ONLY_PREC_BITS - penalty;
            } else {
                abs_bits = GM_ABS_TRANS_BITS;
                prec_bits = GM_TRANS_PREC_BITS;
            }
        }
        let prec_diff = WARPEDMODEL_PREC_BITS - prec_bits;
        let round = if idx % 3 == 2 {
            1i32 << WARPEDMODEL_PREC_BITS
        } else {
            0
        };
        let sub = if idx % 3 == 2 { 1i32 << prec_bits } else { 0 };
        let mx = 1i32 << abs_bits;
        let reference = (self.state.prev_gm_params[ref_idx][idx] >> prec_diff) - sub;
        let value = decode_signed_subexp_with_ref(&mut self.r, -mx, mx + 1, reference)?;
        self.h.gm_params[ref_idx][idx] = (value << prec_diff) + round;
        Ok(())
    }

    fn film_grain_params(&mut self) -> Result<(), FrameHeaderError> {
        let frame_type = self.frame_type();
        self.h.film_grain = read_film_grain(
            &mut self.r,
            self.seq,
            frame_type,
            self.h.show_frame,
            self.h.showable_frame,
        )?;
        Ok(())
    }
}

/// `film_grain_params()` is split out of `Parser` so the in-progress
/// [`FilmGrainParams`] is a local value: holding a `&mut self.h.film_grain`
/// borrow across `self.r` reads is not possible.
fn read_film_grain(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    frame_type: FrameType,
    show_frame: bool,
    showable_frame: bool,
) -> Result<FilmGrainParams, FrameHeaderError> {
    let mut grain = FilmGrainParams::default();
    if !seq.film_grain_params_present || (!show_frame && !showable_frame) {
        return Ok(grain);
    }
    if r.read_f(1)? == 0 {
        return Ok(grain);
    }
    grain.apply_grain = true;
    grain.grain_seed = r.read_f(16)? as u16;
    grain.update_grain = if frame_type == FrameType::Inter {
        r.read_f(1)? != 0
    } else {
        true
    };
    if !grain.update_grain {
        grain.film_grain_params_ref_idx = r.read_f(3)? as u8;
        return Ok(grain);
    }
    grain.num_y_points = r.read_f(4)? as usize;
    for i in 0..grain.num_y_points {
        grain.point_y_value[i] = r.read_f(8)? as u8;
        grain.point_y_scaling[i] = r.read_f(8)? as u8;
    }
    if !seq.color.mono_chrome {
        grain.chroma_scaling_from_luma = r.read_f(1)? != 0;
    }
    let subsampled_420 = seq.color.subsampling_x && seq.color.subsampling_y;
    let chroma_points_disabled = seq.color.mono_chrome
        || grain.chroma_scaling_from_luma
        || (subsampled_420 && grain.num_y_points == 0);
    if !chroma_points_disabled {
        grain.num_cb_points = r.read_f(4)? as usize;
        for i in 0..grain.num_cb_points {
            grain.point_cb_value[i] = r.read_f(8)? as u8;
            grain.point_cb_scaling[i] = r.read_f(8)? as u8;
        }
        grain.num_cr_points = r.read_f(4)? as usize;
        for i in 0..grain.num_cr_points {
            grain.point_cr_value[i] = r.read_f(8)? as u8;
            grain.point_cr_scaling[i] = r.read_f(8)? as u8;
        }
    }
    grain.grain_scaling_minus_8 = r.read_f(2)? as u8;
    grain.ar_coeff_lag = r.read_f(2)? as u8;
    let lag = grain.ar_coeff_lag as usize;
    let num_pos_luma = 2 * lag * (lag + 1);
    let num_pos_chroma = num_pos_luma + usize::from(grain.num_y_points > 0);
    if grain.num_y_points > 0 {
        for i in 0..num_pos_luma {
            grain.ar_coeffs_y[i] = r.read_f(8)? as u8;
        }
    }
    if grain.chroma_scaling_from_luma || grain.num_cb_points > 0 {
        for i in 0..num_pos_chroma {
            grain.ar_coeffs_cb[i] = r.read_f(8)? as u8;
        }
    }
    if grain.chroma_scaling_from_luma || grain.num_cr_points > 0 {
        for i in 0..num_pos_chroma {
            grain.ar_coeffs_cr[i] = r.read_f(8)? as u8;
        }
    }
    grain.ar_coeff_shift_minus_6 = r.read_f(2)? as u8;
    grain.grain_scale_shift = r.read_f(2)? as u8;
    if grain.num_cb_points > 0 {
        grain.cb_mult = r.read_f(8)? as u8;
        grain.cb_luma_mult = r.read_f(8)? as u8;
        grain.cb_offset = r.read_f(9)? as u16;
    }
    if grain.num_cr_points > 0 {
        grain.cr_mult = r.read_f(8)? as u8;
        grain.cr_luma_mult = r.read_f(8)? as u8;
        grain.cr_offset = r.read_f(9)? as u16;
    }
    grain.overlap_flag = r.read_f(1)? != 0;
    grain.clip_to_restricted_range = r.read_f(1)? != 0;
    Ok(grain)
}

/// Parse `uncompressed_header()` from raw OBU payload bits.
pub fn parse_frame_header(
    payload: &[u8],
    seq: &SequenceHeader,
    state: &FrameHeaderState,
) -> Result<FrameHeader, FrameHeaderError> {
    let mut parser = Parser::new(payload, seq, state);
    parser.parse()?;
    Ok(parser.h)
}

/// Parse a whole `OBU_FRAME_HEADER`, including `trailing_bits()`.
pub fn parse_frame_header_obu(
    obu: &Obu<'_>,
    seq: &SequenceHeader,
    state: &FrameHeaderState,
) -> Result<FrameHeader, FrameHeaderError> {
    if obu.header.obu_type != ObuType::FrameHeader {
        return Err(FrameHeaderError::WrongObuType(obu.header.obu_type));
    }
    let header = parse_frame_header(obu.payload, seq, state)?;
    let trailing = obu.trailing_bits_needed(header.bits_consumed)?;
    let mut reader = BitReader::new(obu.payload);
    reader.skip_bits(header.bits_consumed)?;
    reader.read_trailing_bits(trailing)?;
    Ok(header)
}

/// The parts of `frame_obu(sz)` after the header and `byte_alignment()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameObuParts<'a> {
    pub header: FrameHeader,
    /// Remaining bytes passed to `tile_group_obu(sz - headerBytes)`.
    pub tile_group_payload: &'a [u8],
    pub header_bytes: usize,
}

/// Parse an `OBU_FRAME`: header, `byte_alignment()`, and the tile-group tail.
/// `OBU_FRAME` has no `trailing_bits()` (§5.2), so the tail is returned raw.
pub fn parse_frame_obu<'a>(
    obu: &Obu<'a>,
    seq: &SequenceHeader,
    state: &FrameHeaderState,
) -> Result<FrameObuParts<'a>, FrameHeaderError> {
    if obu.header.obu_type != ObuType::Frame {
        return Err(FrameHeaderError::WrongObuType(obu.header.obu_type));
    }
    let header = parse_frame_header(obu.payload, seq, state)?;
    let mut reader = BitReader::new(obu.payload);
    reader.skip_bits(header.bits_consumed)?;
    reader.align_to_byte()?;
    let header_bytes = reader.position() / 8;
    let tail = obu.payload.get(header_bytes..).unwrap_or(&[]);
    Ok(FrameObuParts {
        header,
        tile_group_payload: tail,
        header_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::lr_frame::count_units_in_frame;
    use crate::reference::av1::obu::{parse_ivf, parse_obu, parse_obus};
    use crate::reference::av1::seq_header::parse_sequence_header_obu;

    /// The first `OBU_SEQUENCE_HEADER` in the whole IVF (later frames only
    /// carry a temporal delimiter plus a frame OBU).
    fn sequence_of(data: &[u8]) -> SequenceHeader {
        let ivf = parse_ivf(data).unwrap();
        for frame in &ivf.frames {
            let obus = parse_obus(frame.payload).unwrap();
            for obu in &obus {
                if obu.header.obu_type == ObuType::SequenceHeader {
                    return parse_sequence_header_obu(obu, None).unwrap();
                }
            }
        }
        panic!("no sequence header in the IVF");
    }

    fn fixture(name: &str) -> &'static [u8] {
        match name {
            "64x64" => include_bytes!("../../../tests/data/av1_64x64_10.ivf"),
            "96x96" => include_bytes!("../../../tests/data/av1_96x96_10.ivf"),
            "128x128" => include_bytes!("../../../tests/data/av1_128x128_10.ivf"),
            "192x192" => include_bytes!("../../../tests/data/av1_192x192_10.ivf"),
            "320x240" => include_bytes!("../../../tests/data/av1_320x240_10.ivf"),
            "grain8" => include_bytes!("../../../tests/data/av1_film_grain_320x240_10.ivf"),
            "grain10" => include_bytes!("../../../tests/data/film_grain_10bit.ivf"),
            "tiles" => include_bytes!("../../../tests/data/av1_tiles_320x240_10.ivf"),
            _ => panic!("unknown fixture: {name}"),
        }
    }

    /// Bit position after `uncompressed_header()` for all 14 frame headers in
    /// each IVF. Derived independently with a separate Python bit parser,
    /// cross-checked against `ffmpeg -bsf:v trace_headers` (which prints every
    /// syntax element with its absolute bit position). A one-bit disagreement
    /// anywhere in a header shows up here as a wrong `bits_consumed`.
    const ORACLE_BITS: &[(&str, &[usize])] = &[
        (
            "128x128",
            &[
                85, 146, 135, 135, 131, 4, 131, 4, 135, 131, 4, 131, 131, 118,
            ],
        ),
        (
            "192x192",
            &[
                97, 146, 147, 135, 131, 4, 131, 4, 135, 131, 4, 131, 131, 130,
            ],
        ),
        (
            "320x240",
            &[
                121, 146, 147, 135, 131, 4, 131, 4, 135, 131, 4, 131, 130, 142,
            ],
        ),
        (
            "64x64",
            &[
                80, 129, 130, 130, 129, 4, 129, 4, 130, 129, 4, 128, 128, 128,
            ],
        ),
        (
            "96x96",
            &[85, 134, 135, 132, 131, 4, 131, 4, 135, 131, 4, 131, 131, 4],
        ),
        (
            "grain8",
            &[
                993, 150, 1044, 1042, 1040, 4, 1028, 4, 1032, 1028, 4, 1028, 1027, 1039,
            ],
        ),
        (
            "tiles",
            &[
                103, 152, 153, 141, 137, 4, 137, 4, 141, 137, 4, 137, 136, 148,
            ],
        ),
        (
            "grain10",
            &[
                993, 150, 1044, 1042, 1040, 4, 1028, 4, 1032, 1028, 4, 1028, 1027, 1027,
            ],
        ),
    ];

    struct Probe {
        obu_type: ObuType,
        header: FrameHeader,
        header_bytes: usize,
        tile_payload: usize,
        payload_len: usize,
    }

    fn probe_stream(data: &[u8], skip_mode_allowed: Option<bool>) -> Vec<Probe> {
        let ivf = parse_ivf(data).unwrap();
        let state = FrameHeaderState {
            skip_mode_allowed,
            ..FrameHeaderState::default()
        };
        let mut seq: Option<SequenceHeader> = None;
        let mut out = Vec::new();
        for frame in &ivf.frames {
            let obus = parse_obus(frame.payload).unwrap();
            for obu in &obus {
                match obu.header.obu_type {
                    ObuType::SequenceHeader => {
                        seq = Some(parse_sequence_header_obu(obu, None).unwrap());
                    }
                    ObuType::Frame => {
                        let s = seq.as_ref().expect("frame before sequence header");
                        let parts = parse_frame_obu(obu, s, &state).unwrap();
                        out.push(Probe {
                            obu_type: ObuType::Frame,
                            header: parts.header,
                            header_bytes: parts.header_bytes,
                            tile_payload: parts.tile_group_payload.len(),
                            payload_len: obu.payload.len(),
                        });
                    }
                    ObuType::FrameHeader => {
                        let s = seq.as_ref().expect("frame header before sequence header");
                        let header = parse_frame_header_obu(obu, s, &state).unwrap();
                        out.push(Probe {
                            obu_type: ObuType::FrameHeader,
                            header_bytes: (header.bits_consumed + 7) / 8,
                            header,
                            tile_payload: 0,
                            payload_len: obu.payload.len(),
                        });
                    }
                    _ => {}
                }
            }
        }
        out
    }

    #[test]
    fn real_ivf_frame_headers_match_independent_oracle() {
        let mut total = 0usize;
        let mut show_existing = 0usize;
        let mut with_reference_select = 0usize;
        for (name, expected) in ORACLE_BITS {
            let probes = probe_stream(fixture(name), Some(true));
            assert_eq!(probes.len(), expected.len(), "{name}: frame count");
            for (i, (p, want)) in probes.iter().zip(expected.iter()).enumerate() {
                assert_eq!(p.header.bits_consumed, *want, "{name} frame {i}: bits");
                // byte_alignment()/trailing_bits() must land exactly on the
                // byte boundary implied by the consumed bit count.
                assert_eq!(
                    p.header_bytes,
                    (p.header.bits_consumed + 7) / 8,
                    "{name} frame {i}: alignment"
                );
                if p.obu_type == ObuType::Frame {
                    assert!(!p.header.show_existing_frame, "{name} frame {i}");
                    assert_eq!(
                        p.tile_payload,
                        p.payload_len - p.header_bytes,
                        "{name} frame {i}: tile group tail"
                    );
                    assert!(p.tile_payload > 0, "{name} frame {i}: empty tile group");
                } else {
                    show_existing += 1;
                    assert!(p.header.show_existing_frame, "{name} frame {i}");
                    // With the default (past-independent) state every
                    // reference is a key frame, so show_existing refreshes all.
                    assert_eq!(p.header.refresh_frame_flags, 0xff, "{name} frame {i}");
                }
                if p.header.reference_select {
                    with_reference_select += 1;
                    assert!(p.header.skip_mode_allowed, "{name} frame {i}");
                } else {
                    assert!(!p.header.skip_mode_allowed, "{name} frame {i}");
                }
                total += 1;
            }
        }
        assert_eq!(total, 112);
        assert!(show_existing > 0, "no OBU_FRAME_HEADER was exercised");
        assert!(
            with_reference_select > 0,
            "no skip_mode_present bit was exercised"
        );
    }

    #[test]
    fn key_frame_field_snapshots_match_python_parser() {
        // name, width, height, base_q_idx, loop_filter_level, cdef_bits,
        // tile_cols, tile_rows, apply_grain, num_y_points, delta_q_present, bits
        let cases: &[(
            &str,
            u32,
            u32,
            u8,
            [u8; 4],
            u8,
            usize,
            usize,
            bool,
            usize,
            bool,
            usize,
        )] = &[
            (
                "128x128",
                128,
                128,
                119,
                [9, 9, 11, 10],
                0,
                1,
                1,
                false,
                0,
                true,
                85,
            ),
            (
                "192x192",
                192,
                192,
                126,
                [2, 2, 3, 4],
                1,
                1,
                1,
                false,
                0,
                true,
                97,
            ),
            (
                "320x240",
                320,
                240,
                96,
                [2, 2, 8, 12],
                2,
                1,
                1,
                false,
                0,
                true,
                121,
            ),
            (
                "64x64",
                64,
                64,
                63,
                [3, 3, 4, 4],
                0,
                1,
                1,
                false,
                0,
                false,
                80,
            ),
            (
                "96x96",
                96,
                96,
                123,
                [6, 6, 4, 10],
                0,
                1,
                1,
                false,
                0,
                true,
                85,
            ),
            (
                "grain8",
                320,
                240,
                102,
                [6, 6, 8, 12],
                1,
                1,
                1,
                true,
                14,
                true,
                993,
            ),
            (
                "tiles",
                320,
                240,
                96,
                [6, 6, 6, 12],
                1,
                2,
                2,
                false,
                0,
                true,
                103,
            ),
            (
                "grain10",
                320,
                240,
                101,
                [7, 7, 8, 11],
                1,
                1,
                1,
                true,
                14,
                true,
                993,
            ),
        ];
        for c in cases {
            let probes = probe_stream(fixture(c.0), Some(true));
            let h = &probes[0].header;
            assert_eq!(h.bits_consumed, c.11, "{}", c.0);
            assert_eq!(h.frame_type(), FrameType::Key, "{}", c.0);
            assert!(h.show_frame && h.frame_is_intra, "{}", c.0);
            assert_eq!(h.frame_width, c.1, "{} width", c.0);
            assert_eq!(h.frame_height, c.2, "{} height", c.0);
            assert_eq!(h.upscaled_width, c.1, "{} upscaled width", c.0);
            assert_eq!(h.render_width, c.1, "{} render width", c.0);
            assert_eq!(h.render_height, c.2, "{} render height", c.0);
            assert_eq!(h.superres_denom, SUPERRES_NUM, "{} superres", c.0);
            assert!(!h.use_superres, "{} superres", c.0);
            assert_eq!(h.base_q_idx, c.3, "{} q", c.0);
            assert_eq!(h.loop_filter_level, c.4, "{} lf", c.0);
            assert_eq!(h.cdef_bits, c.5, "{} cdef", c.0);
            assert_eq!(h.tile_cols, c.6, "{} tile cols", c.0);
            assert_eq!(h.tile_rows, c.7, "{} tile rows", c.0);
            assert_eq!(h.film_grain.apply_grain, c.8, "{} grain", c.0);
            assert_eq!(h.film_grain.num_y_points, c.9, "{} grain points", c.0);
            assert_eq!(h.delta_q_present, c.10, "{} delta q", c.0);
            assert!(!h.uses_lr, "{} uses_lr", c.0);
        }
    }

    #[test]
    fn frame_obu_splits_header_from_tile_group() {
        // (name, header_bytes, OBU payload length) from the Python oracle.
        for (name, header_bytes, payload) in [
            ("64x64", 10, 860),
            ("96x96", 11, 792),
            ("128x128", 11, 1083),
            ("192x192", 13, 1274),
            ("320x240", 16, 2259),
            ("grain8", 125, 2271),
            ("grain10", 125, 2299),
            ("tiles", 13, 2333),
        ] {
            let p = &probe_stream(fixture(name), Some(true))[0];
            assert_eq!(p.obu_type, ObuType::Frame, "{name}");
            assert_eq!(p.header_bytes, header_bytes, "{name}");
            assert_eq!(p.payload_len, payload, "{name}");
            assert_eq!(p.tile_payload, payload - header_bytes, "{name}");
        }
    }

    #[test]
    fn film_grain_fields_match_ffmpeg_trace_headers() {
        // `ffmpeg -bsf:v trace_headers` on av1_film_grain_320x240_10.ivf,
        // first Frame Header. Bit positions there include the 3-byte OBU
        // header, so 1016 (clip_to_restricted_range) maps to 993 payload bits.
        let h = &probe_stream(fixture("grain8"), Some(true))[0].header;
        let g = &h.film_grain;
        assert_eq!(h.bits_consumed, 993);
        assert_eq!(h.base_q_idx, 102);
        assert_eq!(h.loop_filter_level, [6, 6, 8, 12]);
        assert_eq!(h.loop_filter_sharpness, 0);
        assert_eq!(h.cdef_bits, 1);
        assert!(g.apply_grain && g.update_grain);
        assert_eq!(g.grain_seed, 45231);
        assert_eq!(g.num_y_points, 14);
        assert_eq!(g.point_y_value[0], 16);
        assert_eq!(g.point_y_scaling[0], 0);
        assert!(!g.chroma_scaling_from_luma);
        assert_eq!(g.num_cb_points, 8);
        assert_eq!(g.point_cb_value[0], 16);
        assert_eq!(g.point_cb_scaling[0], 0);
        assert_eq!(g.num_cr_points, 9);
        assert_eq!(g.point_cr_value[0], 16);
        assert_eq!(g.point_cr_scaling[0], 0);
        assert_eq!(g.grain_scaling_minus_8, 3);
        assert_eq!(g.ar_coeff_lag, 2);
        assert_eq!(g.ar_coeffs_y[0], 128);
        assert_eq!(g.ar_coeffs_y[2], 70);
        assert_eq!(g.ar_coeffs_cb[0], 128);
        assert_eq!(g.ar_coeffs_cb[2], 79);
        assert_eq!(g.ar_coeff_shift_minus_6, 2);
        assert_eq!(g.grain_scale_shift, 0);
        assert_eq!(g.cb_mult, 247);
        assert_eq!(g.cb_luma_mult, 192);
        assert_eq!(g.cb_offset, 18);
        assert_eq!(g.cr_mult, 229);
        assert_eq!(g.cr_luma_mult, 192);
        assert_eq!(g.cr_offset, 54);
        assert!(!g.overlap_flag);
        assert!(g.clip_to_restricted_range);
    }

    #[test]
    fn wrong_obu_type_and_truncated_payload_are_rejected() {
        let ivf = parse_ivf(fixture("64x64")).unwrap();
        let obus = parse_obus(ivf.frames[0].payload).unwrap();
        let seq_obu = obus
            .iter()
            .find(|o| o.header.obu_type == ObuType::SequenceHeader)
            .unwrap();
        let frame_obu = obus
            .iter()
            .find(|o| o.header.obu_type == ObuType::Frame)
            .unwrap();
        let seq = parse_sequence_header_obu(seq_obu, None).unwrap();
        let state = FrameHeaderState::default();

        assert_eq!(
            parse_frame_obu(seq_obu, &seq, &state).unwrap_err(),
            FrameHeaderError::WrongObuType(ObuType::SequenceHeader)
        );
        assert_eq!(
            parse_frame_header_obu(frame_obu, &seq, &state).unwrap_err(),
            FrameHeaderError::WrongObuType(ObuType::Frame)
        );
        assert!(matches!(
            parse_frame_header(&[], &seq, &state),
            Err(FrameHeaderError::Bitstream(
                BitstreamError::OutOfBits { .. }
            ))
        ));
    }

    #[test]
    fn frame_header_obu_requires_the_trailing_one_bit() {
        // OBU_FRAME_HEADER (type 3) with has_size_field=1 and a one-byte
        // payload: show_existing_frame=1, frame_to_show_map_idx=000,
        // trailing_bits() = 1 000.
        let ivf = parse_ivf(fixture("64x64")).unwrap();
        let obus = parse_obus(ivf.frames[0].payload).unwrap();
        let seq = parse_sequence_header_obu(&obus[1], None).unwrap();
        let state = FrameHeaderState::default();

        let good = [0x1a, 0x01, 0x88];
        let (obu, _) = parse_obu(&good, None).unwrap();
        let h = parse_frame_header_obu(&obu, &seq, &state).unwrap();
        assert!(h.show_existing_frame);
        assert_eq!(h.frame_to_show_map_idx, 0);
        assert_eq!(h.bits_consumed, 4);

        let bad = [0x1a, 0x01, 0x80];
        let (obu, _) = parse_obu(&bad, None).unwrap();
        assert_eq!(
            parse_frame_header_obu(&obu, &seq, &state).unwrap_err(),
            FrameHeaderError::Bitstream(BitstreamError::MissingTrailingOneBit)
        );
    }

    #[test]
    fn tile_log2_and_relative_dist_vectors() {
        assert_eq!(tile_log2(1, 1), 0);
        assert_eq!(tile_log2(1, 2), 1);
        assert_eq!(tile_log2(64, 320), 3);
        assert_eq!(tile_log2(64, 4096), 6);
        // MAX_TILE_AREA is a pixel area (4096 * 2304), not a tile count.
        assert_eq!(MAX_TILE_AREA, 4096 * 2304);

        assert_eq!(get_relative_dist(4, 2, false, 7), 0);
        assert_eq!(get_relative_dist(4, 2, true, 7), 2);
        assert_eq!(get_relative_dist(2, 4, true, 7), -2);
        // The signed result always has magnitude < 1 << (bits - 1); with
        // OrderHintBits=7 the wrap picks -62 rather than +66 for (1, 63).
        assert_eq!(get_relative_dist(1, 63, true, 7), -62);
        assert_eq!(get_relative_dist(63, 1, true, 7), 62);
        assert_eq!(get_relative_dist(0, 126, true, 7), 2);
        assert_eq!(get_relative_dist(126, 0, true, 7), -2);
    }

    #[test]
    fn decode_subexp_bitstream_vectors() {
        // (num_syms, expected value, MSB-first bytes, bits consumed).
        // Vectors produced by an independent Python implementation of the
        // §8.3 subexp encoder and verified against its own decoder.
        let vectors: &[(u32, u32, &[u8], usize)] = &[
            (10, 0, &[0x00, 0x00], 3),
            (10, 1, &[0x20, 0x00], 3),
            (10, 5, &[0xa0, 0x00], 3),
            (10, 6, &[0xc0, 0x00], 4),
            (10, 7, &[0xd0, 0x00], 4),
            (10, 9, &[0xf0, 0x00], 4),
            (24, 0, &[0x00, 0x00], 4),
            (24, 8, &[0x80, 0x00], 5),
            (24, 23, &[0xf8, 0x00], 5),
            (100, 0, &[0x00, 0x00], 4),
            (100, 7, &[0x70, 0x00], 4),
            (100, 8, &[0x80, 0x00], 5),
            (100, 23, &[0xce, 0x00], 7),
            (100, 24, &[0xd0, 0x00], 7),
            (100, 99, &[0xff, 0xc0, 0x00], 10),
            (1000, 0, &[0x00, 0x00], 4),
            (1000, 8, &[0x80, 0x00], 5),
            (1000, 64, &[0xf0, 0x00, 0x00], 11),
            (1000, 100, &[0xf4, 0x80, 0x00], 11),
            (1000, 999, &[0xff, 0xff, 0x00], 16),
        ];
        for (num_syms, want, bytes, bits) in vectors {
            let mut r = BitReader::new(bytes);
            let got = decode_subexp(&mut r, *num_syms).unwrap();
            assert_eq!(got, *want, "num_syms={num_syms}");
            assert_eq!(r.position(), *bits, "num_syms={num_syms} position");
        }
    }

    #[test]
    fn subexp_with_ref_recenters_around_the_reference() {
        // (mx, reference, subexp value, expected result, bytes, bits consumed).
        let vectors: &[(i32, i32, i32, &[u8], usize)] = &[
            (20, 5, 5, &[0x00, 0x00], 4),
            (20, 5, 4, &[0x10, 0x00], 4),
            (20, 5, 10, &[0xa0, 0x00], 4),
            (20, 5, 19, &[0xf8, 0x00], 5),
            (20, 15, 15, &[0x00, 0x00], 4),
            (20, 15, 16, &[0x10, 0x00], 4),
            (20, 15, 9, &[0xa0, 0x00], 4),
            (20, 15, 0, &[0xf8, 0x00], 5),
            (4096, 0, 0, &[0x00, 0x00], 4),
            (4096, 0, 1, &[0x10, 0x00], 4),
            (4096, 0, 4095, &[0xff, 0xff, 0xf0, 0x00], 20),
        ];
        for (mx, reference, want, bytes, bits) in vectors {
            let mut r = BitReader::new(bytes);
            let got = decode_unsigned_subexp_with_ref(&mut r, *mx, *reference).unwrap();
            assert_eq!(got, *want, "mx={mx} ref={reference}");
            assert_eq!(r.position(), *bits, "mx={mx} ref={reference} position");
        }

        // The signed form is just the unsigned form shifted by `low`. Here
        // mx = 8193 and reference - low = 4096, so v=0 maps to 0 and v=1 to
        // 4095, i.e. signed 0 and -1.
        let mut r = BitReader::new(&[0x00, 0x00]);
        assert_eq!(
            decode_signed_subexp_with_ref(&mut r, -4096, 4097, 0).unwrap(),
            0
        );
        let mut r = BitReader::new(&[0x10, 0x00]);
        assert_eq!(
            decode_signed_subexp_with_ref(&mut r, -4096, 4097, 0).unwrap(),
            -1
        );
    }

    #[test]
    fn frame_header_feeds_deblock_and_lr_geometry() {
        let h = probe_stream(fixture("320x240"), Some(true))[0]
            .header
            .clone();
        let lf = h.loop_filter_params();
        assert_eq!(lf.level, [2, 2, 8, 12]);
        assert_eq!(lf.ref_deltas, DEFAULT_LF_REF_DELTAS);
        assert_eq!(lf.mode_deltas, DEFAULT_LF_MODE_DELTAS);

        let seq = sequence_of(fixture("320x240"));
        assert!(seq.color.subsampling_x && seq.color.subsampling_y);

        let mut synthetic = FrameHeader::default();
        synthetic.frame_height = 240;
        synthetic.upscaled_width = 320;
        synthetic.loop_restoration_size = [128, 64, 64];
        synthetic.frame_restoration_type = [1, 2, 3];
        let y = synthetic.lr_geometry(0, &seq);
        assert_eq!(y.unit_size, 128);
        assert_eq!(y.unit_cols, count_units_in_frame(128, 320));
        assert_eq!(y.unit_rows, count_units_in_frame(128, 240));
        assert_eq!(y.frame_type, RestorationType::Switchable);
        let uv = synthetic.lr_geometry(1, &seq);
        assert_eq!(uv.sub_x, 1);
        assert_eq!(uv.sub_y, 1);
        assert_eq!(uv.unit_size, 64);
        assert_eq!(uv.unit_cols, count_units_in_frame(64, 160));
        assert_eq!(uv.unit_rows, count_units_in_frame(64, 120));
        assert_eq!(uv.frame_type, RestorationType::Wiener);
        assert_eq!(
            synthetic.lr_geometry(2, &seq).frame_type,
            RestorationType::SelfGuided
        );
    }

    #[test]
    fn injected_skip_mode_state_changes_the_parse() {
        // Some inter frames transmit reference_select=1. With the oracle's
        // state (skipModeAllowed=1) one skip_mode_present bit is read; forcing
        // it to 0 changes the parse. Some IVF packets hold several OBU_FRAMEs,
        // so walk the OBUs themselves rather than assuming one header per
        // container frame.
        let data = fixture("320x240");
        let seq = sequence_of(data);
        let with = FrameHeaderState {
            skip_mode_allowed: Some(true),
            ..FrameHeaderState::default()
        };
        let without = FrameHeaderState {
            skip_mode_allowed: Some(false),
            ..FrameHeaderState::default()
        };
        let mut seen = 0usize;
        for frame in &parse_ivf(data).unwrap().frames {
            let obus = parse_obus(frame.payload).unwrap();
            for obu in &obus {
                if obu.header.obu_type != ObuType::Frame {
                    continue;
                }
                let a = parse_frame_obu(obu, &seq, &with).unwrap();
                if !a.header.reference_select {
                    continue;
                }
                seen += 1;
                assert!(a.header.skip_mode_allowed);
                match parse_frame_obu(obu, &seq, &without) {
                    Ok(b) => {
                        assert!(!b.header.skip_mode_allowed);
                        assert_ne!(
                            b.header.bits_consumed, a.header.bits_consumed,
                            "an injected skipModeAllowed must change the parse"
                        );
                    }
                    // Dropping a transmitted bit can also leave a later
                    // variable-length descriptor reading past the payload.
                    Err(_) => {}
                }
            }
            if seen > 0 {
                break;
            }
        }
        assert!(seen > 0, "no frame with reference_select=1");
    }
}
