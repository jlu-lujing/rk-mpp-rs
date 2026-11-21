//! Stable synchronous encoder APIs.
//!
//! The base configuration selects fixed-QP H.264/H.265 or JPEG. VEPU580 also
//! accepts CBR/VBR/AVBR through [`RateControlConfig`]. Input frames use tightly
//! packed visible dimensions; this layer pads them to the encoder's coded
//! geometry.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::os::fd::{AsRawFd, BorrowedFd};

use crate::codec::h264::encoder::{
    self as h264_encoder, EncoderConfig as H264EncoderConfig, PROFILE_BASELINE, PROFILE_HIGH,
    PROFILE_MAIN,
};
use crate::codec::h264::encoder::{GdrMode as H264GdrMode, RoiRegion as H264RoiRegion};
use crate::codec::h265::encoder::{
    SliceSplit as H265SliceSplit, SplitMode as H265SplitMode, SyntaxOptions as H265SyntaxOptions,
    MPP_ENC_SPLIT_OUT_LOWDELAY,
};
use crate::codec::h265::roi::RoiRegion as H265RoiRegion;
use crate::hal::h265e::RefreshMode as H265RefreshMode;
use crate::hal::{h264e, h265e_session, jpege, jpege720, osd, vepu510_h264, vepu510_h265};
use crate::sdk::{Codec, Error, PixelFormat};
use crate::service::soc::{SocProfile, VideoEncodeHal};

pub const MAX_ROI_REGIONS: usize = 8;

/// Per-picture bitrate policy for H.264 and H.265 encoders.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateControlMode {
    FixedQp,
    Cbr,
    Vbr,
    Avbr,
}

impl RateControlMode {
    pub const fn c_value(self) -> u32 {
        match self {
            Self::FixedQp => 0,
            Self::Cbr => 1,
            Self::Vbr => 2,
            Self::Avbr => 3,
        }
    }

    pub const fn from_c_value(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::FixedQp),
            1 => Some(Self::Cbr),
            2 => Some(Self::Vbr),
            3 => Some(Self::Avbr),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortTermReorder {
    current_frame_num: u32,
    reference_frame_num: u32,
}

impl ShortTermReorder {
    pub const fn new(current_frame_num: u32, reference_frame_num: u32) -> Self {
        Self {
            current_frame_num,
            reference_frame_num,
        }
    }

    pub const fn current_frame_num(self) -> u32 {
        self.current_frame_num
    }

    pub const fn reference_frame_num(self) -> u32 {
        self.reference_frame_num
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceReorder {
    ShortTerm(ShortTermReorder),
    LongTerm { long_term_pic_num: u32 },
}

/// Bitrate and QP bounds for [`RateControlMode::Cbr`],
/// [`RateControlMode::Vbr`] and [`RateControlMode::Avbr`].
///
/// Bitrates are bits per second. The hardware session chooses a per-picture QP
/// inside `(min_qp..=max_qp)` while keeping the encoded rate near
/// `(bps_min..=bps_max)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateControlConfig {
    mode: RateControlMode,
    bps_target: i64,
    bps_min: i64,
    bps_max: i64,
    initial_qp: u8,
    min_qp: u8,
    max_qp: u8,
    hier_qp_enabled: bool,
    hier_qp_delta: [i32; 4],
    hier_frame_num: [i32; 4],
}

/// Advanced H.264 rate-control policy fields carried by `MppEncCfg`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264RcTuning {
    max_reencode_times: u32,
    super_mode: u8,
    super_i_thd: u32,
    super_p_thd: u32,
    rc_priority: u8,
    debreath_enabled: bool,
    debreath_strength: u8,
    drop_mode: u8,
    drop_threshold: u32,
    drop_gap: u32,
}

impl H264RcTuning {
    pub const fn new() -> Self {
        Self {
            max_reencode_times: 1,
            super_mode: 0,
            super_i_thd: 0,
            super_p_thd: 0,
            rc_priority: 0,
            debreath_enabled: false,
            debreath_strength: 0,
            drop_mode: 0,
            drop_threshold: 20,
            drop_gap: 1,
        }
    }

    pub const fn max_reencode_times(self) -> u32 {
        self.max_reencode_times
    }

    pub const fn super_mode(self) -> u8 {
        self.super_mode
    }

    pub const fn super_i_thd(self) -> u32 {
        self.super_i_thd
    }

    pub const fn super_p_thd(self) -> u32 {
        self.super_p_thd
    }

    pub const fn rc_priority(self) -> u8 {
        self.rc_priority
    }

    pub const fn debreath(self) -> (bool, u8) {
        (self.debreath_enabled, self.debreath_strength)
    }

    pub const fn drop_policy(self) -> (u8, u32, u32) {
        (self.drop_mode, self.drop_threshold, self.drop_gap)
    }

    pub const fn with_max_reencode_times(mut self, times: u32) -> Self {
        self.max_reencode_times = times;
        self
    }

    pub const fn with_super_frame(
        mut self,
        mode: u8,
        i_threshold: u32,
        p_threshold: u32,
        priority: u8,
    ) -> Self {
        self.super_mode = mode;
        self.super_i_thd = i_threshold;
        self.super_p_thd = p_threshold;
        self.rc_priority = priority;
        self
    }

    pub const fn with_debreath(mut self, enabled: bool, strength: u8) -> Self {
        self.debreath_enabled = enabled;
        self.debreath_strength = strength;
        self
    }

    pub const fn with_drop_policy(mut self, mode: u8, threshold: u32, gap: u32) -> Self {
        self.drop_mode = mode;
        self.drop_threshold = threshold;
        self.drop_gap = gap;
        self
    }
}

impl RateControlConfig {
    pub const fn cbr(bps_target: i64, bps_min: i64, bps_max: i64) -> Self {
        Self::new(RateControlMode::Cbr, bps_target, bps_min, bps_max)
    }

    pub const fn vbr(bps_target: i64, bps_min: i64, bps_max: i64) -> Self {
        Self::new(RateControlMode::Vbr, bps_target, bps_min, bps_max)
    }

    pub const fn avbr(bps_target: i64, bps_min: i64, bps_max: i64) -> Self {
        Self::new(RateControlMode::Avbr, bps_target, bps_min, bps_max)
    }

    pub const fn new(mode: RateControlMode, bps_target: i64, bps_min: i64, bps_max: i64) -> Self {
        Self {
            mode,
            bps_target,
            bps_min,
            bps_max,
            initial_qp: 26,
            min_qp: 8,
            max_qp: 48,
            hier_qp_enabled: false,
            hier_qp_delta: [0; 4],
            hier_frame_num: [0; 4],
        }
    }

    pub const fn mode(self) -> RateControlMode {
        self.mode
    }

    pub const fn bps_target(self) -> i64 {
        self.bps_target
    }

    pub const fn bps_min(self) -> i64 {
        self.bps_min
    }

    pub const fn bps_max(self) -> i64 {
        self.bps_max
    }

    pub const fn initial_qp(self) -> u8 {
        self.initial_qp
    }

    pub const fn min_qp(self) -> u8 {
        self.min_qp
    }

    pub const fn max_qp(self) -> u8 {
        self.max_qp
    }

    pub const fn hier_qp_enabled(self) -> bool {
        self.hier_qp_enabled
    }

    pub const fn hier_qp_delta(self) -> [i32; 4] {
        self.hier_qp_delta
    }

    pub const fn hier_frame_num(self) -> [i32; 4] {
        self.hier_frame_num
    }

    pub const fn with_initial_qp(mut self, qp: u8) -> Self {
        self.initial_qp = qp;
        self
    }

    pub const fn with_qp_range(mut self, min_qp: u8, max_qp: u8) -> Self {
        self.min_qp = min_qp;
        self.max_qp = max_qp;
        self
    }

    pub const fn with_hierarchical_qp(mut self, qp_delta: [i32; 4], frame_num: [i32; 4]) -> Self {
        self.hier_qp_enabled = true;
        self.hier_qp_delta = qp_delta;
        self.hier_frame_num = frame_num;
        self
    }

    pub fn validate(self) -> Result<(), Error> {
        if self.mode == RateControlMode::FixedQp {
            return Ok(());
        }
        if self.bps_target <= 0
            || self.bps_min <= 0
            || self.bps_max <= 0
            || self.bps_min > self.bps_target
            || self.bps_target > self.bps_max
        {
            return Err(Error::InvalidInput(format!(
                "bitrate bounds must satisfy 0 < min ({}) <= target ({}) <= max ({})",
                self.bps_min, self.bps_target, self.bps_max
            )));
        }
        if self.bps_max > i64::from(i32::MAX) {
            return Err(Error::InvalidInput(format!(
                "maximum bitrate {} exceeds the hardware signed 32-bit limit",
                self.bps_max
            )));
        }
        if self.min_qp > self.max_qp || self.max_qp > 51 {
            return Err(Error::InvalidInput(format!(
                "rate-control QP range {}..={} is invalid",
                self.min_qp, self.max_qp
            )));
        }
        if !(self.min_qp..=self.max_qp).contains(&self.initial_qp) {
            return Err(Error::InvalidInput(format!(
                "initial rate-control QP {} is outside {}..={}",
                self.initial_qp, self.min_qp, self.max_qp
            )));
        }
        Ok(())
    }
}

/// One region-specific QP/intra request for H.264 or H.265 encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiRegion {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    qp: i32,
    force_intra: bool,
    qp_absolute: bool,
}

impl Default for RoiRegion {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            qp: 0,
            force_intra: false,
            qp_absolute: false,
        }
    }
}

impl RoiRegion {
    /// A relative-QP ROI with no forced intra refresh.
    pub const fn new(x: i32, y: i32, width: i32, height: i32, qp: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
            qp,
            force_intra: false,
            qp_absolute: false,
        }
    }

    pub const fn with_force_intra(mut self, force_intra: bool) -> Self {
        self.force_intra = force_intra;
        self
    }

    pub const fn with_absolute_qp(mut self, absolute: bool) -> Self {
        self.qp_absolute = absolute;
        self
    }

    pub const fn x(self) -> i32 {
        self.x
    }

    pub const fn y(self) -> i32 {
        self.y
    }

    pub const fn width(self) -> i32 {
        self.width
    }

    pub const fn height(self) -> i32 {
        self.height
    }

    pub const fn qp(self) -> i32 {
        self.qp
    }

    pub const fn forces_intra(self) -> bool {
        self.force_intra
    }

    pub const fn uses_absolute_qp(self) -> bool {
        self.qp_absolute
    }
}

/// Up to eight static ROI regions copied into the encoder session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiConfig {
    regions: [RoiRegion; MAX_ROI_REGIONS],
    len: u8,
}

impl RoiConfig {
    pub fn new(regions: &[RoiRegion]) -> Result<Self, Error> {
        if regions.len() > MAX_ROI_REGIONS {
            return Err(Error::InvalidInput(format!(
                "{} ROI regions exceed the maximum of {MAX_ROI_REGIONS}",
                regions.len()
            )));
        }
        let mut copied = [RoiRegion::default(); MAX_ROI_REGIONS];
        copied[..regions.len()].copy_from_slice(regions);
        Ok(Self {
            regions: copied,
            len: regions.len() as u8,
        })
    }

    pub fn regions(&self) -> &[RoiRegion] {
        &self.regions[..usize::from(self.len)]
    }
}

/// GDR / progressive intra-refresh axis.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GdrMode {
    Row,
    Col,
}

/// Sliding intra-refresh configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GdrConfig {
    mode: GdrMode,
    refresh_blocks: u32,
}

impl GdrConfig {
    pub const fn new(mode: GdrMode, refresh_blocks: u32) -> Self {
        Self {
            mode,
            refresh_blocks,
        }
    }

    pub const fn mode(self) -> GdrMode {
        self.mode
    }

    pub const fn refresh_blocks(self) -> u32 {
        self.refresh_blocks
    }
}

/// H.264 slice-deblock controls from `MppEncH264Cfg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H264DeblockConfig {
    disable: u32,
    alpha_offset_div2: i32,
    beta_offset_div2: i32,
}

impl H264DeblockConfig {
    pub const fn new(disable: u32, alpha_offset_div2: i32, beta_offset_div2: i32) -> Self {
        Self {
            disable,
            alpha_offset_div2,
            beta_offset_div2,
        }
    }

    pub const fn disable(self) -> u32 {
        self.disable
    }

    pub const fn alpha_offset_div2(self) -> i32 {
        self.alpha_offset_div2
    }

    pub const fn beta_offset_div2(self) -> i32 {
        self.beta_offset_div2
    }

    fn validate(self) -> Result<(), Error> {
        if !(-6..=6).contains(&self.alpha_offset_div2) || !(-6..=6).contains(&self.beta_offset_div2)
        {
            return Err(Error::InvalidInput(
                "H.264 deblock offsets must be in -6..=6".to_string(),
            ));
        }
        Ok(())
    }
}

/// Entropy and transform overrides from `MppEncH264Cfg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H264SyntaxConfig {
    entropy_coding: bool,
    transform_8x8: bool,
    cabac_init_idc: u8,
    constraint_set: Option<u32>,
    base_layer_pid: Option<u8>,
}

impl H264SyntaxConfig {
    pub const fn new(entropy_coding: bool, transform_8x8: bool) -> Self {
        Self {
            entropy_coding,
            transform_8x8,
            cabac_init_idc: 0,
            constraint_set: None,
            base_layer_pid: None,
        }
    }

    pub const fn entropy_coding(self) -> bool {
        self.entropy_coding
    }

    pub const fn transform_8x8(self) -> bool {
        self.transform_8x8
    }

    pub const fn with_cabac_init_idc(mut self, cabac_init_idc: u8) -> Self {
        self.cabac_init_idc = cabac_init_idc;
        self
    }

    pub const fn cabac_init_idc(self) -> u8 {
        self.cabac_init_idc
    }

    pub const fn with_constraint_set(mut self, constraint_set: u32) -> Self {
        self.constraint_set = Some(constraint_set);
        self
    }

    pub const fn constraint_set(self) -> Option<u32> {
        self.constraint_set
    }

    pub const fn with_base_layer_pid(mut self, base_layer_pid: u8) -> Self {
        self.base_layer_pid = Some(base_layer_pid);
        self
    }

    pub const fn base_layer_pid(self) -> Option<u8> {
        self.base_layer_pid
    }
}

/// What cuts one coded picture into multiple slices.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SliceSplitMode {
    /// H.264 macroblocks or H.265 CTUs per slice.
    ByCtu,
    ByByte,
}

/// Whether slices are returned concatenated or as individual packets.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SliceOutputMode {
    CompleteAccessUnit,
    LowDelayPackets,
}

/// Slice splitting and packet-output configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceSplitConfig {
    mode: SliceSplitMode,
    argument: u32,
    output: SliceOutputMode,
}

impl SliceSplitConfig {
    pub const fn new(mode: SliceSplitMode, argument: u32) -> Self {
        Self {
            mode,
            argument,
            output: SliceOutputMode::CompleteAccessUnit,
        }
    }

    pub const fn with_output(mut self, output: SliceOutputMode) -> Self {
        self.output = output;
        self
    }

    pub const fn mode(self) -> SliceSplitMode {
        self.mode
    }

    pub const fn argument(self) -> u32 {
        self.argument
    }

    pub const fn output(self) -> SliceOutputMode {
        self.output
    }

    fn validate(self) -> Result<(), Error> {
        match self.mode {
            SliceSplitMode::ByCtu if self.argument == 0 || self.argument > 0x0010_0000 => {
                Err(Error::InvalidInput(format!(
                    "slice block count {} is outside 1..=1048576",
                    self.argument
                )))
            }
            SliceSplitMode::ByByte if self.argument == 0 || self.argument > 0x000f_ffff => {
                Err(Error::InvalidInput(format!(
                    "slice byte target {} is outside 1..=1048575",
                    self.argument
                )))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    codec: Codec,
    width: usize,
    height: usize,
    input_format: PixelFormat,
    profile: u8,
    level: u8,
    gop: u32,
    qp: u8,
    num_ref_frames: u32,
    fps_num: u32,
    fps_den: u32,
    jpeg_quality: u32,
    jpeg_qfactor: Option<u32>,
    jpeg_restart_mcus: u32,
    rate_control: Option<RateControlConfig>,
    h264_rc_tuning: Option<H264RcTuning>,
    h264_aq_step_i: Option<[i8; 16]>,
    h264_deblock: Option<H264DeblockConfig>,
    h264_syntax: Option<H264SyntaxConfig>,
    h265_syntax: H265SyntaxOptions,
    roi: Option<RoiConfig>,
    gdr: Option<GdrConfig>,
    slice_split: Option<SliceSplitConfig>,
}

const H265_SYNTAX_DEFAULT: H265SyntaxOptions = H265SyntaxOptions {
    sao_luma_disable: false,
    sao_chroma_disable: false,
    deblock_disable: false,
    deblock_beta_offset_div2: 0,
    deblock_tc_offset_div2: 0,
    loop_filter_across_slices: false,
    loop_filter_across_tiles: true,
    auto_tile: false,
};

impl EncoderConfig {
    pub const fn new(codec: Codec, width: usize, height: usize) -> Self {
        Self {
            codec,
            width,
            height,
            input_format: PixelFormat::Nv12,
            profile: 0,
            level: 30,
            gop: 1,
            qp: 26,
            num_ref_frames: 0,
            fps_num: 30,
            fps_den: 1,
            jpeg_quality: 10,
            jpeg_qfactor: None,
            jpeg_restart_mcus: 0,
            rate_control: None,
            h264_rc_tuning: None,
            h264_aq_step_i: None,
            h264_deblock: None,
            h264_syntax: None,
            h265_syntax: H265_SYNTAX_DEFAULT,
            roi: None,
            gdr: None,
            slice_split: None,
        }
    }

    pub const fn codec(self) -> Codec {
        self.codec
    }

    pub const fn width(self) -> usize {
        self.width
    }

    pub const fn height(self) -> usize {
        self.height
    }

    pub const fn input_format(self) -> PixelFormat {
        self.input_format
    }

    pub const fn profile(self) -> u8 {
        self.profile
    }

    pub const fn level(self) -> u8 {
        self.level
    }

    pub const fn gop(self) -> u32 {
        self.gop
    }

    pub const fn qp(self) -> u8 {
        self.qp
    }

    pub const fn num_ref_frames(self) -> u32 {
        self.num_ref_frames
    }

    pub const fn fps(self) -> (u32, u32) {
        (self.fps_num, self.fps_den)
    }

    pub const fn jpeg_quality(self) -> u32 {
        self.jpeg_quality
    }

    pub const fn jpeg_restart_mcus(self) -> u32 {
        self.jpeg_restart_mcus
    }

    pub const fn jpeg_qfactor(self) -> Option<u32> {
        self.jpeg_qfactor
    }

    pub const fn rate_control(self) -> Option<RateControlConfig> {
        self.rate_control
    }

    pub const fn h264_aq_step_i(self) -> Option<[i8; 16]> {
        self.h264_aq_step_i
    }

    pub const fn h264_deblock(self) -> Option<H264DeblockConfig> {
        self.h264_deblock
    }

    pub const fn h264_syntax(self) -> Option<H264SyntaxConfig> {
        self.h264_syntax
    }

    pub const fn h265_syntax(self) -> H265SyntaxOptions {
        self.h265_syntax
    }

    pub const fn h264_rc_tuning(self) -> Option<H264RcTuning> {
        self.h264_rc_tuning
    }

    pub const fn roi(self) -> Option<RoiConfig> {
        self.roi
    }

    pub const fn gdr(self) -> Option<GdrConfig> {
        self.gdr
    }

    pub const fn slice_split(self) -> Option<SliceSplitConfig> {
        self.slice_split
    }

    pub const fn with_input_format(mut self, format: PixelFormat) -> Self {
        self.input_format = format;
        self
    }

    pub const fn with_profile(mut self, profile: u8) -> Self {
        self.profile = profile;
        self
    }

    pub const fn with_level(mut self, level: u8) -> Self {
        self.level = level;
        self
    }

    pub const fn with_gop(mut self, gop: u32) -> Self {
        self.gop = gop;
        self
    }

    pub const fn with_qp(mut self, qp: u8) -> Self {
        self.qp = qp;
        self
    }

    pub const fn with_num_ref_frames(mut self, frames: u32) -> Self {
        self.num_ref_frames = frames;
        self
    }

    pub const fn with_fps(mut self, fps_num: u32, fps_den: u32) -> Self {
        self.fps_num = fps_num;
        self.fps_den = fps_den;
        self
    }

    pub const fn with_jpeg_quality(mut self, quality: u32) -> Self {
        self.jpeg_quality = quality;
        self.jpeg_qfactor = None;
        self
    }

    pub const fn with_jpeg_qfactor(mut self, qfactor: u32) -> Self {
        self.jpeg_qfactor = Some(qfactor);
        self
    }

    pub const fn with_jpeg_restart_mcus(mut self, mcus: u32) -> Self {
        self.jpeg_restart_mcus = mcus;
        self
    }

    pub const fn with_rate_control(mut self, rate_control: RateControlConfig) -> Self {
        self.rate_control = Some(rate_control);
        self
    }

    pub const fn with_h264_aq_step_i(mut self, aq_step_i: [i8; 16]) -> Self {
        self.h264_aq_step_i = Some(aq_step_i);
        self
    }

    pub const fn with_h264_deblock(mut self, deblock: H264DeblockConfig) -> Self {
        self.h264_deblock = Some(deblock);
        self
    }

    pub const fn with_h264_syntax(mut self, syntax: H264SyntaxConfig) -> Self {
        self.h264_syntax = Some(syntax);
        self
    }

    pub const fn with_h265_syntax(mut self, syntax: H265SyntaxOptions) -> Self {
        self.h265_syntax = syntax;
        self
    }

    pub const fn with_h264_rc_tuning(mut self, tuning: H264RcTuning) -> Self {
        self.h264_rc_tuning = Some(tuning);
        self
    }

    pub const fn with_roi(mut self, roi: RoiConfig) -> Self {
        self.roi = Some(roi);
        self
    }

    pub const fn with_gdr(mut self, gdr: GdrConfig) -> Self {
        self.gdr = Some(gdr);
        self
    }

    pub const fn with_slice_split(mut self, slice_split: SliceSplitConfig) -> Self {
        self.slice_split = Some(slice_split);
        self
    }

    fn validate_advanced_controls(self) -> Result<(), Error> {
        if self.roi.is_some() && self.gdr.is_some() {
            return Err(Error::Unsupported(
                "ROI and GDR are not verified together".to_string(),
            ));
        }
        if let Some(roi) = self.roi {
            for region in roi.regions() {
                if region.x < 0
                    || region.y < 0
                    || region.width <= 0
                    || region.height <= 0
                    || region.x.saturating_add(region.width) > self.width as i32
                    || region.y.saturating_add(region.height) > self.height as i32
                {
                    return Err(Error::InvalidInput(format!(
                        "ROI [{}/{} {}x{}] is outside the {}x{} picture",
                        region.x, region.y, region.width, region.height, self.width, self.height
                    )));
                }
                if region.x > i32::from(u16::MAX)
                    || region.y > i32::from(u16::MAX)
                    || region.width > i32::from(u16::MAX)
                    || region.height > i32::from(u16::MAX)
                {
                    return Err(Error::InvalidInput(
                        "ROI geometry exceeds the hardware 16-bit fields".to_string(),
                    ));
                }
                if region.qp_absolute && !(0..=51).contains(&region.qp) {
                    return Err(Error::InvalidInput(format!(
                        "absolute ROI QP {} is outside 0..=51",
                        region.qp
                    )));
                }
                if !region.qp_absolute && region.qp.unsigned_abs() > 51 {
                    return Err(Error::InvalidInput(format!(
                        "relative ROI QP {} exceeds +-51",
                        region.qp
                    )));
                }
            }
        }
        if let Some(gdr) = self.gdr {
            if gdr.refresh_blocks == 0 {
                return Err(Error::InvalidInput(
                    "GDR refresh block count must be non-zero".to_string(),
                ));
            }
        }
        if let Some(slice_split) = self.slice_split {
            slice_split.validate()?;
        }
        if let Some(deblock) = self.h264_deblock {
            deblock.validate()?;
        }
        if self
            .h264_syntax
            .is_some_and(|syntax| syntax.cabac_init_idc > 2)
        {
            return Err(Error::InvalidInput(
                "H.264 cabac_init_idc must be in 0..=2".to_string(),
            ));
        }
        if self
            .h264_syntax
            .and_then(H264SyntaxConfig::constraint_set)
            .is_some_and(|value| value & !0x003f_003f != 0)
        {
            return Err(Error::InvalidInput(
                "H.264 constraint_set uses only bits 0..=5 and 16..=21".to_string(),
            ));
        }
        if self
            .h264_syntax
            .and_then(H264SyntaxConfig::base_layer_pid)
            .is_some_and(|value| value > 56)
        {
            return Err(Error::InvalidInput(
                "H.264 base_layer_pid must be in 0..=56".to_string(),
            ));
        }
        if !(-6..=6).contains(&self.h265_syntax.deblock_beta_offset_div2)
            || !(-6..=6).contains(&self.h265_syntax.deblock_tc_offset_div2)
        {
            return Err(Error::InvalidInput(
                "H.265 deblock offsets must be in -6..=6".to_string(),
            ));
        }
        Ok(())
    }
}

/// One copied frame submission. Pointer data is fully consumed before return.
#[derive(Debug, Clone)]
pub struct InputFrame<'a> {
    pub data: &'a [u8],
    pub pts: i64,
    pub dts: i64,
    pub(crate) osd: Option<osd::Config>,
}

impl<'a> InputFrame<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pts: 0,
            dts: 0,
            osd: None,
        }
    }

    pub const fn with_pts(mut self, pts: i64) -> Self {
        self.pts = pts;
        self
    }

    pub const fn with_dts(mut self, dts: i64) -> Self {
        self.dts = dts;
        self
    }

    pub(crate) fn with_osd(mut self, osd: Option<osd::Config>) -> Self {
        self.osd = osd;
        self
    }
}

/// One externally allocated NV12 frame submitted by fd.
///
/// The borrow keeps the descriptor live through the synchronous submission.
pub struct DmaBufInput<'a> {
    fd: BorrowedFd<'a>,
    size: usize,
    hor_stride: usize,
    ver_stride: usize,
    pts: i64,
    dts: i64,
    osd: Option<osd::Config>,
}

impl<'a> DmaBufInput<'a> {
    pub fn new(
        fd: BorrowedFd<'a>,
        size: usize,
        hor_stride: usize,
        ver_stride: usize,
    ) -> Result<Self, Error> {
        if size == 0 || hor_stride == 0 || ver_stride == 0 {
            return Err(Error::InvalidInput(
                "DMA-BUF input fd, size and strides must be nonzero".to_string(),
            ));
        }
        Ok(Self {
            fd,
            size,
            hor_stride,
            ver_stride,
            pts: 0,
            dts: 0,
            osd: None,
        })
    }

    pub const fn with_pts(mut self, pts: i64) -> Self {
        self.pts = pts;
        self
    }

    pub const fn with_dts(mut self, dts: i64) -> Self {
        self.dts = dts;
        self
    }

    /// Attach a VEPU580 OSD configuration to this picture.
    pub fn with_osd(mut self, osd: osd::Config) -> Self {
        self.osd = Some(osd);
        self
    }

    fn into_submission(self) -> DmaBufSubmission {
        DmaBufSubmission {
            fd: self.fd.as_raw_fd(),
            size: self.size,
            hor_stride: self.hor_stride,
            ver_stride: self.ver_stride,
            pts: self.pts,
            dts: self.dts,
            osd: self.osd,
        }
    }
}

struct DmaBufSubmission {
    fd: i32,
    size: usize,
    hor_stride: usize,
    ver_stride: usize,
    pts: i64,
    dts: i64,
    osd: Option<osd::Config>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStatistics {
    pub inter64: u32,
    pub inter32: u32,
    pub inter16: u32,
    pub inter8: u32,
    pub intra32: u32,
    pub intra16: u32,
    pub intra8: u32,
    pub intra4: u32,
}

impl BlockStatistics {
    fn is_all_zero(self) -> bool {
        self.inter64 == 0
            && self.inter32 == 0
            && self.inter16 == 0
            && self.inter8 == 0
            && self.intra32 == 0
            && self.intra16 == 0
            && self.intra8 == 0
            && self.intra4 == 0
    }
}

/// The encoder statistics MPP exposes through packet metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacketMeta {
    pub block_statistics: Option<BlockStatistics>,
    pub sse: Option<u64>,
    pub output_pskip: Option<bool>,
    pub rt_bits: Option<i32>,
    pub madi_b16: Option<i32>,
    pub madp_ctu: Option<i32>,
    pub use_ltr: Option<i32>,
    pub output_intra: Option<bool>,
    pub start_qp: Option<i32>,
    pub average_qp: Option<i32>,
}

/// One encoded packet in emission order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    data: Vec<u8>,
    pts: i64,
    dts: i64,
    keyframe: bool,
    frame_num: u32,
    poc: i32,
    meta: PacketMeta,
}

impl Packet {
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    pub const fn pts(&self) -> i64 {
        self.pts
    }

    pub const fn dts(&self) -> i64 {
        self.dts
    }

    pub const fn is_keyframe(&self) -> bool {
        self.keyframe
    }

    pub const fn frame_num(&self) -> u32 {
        self.frame_num
    }

    pub const fn poc(&self) -> i32 {
        self.poc
    }

    pub const fn meta(&self) -> &PacketMeta {
        &self.meta
    }
}

impl PacketMeta {
    fn from_h264(frame: &h264e::EncodedFrame, macroblocks: u32) -> Self {
        let statistics = &frame.statistics;
        let blocks = BlockStatistics {
            inter64: statistics.pnum_p64,
            inter32: statistics.pnum_p32,
            inter16: statistics.pnum_p16,
            inter8: statistics.pnum_p8,
            intra32: statistics.pnum_i32,
            intra16: statistics.pnum_i16,
            intra8: statistics.pnum_i8,
            intra4: statistics.pnum_i4,
        };
        let madi = if statistics.num_b16 == 0 {
            0
        } else {
            (statistics.madi / statistics.num_b16) as i32
        };
        let madp = if statistics.num_ctu == 0 {
            0
        } else {
            (statistics.madp / statistics.num_ctu) as i32
        };
        let quality_real = if macroblocks == 0 {
            0
        } else {
            (statistics.qp_sum / macroblocks) as i32
        };
        Self {
            block_statistics: Some(blocks),
            sse: Some(statistics.sse),
            output_pskip: Some(frame.is_pskip || blocks.is_all_zero()),
            rt_bits: Some(frame.rt_bits),
            madi_b16: Some(madi),
            madp_ctu: Some(madp),
            use_ltr: frame.use_ltr.map(|index| index as i32),
            output_intra: Some(frame.is_intra),
            start_qp: Some(frame.quality_target),
            average_qp: Some(quality_real),
        }
    }

    fn from_h265(frame: &h265e_session::EncodedFrame, width: usize, height: usize) -> Self {
        let statistics = &frame.statistics;
        let blocks = BlockStatistics {
            inter64: statistics.pnum_p64,
            inter32: statistics.pnum_p32,
            inter16: statistics.pnum_p16,
            inter8: statistics.pnum_p8,
            intra32: statistics.pnum_i32,
            intra16: statistics.pnum_i16,
            intra8: statistics.pnum_i8,
            intra4: statistics.pnum_i4,
        };
        let madi = if statistics.num_b16 == 0 {
            0
        } else {
            (statistics.madi_sum / statistics.num_b16) as i32
        };
        let madp = if statistics.num_ctu == 0 {
            0
        } else {
            (statistics.madp_sum / statistics.num_ctu) as i32
        };
        Self {
            block_statistics: Some(blocks),
            sse: Some(statistics.sse),
            output_pskip: Some(blocks.is_all_zero()),
            rt_bits: Some(frame.rt_bits),
            madi_b16: Some(madi),
            madp_ctu: Some(madp),
            use_ltr: None,
            output_intra: Some(frame.is_idr),
            start_qp: Some(frame.quality_target),
            average_qp: Some(statistics.quality_real(width, height)),
        }
    }

    pub(crate) fn from_vepu510_status(
        status: &[u32],
        is_intra: bool,
        start_qp: i32,
        average_qp: i32,
    ) -> Self {
        let word = |index: usize| status.get(index).copied().unwrap_or(0);
        let blocks = BlockStatistics {
            inter64: word(32) & 0x1ffff,
            inter32: word(33) & 0x7ffff,
            inter16: word(34) & 0x1f_ffff,
            inter8: word(35) & 0x7f_ffff,
            intra32: word(36) & 0x7ffff,
            intra16: word(37) & 0x1f_ffff,
            intra8: word(38) & 0x7f_ffff,
            intra4: word(39) & 0x7f_ffff,
        };
        Self {
            block_statistics: Some(blocks),
            sse: Some((u64::from(word(2)) << 16) | u64::from((word(1) >> 16) & 0xffff)),
            output_pskip: Some(blocks.is_all_zero()),
            rt_bits: None,
            madi_b16: None,
            madp_ctu: None,
            use_ltr: None,
            output_intra: Some(is_intra),
            start_qp: Some(start_qp),
            average_qp: Some(average_qp),
        }
    }

    fn fixed_qp(is_intra: bool, qp: i32) -> Self {
        Self {
            output_intra: Some(is_intra),
            start_qp: Some(qp),
            average_qp: Some(qp),
            ..Self::default()
        }
    }

    fn jpeg(qfactor: u32, output_intra: Option<bool>) -> Self {
        Self {
            output_intra,
            start_qp: Some(qfactor.min(i32::MAX as u32) as i32),
            average_qp: Some(qfactor.min(i32::MAX as u32) as i32),
            ..Self::default()
        }
    }
}

enum Backend {
    H264Vepu580(h264e::Vepu580H264),
    H264Vepu510(vepu510_h264::Vepu510H264),
    H265Vepu580(h265e_session::Vepu580H265),
    H265Vepu510(vepu510_h265::Vepu510H265),
    Jpeg {
        encoder: jpege720::JpegEncoder,
        frame_number: u32,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ExternalRateControl {
    pub(crate) bit_target: i32,
    pub(crate) qp_target: u32,
    pub(crate) qp_min: u32,
    pub(crate) qp_max: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RateControlFeedback {
    pub(crate) bit_real: i32,
    pub(crate) quality_real: i32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RateControlReencode {
    pub(crate) target: ExternalRateControl,
    pub(crate) is_intra: bool,
    pub(crate) is_idr: bool,
}

/// Persistent encoder session. Frames are encoded synchronously into an owned
/// packet queue, so input storage can be reused as soon as submit returns.
pub struct Encoder {
    config: EncoderConfig,
    backend: Backend,
    packets: VecDeque<Packet>,
    eos: bool,
    mpi_compat: bool,
    last_motion_info: Option<Vec<u8>>,
    next_rate_control: Option<ExternalRateControl>,
    h264_pipeline_timing: VecDeque<(i64, i64)>,
    h265_pipeline_timing: VecDeque<(i64, i64)>,
}

impl Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self, Error> {
        Self::new_with_mode(config, false)
    }

    pub(crate) fn new_mpi_compat(config: EncoderConfig) -> Result<Self, Error> {
        Self::new_with_mode(config, true)
    }

    fn new_with_mode(config: EncoderConfig, mpi_compat: bool) -> Result<Self, Error> {
        config.validate_advanced_controls()?;
        if let Some(rate_control) = config.rate_control {
            rate_control.validate()?;
            if config.codec == Codec::Mjpeg && rate_control.mode != RateControlMode::FixedQp {
                return Err(Error::Unsupported(
                    "JPEG encoder does not support bitrate rate control".to_string(),
                ));
            }
        }
        if config.codec == Codec::Mjpeg
            && (config.roi.is_some() || config.gdr.is_some() || config.slice_split.is_some())
        {
            return Err(Error::Unsupported(
                "JPEG encoder does not support ROI, GDR or slice splitting".to_string(),
            ));
        }
        if config.codec == Codec::H264
            && config.gdr.is_some()
            && config
                .rate_control
                .is_some_and(|rate_control| rate_control.mode != RateControlMode::FixedQp)
        {
            return Err(Error::Unsupported(
                "H.264 GDR is currently verified only with fixed QP".to_string(),
            ));
        }
        let backend = match config.codec {
            Codec::H264 => open_h264_backend(config)?,
            Codec::H265 => open_h265_backend_with_mode(config, mpi_compat)?,
            Codec::Mjpeg => Backend::Jpeg {
                encoder: open_jpeg()?,
                frame_number: 0,
            },
            Codec::Vp8 | Codec::Vp9 | Codec::Av1 | Codec::Mpeg2 | Codec::Mpeg4 | Codec::H263 => {
                return Err(Error::Unsupported(format!(
                    "encoder backend for {:?} is not implemented",
                    config.codec
                )));
            }
        };
        Ok(Self {
            config,
            backend,
            packets: VecDeque::new(),
            eos: false,
            mpi_compat,
            last_motion_info: None,
            next_rate_control: None,
            h264_pipeline_timing: VecDeque::new(),
            h265_pipeline_timing: VecDeque::new(),
        })
    }

    pub const fn config(&self) -> EncoderConfig {
        self.config
    }

    pub fn force_reference_slot(&mut self, slot: Option<usize>) -> Result<(), Error> {
        match &mut self.backend {
            Backend::H264Vepu580(encoder) => {
                encoder.force_reference_slot(slot).map_err(Error::Hardware)
            }
            Backend::H264Vepu510(_) if slot.is_none() => Ok(()),
            _ => Err(Error::Unsupported(
                "reference-slot forcing requires the per-picture VEPU580/VEPU510 layout"
                    .to_string(),
            )),
        }
    }

    pub fn force_idr(&mut self) -> Result<(), Error> {
        match &mut self.backend {
            Backend::H264Vepu580(encoder) => {
                encoder.force_idr();
                Ok(())
            }
            Backend::H264Vepu510(encoder) => {
                encoder.force_idr();
                Ok(())
            }
            Backend::H265Vepu580(encoder) => {
                encoder.force_idr();
                Ok(())
            }
            Backend::H265Vepu510(encoder) => {
                encoder.force_idr();
                Ok(())
            }
            _ => Err(Error::Unsupported(
                "per-picture forced IDR is unavailable for this encoder backend".to_string(),
            )),
        }
    }

    pub fn set_headers_each_idr(&mut self, each_idr: bool) -> Result<(), Error> {
        match &mut self.backend {
            Backend::H264Vepu580(encoder) => {
                encoder.set_headers_each_idr(each_idr);
                Ok(())
            }
            Backend::H264Vepu510(encoder) => {
                encoder.set_headers_each_idr(each_idr);
                Ok(())
            }
            Backend::H265Vepu580(encoder) => {
                encoder.set_headers_each_idr(each_idr);
                Ok(())
            }
            Backend::H265Vepu510(encoder) => {
                encoder.set_headers_each_idr(each_idr);
                Ok(())
            }
            _ => Err(Error::Unsupported(
                "per-IDR header mode is unavailable for this encoder backend".to_string(),
            )),
        }
    }

    pub fn set_sei_enabled(&mut self, enabled: bool) -> Result<(), Error> {
        match &mut self.backend {
            Backend::H264Vepu580(encoder) => {
                encoder.set_sei_enabled(enabled);
                Ok(())
            }
            Backend::H264Vepu510(encoder) => {
                encoder.set_sei_enabled(enabled);
                Ok(())
            }
            Backend::H265Vepu580(encoder) => {
                encoder.set_sei_enabled(enabled);
                Ok(())
            }
            Backend::H265Vepu510(encoder) => {
                encoder.set_sei_enabled(enabled);
                Ok(())
            }
            _ => Err(Error::Unsupported(
                "per-session SEI mode is unavailable for this encoder backend".to_string(),
            )),
        }
    }

    pub fn set_motion_info_enabled(&mut self, enabled: bool) {
        if let Backend::H264Vepu580(encoder) = &mut self.backend {
            encoder.set_motion_info_enabled(enabled);
        }
    }

    pub fn set_deferred_scene_tune(&mut self, enabled: bool) {
        if let Backend::H264Vepu580(encoder) = &mut self.backend {
            encoder.set_deferred_scene_tune(enabled);
        }
    }

    pub(crate) fn set_h264_pipeline(&mut self, enabled: bool, depth: usize) -> bool {
        if let Backend::H264Vepu580(encoder) = &mut self.backend {
            encoder.set_pipeline_depth(depth);
            encoder.set_pipeline(enabled);
            self.h264_pipeline_timing.clear();
            true
        } else {
            false
        }
    }

    pub(crate) fn h264_pipeline_empty(&self) -> bool {
        matches!(
            &self.backend,
            Backend::H264Vepu580(encoder) if encoder.pipeline_empty()
        )
    }

    pub(crate) fn set_h265_pipeline(&mut self, enabled: bool, depth: usize) -> bool {
        if let Backend::H265Vepu580(encoder) = &mut self.backend {
            encoder.set_pipeline_depth(depth);
            encoder.set_pipeline(enabled);
            self.h265_pipeline_timing.clear();
            true
        } else {
            false
        }
    }

    pub(crate) fn h265_pipeline_empty(&self) -> bool {
        matches!(
            &self.backend,
            Backend::H265Vepu580(encoder) if encoder.pipeline_empty()
        )
    }

    pub fn set_h265_long_term_reference(&mut self, long_term: bool) -> Result<(), Error> {
        match &mut self.backend {
            Backend::H265Vepu580(encoder) => {
                encoder.set_long_term(long_term);
                Ok(())
            }
            Backend::H265Vepu510(_) => Ok(()),
            _ => Err(Error::Unsupported(
                "H.265 long-term SPS mode requires an H.265 encoder".to_string(),
            )),
        }
    }

    pub(crate) fn set_h265_mpi_compat(&mut self) -> Result<(), Error> {
        match &mut self.backend {
            Backend::H265Vepu510(encoder) => {
                encoder.set_mpi_compat(true);
                Ok(())
            }
            Backend::H265Vepu580(_) => Ok(()),
            _ => Err(Error::Unsupported(
                "MPI-compatible H.265 headers require an H.265 encoder".to_string(),
            )),
        }
    }

    pub(crate) fn set_rate_control_name(&mut self, name: String) -> Result<(), Error> {
        match &mut self.backend {
            Backend::H264Vepu580(encoder) => {
                encoder.set_rate_control_name(name);
                Ok(())
            }
            Backend::H265Vepu580(encoder) => {
                encoder.set_rate_control_name(name);
                Ok(())
            }
            Backend::H265Vepu510(encoder) => {
                encoder.set_rate_control_name(name);
                Ok(())
            }
            Backend::H264Vepu510(_) => Ok(()),
            _ => Err(Error::Unsupported(
                "custom rate-control names require an H.264/H.265 encoder".to_string(),
            )),
        }
    }

    pub(crate) fn supports_h264_reference_layout(&self) -> bool {
        matches!(
            &self.backend,
            Backend::H264Vepu580(_) | Backend::H264Vepu510(_)
        )
    }

    pub(crate) fn supports_external_rate_control(&self) -> bool {
        matches!(
            &self.backend,
            Backend::H264Vepu580(_)
                | Backend::H264Vepu510(_)
                | Backend::H265Vepu580(_)
                | Backend::H265Vepu510(_)
        )
    }

    pub(crate) fn force_next_rate_control(
        &mut self,
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
    ) {
        self.next_rate_control = Some(ExternalRateControl {
            bit_target,
            qp_target,
            qp_min,
            qp_max,
        });
    }

    pub fn force_next_picture_layout(
        &mut self,
        current_slot: usize,
        reference_slot: usize,
        non_reference: bool,
        temporal_id: Option<u8>,
        frame_num: u32,
        poc: u32,
        reference_reorder: Option<ReferenceReorder>,
        current_long_term_index: Option<u32>,
    ) -> Result<(), Error> {
        match &mut self.backend {
            Backend::H264Vepu580(encoder) => encoder
                .force_next_picture_layout(
                    current_slot,
                    reference_slot,
                    non_reference,
                    temporal_id,
                    frame_num,
                    poc,
                    reference_reorder,
                    current_long_term_index,
                )
                .map_err(Error::Hardware),
            Backend::H264Vepu510(encoder) => encoder
                .force_next_picture_layout(
                    current_slot,
                    reference_slot,
                    non_reference,
                    temporal_id.unwrap_or(0),
                    frame_num,
                    poc,
                    reference_reorder,
                    current_long_term_index,
                )
                .map_err(Error::Hardware),
            _ => Err(Error::Unsupported(
                "per-picture reference layout is currently implemented for VEPU580 H.264"
                    .to_string(),
            )),
        }
    }

    fn queue_h264_frames(&mut self, frames: Vec<h264e::EncodedFrame>) -> Result<usize, Error> {
        let coded_width = self.config.width().div_ceil(16) * 16;
        let coded_height = self.config.height().div_ceil(16) * 16;
        let macroblocks = ((coded_width / 16) * (coded_height / 16)) as u32;
        let mut packets = 0;
        for encoded in frames {
            let (pts, dts) = self.h264_pipeline_timing.pop_front().ok_or_else(|| {
                Error::State("H.264 pipeline completion has no queued input timestamp".into())
            })?;
            let meta = PacketMeta::from_h264(&encoded, macroblocks);
            packets += encoded.packets.len();
            for data in encoded.packets {
                self.packets.push_back(Packet {
                    data,
                    pts,
                    dts,
                    keyframe: encoded.is_idr,
                    frame_num: encoded.frame_num,
                    poc: encoded.poc,
                    meta: meta.clone(),
                });
            }
        }
        Ok(packets)
    }

    fn queue_h265_frames(
        &mut self,
        frames: Vec<h265e_session::EncodedFrame>,
    ) -> Result<usize, Error> {
        let mut packets = 0;
        for encoded in frames {
            let (pts, dts) = self.h265_pipeline_timing.pop_front().ok_or_else(|| {
                Error::State("H.265 pipeline completion has no queued input timestamp".into())
            })?;
            let meta = PacketMeta::from_h265(&encoded, self.config.width(), self.config.height());
            packets += encoded.packets.len();
            for data in encoded.packets {
                self.packets.push_back(Packet {
                    data,
                    pts,
                    dts,
                    keyframe: encoded.is_idr,
                    frame_num: encoded.frame_num,
                    poc: encoded.frame_num as i32 * 2,
                    meta: meta.clone(),
                });
            }
        }
        Ok(packets)
    }

    pub(crate) fn flush_pending(&mut self) -> Result<usize, Error> {
        match &mut self.backend {
            Backend::H264Vepu580(encoder) if encoder.pipeline_enabled() => {
                let frames = encoder
                    .flush_pipeline()
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                self.queue_h264_frames(frames)
            }
            Backend::H265Vepu580(encoder) if encoder.pipeline_enabled() => {
                let frames = encoder
                    .flush_pipeline()
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                self.queue_h265_frames(frames)
            }
            _ => Ok(0),
        }
    }

    pub fn submit_frame(&mut self, frame: InputFrame<'_>) -> Result<usize, Error> {
        if self.eos {
            return Err(Error::State("frame submitted after encoder EOS"));
        }
        let external_rc = self.next_rate_control.take();
        let osd = frame.osd.clone();
        let use_rate_control_registers = self
            .config
            .rate_control
            .is_some_and(|rate_control| rate_control.mode() != RateControlMode::FixedQp);
        let submitted: Result<usize, Error> = match &mut self.backend {
            Backend::H264Vepu580(encoder) => {
                encoder.set_next_osd(osd.clone());
                if let Some(rc) = external_rc.filter(|_| use_rate_control_registers) {
                    encoder.force_next_rate_control(h264e::RateControl {
                        bit_target: rc.bit_target,
                        qp_target: rc.qp_target,
                        qp_min: rc.qp_min,
                        qp_max: rc.qp_max,
                        qp_delta_row: 2,
                        qp_delta_row_i: 2,
                    });
                }
                let encoded = if needs_visible_nv12_copy(self.config) {
                    encoder
                        .load_visible_input_frame(frame.data)
                        .map_err(Error::Hardware)?;
                    encoder
                        .encode_frame_for_queue(&[])
                        .map_err(|error| Error::Hardware(error.to_string()))?
                } else {
                    let input = format_nv12_input(self.config, frame.data)?;
                    encoder
                        .encode_frame_for_queue(&input)
                        .map_err(|error| Error::Hardware(error.to_string()))?
                };
                let count = encoded.packets.len();
                let coded_width = self.config.width().div_ceil(16) * 16;
                let coded_height = self.config.height().div_ceil(16) * 16;
                let macroblocks = ((coded_width / 16) * (coded_height / 16)) as u32;
                let meta = PacketMeta::from_h264(&encoded, macroblocks);
                for data in encoded.packets {
                    self.packets.push_back(Packet {
                        data,
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: encoded.is_idr,
                        frame_num: encoded.frame_num,
                        poc: encoded.poc,
                        meta: meta.clone(),
                    });
                }
                Ok(count)
            }
            Backend::H264Vepu510(encoder) => {
                if osd.is_some() {
                    return Err(Error::Unsupported(
                        "RK3576 H.264 OSD is not available".to_string(),
                    ));
                }
                let input = format_nv12_input(self.config, frame.data)?;
                let encoded = if let Some(rc) = external_rc {
                    encoder.encode_next_with_rate_320x240(
                        &input,
                        rc.bit_target,
                        rc.qp_target,
                        rc.qp_min,
                        rc.qp_max,
                    )
                } else {
                    encoder.encode_next_320x240(&input)
                }
                .map_err(|error| Error::Hardware(error.to_string()))?;
                let (data, keyframe, frame_num, poc) = encoded;
                let meta = encoder
                    .last_packet_meta(keyframe)
                    .unwrap_or_else(|| PacketMeta::fixed_qp(keyframe, i32::from(self.config.qp)));
                self.packets.push_back(Packet {
                    data,
                    pts: frame.pts,
                    dts: frame.dts,
                    keyframe,
                    frame_num,
                    poc,
                    meta,
                });
                Ok(1)
            }
            Backend::H265Vepu580(encoder) => {
                encoder.set_next_osd(osd.clone());
                let encoded = if needs_visible_nv12_copy(self.config) {
                    if let Some(rc) = external_rc.filter(|_| use_rate_control_registers) {
                        encoder
                            .encode_visible_frame_with_rc_packets_for_queue(
                                frame.data,
                                rc.bit_target,
                                rc.qp_target,
                                rc.qp_min,
                                rc.qp_max,
                            )
                            .map_err(Error::Hardware)?
                    } else {
                        encoder
                            .encode_visible_frame_packets_for_queue(frame.data)
                            .map_err(Error::Hardware)?
                    }
                } else if let Some(rc) = external_rc.filter(|_| use_rate_control_registers) {
                    let input = format_nv12_input(self.config, frame.data)?;
                    encoder
                        .encode_frame_with_rc_packets_for_queue(
                            &input,
                            rc.bit_target,
                            rc.qp_target,
                            rc.qp_min,
                            rc.qp_max,
                        )
                        .map_err(|error| Error::Hardware(error))?
                } else {
                    let input = format_nv12_input(self.config, frame.data)?;
                    encoder
                        .encode_frame_packets_for_queue(&input)
                        .map_err(|error| Error::Hardware(error.to_string()))?
                };
                let count = encoded.packets.len();
                let meta =
                    PacketMeta::from_h265(&encoded, self.config.width(), self.config.height());
                for data in encoded.packets {
                    self.packets.push_back(Packet {
                        data,
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: encoded.is_idr,
                        frame_num: encoded.frame_num,
                        poc: encoded.frame_num as i32 * 2,
                        meta: meta.clone(),
                    });
                }
                Ok(count)
            }
            Backend::H265Vepu510(encoder) => {
                if osd.is_some() {
                    return Err(Error::Unsupported(
                        "RK3576 H.265 OSD is not available".to_string(),
                    ));
                }
                let input = format_nv12_input(self.config, frame.data)?;
                let encoded = if let Some(rc) = external_rc {
                    encoder.encode_next_with_rate_320x240(
                        &input,
                        rc.bit_target,
                        rc.qp_target,
                        rc.qp_min,
                        rc.qp_max,
                    )
                } else {
                    encoder.encode_next_320x240(&input)
                }
                .map_err(|error| Error::Hardware(error.to_string()))?;
                let (data, keyframe, frame_num, poc) = encoded;
                let meta = encoder
                    .last_packet_meta(keyframe)
                    .unwrap_or_else(|| PacketMeta::fixed_qp(keyframe, i32::from(self.config.qp)));
                self.packets.push_back(Packet {
                    data,
                    pts: frame.pts,
                    dts: frame.dts,
                    keyframe,
                    frame_num,
                    poc,
                    meta,
                });
                Ok(1)
            }
            Backend::Jpeg {
                encoder,
                frame_number,
            } => {
                if osd.is_some() {
                    return Err(Error::Unsupported(
                        "JPEG encoder OSD is not available".to_string(),
                    ));
                }
                if external_rc.is_some() {
                    return Err(Error::Unsupported(
                        "JPEG custom RC is not available".to_string(),
                    ));
                }
                let input = format_nv12_padded(self.config, frame.data, 8, 2)?;
                let jpeg = jpeg_config(self.config)?;
                let encoded = encoder
                    .encode_packets(&input, jpeg)
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                let count = encoded.packets.len();
                let meta = PacketMeta::jpeg(
                    self.config
                        .jpeg_qfactor()
                        .unwrap_or(self.config.jpeg_quality()),
                    encoder.packet_output_intra(),
                );
                for range in encoded.packets {
                    self.packets.push_back(Packet {
                        data: encoded.bytes[range].to_vec(),
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: true,
                        frame_num: *frame_number,
                        poc: *frame_number as i32,
                        meta: meta.clone(),
                    });
                }
                *frame_number = frame_number.saturating_add(1);
                Ok(count)
            }
        };
        let count = submitted?;
        self.last_motion_info = match &mut self.backend {
            Backend::H264Vepu580(encoder) if encoder.motion_info_enabled() => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            Backend::H264Vepu580(_) => None,
            Backend::H264Vepu510(encoder) => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            Backend::H265Vepu580(encoder) => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            Backend::H265Vepu510(encoder) => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            _ => None,
        };
        Ok(count)
    }

    pub(crate) fn submit_frame_with_rate_control_reencode<F>(
        &mut self,
        frame: InputFrame<'_>,
        mut reencode: F,
    ) -> Result<usize, Error>
    where
        F: FnMut(RateControlFeedback) -> Result<Option<RateControlReencode>, String>,
    {
        if self.eos {
            return Err(Error::State("frame submitted after encoder EOS"));
        }
        let Some(initial) = self.next_rate_control.take() else {
            return Err(Error::State(
                "rate-control reencode requires an initial RC target",
            ));
        };
        if frame.osd.is_some() {
            return Err(Error::Unsupported(
                "rate-control reencode is not available with OSD".to_string(),
            ));
        }
        let use_rate_control_registers = self
            .config
            .rate_control
            .is_some_and(|rate_control| rate_control.mode() != RateControlMode::FixedQp);
        let submitted = match &mut self.backend {
            Backend::H264Vepu580(encoder) => {
                let input = if needs_visible_nv12_copy(self.config) {
                    encoder
                        .load_visible_input_frame(frame.data)
                        .map_err(Error::Hardware)?;
                    None
                } else {
                    Some(format_nv12_input(self.config, frame.data)?)
                };
                let input = input.as_deref().unwrap_or(&[]);
                let encoded = if use_rate_control_registers {
                    encoder.encode_frame_with_rc_reencode(input, initial, &mut reencode)
                } else {
                    encoder.encode_frame_with_fixed_qp_reencode(input, &mut reencode)
                }
                .map_err(Error::Hardware)?;
                let count = encoded.packets.len();
                let coded_width = self.config.width().div_ceil(16) * 16;
                let coded_height = self.config.height().div_ceil(16) * 16;
                let macroblocks = ((coded_width / 16) * (coded_height / 16)) as u32;
                let meta = PacketMeta::from_h264(&encoded, macroblocks);
                for data in encoded.packets {
                    self.packets.push_back(Packet {
                        data,
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: encoded.is_idr,
                        frame_num: encoded.frame_num,
                        poc: encoded.poc,
                        meta: meta.clone(),
                    });
                }
                Ok(count)
            }
            Backend::H264Vepu510(encoder) => {
                let input = format_nv12_input(self.config, frame.data)?;
                let (data, keyframe, frame_num, poc) = encoder
                    .encode_next_with_rate_reencode_320x240(&input, initial, &mut reencode)
                    .map_err(Error::Hardware)?;
                let meta = encoder
                    .last_packet_meta(keyframe)
                    .unwrap_or_else(|| PacketMeta::fixed_qp(keyframe, i32::from(self.config.qp)));
                self.packets.push_back(Packet {
                    data,
                    pts: frame.pts,
                    dts: frame.dts,
                    keyframe,
                    frame_num,
                    poc,
                    meta,
                });
                Ok(1)
            }
            Backend::H265Vepu580(encoder) => {
                let encoded = if needs_visible_nv12_copy(self.config) && use_rate_control_registers
                {
                    encoder.encode_visible_frame_with_rc_reencode(
                        frame.data,
                        initial,
                        &mut reencode,
                    )
                } else if needs_visible_nv12_copy(self.config) {
                    encoder.encode_visible_frame_with_fixed_qp_reencode(frame.data, &mut reencode)
                } else if use_rate_control_registers {
                    let input = format_nv12_input(self.config, frame.data)?;
                    encoder.encode_frame_with_rc_reencode(&input, initial, &mut reencode)
                } else {
                    let input = format_nv12_input(self.config, frame.data)?;
                    encoder.encode_frame_with_fixed_qp_reencode(&input, &mut reencode)
                }
                .map_err(Error::Hardware)?;
                let count = encoded.packets.len();
                let meta =
                    PacketMeta::from_h265(&encoded, self.config.width(), self.config.height());
                for data in encoded.packets {
                    self.packets.push_back(Packet {
                        data,
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: encoded.is_idr,
                        frame_num: encoded.frame_num,
                        poc: encoded.frame_num as i32 * 2,
                        meta: meta.clone(),
                    });
                }
                Ok(count)
            }
            Backend::H265Vepu510(encoder) => {
                let input = format_nv12_input(self.config, frame.data)?;
                let (data, keyframe, frame_num, poc) = encoder
                    .encode_next_with_rate_reencode_320x240(&input, initial, &mut reencode)
                    .map_err(Error::Hardware)?;
                let meta = encoder
                    .last_packet_meta(keyframe)
                    .unwrap_or_else(|| PacketMeta::fixed_qp(keyframe, i32::from(self.config.qp)));
                self.packets.push_back(Packet {
                    data,
                    pts: frame.pts,
                    dts: frame.dts,
                    keyframe,
                    frame_num,
                    poc,
                    meta,
                });
                Ok(1)
            }
            _ => Err(Error::Unsupported(
                "rate-control reencode is not implemented for this backend".to_string(),
            )),
        };
        let count = submitted?;
        self.last_motion_info = match &mut self.backend {
            Backend::H264Vepu580(encoder) if encoder.motion_info_enabled() => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            Backend::H264Vepu580(_) => None,
            Backend::H264Vepu510(encoder) => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            Backend::H265Vepu580(encoder) => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            Backend::H265Vepu510(encoder) => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            _ => None,
        };
        Ok(count)
    }

    pub fn submit_dmabuf_frame(&mut self, frame: DmaBufInput<'_>) -> Result<usize, Error> {
        self.submit_dmabuf_submission(frame.into_submission())
    }

    /// Return whether these DMA-BUF geometry parameters are accepted without
    /// falling back to a CPU staging copy.
    pub(crate) fn accepts_dmabuf_frame(
        &self,
        size: usize,
        hor_stride: usize,
        ver_stride: usize,
        osd: bool,
    ) -> bool {
        !self.eos
            && matches!(
                self.backend,
                Backend::H264Vepu580(_)
                    | Backend::H264Vepu510(_)
                    | Backend::H265Vepu580(_)
                    | Backend::H265Vepu510(_)
            )
            && validate_dmabuf_layout(self.config, size, hor_stride, ver_stride).is_ok()
            && (!osd
                || matches!(
                    self.backend,
                    Backend::H264Vepu580(_) | Backend::H265Vepu580(_)
                ))
    }

    /// Submit a DMA-BUF that remains owned by the caller for the duration of
    /// this synchronous call.
    ///
    /// # Safety
    /// `fd` must reference a readable DMA-BUF allocation of at least `size`
    /// bytes, offset zero, in the supplied NV12 layout for this call.
    pub(crate) unsafe fn submit_dmabuf_frame_borrowed(
        &mut self,
        fd: i32,
        size: usize,
        hor_stride: usize,
        ver_stride: usize,
        pts: i64,
        dts: i64,
        osd: Option<osd::Config>,
    ) -> Result<usize, Error> {
        self.submit_dmabuf_submission(DmaBufSubmission {
            fd,
            size,
            hor_stride,
            ver_stride,
            pts,
            dts,
            osd,
        })
    }

    fn submit_dmabuf_submission(&mut self, frame: DmaBufSubmission) -> Result<usize, Error> {
        if self.eos {
            return Err(Error::State("frame submitted after encoder EOS"));
        }
        validate_dmabuf_layout(self.config, frame.size, frame.hor_stride, frame.ver_stride)?;
        let input_fd = frame.fd;
        let external_rc = self.next_rate_control.take();
        let use_rate_control_registers = self
            .config
            .rate_control
            .is_some_and(|rate_control| rate_control.mode() != RateControlMode::FixedQp);
        let osd = frame.osd;
        let submitted: Result<usize, Error> = match &mut self.backend {
            Backend::H264Vepu580(encoder) => {
                if encoder.pipeline_enabled() {
                    if osd.is_some() {
                        return Err(Error::Unsupported(
                            "H.264 pipelined DMA input does not support per-frame OSD".into(),
                        ));
                    }
                    if external_rc.is_some() {
                        return Err(Error::Unsupported(
                            "H.264 pipelined DMA input does not support custom rate control".into(),
                        ));
                    }
                    let completed = encoder
                        .encode_frame_fd_pipeline(input_fd)
                        .map_err(|error| Error::Hardware(error.to_string()))?;
                    self.h264_pipeline_timing.push_back((frame.pts, frame.dts));
                    return Ok(self.queue_h264_frames(completed)?);
                }
                encoder.set_next_osd(osd);
                if let Some(rc) = external_rc.filter(|_| use_rate_control_registers) {
                    encoder.force_next_rate_control(h264e::RateControl {
                        bit_target: rc.bit_target,
                        qp_target: rc.qp_target,
                        qp_min: rc.qp_min,
                        qp_max: rc.qp_max,
                        qp_delta_row: 2,
                        qp_delta_row_i: 2,
                    });
                }
                let encoded = encoder
                    .encode_frame_fd_for_queue(input_fd)
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                let count = encoded.packets.len();
                let coded_width = self.config.width().div_ceil(16) * 16;
                let coded_height = self.config.height().div_ceil(16) * 16;
                let macroblocks = ((coded_width / 16) * (coded_height / 16)) as u32;
                let meta = PacketMeta::from_h264(&encoded, macroblocks);
                for data in encoded.packets {
                    self.packets.push_back(Packet {
                        data,
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: encoded.is_idr,
                        frame_num: encoded.frame_num,
                        poc: encoded.poc,
                        meta: meta.clone(),
                    });
                }
                Ok(count)
            }
            Backend::H264Vepu510(encoder) => {
                if osd.is_some() {
                    return Err(Error::Unsupported(
                        "RK3576 H.264 OSD is not available".to_string(),
                    ));
                }
                let encoded = if let Some(rc) = external_rc {
                    encoder.encode_next_with_rate_320x240_fd(
                        input_fd,
                        rc.bit_target,
                        rc.qp_target,
                        rc.qp_min,
                        rc.qp_max,
                    )
                } else {
                    encoder.encode_next_320x240_fd(input_fd)
                }
                .map_err(|error| Error::Hardware(error.to_string()))?;
                let (data, keyframe, frame_num, poc) = encoded;
                let meta = encoder
                    .last_packet_meta(keyframe)
                    .unwrap_or_else(|| PacketMeta::fixed_qp(keyframe, i32::from(self.config.qp)));
                self.packets.push_back(Packet {
                    data,
                    pts: frame.pts,
                    dts: frame.dts,
                    keyframe,
                    frame_num,
                    poc,
                    meta,
                });
                Ok(1)
            }
            Backend::H265Vepu580(encoder) => {
                if encoder.pipeline_enabled() {
                    if osd.is_some() {
                        return Err(Error::Unsupported(
                            "H.265 pipelined DMA input does not support per-frame OSD".into(),
                        ));
                    }
                    if external_rc.is_some() {
                        return Err(Error::Unsupported(
                            "H.265 pipelined DMA input does not support custom rate control".into(),
                        ));
                    }
                    let completed = encoder
                        .encode_frame_fd_pipeline(input_fd)
                        .map_err(|error| Error::Hardware(error.to_string()))?;
                    self.h265_pipeline_timing.push_back((frame.pts, frame.dts));
                    return Ok(self.queue_h265_frames(completed)?);
                }
                encoder.set_next_osd(osd);
                if use_rate_control_registers && external_rc.is_some() {
                    return Err(Error::Unsupported(
                        "H.265 DMA-BUF custom RC is not available".to_string(),
                    ));
                }
                let encoded = encoder
                    .encode_frame_packets_fd_for_queue(input_fd)
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                let count = encoded.packets.len();
                let meta =
                    PacketMeta::from_h265(&encoded, self.config.width(), self.config.height());
                for data in encoded.packets {
                    self.packets.push_back(Packet {
                        data,
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: encoded.is_idr,
                        frame_num: encoded.frame_num,
                        poc: encoded.frame_num as i32 * 2,
                        meta: meta.clone(),
                    });
                }
                Ok(count)
            }
            Backend::H265Vepu510(encoder) => {
                if osd.is_some() {
                    return Err(Error::Unsupported(
                        "RK3576 H.265 OSD is not available".to_string(),
                    ));
                }
                let encoded = if let Some(rc) = external_rc {
                    encoder.encode_next_with_rate_320x240_fd(
                        input_fd,
                        rc.bit_target,
                        rc.qp_target,
                        rc.qp_min,
                        rc.qp_max,
                    )
                } else {
                    encoder.encode_next_320x240_fd(input_fd)
                }
                .map_err(|error| Error::Hardware(error.to_string()))?;
                let (data, keyframe, frame_num, poc) = encoded;
                let meta = encoder
                    .last_packet_meta(keyframe)
                    .unwrap_or_else(|| PacketMeta::fixed_qp(keyframe, i32::from(self.config.qp)));
                self.packets.push_back(Packet {
                    data,
                    pts: frame.pts,
                    dts: frame.dts,
                    keyframe,
                    frame_num,
                    poc,
                    meta,
                });
                Ok(1)
            }
            Backend::Jpeg {
                encoder,
                frame_number,
            } => {
                if osd.is_some() {
                    return Err(Error::Unsupported(
                        "JPEG encoder OSD is not available".to_string(),
                    ));
                }
                if external_rc.is_some() {
                    return Err(Error::Unsupported(
                        "JPEG custom RC is not available".to_string(),
                    ));
                }
                let jpeg = jpeg_config(self.config)?;
                let encoded = encoder
                    .encode_fd_packets(input_fd, jpeg)
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                let count = encoded.packets.len();
                let meta = PacketMeta::jpeg(
                    self.config
                        .jpeg_qfactor()
                        .unwrap_or(self.config.jpeg_quality()),
                    encoder.packet_output_intra(),
                );
                for range in encoded.packets {
                    self.packets.push_back(Packet {
                        data: encoded.bytes[range].to_vec(),
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: true,
                        frame_num: *frame_number,
                        poc: *frame_number as i32,
                        meta: meta.clone(),
                    });
                }
                *frame_number = frame_number.saturating_add(1);
                Ok(count)
            }
        };
        let count = submitted?;
        self.last_motion_info = match &mut self.backend {
            Backend::H264Vepu580(encoder) if encoder.motion_info_enabled() => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            Backend::H264Vepu580(_) => None,
            Backend::H264Vepu510(encoder) => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            Backend::H265Vepu580(encoder) => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            Backend::H265Vepu510(encoder) => Some(
                encoder
                    .read_motion_info()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            _ => None,
        };
        Ok(count)
    }

    pub fn receive_packet(&mut self) -> Option<Packet> {
        self.packets.pop_front()
    }

    pub(crate) fn take_motion_info(&mut self) -> Option<Vec<u8>> {
        self.last_motion_info.take()
    }

    pub fn flush(&mut self) -> usize {
        let _ = self.flush_pending();
        self.eos = true;
        self.packets.len()
    }

    pub fn is_finished(&self) -> bool {
        self.eos && self.packets.is_empty()
    }

    pub fn reset(&mut self) -> Result<(), Error> {
        self.packets.clear();
        self.eos = false;
        self.last_motion_info = None;
        self.next_rate_control = None;
        self.h264_pipeline_timing.clear();
        self.h265_pipeline_timing.clear();
        self.backend = match self.config.codec {
            Codec::H264 => open_h264_backend(self.config)?,
            Codec::H265 => open_h265_backend_with_mode(self.config, self.mpi_compat)?,
            Codec::Mjpeg => Backend::Jpeg {
                encoder: open_jpeg()?,
                frame_number: 0,
            },
            _ => {
                return Err(Error::Unsupported(
                    "encoder backend reset is unsupported".to_string(),
                ));
            }
        };
        Ok(())
    }
}

fn open_h264_backend(config: EncoderConfig) -> Result<Backend, Error> {
    validate_h264_input(config)?;
    let profile = SocProfile::detect().map_err(|error| Error::Hardware(error.to_string()))?;
    match profile.video_encode {
        VideoEncodeHal::Vepu580 => Ok(Backend::H264Vepu580(open_h264_vepu580(config)?)),
        VideoEncodeHal::Vepu510 => Ok(Backend::H264Vepu510(open_h264_vepu510(config)?)),
        other => Err(Error::Unsupported(format!(
            "no H.264 encoder backend for {other:?}"
        ))),
    }
}

fn validate_h264_input(config: EncoderConfig) -> Result<(), Error> {
    if !matches!(config.input_format, PixelFormat::Nv12 | PixelFormat::I420) {
        return Err(Error::Unsupported(
            "H.264 encoder accepts NV12 or I420 input".to_string(),
        ));
    }
    if config.width % 2 != 0 || config.height % 2 != 0 {
        return Err(Error::InvalidInput(
            "H.264 NV12/I420 dimensions must be even".to_string(),
        ));
    }
    Ok(())
}

fn open_h264_vepu580(config: EncoderConfig) -> Result<h264e::Vepu580H264, Error> {
    let h264 = h264_encoder_config(config)?;
    h264e::Vepu580H264::open(h264).map_err(|error| Error::Hardware(error.to_string()))
}

fn h264_encoder_config(config: EncoderConfig) -> Result<H264EncoderConfig, Error> {
    let profile = match config.profile {
        0 | PROFILE_BASELINE => PROFILE_BASELINE,
        PROFILE_MAIN => PROFILE_MAIN,
        PROFILE_HIGH => PROFILE_HIGH,
        other => {
            return Err(Error::InvalidInput(format!(
                "unsupported H.264 profile {other}; expected 66, 77 or 100"
            )));
        }
    };
    let mut h264 = H264EncoderConfig::new(config.width, config.height);
    h264.profile = profile;
    h264.level_idc = config.level;
    h264.gop = config.gop.max(1);
    h264.qp_init = config.qp;
    if config.num_ref_frames != 0 {
        h264.num_ref_frames = config.num_ref_frames;
    }
    h264.fps_num = config.fps_num;
    h264.fps_den = config.fps_den;
    h264.aq_step_i = config.h264_aq_step_i;
    if let Some(deblock) = config.h264_deblock {
        h264.deblock_disable = deblock.disable;
        h264.deblock_offset_alpha = deblock.alpha_offset_div2;
        h264.deblock_offset_beta = deblock.beta_offset_div2;
    }
    if let Some(tuning) = config.h264_rc_tuning {
        h264.max_reencode_times = tuning.max_reencode_times;
        h264.super_mode = tuning.super_mode;
        h264.super_i_thd = tuning.super_i_thd;
        h264.super_p_thd = tuning.super_p_thd;
        h264.rc_priority = tuning.rc_priority;
        h264.debreath_en = tuning.debreath_enabled;
        h264.debreath_strength = tuning.debreath_strength;
        h264.drop_mode = tuning.drop_mode;
        h264.drop_threshold = tuning.drop_threshold;
        h264.drop_gap = tuning.drop_gap;
    }
    h264.entropy_coding = profile >= PROFILE_MAIN;
    h264.transform_8x8 = profile >= PROFILE_HIGH;
    if let Some(syntax) = config.h264_syntax {
        h264.entropy_coding = syntax.entropy_coding;
        h264.transform_8x8 = syntax.transform_8x8;
        h264.cabac_init_idc = syntax.cabac_init_idc;
        h264.constraint_set_force = ((syntax.constraint_set.unwrap_or(0) >> 16) & 0x3f) as u8;
        h264.constraint_set_value = (syntax.constraint_set.unwrap_or(0) & 0x3f) as u8;
        h264.base_layer_pid = syntax.base_layer_pid.unwrap_or(0);
    }
    if let Some(rate_control) = config.rate_control {
        match rate_control.mode {
            RateControlMode::FixedQp => {}
            RateControlMode::Cbr => h264.rc_mode = h264_encoder::RcMode::Cbr,
            RateControlMode::Vbr => h264.rc_mode = h264_encoder::RcMode::Vbr,
            RateControlMode::Avbr => h264.rc_mode = h264_encoder::RcMode::Avbr,
        }
        if rate_control.mode != RateControlMode::FixedQp {
            h264.bps_target = rate_control.bps_target;
            h264.bps_min = rate_control.bps_min;
            h264.bps_max = rate_control.bps_max;
            h264.qp_init = rate_control.initial_qp;
            h264.rc_qp_min = u32::from(rate_control.min_qp);
            h264.rc_qp_max = u32::from(rate_control.max_qp);
            h264.hier_qp_enabled = rate_control.hier_qp_enabled;
            h264.hier_qp_delta = rate_control.hier_qp_delta;
            h264.hier_frame_num = rate_control.hier_frame_num;
        }
    }
    if h264.rc_mode == h264_encoder::RcMode::FixedQp {
        h264.bps_target = h264_encoder::default_bps_target(
            config.width,
            config.height,
            config.fps_num / config.fps_den.max(1),
        );
    }
    if let Some(roi) = config.roi {
        h264.rois = roi
            .regions()
            .iter()
            .map(|region| H264RoiRegion {
                x: region.x,
                y: region.y,
                w: region.width,
                h: region.height,
                force_intra: region.force_intra,
                qp_absolute: region.qp_absolute,
                qp_val: region.qp,
            })
            .collect();
    }
    if let Some(gdr) = config.gdr {
        h264.gdr = true;
        h264.gdr_mode = match gdr.mode {
            GdrMode::Row => H264GdrMode::Row,
            GdrMode::Col => H264GdrMode::Col,
        };
        h264.gdr_num = gdr.refresh_blocks;
    }
    if let Some(slice_split) = config.slice_split {
        h264.split_mode = match slice_split.mode {
            SliceSplitMode::ByCtu => h264_encoder::SplitMode::ByCtu,
            SliceSplitMode::ByByte => h264_encoder::SplitMode::ByByte,
        };
        h264.split_arg = slice_split.argument;
        h264.split_out = match slice_split.output {
            SliceOutputMode::CompleteAccessUnit => 0,
            SliceOutputMode::LowDelayPackets => h264_encoder::MPP_ENC_SPLIT_OUT_LOWDELAY,
        };
    }
    h264.validate()
        .map_err(|error| Error::InvalidInput(error.to_string()))?;
    Ok(h264)
}

fn open_h264_vepu510(config: EncoderConfig) -> Result<vepu510_h264::Vepu510H264, Error> {
    if config.roi.is_some() || config.gdr.is_some() || config.slice_split.is_some() {
        return Err(Error::Unsupported(
            "RK3576 H.264 streaming ROI/GDR/slice controls are not exposed yet".to_string(),
        ));
    }
    let fixed_qp = config
        .rate_control
        .is_none_or(|rate_control| rate_control.mode == RateControlMode::FixedQp);
    if config.width != 320
        || config.height != 240
        || (fixed_qp && config.qp != 26)
        || !matches!(config.gop, 6 | 60)
        || config.fps_num != 30
        || config.fps_den != 1
        || !matches!(config.profile, 0 | 66 | 100)
        || !matches!(config.level, 0 | 30 | 40)
    {
        return Err(Error::Unsupported(format!(
            "RK3576 H.264 encoder supports only 320x240 Baseline/High GOP6 or GOP60 at \
             30 fps (fixed-QP requires QP26); got {}x{} profile {} level {} QP {} GOP {} \
             at {}/{} fps",
            config.width,
            config.height,
            config.profile,
            config.level,
            config.qp,
            config.gop,
            config.fps_num,
            config.fps_den
        )));
    }
    let mut h264 = h264_encoder_config(config)?;
    h264.constraint_set = (h264.profile >= PROFILE_HIGH).then_some(0);
    h264.num_ref_frames = if config.num_ref_frames == 0 {
        1
    } else {
        config.num_ref_frames
    };
    if h264.profile >= PROFILE_HIGH {
        h264.video_format = Some(5);
        h264.video_full_range = true;
    }
    h264.vui_log2_max_mv_length = 15;
    vepu510_h264::Vepu510H264::open_with_config(h264)
        .map_err(|error| Error::Hardware(error.to_string()))
}

fn open_h265_backend_with_mode(config: EncoderConfig, mpi_compat: bool) -> Result<Backend, Error> {
    validate_h265_input(config)?;
    let profile = SocProfile::detect().map_err(|error| Error::Hardware(error.to_string()))?;
    match profile.video_encode {
        VideoEncodeHal::Vepu580 => Ok(Backend::H265Vepu580(open_h265_vepu580(config, mpi_compat)?)),
        VideoEncodeHal::Vepu510 => Ok(Backend::H265Vepu510(open_h265_vepu510(config, mpi_compat)?)),
        other => Err(Error::Unsupported(format!(
            "no H.265 encoder backend for {other:?}"
        ))),
    }
}

fn validate_h265_input(config: EncoderConfig) -> Result<(), Error> {
    if !matches!(config.input_format, PixelFormat::Nv12 | PixelFormat::I420) {
        return Err(Error::Unsupported(
            "H.265 encoder accepts NV12 or I420 input".to_string(),
        ));
    }
    if config.profile != 0 {
        return Err(Error::Unsupported(
            "H.265 encoder profile selection is not exposed yet".to_string(),
        ));
    }
    if config.width % 2 != 0 || config.height % 2 != 0 {
        return Err(Error::InvalidInput(
            "H.265 NV12/I420 dimensions must be even".to_string(),
        ));
    }
    Ok(())
}

fn open_h265_vepu580(
    config: EncoderConfig,
    mpi_compat: bool,
) -> Result<h265e_session::Vepu580H265, Error> {
    let mut encoder = match config.rate_control {
        Some(rate_control) if rate_control.mode != RateControlMode::FixedQp => {
            let mode = match rate_control.mode {
                RateControlMode::Cbr => h265e_session::RcMode::Cbr,
                RateControlMode::Vbr => h265e_session::RcMode::Vbr,
                RateControlMode::Avbr => h265e_session::RcMode::Avbr,
                RateControlMode::FixedQp => unreachable!(),
            };
            h265e_session::Vepu580H265::open_rate(
                config.width,
                config.height,
                mode,
                rate_control.bps_target as i32,
                rate_control.bps_min as i32,
                rate_control.bps_max as i32,
                i32::from(rate_control.initial_qp),
                u32::from(rate_control.min_qp),
                u32::from(rate_control.max_qp),
            )
        }
        _ if config.gdr.is_some() => h265e_session::Vepu580H265::open_gop_short_term(
            config.width,
            config.height,
            u32::from(config.qp),
        ),
        _ => {
            h265e_session::Vepu580H265::open_gop(config.width, config.height, u32::from(config.qp))
        }
    }
    .map_err(|error| Error::Hardware(error.to_string()))?;
    encoder
        .set_gop(config.gop.max(1))
        .map_err(|error| Error::Hardware(error.to_string()))?;
    encoder.set_level(config.level());
    encoder.set_temporal_mvp(mpi_compat);
    encoder.set_syntax_options(config.h265_syntax);
    if let Some(slice_split) = config.slice_split {
        encoder
            .set_slice_split(H265SliceSplit {
                mode: match slice_split.mode {
                    SliceSplitMode::ByCtu => H265SplitMode::ByCtu,
                    SliceSplitMode::ByByte => H265SplitMode::ByByte,
                },
                arg: slice_split.argument,
                out: match slice_split.output {
                    SliceOutputMode::CompleteAccessUnit => 0,
                    SliceOutputMode::LowDelayPackets => MPP_ENC_SPLIT_OUT_LOWDELAY,
                },
            })
            .map_err(|error| Error::Hardware(error.to_string()))?;
    }
    if let Some(roi) = config.roi {
        let regions: Vec<_> = roi
            .regions()
            .iter()
            .map(|region| H265RoiRegion {
                x: region.x as u16,
                y: region.y as u16,
                w: region.width as u16,
                h: region.height as u16,
                force_intra: region.force_intra,
                qp_absolute: region.qp_absolute,
                qp_val: region.qp as i16,
            })
            .collect();
        encoder
            .set_roi_regions(&regions)
            .map_err(|error| Error::Hardware(error.to_string()))?;
    }
    if let Some(gdr) = config.gdr {
        encoder
            .enable_refresh(
                match gdr.mode {
                    GdrMode::Row => H265RefreshMode::Row,
                    GdrMode::Col => H265RefreshMode::Col,
                },
                gdr.refresh_blocks,
            )
            .map_err(|error| Error::Hardware(error.to_string()))?;
    }
    Ok(encoder)
}

fn open_h265_vepu510(
    config: EncoderConfig,
    _mpi_compat: bool,
) -> Result<vepu510_h265::Vepu510H265, Error> {
    if config.h265_syntax != H265SyntaxOptions::default() {
        return Err(Error::Unsupported(
            "RK3576 H.265 syntax tuning is not exposed yet".to_string(),
        ));
    }
    if config.roi.is_some() || config.gdr.is_some() || config.slice_split.is_some() {
        return Err(Error::Unsupported(
            "RK3576 H.265 streaming ROI/GDR/slice controls are not exposed yet".to_string(),
        ));
    }
    let fixed_qp = config
        .rate_control
        .is_none_or(|rate_control| rate_control.mode == RateControlMode::FixedQp);
    if config.width != 320
        || config.height != 240
        || (fixed_qp && config.qp != 26)
        || !matches!(config.gop, 6 | 60)
        || config.fps_num != 30
        || config.fps_den != 1
        || !matches!(config.level, 0 | 30)
    {
        return Err(Error::Unsupported(
            "RK3576 H.265 encoder supports only 320x240 Main GOP6 or GOP60 at 30 fps \
             (fixed-QP requires QP26)"
                .to_string(),
        ));
    }
    if let Some(rate) = config
        .rate_control
        .filter(|rate| rate.mode != RateControlMode::FixedQp)
    {
        let mode = match rate.mode {
            RateControlMode::Cbr => h265e_session::RcMode::Cbr,
            RateControlMode::Vbr => h265e_session::RcMode::Vbr,
            RateControlMode::Avbr => h265e_session::RcMode::Avbr,
            RateControlMode::FixedQp => unreachable!(),
        };
        let mut h265 = crate::codec::h265::encoder::EncoderConfig::new(config.width, config.height);
        h265.max_cu_size = 32;
        h265.tu_log2_max_extra = 1;
        return vepu510_h265::Vepu510H265::open_streaming(
            h265,
            false,
            mode,
            rate.bps_target as i32,
            rate.bps_min as i32,
            rate.bps_max as i32,
            i32::from(rate.initial_qp),
            u32::from(rate.min_qp),
            u32::from(rate.max_qp),
            config.gop,
        )
        .map_err(|error| Error::Hardware(error));
    }
    vepu510_h265::Vepu510H265::open().map_err(|error| Error::Hardware(error.to_string()))
}

fn open_jpeg() -> Result<jpege720::JpegEncoder, Error> {
    jpege720::JpegEncoder::open().map_err(|error| Error::Hardware(error.to_string()))
}

fn jpeg_config(config: EncoderConfig) -> Result<jpege::EncoderConfig, Error> {
    if config.jpeg_qfactor.is_none() && config.jpeg_quality > 10 {
        return Err(Error::InvalidInput(
            "JPEG quality must be in 0..=10".to_string(),
        ));
    }
    if config
        .jpeg_qfactor
        .is_some_and(|qfactor| !(1..=100).contains(&qfactor))
    {
        return Err(Error::InvalidInput(
            "JPEG q_factor must be in 1..=100".to_string(),
        ));
    }
    if config.jpeg_restart_mcus > u16::MAX as u32 {
        return Err(Error::InvalidInput(
            "JPEG restart MCU count must fit in 16 bits".to_string(),
        ));
    }
    let mut jpeg = jpege::EncoderConfig::new(config.width, config.height);
    jpeg.quality = match config.jpeg_qfactor {
        Some(qfactor) => jpege::Quality::QFactor(qfactor as u8),
        None => jpege::Quality::Quant(config.jpeg_quality as u8),
    };
    jpeg.input = jpege::InputFormat::Nv12;
    jpeg.chroma = jpege::ChromaFormat::Yuv420;
    jpeg.split_mcus = (config.jpeg_restart_mcus != 0).then_some(config.jpeg_restart_mcus as u16);
    jpeg.validate().map_err(Error::InvalidInput)?;
    Ok(jpeg)
}

fn validate_dmabuf_layout(
    config: EncoderConfig,
    size: usize,
    hor_stride: usize,
    ver_stride: usize,
) -> Result<(), Error> {
    if config.input_format != PixelFormat::Nv12 {
        return Err(Error::Unsupported(
            "DMA-BUF encoder input requires NV12".to_string(),
        ));
    }
    let (width_align, height_align) = if config.codec == Codec::Mjpeg {
        (8, 2)
    } else {
        (16, 16)
    };
    let expected_hor_stride = align_up(config.width, width_align);
    let expected_ver_stride = align_up(config.height, height_align);
    if hor_stride != expected_hor_stride || ver_stride != expected_ver_stride {
        return Err(Error::InvalidInput(format!(
            "DMA-BUF encoder input stride {}x{} does not match required {}x{}",
            hor_stride, ver_stride, expected_hor_stride, expected_ver_stride
        )));
    }
    let required = hor_stride
        .checked_mul(ver_stride)
        .and_then(|luma| luma.checked_mul(3).map(|bytes| bytes / 2))
        .ok_or_else(|| Error::InvalidInput("DMA-BUF encoder input size overflow".to_string()))?;
    if size < required {
        return Err(Error::InvalidInput(format!(
            "DMA-BUF encoder input is {} bytes, expected at least {required}",
            size
        )));
    }
    Ok(())
}

fn align_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

fn format_nv12_input<'a>(config: EncoderConfig, data: &'a [u8]) -> Result<Cow<'a, [u8]>, Error> {
    format_nv12_padded(config, data, 16, 16)
}

fn needs_visible_nv12_copy(config: EncoderConfig) -> bool {
    config.input_format == PixelFormat::Nv12 && (config.width % 16 != 0 || config.height % 16 != 0)
}

fn format_nv12_padded<'a>(
    config: EncoderConfig,
    data: &'a [u8],
    width_alignment: usize,
    height_alignment: usize,
) -> Result<Cow<'a, [u8]>, Error> {
    let visible_luma = config
        .width
        .checked_mul(config.height)
        .ok_or_else(|| Error::InvalidInput("video input size overflow".to_string()))?;
    let visible_nv12 = visible_luma
        .checked_mul(3)
        .map(|bytes| bytes / 2)
        .ok_or_else(|| Error::InvalidInput("video input size overflow".to_string()))?;
    if data.len() < visible_nv12 {
        return Err(Error::InvalidInput(format!(
            "video input is {} bytes, expected at least {visible_nv12}",
            data.len()
        )));
    }
    let coded_width = config.width.div_ceil(width_alignment) * width_alignment;
    let coded_height = config.height.div_ceil(height_alignment) * height_alignment;
    let coded_size = coded_width
        .checked_mul(coded_height)
        .and_then(|luma| luma.checked_mul(3).map(|bytes| bytes / 2))
        .ok_or_else(|| Error::InvalidInput("video coded size overflow".to_string()))?;
    if config.input_format == PixelFormat::Nv12
        && coded_width == config.width
        && coded_height == config.height
    {
        return Ok(Cow::Borrowed(&data[..visible_nv12]));
    }
    let mut output = vec![0u8; coded_size];
    for row in 0..config.height {
        let src = row * config.width;
        let dst = row * coded_width;
        output[dst..dst + config.width].copy_from_slice(&data[src..src + config.width]);
    }
    let chroma_width = config.width / 2;
    let chroma_height = config.height / 2;
    let chroma_plane = chroma_width * chroma_height;
    let uv_base = coded_width * coded_height;
    match config.input_format {
        PixelFormat::Nv12 => {
            let source_uv = visible_luma;
            for row in 0..chroma_height {
                let src = source_uv + row * config.width;
                let dst = uv_base + row * coded_width;
                output[dst..dst + chroma_width * 2]
                    .copy_from_slice(&data[src..src + chroma_width * 2]);
            }
        }
        PixelFormat::I420 => {
            let u = &data[visible_luma..visible_luma + chroma_plane];
            let v = &data[visible_luma + chroma_plane..visible_luma + chroma_plane * 2];
            for row in 0..chroma_height {
                let dst = uv_base + row * coded_width;
                for column in 0..chroma_width {
                    output[dst + column * 2] = u[row * chroma_width + column];
                    output[dst + column * 2 + 1] = v[row * chroma_width + column];
                }
            }
        }
        _ => unreachable!("encoder constructor validated the input format"),
    }
    Ok(Cow::Owned(output))
}

#[cfg(test)]
mod tests {
    use super::{
        format_nv12_input, validate_dmabuf_layout, BlockStatistics, Codec, DmaBufInput,
        EncoderConfig, GdrConfig, GdrMode, H264DeblockConfig, H264RcTuning, H264SyntaxConfig,
        H265SyntaxOptions, PacketMeta, PixelFormat, RateControlConfig, RateControlMode, RoiConfig,
        RoiRegion, SliceOutputMode, SliceSplitConfig, SliceSplitMode,
    };
    use crate::sdk::Error;
    use crate::service::dma_heap::DmaBuf;
    use std::borrow::Cow;
    use std::os::fd::{AsFd, AsRawFd};

    #[test]
    fn aligned_nv12_input_is_borrowed_without_a_frame_copy() {
        let config = EncoderConfig::new(Codec::H264, 16, 16);
        let input = vec![7u8; 16 * 16 * 3 / 2];
        let formatted = format_nv12_input(config, &input).unwrap();
        assert!(matches!(formatted, Cow::Borrowed(_)));
        assert_eq!(formatted.as_ref(), input);
    }

    #[test]
    fn padded_or_i420_input_still_materializes_the_coded_layout() {
        let nv12 = vec![7u8; 18 * 18 * 3 / 2];
        let config = EncoderConfig::new(Codec::H264, 18, 18);
        assert!(matches!(
            format_nv12_input(config, &nv12).unwrap(),
            Cow::Owned(_)
        ));

        let i420 = vec![7u8; 16 * 16 * 3 / 2];
        let config = EncoderConfig::new(Codec::H264, 16, 16).with_input_format(PixelFormat::I420);
        assert!(matches!(
            format_nv12_input(config, &i420).unwrap(),
            Cow::Owned(_)
        ));
    }

    #[test]
    fn dmabuf_layout_requires_nv12_exact_strides_and_complete_picture() {
        let config = EncoderConfig::new(Codec::H264, 1920, 1080);
        assert!(validate_dmabuf_layout(config, 1920 * 1088 * 3 / 2, 1920, 1088).is_ok());
        assert!(validate_dmabuf_layout(config, 1920 * 1088 * 3 / 2, 1920, 1080).is_err());
        assert!(validate_dmabuf_layout(config, 1920 * 1088 * 3 / 2 - 1, 1920, 1088).is_err());

        let i420 = config.with_input_format(PixelFormat::I420);
        assert!(validate_dmabuf_layout(i420, 1920 * 1088 * 3 / 2, 1920, 1088).is_err());
    }

    #[test]
    fn dmabuf_input_borrows_the_fd_without_mapping_it() {
        let file = std::fs::File::open("/dev/null").unwrap();
        let input = DmaBufInput::new(file.as_fd(), 4096, 1920, 1088).unwrap();
        assert_eq!(input.into_submission().fd, file.as_raw_fd());
    }

    #[test]
    fn dmabuf_input_carries_osd_only_when_requested() {
        let file = std::fs::File::open("/dev/null").unwrap();
        let input = DmaBufInput::new(file.as_fd(), 4096, 1920, 1088).unwrap();
        assert!(input.into_submission().osd.is_none());

        let osd = crate::hal::osd::Config {
            buffer_fd: file.as_raw_fd(),
            buffer_size: 256,
            regions: Vec::new(),
            user_palette: None,
        };
        let input = DmaBufInput::new(file.as_fd(), 4096, 1920, 1088)
            .unwrap()
            .with_osd(osd);
        assert!(input.into_submission().osd.is_some());
    }

    #[test]
    fn h264_cabac_init_idc_is_bounded_to_the_three_models() {
        let syntax = H264SyntaxConfig::new(true, false).with_cabac_init_idc(2);
        assert_eq!(syntax.cabac_init_idc(), 2);
        let invalid = EncoderConfig::new(Codec::H264, 320, 240)
            .with_profile(77)
            .with_h264_syntax(H264SyntaxConfig::new(true, false).with_cabac_init_idc(3));
        assert!(invalid.validate_advanced_controls().is_err());
    }

    #[test]
    fn h264_constraint_set_uses_only_force_and_value_fields() {
        let syntax = H264SyntaxConfig::new(true, false).with_constraint_set(0x003f_0025);
        assert_eq!(syntax.constraint_set(), Some(0x003f_0025));

        for invalid in [0x0040_0000, 0x0000_0040, 0x0100_0000] {
            let config = EncoderConfig::new(Codec::H264, 320, 240)
                .with_profile(77)
                .with_h264_syntax(H264SyntaxConfig::new(true, false).with_constraint_set(invalid));
            assert!(config.validate_advanced_controls().is_err());
        }
    }

    #[test]
    fn h264_base_layer_pid_is_bounded_to_the_six_bit_priority_space() {
        let syntax = H264SyntaxConfig::new(true, false).with_base_layer_pid(56);
        assert_eq!(syntax.base_layer_pid(), Some(56));
        let invalid = EncoderConfig::new(Codec::H264, 320, 240)
            .with_h264_syntax(H264SyntaxConfig::new(true, false).with_base_layer_pid(57));
        assert!(invalid.validate_advanced_controls().is_err());
    }

    #[test]
    #[ignore = "requires an RK3588 VEPU580 and /dev/mpp_service"]
    fn h264_base_layer_pid_reaches_the_type14_prefix() {
        let syntax = H264SyntaxConfig::new(true, false).with_base_layer_pid(5);
        let config = EncoderConfig::new(Codec::H264, 320, 240)
            .with_profile(77)
            .with_gop(60)
            .with_h264_syntax(syntax);
        let mut encoder = super::Encoder::new(config).unwrap();
        encoder
            .force_next_picture_layout(0, 0, false, Some(3), 0, 0, None, None)
            .unwrap();
        let frame = crate::hal::h264e::probe_nv12(320, 240);
        encoder
            .submit_frame(super::InputFrame::new(&frame))
            .unwrap();
        let mut stream = Vec::new();
        while let Some(packet) = encoder.receive_packet() {
            stream.extend_from_slice(packet.data());
        }
        let expected = [0, 0, 0, 1, 0x6e, 0xc8, 0x80, 0x67, 0x20];
        assert!(
            stream
                .windows(expected.len())
                .any(|bytes| bytes == expected),
            "base_layer_pid 5 + temporal_id 3 must produce priority_id 8"
        );
    }

    #[test]
    #[ignore = "requires an RK3588 VEPU580 and /dev/mpp_service"]
    fn dmabuf_osd_hw_matches_copy() {
        fn encode(codec: Codec, direct: bool, with_osd: bool) -> Vec<u8> {
            let width = 320;
            let height = 240;
            let mut config = EncoderConfig::new(codec, width, height);
            config.gop = 60;
            let mut encoder = super::Encoder::new(config).unwrap();

            let frame = crate::hal::h264e::probe_nv12(width, height);
            let coded_bytes = width * height * 3 / 2;
            let mut input = DmaBuf::alloc("system", coded_bytes, false).unwrap();
            input.sync_write_begin().unwrap();
            input.as_mut_slice()[..frame.len()].copy_from_slice(&frame);
            input.sync_write_end().unwrap();

            let osd_buffer = with_osd.then(|| {
                let mut buffer = DmaBuf::alloc("system", 256, false).unwrap();
                buffer.sync_write_begin().unwrap();
                buffer.as_mut_slice().fill(0x5a);
                buffer.sync_write_end().unwrap();
                buffer
            });
            let osd = osd_buffer.as_ref().map(|buffer| crate::hal::osd::Config {
                buffer_fd: buffer.fd(),
                buffer_size: buffer.len(),
                regions: vec![crate::hal::osd::Region {
                    enable: 1,
                    inverse: 1,
                    start_mb_x: 2,
                    start_mb_y: 2,
                    num_mb_x: 1,
                    num_mb_y: 1,
                    buffer_offset: 0,
                }],
                user_palette: None,
            });

            if direct {
                let input = DmaBufInput::new(input.as_fd(), coded_bytes, width, height).unwrap();
                encoder
                    .submit_dmabuf_frame(match osd {
                        Some(osd) => input.with_osd(osd),
                        None => input,
                    })
                    .unwrap();
            } else {
                encoder
                    .submit_frame(super::InputFrame::new(&frame).with_osd(osd))
                    .unwrap();
            }

            let mut stream = Vec::new();
            while let Some(packet) = encoder.receive_packet() {
                stream.extend_from_slice(packet.data());
            }
            stream
        }

        for codec in [Codec::H264, Codec::H265] {
            let copied = encode(codec, false, false);
            let direct = encode(codec, true, false);
            assert_eq!(
                direct, copied,
                "{codec:?} dma-buf input differs from copied input"
            );

            let copied = encode(codec, false, true);
            let direct = encode(codec, true, true);
            assert_eq!(
                direct, copied,
                "{codec:?} dma-buf OSD differs from copied-input OSD"
            );
        }
    }

    #[test]
    fn vepu510_packet_meta_uses_the_status_histogram_and_sse_layout() {
        let mut status = [0u32; 148];
        status[1] = 0x2222_1111;
        status[2] = 0x3333_4444;
        status[32] = 1;
        status[33] = 2;
        status[34] = 3;
        status[35] = 4;
        status[36] = 5;
        status[37] = 6;
        status[38] = 7;
        status[39] = 8;

        let meta = PacketMeta::from_vepu510_status(&status, true, 26, 24);
        assert_eq!(meta.sse, Some((0x3333_4444_u64 << 16) | 0x2222));
        assert_eq!(
            meta.block_statistics,
            Some(BlockStatistics {
                inter64: 1,
                inter32: 2,
                inter16: 3,
                inter8: 4,
                intra32: 5,
                intra16: 6,
                intra8: 7,
                intra4: 8,
            })
        );
        assert_eq!(meta.output_pskip, Some(false));
        assert_eq!(meta.output_intra, Some(true));
        assert_eq!((meta.start_qp, meta.average_qp), (Some(26), Some(24)));
        assert_eq!(meta.rt_bits, None);

        let empty = PacketMeta::from_vepu510_status(&[0; 148], false, 26, 26);
        assert_eq!(empty.output_pskip, Some(true));
    }

    #[test]
    fn encoder_builder_keeps_public_configuration() {
        let aq_step_i = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let h264_tuning = H264RcTuning::new()
            .with_max_reencode_times(0)
            .with_super_frame(2, 10_000, 1_000, 1)
            .with_debreath(true, 20)
            .with_drop_policy(1, 12_000, 3);
        let rate_control = RateControlConfig::vbr(2_000_000, 1_500_000, 2_500_000)
            .with_initial_qp(26)
            .with_qp_range(10, 40)
            .with_hierarchical_qp([1, 2, 3, 4], [1, 1, 1, 1]);
        let deblock = H264DeblockConfig::new(1, -2, 3);
        let syntax = H264SyntaxConfig::new(false, false);
        let config = EncoderConfig::new(Codec::H264, 320, 240)
            .with_input_format(PixelFormat::I420)
            .with_profile(77)
            .with_level(31)
            .with_gop(30)
            .with_qp(30)
            .with_num_ref_frames(2)
            .with_fps(25, 1)
            .with_jpeg_quality(7)
            .with_jpeg_qfactor(80)
            .with_jpeg_restart_mcus(16)
            .with_h264_aq_step_i(aq_step_i)
            .with_h264_deblock(deblock)
            .with_h264_syntax(syntax)
            .with_h264_rc_tuning(h264_tuning)
            .with_rate_control(rate_control);
        assert_eq!(config.codec(), Codec::H264);
        assert_eq!(config.input_format(), PixelFormat::I420);
        assert_eq!(config.profile(), 77);
        assert_eq!(config.level(), 31);
        assert_eq!(config.gop(), 30);
        assert_eq!(config.qp(), 30);
        assert_eq!(config.num_ref_frames(), 2);
        assert_eq!(config.fps(), (25, 1));
        assert_eq!(config.jpeg_quality(), 7);
        assert_eq!(config.jpeg_qfactor(), Some(80));
        assert_eq!(config.jpeg_restart_mcus(), 16);
        assert_eq!(config.h264_aq_step_i(), Some(aq_step_i));
        assert_eq!(config.h264_rc_tuning(), Some(h264_tuning));
        assert_eq!(config.h264_deblock(), Some(deblock));
        assert_eq!(config.h264_syntax(), Some(syntax));
        assert_eq!(config.rate_control(), Some(rate_control));
        assert!(rate_control.hier_qp_enabled());
        assert_eq!(rate_control.hier_qp_delta(), [1, 2, 3, 4]);
        assert_eq!(rate_control.hier_frame_num(), [1, 1, 1, 1]);
        assert_eq!(
            (
                deblock.disable(),
                deblock.alpha_offset_div2(),
                deblock.beta_offset_div2(),
            ),
            (1, -2, 3)
        );
        assert!(!syntax.entropy_coding());
        assert!(!syntax.transform_8x8());
        assert!(config.validate_advanced_controls().is_ok());
        let invalid = EncoderConfig::new(Codec::H264, 320, 240)
            .with_h264_deblock(H264DeblockConfig::new(0, 7, 0));
        assert!(matches!(
            invalid.validate_advanced_controls(),
            Err(Error::InvalidInput(_))
        ));
    }

    #[test]
    fn rate_control_modes_match_the_c_contract() {
        for mode in [
            RateControlMode::FixedQp,
            RateControlMode::Cbr,
            RateControlMode::Vbr,
            RateControlMode::Avbr,
        ] {
            assert_eq!(RateControlMode::from_c_value(mode.c_value()), Some(mode));
        }
        assert_eq!(RateControlMode::from_c_value(4), None);
    }

    #[test]
    fn h265_syntax_is_typed_and_validated() {
        let syntax = H265SyntaxOptions {
            sao_luma_disable: true,
            sao_chroma_disable: true,
            deblock_disable: true,
            deblock_beta_offset_div2: 3,
            deblock_tc_offset_div2: -2,
            loop_filter_across_slices: true,
            loop_filter_across_tiles: false,
            auto_tile: true,
        };
        let config = EncoderConfig::new(Codec::H265, 320, 240).with_h265_syntax(syntax);
        assert_eq!(config.h265_syntax(), syntax);
        assert!(config.validate_advanced_controls().is_ok());

        let invalid =
            EncoderConfig::new(Codec::H265, 320, 240).with_h265_syntax(H265SyntaxOptions {
                deblock_beta_offset_div2: 7,
                ..H265SyntaxOptions::default()
            });
        assert!(matches!(
            invalid.validate_advanced_controls(),
            Err(Error::InvalidInput(_))
        ));
    }

    #[test]
    fn rate_control_rejects_invalid_bounds() {
        let bad_bitrate = RateControlConfig::cbr(1_000_000, 2_000_000, 3_000_000).validate();
        assert!(matches!(bad_bitrate, Err(Error::InvalidInput(_))));

        let bad_qp = RateControlConfig::cbr(2_000_000, 1_500_000, 2_500_000)
            .with_initial_qp(52)
            .validate();
        assert!(matches!(bad_qp, Err(Error::InvalidInput(_))));
    }

    #[test]
    fn advanced_controls_are_typed_and_stored_in_the_builder() {
        let roi =
            RoiConfig::new(&[RoiRegion::new(16, 32, 128, 96, -4).with_force_intra(true)]).unwrap();
        let slice = SliceSplitConfig::new(SliceSplitMode::ByCtu, 96)
            .with_output(SliceOutputMode::LowDelayPackets);
        let config = EncoderConfig::new(Codec::H264, 320, 240)
            .with_roi(roi)
            .with_slice_split(slice);
        assert_eq!(config.roi(), Some(roi));
        assert_eq!(config.slice_split(), Some(slice));
        assert_eq!(config.roi().unwrap().regions()[0].qp(), -4);
        assert!(config.roi().unwrap().regions()[0].forces_intra());
    }

    #[test]
    fn roi_and_gdr_require_an_explicit_supported_combination() {
        let roi = RoiConfig::new(&[RoiRegion::new(0, 0, 64, 64, -2)]).unwrap();
        let config = EncoderConfig::new(Codec::H264, 320, 240)
            .with_roi(roi)
            .with_gdr(GdrConfig::new(GdrMode::Row, 2));
        assert!(matches!(
            config.validate_advanced_controls(),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn roi_count_is_bounded_by_the_reference_limit() {
        let regions = [RoiRegion::new(0, 0, 16, 16, 0); 9];
        assert!(matches!(
            RoiConfig::new(&regions),
            Err(Error::InvalidInput(_))
        ));
    }
}
