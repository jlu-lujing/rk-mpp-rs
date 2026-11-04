//! Frame-level driver for an intra (key-frame) AV1 picture.

use crate::reference::av1::cdef::{
    cdef_direction, cdef_filter_block, cdef_luma_primary_strength, CdefIndexGrid, CdefRegion,
    PlaneView, CDEF_UV_DIR,
};
use crate::reference::av1::deblock::{
    loop_filter_frame_dual_grid, DeblockCell, DeblockEdgeInfo, DeblockGrid, DeblockPlane,
};
use crate::reference::av1::frame_header::FrameHeader;
use crate::reference::av1::intra::{IntraMode, UvMode};
use crate::reference::av1::intra_sink::IntraSyntaxBlock;
use crate::reference::av1::lr_frame::{apply_loop_restoration, LrFrameParams, LrPlaneGeometry};
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

fn deblock_grid(header: &FrameHeader, blocks: &[IntraSyntaxBlock], vertical: bool) -> DeblockGrid {
    let cell = DeblockCell {
        info: DeblockEdgeInfo::default(),
        apply_filter: false,
        filter_size: 4,
    };
    let mut grid = DeblockGrid::uniform(header.mi_cols, header.mi_rows, cell);
    for block in blocks {
        let w = block.block.size.num_4x4_wide() as usize;
        let h = block.block.size.num_4x4_high() as usize;
        let (block_extent, tx_extent) = if vertical {
            (block.block.size.width as usize, block.tx.width() as usize)
        } else {
            (block.block.size.height as usize, block.tx.height() as usize)
        };
        let info = DeblockEdgeInfo {
            ref_frame: 0,
            mode_type: 0,
            delta_lf: block.delta_lf,
            seg_lvl_alt_lf: None,
        };
        for row in block.block.row..(block.block.row + h).min(header.mi_rows) {
            for col in block.block.col..(block.block.col + w).min(header.mi_cols) {
                let coordinate = if vertical {
                    (col - block.block.col) * 4
                } else {
                    (row - block.block.row) * 4
                };
                grid.cells[row * header.mi_cols + col] = DeblockCell::from_geometry(
                    coordinate,
                    vertical as usize,
                    block_extent,
                    tx_extent,
                    tx_extent,
                    block.skip,
                    true,
                    info,
                );
            }
        }
    }
    grid
}

/// Apply §7.14 deblocking to the reconstructed frame.
pub fn apply_intra_deblock(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    blocks: &[IntraSyntaxBlock],
    frame: &mut IntraFrame,
) {
    let vertical = deblock_grid(header, blocks, true);
    let horizontal = deblock_grid(header, blocks, false);
    let mut planes = vec![DeblockPlane::new(
        &frame.y.data,
        frame.y.stride,
        frame.y.width,
        frame.y.height,
        frame.bit_depth,
    )];
    if let Some(u) = frame.u.as_ref() {
        planes.push(DeblockPlane::new(
            &u.data,
            u.stride,
            u.width,
            u.height,
            frame.bit_depth,
        ));
    }
    if let Some(v) = frame.v.as_ref() {
        planes.push(DeblockPlane::new(
            &v.data,
            v.stride,
            v.width,
            v.height,
            frame.bit_depth,
        ));
    }
    loop_filter_frame_dual_grid(
        &mut planes,
        &header.loop_filter_params(),
        &vertical,
        &horizontal,
        frame.coded_width,
        frame.coded_height,
        u32::from(sequence.color.subsampling_x),
        u32::from(sequence.color.subsampling_y),
    );
    frame.y.data = planes.remove(0).samples;
    if let Some(u) = frame.u.as_mut() {
        u.data = planes.remove(0).samples;
    }
    if let Some(v) = frame.v.as_mut() {
        v.data = planes.remove(0).samples;
    }
}

fn write_block(dst: &mut [u16], stride: usize, x: usize, y: usize, width: usize, block: &[u16]) {
    for row in 0..block.len() / width {
        let start = (y + row) * stride + x;
        dst[start..start + width].copy_from_slice(&block[row * width..(row + 1) * width]);
    }
}

/// Apply §7.15 CDEF using the tile's transmitted `cdef_idx` grid.
pub fn apply_intra_cdef(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    cdef_grid: &CdefIndexGrid,
    frame: &mut IntraFrame,
) {
    if header.coded_lossless || header.allow_intrabc || !sequence.enable_cdef {
        return;
    }
    let sub_x = u32::from(sequence.color.subsampling_x);
    let sub_y = u32::from(sequence.color.subsampling_y);
    let region = CdefRegion::new(header.mi_cols, header.mi_rows);
    let luma_input = PlaneView::new(&frame.y.data, frame.y.stride, frame.y.width, frame.y.height);
    let mut y_out = frame.y.data.clone();
    let mut u_out = frame.u.as_ref().map(|p| p.data.clone());
    let mut v_out = frame.v.as_ref().map(|p| p.data.clone());

    for r in (0..header.mi_rows).step_by(2) {
        for c in (0..header.mi_cols).step_by(2) {
            let Some(index) = cdef_grid.get(r & !15, c & !15) else {
                continue;
            };
            let index = usize::from(index);
            let direction = cdef_direction(luma_input, c * 4, r * 4, frame.bit_depth);
            let luma_primary = cdef_luma_primary_strength(
                i32::from(header.cdef_y_pri_strength[index]),
                direction.variance,
            );
            let luma = cdef_filter_block(
                luma_input,
                region,
                r,
                c,
                0,
                0,
                luma_primary,
                i32::from(header.cdef_y_sec_strength[index]),
                i32::from(header.cdef_damping),
                direction.dir,
                frame.bit_depth,
            );
            write_block(&mut y_out, frame.y.stride, c * 4, r * 4, 8, &luma);

            let uv_dir = CDEF_UV_DIR[sub_x as usize][sub_y as usize][usize::from(direction.dir)];
            for (plane, out) in [(&frame.u, &mut u_out), (&frame.v, &mut v_out)] {
                let (Some(plane), Some(out)) = (plane.as_ref(), out.as_mut()) else {
                    continue;
                };
                let view = PlaneView::new(&plane.data, plane.stride, plane.width, plane.height);
                let block = cdef_filter_block(
                    view,
                    region,
                    r,
                    c,
                    sub_x,
                    sub_y,
                    i32::from(header.cdef_uv_pri_strength[index]),
                    i32::from(header.cdef_uv_sec_strength[index]),
                    i32::from(header.cdef_damping),
                    uv_dir,
                    frame.bit_depth,
                );
                write_block(
                    out,
                    plane.stride,
                    (c * 4) >> sub_x,
                    (r * 4) >> sub_y,
                    8 >> sub_x,
                    &block,
                );
            }
        }
    }
    frame.y.data = y_out;
    if let Some(u) = frame.u.as_mut() {
        u.data = u_out.unwrap();
    }
    if let Some(v) = frame.v.as_mut() {
        v.data = v_out.unwrap();
    }
}

/// Apply §7.17 loop restoration. `current` is the deblocked frame before CDEF;
/// `frame` is the CDEF output and is replaced with the restored frame.
pub fn apply_intra_loop_restoration(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    params: &LrFrameParams,
    current: &IntraFrame,
    frame: &mut IntraFrame,
) -> Result<(), &'static str> {
    if !header.uses_lr {
        return Ok(());
    }
    if header.use_superres {
        return Err("loop restoration after superresolution is not integrated");
    }
    let geometry: Vec<LrPlaneGeometry> = (0..sequence.color.num_planes)
        .map(|plane| header.lr_geometry(plane, sequence))
        .collect();
    let mut cdef_planes = vec![crate::reference::av1::cdef::PlaneView::new(
        &frame.y.data,
        frame.y.stride,
        frame.y.width,
        frame.y.height,
    )];
    let mut current_planes = vec![crate::reference::av1::cdef::PlaneView::new(
        &current.y.data,
        current.y.stride,
        current.y.width,
        current.y.height,
    )];
    if let Some(u) = frame.u.as_ref() {
        cdef_planes.push(crate::reference::av1::cdef::PlaneView::new(
            &u.data, u.stride, u.width, u.height,
        ));
        let current = current.u.as_ref().ok_or("missing current U plane")?;
        current_planes.push(crate::reference::av1::cdef::PlaneView::new(
            &current.data,
            current.stride,
            current.width,
            current.height,
        ));
    }
    if let Some(v) = frame.v.as_ref() {
        cdef_planes.push(crate::reference::av1::cdef::PlaneView::new(
            &v.data, v.stride, v.width, v.height,
        ));
        let current = current.v.as_ref().ok_or("missing current V plane")?;
        current_planes.push(crate::reference::av1::cdef::PlaneView::new(
            &current.data,
            current.stride,
            current.width,
            current.height,
        ));
    }
    if geometry.len() != cdef_planes.len() {
        return Err("loop restoration plane count does not match the frame");
    }
    let out = apply_loop_restoration(
        &cdef_planes,
        &current_planes,
        &geometry,
        params,
        header.frame_height as usize,
        header.upscaled_width as usize,
        frame.bit_depth,
    );
    frame.y.data = out[0].clone();
    if let Some(u) = frame.u.as_mut() {
        u.data = out[1].clone();
    }
    if let Some(v) = frame.v.as_mut() {
        v.data = out[2].clone();
    }
    Ok(())
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
        let mut picture =
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
        let hashes = |picture: &IntraFrame| {
            (
                hash(&picture.y.data),
                hash(&picture.u.as_ref().unwrap().data),
                hash(&picture.v.as_ref().unwrap().data),
            )
        };
        let pre = hashes(&picture);
        apply_intra_deblock(sequence, &frame.header, &decoded.sink.blocks, &mut picture);
        let deblock = hashes(&picture);
        let deblocked = picture.clone();
        apply_intra_cdef(
            sequence,
            &frame.header,
            decoded.sink.cdef_grid(),
            &mut picture,
        );
        let cdef = hashes(&picture);
        apply_intra_loop_restoration(
            sequence,
            &frame.header,
            decoded.sink.lr_params(),
            &deblocked,
            &mut picture,
        )
        .unwrap();
        let restored = hashes(&picture);
        assert_eq!(
            pre,
            (0xd48aa2c6c48e1241, 0xe7d406c81d16d5f6, 0x578e37ee56d64664)
        );
        assert_eq!(
            deblock,
            (0xa7dc961959181516, 0xbfbe6a5cb6a81dce, 0x32ad7a6a2d669b99)
        );
        assert_eq!(
            cdef,
            (0xe8c54360ff2e7d14, 0xaf1944e6a1adb748, 0x4aabdb9970c7ea8f)
        );
        assert_eq!(restored, cdef);
    }

    #[test]
    fn real_keyframe_consumes_lr_units_and_restores_planes() {
        let data = include_bytes!("../../../tests/data/av1_lr_320x240.ivf");
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
        assert!(frame.header.uses_lr);
        assert!(frame.header.uses_chroma_lr);
        assert_eq!(sequence.color.num_planes, 3);
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
        assert!(decoded
            .sink
            .lr_params()
            .planes
            .iter()
            .any(|plane| plane.iter().any(Option::is_some)));

        let mut picture =
            reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks).unwrap();
        let hash = |data: &[u16]| {
            data.iter().fold(0xcbf29ce484222325u64, |hash, sample| {
                (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
            })
        };
        let pre = (
            hash(&picture.y.data),
            hash(&picture.u.as_ref().unwrap().data),
            hash(&picture.v.as_ref().unwrap().data),
        );
        apply_intra_deblock(sequence, &frame.header, &decoded.sink.blocks, &mut picture);
        let deblock = (
            hash(&picture.y.data),
            hash(&picture.u.as_ref().unwrap().data),
            hash(&picture.v.as_ref().unwrap().data),
        );
        let deblocked = picture.clone();
        apply_intra_cdef(
            sequence,
            &frame.header,
            decoded.sink.cdef_grid(),
            &mut picture,
        );
        let cdef_hash = (
            hash(&picture.y.data),
            hash(&picture.u.as_ref().unwrap().data),
            hash(&picture.v.as_ref().unwrap().data),
        );
        let cdef = picture.clone();
        apply_intra_loop_restoration(
            sequence,
            &frame.header,
            decoded.sink.lr_params(),
            &deblocked,
            &mut picture,
        )
        .unwrap();
        let restored = (
            hash(&picture.y.data),
            hash(&picture.u.as_ref().unwrap().data),
            hash(&picture.v.as_ref().unwrap().data),
        );
        assert_eq!(
            pre,
            (0xf6ea0e7d9ab20dad, 0x7fe43ca90fca0146, 0x9072f1cfe1e5e071)
        );
        assert_eq!(
            deblock,
            (0xd4bbbb62c0a92d13, 0x40b53d7345704318, 0x0011c4e31b77c872)
        );
        assert_eq!(
            cdef_hash,
            (0xa21d327c23420629, 0x51461930cdbf4c10, 0x200bfc6b85a9d2c2)
        );
        assert_eq!(
            restored,
            (0x486c310ad70a9309, 0x01d5a0ff5ad4894c, 0x0144ccde55fd6472)
        );
        assert_ne!(picture.y, cdef.y);
        assert_ne!(picture.u, cdef.u);
        assert_ne!(picture.v, cdef.v);
    }
}
