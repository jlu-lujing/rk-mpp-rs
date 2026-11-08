//! Stable, streaming-oriented codec APIs for library consumers.
//!
//! Unlike the historical JPEG helper in the crate root, this surface accepts
//! in-memory packets, keeps a codec session alive, and returns owned frames
//! with explicit plane metadata.

use std::collections::VecDeque;
use std::fmt;
use std::io;

use crate::codec::av1;
use crate::codec::h264::{
    self,
    decoder::{picture_is_output as h264_picture_is_output, split_access_units},
};
use crate::codec::hevc;
use crate::codec::jpeg::{parser as jpeg_parser, tables as jpeg_tables};
use crate::codec::vp9;
use crate::hal::hevc::ChromaFormat;
use crate::hal::jpegd::{self as jpegd, JpegDownscale};
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{SocProfile, VideoDecodeHal};
use crate::service::{MppService, VPU_CLIENT_JPEG_DEC};

pub const API_VERSION_MAJOR: u32 = 1;
pub const API_VERSION_MINOR: u32 = 3;
pub const API_VERSION: u32 = (API_VERSION_MAJOR << 16) | API_VERSION_MINOR;

const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;

/// Codec selected when a decoder session is created.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    H264,
    H265,
    Mjpeg,
    Vp9,
    Av1,
}

impl Codec {
    pub const fn c_value(self) -> u32 {
        match self {
            Self::H264 => 1,
            Self::H265 => 3,
            Self::Mjpeg => 2,
            Self::Vp9 => 4,
            Self::Av1 => 5,
        }
    }

    pub const fn from_c_value(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::H264),
            2 => Some(Self::Mjpeg),
            3 => Some(Self::H265),
            4 => Some(Self::Vp9),
            5 => Some(Self::Av1),
            _ => None,
        }
    }
}

/// Pixel layout returned by a decoder.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    Nv12,
    I420,
    /// Planar 4:2:0 with little-endian 16-bit samples.
    I420P10,
}

impl PixelFormat {
    pub const fn c_value(self) -> u32 {
        match self {
            Self::Nv12 => 1,
            Self::I420 => 2,
            Self::I420P10 => 3,
        }
    }

    pub const fn from_c_value(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Nv12),
            2 => Some(Self::I420),
            3 => Some(Self::I420P10),
            _ => None,
        }
    }
}

/// Decoder construction options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecoderConfig {
    codec: Codec,
    output_format: PixelFormat,
    max_reorder_frames: u32,
}

impl DecoderConfig {
    pub const fn new(codec: Codec) -> Self {
        Self {
            codec,
            output_format: PixelFormat::Nv12,
            max_reorder_frames: 0,
        }
    }

    pub const fn codec(self) -> Codec {
        self.codec
    }

    pub const fn output_format(self) -> PixelFormat {
        self.output_format
    }

    pub const fn with_output_format(mut self, format: PixelFormat) -> Self {
        self.output_format = format;
        self
    }

    pub const fn max_reorder_frames(self) -> u32 {
        self.max_reorder_frames
    }

    pub const fn with_max_reorder_frames(mut self, frames: u32) -> Self {
        self.max_reorder_frames = frames;
        self
    }
}

/// Per-packet flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PacketFlags(u32);

impl PacketFlags {
    pub const EOS: Self = Self(1 << 0);
    pub const COMPLETE_ACCESS_UNIT: Self = Self(1 << 1);
    const KNOWN: u32 = Self::EOS.0 | Self::COMPLETE_ACCESS_UNIT.0;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    pub const fn with_eos(self) -> Self {
        Self(self.0 | Self::EOS.0)
    }

    pub const fn with_complete_access_unit(self) -> Self {
        Self(self.0 | Self::COMPLETE_ACCESS_UNIT.0)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits & !Self::KNOWN == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }
}

/// Borrowed encoded packet passed to a decoder.
#[derive(Debug)]
pub struct Packet<'a> {
    data: &'a [u8],
    pts: i64,
    dts: i64,
    flags: PacketFlags,
}

impl<'a> Packet<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pts: 0,
            dts: 0,
            flags: PacketFlags::empty(),
        }
    }

    pub const fn data(&self) -> &'a [u8] {
        self.data
    }

    pub const fn pts(&self) -> i64 {
        self.pts
    }

    pub const fn dts(&self) -> i64 {
        self.dts
    }

    pub const fn flags(&self) -> PacketFlags {
        self.flags
    }

    pub const fn with_pts(mut self, pts: i64) -> Self {
        self.pts = pts;
        self
    }

    pub const fn with_dts(mut self, dts: i64) -> Self {
        self.dts = dts;
        self
    }

    pub const fn with_flags(mut self, flags: PacketFlags) -> Self {
        self.flags = flags;
        self
    }
}

/// One output plane. The borrow is valid for the lifetime of `Frame`.
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct PlaneLayout {
    offset: usize,
    len: usize,
    stride: usize,
}

/// One decoded frame, including media timestamps and codec metadata.
#[derive(Debug, Clone)]
pub struct Frame {
    width: usize,
    height: usize,
    format: PixelFormat,
    pts: i64,
    dts: i64,
    poc: i32,
    err_info: u32,
    storage: Vec<u8>,
    planes: [PlaneLayout; 3],
}

impl Frame {
    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn height(&self) -> usize {
        self.height
    }

    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    pub const fn pts(&self) -> i64 {
        self.pts
    }

    pub const fn dts(&self) -> i64 {
        self.dts
    }

    pub const fn poc(&self) -> i32 {
        self.poc
    }

    pub const fn err_info(&self) -> u32 {
        self.err_info
    }

    pub fn data(&self) -> &[u8] {
        &self.storage
    }

    pub fn plane(&self, index: usize) -> Option<Plane<'_>> {
        let plane = *self.planes.get(index)?;
        (plane.len != 0).then(|| Plane {
            data: &self.storage[plane.offset..plane.offset + plane.len],
            stride: plane.stride,
        })
    }

    fn from_h264(
        picture: h264::Picture,
        format: PixelFormat,
        pts: i64,
        dts: i64,
    ) -> Result<Self, Error> {
        let width = picture.width;
        let height = picture.height;
        let storage = match format {
            PixelFormat::Nv12 => picture
                .nv12
                .ok_or_else(|| Error::State("H.264 decoder returned planar data in NV12 mode"))?,
            PixelFormat::I420 => picture.yuv,
            PixelFormat::I420P10 => {
                return Err(Error::Unsupported(
                    "H.264 I420P10 output is not implemented".to_string(),
                ));
            }
        };
        let mut frame = Self {
            width,
            height,
            format,
            pts,
            dts,
            poc: picture.poc,
            err_info: picture.err_info,
            storage,
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    fn from_h265(
        picture: hevc::Picture,
        format: PixelFormat,
        pts: i64,
        dts: i64,
    ) -> Result<Self, Error> {
        if picture.chroma_format != ChromaFormat::Yuv420 {
            return Err(Error::Unsupported(format!(
                "H.265 {:?} chroma output is not exposed by the V1 SDK",
                picture.chroma_format
            )));
        }
        let sample_bytes = match (picture.bit_depth, format) {
            (8, PixelFormat::Nv12 | PixelFormat::I420) => 1,
            (10, PixelFormat::I420P10) => 2,
            (depth, format) => {
                return Err(Error::Unsupported(format!(
                    "{depth}-bit H.265 cannot be exposed as {format:?}"
                )));
            }
        };
        let (chroma_width, chroma_height) = (picture.width / 2, picture.height / 2);
        let expected = picture
            .width
            .checked_mul(picture.height)
            .and_then(|luma| {
                chroma_width
                    .checked_mul(chroma_height)
                    .and_then(|chroma| luma.checked_add(chroma * 2))
            })
            .and_then(|samples| samples.checked_mul(sample_bytes))
            .ok_or_else(|| Error::State("H.265 frame size overflow"))?;
        if picture.yuv.len() < expected {
            return Err(Error::State("H.265 planar frame is truncated"));
        }
        let storage = if format == PixelFormat::Nv12 {
            i420_to_nv12(&picture.yuv[..expected], picture.width, picture.height)
        } else {
            picture.yuv[..expected].to_vec()
        };
        let mut frame = Self {
            width: picture.width,
            height: picture.height,
            format,
            pts,
            dts,
            poc: picture.poc,
            err_info: picture.err_info,
            storage,
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    fn from_vp9(
        picture: vp9::decoder::Picture,
        format: PixelFormat,
        pts: i64,
        dts: i64,
    ) -> Result<Self, Error> {
        let sample_bytes = match (picture.bit_depth, format) {
            (8, PixelFormat::Nv12 | PixelFormat::I420) => 1,
            (10, PixelFormat::I420P10) => 2,
            (depth, format) => {
                return Err(Error::Unsupported(format!(
                    "{depth}-bit VP9 cannot be exposed as {format:?}"
                )));
            }
        };
        let (chroma_width, chroma_height) = (picture.width / 2, picture.height / 2);
        let expected = picture
            .width
            .checked_mul(picture.height)
            .and_then(|luma| {
                chroma_width
                    .checked_mul(chroma_height)
                    .and_then(|chroma| luma.checked_add(chroma * 2))
            })
            .and_then(|samples| samples.checked_mul(sample_bytes))
            .ok_or_else(|| Error::State("VP9 frame size overflow"))?;
        if picture.yuv.len() < expected {
            return Err(Error::State("VP9 planar frame is truncated"));
        }
        let storage = if format == PixelFormat::Nv12 {
            i420_to_nv12(&picture.yuv[..expected], picture.width, picture.height)
        } else {
            picture.yuv[..expected].to_vec()
        };
        let mut frame = Self {
            width: picture.width,
            height: picture.height,
            format,
            pts,
            dts,
            poc: picture.poc as i32,
            err_info: picture.err_info,
            storage,
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    fn from_av1(
        picture: av1::decoder::Picture,
        format: PixelFormat,
        pts: i64,
        dts: i64,
    ) -> Result<Self, Error> {
        let sample_bytes = match (picture.bit_depth, format) {
            (8, PixelFormat::Nv12 | PixelFormat::I420) => 1,
            (10, PixelFormat::I420P10) => 2,
            (depth, format) => {
                return Err(Error::Unsupported(format!(
                    "{depth}-bit AV1 cannot be exposed as {format:?}"
                )));
            }
        };
        let (chroma_width, chroma_height) = (picture.width / 2, picture.height / 2);
        let expected = picture
            .width
            .checked_mul(picture.height)
            .and_then(|luma| {
                chroma_width
                    .checked_mul(chroma_height)
                    .and_then(|chroma| luma.checked_add(chroma * 2))
            })
            .and_then(|samples| samples.checked_mul(sample_bytes))
            .ok_or_else(|| Error::State("AV1 frame size overflow"))?;
        if picture.yuv.len() < expected {
            return Err(Error::State("AV1 planar frame is truncated"));
        }
        let storage = if format == PixelFormat::Nv12 {
            i420_to_nv12(&picture.yuv[..expected], picture.width, picture.height)
        } else {
            picture.yuv[..expected].to_vec()
        };
        let mut frame = Self {
            width: picture.width,
            height: picture.height,
            format,
            pts,
            dts,
            poc: picture.poc as i32,
            err_info: picture.err_info,
            storage,
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    fn from_jpeg_nv12(
        width: usize,
        height: usize,
        source: &[u8],
        source_stride: usize,
        source_height: usize,
        pts: i64,
        dts: i64,
        planar: bool,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 || source_stride < width {
            return Err(Error::Parse("invalid JPEG output geometry".to_string()));
        }
        let chroma_width = width.div_ceil(2);
        let chroma_height = height.div_ceil(2);
        let uv_row_bytes = chroma_width * 2;
        let y_size = width
            .checked_mul(height)
            .ok_or_else(|| Error::Parse("JPEG Y plane overflow".to_string()))?;
        let chroma_size = chroma_width
            .checked_mul(chroma_height)
            .ok_or_else(|| Error::Parse("JPEG chroma plane overflow".to_string()))?;
        let source_uv = source_stride
            .checked_mul(source_height)
            .ok_or_else(|| Error::Parse("JPEG source UV offset overflow".to_string()))?;
        if source_uv + uv_row_bytes * chroma_height > source.len() {
            return Err(Error::Parse("truncated JPEG hardware output".to_string()));
        }

        let mut output = Vec::with_capacity(if planar {
            y_size + chroma_size * 2
        } else {
            y_size + uv_row_bytes * chroma_height
        });
        for row in 0..height {
            let start = row * source_stride;
            output.extend_from_slice(&source[start..start + width]);
        }
        if planar {
            let mut u = Vec::with_capacity(chroma_size);
            let mut v = Vec::with_capacity(chroma_size);
            for row in 0..chroma_height {
                let start = source_uv + row * source_stride;
                for pixel in 0..chroma_width {
                    u.push(source[start + pixel * 2]);
                    v.push(source[start + pixel * 2 + 1]);
                }
            }
            output.extend_from_slice(&u);
            output.extend_from_slice(&v);
        } else {
            for row in 0..chroma_height {
                let start = source_uv + row * source_stride;
                output.extend_from_slice(&source[start..start + uv_row_bytes]);
            }
        }

        let format = if planar {
            PixelFormat::I420
        } else {
            PixelFormat::Nv12
        };
        let mut frame = Self {
            width,
            height,
            format,
            pts,
            dts,
            poc: 0,
            err_info: 0,
            storage: output,
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    fn layout_yuv(&mut self) -> Result<(), Error> {
        let chroma_width = self.width.div_ceil(2);
        let chroma_height = self.height.div_ceil(2);
        let sample_bytes = match self.format {
            PixelFormat::I420P10 => 2,
            PixelFormat::Nv12 | PixelFormat::I420 => 1,
        };
        let y_size = self
            .width
            .checked_mul(self.height)
            .and_then(|samples| samples.checked_mul(sample_bytes))
            .ok_or_else(|| Error::Parse("frame Y plane overflow".to_string()))?;
        let chroma_size = chroma_width
            .checked_mul(chroma_height)
            .and_then(|samples| samples.checked_mul(sample_bytes))
            .ok_or_else(|| Error::Parse("frame chroma plane overflow".to_string()))?;
        match self.format {
            PixelFormat::Nv12 => {
                let uv_size = chroma_width * 2 * chroma_height;
                if self.storage.len() < y_size + uv_size {
                    return Err(Error::State("NV12 frame storage is too small"));
                }
                self.planes = [
                    PlaneLayout {
                        offset: 0,
                        len: y_size,
                        stride: self.width,
                    },
                    PlaneLayout {
                        offset: y_size,
                        len: uv_size,
                        stride: chroma_width * 2,
                    },
                    PlaneLayout::default(),
                ];
            }
            PixelFormat::I420 | PixelFormat::I420P10 => {
                if self.storage.len() < y_size + chroma_size * 2 {
                    return Err(Error::State("planar frame storage is too small"));
                }
                self.planes = [
                    PlaneLayout {
                        offset: 0,
                        len: y_size,
                        stride: self.width * sample_bytes,
                    },
                    PlaneLayout {
                        offset: y_size,
                        len: chroma_size,
                        stride: chroma_width * sample_bytes,
                    },
                    PlaneLayout {
                        offset: y_size + chroma_size,
                        len: chroma_size,
                        stride: chroma_width * sample_bytes,
                    },
                ];
            }
        }
        Ok(())
    }
}

/// Public SDK error.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidInput(String),
    Unsupported(String),
    Parse(String),
    Hardware(String),
    Resource(String),
    State(&'static str),
    TooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message)
            | Self::Unsupported(message)
            | Self::Parse(message)
            | Self::Hardware(message)
            | Self::Resource(message) => formatter.write_str(message),
            Self::State(message) => formatter.write_str(message),
            Self::TooLarge => formatter.write_str("encoded packet exceeds the internal limit"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Hardware(error.to_string())
    }
}

/// Persistent decoder session.
pub struct Decoder {
    config: DecoderConfig,
    backend: Backend,
    frames: VecDeque<Frame>,
}

enum Backend {
    Av1(Av1Backend),
    H264(H264Backend),
    H265(H265Backend),
    Mjpeg(MjpegBackend),
    Vp9(Vp9Backend),
}

struct H264Backend {
    decoder: h264::Decoder,
    pending: VecDeque<PendingH264>,
    input: Vec<u8>,
    output_format: PixelFormat,
    max_reorder: usize,
    pending_pts: i64,
    pending_dts: i64,
    eos: bool,
}

struct PendingH264 {
    picture: h264::Picture,
    pts: i64,
    dts: i64,
}

struct H265Backend {
    decoder: hevc::Decoder,
    pending: VecDeque<PendingH265>,
    input: Vec<u8>,
    output_format: PixelFormat,
    max_reorder_override: usize,
    max_reorder: usize,
    pending_pts: i64,
    pending_dts: i64,
    eos: bool,
}

struct PendingH265 {
    picture: hevc::Picture,
    pts: i64,
    dts: i64,
}

struct MjpegBackend {
    service: Option<MppService>,
    format: PixelFormat,
    input: Vec<u8>,
    eos: bool,
}

struct Vp9Backend {
    decoder: vp9::decoder::Decoder,
    format: PixelFormat,
    eos: bool,
}

struct Av1Backend {
    decoder: Av1Decoder,
    format: PixelFormat,
    eos: bool,
}

enum Av1Decoder {
    Vdpu34x(av1::decoder::Decoder),
    Vdpu383(av1::decoder383::Decoder),
}

impl Av1Decoder {
    fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<av1::decoder::Picture>, String> {
        match self {
            Self::Vdpu34x(decoder) => decoder.decode_packet(packet),
            Self::Vdpu383(decoder) => decoder.decode_packet(packet),
        }
    }

    fn status(&self) -> (u32, u32, u32) {
        match self {
            Self::Vdpu34x(decoder) => decoder.status(),
            Self::Vdpu383(decoder) => decoder.status(),
        }
    }
}

impl Decoder {
    pub fn new(config: DecoderConfig) -> Result<Self, Error> {
        let backend = match config.codec {
            Codec::H264 => Backend::H264(H264Backend::new(config)?),
            Codec::H265 => Backend::H265(H265Backend::new(config)?),
            Codec::Mjpeg => Backend::Mjpeg(MjpegBackend::new(config)?),
            Codec::Vp9 => Backend::Vp9(Vp9Backend::new(config)?),
            Codec::Av1 => Backend::Av1(Av1Backend::new(config)?),
        };
        Ok(Self {
            config,
            backend,
            frames: VecDeque::new(),
        })
    }

    pub const fn config(&self) -> DecoderConfig {
        self.config
    }

    pub fn submit_packet(&mut self, packet: Packet<'_>) -> Result<usize, Error> {
        match &mut self.backend {
            Backend::H264(backend) => backend.submit(packet, &mut self.frames),
            Backend::H265(backend) => backend.submit(packet, &mut self.frames),
            Backend::Mjpeg(backend) => backend.submit(packet, &mut self.frames),
            Backend::Vp9(backend) => backend.submit(packet, &mut self.frames),
            Backend::Av1(backend) => backend.submit(packet, &mut self.frames),
        }
    }

    pub fn receive_frame(&mut self) -> Option<Frame> {
        self.frames.pop_front()
    }

    pub fn is_finished(&self) -> bool {
        if !self.frames.is_empty() {
            return false;
        }
        match &self.backend {
            Backend::H264(backend) => {
                backend.eos && backend.input.is_empty() && backend.pending.is_empty()
            }
            Backend::H265(backend) => {
                backend.eos && backend.input.is_empty() && backend.pending.is_empty()
            }
            Backend::Mjpeg(backend) => backend.eos && backend.input.is_empty(),
            Backend::Vp9(backend) => backend.eos,
            Backend::Av1(backend) => backend.eos,
        }
    }

    pub fn flush(&mut self) -> Result<usize, Error> {
        match &mut self.backend {
            Backend::H264(backend) => backend.flush(&mut self.frames),
            Backend::H265(backend) => backend.flush(&mut self.frames),
            Backend::Mjpeg(backend) => backend.flush(&mut self.frames),
            Backend::Vp9(backend) => backend.flush(),
            Backend::Av1(backend) => backend.flush(),
        }
    }

    pub fn reset(&mut self) -> Result<(), Error> {
        self.frames.clear();
        *self = Self::new(self.config)?;
        Ok(())
    }

    pub fn recover(&mut self) -> Result<(), Error> {
        match &mut self.backend {
            Backend::H264(backend) => backend
                .decoder
                .recover_session()
                .map_err(|error| Error::Hardware(error.to_string())),
            Backend::H265(_) | Backend::Mjpeg(_) | Backend::Vp9(_) | Backend::Av1(_) => {
                self.reset()
            }
        }
    }

    pub fn status(&self) -> Option<(u32, u32, u32)> {
        match &self.backend {
            Backend::H264(backend) => Some(backend.decoder.status()),
            Backend::H265(backend) => Some(backend.decoder.status()),
            Backend::Mjpeg(_) => None,
            Backend::Vp9(backend) => Some(backend.decoder.status()),
            Backend::Av1(backend) => Some(backend.decoder.status()),
        }
    }
}

impl H264Backend {
    fn new(config: DecoderConfig) -> Result<Self, Error> {
        if config.output_format == PixelFormat::I420P10 {
            return Err(Error::Unsupported(
                "H.264 I420P10 output is not implemented".to_string(),
            ));
        }
        let mut decoder =
            h264::Decoder::new().map_err(|error| Error::Hardware(error.to_string()))?;
        decoder.set_output_nv12(config.output_format == PixelFormat::Nv12);
        let reorder_frames = if config.max_reorder_frames == 0 {
            decoder.max_num_reorder_frames()
        } else {
            (config.max_reorder_frames as usize).min(16)
        };
        Ok(Self {
            decoder,
            pending: VecDeque::new(),
            input: Vec::new(),
            output_format: config.output_format,
            max_reorder: reorder_frames,
            pending_pts: 0,
            pending_dts: 0,
            eos: false,
        })
    }

    fn submit(&mut self, packet: Packet<'_>, output: &mut VecDeque<Frame>) -> Result<usize, Error> {
        if self.eos {
            return Err(Error::State("packet submitted after EOS"));
        }
        if self.input.len().saturating_add(packet.data.len()) > MAX_ENCODED_BYTES {
            return Err(Error::TooLarge);
        }
        if self.input.is_empty() {
            self.pending_pts = packet.pts;
            self.pending_dts = packet.dts;
        }
        self.input.extend_from_slice(packet.data);
        let flush_tail = packet.flags.contains(PacketFlags::COMPLETE_ACCESS_UNIT)
            || packet.flags.contains(PacketFlags::EOS);
        let units = take_h264_access_units(&mut self.input, flush_tail);
        let mut decoded = 0;
        for unit in units {
            let picture = self
                .decoder
                .decode_access_unit(&unit)
                .map_err(classify_video_error)?;
            if let Some(picture) = picture {
                decoded += 1;
                self.push_pending(
                    PendingH264 {
                        picture,
                        pts: packet.pts,
                        dts: packet.dts,
                    },
                    output,
                )?;
            }
        }
        if packet.flags.contains(PacketFlags::EOS) {
            self.eos = true;
            self.flush_reorder(output)?;
        }
        Ok(decoded)
    }

    fn push_pending(
        &mut self,
        pending: PendingH264,
        output: &mut VecDeque<Frame>,
    ) -> Result<(), Error> {
        if pending.picture.idr && !self.pending.is_empty() {
            self.flush_reorder(output)?;
        }
        if !h264_picture_is_output(&pending.picture) {
            return Ok(());
        }
        self.pending.push_back(pending);
        self.pending
            .make_contiguous()
            .sort_by_key(|item| item.picture.poc);
        while self.pending.len() > self.max_reorder {
            let item = self.pending.pop_front().unwrap();
            output.push_back(Frame::from_h264(
                item.picture,
                self.output_format,
                item.pts,
                item.dts,
            )?);
        }
        Ok(())
    }

    fn flush_reorder(&mut self, output: &mut VecDeque<Frame>) -> Result<(), Error> {
        self.pending
            .make_contiguous()
            .sort_by_key(|item| item.picture.poc);
        while let Some(item) = self.pending.pop_front() {
            output.push_back(Frame::from_h264(
                item.picture,
                self.output_format,
                item.pts,
                item.dts,
            )?);
        }
        Ok(())
    }

    fn flush(&mut self, output: &mut VecDeque<Frame>) -> Result<usize, Error> {
        if self.eos && self.pending.is_empty() && self.input.is_empty() {
            return Ok(0);
        }
        let units = take_h264_access_units(&mut self.input, true);
        let mut decoded = 0;
        for unit in units {
            if let Some(picture) = self
                .decoder
                .decode_access_unit(&unit)
                .map_err(classify_video_error)?
            {
                decoded += 1;
                self.push_pending(
                    PendingH264 {
                        picture,
                        pts: self.pending_pts,
                        dts: self.pending_dts,
                    },
                    output,
                )?;
            }
        }
        self.flush_reorder(output)?;
        self.eos = true;
        Ok(decoded)
    }
}

impl H265Backend {
    fn new(config: DecoderConfig) -> Result<Self, Error> {
        let decoder = hevc::Decoder::new().map_err(|error| Error::Hardware(error.to_string()))?;
        let max_reorder_override = (config.max_reorder_frames as usize).min(16);
        Ok(Self {
            decoder,
            pending: VecDeque::new(),
            input: Vec::new(),
            output_format: config.output_format,
            max_reorder_override,
            max_reorder: max_reorder_override,
            pending_pts: 0,
            pending_dts: 0,
            eos: false,
        })
    }

    fn submit(&mut self, packet: Packet<'_>, output: &mut VecDeque<Frame>) -> Result<usize, Error> {
        if self.eos {
            return Err(Error::State("packet submitted after EOS"));
        }
        if self.input.len().saturating_add(packet.data.len()) > MAX_ENCODED_BYTES {
            return Err(Error::TooLarge);
        }
        if self.input.is_empty() {
            self.pending_pts = packet.pts;
            self.pending_dts = packet.dts;
        }
        self.input.extend_from_slice(packet.data);
        let flush_tail = packet.flags.contains(PacketFlags::COMPLETE_ACCESS_UNIT)
            || packet.flags.contains(PacketFlags::EOS);
        let units = take_hevc_access_units(&mut self.input, flush_tail);
        let mut decoded = 0;
        for unit in units {
            let picture = self
                .decoder
                .decode_access_unit(&unit)
                .map_err(classify_video_error)?;
            if let Some(picture) = picture {
                decoded += 1;
                self.refresh_reorder();
                self.push_pending(
                    PendingH265 {
                        picture,
                        pts: packet.pts,
                        dts: packet.dts,
                    },
                    output,
                )?;
            }
        }
        if packet.flags.contains(PacketFlags::EOS) {
            self.eos = true;
            self.flush_reorder(output)?;
        }
        Ok(decoded)
    }

    fn refresh_reorder(&mut self) {
        if self.max_reorder_override == 0 {
            self.max_reorder = self.decoder.max_num_reorder_pics();
        }
    }

    fn push_pending(
        &mut self,
        pending: PendingH265,
        output: &mut VecDeque<Frame>,
    ) -> Result<(), Error> {
        if self
            .pending
            .back()
            .is_some_and(|queued| queued.picture.epoch != pending.picture.epoch)
        {
            self.flush_reorder(output)?;
        }
        if !hevc::picture_is_output(&pending.picture) {
            return Ok(());
        }
        self.pending.push_back(pending);
        self.sort_pending();
        while self.pending.len() > self.max_reorder {
            let item = self.pending.pop_front().unwrap();
            output.push_back(Frame::from_h265(
                item.picture,
                self.output_format,
                item.pts,
                item.dts,
            )?);
        }
        Ok(())
    }

    fn sort_pending(&mut self) {
        self.pending
            .make_contiguous()
            .sort_by_key(|item| (item.picture.epoch, item.picture.poc));
    }

    fn flush_reorder(&mut self, output: &mut VecDeque<Frame>) -> Result<(), Error> {
        self.sort_pending();
        while let Some(item) = self.pending.pop_front() {
            output.push_back(Frame::from_h265(
                item.picture,
                self.output_format,
                item.pts,
                item.dts,
            )?);
        }
        Ok(())
    }

    fn flush(&mut self, output: &mut VecDeque<Frame>) -> Result<usize, Error> {
        if self.eos && self.pending.is_empty() && self.input.is_empty() {
            return Ok(0);
        }
        let units = take_hevc_access_units(&mut self.input, true);
        let mut decoded = 0;
        for unit in units {
            if let Some(picture) = self
                .decoder
                .decode_access_unit(&unit)
                .map_err(classify_video_error)?
            {
                decoded += 1;
                self.refresh_reorder();
                self.push_pending(
                    PendingH265 {
                        picture,
                        pts: self.pending_pts,
                        dts: self.pending_dts,
                    },
                    output,
                )?;
            }
        }
        self.flush_reorder(output)?;
        self.eos = true;
        Ok(decoded)
    }
}

impl Vp9Backend {
    fn new(config: DecoderConfig) -> Result<Self, Error> {
        Ok(Self {
            decoder: vp9::decoder::Decoder::new()
                .map_err(|error| Error::Hardware(error.to_string()))?,
            format: config.output_format,
            eos: false,
        })
    }

    fn submit(&mut self, packet: Packet<'_>, output: &mut VecDeque<Frame>) -> Result<usize, Error> {
        if self.eos {
            return Err(Error::State("packet submitted after EOS"));
        }
        if packet.data.len() > MAX_ENCODED_BYTES {
            return Err(Error::TooLarge);
        }
        if !packet.data.is_empty() {
            let pictures = self
                .decoder
                .decode_packet(packet.data)
                .map_err(classify_video_error)?;
            let decoded = pictures.len();
            for picture in pictures {
                if picture.err_info == 0 {
                    output.push_back(Frame::from_vp9(
                        picture,
                        self.format,
                        packet.pts,
                        packet.dts,
                    )?);
                }
            }
            if packet.flags.contains(PacketFlags::EOS) {
                self.eos = true;
            }
            return Ok(decoded);
        }
        if packet.flags.contains(PacketFlags::EOS) {
            self.eos = true;
        }
        Ok(0)
    }

    fn flush(&mut self) -> Result<usize, Error> {
        self.eos = true;
        Ok(0)
    }
}

impl Av1Backend {
    fn new(config: DecoderConfig) -> Result<Self, Error> {
        let profile = SocProfile::detect().map_err(|error| Error::Hardware(error.to_string()))?;
        let decoder = match profile.video_decode {
            VideoDecodeHal::Vdpu34x => Av1Decoder::Vdpu34x(
                av1::decoder::Decoder::new().map_err(|error| Error::Hardware(error.to_string()))?,
            ),
            VideoDecodeHal::Vdpu383 => Av1Decoder::Vdpu383(av1::decoder383::Decoder::new()),
            other => {
                return Err(Error::Unsupported(format!(
                    "no AV1 decoder backend for {other:?}"
                )));
            }
        };
        Ok(Self {
            decoder,
            format: config.output_format,
            eos: false,
        })
    }

    fn submit(&mut self, packet: Packet<'_>, output: &mut VecDeque<Frame>) -> Result<usize, Error> {
        if self.eos {
            return Err(Error::State("packet submitted after EOS"));
        }
        if packet.data.len() > MAX_ENCODED_BYTES {
            return Err(Error::TooLarge);
        }
        if !packet.data.is_empty() {
            let pictures = self
                .decoder
                .decode_packet(packet.data)
                .map_err(classify_video_error)?;
            let decoded = pictures.len();
            for picture in pictures {
                if picture.err_info == 0 {
                    output.push_back(Frame::from_av1(
                        picture,
                        self.format,
                        packet.pts,
                        packet.dts,
                    )?);
                }
            }
            if packet.flags.contains(PacketFlags::EOS) {
                self.eos = true;
            }
            return Ok(decoded);
        }
        if packet.flags.contains(PacketFlags::EOS) {
            self.eos = true;
        }
        Ok(0)
    }

    fn flush(&mut self) -> Result<usize, Error> {
        self.eos = true;
        Ok(0)
    }
}

impl MjpegBackend {
    fn new(config: DecoderConfig) -> Result<Self, Error> {
        if config.output_format == PixelFormat::I420P10 {
            return Err(Error::Unsupported(
                "MJPEG I420P10 output is not implemented".to_string(),
            ));
        }
        Ok(Self {
            service: None,
            format: config.output_format,
            input: Vec::new(),
            eos: false,
        })
    }

    fn submit(&mut self, packet: Packet<'_>, output: &mut VecDeque<Frame>) -> Result<usize, Error> {
        if self.eos {
            return Err(Error::State("packet submitted after EOS"));
        }
        if self.input.len().saturating_add(packet.data.len()) > MAX_ENCODED_BYTES {
            return Err(Error::TooLarge);
        }
        self.input.extend_from_slice(packet.data);
        let frames = extract_jpegs(&mut self.input, packet.flags.contains(PacketFlags::EOS))
            .map_err(Error::Parse)?;
        let mut decoded = 0;
        for frame_data in frames {
            output.push_back(self.decode(&frame_data, packet.pts, packet.dts)?);
            decoded += 1;
        }
        if packet.flags.contains(PacketFlags::EOS) {
            self.eos = true;
        }
        Ok(decoded)
    }

    fn flush(&mut self, output: &mut VecDeque<Frame>) -> Result<usize, Error> {
        let frames = extract_jpegs(&mut self.input, true).map_err(Error::Parse)?;
        let mut decoded = 0;
        for frame_data in frames {
            output.push_back(self.decode(&frame_data, 0, 0)?);
            decoded += 1;
        }
        self.eos = true;
        Ok(decoded)
    }

    fn decode(&mut self, data: &[u8], pts: i64, dts: i64) -> Result<Frame, Error> {
        let info = jpeg_parser::parse(data).map_err(|error| Error::Parse(error.to_string()))?;
        let tables = jpeg_tables::Tables::build(&info)
            .map_err(|error| Error::Parse(format!("{error:?}")))?;
        let rgb = false;
        let geometry = jpegd::geometry(&info, rgb, JpegDownscale::Disabled)
            .map_err(|error| Error::Parse(error.to_string()))?;
        self.service = None;
        self.service = Some(
            MppService::open(VPU_CLIENT_JPEG_DEC)
                .map_err(|error| Error::Hardware(format!("open JPEG decoder: {error}")))?,
        );
        let mut table =
            DmaBuf::alloc("system", jpeg_tables::RKD_TABLE_SIZE, false).map_err(Error::from)?;
        write_dma(&mut table, tables.as_slice())?;
        let mut stream = DmaBuf::alloc("system", data.len(), false).map_err(Error::from)?;
        write_dma(&mut stream, data)?;
        let output = DmaBuf::alloc("system", geometry.frame_size, false).map_err(Error::from)?;
        let (registers, output_geometry) = jpegd::build(&jpegd::Job {
            info: &info,
            table_fd: table.fd(),
            stream_fd: stream.fd(),
            stream_len: data.len(),
            output_fd: output.fd(),
            rgb,
            downscale: JpegDownscale::Disabled,
        })
        .map_err(|error| Error::Parse(error.to_string()))?;
        self.service
            .as_mut()
            .unwrap()
            .submit_regs_recovering(&registers.words)
            .map_err(|error| Error::Hardware(format!("JPEG submit: {error}")))?;
        self.service
            .as_mut()
            .unwrap()
            .poll_hw_finish_recovering()
            .map_err(|error| Error::Hardware(format!("JPEG poll: {error}")))?;
        output.sync_read_begin().map_err(Error::from)?;
        let result = Frame::from_jpeg_nv12(
            info.width,
            info.height,
            output.as_slice(),
            output_geometry.stride_bytes,
            output_geometry.out_height,
            pts,
            dts,
            self.format == PixelFormat::I420,
        );
        output.sync_read_end().map_err(Error::from)?;
        result
    }
}

fn write_dma(buffer: &mut DmaBuf, bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() > buffer.len() {
        return Err(Error::Resource("DMA buffer is too small".to_string()));
    }
    buffer.sync_write_begin().map_err(Error::from)?;
    buffer.as_mut_slice()[..bytes.len()].copy_from_slice(bytes);
    buffer.sync_write_end().map_err(Error::from)
}

fn i420_to_nv12(planar: &[u8], width: usize, height: usize) -> Vec<u8> {
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    let luma_size = width * height;
    let chroma_size = chroma_width * chroma_height;
    let mut packed = Vec::with_capacity(luma_size + chroma_size * 2);
    packed.extend_from_slice(&planar[..luma_size]);
    let u = &planar[luma_size..luma_size + chroma_size];
    let v = &planar[luma_size + chroma_size..luma_size + chroma_size * 2];
    for (u, v) in u.iter().zip(v) {
        packed.push(*u);
        packed.push(*v);
    }
    packed
}

fn classify_video_error(message: String) -> Error {
    if message.contains("hardware") || message.contains("submit") || message.contains("poll") {
        Error::Hardware(message)
    } else {
        Error::Parse(message)
    }
}

fn take_h264_access_units(buffer: &mut Vec<u8>, flush: bool) -> Vec<Vec<u8>> {
    let nals = h264::parser::split_annex_b(buffer);
    if nals.is_empty() {
        if flush {
            buffer.clear();
        }
        return Vec::new();
    }
    let mut start = None;
    let mut has_vcl = false;
    let mut trailing_non_vcl = None;
    let mut ranges = Vec::new();
    for nal in &nals {
        let boundary = match nal.unit_type {
            h264::parser::NAL_AUD => true,
            1..=5 => nal.rbsp.first().is_some_and(|byte| byte & 0x80 != 0),
            _ => false,
        };
        if boundary && has_vcl {
            let next_start = trailing_non_vcl.unwrap_or(nal.offset);
            ranges.push((start.unwrap(), next_start));
            start = Some(next_start);
            has_vcl = nal.is_vcl();
            trailing_non_vcl = None;
            continue;
        }
        if start.is_none() {
            start = Some(nal.offset);
        }
        if nal.is_vcl() {
            has_vcl = true;
            trailing_non_vcl = None;
        } else if has_vcl && trailing_non_vcl.is_none() {
            trailing_non_vcl = Some(nal.offset);
        }
    }
    let keep = start.unwrap_or(buffer.len());
    if flush && has_vcl && start.is_some() {
        ranges.push((start.unwrap(), buffer.len()));
    }
    let output = ranges
        .into_iter()
        .map(|(begin, end)| buffer[begin..end].to_vec())
        .collect();
    if flush {
        buffer.clear();
    } else if keep >= buffer.len() {
        buffer.clear();
    } else {
        buffer.drain(..keep);
    }
    output
}

fn take_hevc_access_units(buffer: &mut Vec<u8>, flush: bool) -> Vec<Vec<u8>> {
    let nals = hevc::parser::split_annex_b(buffer);
    if nals.is_empty() {
        if flush {
            buffer.clear();
        }
        return Vec::new();
    }
    let mut start = None;
    let mut has_vcl = false;
    let mut trailing_non_vcl = None;
    let mut ranges = Vec::new();
    for nal in &nals {
        let boundary = match nal.unit_type {
            hevc::parser::NAL_AUD => true,
            0..=31 => nal.rbsp.first().is_some_and(|byte| byte & 0x80 != 0),
            _ => false,
        };
        if boundary && has_vcl {
            let next_start = trailing_non_vcl.unwrap_or(nal.offset);
            ranges.push((start.unwrap(), next_start));
            start = Some(next_start);
            has_vcl = nal.is_vcl();
            trailing_non_vcl = None;
            continue;
        }
        if start.is_none() {
            start = Some(nal.offset);
        }
        if nal.is_vcl() {
            has_vcl = true;
            trailing_non_vcl = None;
        } else if has_vcl && trailing_non_vcl.is_none() {
            trailing_non_vcl = Some(nal.offset);
        }
    }
    let keep = start.unwrap_or(buffer.len());
    if flush && has_vcl && start.is_some() {
        ranges.push((start.unwrap(), buffer.len()));
    }
    let output = ranges
        .into_iter()
        .map(|(begin, end)| buffer[begin..end].to_vec())
        .collect();
    if flush {
        buffer.clear();
    } else if keep >= buffer.len() {
        buffer.clear();
    } else {
        buffer.drain(..keep);
    }
    output
}

fn extract_jpegs(buffer: &mut Vec<u8>, eos: bool) -> Result<Vec<Vec<u8>>, String> {
    let mut output = Vec::new();
    loop {
        let Some(start) = find_marker(buffer, 0, [0xff, 0xd8]) else {
            if eos {
                buffer.clear();
            }
            break;
        };
        if start != 0 {
            buffer.drain(..start);
        }
        let Some(end) = find_marker(buffer, 2, [0xff, 0xd9]) else {
            if eos {
                if buffer.len() >= 4 {
                    output.push(buffer.clone());
                }
                buffer.clear();
            }
            break;
        };
        let end = end + 2;
        output.push(buffer.drain(..end).collect());
    }
    Ok(output)
}

fn find_marker(buffer: &[u8], start: usize, marker: [u8; 2]) -> Option<usize> {
    buffer
        .get(start..)?
        .windows(2)
        .position(|window| window == marker)
        .map(|offset| offset + start)
}

/// Split Annex-B input for consumers that need the same framing as the SDK.
pub fn split_h264_packets(data: &[u8]) -> Vec<Vec<u8>> {
    let mut input = data.to_vec();
    take_h264_access_units(&mut input, true)
}

/// The existing offline splitter remains useful for complete files.
pub fn h264_access_units(data: &[u8]) -> Vec<&[u8]> {
    split_access_units(data)
}

/// Split Annex-B H.265 input for consumers that need SDK-equivalent framing.
pub fn split_hevc_packets(data: &[u8]) -> Vec<Vec<u8>> {
    let mut input = data.to_vec();
    take_hevc_access_units(&mut input, true)
}

/// The offline H.265 access-unit splitter over a complete byte stream.
pub fn hevc_access_units(data: &[u8]) -> Vec<&[u8]> {
    hevc::decoder::split_access_units(data)
}

#[cfg(test)]
mod tests {
    use super::{
        extract_jpegs, take_h264_access_units, take_hevc_access_units, Codec, DecoderConfig,
        PacketFlags, PixelFormat,
    };

    #[test]
    fn h264_framer_keeps_the_tail_until_a_new_picture_or_flush() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x00]);
        data.extend_from_slice(&[0, 0, 1, 0x65, 0x80, 0x01]);
        data.extend_from_slice(&[0, 0, 1, 0x41, 0x80, 0x02]);
        let first = take_h264_access_units(&mut data, false);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].last(), Some(&0x01));
        assert!(!data.is_empty());
        let tail = take_h264_access_units(&mut data, true);
        assert_eq!(tail.len(), 1);
        assert!(data.is_empty());
    }

    #[test]
    fn mjpeg_framer_extracts_concatenated_pictures() {
        let mut data = vec![
            0xff, 0xd8, 0x11, 0x22, 0xff, 0xd9, 0xff, 0xd8, 0x33, 0xff, 0xd9,
        ];
        let frames = extract_jpegs(&mut data, false).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], [0xff, 0xd8, 0x11, 0x22, 0xff, 0xd9]);
        assert_eq!(frames[1], [0xff, 0xd8, 0x33, 0xff, 0xd9]);
        assert!(data.is_empty());
    }

    #[test]
    fn h264_framer_attaches_parameter_sets_to_the_next_picture() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x65, 0x80]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x68, 0x00]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x41, 0x80]);

        let frames = take_h264_access_units(&mut data, true);
        assert_eq!(frames.len(), 2);
        assert!(frames[1]
            .windows(5)
            .any(|window| window == [0, 0, 0, 1, 0x68]));
        assert!(frames[1]
            .windows(5)
            .any(|window| window == [0, 0, 0, 1, 0x41]));
    }

    #[test]
    fn hevc_framer_keeps_the_tail_and_carries_parameter_sets() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0, 0, 1, 0x26, 0x01, 0x80]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x44, 0x01, 0x00]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x26, 0x01, 0x80]);

        let first = take_hevc_access_units(&mut data, false);
        assert_eq!(first.len(), 1);
        assert!(!data.is_empty());

        let mut data = first[0].clone();
        data.extend_from_slice(&[0, 0, 0, 1, 0x44, 0x01, 0x00]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x26, 0x01, 0x80]);
        let frames = take_hevc_access_units(&mut data, true);
        assert_eq!(frames.len(), 2);
        assert!(frames[1].starts_with(&[0, 0, 0, 1, 0x44, 0x01]));
    }

    #[test]
    fn i420_to_nv12_interleaves_the_planes() {
        let planar = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        assert_eq!(
            super::i420_to_nv12(&planar, 4, 2),
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 10, 12]
        );
    }

    #[test]
    fn mjpeg_eos_finishes_an_empty_public_session() {
        let mut decoder = super::Decoder::new(DecoderConfig::new(Codec::Mjpeg)).unwrap();
        let packet = super::Packet::new(&[]).with_flags(PacketFlags::EOS);
        assert_eq!(decoder.submit_packet(packet).unwrap(), 0);
        assert!(decoder.is_finished());
        assert!(decoder.receive_frame().is_none());
        assert_eq!(
            decoder.submit_packet(super::Packet::new(&[])),
            Err(super::Error::State("packet submitted after EOS"))
        );
        decoder.reset().unwrap();
        assert!(!decoder.is_finished());
    }

    #[test]
    fn unknown_packet_flags_are_rejected_by_the_public_bitset() {
        assert!(PacketFlags::from_bits(0).is_some());
        assert!(PacketFlags::from_bits(PacketFlags::EOS.bits()).is_some());
        assert!(PacketFlags::from_bits(1 << 31).is_none());
    }

    #[test]
    fn configuration_api_is_explicit() {
        let config = DecoderConfig::new(Codec::H265)
            .with_output_format(PixelFormat::I420P10)
            .with_max_reorder_frames(3);
        assert_eq!(config.codec(), Codec::H265);
        assert_eq!(config.output_format(), PixelFormat::I420P10);
        assert_eq!(config.max_reorder_frames(), 3);
        assert!(PacketFlags::empty().with_eos().contains(PacketFlags::EOS));
    }
}
