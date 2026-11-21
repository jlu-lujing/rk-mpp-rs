//! End-to-end H.263 decoder: elementary stream in, planar 4:2:0 out.
//!
//! [`parser::parse`] already cuts the stream into pictures and resolves the size
//! of each one, so this layer is the mechanical part: copy the access unit into
//! DMA memory, program the hardware from the picture header, and take the
//! finished surface back out. Because the format has no B pictures, that output
//! is in the same order the pictures were decoded in.
//!
//! The hardware starts where the picture header stops, so the stream base moves
//! to the 8-byte aligned byte at or before that position and the remaining bit
//! offset goes into `sw122.sw_stream_start_word`. Every H.263 header of a
//! standard-format stream is 50 bits, which lands on byte 6 and therefore on an
//! offset of zero; the arithmetic stays general because the extension bits in
//! front of the quantiser can push it further.
//!
//! Output is planar 4:2:0 like every other decoder in this crate; the hardware
//! writes NV12 and [`crate::codec::yuv::nv12_to_planar`] converts it while
//! cropping the macroblock-aligned coded size down to the visible picture.

use std::io;

use crate::codec::h263::dpb::Dpb;
use crate::codec::h263::parser;
use crate::codec::yuv::nv12_to_planar;
use crate::hal::h263d::{Frame, H263d, PictureJob, IRQ_FAILURE};
use crate::service::dma_heap::DmaBuf;

/// MPP frame error flags.
pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

/// Zeroed bytes kept behind each access unit. The C HAL adds none of its own -
/// the packet slots of the framework are just allocated generously - but the
/// variable length decoder reads whole 64-bit words and can run past the last
/// macroblock, so the padding keeps that read inside our own mapping.
const STREAM_PADDING: usize = 128;

/// Macroblock counts of a coded size, and the size the hardware actually writes.
fn coded_size(width: u32, height: u32) -> (usize, usize, u32, u32) {
    let mb_width = (width + 15) >> 4;
    let mb_height = (height + 15) >> 4;
    let coded_width = usize::try_from(mb_width).unwrap_or(usize::MAX) * 16;
    let coded_height = usize::try_from(mb_height).unwrap_or(usize::MAX) * 16;
    (coded_width, coded_height, mb_width, mb_height)
}

/// The `stream_offset` / `stream_start_bit` / `stream_len` triple the registers
/// want, from the access-unit length and the header length in bits.
fn stream_position(au_len: usize, header_bits: usize) -> (u32, u32, u32) {
    let offset = (header_bits >> 3) & !7;
    let start_bit = (header_bits & 63) as u32;
    (offset as u32, start_bit, (au_len - offset) as u32)
}

/// One VDPU2 picture surface plus the state the output needs.
struct Slot {
    frame: Frame,
    /// Visible picture size, which for SQCIF-to-4CIF standard formats equals the
    /// coded size but is kept separate all the same.
    display_width: usize,
    display_height: usize,
    /// Set when the hardware reported an error interrupt.
    hardware_error: bool,
}

/// One displayed picture in tightly packed planar YUV 4:2:0.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// `TR` of the picture header. It is the only trace of which source frame a
    /// picture stood for, and the encoder's rate control can skip numbers.
    pub temporal_reference: u32,
    pub coding_type: u32,
    pub yuv: Vec<u8>,
    pub err_info: u32,
}

pub struct Decoder {
    hw: H263d,
    slots: Vec<Option<Slot>>,
    stream: Option<DmaBuf>,
    dpb: Dpb,
    found_i_vop: bool,
    /// `sw55` of the most recent job.
    pub last_irq: u32,
}

impl Decoder {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            hw: H263d::open()?,
            slots: Vec::new(),
            stream: None,
            dpb: Dpb::new(),
            found_i_vop: false,
            last_irq: 0,
        })
    }

    /// Decode a whole elementary stream. No flush is needed at the end: every
    /// picture is displayed as it is decoded, so the head of the reference list
    /// is never a picture the caller has not been given already.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<Picture>, String> {
        let mut output = Vec::new();
        self.decode_with(data, |picture| {
            output.push(picture);
            Ok(())
        })?;
        Ok(output)
    }

    /// Decode complete pictures while retaining the final picture start until
    /// the next start code proves its end.
    pub fn decode_incremental(
        &mut self,
        data: &[u8],
        eos: bool,
    ) -> Result<(Vec<Picture>, usize), String> {
        let units = parser::access_units(data);
        let consumed = if eos {
            data.len()
        } else {
            units.last().map_or(0, |unit| unit.0)
        };
        let prefix = &data[..consumed];
        let mut output = Vec::new();
        for picture in parser::parse_with_state(prefix, &mut self.found_i_vop) {
            output.push(self.decode_picture(&picture, prefix)?);
        }
        Ok((output, consumed))
    }

    /// Decode a whole stream, handing each picture to `emit` as soon as it
    /// exists so a CLI that writes immediately keeps one frame alive.
    pub fn decode_with(
        &mut self,
        data: &[u8],
        mut emit: impl FnMut(Picture) -> Result<(), String>,
    ) -> Result<usize, String> {
        let mut emitted = 0usize;
        for picture in parser::parse(data) {
            emit(self.decode_picture(&picture, data)?)?;
            emitted += 1;
        }
        Ok(emitted)
    }

    fn decode_picture(
        &mut self,
        picture: &parser::Picture,
        data: &[u8],
    ) -> Result<Picture, String> {
        if picture.width == 0 || picture.height == 0 {
            return Err("H.263 picture without a size".into());
        }
        let (coded_width, coded_height, mb_width, mb_height) =
            coded_size(picture.width, picture.height);
        let buffer = self.reserve_buffer(coded_width, coded_height, picture)?;
        let current_fd = self.slots[buffer]
            .as_ref()
            .ok_or("H.263 frame buffer disappeared")?
            .frame
            .fd();
        // An intra picture, or an inter one with no head to read, points every
        // reference slot at the picture itself like the C HAL does.
        let ref_fd = self
            .dpb
            .reference(picture.coding_type)
            .and_then(|reference| self.slots[reference].as_ref())
            .map_or(current_fd, |slot| slot.frame.fd());

        let au = &data[picture.au.0..picture.au.1];
        let (stream_offset, stream_start_bit, stream_len) =
            stream_position(au.len(), picture.header_bits);
        let stream_fd = self.load_stream(au)?;

        let job = PictureJob {
            mb_width,
            mb_height,
            mb_width_off: picture.width & 0xf,
            mb_height_off: picture.height & 0xf,
            quant: picture.quant,
            coding_type: picture.coding_type,
            prev_pic_type: self.dpb.prev_pic_type(),
            stream_len,
            stream_offset,
            stream_start_bit,
            stream_fd,
            cur_fd: current_fd,
            ref_fd,
        };

        let irq = self.hw.decode(&job).map_err(|error| error.to_string())?;
        self.last_irq = irq;
        if let Some(slot) = self.slots[buffer].as_mut() {
            slot.hardware_error = irq & IRQ_FAILURE != 0;
        }
        self.dpb.update(picture.coding_type, buffer);

        take_picture(&self.slots, buffer, picture)
    }

    /// Take a frame buffer that no reference picture owns, reallocating it when
    /// the picture size changed. H.263 never needs more than two at a time.
    fn reserve_buffer(
        &mut self,
        coded_width: usize,
        coded_height: usize,
        picture: &parser::Picture,
    ) -> Result<usize, String> {
        let Decoder { slots, dpb, .. } = self;
        let reusable = slots
            .iter()
            .enumerate()
            .find(|(index, slot)| slot.is_some() && !dpb.is_live(*index))
            .map(|(index, _)| index);
        if let Some(index) = reusable {
            let matches = slots[index]
                .as_ref()
                .is_some_and(|slot| slot.frame.matches(coded_width, coded_height));
            if matches {
                return Ok(index);
            }
            slots[index] = None;
        }
        let frame = Frame::alloc(coded_width, coded_height).map_err(|error| error.to_string())?;
        let slot = Slot {
            frame,
            display_width: picture.width as usize,
            display_height: picture.height as usize,
            hardware_error: false,
        };
        let index = slots
            .iter()
            .position(Option::is_none)
            .unwrap_or(slots.len());
        if index == slots.len() {
            slots.push(Some(slot));
        } else {
            slots[index] = Some(slot);
        }
        Ok(index)
    }

    /// Copy one access unit into DMA memory and return its fd. The buffer is
    /// reused and grown across pictures, and everything past the access unit is
    /// zeroed.
    fn load_stream(&mut self, au: &[u8]) -> Result<u32, String> {
        let needed = au.len() + STREAM_PADDING;
        if self
            .stream
            .as_ref()
            .is_none_or(|buffer| buffer.len() < needed)
        {
            self.stream = None;
            self.stream =
                Some(DmaBuf::alloc("system", needed, false).map_err(|error| error.to_string())?);
        }
        let buffer = self
            .stream
            .as_mut()
            .expect("stream buffer was just created");
        let fd = buffer.fd() as u32;
        buffer
            .sync_write_begin()
            .map_err(|error| error.to_string())?;
        {
            let window = buffer.as_mut_slice();
            window[..au.len()].copy_from_slice(au);
            let padding_end = (au.len() + STREAM_PADDING).min(window.len());
            window[au.len()..padding_end].fill(0);
        }
        buffer.sync_write_end().map_err(|error| error.to_string())?;
        Ok(fd)
    }
}

/// Copy one displayed picture out of its DMA mapping. The frame buffer itself
/// stays in the pool for the next picture.
fn take_picture(
    slots: &[Option<Slot>],
    buffer: usize,
    picture: &parser::Picture,
) -> Result<Picture, String> {
    let slot = slots[buffer]
        .as_ref()
        .ok_or("H.263 display picture has no frame buffer")?;
    let _ = slot.frame.texture.sync_read_begin();
    let yuv = nv12_to_planar(
        slot.frame.y_plane(),
        slot.frame.uv_plane(),
        slot.display_width,
        slot.display_height,
        slot.frame.hor_stride,
    );
    let _ = slot.frame.texture.sync_read_end();
    Ok(Picture {
        width: slot.display_width,
        height: slot.display_height,
        temporal_reference: picture.temporal_reference,
        coding_type: picture.coding_type,
        yuv,
        err_info: u32::from(slot.hardware_error) * FRAME_ERR_DEC_HW_ERR,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 50-bit header ends inside byte 6, so the base does not move at all and
    /// the hardware reads to the end of the access unit. Both numbers are what
    /// `sw122` and `sw51` of the matching C job carry.
    #[test]
    fn a_fifty_bit_header_needs_no_base_move() {
        assert_eq!(stream_position(15_877, 50), (0, 50, 15_877));
        assert_eq!(stream_position(7570, 50), (0, 50, 7570));
    }

    /// Once the header runs past byte 8 the base has to move, and it moves in
    /// whole 8-byte steps because that is the alignment the hardware asks for.
    #[test]
    fn the_base_moves_in_eight_byte_steps() {
        assert_eq!(stream_position(20_000, 63), (0, 63, 20_000), "byte 7");
        assert_eq!(stream_position(20_000, 64), (8, 0, 19_992), "byte 8 moves");
        assert_eq!(stream_position(20_000, 72), (8, 8, 19_992), "byte 9");
        assert_eq!(stream_position(20_000, 200), (24, 8, 19_976), "byte 25");
    }

    #[test]
    fn coded_size_rounds_up_to_macroblocks() {
        assert_eq!(coded_size(352, 288), (352, 288, 22, 18));
        assert_eq!(coded_size(176, 144), (176, 144, 11, 9));
        assert_eq!(coded_size(704, 576), (704, 576, 44, 36));
        assert_eq!(coded_size(1408, 1152), (1408, 1152, 88, 72));
        assert_eq!(coded_size(128, 96), (128, 96, 8, 6));
    }

    /// The standard formats are all whole macroblocks, but the registers take the
    /// unaligned remainder too and the arithmetic has to hold for any size the
    /// hardware could be pointed at.
    #[test]
    fn an_unaligned_size_keeps_its_remainder() {
        assert_eq!(coded_size(150, 100), (160, 112, 10, 7));
        assert_eq!(150 & 0xf, 6);
        assert_eq!(100 & 0xf, 4);
    }
}
