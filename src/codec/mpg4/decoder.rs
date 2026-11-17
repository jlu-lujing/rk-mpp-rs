//! End-to-end MPEG-4 Part 2 decoder: elementary stream in, planar 4:2:0 out.
//!
//! [`parser::parse`] already resolves the video object layer state each picture
//! needs and hands back one access unit per video object plane, so this layer is
//! the mechanical part: copy the access unit into DMA memory, program the
//! hardware from the picture header and the object layer, and hand the finished
//! surfaces out in the order the reference rotation produces.
//!
//! The hardware starts decoding where the picture header stops, so the bitstream
//! base moves to the 8-byte aligned byte at or before that position and the
//! remaining bit offset goes into `sw122.sw_stream_start_word`.
//!
//! Output is planar 4:2:0 like every other decoder in this crate; the hardware
//! writes NV12 and [`crate::codec::yuv::nv12_to_planar`] converts it while
//! cropping the macroblock-aligned coded size down to the visible picture.

use std::io;

use crate::codec::mpg4::dpb::Dpb;
use crate::codec::mpg4::parser::{self, Item, Vol};
use crate::codec::yuv::nv12_to_planar;
use crate::hal::mpg4d::{
    Mpg4d, PictureJob, DEFAULT_INTER_MATRIX, DEFAULT_INTRA_MATRIX, IRQ_FAILURE,
};
use crate::service::dma_heap::DmaBuf;

/// MPP frame error flags.
pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

/// Zeroed bytes kept behind each access unit. `hal_m4vd_vdpu2.c` clears up to
/// `ALIGN(len, 16) + 64`, which covers the worst-case lookahead of the variable
/// length decoder past the end of the picture.
const STREAM_PADDING: usize = 96;

/// DivX build whose macroblock-size registers take the unaligned remainder.
const CUSTOM_VERSION_DIVX4: u32 = 4;

/// One VDPU2 picture surface plus the state the output needs.
struct Slot {
    frame: crate::hal::vdpu2::Frame,
    /// Visible picture size, which is smaller than the coded size.
    display_width: usize,
    display_height: usize,
    /// Set when the hardware reported an error interrupt.
    hardware_error: bool,
}

/// One displayed picture in tightly packed planar YUV 4:2:0.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub yuv: Vec<u8>,
    pub err_info: u32,
}

/// The `stream_offset` / `stream_start_bit` / `stream_len` triple the registers
/// want, from the access-unit length and the header length in bits.
fn stream_position(au_len: usize, header_bits: usize) -> (u32, u32, u32) {
    let offset = (header_bits >> 3) & !7;
    let start_bit = (header_bits & 63) as u32;
    ((offset as u32), start_bit, (au_len - offset) as u32)
}

pub struct Decoder {
    hw: Mpg4d,
    slots: Vec<Option<Slot>>,
    stream: Option<DmaBuf>,
    dpb: Dpb,
    /// `sw55` of the most recent job.
    pub last_irq: u32,
}

impl Decoder {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            hw: Mpg4d::open()?,
            slots: Vec::new(),
            stream: None,
            dpb: Dpb::new(),
            last_irq: 0,
        })
    }

    /// Decode a whole elementary stream, including the end-of-stream flush that
    /// releases the trailing reference picture.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<Picture>, String> {
        let mut output = Vec::new();
        self.decode_with(data, |picture| {
            output.push(picture);
            Ok(())
        })?;
        Ok(output)
    }

    /// Decode a whole elementary stream, handing every picture to `emit` as
    /// soon as it exists instead of building a `Vec`. A CLI that writes each
    /// picture immediately keeps only one frame of output alive, so the
    /// allocator reuses its pages instead of faulting in one untouched block
    /// per picture (`perf` puts ~28 % of a 120-frame run there).
    pub fn decode_with(
        &mut self,
        data: &[u8],
        mut emit: impl FnMut(Picture) -> Result<(), String>,
    ) -> Result<usize, String> {
        let mut emitted = 0usize;
        for item in parser::parse(data).map_err(|error| error.to_string())? {
            if let Item::Picture(picture) = item {
                for picture in self.decode_picture(&picture, data)? {
                    emit(picture)?;
                    emitted += 1;
                }
            }
        }
        for picture in self.flush()? {
            emit(picture)?;
            emitted += 1;
        }
        Ok(emitted)
    }

    /// Hand the pictures still held by the reference rotation to the caller.
    pub fn flush(&mut self) -> Result<Vec<Picture>, String> {
        let Decoder { slots, dpb, .. } = self;
        dpb.flush()
            .into_iter()
            .map(|buffer| take_picture(slots, buffer))
            .collect()
    }

    fn decode_picture(
        &mut self,
        picture: &parser::Picture,
        data: &[u8],
    ) -> Result<Vec<Picture>, String> {
        let vol = &picture.vol;
        if !self.dpb.accepts_picture(picture.vop.coding_type) {
            return Err(format!(
                "MPEG-4 B picture with {} reference pictures",
                usize::from(self.dpb.ref0.is_some()) + usize::from(self.dpb.ref1.is_some())
            ));
        }

        let buffer = self.reserve_buffer(vol)?;
        let current_fd = self.slots[buffer]
            .as_ref()
            .ok_or("MPEG-4 frame buffer disappeared")?
            .frame
            .fd();
        let refs = {
            let slots = &self.slots;
            self.dpb.reference_words(
                picture.vop.coding_type,
                |buffer| slots[buffer].as_ref().map_or(0, |slot| slot.frame.fd()),
                current_fd,
            )
        };

        let au = &data[picture.au.0..picture.au.1];
        let (stream_offset, stream_start_bit, stream_len) =
            stream_position(au.len(), picture.vop.header_bits);
        let stream_fd = self.load_stream(au)?;

        self.write_quantiser_tables(vol)?;

        let job = PictureJob {
            mb_width: vol.mb_width(),
            mb_height: vol.mb_height(),
            mb_width_off: u32::from(picture.custom_version == CUSTOM_VERSION_DIVX4)
                * (vol.width & 0xf),
            mb_height_off: u32::from(picture.custom_version == CUSTOM_VERSION_DIVX4)
                * (vol.height & 0xf),
            time_inc_resolution: vol.time_inc_resolution,
            quant_type: vol.quant_type,
            quarter_sample: vol.quarter_sample,
            alternate_vertical_scan: picture.vop.alternate_vertical_scan,
            rounding: picture.vop.rounding,
            top_field_first: picture.vop.top_field_first,
            interlaced: vol.interlaced,
            intra_dc_vlc_thr: picture.vop.intra_dc_vlc_thr,
            quant: picture.vop.quant,
            prev_pic_type: self.dpb.prev_pic_type(),
            coding_type: picture.vop.coding_type,
            fcode_forward: picture.vop.fcode_forward,
            fcode_backward: picture.vop.fcode_backward,
            time_bp: picture.vop.time_bp,
            time_pp: picture.vop.time_pp,
            stream_len,
            stream_offset,
            stream_start_bit,
            stream_fd,
            cur_fd: current_fd,
            refs,
            qp_fd: self.hw.qp_fd(),
            mv_fd: self.hw.mv_fd(),
        };

        let irq = self.hw.decode(&job).map_err(|error| error.to_string())?;
        self.last_irq = irq;
        if let Some(slot) = self.slots[buffer].as_mut() {
            slot.hardware_error = irq & IRQ_FAILURE != 0;
        }

        let Decoder { slots, dpb, .. } = self;
        dpb.update(picture.vop.coding_type, buffer)
            .iter()
            .map(|&buffer| take_picture(slots, buffer))
            .collect()
    }

    /// Load the intra and inter quantiser tables. A loaded matrix only reaches
    /// the hardware when the object layer said `quant_type`, and the intra one
    /// always starts at 8 because that is what the C parser writes.
    fn write_quantiser_tables(&mut self, vol: &Vol) -> Result<(), String> {
        let (intra, inter) = quantiser_tables(vol);
        self.hw
            .write_qp_table(&intra, &inter)
            .map_err(|error| error.to_string())
    }

    /// Take a frame buffer that no reference picture owns, reallocating it when
    /// the coded size changed. MPEG-4 needs at most three at a time, and a B
    /// picture's scratch buffer is the same one every time.
    fn reserve_buffer(&mut self, vol: &Vol) -> Result<usize, String> {
        let coded_width = usize::try_from(vol.mb_width()).unwrap_or(0) * 16;
        let coded_height = usize::try_from(vol.mb_height()).unwrap_or(0) * 16;
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
        let frame = crate::hal::vdpu2::Frame::alloc(coded_width, coded_height)
            .map_err(|error| error.to_string())?;
        let index = slots
            .iter()
            .position(Option::is_none)
            .unwrap_or(slots.len());
        let slot = Slot {
            frame,
            display_width: vol.width as usize,
            display_height: vol.height as usize,
            hardware_error: false,
        };
        if index == slots.len() {
            slots.push(Some(slot));
        } else {
            slots[index] = Some(slot);
        }
        Ok(index)
    }

    /// Copy one access unit into DMA memory and return its fd. The buffer is
    /// reused and grown across pictures, and everything past the access unit is
    /// zeroed, which is what the C HAL does before every job.
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

/// The two tables the hardware reads for one object layer. A matrix only counts
/// when the layer announced `quant_type` *and* the matching load flag; otherwise
/// the vdpu2 default stands, exactly as `vdpu2_mpg4d_setup_regs_by_syntax` picks
/// between `qm->Qmatrix` and `default_*_matrix`. `mpg4d_set_intra_matrix` writes
/// 8 over the first intra entry and leaves the inter table alone.
fn quantiser_tables(vol: &Vol) -> ([u8; 64], [u8; 64]) {
    let intra = if vol.quant_type && vol.load_intra_matrix {
        let mut table = vol.intra_matrix;
        table[0] = 8;
        table
    } else {
        DEFAULT_INTRA_MATRIX
    };
    let inter = if vol.quant_type && vol.load_inter_matrix {
        vol.inter_matrix
    } else {
        DEFAULT_INTER_MATRIX
    };
    (intra, inter)
}

/// Copy one displayed picture out of its DMA mapping. The frame buffer itself
/// stays in the pool for the next picture.
fn take_picture(slots: &[Option<Slot>], buffer: usize) -> Result<Picture, String> {
    let slot = slots[buffer]
        .as_ref()
        .ok_or("MPEG-4 display picture has no frame buffer")?;
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
        yuv,
        err_info: u32::from(slot.hardware_error) * FRAME_ERR_DEC_HW_ERR,
    })
}

#[cfg(test)]
mod tests {
    use super::{parser, stream_position};

    /// Per picture, in decode order: the `sw122` start word and `sw51` stream
    /// length the registers would carry, plus the quantiser, forward fcode and
    /// the B interpolation counters. Checked against the `hal_mpg4d_debug=1`
    /// dumps of the same streams on t6.
    ///
    /// The one deliberate difference from C is the first picture: the C splitter
    /// leaves the VOS, VOL and GOP headers in front of that access unit, which
    /// moves its start word to 35 and its length by the same 54 bytes. The
    /// absolute bit position the hardware decodes from is identical.
    fn positions(path: &str) -> Vec<(u32, u32, u32, u32, u32, u32, u32)> {
        let data = std::fs::read(path).expect("test fixture");
        parser::parse(&data)
            .expect("parse stream")
            .into_iter()
            .filter_map(|item| match item {
                parser::Item::Picture(picture) => {
                    let au = picture.au.1 - picture.au.0;
                    let (offset, start_bit, len) = stream_position(au, picture.vop.header_bits);
                    assert_eq!(offset as usize, (picture.vop.header_bits >> 3) & !7);
                    Some((
                        picture.vop.coding_type,
                        start_bit,
                        len,
                        picture.vop.quant,
                        picture.vop.fcode_forward,
                        picture.vop.time_bp,
                        picture.vop.time_pp,
                    ))
                }
                parser::Item::Vol(_) => None,
            })
            .collect()
    }

    #[test]
    fn ip_stream_positions_match_the_c_hal() {
        let mut rows = positions("tests/data/mp4_320x240_12_ip.m4v");
        let first = rows.remove(0);
        assert_eq!(first, (0, 51, 8755, 6, 0, 0, 0), "one I picture");
        let lengths: Vec<u32> = rows.iter().map(|row| row.2).collect();
        assert_eq!(
            lengths,
            [2548, 3302, 2906, 2782, 2810, 3544, 2913, 2982, 2484, 3132, 2458]
        );
        assert!(
            rows.iter()
                .all(|row| (row.0, row.1, row.3, row.4) == (1, 55, 6, 1)),
            "eleven P pictures at start word 55, quant 6, fcode 1"
        );
    }

    /// The B picture counters are what `sw137` to `sw139` are divided out of, so
    /// they have to agree with C to the unit.
    #[test]
    fn ibpb_stream_positions_match_the_c_hal() {
        let rows = positions("tests/data/mp4_320x240_12_ibpb.m4v");
        let coding: Vec<u32> = rows.iter().map(|row| row.0).collect();
        assert_eq!(
            coding,
            [
                parser::VOP_I,
                parser::VOP_P,
                parser::VOP_B,
                parser::VOP_B,
                parser::VOP_I,
                parser::VOP_B,
                parser::VOP_B,
                parser::VOP_P,
                parser::VOP_B,
                parser::VOP_B,
                parser::VOP_I,
                parser::VOP_B
            ]
        );
        let lengths: Vec<u32> = rows.iter().map(|row| row.2).collect();
        assert_eq!(
            lengths,
            [8755, 3793, 2021, 2196, 9172, 2418, 2533, 3925, 2131, 1915, 9694, 1868]
        );

        let ratios: Vec<(u32, u32)> = rows
            .iter()
            .filter(|row| row.0 == parser::VOP_B)
            .map(|row| (row.5, row.6))
            .collect();
        assert_eq!(
            ratios,
            [(1, 3), (2, 3), (1, 3), (2, 3), (1, 3), (2, 3), (1, 2)],
            "time_bp and time_pp as the C register file reports them"
        );
        assert_eq!(rows[1].4, 2, "the opening P picture reads fcode 2");
    }

    /// 150x100 is not a multiple of 16, so the coded size the hardware sees is
    /// 160x112 while the visible picture stays 150x100.
    #[test]
    fn odd_sizes_align_up_to_macroblocks() {
        let rows = positions("tests/data/mp4_150x100_9_ip.m4v");
        assert_eq!(rows.len(), 9);
        let data = std::fs::read("tests/data/mp4_150x100_9_ip.m4v").expect("test fixture");
        let vol = match parser::parse(&data).expect("parse").remove(0) {
            parser::Item::Vol(vol) => vol,
            parser::Item::Picture(_) => panic!("the stream opens with a picture"),
        };
        assert_eq!((vol.width, vol.height), (150, 100));
        assert_eq!((vol.mb_width() * 16, vol.mb_height() * 16), (160, 112));
        assert_eq!(vol.time_inc_resolution, 25, "the fixture is 25 fps");
        assert_eq!(vol.time_inc_bits, 5);
    }

    /// An N-VOP is not coded: the C parser drops it before the hardware sees it,
    /// and no reference rotation happens either.
    #[test]
    fn skipped_pictures_never_reach_the_hardware() {
        let data = std::fs::read("tests/data/mp4_320x240_12_ip.m4v").expect("test fixture");
        let pictures = parser::parse(&data)
            .expect("parse")
            .into_iter()
            .filter(|item| matches!(item, parser::Item::Picture(_)))
            .count();
        assert_eq!(pictures, 12);
        assert_eq!(
            parser::start_codes(&data)
                .into_iter()
                .filter(|(_, code)| *code == parser::CODE_VOP)
                .count(),
            pictures
        );
    }
}
