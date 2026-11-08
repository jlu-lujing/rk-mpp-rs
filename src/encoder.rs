//! Stable synchronous encoder APIs.
//!
//! The base configuration selects fixed-QP H.264/H.265 or JPEG. VEPU580 also
//! accepts CBR/VBR/AVBR through [`RateControlConfig`]. Input frames use tightly
//! packed visible dimensions; this layer pads them to the encoder's coded
//! geometry.

use std::collections::VecDeque;

use crate::codec::h264::encoder::{
    self as h264_encoder, EncoderConfig as H264EncoderConfig, PROFILE_BASELINE, PROFILE_HIGH,
    PROFILE_MAIN,
};
use crate::codec::h264::encoder::{GdrMode as H264GdrMode, RoiRegion as H264RoiRegion};
use crate::codec::h265::encoder::{
    SliceSplit as H265SliceSplit, SplitMode as H265SplitMode, MPP_ENC_SPLIT_OUT_LOWDELAY,
};
use crate::codec::h265::roi::RoiRegion as H265RoiRegion;
use crate::hal::h265e::RefreshMode as H265RefreshMode;
use crate::hal::{h264e, h265e_session, jpege, jpege720, vepu510_h264, vepu510_h265};
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

    pub const fn with_initial_qp(mut self, qp: u8) -> Self {
        self.initial_qp = qp;
        self
    }

    pub const fn with_qp_range(mut self, min_qp: u8, max_qp: u8) -> Self {
        self.min_qp = min_qp;
        self.max_qp = max_qp;
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
    fps_num: u32,
    fps_den: u32,
    jpeg_quality: u32,
    jpeg_restart_mcus: u32,
    rate_control: Option<RateControlConfig>,
    roi: Option<RoiConfig>,
    gdr: Option<GdrConfig>,
    slice_split: Option<SliceSplitConfig>,
}

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
            fps_num: 30,
            fps_den: 1,
            jpeg_quality: 10,
            jpeg_restart_mcus: 0,
            rate_control: None,
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

    pub const fn fps(self) -> (u32, u32) {
        (self.fps_num, self.fps_den)
    }

    pub const fn jpeg_quality(self) -> u32 {
        self.jpeg_quality
    }

    pub const fn jpeg_restart_mcus(self) -> u32 {
        self.jpeg_restart_mcus
    }

    pub const fn rate_control(self) -> Option<RateControlConfig> {
        self.rate_control
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

    pub const fn with_fps(mut self, fps_num: u32, fps_den: u32) -> Self {
        self.fps_num = fps_num;
        self.fps_den = fps_den;
        self
    }

    pub const fn with_jpeg_quality(mut self, quality: u32) -> Self {
        self.jpeg_quality = quality;
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
        Ok(())
    }
}

/// One copied frame submission. Pointer data is fully consumed before return.
#[derive(Debug, Clone, Copy)]
pub struct InputFrame<'a> {
    pub data: &'a [u8],
    pub pts: i64,
    pub dts: i64,
}

impl<'a> InputFrame<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pts: 0,
            dts: 0,
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

/// Persistent encoder session. Frames are encoded synchronously into an owned
/// packet queue, so input storage can be reused as soon as submit returns.
pub struct Encoder {
    config: EncoderConfig,
    backend: Backend,
    packets: VecDeque<Packet>,
    eos: bool,
}

impl Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self, Error> {
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
            Codec::H265 => open_h265_backend(config)?,
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
        })
    }

    pub const fn config(&self) -> EncoderConfig {
        self.config
    }

    pub fn submit_frame(&mut self, frame: InputFrame<'_>) -> Result<usize, Error> {
        if self.eos {
            return Err(Error::State("frame submitted after encoder EOS"));
        }
        match &mut self.backend {
            Backend::H264Vepu580(encoder) => {
                let input = format_nv12_input(self.config, frame.data)?;
                let encoded = encoder
                    .encode_frame(&input)
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                let count = encoded.packets.len();
                for data in encoded.packets {
                    self.packets.push_back(Packet {
                        data,
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: encoded.is_idr,
                        frame_num: encoded.frame_num,
                        poc: encoded.poc,
                    });
                }
                Ok(count)
            }
            Backend::H264Vepu510(encoder) => {
                let input = format_nv12_input(self.config, frame.data)?;
                let (data, keyframe, frame_num, poc) = encoder
                    .encode_fixed_qp_next_320x240(&input)
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                self.packets.push_back(Packet {
                    data,
                    pts: frame.pts,
                    dts: frame.dts,
                    keyframe,
                    frame_num,
                    poc,
                });
                Ok(1)
            }
            Backend::H265Vepu580(encoder) => {
                let input = format_nv12_input(self.config, frame.data)?;
                let encoded = encoder
                    .encode_frame_packets(&input)
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                let count = encoded.packets.len();
                for data in encoded.packets {
                    self.packets.push_back(Packet {
                        data,
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: encoded.is_idr,
                        frame_num: encoded.frame_num,
                        poc: encoded.frame_num as i32 * 2,
                    });
                }
                Ok(count)
            }
            Backend::H265Vepu510(encoder) => {
                let input = format_nv12_input(self.config, frame.data)?;
                let (data, keyframe, frame_num, poc) = encoder
                    .encode_fixed_qp_next_320x240(&input)
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                self.packets.push_back(Packet {
                    data,
                    pts: frame.pts,
                    dts: frame.dts,
                    keyframe,
                    frame_num,
                    poc,
                });
                Ok(1)
            }
            Backend::Jpeg {
                encoder,
                frame_number,
            } => {
                let input = format_nv12_padded(self.config, frame.data, 8, 2)?;
                let jpeg = jpeg_config(self.config)?;
                let encoded = encoder
                    .encode_packets(&input, jpeg)
                    .map_err(|error| Error::Hardware(error.to_string()))?;
                let count = encoded.packets.len();
                for range in encoded.packets {
                    self.packets.push_back(Packet {
                        data: encoded.bytes[range].to_vec(),
                        pts: frame.pts,
                        dts: frame.dts,
                        keyframe: true,
                        frame_num: *frame_number,
                        poc: *frame_number as i32,
                    });
                }
                *frame_number = frame_number.saturating_add(1);
                Ok(count)
            }
        }
    }

    pub fn receive_packet(&mut self) -> Option<Packet> {
        self.packets.pop_front()
    }

    pub fn flush(&mut self) -> usize {
        self.eos = true;
        self.packets.len()
    }

    pub fn is_finished(&self) -> bool {
        self.eos && self.packets.is_empty()
    }

    pub fn reset(&mut self) -> Result<(), Error> {
        self.packets.clear();
        self.eos = false;
        self.backend = match self.config.codec {
            Codec::H264 => open_h264_backend(self.config)?,
            Codec::H265 => open_h265_backend(self.config)?,
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
    h264.fps_num = config.fps_num;
    h264.fps_den = config.fps_den;
    h264.entropy_coding = profile >= PROFILE_MAIN;
    h264.transform_8x8 = profile >= PROFILE_HIGH;
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
    h264e::Vepu580H264::open(h264).map_err(|error| Error::Hardware(error.to_string()))
}

fn open_h264_vepu510(config: EncoderConfig) -> Result<vepu510_h264::Vepu510H264, Error> {
    if config.roi.is_some() || config.gdr.is_some() || config.slice_split.is_some() {
        return Err(Error::Unsupported(
            "RK3576 H.264 streaming ROI/GDR/slice controls are not exposed yet".to_string(),
        ));
    }
    if config
        .rate_control
        .is_some_and(|rate_control| rate_control.mode != RateControlMode::FixedQp)
    {
        return Err(Error::Unsupported(
            "RK3576 H.264 streaming rate control is not exposed yet".to_string(),
        ));
    }
    if config.width != 320
        || config.height != 240
        || config.qp != 26
        || config.gop != 6
        || config.fps_num != 30
        || config.fps_den != 1
        || !matches!(config.profile, 0 | 66)
        || !matches!(config.level, 0 | 30)
    {
        return Err(Error::Unsupported(
            "RK3576 H.264 encoder currently supports only 320x240 Baseline QP26 \
             GOP6 at 30 fps"
                .to_string(),
        ));
    }
    vepu510_h264::Vepu510H264::open().map_err(|error| Error::Hardware(error.to_string()))
}

fn open_h265_backend(config: EncoderConfig) -> Result<Backend, Error> {
    validate_h265_input(config)?;
    let profile = SocProfile::detect().map_err(|error| Error::Hardware(error.to_string()))?;
    match profile.video_encode {
        VideoEncodeHal::Vepu580 => Ok(Backend::H265Vepu580(open_h265_vepu580(config)?)),
        VideoEncodeHal::Vepu510 => Ok(Backend::H265Vepu510(open_h265_vepu510(config)?)),
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

fn open_h265_vepu580(config: EncoderConfig) -> Result<h265e_session::Vepu580H265, Error> {
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

fn open_h265_vepu510(config: EncoderConfig) -> Result<vepu510_h265::Vepu510H265, Error> {
    if config.roi.is_some() || config.gdr.is_some() || config.slice_split.is_some() {
        return Err(Error::Unsupported(
            "RK3576 H.265 streaming ROI/GDR/slice controls are not exposed yet".to_string(),
        ));
    }
    if config
        .rate_control
        .is_some_and(|rate_control| rate_control.mode != RateControlMode::FixedQp)
    {
        return Err(Error::Unsupported(
            "RK3576 H.265 streaming rate control is not exposed yet".to_string(),
        ));
    }
    if config.width != 320
        || config.height != 240
        || config.qp != 26
        || config.gop != 6
        || config.fps_num != 30
        || config.fps_den != 1
        || !matches!(config.level, 0 | 30)
    {
        return Err(Error::Unsupported(
            "RK3576 H.265 encoder currently supports only 320x240 Main QP26 \
             GOP6 at 30 fps"
                .to_string(),
        ));
    }
    vepu510_h265::Vepu510H265::open().map_err(|error| Error::Hardware(error.to_string()))
}

fn open_jpeg() -> Result<jpege720::JpegEncoder, Error> {
    jpege720::JpegEncoder::open().map_err(|error| Error::Hardware(error.to_string()))
}

fn jpeg_config(config: EncoderConfig) -> Result<jpege::EncoderConfig, Error> {
    if config.jpeg_quality > 10 {
        return Err(Error::InvalidInput(
            "JPEG quality must be in 0..=10".to_string(),
        ));
    }
    if config.jpeg_restart_mcus > u16::MAX as u32 {
        return Err(Error::InvalidInput(
            "JPEG restart MCU count must fit in 16 bits".to_string(),
        ));
    }
    let mut jpeg = jpege::EncoderConfig::new(config.width, config.height);
    jpeg.quality = jpege::Quality::Quant(config.jpeg_quality as u8);
    jpeg.input = jpege::InputFormat::Nv12;
    jpeg.chroma = jpege::ChromaFormat::Yuv420;
    jpeg.split_mcus = (config.jpeg_restart_mcus != 0).then_some(config.jpeg_restart_mcus as u16);
    jpeg.validate().map_err(Error::InvalidInput)?;
    Ok(jpeg)
}

fn format_nv12_input(config: EncoderConfig, data: &[u8]) -> Result<Vec<u8>, Error> {
    format_nv12_padded(config, data, 16, 16)
}

fn format_nv12_padded(
    config: EncoderConfig,
    data: &[u8],
    width_alignment: usize,
    height_alignment: usize,
) -> Result<Vec<u8>, Error> {
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
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{
        Codec, EncoderConfig, GdrConfig, GdrMode, PixelFormat, RateControlConfig, RateControlMode,
        RoiConfig, RoiRegion, SliceOutputMode, SliceSplitConfig, SliceSplitMode,
    };
    use crate::sdk::Error;

    #[test]
    fn encoder_builder_keeps_public_configuration() {
        let rate_control = RateControlConfig::vbr(2_000_000, 1_500_000, 2_500_000)
            .with_initial_qp(26)
            .with_qp_range(10, 40);
        let config = EncoderConfig::new(Codec::H264, 320, 240)
            .with_input_format(PixelFormat::I420)
            .with_profile(77)
            .with_level(31)
            .with_gop(30)
            .with_qp(30)
            .with_fps(25, 1)
            .with_jpeg_quality(7)
            .with_jpeg_restart_mcus(16)
            .with_rate_control(rate_control);
        assert_eq!(config.codec(), Codec::H264);
        assert_eq!(config.input_format(), PixelFormat::I420);
        assert_eq!(config.profile(), 77);
        assert_eq!(config.level(), 31);
        assert_eq!(config.gop(), 30);
        assert_eq!(config.qp(), 30);
        assert_eq!(config.fps(), (25, 1));
        assert_eq!(config.jpeg_quality(), 7);
        assert_eq!(config.jpeg_restart_mcus(), 16);
        assert_eq!(config.rate_control(), Some(rate_control));
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
