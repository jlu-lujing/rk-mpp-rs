//! Stable synchronous encoder APIs.
//!
//! The first backend is fixed-QP VEPU580 H.264. Input frames use tightly packed
//! visible dimensions; this layer pads them to the encoder's coded geometry.

use std::collections::VecDeque;

use crate::codec::h264::encoder::{
    self as h264_encoder, EncoderConfig as H264EncoderConfig, PROFILE_BASELINE, PROFILE_HIGH,
    PROFILE_MAIN,
};
use crate::hal::{h264e, h265e_session, vepu510_h264, vepu510_h265};
use crate::sdk::{Codec, Error, PixelFormat};
use crate::service::soc::{SocProfile, VideoEncodeHal};

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
        let backend = match config.codec {
            Codec::H264 => open_h264_backend(config)?,
            Codec::H265 => open_h265_backend(config)?,
            Codec::Mjpeg | Codec::Vp9 | Codec::Av1 => {
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
    h264.bps_target = h264_encoder::default_bps_target(
        config.width,
        config.height,
        config.fps_num / config.fps_den.max(1),
    );
    h264.validate()
        .map_err(|error| Error::InvalidInput(error.to_string()))?;
    h264e::Vepu580H264::open(h264).map_err(|error| Error::Hardware(error.to_string()))
}

fn open_h264_vepu510(config: EncoderConfig) -> Result<vepu510_h264::Vepu510H264, Error> {
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
    let mut encoder =
        h265e_session::Vepu580H265::open_gop(config.width, config.height, u32::from(config.qp))
            .map_err(|error| Error::Hardware(error.to_string()))?;
    encoder
        .set_gop(config.gop.max(1))
        .map_err(|error| Error::Hardware(error.to_string()))?;
    Ok(encoder)
}

fn open_h265_vepu510(config: EncoderConfig) -> Result<vepu510_h265::Vepu510H265, Error> {
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

fn format_nv12_input(config: EncoderConfig, data: &[u8]) -> Result<Vec<u8>, Error> {
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
    let coded_width = config.width.div_ceil(16) * 16;
    let coded_height = config.height.div_ceil(16) * 16;
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
    use super::{Codec, EncoderConfig, PixelFormat};

    #[test]
    fn encoder_builder_keeps_public_configuration() {
        let config = EncoderConfig::new(Codec::H264, 320, 240)
            .with_input_format(PixelFormat::I420)
            .with_profile(77)
            .with_level(31)
            .with_gop(30)
            .with_qp(30)
            .with_fps(25, 1);
        assert_eq!(config.codec(), Codec::H264);
        assert_eq!(config.input_format(), PixelFormat::I420);
        assert_eq!(config.profile(), 77);
        assert_eq!(config.level(), 31);
        assert_eq!(config.gop(), 30);
        assert_eq!(config.qp(), 30);
        assert_eq!(config.fps(), (25, 1));
    }
}
