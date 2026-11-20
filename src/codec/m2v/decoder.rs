//! End-to-end MPEG-2 video decoder: elementary stream in, planar 4:2:0 out.
//!
//! The stream is cut into picture groups ([`parser::picture_groups`], the split
//! `mpp_m2vd_parser_split` performs) and each group is either a header run that
//! only updates sequence state, or one picture that goes to VDPU2. The hardware
//! wants the bitstream position of the first *slice* rather than of the picture,
//! so the header units in front of it are skipped with an 8-byte aligned base
//! plus a bit offset, and the hardware finds the slice boundaries itself.
//!
//! Output is planar because that is what the other decoders in this crate
//! produce and what the FFmpeg references are compared against; the hardware
//! writes NV12 and the conversion runs over the uncached mapping through
//! [`crate::codec::yuv::nv12_to_planar`].

use std::io;

use crate::codec::m2v::dpb::{self, Dpb, STRUCT_FRAME};
use crate::codec::m2v::parser::{self, PictureCodingExtension, Unit};
use crate::codec::yuv::nv12_to_planar;
use crate::hal::m2vd::{Frame, M2vd, PictureJob, IRQ_FAILURE};
use crate::service::dma_heap::DmaBuf;

/// MPP frame error flags.
pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

/// Zeroed bytes kept behind each picture, so a slice decoder that reads a word
/// past the last slice stays inside our own allocation.
const STREAM_PADDING: usize = 128;

/// `scanOrder[0]` of `m2vd_parser.c`: the raster index of zig-zag position `i`.
/// Quantiser matrices arrive in zig-zag order and the hardware table is raster
/// order, which is why every loaded matrix goes through this map.
const ZIGZAG_TO_RASTER: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// ISO/IEC 13818-2 default intra quantiser matrix, in raster order.
const DEFAULT_INTRA_MATRIX: [u8; 64] = [
    8, 16, 19, 22, 26, 27, 29, 34, 16, 16, 22, 24, 27, 29, 34, 37, 19, 22, 26, 27, 29, 34, 34, 38,
    22, 22, 26, 27, 29, 34, 37, 40, 22, 26, 27, 29, 32, 35, 40, 48, 26, 27, 29, 32, 35, 40, 48, 58,
    26, 27, 29, 34, 38, 46, 56, 69, 27, 29, 35, 38, 46, 56, 69, 83,
];

/// MPEG-2 falls back to a flat non-intra matrix.
const DEFAULT_INTER_MATRIX: [u8; 64] = [16; 64];

/// Reorder a matrix that was read in zig-zag order into the raster order the
/// quantiser table wants.
fn dezigzag(source: &[u8; 64]) -> [u8; 64] {
    let mut raster = [0u8; 64];
    for (index, &value) in source.iter().enumerate() {
        raster[ZIGZAG_TO_RASTER[index]] = value;
    }
    raster
}

/// Byte of the first slice's `00 00 01` prefix inside a picture group. The C
/// parser lands on the same byte by walking one access unit, and a slice that
/// somehow precedes its picture header does not start the picture.
fn slice_start_of(group: &[u8], units: &[Unit]) -> Option<usize> {
    let header = units
        .iter()
        .position(|unit| matches!(unit, Unit::Picture(_)))?;
    units.iter().skip(header + 1).find_map(|unit| match unit {
        // The payload points past the code byte, so the prefix is four back.
        Unit::Slice { data, .. } => {
            Some((data.as_ptr() as usize - group.as_ptr() as usize).saturating_sub(4))
        }
        _ => None,
    })
}

/// The `bitstream_offset` / `bitstream_start_bit` / `bitstream_length` triple
/// the registers want, from the group length and a [`slice_start_of`] byte. The
/// base is the 8-byte aligned byte under the first slice and the length runs to
/// the end of the group, which is what lets the hardware find the slice
/// boundaries itself.
fn stream_position(group_len: usize, slice_start: usize) -> (u32, u32, u32) {
    let offset = (slice_start & !7) as u32;
    let start_bit = ((slice_start * 8) & 63) as u32;
    let len = (group_len - offset as usize) as u32;
    (offset, start_bit, len)
}

/// Sequence level state carried between pictures.
#[derive(Debug, Clone)]
struct Sequence {
    /// Coded size aligned up to macroblocks, which is what the hardware decodes.
    decode_width: usize,
    decode_height: usize,
    /// Coded size before that alignment, which is what gets displayed.
    display_width: usize,
    display_height: usize,
    progressive_sequence: bool,
    /// `MPEG2_Flag`: set by the first sequence extension and never cleared
    /// again, exactly as the C parser leaves it.
    has_sequence_extension: bool,
    intra_matrix: [u8; 64],
    inter_matrix: [u8; 64],
}

impl Sequence {
    fn new() -> Self {
        Self {
            decode_width: 0,
            decode_height: 0,
            display_width: 0,
            display_height: 0,
            progressive_sequence: true,
            has_sequence_extension: false,
            intra_matrix: DEFAULT_INTRA_MATRIX,
            inter_matrix: DEFAULT_INTER_MATRIX,
        }
    }

    /// `horizontal_size_value` is 12 bits and the sequence extension that may
    /// follow it carries the two-bit extension. The C decoder ignores the
    /// extension for sizes, which caps it at 4095; folding it in here costs
    /// nothing on the streams C handles and covers 8K-sized MPEG-2.
    fn apply_sequence_header(&mut self, header: &parser::SequenceHeader) {
        let width = usize::try_from(header.horizontal_size_value).unwrap_or(usize::MAX);
        let height = usize::try_from(header.vertical_size_value).unwrap_or(usize::MAX);
        self.set_size(width, height);
        self.intra_matrix = if header.load_intra_quant_matrix {
            dezigzag(&header.intra_quant_matrix)
        } else {
            DEFAULT_INTRA_MATRIX
        };
        self.inter_matrix = if header.load_non_intra_quant_matrix {
            dezigzag(&header.non_intra_quant_matrix)
        } else {
            DEFAULT_INTER_MATRIX
        };
    }

    fn apply_sequence_extension(&mut self, extension: &parser::SequenceExtension) {
        self.has_sequence_extension = true;
        self.progressive_sequence = extension.progressive_sequence;
        if extension.horizontal_size_extension != 0 || extension.vertical_size_extension != 0 {
            let width = self
                .display_width
                .saturating_add((extension.horizontal_size_extension << 12) as usize);
            let height = self
                .display_height
                .saturating_add((extension.vertical_size_extension << 12) as usize);
            self.set_size(width, height);
        }
    }

    fn set_size(&mut self, width: usize, height: usize) {
        self.display_width = width;
        self.display_height = height;
        self.decode_width = width.div_ceil(16) * 16;
        self.decode_height = height.div_ceil(16) * 16;
    }

    fn apply_quant_matrix(&mut self, extension: &parser::QuantMatrixExtension) {
        if extension.load_intra_quant_matrix {
            self.intra_matrix = dezigzag(&extension.intra_quant_matrix);
        }
        if extension.load_non_intra_quant_matrix {
            self.inter_matrix = dezigzag(&extension.non_intra_quant_matrix);
        }
    }
}

/// One displayed picture in tightly packed planar YUV 4:2:0.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// `temporal_reference` of the picture header. It orders pictures inside a
    /// GOP, wraps at 512 and restarts at every sequence header, so it is
    /// diagnostic rather than a presentation timestamp.
    pub temporal_reference: u32,
    pub yuv: Vec<u8>,
    pub err_info: u32,
}

/// The picture level syntax one group carries.
struct PendingPicture {
    temporal_reference: u32,
    coding_type: u32,
    extension: Option<PictureCodingExtension>,
    /// Byte of the first slice's `00 00 01`, relative to the group start.
    slice_start: Option<usize>,
    /// `(full_pel, f_code)` per direction from the MPEG-1 picture header.
    header_f_code: [[u32; 2]; 2],
}

/// One VDPU2 picture surface plus the state the output needs.
struct Slot {
    frame: Frame,
    /// Coded size before macroblock alignment, which is the visible picture.
    display_width: usize,
    display_height: usize,
    temporal_reference: u32,
    /// Set when a reference picture was damaged.
    invalid: bool,
    /// Set when the hardware reported an error interrupt.
    hardware_error: bool,
}

pub struct Decoder {
    hw: M2vd,
    slots: Vec<Option<Slot>>,
    stream: Option<DmaBuf>,
    stream_fd: u32,
    sequence: Sequence,
    dpb: Dpb,
    /// `sw55` of the most recent job.
    pub last_irq: u32,
}

impl Decoder {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            hw: M2vd::open()?,
            slots: Vec::new(),
            stream: None,
            stream_fd: 0,
            sequence: Sequence::new(),
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

    /// Decode a whole stream, handing each picture to `emit` as soon as it
    /// exists so a CLI that writes immediately keeps one frame alive.
    pub fn decode_with(
        &mut self,
        data: &[u8],
        mut emit: impl FnMut(Picture) -> Result<(), String>,
    ) -> Result<usize, String> {
        let mut emitted = 0usize;
        for (start, end) in parser::picture_groups(data) {
            for picture in self.decode_group(&data[start..end])? {
                emit(picture)?;
                emitted += 1;
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
        let buffers = dpb.flush(&mut |_| ());
        let _ = slots;
        buffers
            .into_iter()
            .map(|buffer| take_picture(slots, buffer))
            .collect()
    }

    fn decode_group(&mut self, group: &[u8]) -> Result<Vec<Picture>, String> {
        let units = parser::parse_units(group).map_err(|error| error.to_string())?;
        let slice_start = slice_start_of(group, &units);
        let mut picture: Option<PendingPicture> = None;
        for unit in units {
            match unit {
                Unit::SequenceHeader(header) => self.sequence.apply_sequence_header(&header),
                Unit::SequenceExtension(extension) => {
                    if extension.chroma_format != 1 {
                        // The register file has no chroma stride, so the C HAL
                        // refuses every format but 4:2:0 and so do we.
                        return Err(format!(
                            "MPEG-2 chroma format {} is not supported",
                            extension.chroma_format
                        ));
                    }
                    self.sequence.apply_sequence_extension(&extension);
                }
                Unit::QuantMatrixExtension(extension) => {
                    self.sequence.apply_quant_matrix(&extension)
                }
                Unit::Picture(header) => {
                    picture = Some(PendingPicture {
                        temporal_reference: header.temporal_reference,
                        coding_type: header.picture_coding_type,
                        extension: None,
                        slice_start,
                        header_f_code: header.f_code(),
                    });
                }
                Unit::PictureCodingExtension(extension) => {
                    if let Some(picture) = picture.as_mut() {
                        picture.extension = Some(extension);
                    }
                }
                _ => {}
            }
        }
        match picture {
            None => Ok(Vec::new()),
            Some(picture) => self.decode_picture(group, &picture),
        }
    }

    fn decode_picture(
        &mut self,
        group: &[u8],
        picture: &PendingPicture,
    ) -> Result<Vec<Picture>, String> {
        let sequence = self.sequence.clone();
        if sequence.decode_width == 0 || sequence.decode_height == 0 {
            return Err("MPEG-2 picture before any sequence header".into());
        }
        if !self.dpb.accepts_picture(picture.coding_type) {
            return Err(format!(
                "B picture with only {} reference pictures",
                self.dpb.reference_count
            ));
        }
        let extension = picture.extension;
        let structure = extension.map_or(STRUCT_FRAME, |e| e.picture_structure);
        let top_field_first = extension.is_some_and(|e| e.top_field_first);

        let buffer = if dpb::starts_new_picture(structure, top_field_first) {
            let buffer = self.reserve_buffer(&sequence)?;
            // A half-decoded field pair leaves its buffer behind; the C parser
            // leaks it, we just let the pool hand it out again.
            let _orphan = self.dpb.allocate(picture.coding_type, buffer);
            buffer
        } else {
            self.dpb
                .current()
                .ok_or("MPEG-2 field picture without a frame buffer")?
        };
        let current_fd = self.slots[buffer]
            .as_ref()
            .ok_or("MPEG-2 frame buffer disappeared")?
            .frame
            .fd();

        let slice_start = picture.slice_start.ok_or("MPEG-2 picture has no slice")?;
        let (stream_offset, stream_start_bit, stream_len) =
            stream_position(group.len(), slice_start);
        let stream_fd = self.load_stream(group)?;

        let refs = {
            let slots = &self.slots;
            self.dpb
                .references(structure, top_field_first)
                .map(|reference| {
                    reference
                        .and_then(|buffer| slots[buffer].as_ref())
                        .map_or(current_fd, |slot| slot.frame.fd())
                })
        };

        self.hw
            .write_qp_table(&sequence.intra_matrix, &sequence.inter_matrix)
            .map_err(|error| error.to_string())?;

        // With a sequence extension the C parser replaces the picture header's
        // f_code fields with the extension's four-bit values, and the register
        // file is written from those.
        let f_code = match extension {
            Some(extension) if sequence.has_sequence_extension => extension.f_code,
            _ => picture.header_f_code,
        };
        let job = PictureJob {
            mb_width: (sequence.decode_width.div_ceil(16)) as u32,
            mb_height_p: (sequence.decode_height.div_ceil(16)) as u32,
            has_sequence_extension: sequence.has_sequence_extension,
            progressive_sequence: sequence.progressive_sequence,
            picture_structure: structure,
            top_field_first,
            picture_coding_type: picture.coding_type,
            intra_dc_precision: extension.map_or(0, |e| e.intra_dc_precision),
            q_scale_type: extension.is_some_and(|e| e.q_scale_type),
            intra_vlc_format: extension.is_some_and(|e| e.intra_vlc_format),
            concealment_motion_vectors: extension.is_some_and(|e| e.concealment_motion_vectors),
            alternate_scan: extension.is_some_and(|e| e.alternate_scan),
            frame_pred_frame_dct: extension.is_some_and(|e| e.frame_pred_frame_dct),
            forward_full_pel: f_code[0][0],
            forward_f_code: f_code[0][1],
            backward_full_pel: f_code[1][0],
            backward_f_code: f_code[1][1],
            stream_len,
            stream_offset,
            stream_start_bit,
            stream_fd,
            cur_fd: current_fd,
            // A bottom field starts one aligned luma row into the frame buffer.
            cur_offset: u32::from(structure == dpb::STRUCT_BOTTOM_FIELD)
                * sequence.decode_width as u32,
            refs,
            qp_fd: self.hw.qp_fd(),
        };

        let error_from_references =
            self.dpb.inherits_reference_errors() && self.dpb.references_in_error();
        let irq = self.hw.decode(&job).map_err(|error| error.to_string())?;
        self.last_irq = irq;
        if irq & IRQ_FAILURE != 0 {
            self.dpb.mark_current_error();
            if let Some(slot) = self.slots[buffer].as_mut() {
                slot.hardware_error = true;
            }
        }
        if error_from_references {
            self.dpb.mark_current_error();
            if let Some(slot) = self.slots[buffer].as_mut() {
                slot.invalid = true;
            }
        }
        if let Some(slot) = self.slots[buffer].as_mut() {
            slot.temporal_reference = picture.temporal_reference;
        }

        let Decoder { slots, dpb, .. } = self;
        let displayed = dpb.update(
            dpb::completes_picture(structure, top_field_first),
            &mut |_| (),
        );
        displayed
            .iter()
            .map(|&buffer| take_picture(slots, buffer))
            .collect()
    }

    /// Take a frame buffer that no reference picture holds, reallocating it when
    /// the coded size changed. MPEG-2 needs at most three at a time.
    fn reserve_buffer(&mut self, sequence: &Sequence) -> Result<usize, String> {
        let Decoder { slots, dpb, .. } = self;
        let reusable = slots
            .iter()
            .enumerate()
            .find(|(index, slot)| slot.is_some() && !dpb.is_live(*index))
            .map(|(index, _)| index);
        if let Some(index) = reusable {
            let matches = slots[index].as_ref().is_some_and(|slot| {
                slot.frame
                    .matches(sequence.decode_width, sequence.decode_height)
            });
            if matches {
                return Ok(index);
            }
            slots[index] = None;
        }
        let frame = Frame::alloc(sequence.decode_width, sequence.decode_height)
            .map_err(|error| error.to_string())?;
        let index = slots
            .iter()
            .position(Option::is_none)
            .unwrap_or(slots.len());
        let slot = Slot {
            frame,
            display_width: sequence.display_width,
            display_height: sequence.display_height,
            temporal_reference: 0,
            invalid: false,
            hardware_error: false,
        };
        if index == slots.len() {
            slots.push(Some(slot));
        } else {
            slots[index] = Some(slot);
        }
        Ok(index)
    }

    /// Copy one picture group into DMA memory and return its fd. The buffer is
    /// kept and grown across pictures, which is the same trick the C parser
    /// plays with `bitstream_sw_buf`.
    fn load_stream(&mut self, group: &[u8]) -> Result<u32, String> {
        let needed = group
            .len()
            .checked_add(STREAM_PADDING)
            .ok_or("MPEG-2 picture is too large")?;
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
            window[..group.len()].copy_from_slice(group);
            let padding_end = (group.len() + STREAM_PADDING).min(window.len());
            window[group.len()..padding_end].fill(0);
        }
        buffer.sync_write_end().map_err(|error| error.to_string())?;
        self.stream_fd = fd;
        Ok(fd)
    }
}

/// Copy one displayed picture out of its DMA mapping. The frame buffer itself
/// stays in the pool: once the rotation lets go of it, `reserve_buffer` hands it
/// to a later picture, which is what keeps a long stream down to three
/// allocations.
fn take_picture(slots: &[Option<Slot>], buffer: usize) -> Result<Picture, String> {
    let slot = slots[buffer]
        .as_ref()
        .ok_or("MPEG-2 display picture has no frame buffer")?;
    let _ = slot.frame.texture.sync_read_begin();
    let yuv = nv12_to_planar(
        slot.frame.y_plane(),
        slot.frame.uv_plane(),
        slot.display_width,
        slot.display_height,
        slot.frame.hor_stride,
    );
    let _ = slot.frame.texture.sync_read_end();
    let err_info = if slot.invalid {
        FRAME_ERR_DEC_INVALID
    } else if slot.hardware_error {
        FRAME_ERR_DEC_HW_ERR
    } else {
        0
    };
    Ok(Picture {
        width: slot.display_width,
        height: slot.display_height,
        temporal_reference: slot.temporal_reference,
        yuv,
        err_info,
    })
}

#[cfg(test)]
mod tests {
    use super::{parser, slice_start_of, stream_position};

    /// `sw122.stream_start_bit` and `sw51.stream_len` per submitted picture, in
    /// group order, as `hal_m2vd_debug=2` printed them on RK3588.
    fn positions(path: &str) -> Vec<(u32, u32)> {
        let data = std::fs::read(path).expect("test fixture");
        parser::picture_groups(&data)
            .into_iter()
            .filter_map(|(start, end)| {
                let group = &data[start..end];
                let units = parser::parse_units(group).expect("parse group");
                let slice_start = slice_start_of(group, &units)?;
                let (_offset, start_bit, len) = stream_position(group.len(), slice_start);
                Some((start_bit, len))
            })
            .collect()
    }

    #[test]
    fn picture_stream_positions_match_the_c_hal() {
        assert_eq!(
            positions("tests/data/m2v_320x240_12.m2v"),
            [
                (8, 8017),
                (16, 4757),
                (16, 573),
                (16, 1288),
                (16, 228),
                (8, 11430),
                (16, 2194),
                (16, 242),
                (16, 1143),
                (16, 239),
            ]
        );
    }

    /// Every picture of this stream carries its own sequence header, so the
    /// group walk sees three header runs that submit nothing before each
    /// picture. The C decoder skips them the same way.
    #[test]
    fn header_only_groups_submit_nothing() {
        let data = std::fs::read("tests/data/m2v_64x64_3.m2v").expect("test fixture");
        assert_eq!(parser::picture_groups(&data).len(), 6);
        assert_eq!(
            positions("tests/data/m2v_64x64_3.m2v"),
            [(8, 1760), (8, 2163), (8, 2165)]
        );
    }
}
