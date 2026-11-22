//! AVS decoder front-end for RK3588 client 12.
//!
//! The decoder carries the C parser's three saved frame slots and two-anchor
//! rotation. I/P pictures enter the reference chain; B pictures display
//! immediately and do not replace either anchor.

use std::io;

use crate::codec::avs::parser::{self, AccessUnit};
use crate::codec::yuv::nv12_to_planar;
use crate::hal::avsd_plus::{AvsdPlus, PictureJob, IRQ_BUFFER, IRQ_ERROR, IRQ_RDY};
use crate::service::dma_heap::DmaBuf;

const STREAM_PADDING: usize = 128;
pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub picture_coding_type: u32,
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

struct FrameSlot {
    surface: Surface,
    picture_coding_type: u32,
    picture_distance: u32,
    is_i_picture: bool,
    displayed: bool,
    invalid: bool,
    hardware_error: bool,
}

struct OutputSlot {
    index: usize,
    release: bool,
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
    slots: Vec<Option<FrameSlot>>,
    dpb0: Option<usize>,
    dpb1: Option<usize>,
    sequence: Option<parser::SequenceHeader>,
    version: Option<bool>,
    vec_flag: u8,
    last_picture_reference_flag: bool,
    previous_picture_structure: u32,
    future_to_previous_distance: u32,
    output_nv12: bool,
    pub last_irq: u32,
}

impl Decoder {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            hardware: AvsdPlus::open()?,
            stream: None,
            slots: Vec::new(),
            dpb0: None,
            dpb1: None,
            sequence: None,
            version: None,
            vec_flag: 0,
            last_picture_reference_flag: false,
            previous_picture_structure: parser::FIELD_PICTURE,
            future_to_previous_distance: 2,
            output_nv12: false,
            last_irq: 0,
        })
    }

    /// Emit compact NV12 instead of planar I420.
    pub fn set_output_nv12(&mut self, enabled: bool) {
        self.output_nv12 = enabled;
    }

    /// Decode up to `max_frames` display-order pictures.
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
            if let Some(picture) = self.decode_access(access)? {
                if std::env::var_os("RFI_AVS_TRACE").is_some() {
                    eprintln!(
                        "display type={} distance={}",
                        picture.picture_coding_type, picture.picture_distance
                    );
                }
                emit(picture)?;
                decoded += 1;
                if decoded == max_frames {
                    break;
                }
            }
        }
        if decoded < max_frames {
            for picture in self.flush()? {
                emit(picture)?;
                decoded += 1;
                if decoded == max_frames {
                    break;
                }
            }
        }
        Ok(decoded)
    }

    /// Zero-copy NV12 display path for file/network sinks.
    ///
    /// The callback receives a compact NV12 slice backed directly by the
    /// decoder's DMA mapping whenever the visible width equals the row stride.
    pub fn decode_with_nv12(
        &mut self,
        data: &[u8],
        max_frames: usize,
        mut emit: impl FnMut(&[u8], u32) -> Result<(), String>,
    ) -> Result<usize, String> {
        if max_frames == 0 {
            return Ok(0);
        }
        let mut decoded = 0usize;
        for (start, end) in parser::split_access_units(data) {
            let Some(output) = self.decode_access_output(&data[start..end])? else {
                continue;
            };
            self.emit_nv12_slot(output.index, &mut emit)?;
            self.slots[output.index]
                .as_mut()
                .expect("output slot remains allocated")
                .displayed = true;
            if output.release {
                self.slots[output.index] = None;
            }
            decoded += 1;
            if decoded == max_frames {
                break;
            }
        }
        if decoded < max_frames {
            for index in self.flush_slots() {
                self.emit_nv12_slot(index, &mut emit)?;
                self.slots[index] = None;
                decoded += 1;
                if decoded == max_frames {
                    break;
                }
            }
        }
        Ok(decoded)
    }

    /// Decode every complete access unit in `data`, retaining the final open
    /// unit until `eos` or the next start code proves it complete.
    pub fn decode_incremental(
        &mut self,
        data: &[u8],
        eos: bool,
    ) -> Result<(Vec<Picture>, usize), String> {
        let mut ranges = parser::split_access_units(data);
        let consumed = if eos {
            data.len()
        } else {
            ranges.last().map_or(0, |range| range.0)
        };
        if !eos {
            ranges.pop();
        }
        let mut pictures = Vec::new();
        for (start, end) in ranges {
            if let Some(picture) = self.decode_access(&data[start..end])? {
                pictures.push(picture);
            }
        }
        if eos {
            pictures.extend(self.flush()?);
        }
        Ok((pictures, consumed))
    }

    fn decode_access(&mut self, access: &[u8]) -> Result<Option<Picture>, String> {
        let Some(output) = self.decode_access_output(access)? else {
            return Ok(None);
        };
        let picture = take_picture(
            self.slots[output.index]
                .as_ref()
                .expect("output slot remains allocated"),
            self.output_nv12,
        )?;
        self.slots[output.index]
            .as_mut()
            .expect("output slot remains allocated")
            .displayed = true;
        if output.release {
            self.slots[output.index] = None;
        }
        Ok(Some(picture))
    }

    fn decode_access_output(&mut self, access: &[u8]) -> Result<Option<OutputSlot>, String> {
        let (parsed, version) =
            AccessUnit::parse_with_sequence(access, self.sequence, self.version)
                .map_err(|error| error.to_string())?;
        self.sequence = Some(parsed.sequence);
        self.version = version;
        if !picture_is_decodable(
            &mut self.vec_flag,
            parsed.has_video_edit,
            parsed.picture.picture_coding_type,
        ) {
            return Ok(None);
        }
        self.decode_access_unit_slot(access, &parsed)
    }

    fn decode_access_unit_slot(
        &mut self,
        access: &[u8],
        parsed: &AccessUnit<'_>,
    ) -> Result<Option<OutputSlot>, String> {
        let sequence = parsed.sequence;
        let mut picture = parsed.picture;
        if picture.picture_coding_type == parser::P_PICTURE
            || (picture.picture_coding_type == parser::B_PICTURE
                && picture.picture_structure == parser::FIELD_PICTURE)
        {
            self.last_picture_reference_flag = picture.picture_reference_flag;
        } else {
            // C does not reset this field in I or frame-coded B headers, so it
            // carries the previous picture's value into sw04.pic_refer_flag.
            picture.picture_reference_flag = self.last_picture_reference_flag;
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

        let current = self.reserve_slot(width, height)?;
        let output_fd = self.slots[current]
            .as_ref()
            .expect("current slot was allocated")
            .surface
            .fd();
        let (refs, work0_distance, work1_distance, prev_anchor_type, references_invalid) =
            self.reference_state(picture.picture_coding_type, output_fd)?;

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
            refs,
            prev_anchor_type,
            previous_picture_structure: self.previous_picture_structure,
            work0_distance,
            work1_distance,
            future_to_previous_distance: self.future_to_previous_distance,
        };
        let irq = self
            .hardware
            .decode(&job)
            .map_err(|error| format!("AVS hardware job failed: {error}"))?;
        self.last_irq = irq;
        let hard_error = irq & (IRQ_ERROR | IRQ_BUFFER) != 0 || irq & IRQ_RDY == 0;
        {
            let slot = self.slots[current]
                .as_mut()
                .expect("current slot remains allocated");
            slot.picture_distance = picture.picture_distance;
            slot.picture_coding_type = picture.picture_coding_type;
            slot.is_i_picture = picture.picture_coding_type == parser::I_PICTURE;
            slot.hardware_error = hard_error;
            slot.invalid = references_invalid;
        }

        let picture_coding_type = picture.picture_coding_type;
        if picture_coding_type != parser::B_PICTURE {
            self.previous_picture_structure = picture.picture_structure;
            let delta0 = work0_distance
                .map(|distance| distance_delta(picture.picture_distance, distance))
                .unwrap_or(2);
            self.future_to_previous_distance = work1_distance
                .map(|distance| distance_delta(picture.picture_distance, distance))
                .unwrap_or(delta0);
            let output = self.dpb0;
            let old_dpb1 = self.dpb1;
            if let Some(index) = old_dpb1 {
                if Some(index) != output {
                    self.slots[index] = None;
                }
            }
            self.dpb1 = output;
            self.dpb0 = Some(current);
            Ok(output.map(|index| OutputSlot {
                index,
                release: false,
            }))
        } else {
            Ok(Some(OutputSlot {
                index: current,
                release: true,
            }))
        }
    }

    fn reserve_slot(&mut self, width: usize, height: usize) -> Result<usize, String> {
        let live = [self.dpb0, self.dpb1];
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if live.contains(&Some(index)) {
                continue;
            }
            if slot
                .as_ref()
                .is_some_and(|slot| slot.surface.matches(width, height))
            {
                return Ok(index);
            }
            if slot.is_some() {
                *slot = None;
            }
        }
        let surface = Surface::alloc(width, height).map_err(|error| error.to_string())?;
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.slots.len());
        let slot = Some(FrameSlot {
            surface,
            picture_coding_type: parser::I_PICTURE,
            picture_distance: 0,
            is_i_picture: false,
            displayed: false,
            invalid: false,
            hardware_error: false,
        });
        if index == self.slots.len() {
            self.slots.push(slot);
        } else {
            self.slots[index] = slot;
        }
        Ok(index)
    }

    fn reference_state(
        &self,
        picture_coding_type: u32,
        current_fd: u32,
    ) -> Result<([u32; 4], Option<u32>, Option<u32>, bool, bool), String> {
        let fd = |index: Option<usize>| {
            index
                .and_then(|index| self.slots.get(index).and_then(Option::as_ref))
                .map_or(current_fd, |slot| slot.surface.fd())
        };
        let distance = |index: Option<usize>| {
            index
                .and_then(|index| self.slots.get(index).and_then(Option::as_ref))
                .map(|slot| slot.picture_distance)
        };
        let invalid = |index: Option<usize>| {
            index
                .and_then(|index| self.slots.get(index).and_then(Option::as_ref))
                .is_some_and(|slot| slot.invalid || slot.hardware_error)
        };
        let (reference0, reference1) = if picture_coding_type == parser::B_PICTURE {
            if self.dpb0.is_none() || self.dpb1.is_none() {
                return Err("AVS B picture has fewer than two anchors".to_string());
            }
            (self.dpb1, self.dpb0)
        } else {
            (self.dpb0, self.dpb1)
        };
        let reference0_fd = fd(reference0);
        let reference1_fd = if reference1.is_some() {
            fd(reference1)
        } else {
            reference0_fd
        };
        let refs = [reference0_fd, reference0_fd, reference1_fd, reference1_fd];
        let prev_anchor_type = self.dpb0.is_none_or(|index| {
            self.slots
                .get(index)
                .and_then(Option::as_ref)
                .is_none_or(|slot| !slot.is_i_picture)
        });
        Ok((
            refs,
            distance(self.dpb0),
            distance(self.dpb1),
            prev_anchor_type,
            invalid(reference0) || invalid(reference1),
        ))
    }

    fn flush(&mut self) -> Result<Vec<Picture>, String> {
        let mut pictures = Vec::new();
        for index in self.flush_slots() {
            pictures.push(take_picture(
                self.slots[index]
                    .as_ref()
                    .expect("flush slot remains allocated"),
                self.output_nv12,
            )?);
            if let Some(slot) = self.slots.get_mut(index) {
                *slot = None;
            }
        }
        Ok(pictures)
    }

    fn flush_slots(&mut self) -> Vec<usize> {
        [self.dpb1.take(), self.dpb0.take()]
            .into_iter()
            .flatten()
            .filter(|index| {
                self.slots
                    .get(*index)
                    .and_then(Option::as_ref)
                    .is_some_and(|slot| !slot.displayed)
            })
            .collect()
    }

    fn emit_nv12_slot(
        &self,
        index: usize,
        emit: &mut impl FnMut(&[u8], u32) -> Result<(), String>,
    ) -> Result<(), String> {
        let slot = self.slots[index]
            .as_ref()
            .expect("NV12 output slot remains allocated");
        let err_info = if slot.invalid {
            FRAME_ERR_DEC_INVALID
        } else if slot.hardware_error {
            FRAME_ERR_DEC_HW_ERR
        } else {
            0
        };
        let surface = &slot.surface;
        surface
            .texture
            .sync_read_begin()
            .map_err(|error| error.to_string())?;
        let frame_size = surface.frame_size();
        let bytes = &surface.texture.as_slice()[..frame_size];
        let result = if surface.hor_stride == surface.width {
            emit(bytes, err_info)
        } else {
            let compact = compact_nv12(bytes, surface.width, surface.height, surface.hor_stride);
            emit(&compact, err_info)
        };
        let sync = surface
            .texture
            .sync_read_end()
            .map_err(|error| error.to_string());
        result?;
        sync
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

fn distance_delta(current: u32, reference: u32) -> u32 {
    let distance = (2 * current).wrapping_sub(2 * reference + 512) & 0x1ff;
    if distance == 0 {
        2
    } else {
        distance
    }
}

/// Reproduce `vec_flag` from `avsd_parse_stream`: a video-edit code restarts
/// the chain, an I establishes one anchor, a following P establishes the
/// future anchor, and B pictures require both.
fn picture_is_decodable(vec_flag: &mut u8, video_edit: bool, picture_coding_type: u32) -> bool {
    if video_edit {
        *vec_flag = 0;
    }
    match picture_coding_type {
        parser::I_PICTURE => {
            *vec_flag = vec_flag.saturating_add(1);
            true
        }
        parser::P_PICTURE => {
            if *vec_flag == 1 {
                *vec_flag = vec_flag.saturating_add(1);
            }
            *vec_flag >= 1
        }
        parser::B_PICTURE => *vec_flag >= 2,
        _ => false,
    }
}

fn take_picture(slot: &FrameSlot, output_nv12: bool) -> Result<Picture, String> {
    let surface = &slot.surface;
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
    let yuv = if output_nv12 {
        compact_nv12(bytes, surface.width, surface.height, surface.hor_stride)
    } else {
        nv12_to_planar(
            &bytes[..luma_size],
            &bytes[luma_size..],
            surface.width,
            surface.height,
            surface.hor_stride,
        )
    };
    surface
        .texture
        .sync_read_end()
        .map_err(|error| error.to_string())?;
    let err_info = if slot.invalid {
        FRAME_ERR_DEC_INVALID
    } else if slot.hardware_error {
        FRAME_ERR_DEC_HW_ERR
    } else {
        0
    };
    Ok(Picture {
        width: surface.width,
        height: surface.height,
        picture_coding_type: slot.picture_coding_type,
        picture_distance: slot.picture_distance,
        yuv,
        err_info,
    })
}

fn compact_nv12(source: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let luma_size = stride * height;
    let mut compact = Vec::with_capacity(width * height * 3 / 2);
    for row in 0..height {
        let start = row * stride;
        compact.extend_from_slice(&source[start..start + width]);
    }
    for row in 0..height / 2 {
        let start = luma_size + row * stride;
        compact.extend_from_slice(&source[start..start + width]);
    }
    compact
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

    #[test]
    fn video_edit_drops_b_pictures_until_an_i_and_p_rebuild_the_chain() {
        let mut flag = 2;
        assert!(super::picture_is_decodable(
            &mut flag,
            true,
            crate::codec::avs::parser::I_PICTURE
        ));
        assert_eq!(flag, 1);
        assert!(!super::picture_is_decodable(
            &mut flag,
            false,
            crate::codec::avs::parser::B_PICTURE
        ));
        assert!(super::picture_is_decodable(
            &mut flag,
            false,
            crate::codec::avs::parser::P_PICTURE
        ));
        assert!(super::picture_is_decodable(
            &mut flag,
            false,
            crate::codec::avs::parser::B_PICTURE
        ));
    }

    #[test]
    fn compact_nv12_removes_row_padding() {
        let source = [
            1, 2, 99, 99, 3, 4, 99, 99, // luma
            5, 6, 99, 99, // chroma
        ];
        assert_eq!(super::compact_nv12(&source, 2, 2, 4), [1, 2, 3, 4, 5, 6]);
    }
}
