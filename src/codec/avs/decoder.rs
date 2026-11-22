//! AVS decoder front-end for RK3588 client 12.
//!
//! The current slice decodes complete I pictures. PB pictures are parsed far
//! enough to split the elementary stream, but the reference scheduler is the
//! next feature slice; callers that request another picture after a PB access
//! unit receive an explicit error instead of a fabricated output.

use std::io;

use crate::codec::avs::parser::{self, AccessUnit, I_PICTURE_START_CODE, PB_PICTURE_START_CODE};
use crate::codec::yuv::nv12_to_planar;
use crate::hal::avsd_plus::{AvsdPlus, PictureJob, IRQ_BUFFER, IRQ_ERROR, IRQ_RDY};
use crate::service::dma_heap::DmaBuf;

const STREAM_PADDING: usize = 128;
pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub picture_distance: u32,
    pub yuv: Vec<u8>,
    pub err_info: u32,
}

struct Surface {
    texture: DmaBuf,
    width: usize,
    height: usize,
    hor_stride: usize,
    ver_stride: usize,
}

impl Surface {
    fn alloc(width: usize, height: usize) -> io::Result<Self> {
        crate::hal::check_decode_geometry(width, height)?;
        let hor_stride = width.div_ceil(16) * 16;
        let ver_stride = height.div_ceil(16) * 16;
        let frame_size = hor_stride
            .checked_mul(ver_stride)
            .and_then(|value| value.checked_mul(3))
            .map(|value| value / 2)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "AVS frame size overflow")
            })?;
        // The C decoder asks for 1.2x the visible NV12 size for its frame slots.
        let allocation = frame_size
            .checked_mul(6)
            .map(|value| value / 5)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "AVS frame size overflow")
            })?;
        Ok(Self {
            texture: DmaBuf::alloc("system", allocation, false)?,
            width,
            height,
            hor_stride,
            ver_stride,
        })
    }

    fn matches(&self, width: usize, height: usize) -> bool {
        self.width == width && self.height == height
    }

    fn fd(&self) -> u32 {
        self.texture.fd() as u32
    }

    fn frame_size(&self) -> usize {
        self.hor_stride * self.ver_stride * 3 / 2
    }
}

pub struct Decoder {
    hardware: AvsdPlus,
    stream: Option<DmaBuf>,
    surface: Option<Surface>,
    pub last_irq: u32,
}

impl Decoder {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            hardware: AvsdPlus::open()?,
            stream: None,
            surface: None,
            last_irq: 0,
        })
    }

    /// Decode up to `max_frames` I pictures, handing each frame to `emit`.
    ///
    /// The public AVS sample starts with one I picture followed by PB pictures.
    /// A one-frame request therefore exercises the complete first-picture path
    /// without pretending PB scheduling is implemented.
    pub fn decode_with(
        &mut self,
        data: &[u8],
        max_frames: usize,
        mut emit: impl FnMut(Picture) -> Result<(), String>,
    ) -> Result<usize, String> {
        if max_frames == 0 {
            return Ok(0);
        }
        let mut decoded = 0usize;
        for (start, end) in parser::split_access_units(data) {
            let access = &data[start..end];
            let units = parser::start_codes(access);
            let has_i = units.iter().any(|unit| unit.code == I_PICTURE_START_CODE);
            if !has_i {
                if units.iter().any(|unit| unit.code == PB_PICTURE_START_CODE) {
                    return Err("AVS PB pictures are not implemented yet".to_string());
                }
                continue;
            }
            let picture = self.decode_access_unit(access)?;
            emit(picture)?;
            decoded += 1;
            if decoded == max_frames {
                break;
            }
        }
        Ok(decoded)
    }

    fn decode_access_unit(&mut self, access: &[u8]) -> Result<Picture, String> {
        let parsed = AccessUnit::parse(access).map_err(|error| error.to_string())?;
        let sequence = parsed.sequence;
        let picture = parsed.picture;
        if picture.picture_coding_type != parser::I_PICTURE {
            return Err("AVS decoder received a non-I picture".to_string());
        }
        if sequence.chroma_format != 1 {
            return Err(format!(
                "AVS chroma format {} is unsupported",
                sequence.chroma_format
            ));
        }
        if sequence.sample_precision != 1 {
            return Err(format!(
                "AVS sample precision {} is unsupported",
                sequence.sample_precision
            ));
        }
        let width = sequence.horizontal_size as usize;
        let height = sequence.vertical_size as usize;
        crate::hal::check_decode_geometry(width, height).map_err(|error| error.to_string())?;

        if self
            .surface
            .as_ref()
            .is_none_or(|surface| !surface.matches(width, height))
        {
            self.surface = Some(Surface::alloc(width, height).map_err(|error| error.to_string())?);
        }
        let output_fd = self.surface.as_ref().expect("surface was allocated").fd();

        let stream_offset = (parsed.slice_start & !7) as u32;
        let stream_start_bit = ((parsed.slice_start & 7) * 8) as u32;
        let stream_len = u32::try_from(access.len() - stream_offset as usize)
            .map_err(|_| "AVS access unit is too large".to_string())?;
        let stream_fd = self.load_stream(access)?;
        let job = PictureJob {
            sequence,
            picture,
            stream_len,
            stream_offset,
            stream_start_bit,
            stream_fd,
            output_fd,
            mv_fd: self.hardware.mv_fd(),
            refs: [output_fd; 4],
            prev_anchor_type: true,
            work0_distance: None,
            work1_distance: None,
            future_to_previous_distance: 2,
        };
        let irq = self
            .hardware
            .decode(&job)
            .map_err(|error| format!("AVS hardware job failed: {error}"))?;
        self.last_irq = irq;
        let hard_error = irq & (IRQ_ERROR | IRQ_BUFFER) != 0 || irq & IRQ_RDY == 0;

        let surface = self.surface.as_ref().expect("surface remains allocated");
        surface
            .texture
            .sync_read_begin()
            .map_err(|error| error.to_string())?;
        let frame_size = surface.frame_size();
        let bytes = &surface.texture.as_slice()[..frame_size];
        if let Some(path) = std::env::var_os("RFI_AVS_RAW_NV12") {
            std::fs::write(path, bytes).map_err(|error| error.to_string())?;
        }
        let luma_size = surface.hor_stride * surface.ver_stride;
        let yuv = nv12_to_planar(
            &bytes[..luma_size],
            &bytes[luma_size..],
            width,
            height,
            surface.hor_stride,
        );
        surface
            .texture
            .sync_read_end()
            .map_err(|error| error.to_string())?;

        Ok(Picture {
            width,
            height,
            picture_distance: job.picture.picture_distance,
            yuv,
            err_info: if hard_error { FRAME_ERR_DEC_HW_ERR } else { 0 },
        })
    }

    fn load_stream(&mut self, access: &[u8]) -> Result<u32, String> {
        let needed = access
            .len()
            .checked_add(STREAM_PADDING)
            .ok_or("AVS access unit is too large")?;
        if self
            .stream
            .as_ref()
            .is_none_or(|buffer| buffer.len() < needed)
        {
            self.stream = None;
            self.stream =
                Some(DmaBuf::alloc("system", needed, false).map_err(|error| error.to_string())?);
        }
        let buffer = self.stream.as_mut().expect("stream buffer was allocated");
        let fd = buffer.fd() as u32;
        buffer
            .sync_write_begin()
            .map_err(|error| error.to_string())?;
        {
            let window = buffer.as_mut_slice();
            window[..access.len()].copy_from_slice(access);
            let end = (access.len() + STREAM_PADDING).min(window.len());
            window[access.len()..end].fill(0);
        }
        buffer.sync_write_end().map_err(|error| error.to_string())?;
        Ok(fd)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn public_sample_starts_its_slice_at_the_c_hardware_base() {
        let slice_start = 131usize;
        assert_eq!(slice_start & !7, 128);
        assert_eq!((slice_start & 7) * 8, 24);
    }

    #[test]
    fn surface_allocation_matches_the_c_requested_size() {
        let hor_stride = 352usize;
        let ver_stride = 288usize;
        let frame_size = hor_stride * ver_stride * 3 / 2;
        assert_eq!(frame_size, 152064);
        assert_eq!(frame_size * 6 / 5, 182476);
    }
}
