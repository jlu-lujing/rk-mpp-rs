//! IVF framing and AV1 Open Bitstream Unit parsing.
//!
//! The OBU boundary rules follow `av1d_get_fragment_units()` and
//! `av1d_read_obu_header()` in the official MPP.

use std::fmt;

use super::cdf::{Av1Cdf, Av1CdfStore, Av1Cdfs, Av1MvCdfs};
use super::global_motion::WarpedMotion;

const IVF_HEADER_MIN_SIZE: usize = 32;
const IVF_FRAME_HEADER_SIZE: usize = 12;
pub const NUM_REF_FRAMES: usize = 8;
pub const REFS_PER_FRAME: usize = 7;
const TOTAL_REFS_PER_FRAME: usize = 8;
const PRIMARY_REF_NONE: u8 = 7;
const MAX_SEGMENTS: usize = 8;
const SEG_LVL_FEATURES: usize = 8;
const MAX_TILE_COLS: usize = 64;
const MAX_TILE_ROWS: usize = 64;
pub const MAX_TILES: usize = 128;
const MAX_TILE_WIDTH: u32 = 4096;
const MAX_TILE_AREA: u32 = 4096 * 2304;
pub const MAX_OPERATING_POINTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Av1Error {
    BadIvf,
    TruncatedIvfFrameHeader,
    BadFrameCount { declared: u32, actual: usize },
    BadObu,
    BadLeb128,
    ObuExceedsPacket { needed: usize, packet: usize },
    Truncated { bit: usize, width: u8 },
    InvalidValue(&'static str),
}

impl fmt::Display for Av1Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Av1Error::BadIvf => write!(f, "invalid or truncated IVF container"),
            Av1Error::TruncatedIvfFrameHeader => {
                write!(f, "truncated IVF frame header")
            }
            Av1Error::BadFrameCount { declared, actual } => {
                write!(f, "IVF declares {declared} frames but contains {actual}")
            }
            Av1Error::BadObu => write!(f, "invalid AV1 OBU header"),
            Av1Error::BadLeb128 => write!(f, "invalid AV1 OBU LEB128 size"),
            Av1Error::ObuExceedsPacket { needed, packet } => {
                write!(f, "AV1 OBU length {needed} exceeds packet size {packet}")
            }
            Av1Error::Truncated { bit, width } => {
                write!(
                    f,
                    "AV1 bitstream truncated at bit {bit} reading {width} bits"
                )
            }
            Av1Error::InvalidValue(field) => write!(f, "invalid AV1 field: {field}"),
        }
    }
}

impl std::error::Error for Av1Error {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IvfFrame<'a> {
    pub timestamp: u64,
    pub data: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ivf<'a> {
    pub version: u16,
    pub width: u16,
    pub height: u16,
    pub frame_rate_numerator: u32,
    pub frame_rate_denominator: u32,
    pub frames: Vec<IvfFrame<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObuType {
    Reserved0 = 0,
    SequenceHeader = 1,
    TemporalDelimiter = 2,
    FrameHeader = 3,
    TileGroup = 4,
    Metadata = 5,
    Frame = 6,
    RedundantFrameHeader = 7,
    TileList = 8,
    Reserved9 = 9,
    Reserved10 = 10,
    Reserved11 = 11,
    Reserved12 = 12,
    Reserved13 = 13,
    Reserved14 = 14,
    Padding = 15,
}

impl ObuType {
    fn from_bits(value: u8) -> Self {
        match value {
            0 => Self::Reserved0,
            1 => Self::SequenceHeader,
            2 => Self::TemporalDelimiter,
            3 => Self::FrameHeader,
            4 => Self::TileGroup,
            5 => Self::Metadata,
            6 => Self::Frame,
            7 => Self::RedundantFrameHeader,
            8 => Self::TileList,
            9 => Self::Reserved9,
            10 => Self::Reserved10,
            11 => Self::Reserved11,
            12 => Self::Reserved12,
            13 => Self::Reserved13,
            14 => Self::Reserved14,
            15 => Self::Padding,
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObuHeader {
    pub obu_type: ObuType,
    pub extension_flag: bool,
    pub has_size_field: bool,
    pub temporal_id: u8,
    pub spatial_id: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Obu<'a> {
    pub header: ObuHeader,
    pub obu_type: ObuType,
    pub temporal_id: u8,
    pub spatial_id: u8,
    /// Byte offset of `raw` from the start of the packet.
    pub start: usize,
    /// OBU payload, without the header, extension byte and size field.
    pub payload: &'a [u8],
    /// Complete OBU bytes, including header and size field.
    pub raw: &'a [u8],
}

/// Whether an OBU survives MPP's selected operating-point layer filter.
pub fn obu_in_operating_point(obu: &Obu<'_>, operating_point_idc: u16) -> bool {
    if !obu.header.extension_flag
        || operating_point_idc == 0
        || matches!(
            obu.obu_type,
            ObuType::SequenceHeader | ObuType::TemporalDelimiter
        )
    {
        return true;
    }
    let in_temporal_layer = (operating_point_idc >> obu.temporal_id) & 1 != 0;
    let in_spatial_layer = (operating_point_idc >> (obu.spatial_id + 8)) & 1 != 0;
    in_temporal_layer && in_spatial_layer
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimingInfo {
    pub num_units_in_display_tick: u32,
    pub time_scale: u32,
    pub equal_picture_interval: bool,
    pub num_ticks_per_picture_minus_1: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DecoderModelInfo {
    pub buffer_delay_length_minus_1: u8,
    pub num_units_in_decoding_tick: u32,
    pub buffer_removal_time_length_minus_1: u8,
    pub frame_presentation_time_length_minus_1: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperatingPoint {
    pub idc: u16,
    pub level_idx: u8,
    pub tier: u8,
    pub decoder_model_present: bool,
    pub decoder_buffer_delay: u32,
    pub encoder_buffer_delay: u32,
    pub low_delay_mode: bool,
    pub initial_display_delay_present: bool,
    pub initial_display_delay_minus_1: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorConfig {
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub mono_chrome: bool,
    pub color_description_present: bool,
    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub color_range: bool,
    pub subsampling_x: bool,
    pub subsampling_y: bool,
    pub chroma_sample_position: u8,
    pub separate_uv_delta_q: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceHeader {
    pub profile: u8,
    pub still_picture: bool,
    pub reduced_still_picture_header: bool,
    pub timing_info_present: bool,
    pub timing_info: TimingInfo,
    pub decoder_model_info_present: bool,
    pub decoder_model_info: DecoderModelInfo,
    pub initial_display_delay_present: bool,
    pub operating_points: Vec<OperatingPoint>,
    pub frame_width_bits: u8,
    pub frame_height_bits: u8,
    pub max_frame_width: u16,
    pub max_frame_height: u16,
    pub frame_id_numbers_present: bool,
    pub delta_frame_id_length_minus_2: u8,
    pub additional_frame_id_length_minus_1: u8,
    pub use_128x128_superblock: bool,
    pub enable_filter_intra: bool,
    pub enable_intra_edge_filter: bool,
    pub enable_interintra_compound: bool,
    pub enable_masked_compound: bool,
    pub enable_warped_motion: bool,
    pub enable_dual_filter: bool,
    pub enable_order_hint: bool,
    pub enable_jnt_comp: bool,
    pub enable_ref_frame_mvs: bool,
    /// 0..2, where 2 means "select".
    pub seq_force_screen_content_tools: u8,
    /// 0..2, where 2 means "select".
    pub seq_force_integer_mv: u8,
    pub order_hint_bits: u8,
    pub enable_superres: bool,
    pub enable_cdef: bool,
    pub enable_restoration: bool,
    pub color_config: ColorConfig,
    pub bit_depth: u8,
    pub film_grain_params_present: bool,
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    fn read_bit(&mut self) -> Result<bool, Av1Error> {
        Ok(self.read_bits(1)? != 0)
    }

    fn read_bits(&mut self, width: u8) -> Result<u32, Av1Error> {
        if width > 32 {
            return Err(Av1Error::InvalidValue("bit width"));
        }
        if self.data.len() * 8 - self.bit < width as usize {
            return Err(Av1Error::Truncated {
                bit: self.bit,
                width,
            });
        }
        let mut value = 0u32;
        for _ in 0..width {
            let byte = self.data[self.bit / 8];
            value = (value << 1) | u32::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Ok(value)
    }

    fn read_bits_u64(&mut self, width: u8) -> Result<u64, Av1Error> {
        if width > 32 {
            return Err(Av1Error::InvalidValue("bit width"));
        }
        Ok(u64::from(self.read_bits(width)?))
    }

    fn align(&mut self) -> Result<(), Av1Error> {
        let aligned = self
            .bit
            .checked_add(7)
            .ok_or(Av1Error::InvalidValue("bit position"))?
            & !7;
        if aligned > self.data.len() * 8 {
            return Err(Av1Error::Truncated {
                bit: self.bit,
                width: (aligned - self.bit) as u8,
            });
        }
        self.bit = aligned;
        Ok(())
    }
}

fn read_timing_info(r: &mut BitReader<'_>) -> Result<TimingInfo, Av1Error> {
    let mut info = TimingInfo {
        num_units_in_display_tick: r.read_bits(32)?,
        time_scale: r.read_bits(32)?,
        ..TimingInfo::default()
    };
    info.equal_picture_interval = r.read_bit()?;
    if info.equal_picture_interval {
        info.num_ticks_per_picture_minus_1 = read_ue(r)?;
    }
    Ok(info)
}

fn read_decoder_model_info(r: &mut BitReader<'_>) -> Result<DecoderModelInfo, Av1Error> {
    Ok(DecoderModelInfo {
        buffer_delay_length_minus_1: r.read_bits(5)? as u8,
        num_units_in_decoding_tick: r.read_bits(32)?,
        buffer_removal_time_length_minus_1: r.read_bits(5)? as u8,
        frame_presentation_time_length_minus_1: r.read_bits(5)? as u8,
    })
}

fn invalidate_reference_frame_ids(
    references: &mut [ReferenceFrame; NUM_REF_FRAMES],
    current_frame_id: u32,
    id_len: u8,
    diff_len: u8,
) {
    let current = i64::from(current_frame_id);
    let id_cycle = 1i64 << id_len;
    let diff_window = 1i64 << diff_len;
    for reference in references {
        let reference_id = i64::from(reference.frame_id);
        let outside_window = if current > diff_window {
            reference_id > current || reference_id < current - diff_window
        } else {
            reference_id > current && reference_id < id_cycle + current - diff_window
        };
        if outside_window {
            reference.valid = false;
        }
    }
}

fn read_ue(r: &mut BitReader<'_>) -> Result<u32, Av1Error> {
    let mut leading_zeroes = 0u8;
    while !r.read_bit()? {
        leading_zeroes = leading_zeroes
            .checked_add(1)
            .ok_or(Av1Error::InvalidValue("Exp-Golomb"))?;
        if leading_zeroes > 31 {
            return Err(Av1Error::InvalidValue("Exp-Golomb"));
        }
    }
    if leading_zeroes == 0 {
        Ok(0)
    } else {
        let rest = r.read_bits(leading_zeroes)?;
        Ok((1u32 << leading_zeroes) - 1 + rest)
    }
}

fn read_color_config(r: &mut BitReader<'_>, profile: u8) -> Result<(ColorConfig, u8), Av1Error> {
    let mut cfg = ColorConfig::default();
    cfg.high_bitdepth = r.read_bit()?;

    let bit_depth = if profile == 2 && cfg.high_bitdepth {
        cfg.twelve_bit = r.read_bit()?;
        if cfg.twelve_bit {
            12
        } else {
            10
        }
    } else if cfg.high_bitdepth {
        10
    } else {
        8
    };

    cfg.mono_chrome = profile != 1 && r.read_bit()?;
    cfg.color_description_present = r.read_bit()?;
    if cfg.color_description_present {
        cfg.color_primaries = r.read_bits(8)? as u8;
        cfg.transfer_characteristics = r.read_bits(8)? as u8;
        cfg.matrix_coefficients = r.read_bits(8)? as u8;
    } else {
        cfg.color_primaries = 2;
        cfg.transfer_characteristics = 2;
        cfg.matrix_coefficients = 2;
    }

    if cfg.mono_chrome {
        cfg.color_range = r.read_bit()?;
        cfg.subsampling_x = true;
        cfg.subsampling_y = true;
    } else if cfg.color_primaries == 1
        && cfg.transfer_characteristics == 13
        && cfg.matrix_coefficients == 0
    {
        cfg.color_range = true;
        cfg.separate_uv_delta_q = r.read_bit()?;
    } else {
        cfg.color_range = r.read_bit()?;
        match profile {
            0 => {
                cfg.subsampling_x = true;
                cfg.subsampling_y = true;
            }
            1 => {}
            2 if bit_depth == 12 => {
                cfg.subsampling_x = r.read_bit()?;
                if cfg.subsampling_x {
                    cfg.subsampling_y = r.read_bit()?;
                }
            }
            2 => {
                cfg.subsampling_x = true;
            }
            _ => return Err(Av1Error::InvalidValue("seq_profile")),
        }
        if cfg.subsampling_x && cfg.subsampling_y {
            cfg.chroma_sample_position = r.read_bits(2)? as u8;
        }
        cfg.separate_uv_delta_q = r.read_bit()?;
    }

    Ok((cfg, bit_depth))
}

/// Parse a sequence header OBU payload.
pub fn parse_sequence_header(payload: &[u8]) -> Result<SequenceHeader, Av1Error> {
    let mut r = BitReader::new(payload);
    let profile = r.read_bits(3)? as u8;
    let still_picture = r.read_bit()?;
    let reduced_still_picture_header = r.read_bit()?;

    let mut timing_info_present = false;
    let mut timing_info = TimingInfo::default();
    let mut decoder_model_info_present = false;
    let mut decoder_model_info = DecoderModelInfo::default();
    let mut initial_display_delay_present = false;
    let mut operating_points = Vec::new();

    if !reduced_still_picture_header {
        timing_info_present = r.read_bit()?;
        if timing_info_present {
            timing_info = read_timing_info(&mut r)?;
            decoder_model_info_present = r.read_bit()?;
            if decoder_model_info_present {
                decoder_model_info = read_decoder_model_info(&mut r)?;
            }
        }

        initial_display_delay_present = r.read_bit()?;
        let operating_points_count = r.read_bits(5)? as usize + 1;
        operating_points.reserve(operating_points_count);
        for _ in 0..operating_points_count {
            let mut op = OperatingPoint {
                idc: r.read_bits(12)? as u16,
                level_idx: r.read_bits(5)? as u8,
                ..OperatingPoint::default()
            };
            if op.level_idx > 7 {
                op.tier = r.read_bits(1)? as u8;
            }

            if decoder_model_info_present {
                op.decoder_model_present = r.read_bit()?;
                if op.decoder_model_present {
                    let width = decoder_model_info.buffer_delay_length_minus_1 + 1;
                    op.decoder_buffer_delay = r.read_bits_u64(width)? as u32;
                    op.encoder_buffer_delay = r.read_bits_u64(width)? as u32;
                    op.low_delay_mode = r.read_bit()?;
                }
            }

            if initial_display_delay_present {
                op.initial_display_delay_present = r.read_bit()?;
                if op.initial_display_delay_present {
                    op.initial_display_delay_minus_1 = r.read_bits(4)? as u8;
                }
            }
            operating_points.push(op);
        }
    } else {
        operating_points.push(OperatingPoint {
            level_idx: r.read_bits(5)? as u8,
            ..OperatingPoint::default()
        });
    }

    let frame_width_bits = r.read_bits(4)? as u8 + 1;
    let frame_height_bits = r.read_bits(4)? as u8 + 1;
    let max_frame_width = r.read_bits_u64(frame_width_bits)? as u16 + 1;
    let max_frame_height = r.read_bits_u64(frame_height_bits)? as u16 + 1;

    let frame_id_numbers_present = !reduced_still_picture_header && r.read_bit()?;
    let (delta_frame_id_length_minus_2, additional_frame_id_length_minus_1) =
        if frame_id_numbers_present {
            (r.read_bits(4)? as u8, r.read_bits(3)? as u8)
        } else {
            (0, 0)
        };

    let use_128x128_superblock = r.read_bit()?;
    let enable_filter_intra = r.read_bit()?;
    let enable_intra_edge_filter = r.read_bit()?;

    let (
        enable_interintra_compound,
        enable_masked_compound,
        enable_warped_motion,
        enable_dual_filter,
        enable_order_hint,
        enable_jnt_comp,
        enable_ref_frame_mvs,
        seq_force_screen_content_tools,
        seq_force_integer_mv,
        order_hint_bits,
    ) = if reduced_still_picture_header {
        (false, false, false, false, false, false, false, 2, 2, 0)
    } else {
        let interintra = r.read_bit()?;
        let masked = r.read_bit()?;
        let warped = r.read_bit()?;
        let dual = r.read_bit()?;
        let order_hint = r.read_bit()?;
        let (jnt, ref_mvs) = if order_hint {
            (r.read_bit()?, r.read_bit()?)
        } else {
            (false, false)
        };
        let choose_screen = r.read_bit()?;
        let force_screen = if choose_screen {
            2
        } else {
            r.read_bits(1)? as u8
        };
        let force_integer = if force_screen > 0 {
            if r.read_bit()? {
                2
            } else {
                r.read_bits(1)? as u8
            }
        } else {
            2
        };
        let hint_bits = if order_hint {
            r.read_bits(3)? as u8 + 1
        } else {
            0
        };
        (
            interintra,
            masked,
            warped,
            dual,
            order_hint,
            jnt,
            ref_mvs,
            force_screen,
            force_integer,
            hint_bits,
        )
    };

    let enable_superres = r.read_bit()?;
    let enable_cdef = r.read_bit()?;
    let enable_restoration = r.read_bit()?;
    let (color_config, bit_depth) = read_color_config(&mut r, profile)?;
    let film_grain_params_present = r.read_bit()?;

    Ok(SequenceHeader {
        profile,
        still_picture,
        reduced_still_picture_header,
        timing_info_present,
        timing_info,
        decoder_model_info_present,
        decoder_model_info,
        initial_display_delay_present,
        operating_points,
        frame_width_bits,
        frame_height_bits,
        max_frame_width,
        max_frame_height,
        frame_id_numbers_present,
        delta_frame_id_length_minus_2,
        additional_frame_id_length_minus_1,
        use_128x128_superblock,
        enable_filter_intra,
        enable_intra_edge_filter,
        enable_interintra_compound,
        enable_masked_compound,
        enable_warped_motion,
        enable_dual_filter,
        enable_order_hint,
        enable_jnt_comp,
        enable_ref_frame_mvs,
        seq_force_screen_content_tools,
        seq_force_integer_mv,
        order_hint_bits,
        enable_superres,
        enable_cdef,
        enable_restoration,
        color_config,
        bit_depth,
        film_grain_params_present,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum FrameType {
    #[default]
    Key = 0,
    Inter = 1,
    IntraOnly = 2,
    Switch = 3,
}

impl FrameType {
    fn from_bits(value: u32) -> Result<Self, Av1Error> {
        match value {
            0 => Ok(Self::Key),
            1 => Ok(Self::Inter),
            2 => Ok(Self::IntraOnly),
            3 => Ok(Self::Switch),
            _ => Err(Av1Error::InvalidValue("frame_type")),
        }
    }

    fn is_intra(self) -> bool {
        matches!(self, Self::Key | Self::IntraOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum TxMode {
    #[default]
    Only4x4 = 0,
    Largest = 1,
    Select = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceFrame {
    pub valid: bool,
    pub frame_id: u32,
    pub upscaled_width: u32,
    pub frame_width: u32,
    pub frame_height: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub frame_type: FrameType,
    pub subsampling_x: bool,
    pub subsampling_y: bool,
    pub bit_depth: u8,
    pub order_hint: u8,
    pub loop_filter_ref_deltas: [i8; TOTAL_REFS_PER_FRAME],
    pub loop_filter_mode_deltas: [i8; 2],
    pub feature_enabled: [[bool; SEG_LVL_FEATURES]; MAX_SEGMENTS],
    pub feature_value: [[i16; SEG_LVL_FEATURES]; MAX_SEGMENTS],
    pub warped_motion: [WarpedMotion; NUM_REF_FRAMES],
    pub lst_frame_offset: u32,
    pub lst2_frame_offset: u32,
    pub lst3_frame_offset: u32,
    pub gld_frame_offset: u32,
    pub bwd_frame_offset: u32,
    pub alt2_frame_offset: u32,
    pub alt_frame_offset: u32,
    pub is_intra_frame: bool,
    pub intra_only: bool,
    pub film_grain: FilmGrainParams,
}

impl Default for ReferenceFrame {
    fn default() -> Self {
        Self {
            valid: false,
            frame_id: 0,
            upscaled_width: 0,
            frame_width: 0,
            frame_height: 0,
            render_width: 0,
            render_height: 0,
            frame_type: FrameType::Key,
            subsampling_x: false,
            subsampling_y: false,
            bit_depth: 0,
            order_hint: 0,
            loop_filter_ref_deltas: [0; TOTAL_REFS_PER_FRAME],
            loop_filter_mode_deltas: [0; 2],
            feature_enabled: [[false; SEG_LVL_FEATURES]; MAX_SEGMENTS],
            feature_value: [[0; SEG_LVL_FEATURES]; MAX_SEGMENTS],
            warped_motion: [WarpedMotion::identity(); NUM_REF_FRAMES],
            lst_frame_offset: 0,
            lst2_frame_offset: 0,
            lst3_frame_offset: 0,
            gld_frame_offset: 0,
            bwd_frame_offset: 0,
            alt2_frame_offset: 0,
            alt_frame_offset: 0,
            is_intra_frame: false,
            intra_only: false,
            film_grain: FilmGrainParams::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameDecodeState {
    pub references: [ReferenceFrame; NUM_REF_FRAMES],
    pub cdfs: Av1CdfStore,
    pub active_cdf: Av1Cdf,
    pub references_at_header: [ReferenceFrame; NUM_REF_FRAMES],
    pub order_hint: u8,
}

impl FrameDecodeState {
    pub fn new() -> Self {
        Self {
            references: [ReferenceFrame::default(); NUM_REF_FRAMES],
            cdfs: Av1CdfStore::new(),
            active_cdf: Av1Cdf::new(Av1Cdfs::default(), Av1MvCdfs::default()),
            references_at_header: [ReferenceFrame::default(); NUM_REF_FRAMES],
            order_hint: 0,
        }
    }
}

impl Default for FrameDecodeState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalMotion {
    pub identity: bool,
    pub rotation_zoom: bool,
    pub translation: bool,
    pub parameters: [i32; 6],
}

impl Default for GlobalMotion {
    fn default() -> Self {
        Self {
            identity: true,
            rotation_zoom: false,
            translation: false,
            parameters: [0; 6],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FilmGrainParams {
    pub apply_grain: bool,
    pub grain_seed: u16,
    pub update_grain: bool,
    pub film_grain_params_ref_idx: u8,
    pub num_y_points: u8,
    pub point_y_value: [u8; 14],
    pub point_y_scaling: [u8; 14],
    pub chroma_scaling_from_luma: bool,
    pub num_cb_points: u8,
    pub point_cb_value: [u8; 10],
    pub point_cb_scaling: [u8; 10],
    pub num_cr_points: u8,
    pub point_cr_value: [u8; 10],
    pub point_cr_scaling: [u8; 10],
    pub grain_scaling_minus_8: u8,
    pub ar_coeff_lag: u8,
    pub ar_coeffs_y_plus_128: [u8; 24],
    pub ar_coeffs_cb_plus_128: [u8; 25],
    pub ar_coeffs_cr_plus_128: [u8; 25],
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrameHeader {
    pub show_existing_frame: bool,
    pub frame_to_show_map_idx: u8,
    pub display_frame_id: u32,
    pub frame_presentation_time: u32,
    pub frame_type: FrameType,
    pub show_frame: bool,
    pub showable_frame: bool,
    pub error_resilient_mode: bool,
    pub disable_cdf_update: bool,
    pub allow_screen_content_tools: bool,
    pub force_integer_mv: bool,
    pub current_frame_id: u32,
    pub frame_size_override_flag: bool,
    pub order_hint: u8,
    pub buffer_removal_time_present_flag: bool,
    pub buffer_removal_time: [u32; MAX_OPERATING_POINTS],
    pub primary_ref_frame: u8,
    pub refresh_frame_flags: u8,
    pub ref_order_hint: [u8; NUM_REF_FRAMES],
    pub frame_refs_short_signaling: bool,
    pub last_frame_idx: u8,
    pub golden_frame_idx: u8,
    pub ref_frame_idx: [u8; REFS_PER_FRAME],
    pub delta_frame_id_minus1: [u32; REFS_PER_FRAME],
    pub width: u32,
    pub height: u32,
    pub upscaled_width: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub use_superres: bool,
    pub coded_denom: u8,
    pub allow_intrabc: bool,
    pub allow_high_precision_mv: bool,
    pub interpolation_filter: u8,
    pub is_motion_mode_switchable: bool,
    pub use_ref_frame_mvs: bool,
    pub disable_frame_end_update_cdf: bool,
    pub uniform_tile_spacing_flag: bool,
    pub tile_cols_log2: u8,
    pub tile_rows_log2: u8,
    pub tile_cols: u32,
    pub tile_rows: u32,
    pub width_in_sbs_minus_1: Vec<u8>,
    pub height_in_sbs_minus_1: Vec<u8>,
    pub context_update_tile_id: u16,
    pub tile_size_bytes_minus_1: u8,
    pub base_q_idx: u8,
    pub delta_q_y_dc: i8,
    pub diff_uv_delta: bool,
    pub delta_q_u_dc: i8,
    pub delta_q_u_ac: i8,
    pub delta_q_v_dc: i8,
    pub delta_q_v_ac: i8,
    pub using_qmatrix: bool,
    pub qm_y: u8,
    pub qm_u: u8,
    pub qm_v: u8,
    pub segmentation_enabled: bool,
    pub segmentation_update_map: bool,
    pub segmentation_temporal_update: bool,
    pub segmentation_update_data: bool,
    pub feature_enabled: [[bool; SEG_LVL_FEATURES]; MAX_SEGMENTS],
    pub feature_value: [[i16; SEG_LVL_FEATURES]; MAX_SEGMENTS],
    pub segmentation_id_last_active: u8,
    pub segmentation_id_preskip: bool,
    pub delta_q_present: bool,
    pub delta_q_res: u8,
    pub delta_lf_present: bool,
    pub delta_lf_res: u8,
    pub delta_lf_multi: bool,
    pub loop_filter_level: [u8; 4],
    pub loop_filter_sharpness: u8,
    pub loop_filter_delta_enabled: bool,
    pub loop_filter_delta_update: bool,
    pub loop_filter_ref_deltas: [i8; TOTAL_REFS_PER_FRAME],
    pub loop_filter_mode_deltas: [i8; 2],
    pub cdef_damping_minus_3: u8,
    pub cdef_bits: u8,
    pub cdef_y_pri_strength: [u8; 8],
    pub cdef_y_sec_strength: [u8; 8],
    pub cdef_uv_pri_strength: [u8; 8],
    pub cdef_uv_sec_strength: [u8; 8],
    pub lr_type: [u8; 3],
    pub lr_unit_shift: u8,
    pub lr_uv_shift: bool,
    pub tx_mode: TxMode,
    pub reference_select: bool,
    pub skip_mode_present: bool,
    /// AV1 reference number (LAST..ALTREF) for each skip-mode reference.
    /// Zero means no reference was selected, matching MPP's `skip_ref0/1`.
    pub skip_mode_frame_idx: [u8; 2],
    pub allow_warped_motion: bool,
    pub reduced_tx_set: bool,
    pub global_motion: [GlobalMotion; TOTAL_REFS_PER_FRAME],
    pub film_grain: FilmGrainParams,
    pub coded_lossless: bool,
    pub all_lossless: bool,
    pub header_bits: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TileGroup {
    pub tile_start_and_end_present_flag: bool,
    pub tg_start: u32,
    pub tg_end: u32,
}

/// Accumulates the disjoint tile ranges carried by a frame's tile groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileGroupAccumulator {
    ranges: [Option<std::ops::Range<usize>>; MAX_TILES],
}

impl TileGroupAccumulator {
    pub fn new() -> Self {
        Self {
            ranges: std::array::from_fn(|_| None),
        }
    }

    pub fn insert_group(
        &mut self,
        payload: &[u8],
        header: &FrameHeader,
        group: TileGroup,
        payload_offset: usize,
    ) -> Result<(), Av1Error> {
        let ranges = parse_tile_group_layout(payload, header, group, payload_offset)?;
        for (tile, range) in ranges.into_iter().enumerate() {
            if let Some(range) = range {
                if self.ranges[tile].replace(range).is_some() {
                    return Err(Av1Error::InvalidValue("duplicate tile in tile groups"));
                }
            }
        }
        Ok(())
    }

    pub fn finish(
        self,
        header: &FrameHeader,
    ) -> Result<[Option<std::ops::Range<usize>>; MAX_TILES], Av1Error> {
        let num_tiles = header
            .tile_cols
            .checked_mul(header.tile_rows)
            .filter(|&count| count > 0 && count as usize <= MAX_TILES)
            .ok_or(Av1Error::InvalidValue("tile count"))? as usize;
        if self.ranges[..num_tiles].iter().any(Option::is_none) {
            return Err(Av1Error::InvalidValue("incomplete tile groups"));
        }
        Ok(self.ranges)
    }
}

impl Default for TileGroupAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFrameObu<'a> {
    pub header: FrameHeader,
    pub tile_group: TileGroup,
    /// Offset of the first tile byte from the start of the complete OBU.
    pub tag_size: usize,
    pub tile_data: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTileGroupObu<'a> {
    pub tile_group: TileGroup,
    /// Offset of the first tile byte from the start of the complete OBU.
    pub tag_size: usize,
    pub tile_data: &'a [u8],
}

fn read_tile_group(r: &mut BitReader<'_>, header: &FrameHeader) -> Result<TileGroup, Av1Error> {
    let num_tiles = header
        .tile_cols
        .checked_mul(header.tile_rows)
        .filter(|&count| count > 0)
        .ok_or(Av1Error::InvalidValue("tile count"))?;
    let tile_start_and_end_present_flag = num_tiles > 1 && r.read_bit()?;
    let (tg_start, tg_end) = if !tile_start_and_end_present_flag {
        (0, num_tiles - 1)
    } else {
        let tile_bits = tile_log2(1, header.tile_cols) + tile_log2(1, header.tile_rows);
        let start = r.read_bits(tile_bits)?;
        let end = r.read_bits(tile_bits)?;
        if start > end || end >= num_tiles {
            return Err(Av1Error::InvalidValue("tile group range"));
        }
        (start, end)
    };
    r.align()?;
    Ok(TileGroup {
        tile_start_and_end_present_flag,
        tg_start,
        tg_end,
    })
}

fn tile_log2(block_size: u32, target: u32) -> u8 {
    let mut value = 0u8;
    while block_size.saturating_mul(1u32 << value) < target {
        value += 1;
    }
    value
}

fn read_increment(r: &mut BitReader<'_>, range_min: u32, range_max: u32) -> Result<u32, Av1Error> {
    let mut value = range_min;
    while value < range_max && r.read_bit()? {
        value += 1;
    }
    Ok(value)
}

fn read_ns(r: &mut BitReader<'_>, n: u32) -> Result<u32, Av1Error> {
    if n == 0 {
        return Err(Av1Error::InvalidValue("ns"));
    }
    let width = 32 - n.leading_zeros();
    let m = (1u32 << width) - n;
    let value = if width > 1 {
        r.read_bits((width - 1) as u8)?
    } else {
        0
    };
    if value < m {
        Ok(value)
    } else {
        Ok((value << 1) - m + u32::from(r.read_bit()?))
    }
}

fn read_su(r: &mut BitReader<'_>, width: u8) -> Result<i32, Av1Error> {
    let raw = r.read_bits(width)?;
    let shift = 32 - width;
    Ok(((raw << shift) as i32) >> shift)
}

fn read_delta_q(r: &mut BitReader<'_>) -> Result<i8, Av1Error> {
    if r.read_bit()? {
        Ok(r.read_bits(7)? as u8 as i8)
    } else {
        Ok(0)
    }
}

fn read_superres(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
) -> Result<(), Av1Error> {
    header.upscaled_width = header.width;
    if seq.enable_superres {
        header.use_superres = r.read_bit()?;
    }
    let denom = if header.use_superres {
        header.coded_denom = r.read_bits(3)? as u8;
        9 + header.coded_denom
    } else {
        8
    };
    header.width = (header.upscaled_width * 8 + u32::from(denom / 2)) / u32::from(denom);
    Ok(())
}

fn read_frame_size(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
) -> Result<(), Av1Error> {
    if header.frame_size_override_flag {
        header.width = r.read_bits(seq.frame_width_bits)? as u32 + 1;
        header.height = r.read_bits(seq.frame_height_bits)? as u32 + 1;
    } else {
        header.width = u32::from(seq.max_frame_width);
        header.height = u32::from(seq.max_frame_height);
    }
    read_superres(r, seq, header)
}

fn read_render_size(r: &mut BitReader<'_>, header: &mut FrameHeader) -> Result<(), Av1Error> {
    if r.read_bit()? {
        header.render_width = r.read_bits(16)? as u32 + 1;
        header.render_height = r.read_bits(16)? as u32 + 1;
    } else {
        header.render_width = header.width;
        header.render_height = header.height;
    }
    Ok(())
}

pub fn relative_distance(enable_order_hint: bool, order_hint_bits: u8, a: u8, b: u8) -> i32 {
    if !enable_order_hint {
        return 0;
    }
    let half = 1i32 << order_hint_bits.saturating_sub(1);
    let diff = i32::from(a).wrapping_sub(i32::from(b)) & (half * 2 - 1);
    (diff & (half - 1)) - (diff & half)
}

fn relative_dist(seq: &SequenceHeader, a: u8, b: u8) -> i32 {
    relative_distance(seq.enable_order_hint, seq.order_hint_bits, a, b)
}

fn set_frame_refs(
    seq: &SequenceHeader,
    header: &mut FrameHeader,
    references: &[ReferenceFrame; NUM_REF_FRAMES],
) -> Result<(), Av1Error> {
    let current_hint = 1i32 << seq.order_hint_bits;
    let mut used = [false; NUM_REF_FRAMES];
    let mut indices: [Option<usize>; REFS_PER_FRAME] = [None; REFS_PER_FRAME];
    indices[0] = Some(header.last_frame_idx as usize);
    indices[3] = Some(header.golden_frame_idx as usize);
    used[header.last_frame_idx as usize] = true;
    used[header.golden_frame_idx as usize] = true;

    let mut hints = [0i32; NUM_REF_FRAMES];
    for (idx, reference) in references.iter().enumerate() {
        hints[idx] = current_hint + relative_dist(seq, reference.order_hint, header.order_hint);
    }
    let find_backward = |used: &mut [bool; NUM_REF_FRAMES], latest: bool| {
        let mut selected = None;
        for idx in 0..NUM_REF_FRAMES {
            if !used[idx]
                && hints[idx] >= current_hint
                && selected.is_none_or(|current| {
                    if latest {
                        hints[idx] >= hints[current]
                    } else {
                        hints[idx] < hints[current]
                    }
                })
            {
                selected = Some(idx);
            }
        }
        selected
    };

    indices[6] = find_backward(&mut used, true).inspect(|&idx| used[idx] = true);
    indices[4] = find_backward(&mut used, false).inspect(|&idx| used[idx] = true);
    indices[5] = find_backward(&mut used, false).inspect(|&idx| used[idx] = true);

    for slot in [1usize, 2, 6, 5] {
        if indices[slot - 1].is_some() {
            continue;
        }
        let mut selected = None;
        for idx in 0..NUM_REF_FRAMES {
            if !used[idx]
                && hints[idx] < current_hint
                && selected.is_none_or(|current| hints[idx] >= hints[current])
            {
                selected = Some(idx);
            }
        }
        if let Some(idx) = selected {
            indices[slot - 1] = Some(idx);
            used[idx] = true;
        }
    }

    let fallback = (0..NUM_REF_FRAMES)
        .min_by_key(|&idx| hints[idx])
        .unwrap_or(0) as u8;
    for (idx, slot) in indices.iter_mut().enumerate() {
        let selected = slot.unwrap_or(fallback as usize);
        if selected >= NUM_REF_FRAMES {
            return Err(Av1Error::InvalidValue("reference index"));
        }
        header.ref_frame_idx[idx] = selected as u8;
    }
    Ok(())
}

fn read_frame_size_with_refs(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
    references: &[ReferenceFrame; NUM_REF_FRAMES],
) -> Result<(), Av1Error> {
    let mut found = false;
    for idx in 0..REFS_PER_FRAME {
        if r.read_bit()? {
            let reference = &references[header.ref_frame_idx[idx] as usize];
            if !reference.valid {
                return Err(Av1Error::InvalidValue("missing reference frame"));
            }
            header.upscaled_width = reference.upscaled_width;
            header.width = reference.frame_width;
            header.height = reference.frame_height;
            header.render_width = reference.render_width;
            header.render_height = reference.render_height;
            found = true;
            break;
        }
    }
    if found {
        read_superres(r, seq, header)
    } else {
        read_frame_size(r, seq, header)?;
        read_render_size(r, header)
    }
}

fn read_tile_info(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
) -> Result<(), Av1Error> {
    let sb_size = if seq.use_128x128_superblock { 7u8 } else { 6u8 };
    let sb_cols = (header.width + (1 << sb_size) - 1) >> sb_size;
    let sb_rows = (header.height + (1 << sb_size) - 1) >> sb_size;
    let max_tile_width_sb = MAX_TILE_WIDTH >> sb_size;
    let max_tile_area_sb = MAX_TILE_AREA >> (2 * sb_size);
    let min_log2_tile_cols = tile_log2(max_tile_width_sb, sb_cols);
    let max_log2_tile_cols = tile_log2(1, sb_cols.min(MAX_TILE_COLS as u32));
    let max_log2_tile_rows = tile_log2(1, sb_rows.min(MAX_TILE_ROWS as u32));
    let min_log2_tiles = min_log2_tile_cols.max(tile_log2(max_tile_area_sb, sb_rows * sb_cols));

    header.uniform_tile_spacing_flag = r.read_bit()?;
    if header.uniform_tile_spacing_flag {
        header.tile_cols_log2 = read_increment(
            r,
            u32::from(min_log2_tile_cols),
            u32::from(max_log2_tile_cols),
        )? as u8;
        let tile_width_sb = sb_cols.div_ceil(1 << header.tile_cols_log2);
        header.tile_cols = sb_cols.div_ceil(tile_width_sb);

        let min_log2_tile_rows = min_log2_tiles.saturating_sub(header.tile_cols_log2) as u32;
        header.tile_rows_log2 =
            read_increment(r, min_log2_tile_rows, u32::from(max_log2_tile_rows))? as u8;
        let tile_height_sb = sb_rows.div_ceil(1 << header.tile_rows_log2);
        header.tile_rows = sb_rows.div_ceil(tile_height_sb);

        header.width_in_sbs_minus_1 = vec![(tile_width_sb - 1) as u8; header.tile_cols as usize];
        let last_width = sb_cols - (header.tile_cols - 1) * tile_width_sb;
        *header.width_in_sbs_minus_1.last_mut().unwrap() = (last_width - 1) as u8;
        header.height_in_sbs_minus_1 = vec![(tile_height_sb - 1) as u8; header.tile_rows as usize];
        let last_height = sb_rows - (header.tile_rows - 1) * tile_height_sb;
        *header.height_in_sbs_minus_1.last_mut().unwrap() = (last_height - 1) as u8;
    } else {
        let mut widest_tile_sb = 0u32;
        let mut start_sb = 0u32;
        while start_sb < sb_cols && header.width_in_sbs_minus_1.len() < MAX_TILE_COLS {
            let max_width = (sb_cols - start_sb).min(max_tile_width_sb);
            let value = read_ns(r, max_width)?;
            header.width_in_sbs_minus_1.push(value as u8);
            let size_sb = value + 1;
            widest_tile_sb = widest_tile_sb.max(size_sb);
            start_sb += size_sb;
        }
        header.tile_cols_log2 = tile_log2(1, header.width_in_sbs_minus_1.len() as u32);
        header.tile_cols = header.width_in_sbs_minus_1.len() as u32;

        let max_tile_area = if min_log2_tiles > 0 {
            (sb_rows * sb_cols) >> (min_log2_tiles + 1)
        } else {
            sb_rows * sb_cols
        };
        let max_tile_height_sb = (max_tile_area / widest_tile_sb.max(1)).max(1);
        start_sb = 0;
        while start_sb < sb_rows && header.height_in_sbs_minus_1.len() < MAX_TILE_ROWS {
            let max_height = (sb_rows - start_sb).min(max_tile_height_sb);
            let value = read_ns(r, max_height)?;
            header.height_in_sbs_minus_1.push(value as u8);
            start_sb += value + 1;
        }
        header.tile_rows_log2 = tile_log2(1, header.height_in_sbs_minus_1.len() as u32);
        header.tile_rows = header.height_in_sbs_minus_1.len() as u32;
    }

    if header.tile_cols_log2 > 0 || header.tile_rows_log2 > 0 {
        header.context_update_tile_id =
            r.read_bits(header.tile_cols_log2 + header.tile_rows_log2)? as u16;
        header.tile_size_bytes_minus_1 = r.read_bits(2)? as u8;
    } else {
        header.context_update_tile_id = 0;
        header.tile_size_bytes_minus_1 = 3;
    }
    Ok(())
}

fn read_quantization(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
) -> Result<(), Av1Error> {
    header.base_q_idx = r.read_bits(8)? as u8;
    header.delta_q_y_dc = read_delta_q(r)?;
    if !seq.color_config.mono_chrome {
        if seq.color_config.separate_uv_delta_q {
            header.diff_uv_delta = r.read_bit()?;
        }
        header.delta_q_u_dc = read_delta_q(r)?;
        header.delta_q_u_ac = read_delta_q(r)?;
        if header.diff_uv_delta {
            header.delta_q_v_dc = read_delta_q(r)?;
            header.delta_q_v_ac = read_delta_q(r)?;
        } else {
            header.delta_q_v_dc = header.delta_q_u_dc;
            header.delta_q_v_ac = header.delta_q_u_ac;
        }
    }

    header.using_qmatrix = r.read_bit()?;
    if header.using_qmatrix {
        header.qm_y = r.read_bits(4)? as u8;
        header.qm_u = r.read_bits(4)? as u8;
        header.qm_v = if seq.color_config.separate_uv_delta_q {
            r.read_bits(4)? as u8
        } else {
            header.qm_u
        };
    }
    Ok(())
}

fn read_segmentation(
    r: &mut BitReader<'_>,
    header: &mut FrameHeader,
    references: &[ReferenceFrame; NUM_REF_FRAMES],
) -> Result<(), Av1Error> {
    const BITS: [u8; SEG_LVL_FEATURES] = [8, 6, 6, 6, 6, 3, 0, 0];
    const SIGNED: [bool; SEG_LVL_FEATURES] = [true, true, true, true, true, false, false, false];

    header.segmentation_enabled = r.read_bit()?;
    if header.segmentation_enabled {
        if header.primary_ref_frame == PRIMARY_REF_NONE {
            header.segmentation_update_map = true;
            header.segmentation_update_data = true;
        } else {
            header.segmentation_update_map = r.read_bit()?;
            if header.segmentation_update_map {
                header.segmentation_temporal_update = r.read_bit()?;
            }
            header.segmentation_update_data = r.read_bit()?;
        }

        let reference = if header.primary_ref_frame != PRIMARY_REF_NONE {
            Some(&references[header.ref_frame_idx[header.primary_ref_frame as usize] as usize])
        } else {
            None
        };
        for segment in 0..MAX_SEGMENTS {
            for feature in 0..SEG_LVL_FEATURES {
                if header.segmentation_update_data {
                    header.feature_enabled[segment][feature] = r.read_bit()?;
                    if header.feature_enabled[segment][feature] && BITS[feature] != 0 {
                        let value = if SIGNED[feature] {
                            let sign = r.read_bit()?;
                            let value = r.read_bits(BITS[feature])? as i32;
                            if sign {
                                value - (1 << BITS[feature])
                            } else {
                                value
                            }
                        } else {
                            r.read_bits(BITS[feature])? as i32
                        };
                        header.feature_value[segment][feature] = value as i16;
                    }
                } else if let Some(reference) = reference {
                    header.feature_enabled[segment][feature] =
                        reference.feature_enabled[segment][feature];
                    header.feature_value[segment][feature] =
                        reference.feature_value[segment][feature];
                }
            }
        }
    }

    for segment in 0..MAX_SEGMENTS {
        for feature in 0..SEG_LVL_FEATURES {
            if header.feature_enabled[segment][feature] {
                header.segmentation_id_last_active = segment as u8;
                if feature > 5 {
                    header.segmentation_id_preskip = true;
                }
            }
        }
    }
    Ok(())
}

fn read_delta_q_lf(r: &mut BitReader<'_>, header: &mut FrameHeader) -> Result<(), Av1Error> {
    if header.base_q_idx > 0 {
        header.delta_q_present = r.read_bit()?;
    }
    if header.delta_q_present {
        header.delta_q_res = r.read_bits(2)? as u8;
        if !header.allow_intrabc {
            header.delta_lf_present = r.read_bit()?;
        }
        if header.delta_lf_present {
            header.delta_lf_res = r.read_bits(2)? as u8;
            header.delta_lf_multi = r.read_bit()?;
        }
    }
    Ok(())
}

fn read_loop_filter(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
    references: &[ReferenceFrame; NUM_REF_FRAMES],
) -> Result<(), Av1Error> {
    const DEFAULT_REF_DELTAS: [i8; TOTAL_REFS_PER_FRAME] = [1, 0, 0, 0, -1, 0, -1, -1];
    if header.coded_lossless || header.allow_intrabc {
        header.loop_filter_ref_deltas = DEFAULT_REF_DELTAS;
        return Ok(());
    }

    header.loop_filter_level[0] = r.read_bits(6)? as u8;
    header.loop_filter_level[1] = r.read_bits(6)? as u8;
    if !seq.color_config.mono_chrome
        && (header.loop_filter_level[0] != 0 || header.loop_filter_level[1] != 0)
    {
        header.loop_filter_level[2] = r.read_bits(6)? as u8;
        header.loop_filter_level[3] = r.read_bits(6)? as u8;
    }
    header.loop_filter_sharpness = r.read_bits(3)? as u8;
    header.loop_filter_delta_enabled = r.read_bit()?;
    if header.loop_filter_delta_enabled {
        let reference = if header.primary_ref_frame == PRIMARY_REF_NONE {
            None
        } else {
            Some(&references[header.ref_frame_idx[header.primary_ref_frame as usize] as usize])
        };
        let default_ref = reference
            .map(|reference| reference.loop_filter_ref_deltas)
            .unwrap_or(DEFAULT_REF_DELTAS);
        let default_mode = reference
            .map(|reference| reference.loop_filter_mode_deltas)
            .unwrap_or([0, 0]);

        header.loop_filter_delta_update = r.read_bit()?;
        for idx in 0..TOTAL_REFS_PER_FRAME {
            if header.loop_filter_delta_update && r.read_bit()? {
                header.loop_filter_ref_deltas[idx] = read_su(r, 7)? as i8;
            } else {
                header.loop_filter_ref_deltas[idx] = default_ref[idx];
            }
        }
        for idx in 0..2 {
            if header.loop_filter_delta_update && r.read_bit()? {
                header.loop_filter_mode_deltas[idx] = read_su(r, 7)? as i8;
            } else {
                header.loop_filter_mode_deltas[idx] = default_mode[idx];
            }
        }
    } else {
        header.loop_filter_ref_deltas = DEFAULT_REF_DELTAS;
    }
    Ok(())
}

fn read_cdef(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
) -> Result<(), Av1Error> {
    if header.coded_lossless || header.allow_intrabc || !seq.enable_cdef {
        return Ok(());
    }
    header.cdef_damping_minus_3 = r.read_bits(2)? as u8;
    header.cdef_bits = r.read_bits(2)? as u8;
    for idx in 0..1 << header.cdef_bits {
        header.cdef_y_pri_strength[idx] = r.read_bits(4)? as u8;
        header.cdef_y_sec_strength[idx] = r.read_bits(2)? as u8;
        if !seq.color_config.mono_chrome {
            header.cdef_uv_pri_strength[idx] = r.read_bits(4)? as u8;
            header.cdef_uv_sec_strength[idx] = r.read_bits(2)? as u8;
        }
    }
    Ok(())
}

fn read_restoration(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
) -> Result<(), Av1Error> {
    if header.all_lossless || header.allow_intrabc || !seq.enable_restoration {
        return Ok(());
    }
    let planes = if seq.color_config.mono_chrome { 1 } else { 3 };
    let mut uses_lr = false;
    let mut uses_chroma_lr = false;
    for plane in 0..planes {
        header.lr_type[plane] = r.read_bits(2)? as u8;
        if header.lr_type[plane] != 0 {
            uses_lr = true;
            uses_chroma_lr |= plane > 0;
        }
    }
    if uses_lr {
        let min = u32::from(seq.use_128x128_superblock);
        header.lr_unit_shift = read_increment(r, min, 2)? as u8;
        if seq.color_config.subsampling_x && seq.color_config.subsampling_y && uses_chroma_lr {
            header.lr_uv_shift = r.read_bit()?;
        }
    }
    Ok(())
}

fn read_skip_mode(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
    references: &[ReferenceFrame; NUM_REF_FRAMES],
) -> Result<(), Av1Error> {
    if header.frame_type.is_intra() || !header.reference_select {
        header.skip_mode_frame_idx = [0; 2];
        return Ok(());
    }
    header.skip_mode_frame_idx =
        skip_mode_references(seq, header.order_hint, &header.ref_frame_idx, references);
    if header.skip_mode_frame_idx != [0; 2] {
        header.skip_mode_present = r.read_bit()?;
    }
    Ok(())
}

fn skip_mode_references(
    seq: &SequenceHeader,
    current_hint: u8,
    ref_frame_idx: &[u8; REFS_PER_FRAME],
    references: &[ReferenceFrame; NUM_REF_FRAMES],
) -> [u8; 2] {
    if !seq.enable_order_hint {
        return [0; 2];
    }

    let mut forward_idx = None;
    let mut forward_hint = 0;
    let mut backward_idx = None;
    let mut backward_hint = 0;

    for idx in 0..REFS_PER_FRAME {
        let hint = references[ref_frame_idx[idx] as usize].order_hint;
        let distance = relative_dist(seq, hint, current_hint);
        if distance < 0 && (forward_idx.is_none() || relative_dist(seq, hint, forward_hint) > 0) {
            forward_idx = Some(idx);
            forward_hint = hint;
        } else if distance > 0
            && (backward_idx.is_none() || relative_dist(seq, hint, backward_hint) < 0)
        {
            backward_idx = Some(idx);
            backward_hint = hint;
        }
    }

    let Some(forward_idx) = forward_idx else {
        return [0; 2];
    };
    if let Some(backward_idx) = backward_idx {
        return [
            forward_idx.min(backward_idx) as u8 + 1,
            forward_idx.max(backward_idx) as u8 + 1,
        ];
    }

    let mut second_forward_idx = None;
    let mut second_forward_hint = 0;
    for idx in 0..REFS_PER_FRAME {
        let hint = references[ref_frame_idx[idx] as usize].order_hint;
        if relative_dist(seq, hint, forward_hint) < 0
            && (second_forward_idx.is_none() || relative_dist(seq, hint, second_forward_hint) > 0)
        {
            second_forward_idx = Some(idx);
            second_forward_hint = hint;
        }
    }

    second_forward_idx.map_or([0; 2], |second_forward_idx| {
        [
            forward_idx.min(second_forward_idx) as u8 + 1,
            forward_idx.max(second_forward_idx) as u8 + 1,
        ]
    })
}

fn read_signed_subexp(r: &mut BitReader<'_>, range_max: u32) -> Result<u32, Av1Error> {
    let max_len = 32 - (range_max - 1).leading_zeros() - 4;
    let length = read_increment(r, 0, max_len)?;
    let (bits, offset) = if length > 0 {
        let bits = 2 + length;
        (bits, 1 << bits)
    } else {
        (3, 0)
    };
    let value = if length < max_len {
        r.read_bits(bits as u8)?
    } else {
        read_ns(r, range_max - offset)?
    };
    Ok(value + offset)
}

fn read_global_motion(r: &mut BitReader<'_>, header: &mut FrameHeader) -> Result<(), Av1Error> {
    if header.frame_type.is_intra() {
        return Ok(());
    }
    for reference in 1..TOTAL_REFS_PER_FRAME {
        let global = &mut header.global_motion[reference];
        if !r.read_bit()? {
            global.identity = true;
            continue;
        }
        global.rotation_zoom = r.read_bit()?;
        let model = if global.rotation_zoom {
            2
        } else if r.read_bit()? {
            global.translation = true;
            1
        } else {
            3
        };
        if model >= 2 {
            read_gm_param(r, header, reference, model, 2)?;
            read_gm_param(r, header, reference, model, 3)?;
            if model == 3 {
                read_gm_param(r, header, reference, model, 4)?;
                read_gm_param(r, header, reference, model, 5)?;
            } else {
                header.global_motion[reference].parameters[4] =
                    -header.global_motion[reference].parameters[3];
                header.global_motion[reference].parameters[5] =
                    header.global_motion[reference].parameters[2];
            }
        }
        if model >= 1 {
            read_gm_param(r, header, reference, model, 0)?;
            read_gm_param(r, header, reference, model, 1)?;
        }
    }
    Ok(())
}

fn read_gm_param(
    r: &mut BitReader<'_>,
    header: &mut FrameHeader,
    reference: usize,
    model: u8,
    index: usize,
) -> Result<(), Av1Error> {
    let abs_bits = if index < 2 {
        if model == 1 {
            9 - u32::from(!header.allow_high_precision_mv)
        } else {
            12
        }
    } else {
        12
    };
    let symbols = 2 * (1 << abs_bits) + 1;
    header.global_motion[reference].parameters[index] = read_signed_subexp(r, symbols)? as i32;
    Ok(())
}

fn read_film_grain(
    r: &mut BitReader<'_>,
    seq: &SequenceHeader,
    header: &mut FrameHeader,
) -> Result<(), Av1Error> {
    if !seq.film_grain_params_present || (!header.show_frame && !header.showable_frame) {
        return Ok(());
    }
    let grain = &mut header.film_grain;
    grain.apply_grain = r.read_bit()?;
    if !grain.apply_grain {
        return Ok(());
    }
    grain.grain_seed = r.read_bits(16)? as u16;
    grain.update_grain = if header.frame_type == FrameType::Inter {
        r.read_bit()?
    } else {
        true
    };
    if !grain.update_grain {
        grain.film_grain_params_ref_idx = r.read_bits(3)? as u8;
        return Ok(());
    }

    grain.num_y_points = r.read_bits(4)? as u8;
    for idx in 0..grain.num_y_points as usize {
        grain.point_y_value[idx] = r.read_bits(8)? as u8;
        grain.point_y_scaling[idx] = r.read_bits(8)? as u8;
    }
    if !seq.color_config.mono_chrome {
        grain.chroma_scaling_from_luma = r.read_bit()?;
    }
    if !seq.color_config.mono_chrome
        && !grain.chroma_scaling_from_luma
        && !(seq.color_config.subsampling_x
            && seq.color_config.subsampling_y
            && grain.num_y_points == 0)
    {
        grain.num_cb_points = r.read_bits(4)? as u8;
        for idx in 0..grain.num_cb_points as usize {
            grain.point_cb_value[idx] = r.read_bits(8)? as u8;
            grain.point_cb_scaling[idx] = r.read_bits(8)? as u8;
        }
        grain.num_cr_points = r.read_bits(4)? as u8;
        for idx in 0..grain.num_cr_points as usize {
            grain.point_cr_value[idx] = r.read_bits(8)? as u8;
            grain.point_cr_scaling[idx] = r.read_bits(8)? as u8;
        }
    }
    grain.grain_scaling_minus_8 = r.read_bits(2)? as u8;
    grain.ar_coeff_lag = r.read_bits(2)? as u8;
    let num_pos_luma = 2 * grain.ar_coeff_lag * (grain.ar_coeff_lag + 1);
    let num_pos_chroma = if grain.num_y_points > 0 {
        for idx in 0..num_pos_luma as usize {
            grain.ar_coeffs_y_plus_128[idx] = r.read_bits(8)? as u8;
        }
        num_pos_luma + 1
    } else {
        num_pos_luma
    };
    if grain.chroma_scaling_from_luma || grain.num_cb_points > 0 {
        for idx in 0..num_pos_chroma as usize {
            grain.ar_coeffs_cb_plus_128[idx] = r.read_bits(8)? as u8;
        }
    }
    if grain.chroma_scaling_from_luma || grain.num_cr_points > 0 {
        for idx in 0..num_pos_chroma as usize {
            grain.ar_coeffs_cr_plus_128[idx] = r.read_bits(8)? as u8;
        }
    }
    grain.ar_coeff_shift_minus_6 = r.read_bits(2)? as u8;
    grain.grain_scale_shift = r.read_bits(2)? as u8;
    if grain.num_cb_points > 0 {
        grain.cb_mult = r.read_bits(8)? as u8;
        grain.cb_luma_mult = r.read_bits(8)? as u8;
        grain.cb_offset = r.read_bits(9)? as u16;
    }
    if grain.num_cr_points > 0 {
        grain.cr_mult = r.read_bits(8)? as u8;
        grain.cr_luma_mult = r.read_bits(8)? as u8;
        grain.cr_offset = r.read_bits(9)? as u16;
    }
    grain.overlap_flag = r.read_bit()?;
    grain.clip_to_restricted_range = r.read_bit()?;
    Ok(())
}

/// Parse a complete `OBU_FRAME` through the first byte of its tile payload.
pub fn parse_frame_obu<'a>(
    obu: &Obu<'a>,
    seq: &SequenceHeader,
    state: &mut FrameDecodeState,
) -> Result<ParsedFrameObu<'a>, Av1Error> {
    if obu.obu_type != ObuType::Frame {
        return Err(Av1Error::InvalidValue("OBU_FRAME type"));
    }
    let header = parse_frame_header_with_layer_ids(
        obu.payload,
        seq,
        state,
        obu.temporal_id,
        obu.spatial_id,
    )?;
    if header.show_existing_frame {
        return Err(Av1Error::InvalidValue("show_existing_frame in OBU_FRAME"));
    }

    let mut r = BitReader::new(obu.payload);
    r.bit = header.header_bits;
    r.align()?;
    let tile_group = read_tile_group(&mut r, &header)?;
    let payload_offset = r.bit / 8;
    if payload_offset >= obu.payload.len() {
        return Err(Av1Error::InvalidValue("missing OBU_FRAME tile data"));
    }
    let obu_header_bytes = obu.raw.len() - obu.payload.len();
    Ok(ParsedFrameObu {
        header,
        tile_group,
        tag_size: obu_header_bytes + payload_offset,
        tile_data: &obu.payload[payload_offset..],
    })
}

/// Parse a standalone `OBU_TILE_GROUP` through the first tile byte.
pub fn parse_tile_group_obu<'a>(
    obu: &Obu<'a>,
    header: &FrameHeader,
) -> Result<ParsedTileGroupObu<'a>, Av1Error> {
    if obu.obu_type != ObuType::TileGroup {
        return Err(Av1Error::InvalidValue("OBU_TILE_GROUP type"));
    }
    let mut r = BitReader::new(obu.payload);
    let tile_group = read_tile_group(&mut r, header)?;
    let payload_offset = r.bit / 8;
    if payload_offset >= obu.payload.len() {
        return Err(Av1Error::InvalidValue("missing OBU_TILE_GROUP tile data"));
    }
    let obu_header_bytes = obu.raw.len() - obu.payload.len();
    Ok(ParsedTileGroupObu {
        tile_group,
        tag_size: obu_header_bytes + payload_offset,
        tile_data: &obu.payload[payload_offset..],
    })
}

/// Byte ranges for the tiles covered by one `OBU_TILE_GROUP`.
///
/// `payload_offset` is the absolute position of `payload` in the packet. MPP
/// accumulates this value across tile groups before handing the ranges to the
/// hardware.
pub fn parse_tile_group_layout(
    payload: &[u8],
    header: &FrameHeader,
    group: TileGroup,
    payload_offset: usize,
) -> Result<[Option<std::ops::Range<usize>>; MAX_TILES], Av1Error> {
    let mut layout = std::array::from_fn(|_| None);
    let size_bytes = usize::from(header.tile_size_bytes_minus_1) + 1;
    let mut cursor = 0usize;
    let num_tiles = header.tile_cols.saturating_mul(header.tile_rows) as usize;

    for tile in group.tg_start..=group.tg_end {
        let tile_index = tile as usize;
        if tile_index >= MAX_TILES || tile_index >= num_tiles {
            return Err(Av1Error::InvalidValue("tile index"));
        }
        if tile == group.tg_end {
            let start = payload_offset
                .checked_add(cursor)
                .ok_or(Av1Error::InvalidValue("tile offset"))?;
            let end = payload_offset
                .checked_add(payload.len())
                .ok_or(Av1Error::InvalidValue("tile offset"))?;
            layout[tile_index] = Some(start..end);
            return Ok(layout);
        }

        let size_end = cursor
            .checked_add(size_bytes)
            .filter(|&end| end <= payload.len())
            .ok_or(Av1Error::InvalidValue("truncated tile size"))?;
        let mut tile_size = 0u32;
        for (byte, shift) in payload[cursor..size_end]
            .iter()
            .zip((0..size_bytes).map(|index| index * 8))
        {
            tile_size |= u32::from(*byte) << shift;
        }
        cursor = size_end;
        let tile_size = usize::try_from(tile_size)
            .ok()
            .and_then(|size| size.checked_add(1))
            .ok_or(Av1Error::InvalidValue("tile size"))?;
        if payload.len() - cursor <= tile_size {
            return Err(Av1Error::InvalidValue("truncated tile payload"));
        }
        let start = payload_offset
            .checked_add(cursor)
            .ok_or(Av1Error::InvalidValue("tile offset"))?;
        cursor += tile_size;
        let end = payload_offset
            .checked_add(cursor)
            .ok_or(Av1Error::InvalidValue("tile offset"))?;
        layout[tile_index] = Some(start..end);
    }

    Ok(layout)
}

/// Parse one uncompressed AV1 frame header from a frame or frame-header OBU.
pub fn parse_frame_header(
    payload: &[u8],
    seq: &SequenceHeader,
    state: &mut FrameDecodeState,
) -> Result<FrameHeader, Av1Error> {
    parse_frame_header_with_layer_ids(payload, seq, state, 0, 0)
}

fn parse_frame_header_with_layer_ids(
    payload: &[u8],
    seq: &SequenceHeader,
    state: &mut FrameDecodeState,
    temporal_id: u8,
    spatial_id: u8,
) -> Result<FrameHeader, Av1Error> {
    let mut r = BitReader::new(payload);
    let mut header = FrameHeader {
        primary_ref_frame: PRIMARY_REF_NONE,
        upscaled_width: seq.max_frame_width as u32,
        width: seq.max_frame_width as u32,
        height: seq.max_frame_height as u32,
        render_width: seq.max_frame_width as u32,
        render_height: seq.max_frame_height as u32,
        ref_order_hint: state.references.map(|reference| reference.order_hint),
        loop_filter_ref_deltas: [1, 0, 0, 0, -1, 0, -1, -1],
        ..FrameHeader::default()
    };

    if seq.reduced_still_picture_header {
        header.frame_type = FrameType::Key;
        header.show_frame = true;
    } else {
        header.show_existing_frame = r.read_bit()?;
        if header.show_existing_frame {
            header.frame_to_show_map_idx = r.read_bits(3)? as u8;
            let reference = &state.references[header.frame_to_show_map_idx as usize];
            if !reference.valid {
                return Err(Av1Error::InvalidValue(
                    "show_existing_frame references an invalid slot",
                ));
            }
            if seq.decoder_model_info_present && !seq.timing_info.equal_picture_interval {
                header.frame_presentation_time = r.read_bits(
                    seq.decoder_model_info
                        .frame_presentation_time_length_minus_1
                        + 1,
                )?;
            }
            if seq.frame_id_numbers_present {
                let id_len =
                    seq.additional_frame_id_length_minus_1 + seq.delta_frame_id_length_minus_2 + 3;
                header.display_frame_id = r.read_bits(id_len)?;
            }
            header.frame_type = reference.frame_type;
            header.width = reference.upscaled_width;
            header.upscaled_width = reference.upscaled_width;
            header.height = reference.frame_height;
            header.render_width = reference.render_width;
            header.render_height = reference.render_height;
            header.refresh_frame_flags = if header.frame_type == FrameType::Key {
                header.current_frame_id = reference.frame_id;
                state.order_hint = reference.order_hint;
                state
                    .cdfs
                    .copy_key_frame_to_all_slots(header.frame_to_show_map_idx as usize);
                0xff
            } else {
                0
            };
            header.header_bits = r.bit;
            return Ok(header);
        }

        header.frame_type = FrameType::from_bits(r.read_bits(2)?)?;
        header.show_frame = r.read_bit()?;
        if header.show_frame
            && seq.decoder_model_info_present
            && !seq.timing_info.equal_picture_interval
        {
            header.frame_presentation_time = r.read_bits(
                seq.decoder_model_info
                    .frame_presentation_time_length_minus_1
                    + 1,
            )?;
        }
        if header.show_frame {
            header.showable_frame = header.frame_type != FrameType::Key;
        } else {
            header.showable_frame = r.read_bit()?;
        }
        header.error_resilient_mode = header.frame_type == FrameType::Switch
            || (header.frame_type == FrameType::Key && header.show_frame)
            || r.read_bit()?;
    }

    let frame_is_intra = header.frame_type.is_intra();
    if header.frame_type == FrameType::Key && header.show_frame {
        for reference in &mut state.references {
            reference.valid = false;
            reference.order_hint = 0;
        }
    }

    header.disable_cdf_update = r.read_bit()?;
    header.allow_screen_content_tools = if seq.seq_force_screen_content_tools == 2 {
        r.read_bit()?
    } else {
        seq.seq_force_screen_content_tools != 0
    };
    if header.allow_screen_content_tools {
        header.force_integer_mv = if seq.seq_force_integer_mv == 2 {
            r.read_bit()?
        } else {
            seq.seq_force_integer_mv != 0
        };
    }

    if seq.frame_id_numbers_present {
        let id_len = seq.additional_frame_id_length_minus_1 + seq.delta_frame_id_length_minus_2 + 3;
        header.current_frame_id = r.read_bits(id_len)?;
        invalidate_reference_frame_ids(
            &mut state.references,
            header.current_frame_id,
            id_len,
            seq.delta_frame_id_length_minus_2 + 2,
        );
    }

    header.frame_size_override_flag = if header.frame_type == FrameType::Switch {
        true
    } else if seq.reduced_still_picture_header {
        false
    } else {
        r.read_bit()?
    };
    if seq.enable_order_hint {
        header.order_hint = r.read_bits(seq.order_hint_bits)? as u8;
    }
    state.order_hint = header.order_hint;

    header.primary_ref_frame = if frame_is_intra || header.error_resilient_mode {
        PRIMARY_REF_NONE
    } else {
        r.read_bits(3)? as u8
    };

    if seq.decoder_model_info_present {
        header.buffer_removal_time_present_flag = r.read_bit()?;
        if header.buffer_removal_time_present_flag {
            let width = seq.decoder_model_info.buffer_removal_time_length_minus_1 + 1;
            for (idx, operating_point) in seq.operating_points.iter().enumerate() {
                if operating_point.decoder_model_present {
                    let in_temporal_layer = (operating_point.idc >> temporal_id) & 1 != 0;
                    let in_spatial_layer = (operating_point.idc >> (spatial_id + 8)) & 1 != 0;
                    if operating_point.idc == 0 || (in_temporal_layer && in_spatial_layer) {
                        header.buffer_removal_time[idx] = r.read_bits(width)?;
                    }
                }
            }
        }
    }

    header.refresh_frame_flags = if header.frame_type == FrameType::Switch
        || (header.frame_type == FrameType::Key && header.show_frame)
    {
        0xff
    } else {
        r.read_bits(8)? as u8
    };

    if seq.enable_order_hint && (!frame_is_intra || header.refresh_frame_flags != 0xff) {
        for idx in 0..NUM_REF_FRAMES {
            header.ref_order_hint[idx] = if header.error_resilient_mode {
                r.read_bits(seq.order_hint_bits)? as u8
            } else {
                state.references[idx].order_hint
            };
            if header.ref_order_hint[idx] != state.references[idx].order_hint {
                state.references[idx].valid = false;
            }
        }
    }

    if frame_is_intra {
        read_frame_size(&mut r, seq, &mut header)?;
        read_render_size(&mut r, &mut header)?;
        if header.allow_screen_content_tools && header.upscaled_width == header.width {
            header.allow_intrabc = r.read_bit()?;
        }
    } else {
        if seq.enable_order_hint {
            header.frame_refs_short_signaling = r.read_bit()?;
            if header.frame_refs_short_signaling {
                header.last_frame_idx = r.read_bits(3)? as u8;
                header.golden_frame_idx = r.read_bits(3)? as u8;
                set_frame_refs(seq, &mut header, &state.references)?;
            }
        }
        for idx in 0..REFS_PER_FRAME {
            if !header.frame_refs_short_signaling {
                header.ref_frame_idx[idx] = r.read_bits(3)? as u8;
            }
            if seq.frame_id_numbers_present {
                header.delta_frame_id_minus1[idx] =
                    r.read_bits(seq.delta_frame_id_length_minus_2 + 2)?;
            }
        }
        if header.frame_size_override_flag && !header.error_resilient_mode {
            read_frame_size_with_refs(&mut r, seq, &mut header, &state.references)?;
        } else {
            read_frame_size(&mut r, seq, &mut header)?;
            read_render_size(&mut r, &mut header)?;
        }
        header.allow_high_precision_mv = !header.force_integer_mv && r.read_bit()?;
        if r.read_bit()? {
            header.interpolation_filter = 4;
        } else {
            header.interpolation_filter = r.read_bits(2)? as u8;
        }
        header.is_motion_mode_switchable = r.read_bit()?;
        if !header.error_resilient_mode && seq.enable_ref_frame_mvs {
            header.use_ref_frame_mvs = r.read_bit()?;
        }
    }

    header.disable_frame_end_update_cdf =
        if seq.reduced_still_picture_header || header.disable_cdf_update {
            true
        } else {
            r.read_bit()?
        };
    read_tile_info(&mut r, seq, &mut header)?;
    read_quantization(&mut r, seq, &mut header)?;
    read_segmentation(&mut r, &mut header, &state.references)?;
    read_delta_q_lf(&mut r, &mut header)?;

    header.coded_lossless = true;
    for segment in 0..MAX_SEGMENTS {
        let qindex = if header.feature_enabled[segment][0] {
            i32::from(header.base_q_idx) + i32::from(header.feature_value[segment][0])
        } else {
            i32::from(header.base_q_idx)
        }
        .clamp(0, 255);
        if qindex != 0
            || header.delta_q_y_dc != 0
            || header.delta_q_u_dc != 0
            || header.delta_q_u_ac != 0
            || header.delta_q_v_dc != 0
            || header.delta_q_v_ac != 0
        {
            header.coded_lossless = false;
        }
    }
    header.all_lossless = header.coded_lossless && header.width == header.upscaled_width;

    read_loop_filter(&mut r, seq, &mut header, &state.references)?;
    read_cdef(&mut r, seq, &mut header)?;
    read_restoration(&mut r, seq, &mut header)?;
    if header.coded_lossless {
        header.tx_mode = TxMode::Only4x4;
    } else {
        header.tx_mode = if r.read_bit()? {
            TxMode::Select
        } else {
            TxMode::Largest
        };
    }
    if !header.frame_type.is_intra() {
        header.reference_select = r.read_bit()?;
    }
    read_skip_mode(&mut r, seq, &mut header, &state.references)?;
    if !frame_is_intra && !header.error_resilient_mode && seq.enable_warped_motion {
        header.allow_warped_motion = r.read_bit()?;
    }
    header.reduced_tx_set = r.read_bit()?;
    read_global_motion(&mut r, &mut header)?;
    read_film_grain(&mut r, seq, &mut header)?;
    resolve_film_grain_reference(&mut header, &state.references);
    header.header_bits = r.bit;

    let cdf = if header.error_resilient_mode
        || frame_is_intra
        || header.primary_ref_frame == PRIMARY_REF_NONE
    {
        Av1Cdf::new(
            Av1Cdfs::default_for_q_index(u32::from(header.base_q_idx)),
            Av1MvCdfs::default_intrabc(),
        )
    } else {
        *state
            .cdfs
            .load(header.ref_frame_idx[header.primary_ref_frame as usize] as usize)
    };
    state.active_cdf = cdf;
    state.cdfs.store(header.refresh_frame_flags, &cdf);
    state.references_at_header = state.references;

    for idx in 0..NUM_REF_FRAMES {
        if header.refresh_frame_flags & (1 << idx) != 0 {
            state.references[idx] = ReferenceFrame {
                valid: true,
                frame_id: header.current_frame_id,
                upscaled_width: header.upscaled_width,
                frame_width: header.width,
                frame_height: header.height,
                render_width: header.render_width,
                render_height: header.render_height,
                frame_type: header.frame_type,
                subsampling_x: seq.color_config.subsampling_x,
                subsampling_y: seq.color_config.subsampling_y,
                bit_depth: seq.bit_depth,
                order_hint: header.order_hint,
                loop_filter_ref_deltas: header.loop_filter_ref_deltas,
                loop_filter_mode_deltas: header.loop_filter_mode_deltas,
                feature_enabled: header.feature_enabled,
                feature_value: header.feature_value,
                warped_motion: super::global_motion::resolve(&header, &state.references_at_header),
                lst_frame_offset: u32::from(
                    state.references[header.ref_frame_idx[0] as usize].order_hint,
                ),
                lst2_frame_offset: u32::from(
                    state.references[header.ref_frame_idx[1] as usize].order_hint,
                ),
                lst3_frame_offset: u32::from(
                    state.references[header.ref_frame_idx[2] as usize].order_hint,
                ),
                gld_frame_offset: u32::from(
                    state.references[header.ref_frame_idx[3] as usize].order_hint,
                ),
                bwd_frame_offset: u32::from(
                    state.references[header.ref_frame_idx[4] as usize].order_hint,
                ),
                alt2_frame_offset: u32::from(
                    state.references[header.ref_frame_idx[5] as usize].order_hint,
                ),
                alt_frame_offset: u32::from(
                    state.references[header.ref_frame_idx[6] as usize].order_hint,
                ),
                is_intra_frame: header.frame_type == FrameType::Key,
                intra_only: header.frame_type == FrameType::IntraOnly,
                film_grain: header.film_grain.clone(),
            };
        }
    }
    Ok(header)
}

fn resolve_film_grain_reference(
    header: &mut FrameHeader,
    references: &[ReferenceFrame; NUM_REF_FRAMES],
) {
    let parse_grain = &header.film_grain;
    if !parse_grain.apply_grain || parse_grain.update_grain {
        return;
    }

    // MPP inherits every grain parameter from the selected reference and
    // replaces only the seed. Reference frames carry the fully resolved grain
    // parameters, not just the bits parsed in their own frame header.
    let seed = parse_grain.grain_seed;
    header.film_grain = references[usize::from(parse_grain.film_grain_params_ref_idx)]
        .film_grain
        .clone();
    header.film_grain.grain_seed = seed;
}

fn u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn u64_le(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

fn read_leb128(data: &[u8], cursor: &mut usize) -> Result<u32, Av1Error> {
    let mut value = 0u64;
    for shift in (0..56).step_by(7) {
        let byte = *data.get(*cursor).ok_or(Av1Error::BadLeb128)?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return u32::try_from(value).map_err(|_| Av1Error::BadLeb128);
        }
    }
    Err(Av1Error::BadLeb128)
}

/// Parse the IVF container used by AV1 test streams.
pub fn parse_ivf(data: &[u8]) -> Result<Ivf<'_>, Av1Error> {
    if data.len() < IVF_HEADER_MIN_SIZE || &data[..4] != b"DKIF" || &data[8..12] != b"AV01" {
        return Err(Av1Error::BadIvf);
    }

    let version = u16_le(data, 4);
    let header_len = usize::from(u16_le(data, 6));
    if header_len < IVF_HEADER_MIN_SIZE || header_len > data.len() {
        return Err(Av1Error::BadIvf);
    }

    let width = u16_le(data, 12);
    let height = u16_le(data, 14);
    let frame_rate_numerator = u32_le(data, 16);
    let frame_rate_denominator = u32_le(data, 20);
    let declared = u32_le(data, 24);
    if width == 0 || height == 0 || frame_rate_numerator == 0 || frame_rate_denominator == 0 {
        return Err(Av1Error::BadIvf);
    }

    let mut cursor = header_len;
    let mut frames = Vec::with_capacity(declared as usize);
    while cursor < data.len() {
        if data.len() - cursor < IVF_FRAME_HEADER_SIZE {
            return Err(Av1Error::TruncatedIvfFrameHeader);
        }
        let size = u32_le(data, cursor) as usize;
        let timestamp = u64_le(data, cursor + 4);
        cursor += IVF_FRAME_HEADER_SIZE;
        let end = cursor
            .checked_add(size)
            .filter(|&end| end <= data.len())
            .ok_or(Av1Error::BadIvf)?;
        frames.push(IvfFrame {
            timestamp,
            data: &data[cursor..end],
        });
        cursor = end;
    }

    if declared != 0 && declared as usize != frames.len() {
        return Err(Av1Error::BadFrameCount {
            declared,
            actual: frames.len(),
        });
    }

    Ok(Ivf {
        version,
        width,
        height,
        frame_rate_numerator,
        frame_rate_denominator,
        frames,
    })
}

/// Split one temporal unit into complete OBUs.
pub fn split_obus(packet: &[u8]) -> Result<Vec<Obu<'_>>, Av1Error> {
    let mut cursor = 0usize;
    let mut obus = Vec::new();

    while cursor < packet.len() {
        let start = cursor;
        let byte = packet[cursor];
        cursor += 1;
        if byte & 0x80 != 0 {
            return Err(Av1Error::BadObu);
        }

        let obu_type = ObuType::from_bits((byte >> 3) & 0x0f);
        let extension_flag = byte & 0x04 != 0;
        let has_size_field = byte & 0x02 != 0;

        let (temporal_id, spatial_id) = if extension_flag {
            let extension = *packet.get(cursor).ok_or(Av1Error::BadObu)?;
            cursor += 1;
            ((extension >> 5) & 0x07, (extension >> 3) & 0x03)
        } else {
            (0, 0)
        };

        let payload_size = if has_size_field {
            read_leb128(packet, &mut cursor)? as usize
        } else {
            packet.len() - cursor
        };
        let payload_start = cursor;
        let payload_end = cursor
            .checked_add(payload_size)
            .filter(|&end| end <= packet.len())
            .ok_or(Av1Error::ObuExceedsPacket {
                needed: payload_start.saturating_add(payload_size),
                packet: packet.len(),
            })?;
        cursor = payload_end;

        let header = ObuHeader {
            obu_type,
            extension_flag,
            has_size_field,
            temporal_id,
            spatial_id,
        };
        obus.push(Obu {
            header,
            obu_type,
            temporal_id,
            spatial_id,
            start,
            payload: &packet[payload_start..payload_end],
            raw: &packet[start..payload_end],
        });
    }

    Ok(obus)
}

/// Encode a basic AV1 OBU with no extension and an explicit payload size.
pub fn encode_obu(obu_type: ObuType, payload: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(payload.len() + 7);
    encoded.push(((obu_type as u8) << 3) | 0x02);
    let mut size = payload.len() as u64;
    loop {
        let mut byte = (size & 0x7f) as u8;
        size >>= 7;
        if size != 0 {
            byte |= 0x80;
        }
        encoded.push(byte);
        if size == 0 {
            break;
        }
    }
    encoded.extend_from_slice(payload);
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn references_with_ids(ids: [u32; NUM_REF_FRAMES]) -> [ReferenceFrame; NUM_REF_FRAMES] {
        ids.map(|frame_id| ReferenceFrame {
            valid: true,
            frame_id,
            ..ReferenceFrame::default()
        })
    }

    #[test]
    fn frame_id_window_invalidates_references_outside_the_current_cycle() {
        let mut references = references_with_ids([12, 11, 17, 20, 21, 5, 30, 0]);

        invalidate_reference_frame_ids(&mut references, 20, 5, 3);

        assert_eq!(
            references.map(|reference| reference.valid),
            [true, false, true, true, false, false, false, false]
        );
    }

    #[test]
    fn frame_id_window_handles_id_wraparound() {
        let mut references = references_with_ids([4, 5, 6, 28, 29, 30, 31, 0]);

        invalidate_reference_frame_ids(&mut references, 5, 5, 3);

        assert_eq!(
            references.map(|reference| reference.valid),
            [true, true, false, false, true, true, true, true]
        );
    }

    #[test]
    fn film_grain_without_update_inherits_the_selected_reference_and_replaces_the_seed() {
        let mut references = [ReferenceFrame::default(); NUM_REF_FRAMES];
        references[3].film_grain = FilmGrainParams {
            apply_grain: true,
            grain_seed: 0x1111,
            update_grain: true,
            num_y_points: 2,
            point_y_value: [12, 240, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            point_y_scaling: [40, 90, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ar_coeff_lag: 2,
            ar_coeff_shift_minus_6: 1,
            grain_scale_shift: 3,
            overlap_flag: true,
            ..FilmGrainParams::default()
        };
        let mut header = FrameHeader {
            film_grain: FilmGrainParams {
                apply_grain: true,
                grain_seed: 0xabcd,
                update_grain: false,
                film_grain_params_ref_idx: 3,
                ..FilmGrainParams::default()
            },
            ..FrameHeader::default()
        };

        resolve_film_grain_reference(&mut header, &references);

        assert_eq!(header.film_grain.grain_seed, 0xabcd);
        assert_eq!(header.film_grain.num_y_points, 2);
        assert_eq!(header.film_grain.point_y_value[1], 240);
        assert_eq!(header.film_grain.ar_coeff_lag, 2);
        assert_eq!(header.film_grain.grain_scale_shift, 3);
        assert!(header.film_grain.overlap_flag);
        assert!(header.film_grain.update_grain);
    }
}
