//! End-to-end AV1 decoder: IVF packets in, planar 8- or 10-bit YUV out.
//!
//! The hardware returns NV12 for 8-bit and Rockchip's packed 4:2:0 layout for
//! 10-bit. The public picture format stays planar so it can be compared
//! directly with libaom and FFmpeg YUV files.

use std::io;
use std::time::Instant;

use crate::codec::av1::assembler::{Av1CodedFrame, Av1PacketEvent, Av1PacketParser};
use crate::codec::av1::parser::{split_obus, SequenceHeader};
use crate::codec::av1::picparams::{Av1PicParams, PicParamsPointers};
use crate::codec::DmaBufSurface;
use crate::hal::av1d::{Av1RegisterState, Av1d, Frame, AV1_MAX_PIC_BUFFERS};
use crate::service::buffer_pool::BufferPool;
use crate::service::dma_heap::DmaBuf;

pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

#[derive(Debug, Clone, Copy)]
struct Reference {
    frame: usize,
    poc: u32,
    err_info: u32,
}

/// One displayed picture in tightly packed planar YUV420. Ten-bit samples are
/// little-endian 16-bit words containing their unscaled 10-bit value.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub bit_depth: u32,
    pub poc: u32,
    pub yuv: Vec<u8>,
    /// Strided hardware NV12 display surface when DMA-BUF output is enabled.
    pub dmabuf: Option<DmaBufSurface>,
    pub err_info: u32,
}

pub struct Decoder {
    parser: Av1PacketParser,
    hw: Av1d,
    frames: Vec<Option<Frame>>,
    tile_pool: BufferPool<DmaBuf>,
    display_pool: BufferPool<DmaBuf>,
    reference_slots: [Option<Reference>; 8],
    register_state: Av1RegisterState,
    next_poc: u32,
    output_dmabuf: bool,
    output_injection: Option<DmaBuf>,
    output_recycle: Vec<Vec<u8>>,
}

impl Decoder {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            parser: Av1PacketParser::new(),
            hw: Av1d::open()?,
            frames: Vec::new(),
            tile_pool: Frame::new_tile_pool(AV1_MAX_PIC_BUFFERS),
            display_pool: Frame::new_display_pool(AV1_MAX_PIC_BUFFERS),
            reference_slots: [None; 8],
            register_state: Av1RegisterState::default(),
            next_poc: 1,
            output_dmabuf: false,
            output_injection: None,
            output_recycle: Vec::new(),
        })
    }

    pub fn status(&self) -> (u32, u32, u32) {
        self.hw.last_status
    }

    /// Decode one temporal unit. One packet can contain zero, one or several
    /// final pictures.
    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<Picture>, String> {
        let mut output_injection = self.output_injection.take();
        self.decode_packet_inner(packet, &mut output_injection)
    }

    fn decode_packet_inner(
        &mut self,
        packet: &[u8],
        output_injection: &mut Option<DmaBuf>,
    ) -> Result<Vec<Picture>, String> {
        let trace = std::env::var_os("RK_MPP_AV1_TRACE").is_some();
        if trace {
            eprintln!(
                "av1-packet-before slot0={:016x} slot4={:016x}",
                cdf_hash(self.parser.frame_state().cdfs.load(0)),
                cdf_hash(self.parser.frame_state().cdfs.load(4)),
            );
        }
        let obus = split_obus(packet).map_err(|error| error.to_string())?;
        self.parser.begin_packet();
        let mut cursor = 0;
        if trace {
            eprintln!(
                "av1-packet-start slot0={:016x} slot4={:016x}",
                cdf_hash(self.parser.frame_state().cdfs.load(0)),
                cdf_hash(self.parser.frame_state().cdfs.load(4)),
            );
        }
        let mut output = Vec::new();
        while let Some(event) = self
            .parser
            .parse_next_event(&obus, &mut cursor)
            .map_err(|error| error.to_string())?
        {
            match event {
                Av1PacketEvent::ShowExisting(header) => {
                    output.push(self.show_existing(header.frame_to_show_map_idx as usize)?);
                }
                Av1PacketEvent::Coded(frame) => {
                    if let Some(picture) = self.decode_frame(&frame, packet, output_injection)? {
                        output.push(picture);
                    }
                }
            }
        }
        Ok(output)
    }

    fn decode_frame(
        &mut self,
        coded: &Av1CodedFrame,
        packet: &[u8],
        output_injection: &mut Option<DmaBuf>,
    ) -> Result<Option<Picture>, String> {
        if std::env::var_os("RK_MPP_AV1_TRACE").is_some() {
            eprintln!(
                "av1-entry order={} primary={} refresh={:#04x} slot4={:016x}",
                coded.header.order_hint,
                coded.header.primary_ref_frame,
                coded.header.refresh_frame_flags,
                cdf_hash(self.parser.frame_state().cdfs.load(4)),
            );
        }
        let sequence = self
            .parser
            .sequence()
            .cloned()
            .ok_or("missing AV1 sequence header")?;
        if self.output_dmabuf && sequence.bit_depth != 8 {
            return Err(format!(
                "AV1 DMA-BUF output supports only 8-bit NV12, got {}-bit",
                sequence.bit_depth
            ));
        }
        let poc = self.next_poc;
        self.next_poc = if self.next_poc == u32::MAX {
            1
        } else {
            self.next_poc + 1
        };

        let reference_error = self.reference_error(coded);
        let frame_index = self.allocate_frame(&sequence, coded, output_injection)?;
        let (current_slot, reference_texture_slots, hardware_reference_indices) =
            self.hardware_reference_map(coded, frame_index)?;
        let params = Av1PicParams::build(
            &sequence,
            &coded.header,
            &coded.references_at_header,
            &coded.cdf,
            &coded.tile_ranges,
            coded.frame_start,
            coded.frame_tag_size,
            current_slot,
            &reference_texture_slots,
            PicParamsPointers::default(),
        );
        if std::env::var_os("RK_MPP_AV1_TRACE").is_some() {
            let references: Vec<String> = (0..8)
                .map(|slot| {
                    let logical = coded.header.ref_frame_idx.get(slot).copied();
                    let physical = reference_texture_slots
                        .get(slot)
                        .copied()
                        .map(|value| format!("{value:02x}"))
                        .unwrap_or_else(|| "--".into());
                    let hint = coded.references_at_header[slot].order_hint;
                    format!("{slot}:{logical:?}/{physical}/oh{hint}")
                })
                .collect();
            eprintln!(
                "av1-trace frame={frame_index} poc={poc} type={} order={} show={} \
                 refresh={:#04x} slot={current_slot} start={} len={} cdf={:016x} refs=[{}]",
                coded.header.frame_type as u8,
                coded.header.order_hint,
                u8::from(coded.header.show_frame),
                coded.header.refresh_frame_flags,
                coded.frame_start,
                coded.stream_len,
                cdf_hash(&coded.cdf),
                references.join(","),
            );
        }
        if let Some(directory) = std::env::var_os("RK_MPP_AV1_PICPARAMS_DIR") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory)
                .map_err(|error| format!("create AV1 PicParams trace directory: {error}"))?;
            std::fs::write(
                directory.join(format!("picparams_{:03}.bin", poc.saturating_sub(1))),
                params.as_bytes(),
            )
            .map_err(|error| format!("write AV1 PicParams trace: {error}"))?;
        }

        let start = coded.frame_start as usize;
        let end = start
            .checked_add(coded.stream_len as usize)
            .filter(|end| *end <= packet.len())
            .ok_or("AV1 split frame exceeds packet")?;
        let stream = &packet[start..end];
        let frame_is_intra = matches!(coded.header.frame_type as u8, 0 | 2);

        let mut hardware_error = false;
        if !reference_error {
            hardware_error = {
                let Decoder {
                    hw,
                    frames,
                    register_state,
                    ..
                } = self;
                let mut hardware_references = [None; AV1_MAX_PIC_BUFFERS];
                for (destination, index) in hardware_references
                    .iter_mut()
                    .zip(hardware_reference_indices)
                {
                    *destination = index.and_then(|index| frames[index].as_ref());
                }
                let current = frames[frame_index]
                    .as_ref()
                    .ok_or("AV1 frame allocation failed")?;
                hw.decode(
                    &params,
                    &coded.cdf,
                    stream,
                    current,
                    &hardware_references,
                    register_state,
                )
                .map_err(|error| error.to_string())?;
                hw.last_hardware_error
            };
            if std::env::var_os("RK_MPP_AV1_TRACE").is_some() {
                eprintln!(
                    "av1-status frame={frame_index} status={:?} hardware_error={hardware_error}",
                    self.hw.last_status
                );
            }
            if !hardware_error && !coded.header.disable_frame_end_update_cdf {
                if std::env::var_os("RK_MPP_AV1_TRACE").is_some() {
                    eprintln!(
                        "av1-cdf-update frame={frame_index} poc={poc} refresh={:#04x} \
                         before_slot0={:016x} hardware={:016x}",
                        coded.header.refresh_frame_flags,
                        cdf_hash(self.parser.frame_state().cdfs.load(0)),
                        cdf_hash_from_bytes(&self.hw.last_probability),
                    );
                }
                self.parser.apply_hardware_cdf(
                    coded.header.refresh_frame_flags,
                    frame_is_intra,
                    &self.hw.last_probability,
                );
                if std::env::var_os("RK_MPP_AV1_TRACE").is_some() {
                    eprintln!(
                        "av1-cdf-updated frame={frame_index} poc={poc} \
                         after_slot0={:016x} after_slot4={:016x}",
                        cdf_hash(self.parser.frame_state().cdfs.load(0)),
                        cdf_hash(self.parser.frame_state().cdfs.load(4)),
                    );
                }
            }
        }

        let err_info = if reference_error {
            FRAME_ERR_DEC_INVALID
        } else if hardware_error {
            FRAME_ERR_DEC_HW_ERR
        } else {
            0
        };
        for slot in 0..8 {
            if coded.header.refresh_frame_flags & (1 << slot) != 0 {
                self.reference_slots[slot] = Some(Reference {
                    frame: frame_index,
                    poc,
                    err_info,
                });
            }
        }

        if !coded.header.show_frame {
            return Ok(None);
        }
        let reusable = (!self.output_dmabuf).then(|| self.output_recycle.pop().unwrap_or_default());
        let frame = self.frames[frame_index]
            .as_ref()
            .ok_or("AV1 output frame disappeared")?;
        let (yuv, dmabuf) = if err_info == 0 {
            if self.output_dmabuf {
                let lease = frame
                    .display
                    .try_clone()
                    .map_err(|error| format!("AV1 DMA-BUF lease: {error}"))?;
                (
                    Vec::new(),
                    Some(DmaBufSurface::pooled(
                        lease,
                        frame.display_hor_stride,
                        frame.display_ver_stride,
                    )),
                )
            } else {
                (copy_planar(frame, reusable.unwrap_or_default())?, None)
            }
        } else {
            (Vec::new(), None)
        };
        Ok(Some(Picture {
            width: frame.width,
            height: frame.height,
            bit_depth: frame.bit_depth,
            poc,
            yuv,
            dmabuf,
            err_info,
        }))
    }

    fn reference_error(&self, coded: &Av1CodedFrame) -> bool {
        if matches!(coded.header.frame_type as u8, 0 | 2) {
            return false;
        }
        coded
            .references_at_header
            .iter()
            .enumerate()
            .any(|(slot, reference)| {
                reference.valid
                    && self.reference_slots[slot].is_none_or(|mapped| mapped.err_info != 0)
            })
    }

    fn allocate_frame(
        &mut self,
        sequence: &SequenceHeader,
        coded: &Av1CodedFrame,
        output_injection: &mut Option<DmaBuf>,
    ) -> Result<usize, String> {
        let injected_display = if coded.header.show_frame {
            if let Some(buffer) = output_injection.take() {
                self.display_pool.clear();
                self.display_pool
                    .import(buffer)
                    .map_err(|error| error.to_string())?;
                true
            } else {
                false
            }
        } else {
            false
        };
        let live: Vec<usize> = self
            .reference_slots
            .iter()
            .flatten()
            .map(|reference| reference.frame)
            .collect();
        let max_width = sequence.max_frame_width as usize;
        let max_height = sequence.max_frame_height as usize;
        let width = coded.header.width as usize;
        let height = coded.header.height as usize;
        let bit_depth = u32::from(sequence.bit_depth);
        for (index, frame) in self.frames.iter_mut().enumerate() {
            if !live.contains(&index) {
                *frame = None;
            }
        }
        let reusable = self.frames.iter().enumerate().find_map(|(index, frame)| {
            let matches_layout = frame.as_ref().is_some_and(|frame| {
                frame.max_width == max_width
                    && frame.max_height == max_height
                    && frame.bit_depth == bit_depth
            });
            (!live.contains(&index) && matches_layout).then_some(index)
        });
        if let Some(index) = reusable {
            let needs_reallocation = self.frames[index].as_ref().is_some_and(|frame| {
                frame.width != width
                    || frame.height != height
                    || frame.display_hor_stride < width
                    || frame.display_ver_stride < height
            });
            if needs_reallocation {
                let texture_index = self.frames[index]
                    .as_ref()
                    .map_or(index as u8, |frame| frame.texture_index);
                self.frames[index] = Some(
                    Frame::acquire(
                        &self.tile_pool,
                        &self.display_pool,
                        max_width,
                        max_height,
                        width,
                        height,
                        bit_depth,
                        texture_index,
                    )
                    .map_err(|error| error.to_string())?,
                );
            } else if injected_display {
                self.frames[index]
                    .as_mut()
                    .ok_or("AV1 reusable frame disappeared")?
                    .replace_display(&self.display_pool)
                    .map_err(|error| error.to_string())?;
            }
            return Ok(index);
        }

        let texture_index = self
            .frames
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.frames.len());
        if texture_index >= AV1_MAX_PIC_BUFFERS {
            return Err("AV1 DPB exhausted all 24 texture slots".into());
        }
        let frame = Frame::acquire(
            &self.tile_pool,
            &self.display_pool,
            max_width,
            max_height,
            width,
            height,
            bit_depth,
            texture_index as u8,
        )
        .map_err(|error| error.to_string())?;
        if texture_index == self.frames.len() {
            self.frames.push(Some(frame));
        } else {
            self.frames[texture_index] = Some(frame);
        }
        Ok(texture_index)
    }

    #[allow(clippy::type_complexity)]
    fn hardware_reference_map(
        &self,
        coded: &Av1CodedFrame,
        current_frame: usize,
    ) -> Result<(u8, [u8; 8], [Option<usize>; AV1_MAX_PIC_BUFFERS]), String> {
        let current_slot = self.frames[current_frame]
            .as_ref()
            .ok_or("AV1 current frame missing")?
            .texture_index;
        let mut physical_slots = [0x7f; 8];
        let mut hardware = [None; AV1_MAX_PIC_BUFFERS];
        for slot in 0..8 {
            if !coded.references_at_header[slot].valid {
                continue;
            }
            let Some(reference) = self.reference_slots[slot] else {
                continue;
            };
            let frame = self
                .frames
                .get(reference.frame)
                .and_then(Option::as_ref)
                .ok_or("AV1 reference points to a released frame")?;
            physical_slots[slot] = frame.texture_index;
            hardware[frame.texture_index as usize] = Some(reference.frame);
        }
        Ok((current_slot, physical_slots, hardware))
    }

    fn show_existing(&mut self, slot: usize) -> Result<Picture, String> {
        if self.output_dmabuf
            && self
                .parser
                .sequence()
                .is_some_and(|sequence| sequence.bit_depth != 8)
        {
            return Err("AV1 DMA-BUF output supports only 8-bit NV12".to_string());
        }
        let reference = self
            .reference_slots
            .get(slot)
            .and_then(|reference| *reference)
            .ok_or_else(|| format!("AV1 show-existing slot {slot} is empty"))?;
        let reusable = (!self.output_dmabuf).then(|| self.output_recycle.pop().unwrap_or_default());
        let frame = self
            .frames
            .get(reference.frame)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("AV1 show-existing slot {slot} points to a released frame"))?;
        let (yuv, dmabuf) = if reference.err_info == 0 {
            if self.output_dmabuf {
                let lease = frame
                    .display
                    .try_clone()
                    .map_err(|error| format!("AV1 DMA-BUF lease: {error}"))?;
                (
                    Vec::new(),
                    Some(DmaBufSurface::pooled(
                        lease,
                        frame.display_hor_stride,
                        frame.display_ver_stride,
                    )),
                )
            } else {
                (copy_planar(frame, reusable.unwrap_or_default())?, None)
            }
        } else {
            (Vec::new(), None)
        };
        Ok(Picture {
            width: frame.width,
            height: frame.height,
            bit_depth: frame.bit_depth,
            poc: reference.poc,
            yuv,
            dmabuf,
            err_info: reference.err_info,
        })
    }

    pub fn set_output_dmabuf(&mut self, enabled: bool) {
        self.output_dmabuf = enabled;
    }

    pub fn recycle_output(&mut self, buffer: Vec<u8>) {
        if buffer.capacity() != 0 && self.output_recycle.len() < 4 {
            self.output_recycle.push(buffer);
        }
    }

    pub fn import_output_dmabuf(&mut self, buffer: DmaBuf) -> Result<(), String> {
        self.set_output_dmabuf(true);
        self.output_injection = Some(buffer);
        Ok(())
    }
}

fn copy_planar(frame: &Frame, out: Vec<u8>) -> Result<Vec<u8>, String> {
    let traced = std::env::var_os("RK_MPP_AV1_TRACE").is_some();
    let start = Instant::now();
    let display = frame
        .display
        .data()
        .map_err(|error| format!("AV1 display view: {error}"))?;
    let size = frame.display_hor_stride * frame.display_ver_stride;
    let _ = display.buffer().sync_read_begin();
    let output = copy_planar_data(
        frame.width,
        frame.height,
        frame.display_hor_stride,
        frame.bit_depth,
        &display[..size],
        &display[size..size + size / 2],
        out,
    );
    let _ = display.buffer().sync_read_end();
    if traced {
        eprintln!(
            "av1-copy {}x{} bitdepth={} bytes={} elapsed_us={}",
            frame.width,
            frame.height,
            frame.bit_depth,
            output.len(),
            start.elapsed().as_micros()
        );
    }
    Ok(output)
}

fn copy_planar_data(
    width: usize,
    height: usize,
    stride: usize,
    bit_depth: u32,
    y: &[u8],
    uv: &[u8],
    mut out: Vec<u8>,
) -> Vec<u8> {
    // The shared readback covers the 8-bit 4:2:0 case with even dimensions, which
    // is the only shape where its truncated halves match the div_ceil chroma
    // size used here.
    if bit_depth == 8 && width % 2 == 0 && height % 2 == 0 {
        crate::codec::yuv::nv12_to_planar_into(&mut out, y, uv, width, height, stride);
        return out;
    }
    if bit_depth == 10 {
        return crate::codec::yuv::packed10_nv12_to_planar(y, uv, width, height, stride);
    }
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let sample_bytes = if bit_depth > 8 { 2 } else { 1 };
    out.clear();
    out.reserve((width * height + 2 * chroma_width * chroma_height) * sample_bytes);
    if bit_depth == 8 {
        for row in 0..height {
            let start = row * stride;
            out.extend_from_slice(&y[start..start + width]);
        }
        for component in 0..2 {
            for row in 0..chroma_height {
                let start = row * stride + component;
                for column in 0..chroma_width {
                    out.push(uv[start + column * 2]);
                }
            }
        }
    }
    out
}

fn cdf_hash(cdf: &crate::codec::av1::cdf::Av1Cdf) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in cdf
        .regular
        .as_bytes()
        .iter()
        .chain(cdf.intrabc_mv.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn cdf_hash_from_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_nv12_to_planar_without_mixing_the_components() {
        let stride = 8;
        let mut data = vec![0u8; stride * 4 + stride * 2];
        for row in 0..4 {
            for column in 0..4 {
                data[row * stride + column] = (row * 4 + column) as u8;
            }
        }
        let uv = stride * 4;
        for row in 0..2 {
            for column in 0..2 {
                data[uv + row * stride + column * 2] = 10 + column as u8;
                data[uv + row * stride + column * 2 + 1] = 20 + column as u8;
            }
        }
        let planar = copy_planar_data(4, 4, stride, 8, &data[..uv], &data[uv..], Vec::new());
        assert_eq!(
            &planar[..16],
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
        assert_eq!(&planar[16..20], &[10, 11, 10, 11]);
        assert_eq!(&planar[20..24], &[20, 21, 20, 21]);
    }

    #[test]
    fn copies_packed_10bit_nv12_to_planar_16bit_words() {
        let width = 8;
        let height = 2;
        let stride = 10;
        let mut data = vec![0u8; stride * height + stride];
        write_packed_10bit(&mut data[..stride], &[0, 1, 2, 3, 4, 5, 6, 7]);
        write_packed_10bit(
            &mut data[stride..stride * 2],
            &[8, 9, 10, 11, 12, 13, 14, 15],
        );
        write_packed_10bit(
            &mut data[stride * height..stride * (height + 1)],
            &[100, 200, 101, 201, 102, 202, 103, 203],
        );

        let planar = copy_planar_data(
            width,
            height,
            stride,
            10,
            &data[..stride * height],
            &data[stride * height..],
            Vec::new(),
        );
        let samples: Vec<u16> = planar
            .chunks_exact(2)
            .map(|word| u16::from_le_bytes([word[0], word[1]]))
            .collect();
        assert_eq!(
            samples,
            [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 100, 101, 102, 103, 200, 201,
                202, 203
            ]
        );
    }

    fn write_packed_10bit(dst: &mut [u8], samples: &[u16]) {
        for (index, sample) in samples.iter().copied().enumerate() {
            let bit = index * 10;
            let byte = bit / 8;
            let shift = bit % 8;
            let word = u32::from(sample) << shift;
            dst[byte] |= word as u8;
            dst[byte + 1] |= (word >> 8) as u8;
            if shift > 6 {
                dst[byte + 2] |= (word >> 16) as u8;
            }
        }
    }
}
