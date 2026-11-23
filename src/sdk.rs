//! Stable, streaming-oriented codec APIs for library consumers.
//!
//! Unlike the historical JPEG helper in the crate root, this surface accepts
//! in-memory packets, keeps a codec session alive, and returns owned frames
//! with explicit plane metadata.

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};

use crate::codec::av1;
use crate::codec::avs;
use crate::codec::h263;
use crate::codec::h264::{
    self,
    decoder::{picture_is_output as h264_picture_is_output, split_access_units, AccessUnitScanner},
};
use crate::codec::hevc;
use crate::codec::jpeg::{parser as jpeg_parser, tables as jpeg_tables};
use crate::codec::m2v;
use crate::codec::mpg4;
use crate::codec::vp8;
use crate::codec::vp9;
use crate::codec::DmaBufSurface;
use crate::hal::hevc::ChromaFormat;
use crate::hal::jpegd::{self as jpegd, JpegDownscale};
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{SocProfile, VideoDecodeHal};
use crate::service::{MppService, VPU_CLIENT_JPEG_DEC};

pub const API_VERSION_MAJOR: u32 = 1;
pub const API_VERSION_MINOR: u32 = 14;
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
    Vp8,
    Mpeg2,
    Mpeg4,
    H263,
    Avs,
}

impl Codec {
    pub const fn c_value(self) -> u32 {
        match self {
            Self::H264 => 1,
            Self::H265 => 3,
            Self::Mjpeg => 2,
            Self::Vp9 => 4,
            Self::Av1 => 5,
            Self::Vp8 => 6,
            Self::Mpeg2 => 7,
            Self::Mpeg4 => 8,
            Self::H263 => 9,
            Self::Avs => 10,
        }
    }

    pub const fn from_c_value(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::H264),
            2 => Some(Self::Mjpeg),
            3 => Some(Self::H265),
            4 => Some(Self::Vp9),
            5 => Some(Self::Av1),
            6 => Some(Self::Vp8),
            7 => Some(Self::Mpeg2),
            8 => Some(Self::Mpeg4),
            9 => Some(Self::H263),
            10 => Some(Self::Avs),
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
    /// Semi-planar 4:2:0 with 10-bit samples packed at ten bits per pixel.
    ///
    /// This is MPP's `YUV420SP_10BIT` and FFmpeg's `NV15` layout. It is
    /// produced for hardware DMA-BUF output; CPU output remains `I420P10`.
    Nv15,
}

impl PixelFormat {
    pub const fn c_value(self) -> u32 {
        match self {
            Self::Nv12 => 1,
            Self::I420 => 2,
            Self::I420P10 => 3,
            Self::Nv15 => 4,
        }
    }

    pub const fn from_c_value(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Nv12),
            2 => Some(Self::I420),
            3 => Some(Self::I420P10),
            4 => Some(Self::Nv15),
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
    dmabuf_output: bool,
    vproc_output: bool,
}

impl DecoderConfig {
    pub const fn new(codec: Codec) -> Self {
        Self {
            codec,
            output_format: PixelFormat::Nv12,
            max_reorder_frames: 0,
            dmabuf_output: false,
            vproc_output: false,
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

    pub const fn dmabuf_output(self) -> bool {
        self.dmabuf_output
    }

    pub const fn with_max_reorder_frames(mut self, frames: u32) -> Self {
        self.max_reorder_frames = frames;
        self
    }

    pub const fn with_dmabuf_output(mut self, enabled: bool) -> Self {
        self.dmabuf_output = enabled;
        self
    }

    /// Apply the platform's default IEP2 post-processing before returning
    /// frames. Currently supported by the AVS decoder.
    pub const fn vproc_output(self) -> bool {
        self.vproc_output
    }

    pub const fn with_vproc_output(mut self, enabled: bool) -> Self {
        self.vproc_output = enabled;
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

/// Borrowed DMA-BUF metadata for one frame plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaBufPlane {
    fd: i32,
    offset: u64,
    size: u64,
    stride: u32,
}

impl DmaBufPlane {
    /// Borrowed fd. Use [`Frame::duplicate_dmabuf`] to transfer ownership.
    pub const fn fd(self) -> i32 {
        self.fd
    }

    pub const fn offset(self) -> u64 {
        self.offset
    }

    pub const fn size(self) -> u64 {
        self.size
    }

    pub const fn stride(self) -> u32 {
        self.stride
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PlaneLayout {
    offset: usize,
    len: usize,
    stride: usize,
}

enum FrameStorage {
    Cpu(Vec<u8>),
    DmaBuf(DmaBuf),
    DecodedDmaBuf {
        mapping: DmaBuf,
        surface: DmaBufSurface,
    },
}

impl FrameStorage {
    fn len(&self) -> usize {
        match self {
            Self::Cpu(bytes) => bytes.len(),
            Self::DmaBuf(buffer) => buffer.len(),
            Self::DecodedDmaBuf { mapping, .. } => mapping.len(),
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Cpu(bytes) => bytes,
            Self::DmaBuf(buffer) => buffer.as_slice(),
            Self::DecodedDmaBuf { mapping, .. } => mapping.as_slice(),
        }
    }

    fn cpu_copy(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}

impl std::fmt::Debug for FrameStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cpu(bytes) => formatter
                .debug_struct("Cpu")
                .field("len", &bytes.len())
                .finish(),
            Self::DmaBuf(buffer) => formatter
                .debug_struct("DmaBuf")
                .field("fd", &buffer.fd())
                .field("len", &buffer.len())
                .finish(),
            Self::DecodedDmaBuf { mapping, surface } => formatter
                .debug_struct("DecodedDmaBuf")
                .field("fd", &mapping.fd())
                .field("len", &mapping.len())
                .field("surface", surface)
                .finish(),
        }
    }
}

/// One decoded frame, including media timestamps and codec metadata.
///
/// DMA-BUF-backed frames keep their hardware buffer; cloning such a frame
/// makes an independent CPU byte copy rather than sharing the mutable surface.
#[derive(Debug)]
pub struct Frame {
    width: usize,
    height: usize,
    format: PixelFormat,
    pts: i64,
    dts: i64,
    poc: i32,
    err_info: u32,
    storage: FrameStorage,
    planes: [PlaneLayout; 3],
}

impl Clone for Frame {
    fn clone(&self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            format: self.format,
            pts: self.pts,
            dts: self.dts,
            poc: self.poc,
            err_info: self.err_info,
            storage: FrameStorage::Cpu(self.storage.cpu_copy()),
            planes: self.planes,
        }
    }
}

impl Frame {
    /// Take the host storage of a consumed frame so a decoder can reuse it.
    pub fn into_cpu_storage(self) -> Option<Vec<u8>> {
        match self.storage {
            FrameStorage::Cpu(vec) => Some(vec),
            _ => None,
        }
    }

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
        self.storage.as_slice()
    }

    pub const fn uses_dmabuf(&self) -> bool {
        matches!(
            &self.storage,
            FrameStorage::DmaBuf(_) | FrameStorage::DecodedDmaBuf { .. }
        )
    }

    pub fn plane(&self, index: usize) -> Option<Plane<'_>> {
        let plane = *self.planes.get(index)?;
        (plane.len != 0).then(|| Plane {
            data: &self.storage.as_slice()[plane.offset..plane.offset + plane.len],
            stride: plane.stride,
        })
    }

    pub fn dmabuf_plane(&self, index: usize) -> Option<DmaBufPlane> {
        let fd = match &self.storage {
            FrameStorage::DmaBuf(buffer) => buffer.fd(),
            FrameStorage::DecodedDmaBuf { mapping, .. } => mapping.fd(),
            FrameStorage::Cpu(_) => return None,
        };
        let plane = *self.planes.get(index)?;
        (plane.len != 0).then(|| DmaBufPlane {
            fd,
            offset: plane.offset as u64,
            size: plane.len as u64,
            stride: plane.stride as u32,
        })
    }

    pub fn duplicate_dmabuf(&self) -> io::Result<Option<OwnedFd>> {
        match &self.storage {
            FrameStorage::DmaBuf(buffer) => buffer.dup_fd().map(Some),
            FrameStorage::DecodedDmaBuf { mapping, .. } => mapping.dup_fd().map(Some),
            FrameStorage::Cpu(_) => Ok(None),
        }
    }

    /// Transfer the canonical NV12 DMA-BUF storage out of this frame.
    ///
    /// The compatibility layer uses this to expose a decoded hardware surface
    /// through the legacy MPP object API without copying a full picture.
    pub(crate) fn into_nv12_dmabuf(self) -> Result<(DmaBuf, usize, usize), Self> {
        let first = self.planes[0];
        let second = self.planes[1];
        let canonical = self.format == PixelFormat::Nv12
            && first.offset == 0
            && first.stride != 0
            && first.len % first.stride == 0
            && second.offset == first.len
            && second.stride == first.stride;
        if !canonical {
            return Err(self);
        }
        if !matches!(
            self.storage,
            FrameStorage::DmaBuf(_) | FrameStorage::DecodedDmaBuf { .. }
        ) {
            return Err(self);
        }
        let mapping = match self.storage {
            FrameStorage::DmaBuf(buffer) => buffer,
            FrameStorage::DecodedDmaBuf { mapping, .. } => mapping,
            FrameStorage::Cpu(_) => unreachable!("DMA-BUF storage was checked above"),
        };
        Ok((mapping, first.stride, first.len / first.stride))
    }

    /// Transfer the packed 10-bit semi-planar DMA-BUF storage out of this
    /// frame. The layout is MPP `YUV420SP_10BIT` / FFmpeg `NV15`.
    pub(crate) fn into_nv15_dmabuf(self) -> Result<(DmaBuf, usize, usize), Self> {
        let first = self.planes[0];
        let second = self.planes[1];
        let canonical = self.format == PixelFormat::Nv15
            && first.offset == 0
            && first.stride != 0
            && first.len % first.stride == 0
            && second.offset == first.len
            && second.len == first.len / 2
            && second.stride == first.stride;
        if !canonical {
            return Err(self);
        }
        if !matches!(
            self.storage,
            FrameStorage::DmaBuf(_) | FrameStorage::DecodedDmaBuf { .. }
        ) {
            return Err(self);
        }
        let mapping = match self.storage {
            FrameStorage::DmaBuf(buffer) => buffer,
            FrameStorage::DecodedDmaBuf { mapping, .. } => mapping,
            FrameStorage::Cpu(_) => unreachable!("DMA-BUF storage was checked above"),
        };
        Ok((mapping, first.stride, first.len / first.stride))
    }

    /// Convert between the supported 8-bit NV12/I420 layouts and planar
    /// 10-bit I420 using CPU-accessible storage.
    ///
    /// 8-bit values expand to 10 bits with `(v << 2) | (v >> 6)` and contract
    /// with `v >> 2`, so a round trip is lossless before clipping. The
    /// result owns a compact CPU frame; timestamps, POC and error metadata are
    /// preserved.
    pub fn convert_format(&self, format: PixelFormat) -> Result<Self, Error> {
        if format == self.format {
            return Ok(self.clone());
        }
        if matches!(self.format, PixelFormat::Nv15) || matches!(format, PixelFormat::Nv15) {
            return Err(Error::Unsupported(
                "packed 10-bit semi-planar conversion is not exposed by the SDK".to_string(),
            ));
        }
        let width = self.width;
        let height = self.height;
        let chroma_width = width.div_ceil(2);
        let chroma_height = height.div_ceil(2);
        let y_size = width * height;
        let chroma_size = chroma_width * chroma_height;

        let read_u8 = |index: usize, plane_width: usize, plane_height: usize| {
            let plane = self
                .plane(index)
                .ok_or(Error::State("source frame is missing a plane"))?;
            let mut compact = Vec::with_capacity(plane_width * plane_height);
            for row in 0..plane_height {
                let start = row * plane.stride;
                compact.extend_from_slice(&plane.data[start..start + plane_width]);
            }
            Ok::<_, Error>(compact)
        };
        let read_u16 = |index: usize, plane_width: usize, plane_height: usize| {
            let plane = self
                .plane(index)
                .ok_or(Error::State("source frame is missing a plane"))?;
            let mut compact = Vec::with_capacity(plane_width * plane_height);
            for row in 0..plane_height {
                for column in 0..plane_width {
                    let offset = row * plane.stride + column * 2;
                    compact.push(u16::from_le_bytes([
                        plane.data[offset],
                        plane.data[offset + 1],
                    ]));
                }
            }
            Ok::<_, Error>(compact)
        };

        let (y, u, v) = match self.format {
            PixelFormat::I420 => {
                let y = read_u8(0, width, height)?;
                let u = read_u8(1, chroma_width, chroma_height)?;
                let v = read_u8(2, chroma_width, chroma_height)?;
                (
                    y.into_iter().map(u16::from).collect(),
                    u.into_iter().map(u16::from).collect(),
                    v.into_iter().map(u16::from).collect(),
                )
            }
            PixelFormat::Nv12 => {
                let y = read_u8(0, width, height)?;
                let uv = read_u8(1, chroma_width * 2, chroma_height)?;
                let mut u = Vec::with_capacity(chroma_size);
                let mut v = Vec::with_capacity(chroma_size);
                for row in uv.chunks_exact(chroma_width * 2) {
                    for pair in row.chunks_exact(2) {
                        u.push(u16::from(pair[0]));
                        v.push(u16::from(pair[1]));
                    }
                }
                (y.into_iter().map(u16::from).collect(), u, v)
            }
            PixelFormat::I420P10 => (
                read_u16(0, width, height)?,
                read_u16(1, chroma_width, chroma_height)?,
                read_u16(2, chroma_width, chroma_height)?,
            ),
            PixelFormat::Nv15 => unreachable!("NV15 was rejected above"),
        };
        let source_is_ten_bit = self.format == PixelFormat::I420P10;
        let to_8bit = |sample: u16| {
            if source_is_ten_bit {
                contract_10bit(sample)
            } else {
                sample as u8
            }
        };
        let to_10bit = |sample: u16| {
            if source_is_ten_bit {
                sample
            } else {
                expand_8bit(sample)
            }
        };

        let bytes = match format {
            PixelFormat::I420 => {
                let mut bytes = Vec::with_capacity(y_size + chroma_size * 2);
                bytes.extend(y.into_iter().map(to_8bit));
                bytes.extend(u.into_iter().map(to_8bit));
                bytes.extend(v.into_iter().map(to_8bit));
                bytes
            }
            PixelFormat::Nv12 => {
                let mut bytes = Vec::with_capacity(y_size + chroma_size * 2);
                bytes.extend(y.into_iter().map(to_8bit));
                for (u, v) in u.into_iter().zip(v) {
                    bytes.push(to_8bit(u));
                    bytes.push(to_8bit(v));
                }
                bytes
            }
            PixelFormat::I420P10 => {
                let mut bytes = Vec::with_capacity((y_size + chroma_size * 2) * 2);
                for sample in y.into_iter().chain(u).chain(v) {
                    bytes.extend_from_slice(&to_10bit(sample).to_le_bytes());
                }
                bytes
            }
            PixelFormat::Nv15 => unreachable!("NV15 was rejected above"),
        };
        let mut frame = Self {
            width,
            height,
            format,
            pts: self.pts,
            dts: self.dts,
            poc: self.poc,
            err_info: self.err_info,
            storage: FrameStorage::Cpu(bytes),
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    /// Scale a strided NV12 frame directly into RGB888 model input.
    ///
    /// DMA-BUF frames are read in place; no whole-frame CPU NV12 or RGB
    /// intermediate is allocated.
    pub fn letterbox_nv12(
        &self,
        scaler: &mut crate::scale::Scaler,
        dst: &mut [u8],
    ) -> Result<crate::scale::Geometry, Error> {
        if self.format != PixelFormat::Nv12 {
            return Err(Error::Unsupported(
                "direct NV12 scaling requires an NV12 frame".to_string(),
            ));
        }
        let y = self
            .plane(0)
            .ok_or_else(|| Error::State("NV12 frame has no luma plane"))?;
        let uv = self
            .plane(1)
            .ok_or_else(|| Error::State("NV12 frame has no chroma plane"))?;
        Ok(scaler.letterbox_nv12_strided(y.data, uv.data, y.stride, self.width, self.height, dst))
    }

    fn from_h264(
        picture: h264::Picture,
        format: PixelFormat,
        pts: i64,
        dts: i64,
    ) -> Result<Self, Error> {
        let width = picture.width;
        let height = picture.height;
        let poc = picture.poc;
        let err_info = picture.err_info;
        if let Some(dmabuf) = picture.dmabuf {
            if format != PixelFormat::Nv12 {
                return Err(Error::Unsupported(
                    "H.264 DMA-BUF output requires NV12".to_string(),
                ));
            }
            return Self::from_nv12_dmabuf("H.264", width, height, poc, err_info, dmabuf, pts, dts);
        }
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
            PixelFormat::Nv15 => {
                return Err(Error::Unsupported(
                    "H.264 NV15 output is not implemented".to_string(),
                ));
            }
        };
        let mut frame = Self {
            width,
            height,
            format,
            pts,
            dts,
            poc,
            err_info,
            storage: FrameStorage::Cpu(storage),
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    fn from_nv12_dmabuf(
        codec: &str,
        width: usize,
        height: usize,
        poc: i32,
        err_info: u32,
        source: DmaBufSurface,
        pts: i64,
        dts: i64,
    ) -> Result<Self, Error> {
        let hor_stride = source.hor_stride;
        let ver_stride = source.ver_stride;
        let y_size = hor_stride
            .checked_mul(ver_stride)
            .ok_or_else(|| Error::Hardware(format!("{codec} DMA-BUF size overflow")))?;
        let uv_size = y_size / 2;
        let duplicate = source
            .dup_fd()
            .map_err(|error| Error::Resource(format!("{codec} fd duplicate: {error}")))?;
        let len = source
            .data_len()
            .map_err(|error| Error::Hardware(format!("{codec} surface mapping: {error}")))?;
        if len < y_size + uv_size {
            return Err(Error::Hardware(format!(
                "{codec} DMA-BUF surface is truncated"
            )));
        }
        let mapping = DmaBuf::import_fd(duplicate.as_raw_fd(), len, false).map_err(Error::from)?;
        mapping.sync_read_begin().map_err(Error::from)?;
        mapping.sync_read_end().map_err(Error::from)?;
        Ok(Self {
            width,
            height,
            format: PixelFormat::Nv12,
            pts,
            dts,
            poc,
            err_info,
            storage: FrameStorage::DecodedDmaBuf {
                mapping,
                surface: source,
            },
            planes: [
                PlaneLayout {
                    offset: 0,
                    len: y_size,
                    stride: hor_stride,
                },
                PlaneLayout {
                    offset: y_size,
                    len: uv_size,
                    stride: hor_stride,
                },
                PlaneLayout::default(),
            ],
        })
    }

    fn from_nv15_dmabuf(
        codec: &str,
        width: usize,
        height: usize,
        poc: i32,
        err_info: u32,
        source: DmaBufSurface,
        pts: i64,
        dts: i64,
    ) -> Result<Self, Error> {
        let hor_stride = source.hor_stride;
        let ver_stride = source.ver_stride;
        let y_size = hor_stride
            .checked_mul(ver_stride)
            .ok_or_else(|| Error::Hardware(format!("{codec} DMA-BUF size overflow")))?;
        let uv_size = y_size / 2;
        let duplicate = source
            .dup_fd()
            .map_err(|error| Error::Resource(format!("{codec} fd duplicate: {error}")))?;
        let len = source
            .data_len()
            .map_err(|error| Error::Hardware(format!("{codec} surface mapping: {error}")))?;
        if len < y_size + uv_size {
            return Err(Error::Hardware(format!(
                "{codec} DMA-BUF surface is truncated"
            )));
        }
        let mapping = DmaBuf::import_fd(duplicate.as_raw_fd(), len, false).map_err(Error::from)?;
        mapping.sync_read_begin().map_err(Error::from)?;
        mapping.sync_read_end().map_err(Error::from)?;
        Ok(Self {
            width,
            height,
            format: PixelFormat::Nv15,
            pts,
            dts,
            poc,
            err_info,
            storage: FrameStorage::DecodedDmaBuf {
                mapping,
                surface: source,
            },
            planes: [
                PlaneLayout {
                    offset: 0,
                    len: y_size,
                    stride: hor_stride,
                },
                PlaneLayout {
                    offset: y_size,
                    len: uv_size,
                    stride: hor_stride,
                },
                PlaneLayout::default(),
            ],
        })
    }

    fn from_h265(
        picture: hevc::Picture,
        format: PixelFormat,
        pts: i64,
        dts: i64,
    ) -> Result<Self, Error> {
        if let Some(dmabuf) = picture.dmabuf {
            if picture.bit_depth == 10 && picture.chroma_format == ChromaFormat::Yuv420 {
                return Self::from_nv15_dmabuf(
                    "H.265",
                    picture.width,
                    picture.height,
                    picture.poc,
                    picture.err_info,
                    dmabuf,
                    pts,
                    dts,
                );
            }
            if format != PixelFormat::Nv12 {
                return Err(Error::Unsupported(
                    "H.265 DMA-BUF output requires NV12".to_string(),
                ));
            }
            return Self::from_nv12_dmabuf(
                "H.265",
                picture.width,
                picture.height,
                picture.poc,
                picture.err_info,
                dmabuf,
                pts,
                dts,
            );
        }
        if picture.chroma_format != ChromaFormat::Yuv420 {
            return Err(Error::Unsupported(format!(
                "H.265 {:?} chroma output is not exposed by the V1 SDK",
                picture.chroma_format
            )));
        }
        let sample_bytes = match (picture.bit_depth, format) {
            (8, PixelFormat::Nv12 | PixelFormat::I420) => 1,
            (10, PixelFormat::I420P10) => 2,
            (10, PixelFormat::Nv15) => {
                return Err(Error::Unsupported(
                    "NV15 output requires a hardware DMA-BUF".to_string(),
                ));
            }
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
            storage: FrameStorage::Cpu(storage),
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
        if let Some(dmabuf) = picture.dmabuf {
            if picture.bit_depth == 10 {
                return Self::from_nv15_dmabuf(
                    "VP9",
                    picture.width,
                    picture.height,
                    picture.poc as i32,
                    picture.err_info,
                    dmabuf,
                    pts,
                    dts,
                );
            }
            if format != PixelFormat::Nv12 {
                return Err(Error::Unsupported(
                    "VP9 DMA-BUF output requires NV12".to_string(),
                ));
            }
            return Self::from_nv12_dmabuf(
                "VP9",
                picture.width,
                picture.height,
                picture.poc as i32,
                picture.err_info,
                dmabuf,
                pts,
                dts,
            );
        }
        let sample_bytes = match (picture.bit_depth, format) {
            (8, PixelFormat::Nv12 | PixelFormat::I420) => 1,
            (10, PixelFormat::I420P10) => 2,
            (10, PixelFormat::Nv15) => {
                return Err(Error::Unsupported(
                    "NV15 output requires a hardware DMA-BUF".to_string(),
                ));
            }
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
            storage: FrameStorage::Cpu(storage),
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
        if let Some(dmabuf) = picture.dmabuf {
            if picture.bit_depth == 10 {
                return Self::from_nv15_dmabuf(
                    "AV1",
                    picture.width,
                    picture.height,
                    picture.poc as i32,
                    picture.err_info,
                    dmabuf,
                    pts,
                    dts,
                );
            }
            if format != PixelFormat::Nv12 {
                return Err(Error::Unsupported(
                    "AV1 DMA-BUF output requires NV12".to_string(),
                ));
            }
            return Self::from_nv12_dmabuf(
                "AV1",
                picture.width,
                picture.height,
                picture.poc as i32,
                picture.err_info,
                dmabuf,
                pts,
                dts,
            );
        }
        let sample_bytes = match (picture.bit_depth, format) {
            (8, PixelFormat::Nv12 | PixelFormat::I420) => 1,
            (10, PixelFormat::I420P10) => 2,
            (10, PixelFormat::Nv15) => {
                return Err(Error::Unsupported(
                    "NV15 output requires a hardware DMA-BUF".to_string(),
                ));
            }
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
            storage: FrameStorage::Cpu(storage),
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    fn from_vp8(
        picture: vp8::decoder::Picture,
        format: PixelFormat,
        pts: i64,
        dts: i64,
        poc: i32,
    ) -> Result<Self, Error> {
        if !matches!(format, PixelFormat::Nv12 | PixelFormat::I420) {
            return Err(Error::Unsupported(
                "VP8 supports NV12 or I420 output".to_string(),
            ));
        }
        let expected = picture
            .width
            .checked_mul(picture.height)
            .and_then(|luma| luma.checked_mul(3).map(|bytes| bytes / 2))
            .ok_or_else(|| Error::State("VP8 frame size overflow"))?;
        if picture.yuv.len() < expected {
            return Err(Error::State("VP8 planar frame is truncated"));
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
            poc,
            err_info: picture.err_info,
            storage: FrameStorage::Cpu(storage),
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    fn from_legacy(
        codec: &str,
        width: usize,
        height: usize,
        yuv: Vec<u8>,
        err_info: u32,
        format: PixelFormat,
        source_format: PixelFormat,
        pts: i64,
        dts: i64,
        poc: i32,
    ) -> Result<Self, Error> {
        if !matches!(format, PixelFormat::Nv12 | PixelFormat::I420) {
            return Err(Error::Unsupported(format!(
                "{codec} supports NV12 or I420 output"
            )));
        }
        let expected = width
            .checked_mul(height)
            .and_then(|luma| luma.checked_mul(3).map(|bytes| bytes / 2))
            .ok_or_else(|| Error::State("legacy frame size overflow"))?;
        if yuv.len() < expected {
            return Err(Error::State("legacy planar frame is truncated"));
        }
        let storage = if format == source_format {
            if yuv.len() == expected {
                yuv
            } else {
                yuv[..expected].to_vec()
            }
        } else if format == PixelFormat::Nv12 {
            i420_to_nv12(&yuv[..expected], width, height)
        } else {
            return Err(Error::Unsupported(format!(
                "{codec} source {source_format:?} cannot become {format:?}"
            )));
        };
        let mut frame = Self {
            width,
            height,
            format,
            pts,
            dts,
            poc,
            err_info,
            storage: FrameStorage::Cpu(storage),
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
            storage: FrameStorage::Cpu(output),
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv()?;
        Ok(frame)
    }

    fn from_jpeg_dmabuf(
        width: usize,
        height: usize,
        buffer: DmaBuf,
        source_stride: usize,
        source_height: usize,
        pts: i64,
        dts: i64,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 || source_stride < width.div_ceil(2) * 2 {
            return Err(Error::Parse("invalid JPEG output geometry".to_string()));
        }
        let y_plane_size = source_stride
            .checked_mul(source_height)
            .ok_or_else(|| Error::Parse("JPEG Y plane overflow".to_string()))?;
        let chroma_height = height.div_ceil(2);
        let chroma_size = source_stride
            .checked_mul(chroma_height)
            .ok_or_else(|| Error::Parse("JPEG chroma plane overflow".to_string()))?;
        if y_plane_size
            .checked_add(chroma_size)
            .is_none_or(|size| size > buffer.len())
        {
            return Err(Error::Parse("truncated JPEG hardware output".to_string()));
        }
        Ok(Self {
            width,
            height,
            format: PixelFormat::Nv12,
            pts,
            dts,
            poc: 0,
            err_info: 0,
            storage: FrameStorage::DmaBuf(buffer),
            planes: [
                PlaneLayout {
                    offset: 0,
                    len: y_plane_size,
                    stride: source_stride,
                },
                PlaneLayout {
                    offset: y_plane_size,
                    len: chroma_size,
                    stride: source_stride,
                },
                PlaneLayout::default(),
            ],
        })
    }

    fn layout_yuv(&mut self) -> Result<(), Error> {
        if self.format == PixelFormat::Nv15 {
            return Err(Error::Unsupported(
                "NV15 is available only as hardware DMA-BUF storage".to_string(),
            ));
        }
        let chroma_width = self.width.div_ceil(2);
        let chroma_height = self.height.div_ceil(2);
        let sample_bytes = match self.format {
            PixelFormat::I420P10 => 2,
            PixelFormat::Nv12 | PixelFormat::I420 => 1,
            PixelFormat::Nv15 => unreachable!("NV15 was rejected above"),
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
            PixelFormat::Nv15 => unreachable!("NV15 was rejected above"),
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
    Legacy(LegacyBackend),
    H264(H264Backend),
    H265(H265Backend),
    Mjpeg(MjpegBackend),
    Vp9(Vp9Backend),
    Vp8(Vp8Backend),
}

struct H264Backend {
    decoder: h264::Decoder,
    pending: VecDeque<PendingH264>,
    input: Vec<u8>,
    scanner: AccessUnitScanner,
    output_format: PixelFormat,
    max_reorder_override: usize,
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
    dmabuf_output: bool,
    output_override: Option<DmaBuf>,
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

struct Vp8Backend {
    decoder: vp8::decoder::Decoder,
    format: PixelFormat,
    eos: bool,
    frame_number: u32,
}

struct LegacyBackend {
    decoder: LegacyDecoder,
    codec: Codec,
    format: PixelFormat,
    input: Vec<u8>,
    stream_pts: i64,
    stream_dts: i64,
    frame_number: u32,
    packet_decode: bool,
    eos: bool,
}

enum LegacyDecoder {
    Mpeg2(m2v::decoder::Decoder),
    Mpeg4(mpg4::decoder::Decoder),
    H263(h263::decoder::Decoder),
    Avs(avs::decoder::Decoder),
}

struct LegacyPicture {
    width: usize,
    height: usize,
    source_format: PixelFormat,
    yuv: Vec<u8>,
    err_info: u32,
}

impl From<m2v::decoder::Picture> for LegacyPicture {
    fn from(picture: m2v::decoder::Picture) -> Self {
        Self {
            width: picture.width,
            height: picture.height,
            source_format: PixelFormat::I420,
            yuv: picture.yuv,
            err_info: picture.err_info,
        }
    }
}

impl From<mpg4::decoder::Picture> for LegacyPicture {
    fn from(picture: mpg4::decoder::Picture) -> Self {
        Self {
            width: picture.width,
            height: picture.height,
            source_format: PixelFormat::I420,
            yuv: picture.yuv,
            err_info: picture.err_info,
        }
    }
}

impl From<h263::decoder::Picture> for LegacyPicture {
    fn from(picture: h263::decoder::Picture) -> Self {
        Self {
            width: picture.width,
            height: picture.height,
            source_format: PixelFormat::I420,
            yuv: picture.yuv,
            err_info: picture.err_info,
        }
    }
}

impl From<avs::decoder::Picture> for LegacyPicture {
    fn from(picture: avs::decoder::Picture) -> Self {
        Self {
            width: picture.width,
            height: picture.height,
            source_format: if picture.nv12 {
                PixelFormat::Nv12
            } else {
                PixelFormat::I420
            },
            yuv: picture.yuv,
            err_info: picture.err_info,
        }
    }
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

    fn set_output_dmabuf(&mut self, enabled: bool) {
        match self {
            Self::Vdpu34x(decoder) => decoder.set_output_dmabuf(enabled),
            Self::Vdpu383(decoder) => decoder.set_output_dmabuf(enabled),
        }
    }

    fn import_output_dmabuf(&mut self, buffer: DmaBuf) -> Result<(), String> {
        match self {
            Self::Vdpu34x(decoder) => decoder.import_output_dmabuf(buffer),
            Self::Vdpu383(decoder) => decoder.import_output_dmabuf(buffer),
        }
    }

    fn recycle_output(&mut self, buffer: Vec<u8>) {
        match self {
            Self::Vdpu34x(decoder) => decoder.recycle_output(buffer),
            Self::Vdpu383(decoder) => decoder.recycle_output(buffer),
        }
    }
}

impl Decoder {
    pub fn new(config: DecoderConfig) -> Result<Self, Error> {
        if config.vproc_output && config.codec != Codec::Avs {
            return Err(Error::Unsupported(
                "vproc output is currently implemented for AVS only".to_string(),
            ));
        }
        if config.vproc_output && config.dmabuf_output {
            return Err(Error::Unsupported(
                "vproc output does not support DMA-BUF frames".to_string(),
            ));
        }
        if config.dmabuf_output {
            if !matches!(
                config.codec,
                Codec::H264 | Codec::H265 | Codec::Mjpeg | Codec::Vp9 | Codec::Av1
            ) {
                return Err(Error::Unsupported(
                    "DMA-BUF frame output is currently implemented for H.264, H.265, VP9, AV1 and MJPEG"
                        .to_string(),
                ));
            }
            if config.output_format != PixelFormat::Nv12 {
                return Err(Error::Unsupported(
                    "H.264/H.265/VP9/AV1/MJPEG DMA-BUF output requires NV12".to_string(),
                ));
            }
        }
        let backend = match config.codec {
            Codec::H264 => Backend::H264(H264Backend::new(config)?),
            Codec::H265 => Backend::H265(H265Backend::new(config)?),
            Codec::Mjpeg => Backend::Mjpeg(MjpegBackend::new(config)?),
            Codec::Vp9 => Backend::Vp9(Vp9Backend::new(config)?),
            Codec::Av1 => Backend::Av1(Av1Backend::new(config)?),
            Codec::Vp8 => Backend::Vp8(Vp8Backend::new(config)?),
            Codec::Mpeg2 | Codec::Mpeg4 | Codec::H263 | Codec::Avs => {
                Backend::Legacy(LegacyBackend::new(config)?)
            }
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
            Backend::Vp8(backend) => backend.submit(packet, &mut self.frames),
            Backend::Legacy(backend) => backend.submit(packet, &mut self.frames),
        }
    }

    /// The unchanged MPP tools submit elementary streams in fixed-size packets
    /// and expect completed pictures before EOS. Legacy backends otherwise
    /// retain the whole stream, so this opt-in compatibility mode replays the
    /// accumulated bytes and emits only pictures not returned by an earlier
    /// packet.
    pub(crate) fn enable_legacy_packet_decode(&mut self) {
        if let Backend::Legacy(backend) = &mut self.backend {
            backend.packet_decode = true;
        }
    }

    pub fn receive_frame(&mut self) -> Option<Frame> {
        self.frames.pop_front()
    }

    /// Current SPS-derived display reorder depth once the stream has declared
    /// it. H.264/H.265 return their bounded reorder size; other codecs return
    /// zero because they emit display-ready frames directly.
    pub fn max_num_reorder_frames(&self) -> usize {
        match &self.backend {
            Backend::H264(backend) => backend.max_reorder,
            Backend::H265(backend) => backend.max_reorder,
            Backend::Mjpeg(_)
            | Backend::Vp9(_)
            | Backend::Av1(_)
            | Backend::Vp8(_)
            | Backend::Legacy(_) => 0,
        }
    }

    /// Return a consumed frame's host buffer to the decoder that produced it.
    pub fn recycle_frame(&mut self, frame: Frame) {
        let buffer = match frame.into_cpu_storage() {
            Some(buffer) => buffer,
            None => return,
        };
        match &mut self.backend {
            Backend::H264(backend) => backend.decoder.recycle_output(buffer),
            Backend::H265(backend) => backend.decoder.recycle_output(buffer),
            Backend::Vp9(backend) => backend.decoder.recycle_output(buffer),
            Backend::Av1(backend) => backend.decoder.recycle_output(buffer),
            _ => {}
        }
    }

    pub fn import_output_dmabuf(&mut self, buffer: DmaBuf) -> Result<(), Error> {
        let result = match &mut self.backend {
            Backend::H264(backend) => backend.decoder.import_output_dmabuf(buffer),
            Backend::H265(backend) => backend.decoder.import_output_dmabuf(buffer),
            Backend::Mjpeg(backend) => backend
                .import_output_dmabuf(buffer)
                .map_err(|error| error.to_string()),
            Backend::Vp9(backend) => backend.decoder.import_output_dmabuf(buffer),
            Backend::Av1(backend) => backend.decoder.import_output_dmabuf(buffer),
            Backend::Vp8(_) | Backend::Legacy(_) => {
                return Err(Error::Unsupported(
                    "output DMA-BUF injection is unavailable for this decoder".to_string(),
                ));
            }
        };
        result.map_err(Error::Hardware)
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
            Backend::Vp8(backend) => backend.eos,
            Backend::Legacy(backend) => backend.eos && backend.input.is_empty(),
        }
    }

    pub fn flush(&mut self) -> Result<usize, Error> {
        match &mut self.backend {
            Backend::H264(backend) => backend.flush(&mut self.frames),
            Backend::H265(backend) => backend.flush(&mut self.frames),
            Backend::Mjpeg(backend) => backend.flush(&mut self.frames),
            Backend::Vp9(backend) => backend.flush(),
            Backend::Av1(backend) => backend.flush(),
            Backend::Vp8(backend) => backend.flush(),
            Backend::Legacy(backend) => backend.flush(&mut self.frames),
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
            Backend::H265(_)
            | Backend::Mjpeg(_)
            | Backend::Vp9(_)
            | Backend::Av1(_)
            | Backend::Vp8(_)
            | Backend::Legacy(_) => self.reset(),
        }
    }

    pub fn status(&self) -> Option<(u32, u32, u32)> {
        match &self.backend {
            Backend::H264(backend) => Some(backend.decoder.status()),
            Backend::H265(backend) => Some(backend.decoder.status()),
            Backend::Mjpeg(_) => None,
            Backend::Vp9(backend) => Some(backend.decoder.status()),
            Backend::Av1(backend) => Some(backend.decoder.status()),
            Backend::Vp8(_) => None,
            Backend::Legacy(_) => None,
        }
    }
}

/// Ring depth for the SDK's in-process decoders. Three matches the CLI; the
/// EOS paths drain the ring before flushing the reorder buffer, and `flush()`
/// drains it for callers that do not send an EOS packet. `0` restores blocking
/// decode through `RKMPP_SDK_PIPELINE=0`.
fn sdk_pipeline_depth() -> usize {
    static DEPTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *DEPTH.get_or_init(|| {
        std::env::var("RKMPP_SDK_PIPELINE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(3)
    })
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
        let depth = sdk_pipeline_depth();
        if depth != 0 {
            decoder.set_pipeline_depth(true, depth);
        }
        decoder.set_output_nv12(config.output_format == PixelFormat::Nv12);
        decoder.set_output_dmabuf(config.dmabuf_output);
        let max_reorder_override = (config.max_reorder_frames as usize).min(16);
        let reorder_frames = if max_reorder_override == 0 {
            decoder.max_num_reorder_frames()
        } else {
            max_reorder_override
        };
        Ok(Self {
            decoder,
            pending: VecDeque::new(),
            input: Vec::new(),
            scanner: AccessUnitScanner::default(),
            output_format: config.output_format,
            max_reorder_override,
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
        self.scanner.push(&self.input);
        let flush_tail = packet.flags.contains(PacketFlags::COMPLETE_ACCESS_UNIT)
            || packet.flags.contains(PacketFlags::EOS);
        let units = self.scanner.take(&mut self.input, flush_tail);
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
            // The submission ring still holds the last pictures; drain it
            // before declaring the stream finished, otherwise an EOS packet
            // silently drops `depth - 1` frames.
            for picture in self.decoder.flush().map_err(classify_video_error)? {
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

    fn refresh_reorder(&mut self) {
        if self.max_reorder_override == 0 {
            self.max_reorder = self.decoder.max_num_reorder_frames();
        }
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
        self.scanner.push(&self.input);
        let units = self.scanner.take(&mut self.input, true);
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
                    PendingH264 {
                        picture,
                        pts: self.pending_pts,
                        dts: self.pending_dts,
                    },
                    output,
                )?;
            }
        }
        for picture in self.decoder.flush().map_err(classify_video_error)? {
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
        self.flush_reorder(output)?;
        self.eos = true;
        Ok(decoded)
    }
}

impl H265Backend {
    fn new(config: DecoderConfig) -> Result<Self, Error> {
        let mut decoder =
            hevc::Decoder::new().map_err(|error| Error::Hardware(error.to_string()))?;
        let depth = sdk_pipeline_depth();
        if depth != 0 {
            decoder.set_pipeline_depth(true, depth);
        }
        decoder.set_output_dmabuf(config.dmabuf_output);
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
            for picture in self.decoder.flush().map_err(classify_video_error)? {
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
        for picture in self.decoder.flush().map_err(classify_video_error)? {
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
        self.flush_reorder(output)?;
        self.eos = true;
        Ok(decoded)
    }
}

impl Vp9Backend {
    fn new(config: DecoderConfig) -> Result<Self, Error> {
        let mut decoder =
            vp9::decoder::Decoder::new().map_err(|error| Error::Hardware(error.to_string()))?;
        decoder.set_output_dmabuf(config.dmabuf_output);
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
        let mut decoder = match profile.video_decode {
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
        decoder.set_output_dmabuf(config.dmabuf_output);
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

impl Vp8Backend {
    fn new(config: DecoderConfig) -> Result<Self, Error> {
        if config.output_format == PixelFormat::I420P10 {
            return Err(Error::Unsupported(
                "VP8 supports NV12 or I420 output".to_string(),
            ));
        }
        Ok(Self {
            decoder: vp8::decoder::Decoder::new()
                .map_err(|error| Error::Hardware(error.to_string()))?,
            format: config.output_format,
            eos: false,
            frame_number: 0,
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
                    output.push_back(Frame::from_vp8(
                        picture,
                        self.format,
                        packet.pts,
                        packet.dts,
                        self.frame_number as i32,
                    )?);
                    self.frame_number = self.frame_number.saturating_add(1);
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

impl LegacyBackend {
    fn new(config: DecoderConfig) -> Result<Self, Error> {
        if config.output_format == PixelFormat::I420P10 {
            return Err(Error::Unsupported(
                "legacy MPEG/H.263/AVS decoders support NV12 or I420 output".to_string(),
            ));
        }
        let mut decoder = LegacyDecoder::new(config.codec)?;
        if config.vproc_output {
            let LegacyDecoder::Avs(decoder) = &mut decoder else {
                unreachable!("vproc output is restricted to AVS");
            };
            decoder.set_incremental_vproc(true);
        }
        if let LegacyDecoder::Avs(decoder) = &mut decoder {
            decoder.set_output_nv12(config.output_format == PixelFormat::Nv12);
        }
        Ok(Self {
            decoder,
            codec: config.codec,
            format: config.output_format,
            input: Vec::new(),
            stream_pts: 0,
            stream_dts: 0,
            frame_number: 0,
            packet_decode: false,
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
            self.stream_pts = packet.pts;
            self.stream_dts = packet.dts;
        }
        self.input.extend_from_slice(packet.data);
        let eos = packet.flags.contains(PacketFlags::EOS);
        if !eos && !self.packet_decode {
            return Ok(0);
        }
        let (pictures, consumed) = self
            .decoder
            .decode_incremental(&self.input, eos)
            .map_err(classify_video_error)?;
        if consumed != 0 {
            self.input.drain(..consumed);
        }
        let decoded = pictures.len();
        for picture in pictures {
            self.push_picture(picture, output)?;
        }
        if eos {
            self.input.clear();
            self.eos = true;
        }
        Ok(decoded)
    }

    fn flush(&mut self, output: &mut VecDeque<Frame>) -> Result<usize, Error> {
        if self.eos && self.input.is_empty() {
            return Ok(0);
        }
        let (pictures, consumed) = self
            .decoder
            .decode_incremental(&self.input, true)
            .map_err(classify_video_error)?;
        debug_assert_eq!(consumed, self.input.len());
        self.input.clear();
        let decoded = pictures.len();
        for picture in pictures {
            self.push_picture(picture, output)?;
        }
        self.eos = true;
        Ok(decoded)
    }

    fn push_picture(
        &mut self,
        picture: LegacyPicture,
        output: &mut VecDeque<Frame>,
    ) -> Result<(), Error> {
        let codec_name = match self.codec {
            Codec::Mpeg2 => "MPEG-2",
            Codec::Mpeg4 => "MPEG-4",
            Codec::H263 => "H.263",
            Codec::Avs => "AVS+",
            _ => unreachable!("LegacyBackend is constructed only for legacy codecs"),
        };
        output.push_back(Frame::from_legacy(
            codec_name,
            picture.width,
            picture.height,
            picture.yuv,
            picture.err_info,
            self.format,
            picture.source_format,
            self.stream_pts,
            self.stream_dts,
            self.frame_number as i32,
        )?);
        self.frame_number = self.frame_number.saturating_add(1);
        Ok(())
    }
}

impl LegacyDecoder {
    fn new(codec: Codec) -> Result<Self, Error> {
        match codec {
            Codec::Mpeg2 => Ok(Self::Mpeg2(
                m2v::decoder::Decoder::new().map_err(|error| Error::Hardware(error.to_string()))?,
            )),
            Codec::Mpeg4 => Ok(Self::Mpeg4(
                mpg4::decoder::Decoder::new()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            )),
            Codec::H263 => Ok(Self::H263(
                h263::decoder::Decoder::new()
                    .map_err(|error| Error::Hardware(error.to_string()))?,
            )),
            Codec::Avs => Ok(Self::Avs(
                avs::decoder::Decoder::new().map_err(|error| Error::Hardware(error.to_string()))?,
            )),
            _ => unreachable!("LegacyBackend is constructed only for legacy codecs"),
        }
    }

    fn decode_incremental(
        &mut self,
        data: &[u8],
        eos: bool,
    ) -> Result<(Vec<LegacyPicture>, usize), String> {
        match self {
            Self::Mpeg2(decoder) => {
                decoder
                    .decode_incremental(data, eos)
                    .map(|(pictures, consumed)| {
                        (
                            pictures.into_iter().map(LegacyPicture::from).collect(),
                            consumed,
                        )
                    })
            }
            Self::Mpeg4(decoder) => {
                decoder
                    .decode_incremental(data, eos)
                    .map(|(pictures, consumed)| {
                        (
                            pictures.into_iter().map(LegacyPicture::from).collect(),
                            consumed,
                        )
                    })
            }
            Self::H263(decoder) => {
                decoder
                    .decode_incremental(data, eos)
                    .map(|(pictures, consumed)| {
                        (
                            pictures.into_iter().map(LegacyPicture::from).collect(),
                            consumed,
                        )
                    })
            }
            Self::Avs(decoder) => {
                decoder
                    .decode_incremental(data, eos)
                    .map(|(pictures, consumed)| {
                        (
                            pictures.into_iter().map(LegacyPicture::from).collect(),
                            consumed,
                        )
                    })
            }
        }
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
            dmabuf_output: config.dmabuf_output,
            output_override: None,
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
        let output = match self.output_override.take() {
            Some(output) if output.len() >= geometry.frame_size => output,
            Some(_) => {
                return Err(Error::Resource(
                    "caller JPEG output buffer is too small".to_string(),
                ));
            }
            None => DmaBuf::alloc("system", geometry.frame_size, false).map_err(Error::from)?,
        };
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
        if self.dmabuf_output {
            output.sync_read_end().map_err(Error::from)?;
            Frame::from_jpeg_dmabuf(
                info.width,
                info.height,
                output,
                output_geometry.stride_bytes,
                output_geometry.out_height,
                pts,
                dts,
            )
        } else {
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

    fn import_output_dmabuf(&mut self, buffer: DmaBuf) -> Result<(), Error> {
        self.dmabuf_output = true;
        self.output_override = Some(buffer);
        Ok(())
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

fn expand_8bit(value: u16) -> u16 {
    let value = value & 0xff;
    (value << 2) | (value >> 6)
}

fn contract_10bit(value: u16) -> u8 {
    (value.min(1023) >> 2) as u8
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
    // Header-only split: access-unit boundaries need the NAL type and the
    // first payload byte, not a de-escaped RBSP. The full split de-escaped
    // every NAL on every call, which `perf` showed as ~11 % of a 720p run.
    let nals = h264::parser::split_annex_b_headers(buffer);
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
            1..=5 => nal.payload.first().is_some_and(|byte| byte & 0x80 != 0),
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
        extract_jpegs, take_h264_access_units, take_hevc_access_units, Codec, DecoderConfig, Frame,
        FrameStorage, PacketFlags, PixelFormat, PlaneLayout,
    };

    fn cpu_frame(width: usize, height: usize, format: PixelFormat, bytes: Vec<u8>) -> Frame {
        let mut frame = Frame {
            width,
            height,
            format,
            pts: 17,
            dts: 11,
            poc: 3,
            err_info: 9,
            storage: FrameStorage::Cpu(bytes),
            planes: [PlaneLayout::default(); 3],
        };
        frame.layout_yuv().unwrap();
        frame
    }

    #[test]
    fn pixel_formats_round_trip_through_c_values() {
        for format in [
            PixelFormat::Nv12,
            PixelFormat::I420,
            PixelFormat::I420P10,
            PixelFormat::Nv15,
        ] {
            assert_eq!(PixelFormat::from_c_value(format.c_value()), Some(format));
        }
    }

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
    fn legacy_frames_preserve_nv12_source_without_reinterleaving() {
        let packed = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let frame = super::Frame::from_legacy(
            "AVS+",
            4,
            2,
            packed.to_vec(),
            0,
            super::PixelFormat::Nv12,
            super::PixelFormat::Nv12,
            0,
            0,
            0,
        )
        .unwrap();
        assert_eq!(frame.data(), packed);
    }

    #[test]
    fn legacy_frames_convert_i420_source_to_nv12() {
        let planar = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let frame = super::Frame::from_legacy(
            "MPEG-2",
            4,
            2,
            planar.to_vec(),
            0,
            super::PixelFormat::Nv12,
            super::PixelFormat::I420,
            0,
            0,
            0,
        )
        .unwrap();
        assert_eq!(frame.data(), [1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 10, 12]);
    }

    #[test]
    fn frame_8bit_to_10bit_round_trips_i420_and_preserves_metadata() {
        let input: Vec<u8> = [0, 1, 64, 255, 32, 128, 96, 224]
            .into_iter()
            .chain([0, 255].into_iter())
            .chain([64, 128].into_iter())
            .collect();
        let frame = cpu_frame(4, 2, PixelFormat::I420, input.clone());
        let ten_bit = frame.convert_format(PixelFormat::I420P10).unwrap();
        assert_eq!(ten_bit.format(), PixelFormat::I420P10);
        assert_eq!(
            (
                ten_bit.width(),
                ten_bit.height(),
                ten_bit.pts(),
                ten_bit.dts()
            ),
            (4, 2, 17, 11)
        );
        assert_eq!((ten_bit.poc(), ten_bit.err_info()), (3, 9));
        let samples: Vec<u16> = ten_bit
            .data()
            .chunks_exact(2)
            .map(|sample| u16::from_le_bytes([sample[0], sample[1]]))
            .collect();
        assert_eq!(
            samples,
            [0, 4, 257, 1023, 128, 514, 385, 899, 0, 1023, 257, 514]
        );

        let eight_bit = ten_bit.convert_format(PixelFormat::I420).unwrap();
        assert_eq!(eight_bit.format(), PixelFormat::I420);
        assert_eq!(eight_bit.data(), input);
    }

    #[test]
    fn frame_nv12_to_i420p10_deinterleaves_and_expands_chroma() {
        let frame = cpu_frame(
            4,
            2,
            PixelFormat::Nv12,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 64, 255, 1, 128],
        );
        let ten_bit = frame.convert_format(PixelFormat::I420P10).unwrap();
        let samples: Vec<u16> = ten_bit
            .data()
            .chunks_exact(2)
            .map(|sample| u16::from_le_bytes([sample[0], sample[1]]))
            .collect();
        assert_eq!(samples, [4, 8, 12, 16, 20, 24, 28, 32, 257, 4, 1023, 514]);
        let eight_bit = ten_bit.convert_format(PixelFormat::Nv12).unwrap();
        assert_eq!(eight_bit.data(), [1, 2, 3, 4, 5, 6, 7, 8, 64, 255, 1, 128]);
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
            .with_max_reorder_frames(3)
            .with_vproc_output(false);
        assert_eq!(config.codec(), Codec::H265);
        assert_eq!(config.output_format(), PixelFormat::I420P10);
        assert_eq!(config.max_reorder_frames(), 3);
        assert!(!config.dmabuf_output());
        assert!(!config.vproc_output());
        assert!(DecoderConfig::new(Codec::Mjpeg)
            .with_dmabuf_output(true)
            .dmabuf_output());
        assert!(DecoderConfig::new(Codec::Avs)
            .with_vproc_output(true)
            .vproc_output());
        assert!(PacketFlags::empty().with_eos().contains(PacketFlags::EOS));
        assert_eq!(Codec::from_c_value(Codec::Avs.c_value()), Some(Codec::Avs));
    }

    #[test]
    fn dmabuf_output_rejects_unimplemented_codecs() {
        assert!(matches!(
            super::Decoder::new(
                super::DecoderConfig::new(super::Codec::H264).with_vproc_output(true)
            ),
            Err(super::Error::Unsupported(_))
        ));
        assert!(matches!(
            super::Decoder::new(
                super::DecoderConfig::new(super::Codec::Avs)
                    .with_vproc_output(true)
                    .with_dmabuf_output(true)
            ),
            Err(super::Error::Unsupported(_))
        ));
        assert!(matches!(
            super::Decoder::new(
                super::DecoderConfig::new(super::Codec::Vp8).with_dmabuf_output(true)
            ),
            Err(super::Error::Unsupported(_))
        ));
        assert!(matches!(
            super::Decoder::new(
                super::DecoderConfig::new(super::Codec::H265)
                    .with_output_format(super::PixelFormat::I420P10)
                    .with_dmabuf_output(true)
            ),
            Err(super::Error::Unsupported(_))
        ));
        assert!(matches!(
            super::Decoder::new(
                super::DecoderConfig::new(super::Codec::Vp9)
                    .with_output_format(super::PixelFormat::I420P10)
                    .with_dmabuf_output(true)
            ),
            Err(super::Error::Unsupported(_))
        ));
        assert!(matches!(
            super::Decoder::new(
                super::DecoderConfig::new(super::Codec::Av1)
                    .with_output_format(super::PixelFormat::I420P10)
                    .with_dmabuf_output(true)
            ),
            Err(super::Error::Unsupported(_))
        ));
    }
    #[test]
    fn incremental_scanner_matches_the_one_shot_splitter() {
        // Three pictures: SPS/PPS once, an IDR (first_mb_in_slice == 0), then
        // two P slices that also start a new access unit.
        let mut stream = Vec::new();
        let mut nal = |unit_type: u8, payload: &[u8]| {
            stream.extend_from_slice(&[0, 0, 1]);
            stream.push(unit_type);
            stream.extend_from_slice(payload);
        };
        nal(7, &[0x42, 1, 2, 3, 4, 5]);
        nal(8, &[0x68, 9]);
        nal(5, &[0x88, 1, 2, 3, 4, 5, 6]);
        nal(1, &[0x80, 7, 8, 9]);
        nal(1, &[0x80, 10, 11, 12]);
        let whole = stream.clone();

        let mut one_shot = stream.clone();
        let expected = super::take_h264_access_units(&mut one_shot, true);

        for chunk in [1usize, 2, 3, 5, 7, 313] {
            let mut buffer = Vec::new();
            let mut scanner = super::AccessUnitScanner::default();
            let mut got = Vec::new();
            for piece in stream.chunks(chunk) {
                buffer.extend_from_slice(piece);
                scanner.push(&buffer);
                got.extend(scanner.take(&mut buffer, false));
            }
            scanner.push(&buffer);
            got.extend(scanner.take(&mut buffer, true));
            assert_eq!(got, expected, "chunk {chunk}");
            assert_eq!(got.concat(), whole, "chunk {chunk} reassembles the stream");
        }
    }
    #[test]
    fn incremental_scanner_matches_a_real_stream() {
        let Ok(stream) = std::fs::read("/tmp/test720.264") else {
            return;
        };
        let mut one_shot = stream.clone();
        let expected = super::take_h264_access_units(&mut one_shot, true);
        for chunk in [313usize, 4096] {
            let mut buffer = Vec::new();
            let mut scanner = super::AccessUnitScanner::default();
            let mut got = Vec::new();
            for piece in stream.chunks(chunk) {
                buffer.extend_from_slice(piece);
                scanner.push(&buffer);
                got.extend(scanner.take(&mut buffer, false));
            }
            scanner.push(&buffer);
            got.extend(scanner.take(&mut buffer, true));
            assert_eq!(got.len(), expected.len(), "chunk {chunk} AU count");
            for (index, (a, b)) in got.iter().zip(&expected).enumerate() {
                assert_eq!(a, b, "chunk {chunk} AU {index}");
            }
        }
    }
}
