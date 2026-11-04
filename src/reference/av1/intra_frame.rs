//! Frame-level driver for an intra (key-frame) AV1 picture.

use crate::reference::av1::frame_header::FrameHeader;
use crate::reference::av1::intra::{IntraMode, UvMode};
use crate::reference::av1::intra_sink::IntraSyntaxBlock;
use crate::reference::av1::reconstruct::{
    reconstruct_intra_block_plane, PlaneQDelta, ReconstructedLuma,
};
use crate::reference::av1::seq_header::SequenceHeader;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneBuffer {
    pub data: Vec<u16>,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl PlaneBuffer {
    fn new(width: usize, height: usize) -> Self {
        let stride = width.div_ceil(64) * 64;
        let allocated_height = height.div_ceil(64) * 64;
        Self {
            data: vec![0; stride * allocated_height],
            width,
            height,
            stride,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntraFrame {
    pub coded_width: usize,
    pub coded_height: usize,
    pub viewport_width: usize,
    pub viewport_height: usize,
    pub bit_depth: u32,
    pub y: PlaneBuffer,
    pub u: Option<PlaneBuffer>,
    pub v: Option<PlaneBuffer>,
}

fn uv_mode(mode: UvMode) -> Option<IntraMode> {
    Some(match mode {
        UvMode::Dc => IntraMode::Dc,
        UvMode::V => IntraMode::V,
        UvMode::H => IntraMode::H,
        UvMode::D45 => IntraMode::D45,
        UvMode::D135 => IntraMode::D135,
        UvMode::D113 => IntraMode::D113,
        UvMode::D157 => IntraMode::D157,
        UvMode::D203 => IntraMode::D203,
        UvMode::D67 => IntraMode::D67,
        UvMode::Smooth => IntraMode::Smooth,
        UvMode::SmoothV => IntraMode::SmoothV,
        UvMode::SmoothH => IntraMode::SmoothH,
        UvMode::Paeth => IntraMode::Paeth,
        UvMode::Cfl => return None,
    })
}

fn fill_modes(
    grid: &mut [Option<IntraMode>],
    cols: usize,
    block: &IntraSyntaxBlock,
    mode: Option<IntraMode>,
) {
    let w = block.block.size.num_4x4_wide() as usize;
    let h = block.block.size.num_4x4_high() as usize;
    for row in block.block.row..block.block.row + h {
        for col in block.block.col..block.block.col + w {
            if let Some(slot) = grid.get_mut(row * cols + col) {
                *slot = mode;
            }
        }
    }
}

fn neighbor_modes(
    grid: &[Option<IntraMode>],
    cols: usize,
    block: &IntraSyntaxBlock,
) -> (Option<IntraMode>, Option<IntraMode>) {
    let above = (block.block.row > 0)
        .then(|| grid[(block.block.row - 1) * cols + block.block.col])
        .flatten();
    let left = (block.block.col > 0)
        .then(|| grid[block.block.row * cols + block.block.col - 1])
        .flatten();
    (above, left)
}

/// Reconstruct an intra frame from the syntax blocks in tile order. Loop
/// filters and CDEF/restoration run after this stage.
pub fn reconstruct_intra_frame(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    blocks: &[IntraSyntaxBlock],
) -> Result<IntraFrame, &'static str> {
    let width = header.mi_cols * 4;
    let height = header.mi_rows * 4;
    let sub_x = u32::from(sequence.color.subsampling_x);
    let sub_y = u32::from(sequence.color.subsampling_y);
    let chroma_width = width >> sub_x;
    let chroma_height = height >> sub_y;
    let bit_depth = u32::from(sequence.color.bit_depth);

    let mut frame = IntraFrame {
        coded_width: width,
        coded_height: height,
        viewport_width: header.upscaled_width as usize,
        viewport_height: header.frame_height as usize,
        bit_depth,
        y: PlaneBuffer::new(width, height),
        u: (!sequence.color.mono_chrome).then(|| PlaneBuffer::new(chroma_width, chroma_height)),
        v: (!sequence.color.mono_chrome).then(|| PlaneBuffer::new(chroma_width, chroma_height)),
    };

    let mut y_modes = vec![None; header.mi_cols * header.mi_rows];
    let mut uv_modes = vec![None; header.mi_cols * header.mi_rows];
    for block in blocks {
        fill_modes(&mut y_modes, header.mi_cols, block, Some(block.y_mode));
        fill_modes(
            &mut uv_modes,
            header.mi_cols,
            block,
            block.uv_mode.and_then(uv_mode),
        );
    }

    let y_delta = PlaneQDelta {
        dc: header.delta_q_y_dc,
        ac: 0,
    };
    let u_delta = PlaneQDelta {
        dc: header.delta_q_u_dc,
        ac: header.delta_q_u_ac,
    };
    let v_delta = PlaneQDelta {
        dc: header.delta_q_v_dc,
        ac: header.delta_q_v_ac,
    };

    for block in blocks {
        let (y_above, y_left) = neighbor_modes(&y_modes, header.mi_cols, block);
        reconstruct_intra_block_plane(
            &mut frame.y.data,
            frame.y.stride,
            frame.y.width,
            frame.y.height,
            0,
            sub_x,
            sub_y,
            block,
            y_delta,
            bit_depth,
            sequence.enable_intra_edge_filter,
            y_above,
            y_left,
            None,
        )?;

        let (uv_above, uv_left) = neighbor_modes(&uv_modes, header.mi_cols, block);
        if block.transforms.iter().any(|tx| tx.plane == 1) {
            let u = frame
                .u
                .as_mut()
                .ok_or("chroma transform without a chroma plane")?;
            reconstruct_intra_block_plane(
                &mut u.data,
                u.stride,
                u.width,
                u.height,
                1,
                sub_x,
                sub_y,
                block,
                u_delta,
                bit_depth,
                sequence.enable_intra_edge_filter,
                uv_above,
                uv_left,
                Some(ReconstructedLuma {
                    samples: &frame.y.data,
                    stride: frame.y.stride,
                    width: frame.y.width,
                    height: frame.y.height,
                }),
            )?;
        }
        if block.transforms.iter().any(|tx| tx.plane == 2) {
            let v = frame
                .v
                .as_mut()
                .ok_or("chroma transform without a chroma plane")?;
            reconstruct_intra_block_plane(
                &mut v.data,
                v.stride,
                v.width,
                v.height,
                2,
                sub_x,
                sub_y,
                block,
                v_delta,
                bit_depth,
                sequence.enable_intra_edge_filter,
                uv_above,
                uv_left,
                Some(ReconstructedLuma {
                    samples: &frame.y.data,
                    stride: frame.y.stride,
                    width: frame.y.width,
                    height: frame.y.height,
                }),
            )?;
        }
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::intra_sink::{IntraTileBlockSink, IntraTileConfig};
    use crate::reference::av1::obu::parse_ivf;
    use crate::reference::av1::stream::{ReferenceStream, StreamEvent};
    use crate::reference::av1::tile_entropy::decode_tile_partitions;

    #[test]
    fn real_keyframe_reconstructs_all_three_planes() {
        let data = include_bytes!("../../../tests/data/av1_320x240_10.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let packets = stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        let frame = packets
            .iter()
            .flatten()
            .find_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .unwrap();
        let config = IntraTileConfig::from_headers(sequence, &frame.header);
        let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
        let tile = &frame.layout.tiles[0];
        let decoded = decode_tile_partitions(
            &frame.payload[tile.bytes.clone()],
            &frame.header,
            tile.tile,
            sb_size,
            false,
            true,
            IntraTileBlockSink::new(config),
        )
        .unwrap();
        let picture =
            reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks).unwrap();
        assert_eq!(
            (picture.viewport_width, picture.viewport_height),
            (320, 240)
        );
        assert_eq!((picture.y.width, picture.y.height), (320, 240));
        assert_eq!(
            (
                picture.u.as_ref().unwrap().width,
                picture.u.as_ref().unwrap().height
            ),
            (160, 120)
        );
        assert!(picture.y.data.iter().any(|sample| *sample != 0));
        assert!(picture
            .u
            .as_ref()
            .unwrap()
            .data
            .iter()
            .any(|sample| *sample != 0));
        assert!(picture
            .v
            .as_ref()
            .unwrap()
            .data
            .iter()
            .any(|sample| *sample != 0));
        let hash = |data: &[u16]| {
            data.iter().fold(0xcbf29ce484222325u64, |hash, sample| {
                (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
            })
        };
        assert_eq!(hash(&picture.y.data), 0x91bf3f1db09c78e9);
        assert_eq!(hash(&picture.u.as_ref().unwrap().data), 0x704bc148f0e31fbc);
        assert_eq!(hash(&picture.v.as_ref().unwrap().data), 0x63ac4af721f7b9d3);
    }
}
