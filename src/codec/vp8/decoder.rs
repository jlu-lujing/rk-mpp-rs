//! End-to-end VP8 decode: IVF in, planar 4:2:0 out.
//!
//! [`super::parser::Parser`] produces the syntax and [`super::dpb::Dpb`] says
//! which surface each reference is, which leaves the mechanical part: copy the
//! packet into DMA memory, program the hardware from the header and the three
//! reference buffers, and hand the finished surfaces out.
//!
//! Output order needs no reordering. A picture with `show_frame` clear is a
//! hidden reference frame - the alternate reference a two-frame delay produces -
//! and the reference decoder simply never enqueues it to the display queue, so
//! the pictures leave this decoder in decode order, exactly as `mpi_dec_test`
//! collects them.
//!
//! The hardware starts where the header stops, so the stream base moves to the
//! 8-byte aligned byte in front of that position and the remaining bits go to
//! `sw122`; each DCT partition is a separate base on top of the same buffer.

use std::io;

use super::dpb::Dpb;
use super::parser::{self, Parser, Picture as Syntax};
use crate::codec::yuv::nv12_to_planar;
use crate::hal::vp8d::{PictureJob, Vp8d, IRQ_FAILURE};
use crate::service::dma_heap::DmaBuf;

/// MPP frame error flags.
pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

/// Zeroed bytes kept behind each packet. The C decoder copies the packet into a
/// buffer of its own and the variable length decoder reads past the end of the
/// picture, so the tail has to be readable.
const STREAM_PADDING: usize = 96;

/// One VDPU2 picture surface plus what the output needs to know about it.
struct Slot {
    frame: crate::hal::vdpu2::Frame,
    hardware_error: bool,
}

/// One displayed picture in tightly packed planar YUV 4:2:0.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub yuv: Vec<u8>,
    pub err_info: u32,
}

pub struct Decoder {
    hw: Vp8d,
    parser: Parser,
    dpb: Dpb,
    /// Frame buffers, indexed by the slot numbers the DPB hands out.
    slots: Vec<Option<Slot>>,
    stream: Option<DmaBuf>,
    /// `sw55` of the most recent job.
    pub last_irq: u32,
    /// Packets that arrived before the first key frame and were dropped.
    pub dropped_before_key_frame: usize,
}

impl Decoder {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            hw: Vp8d::open()?,
            parser: Parser::new(),
            dpb: Dpb::new(),
            slots: Vec::new(),
            stream: None,
            last_irq: 0,
            dropped_before_key_frame: 0,
        })
    }

    /// Decode a whole IVF file.
    pub fn decode_ivf(&mut self, data: &[u8]) -> Result<Vec<Picture>, String> {
        let mut output = Vec::new();
        for frame in parser::parse_ivf(data).map_err(|error| error.to_string())? {
            output.extend(self.decode_packet(frame.data)?);
        }
        for slot in self.dpb.flush() {
            self.slots[slot] = None;
        }
        Ok(output)
    }

    /// Decode one coded picture, which may produce no display picture at all
    /// when the frame tag says it is a hidden reference frame.
    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<Picture>, String> {
        let Some(picture) = self
            .parser
            .parse_picture(packet)
            .map_err(|error| error.to_string())?
        else {
            self.dropped_before_key_frame += 1;
            return Ok(Vec::new());
        };

        let coded_width = usize::try_from((picture.width + 15) & !15).unwrap_or(0);
        let coded_height = usize::try_from((picture.height + 15) & !15).unwrap_or(0);
        let slot = self
            .dpb
            .allocate(picture.show_frame)
            .map_err(|error| error.to_string())?;
        let current_fd = {
            if self.slots.len() == slot {
                self.slots.push(None);
            }
            if self.slots[slot].is_none() {
                let frame = crate::hal::vdpu2::Frame::alloc(coded_width, coded_height)
                    .map_err(|error| error.to_string())?;
                self.slots[slot] = Some(Slot {
                    frame,
                    hardware_error: false,
                });
            }
            self.slots[slot]
                .as_ref()
                .ok_or("VP8 frame buffer disappeared")?
                .frame
                .fd()
        };

        let refs = {
            let slots = &self.slots;
            let [last, golden, alternate] = self.dpb.references();
            let fd = |slot: Option<usize>| {
                slot.and_then(|slot| slots[slot].as_ref().map(|slot| slot.frame.fd()))
                    .unwrap_or(current_fd)
            };
            [fd(last), fd(golden), fd(alternate)]
        };

        let stream_fd = self.load_stream(packet)?;
        let job = PictureJob {
            mb_width: (picture.width + 15) >> 4,
            mb_height: (picture.height + 15) >> 4,
            frame_type: u32::from(!picture.key_frame),
            version: picture.version,
            loop_filter_type: u32::from(picture.loop_filter_type),
            loop_filter_level: u32::from(picture.loop_filter_level),
            sharpness: u32::from(picture.sharpness),
            mode_ref_lf_delta_enabled: picture.mode_ref_lf_delta_enabled,
            ref_lf_deltas: picture.ref_lf_deltas,
            mode_lf_deltas: picture.mode_lf_deltas,
            segmentation_enabled: picture.segmentation_enabled,
            segmentation_map_update: picture.segmentation_map_update,
            segment_feature_mode: picture.segment_feature_mode,
            segment_qp: picture.segment_qp,
            segment_loopfilter: picture.segment_loopfilter,
            y1ac_delta_q: picture.y1ac_delta_q,
            y1dc_delta_q: picture.y1dc_delta_q,
            y2dc_delta_q: picture.y2dc_delta_q,
            y2ac_delta_q: picture.y2ac_delta_q,
            uvdc_delta_q: picture.uvdc_delta_q,
            uvac_delta_q: picture.uvac_delta_q,
            mb_no_coeff_skip: picture.mb_no_coeff_skip,
            bool_value: u32::from(picture.bool_value),
            bool_range: u32::from(picture.bool_range),
            stream_start_offset: picture.stream_start_offset,
            stream_start_bit: picture.stream_start_bit,
            stream_end_pos: picture.stream_end_pos,
            frame_tag_size: picture.frame_tag_size,
            offset_to_dct_parts: picture.offset_to_dct_parts,
            log2_dct_partitions: picture.log2_dct_partitions,
            dct_partition_offsets: picture.dct_partition_offsets,
            entropy: picture.entropy,
            prob_skip_false: picture.prob_skip_false,
            prob_intra: picture.prob_intra,
            prob_last: picture.prob_last,
            prob_golden: picture.prob_golden,
            segment_tree_probs: picture.segment_tree_probs,
            sign_bias_golden: u32::from(picture.ref_frame_sign_bias_golden),
            sign_bias_altref: u32::from(picture.ref_frame_sign_bias_altref),
            stream_fd,
            cur_fd: current_fd,
            refs,
            probe_fd: self.hw.prob_fd(),
            seg_map_fd: self.hw.seg_map_fd(),
        };

        let irq = self.hw.decode(&job).map_err(|error| error.to_string())?;
        self.last_irq = irq;
        if let Some(slot_state) = self.slots[slot].as_mut() {
            slot_state.hardware_error = irq & IRQ_FAILURE != 0;
        }

        let mut output = Vec::new();
        if picture.show_frame {
            output.push(self.take_picture(slot, &picture)?);
        }
        let released = self.dpb.update(&picture);
        for index in released {
            self.slots[index] = None;
        }
        Ok(output)
    }

    /// Copy one displayed picture out of its DMA mapping. The frame buffer stays
    /// in the pool while any reference buffer still names it.
    fn take_picture(&self, slot: usize, picture: &Syntax) -> Result<Picture, String> {
        let slot_state = self.slots[slot]
            .as_ref()
            .ok_or("VP8 display picture has no frame buffer")?;
        let width = usize::try_from(picture.width).map_err(|_| "picture width")?;
        let height = usize::try_from(picture.height).map_err(|_| "picture height")?;
        let _ = slot_state.frame.texture.sync_read_begin();
        let yuv = nv12_to_planar(
            slot_state.frame.y_plane(),
            slot_state.frame.uv_plane(),
            width,
            height,
            slot_state.frame.hor_stride,
        );
        let _ = slot_state.frame.texture.sync_read_end();
        Ok(Picture {
            width,
            height,
            yuv,
            err_info: u32::from(slot_state.hardware_error) * FRAME_ERR_DEC_HW_ERR,
        })
    }

    /// Copy one packet into DMA memory and return its fd, reusing and growing the
    /// buffer across pictures and zeroing everything past the packet.
    fn load_stream(&mut self, packet: &[u8]) -> Result<u32, String> {
        let needed = packet.len() + STREAM_PADDING;
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
            window[..packet.len()].copy_from_slice(packet);
            let padding_end = (packet.len() + STREAM_PADDING).min(window.len());
            window[packet.len()..padding_end].fill(0);
        }
        buffer.sync_write_end().map_err(|error| error.to_string())?;
        Ok(fd)
    }
}
