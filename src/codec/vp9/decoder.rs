//! End-to-end VP9 decoder: VP9/IVF packets in, planar YUV out.

use std::io;

use crate::hal::vp9d::{Frame, Vp9d};
use crate::service::dma_heap::DmaBuf;

use super::parser::{self, FrameKind, Header, Parser};

pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

#[derive(Debug, Clone, Copy)]
struct Reference {
    frame: usize,
    poc: u32,
    err_info: u32,
}

/// One displayed VP9 picture.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub poc: u32,
    /// Tightly packed planar YUV. High-bit-depth samples are little-endian
    /// 16-bit words containing their unscaled 10-bit value.
    pub yuv: Vec<u8>,
    pub err_info: u32,
}

pub struct Decoder {
    parser: Parser,
    hw: Vp9d,
    frames: Vec<Option<Frame>>,
    reference_map: [Option<Reference>; 8],
    next_poc: u32,
}

impl Decoder {
    pub fn new() -> io::Result<Decoder> {
        Ok(Decoder {
            parser: Parser::new(),
            hw: Vp9d::open()?,
            frames: Vec::new(),
            reference_map: [None; 8],
            next_poc: 1,
        })
    }

    pub fn status(&self) -> (u32, u32, u32) {
        self.hw.last_status
    }

    /// Decode one VP9 packet. A superframe can display more than one picture.
    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<Picture>, String> {
        let frames = parser::split_superframe(packet).map_err(|error| error.to_string())?;
        let mut output = Vec::new();
        for frame in frames {
            if let Some(picture) = self.decode_frame(frame)? {
                output.push(picture);
            }
        }
        Ok(output)
    }

    fn decode_frame(&mut self, packet: &[u8]) -> Result<Option<Picture>, String> {
        let parsed = self
            .parser
            .parse_frame(packet)
            .map_err(|error| error.to_string())?;
        if matches!(parsed.header.frame_kind, FrameKind::ShowExisting) {
            if std::env::var_os("RFI_VP9_TRACE").is_some() {
                eprintln!(
                    "  show_existing map_idx={:?} {}x{}",
                    parsed.header.frame_to_show_map_idx, parsed.header.width, parsed.header.height,
                );
            }
            return self.show_existing(&parsed.header).map(Some);
        }
        let compressed = parsed
            .compressed_header
            .as_ref()
            .ok_or("missing VP9 compressed header")?;
        let poc = self.next_poc;
        if std::env::var_os("RFI_VP9_CONTEXT_DUMP").is_some() {
            let path = format!("/tmp/rk_mpp_rs_vp9_context_{poc:04}.bin");
            let _ = std::fs::write(path, compressed.probabilities.snapshot_bytes());
        }
        self.next_poc = if self.next_poc == u16::MAX as u32 {
            1
        } else {
            self.next_poc + 1
        };
        if std::env::var_os("RFI_VP9_TRACE").is_some() {
            eprintln!(
                "  decode poc={poc} kind={:?} size={}x{} refs={:?} bias={:?} \
                 seg={}/{}/{} tree={:?} pred={:?} masks={:?} data={:?} reset={} ctx={} refresh={:02x} \
                 errres={} show={} refresh_ctx={} parallel={} use_prev_mvs={} \
                 filter={} sharp={} q={} tile={}x{} header={} partition={}",
                parsed.header.frame_kind,
                parsed.header.width,
                parsed.header.height,
                parsed.header.ref_frame_idx,
                parsed.header.ref_frame_sign_bias,
                parsed.header.segmentation.enabled,
                parsed.header.segmentation.update_map,
                parsed.header.segmentation.temporal_update,
                parsed.header.segmentation.tree_probs,
                parsed.header.segmentation.pred_probs,
                parsed.header.segmentation.feature_mask,
                parsed.header.segmentation.feature_data,
                parsed.header.reset_frame_context,
                parsed.header.frame_context_idx,
                parsed.header.refresh_frame_flags,
                parsed.header.error_resilient_mode,
                parsed.header.show_frame,
                parsed.header.refresh_frame_context,
                parsed.header.frame_parallel_mode,
                parsed.header.use_prev_in_find_mv_refs,
                parsed.header.loop_filter.level,
                parsed.header.loop_filter.sharpness,
                parsed.header.quantization.base_q_idx,
                parsed.header.log2_tile_cols,
                parsed.header.log2_tile_rows,
                parsed.header.header_size,
                parsed.header.first_partition_size,
            );
        }

        let frame_index = self.allocate_frame(&parsed.header)?;
        let references = hardware_references(&parsed.header, &self.reference_map, &self.frames);
        let current = self.frames[frame_index]
            .as_ref()
            .ok_or("VP9 frame allocation failed")?;

        let padded_len = ((packet.len() + 15) & !15) + 0x80;
        let mut stream = DmaBuf::alloc("system", padded_len, false).map_err(|e| e.to_string())?;
        stream.sync_write_begin().map_err(|e| e.to_string())?;
        stream.as_mut_slice()[..packet.len()].copy_from_slice(packet);
        stream.sync_write_end().map_err(|e| e.to_string())?;

        if std::env::var_os("RFI_VP9_STREAM_DUMP").is_some() {
            let _ = std::fs::write(format!("/tmp/rk_mpp_rs_vp9_{poc:04}.vp9"), packet);
        }
        self.hw
            .decode(
                &parsed.header,
                compressed,
                poc,
                current,
                &references,
                &stream,
                packet.len(),
            )
            .map_err(|error| error.to_string())?;

        let err_info = frame_error(
            reference_error(&parsed.header, &self.reference_map),
            self.hw.last_hardware_error,
        );
        for index in 0..8 {
            if parsed.header.refresh_frame_flags & (1 << index) != 0 {
                self.reference_map[index] = Some(Reference {
                    frame: frame_index,
                    poc,
                    err_info,
                });
            }
        }
        if !parsed.header.show_frame {
            return Ok(None);
        }
        let frame = self.frames[frame_index]
            .as_ref()
            .ok_or("VP9 output frame disappeared")?;
        Ok(Some(Picture {
            width: frame.width,
            height: frame.height,
            poc,
            yuv: if err_info == 0 {
                copy_planar(frame)
            } else {
                Vec::new()
            },
            err_info,
        }))
    }

    fn show_existing(&self, header: &Header) -> Result<Picture, String> {
        let map_index = header
            .frame_to_show_map_idx
            .ok_or("show-existing frame has no reference-map index")?;
        let reference = self.reference_map[map_index as usize]
            .ok_or_else(|| format!("show-existing reference slot {map_index} is empty"))?;
        let frame = self
            .frames
            .get(reference.frame)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                format!("show-existing reference slot {map_index} points to a released frame")
            })?;
        Ok(Picture {
            width: frame.width,
            height: frame.height,
            poc: reference.poc,
            yuv: if reference.err_info == 0 {
                copy_planar(frame)
            } else {
                Vec::new()
            },
            err_info: reference.err_info,
        })
    }

    fn allocate_frame(&mut self, header: &Header) -> Result<usize, String> {
        let live: Vec<usize> = self
            .reference_map
            .iter()
            .flatten()
            .map(|reference| reference.frame)
            .collect();
        let hardware_live_colmv = self.hw.colmv_liveness_fds();
        let reusable = self.frames.iter().enumerate().find_map(|(index, frame)| {
            let frame = frame.as_ref()?;
            (!live.contains(&index)
                && !hardware_live_colmv.contains(&Some(frame.colmv.fd() as u32))
                && frame.width == header.width as usize
                && frame.height == header.height as usize
                && frame.bit_depth == header.bit_depth as u32)
                .then_some(index)
        });
        let index = if let Some(index) = reusable {
            index
        } else {
            let frame = Frame::alloc(
                header.width as usize,
                header.height as usize,
                header.bit_depth as u32,
            )
            .map_err(|error| error.to_string())?;
            if let Some(index) = self.frames.iter().position(Option::is_none) {
                self.frames[index] = Some(frame);
                index
            } else {
                self.frames.push(Some(frame));
                self.frames.len() - 1
            }
        };
        Ok(index)
    }
}

fn hardware_references<'a>(
    header: &Header,
    reference_map: &[Option<Reference>; 8],
    frames: &'a [Option<Frame>],
) -> [Option<(&'a Frame, u32)>; 3] {
    std::array::from_fn(|index| {
        let map_index = header.ref_frame_idx[index] as usize;
        reference_map
            .get(map_index)
            .and_then(|reference| *reference)
            .and_then(|reference| {
                frames
                    .get(reference.frame)
                    .and_then(|frame| frame.as_ref())
                    .map(|frame| (frame, reference.poc))
            })
    })
}

fn reference_error(header: &Header, reference_map: &[Option<Reference>; 8]) -> bool {
    if matches!(header.frame_kind, FrameKind::Key) || header.intra_only {
        return false;
    }
    header.ref_frame_idx.iter().any(|&map_index| {
        reference_map
            .get(map_index as usize)
            .and_then(|reference| *reference)
            .is_some_and(|reference| reference.err_info != 0)
    })
}

fn frame_error(reference_error: bool, hardware_error: bool) -> u32 {
    if reference_error {
        FRAME_ERR_DEC_INVALID
    } else if hardware_error {
        FRAME_ERR_DEC_HW_ERR
    } else {
        0
    }
}

fn copy_planar(frame: &Frame) -> Vec<u8> {
    let _ = frame.video.sync_read_begin();
    let out = copy_planar_data(
        frame.width,
        frame.height,
        frame.hor_stride,
        frame.bit_depth,
        frame.y_plane(),
        frame.uv_plane(),
    );
    let _ = frame.video.sync_read_end();
    out
}

fn copy_planar_data(
    width: usize,
    height: usize,
    stride: usize,
    bit_depth: u32,
    y: &[u8],
    uv: &[u8],
) -> Vec<u8> {
    // The shared readback covers the 8-bit 4:2:0 case with even dimensions, which
    // is the only shape where its truncated halves match the div_ceil chroma
    // size used here.
    if bit_depth == 8 && width % 2 == 0 && height % 2 == 0 {
        return crate::codec::yuv::nv12_to_planar(y, uv, width, height, stride);
    }
    let sample_bytes = if bit_depth > 8 { 2 } else { 1 };
    let luma_row = width * sample_bytes;
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let mut out =
        Vec::with_capacity((width * height + 2 * chroma_width * chroma_height) * sample_bytes);

    if bit_depth > 8 {
        for row in 0..height {
            let start = row * stride;
            for index in 0..width {
                out.extend_from_slice(&packed_10bit_sample(&y[start..], index).to_le_bytes());
            }
        }
        for component in 0..2 {
            for row in 0..chroma_height {
                let start = row * stride;
                for column in 0..chroma_width {
                    let index = column * 2 + component;
                    out.extend_from_slice(&packed_10bit_sample(&uv[start..], index).to_le_bytes());
                }
            }
        }
        return out;
    }

    for row in 0..height {
        let start = row * stride;
        out.extend_from_slice(&y[start..start + luma_row]);
    }
    for component in 0..2 {
        for row in 0..chroma_height {
            let start = row * stride + component * sample_bytes;
            for column in 0..chroma_width {
                let offset = start + column * sample_bytes * 2;
                out.extend_from_slice(&uv[offset..offset + sample_bytes]);
            }
        }
    }
    out
}

fn packed_10bit_sample(data: &[u8], index: usize) -> u16 {
    let bit = index * 10;
    let byte = bit / 8;
    let shift = bit % 8;
    let word = u16::from_le_bytes([data[byte], data[byte + 1]]) as u32;
    ((word >> shift) & 0x3ff) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_packed_10bit(dst: &mut [u8], samples: &[u16]) {
        for (index, sample) in samples.iter().copied().enumerate() {
            let base = index / 8 * 10;
            let bit = index % 8 * 10;
            let word = (sample as u128) << bit;
            for byte in 0..10 {
                let target = base + byte;
                if target < dst.len() {
                    dst[target] |= ((word >> (byte * 8)) & 0xff) as u8;
                }
            }
        }
    }

    #[test]
    fn planar_copy_deinterleaves_rows_without_mixing_u_and_v() {
        let stride = 8;
        let mut data = vec![0u8; stride * 4 + stride * 2];
        for row in 0..4 {
            let start = row * stride;
            for column in 0..stride {
                data[start + column] = row as u8;
            }
        }
        let uv = stride * 4;
        for row in 0..2 {
            let start = uv + row * stride;
            for column in 0..2 {
                data[start + column * 2] = 10 + column as u8;
                data[start + column * 2 + 1] = 20 + column as u8;
            }
        }
        let planar = copy_planar_data(4, 4, stride, 8, &data[..uv], &data[uv..]);
        assert_eq!(
            &planar[..16],
            &[0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3]
        );
        assert_eq!(&planar[16..20], &[10, 11, 10, 11]);
        assert_eq!(&planar[20..24], &[20, 21, 20, 21]);
    }

    #[test]
    fn planar_copy_uses_ceiling_chroma_dimensions_for_odd_sizes() {
        let width = 5;
        let height = 3;
        let stride = 8;
        let mut data = vec![0u8; stride * height + stride * 2];
        let y = stride * height;
        for row in 0..height {
            for column in 0..width {
                data[row * stride + column] = (row * width + column) as u8;
            }
        }
        for row in 0..2 {
            for column in 0..3 {
                data[y + row * stride + column * 2] = 10 + row as u8 * 3 + column as u8;
                data[y + row * stride + column * 2 + 1] = 20 + row as u8 * 3 + column as u8;
            }
        }

        let planar = copy_planar_data(width, height, stride, 8, &data[..y], &data[y..]);
        assert_eq!(planar.len(), (width * height + 2 * 3 * 2) as usize);
        assert_eq!(
            &planar[15..],
            &[10, 11, 12, 13, 14, 15, 20, 21, 22, 23, 24, 25]
        );
    }

    #[test]
    fn packed_10bit_rows_match_mpp_rearrange_pix() {
        let samples = [1023u16, 512, 1, 256, 768, 42, 900, 17];
        let mut packed = [0u8; 10];
        for (index, sample) in samples.into_iter().enumerate() {
            for bit in 0..10 {
                let target = index * 10 + bit;
                packed[target / 8] |= (((sample >> bit) & 1) as u8) << (target % 8);
            }
        }

        let unpacked: Vec<u16> = (0..8)
            .map(|index| packed_10bit_sample(&packed, index))
            .collect();
        assert_eq!(unpacked, samples);
    }

    #[test]
    fn planar_copy_unpacks_10bit_luma_and_chroma() {
        let stride = 256;
        let mut data = vec![0u8; stride * 3];
        write_packed_10bit(&mut data[..], &[0, 1, 2, 3, 4, 5, 6, 7]);
        write_packed_10bit(&mut data[stride..], &[8, 9, 10, 11, 12, 13, 14, 15]);
        write_packed_10bit(
            &mut data[stride * 2..],
            &[100, 200, 101, 201, 102, 202, 103, 203],
        );

        let planar = copy_planar_data(8, 2, stride, 10, &data[..stride * 2], &data[stride * 2..]);
        let expected: Vec<u16> = (0..16)
            .chain([100, 101, 102, 103])
            .chain([200, 201, 202, 203])
            .collect();
        let actual: Vec<u16> = planar
            .chunks_exact(2)
            .map(|sample| u16::from_le_bytes([sample[0], sample[1]]))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn selected_reference_errors_invalidate_inter_frames_only() {
        let mut reference_map = [None; 8];
        reference_map[3] = Some(Reference {
            frame: 0,
            poc: 1,
            err_info: FRAME_ERR_DEC_INVALID,
        });
        let ivf =
            parser::parse_ivf(include_bytes!("../../../tests/data/vp9_p0_64x64.ivf")).unwrap();
        let payload = parser::split_superframe(ivf.frames[0].data).unwrap()[0];
        let mut header = Parser::new().parse(payload).unwrap();
        header.frame_kind = FrameKind::Inter;
        header.intra_only = false;
        header.ref_frame_idx = [3, 0, 0];

        assert!(reference_error(&header, &reference_map));
        assert_eq!(frame_error(true, true), FRAME_ERR_DEC_INVALID);
        assert_eq!(frame_error(false, true), FRAME_ERR_DEC_HW_ERR);

        let mut intra = header;
        intra.intra_only = true;
        assert!(!reference_error(&intra, &reference_map));
        intra.intra_only = false;
        intra.frame_kind = FrameKind::Key;
        assert!(!reference_error(&intra, &reference_map));
    }
}
