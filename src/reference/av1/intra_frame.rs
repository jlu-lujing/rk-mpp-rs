//! Frame-level driver for an intra (key-frame) AV1 picture.

use crate::reference::av1::cdef::{
    cdef_direction, cdef_filter_block, cdef_luma_primary_strength, CdefIndexGrid, CdefRegion,
    PlaneView, CDEF_UV_DIR,
};
use crate::reference::av1::cdf_state::CdfState;
use crate::reference::av1::deblock::{
    DeblockCell, DeblockEdgeInfo, DeblockGrid, DeblockPlane, SEG_LVL_ALT_LF_Y_V,
};
use crate::reference::av1::frame_header::FrameHeader;
use crate::reference::av1::intra::{IntraMode, UvMode};
use crate::reference::av1::intra_sink::IntraSyntaxBlock;
use crate::reference::av1::intra_sink::{IntraTileBlockSink, IntraTileConfig};
use crate::reference::av1::lr_frame::{apply_loop_restoration, LrFrameParams, LrPlaneGeometry};
use crate::reference::av1::reconstruct::{
    reconstruct_intra_block_plane_with_grid, DecodedPlane, PlaneQDelta, ReconstructedLuma,
};
use crate::reference::av1::residual::plane_tx_size;
use crate::reference::av1::segment::{SegmentCarry, SegmentMap};
use crate::reference::av1::seq_header::SequenceHeader;
use crate::reference::av1::tile_entropy::{
    decode_tile_partitions_with_cdfs, tile_bounds, TileEntropyError,
};
use crate::reference::av1::tile_group::ParsedTileGroup;
use crate::reference::av1::upscale::upscale_plane;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneBuffer {
    pub data: Vec<u16>,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl PlaneBuffer {
    pub(crate) fn new(width: usize, height: usize) -> Self {
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

pub(crate) fn uv_mode(mode: UvMode) -> Option<IntraMode> {
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

pub(crate) fn fill_modes(
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

/// The tile (in mi units) that owns one block origin, as `set_mi_row_col`
/// sees it. Blocks never straddle a tile boundary, so the block origin is
/// enough to find its tile.
pub(crate) fn tile_of(header: &FrameHeader, row: usize, col: usize) -> (usize, usize) {
    let start_at_or_before = |starts: &[usize], at: usize| {
        starts
            .iter()
            .copied()
            .take_while(|&start| start <= at)
            .last()
            .unwrap_or(0)
    };
    (
        start_at_or_before(&header.mi_row_starts, row),
        start_at_or_before(&header.mi_col_starts, col),
    )
}

pub(crate) fn neighbor_modes(
    grid: &[Option<IntraMode>],
    cols: usize,
    block: &IntraSyntaxBlock,
    tile: (usize, usize),
) -> (Option<IntraMode>, Option<IntraMode>) {
    let above = (block.block.row > tile.0)
        .then(|| grid[(block.block.row - 1) * cols + block.block.col])
        .flatten();
    let left = (block.block.col > tile.1)
        .then(|| grid[block.block.row * cols + block.block.col - 1])
        .flatten();
    (above, left)
}

pub(crate) fn chroma_neighbor_modes(
    grid: &[Option<IntraMode>],
    cols: usize,
    rows: usize,
    block: &IntraSyntaxBlock,
    sub_x: u32,
    sub_y: u32,
    tile: (usize, usize),
) -> (Option<IntraMode>, Option<IntraMode>) {
    let mut above_row = block.block.row as isize - 1;
    let mut above_col = block.block.col as isize;
    if sub_x != 0 && block.block.col & 1 == 0 {
        above_col += 1;
    }
    if sub_y != 0 && block.block.row & 1 != 0 {
        above_row -= 1;
    }

    let mut left_row = block.block.row as isize;
    let mut left_col = block.block.col as isize - 1;
    if sub_x != 0 && block.block.col & 1 != 0 {
        left_col -= 1;
    }
    if sub_y != 0 && block.block.row & 1 == 0 {
        left_row += 1;
    }

    // A sub-8 chroma block needs one extra mi of clearance inside the tile,
    // matching `chroma_{up,left}_available` in `set_mi_row_col`.
    let strict_up = sub_y != 0 && (block.block.size.num_4x4_high() as usize) < 2;
    let strict_left = sub_x != 0 && (block.block.size.num_4x4_wide() as usize) < 2;
    let in_tile = |row: isize, col: isize| {
        row as usize >= tile.0 + usize::from(strict_up)
            && col as usize >= tile.1 + usize::from(strict_left)
    };
    let at = |row: isize, col: isize| {
        (row >= 0 && col >= 0 && row < rows as isize && col < cols as isize && in_tile(row, col))
            .then(|| grid[row as usize * cols + col as usize])
            .flatten()
    };
    (at(above_row, above_col), at(left_row, left_col))
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
    let mut y_decoded = DecodedPlane::new(header.mi_cols, header.mi_rows, 0, 0);
    let mut u_decoded = DecodedPlane::new(header.mi_cols, header.mi_rows, sub_x, sub_y);
    let mut v_decoded = DecodedPlane::new(header.mi_cols, header.mi_rows, sub_x, sub_y);
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
        let block_tile = tile_of(header, block.block.row, block.block.col);
        let (y_above, y_left) = neighbor_modes(&y_modes, header.mi_cols, block, block_tile);
        reconstruct_intra_block_plane_with_grid(
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
            Some(&mut y_decoded),
            block_tile,
        )?;

        let (uv_above, uv_left) = chroma_neighbor_modes(
            &uv_modes,
            header.mi_cols,
            header.mi_rows,
            block,
            sub_x,
            sub_y,
            block_tile,
        );
        if block.transforms.iter().any(|tx| tx.plane == 1) {
            let u = frame
                .u
                .as_mut()
                .ok_or("chroma transform without a chroma plane")?;
            reconstruct_intra_block_plane_with_grid(
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
                Some(&mut u_decoded),
                block_tile,
            )?;
        }
        if block.transforms.iter().any(|tx| tx.plane == 2) {
            let v = frame
                .v
                .as_mut()
                .ok_or("chroma transform without a chroma plane")?;
            reconstruct_intra_block_plane_with_grid(
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
                Some(&mut v_decoded),
                block_tile,
            )?;
        }
    }
    Ok(frame)
}

fn deblock_grid(
    header: &FrameHeader,
    blocks: &[IntraSyntaxBlock],
    vertical: bool,
    plane: usize,
    sub_x: u32,
    sub_y: u32,
) -> DeblockGrid {
    let (plane_sub_x, plane_sub_y) = if plane == 0 { (0, 0) } else { (sub_x, sub_y) };
    let cell = DeblockCell {
        info: DeblockEdgeInfo::default(),
        apply_filter: false,
        filter_size: 4,
    };
    let mut grid = DeblockGrid::uniform(header.mi_cols, header.mi_rows, cell);
    let mut tx_extents = vec![0usize; header.mi_cols * header.mi_rows];
    for block in blocks {
        let w = block.block.size.num_4x4_wide() as usize;
        let h = block.block.size.num_4x4_high() as usize;
        let fallback_tx =
            plane_tx_size(block.block.size, plane, block.tx, plane_sub_x, plane_sub_y)
                .unwrap_or(block.tx);
        let extent = if vertical {
            fallback_tx.width() as usize
        } else {
            fallback_tx.height() as usize
        };
        for row in block.block.row..(block.block.row + h).min(header.mi_rows) {
            for col in block.block.col..(block.block.col + w).min(header.mi_cols) {
                tx_extents[row * header.mi_cols + col] = extent;
            }
        }
        if plane == 0 {
            for tx in block.transforms.iter().filter(|tx| tx.plane == 0) {
                let extent = if vertical {
                    tx.tx.width() as usize
                } else {
                    tx.tx.height() as usize
                };
                let w = tx.tx.width() as usize / 4;
                let h = tx.tx.height() as usize / 4;
                let base_row = tx.y / 4;
                let base_col = tx.x / 4;
                for row in base_row..(base_row + h).min(header.mi_rows) {
                    for col in base_col..(base_col + w).min(header.mi_cols) {
                        tx_extents[row * header.mi_cols + col] = extent;
                    }
                }
            }
        }
    }
    for block in blocks {
        let w = block.block.size.num_4x4_wide() as usize;
        let h = block.block.size.num_4x4_high() as usize;
        let block_extent = if vertical {
            (block.block.size.width as usize) >> plane_sub_x
        } else {
            (block.block.size.height as usize) >> plane_sub_y
        };
        let mut seg_lvl_alt_lf = [0i32; 4];
        let mut has_seg_lvl_alt_lf = false;
        for (feature, value) in seg_lvl_alt_lf.iter_mut().enumerate() {
            let feature = SEG_LVL_ALT_LF_Y_V + feature;
            if header.feature_enabled[block.segment_id][feature] {
                *value = header.feature_data[block.segment_id][feature];
                has_seg_lvl_alt_lf = true;
            }
        }
        let info = DeblockEdgeInfo {
            ref_frame: 0,
            mode_type: 0,
            delta_lf: block.delta_lf,
            seg_lvl_alt_lf: has_seg_lvl_alt_lf.then_some(seg_lvl_alt_lf),
        };
        for row in block.block.row..(block.block.row + h).min(header.mi_rows) {
            for col in block.block.col..(block.block.col + w).min(header.mi_cols) {
                let coordinate = if vertical {
                    (((col & !((1 << plane_sub_x) - 1))
                        - (block.block.col & !((1 << plane_sub_x) - 1)))
                        * 4)
                        >> plane_sub_x
                } else {
                    (((row & !((1 << plane_sub_y) - 1))
                        - (block.block.row & !((1 << plane_sub_y) - 1)))
                        * 4)
                        >> plane_sub_y
                };
                let current_tx_extent = tx_extents[row * header.mi_cols + col];
                let col_step = 1usize << plane_sub_x;
                let row_step = 1usize << plane_sub_y;
                let prev_tx_extent =
                    if (vertical && col < col_step) || (!vertical && row < row_step) {
                        current_tx_extent
                    } else {
                        let (prev_row, prev_col) = if vertical {
                            (row, col - col_step)
                        } else {
                            (row - row_step, col)
                        };
                        tx_extents[prev_row * header.mi_cols + prev_col]
                    };
                grid.cells[row * header.mi_cols + col] = DeblockCell::from_geometry(
                    coordinate,
                    plane,
                    block_extent,
                    current_tx_extent,
                    prev_tx_extent,
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
    let sub_x = u32::from(sequence.color.subsampling_x);
    let sub_y = u32::from(sequence.color.subsampling_y);
    let vertical = (0..sequence.color.num_planes)
        .map(|plane| deblock_grid(header, blocks, true, plane, sub_x, sub_y))
        .collect::<Vec<_>>();
    let horizontal = (0..sequence.color.num_planes)
        .map(|plane| deblock_grid(header, blocks, false, plane, sub_x, sub_y))
        .collect::<Vec<_>>();
    apply_deblock_grids(header, &vertical, &horizontal, sub_x, sub_y, frame);
}

pub(crate) fn apply_deblock_grids(
    header: &FrameHeader,
    vertical: &[DeblockGrid],
    horizontal: &[DeblockGrid],
    sub_x: u32,
    sub_y: u32,
    frame: &mut IntraFrame,
) {
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
    crate::reference::av1::deblock::loop_filter_frame_plane_grids(
        &mut planes,
        &header.loop_filter_params(),
        vertical,
        horizontal,
        frame.coded_width,
        frame.coded_height,
        sub_x,
        sub_y,
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
    blocks: &[IntraSyntaxBlock],
    cdef_grid: &CdefIndexGrid,
    frame: &mut IntraFrame,
) {
    if header.coded_lossless || header.allow_intrabc || !sequence.enable_cdef {
        return;
    }
    let mut skip_grid = vec![false; header.mi_cols * header.mi_rows];
    for block in blocks {
        let width = block.block.size.num_4x4_wide() as usize;
        let height = block.block.size.num_4x4_high() as usize;
        for row in block.block.row..(block.block.row + height).min(header.mi_rows) {
            for col in block.block.col..(block.block.col + width).min(header.mi_cols) {
                skip_grid[row * header.mi_cols + col] = block.skip;
            }
        }
    }
    apply_cdef_with_skip_grid(sequence, header, &skip_grid, cdef_grid, frame);
}

pub(crate) fn apply_cdef_with_skip_grid(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    skip_grid: &[bool],
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
    let coeff_shift = frame.bit_depth - 8;

    for r in (0..header.mi_rows).step_by(2) {
        for c in (0..header.mi_cols).step_by(2) {
            let Some(index) = cdef_grid.get(r & !15, c & !15) else {
                continue;
            };
            let skip = (0..2).all(|dy| {
                (0..2).all(|dx| {
                    skip_grid
                        .get((r + dy) * header.mi_cols + c + dx)
                        .copied()
                        .unwrap_or(false)
                })
            });
            if skip {
                continue;
            }
            let index = usize::from(index);
            let direction = cdef_direction(luma_input, c * 4, r * 4, frame.bit_depth);
            let primary = i32::from(header.cdef_y_pri_strength[index]) << coeff_shift;
            let luma_primary = cdef_luma_primary_strength(primary, direction.variance);
            let luma_direction = if primary == 0 { 0 } else { direction.dir };
            let luma = cdef_filter_block(
                luma_input,
                region,
                r,
                c,
                0,
                0,
                luma_primary,
                i32::from(header.cdef_y_sec_strength[index]) << coeff_shift,
                i32::from(header.cdef_damping) + coeff_shift as i32,
                luma_direction,
                frame.bit_depth,
            );
            write_block(&mut y_out, frame.y.stride, c * 4, r * 4, 8, &luma);

            let uv_primary = i32::from(header.cdef_uv_pri_strength[index]) << coeff_shift;
            let uv_dir = if uv_primary == 0 {
                0
            } else {
                CDEF_UV_DIR[sub_x as usize][sub_y as usize][usize::from(direction.dir)]
            };
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
                    uv_primary,
                    i32::from(header.cdef_uv_sec_strength[index]) << coeff_shift,
                    i32::from(header.cdef_damping) + coeff_shift as i32 - 1,
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

/// Apply §7.16 super-resolution to every plane of a reconstructed frame.
pub fn upscale_frame(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    input: &IntraFrame,
) -> Result<IntraFrame, &'static str> {
    if !header.use_superres {
        return Ok(input.clone());
    }
    let sub_x = u32::from(sequence.color.subsampling_x);
    let sub_y = u32::from(sequence.color.subsampling_y);
    let upscale =
        |data: &[u16], stride: usize, input_width: usize, plane_sub_x: u32, plane_sub_y: u32| {
            let plane = upscale_plane(
                data,
                stride,
                input_width,
                header.frame_width as usize,
                header.frame_height as usize,
                header.upscaled_width as usize,
                plane_sub_x,
                plane_sub_y,
                input.bit_depth,
            );
            PlaneBuffer {
                data: plane.data,
                width: plane.width,
                height: plane.height,
                stride: plane.stride,
            }
        };
    let u = input
        .u
        .as_ref()
        .map(|plane| upscale(&plane.data, plane.stride, plane.width, sub_x, sub_y));
    let v = input
        .v
        .as_ref()
        .map(|plane| upscale(&plane.data, plane.stride, plane.width, sub_x, sub_y));
    Ok(IntraFrame {
        coded_width: header.upscaled_width as usize,
        coded_height: header.frame_height as usize,
        viewport_width: header.upscaled_width as usize,
        viewport_height: header.frame_height as usize,
        bit_depth: input.bit_depth,
        y: upscale(&input.y.data, input.y.stride, input.y.width, 0, 0),
        u,
        v,
    })
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
    if !header.uses_lr && !header.use_superres {
        return Ok(());
    }
    let cdef_upscaled = header
        .use_superres
        .then(|| upscale_frame(sequence, header, frame))
        .transpose()?;
    let current_upscaled = header
        .use_superres
        .then(|| upscale_frame(sequence, header, current))
        .transpose()?;
    let cdef_frame = cdef_upscaled.as_ref().unwrap_or(frame);
    let current_frame = current_upscaled.as_ref().unwrap_or(current);
    if !header.uses_lr {
        *frame = cdef_frame.clone();
        return Ok(());
    }
    let geometry: Vec<LrPlaneGeometry> = (0..sequence.color.num_planes)
        .map(|plane| header.lr_geometry(plane, sequence))
        .collect();
    let mut cdef_planes = vec![crate::reference::av1::cdef::PlaneView::new(
        &cdef_frame.y.data,
        cdef_frame.y.stride,
        cdef_frame.y.width,
        cdef_frame.y.height,
    )];
    let mut current_planes = vec![crate::reference::av1::cdef::PlaneView::new(
        &current_frame.y.data,
        current_frame.y.stride,
        current_frame.y.width,
        current_frame.y.height,
    )];
    if let Some(u) = cdef_frame.u.as_ref() {
        cdef_planes.push(crate::reference::av1::cdef::PlaneView::new(
            &u.data, u.stride, u.width, u.height,
        ));
        let current = current_frame.u.as_ref().ok_or("missing current U plane")?;
        current_planes.push(crate::reference::av1::cdef::PlaneView::new(
            &current.data,
            current.stride,
            current.width,
            current.height,
        ));
    }
    if let Some(v) = cdef_frame.v.as_ref() {
        cdef_planes.push(crate::reference::av1::cdef::PlaneView::new(
            &v.data, v.stride, v.width, v.height,
        ));
        let current = current_frame.v.as_ref().ok_or("missing current V plane")?;
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
    let coded_size = (cdef_frame.coded_width, cdef_frame.coded_height);
    let viewport_size = (cdef_frame.viewport_width, cdef_frame.viewport_height);
    let y_geometry = (cdef_frame.y.width, cdef_frame.y.height, cdef_frame.y.stride);
    let u_geometry = cdef_frame
        .u
        .as_ref()
        .map(|plane| (plane.width, plane.height, plane.stride));
    let v_geometry = cdef_frame
        .v
        .as_ref()
        .map(|plane| (plane.width, plane.height, plane.stride));
    frame.coded_width = coded_size.0;
    frame.coded_height = coded_size.1;
    frame.viewport_width = viewport_size.0;
    frame.viewport_height = viewport_size.1;
    frame.y.data = out[0].clone();
    frame.y.width = y_geometry.0;
    frame.y.height = y_geometry.1;
    frame.y.stride = y_geometry.2;
    if let Some(u) = frame.u.as_mut() {
        u.data = out[1].clone();
        let geometry = u_geometry.unwrap();
        u.width = geometry.0;
        u.height = geometry.1;
        u.stride = geometry.2;
    }
    if let Some(v) = frame.v.as_mut() {
        v.data = out[2].clone();
        let geometry = v_geometry.unwrap();
        v.width = geometry.0;
        v.height = geometry.1;
        v.stride = geometry.2;
    }
    Ok(())
}

/// Frame-level syntax of one intra tile group.
///
/// The tiles of a tile group are independent entropy streams. libaom copies
/// the frame coding state into `tile_data->tctx` before every tile
/// (`decodeframe.c:3129`), so CDF adaptation never crosses a tile boundary,
/// and the state carried into the next frame is the one belonging to
/// `context_update_tile_id` rather than the last tile decoded
/// (`decodeframe.c:5683`).
pub struct IntraTileGroup {
    /// Leaf blocks of every tile, in tile decode (raster) order.
    pub blocks: Vec<IntraSyntaxBlock>,
    /// Union of the CDEF indices transmitted by the tiles.
    pub cdef: CdefIndexGrid,
    /// Union of the loop-restoration parameters read by the tiles.
    pub lr: LrFrameParams,
    /// Segment ids of the whole frame.
    pub segment_map: SegmentMap,
    /// Final CDF state of `header.context_update_tile_id`.
    pub cdfs: CdfState,
}

/// Decode every tile of an intra frame's tile group.
///
/// Each tile starts from `initial_cdfs`, exactly like libaom's per-tile copy
/// of the frame coding state, and the returned block list stays in decode
/// order so the frame-level reconstruction can keep using "already decoded"
/// availability for above, left and above-right neighbours.
pub fn decode_intra_tile_group(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    layout: &ParsedTileGroup,
    payload: &[u8],
    initial_cdfs: CdfState,
) -> Result<IntraTileGroup, TileEntropyError> {
    let config = IntraTileConfig::from_headers(sequence, header);
    let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
    let mut blocks = Vec::new();
    let mut cdef = CdefIndexGrid::new(header.mi_cols, header.mi_rows);
    let mut lr = LrFrameParams::for_geometry(&config.lr_geometry);
    let mut cdfs = initial_cdfs.clone();
    let mut segments: Option<SegmentCarry> = None;
    let mut segment_map: Option<SegmentMap> = None;
    for range in &layout.tiles {
        let bounds = tile_bounds(header, range.tile)?;
        let sink = IntraTileBlockSink::new(
            config
                .clone()
                .with_tile_origin(bounds.mi_row_start, bounds.mi_col_start),
        );
        let sink = match segments.take() {
            Some(carry) => sink.with_segment_carry(carry),
            None => sink,
        };
        let decoded = decode_tile_partitions_with_cdfs(
            &payload[range.bytes.clone()],
            header,
            range.tile,
            sb_size,
            header.disable_cdf_update,
            true,
            initial_cdfs.clone(),
            sink,
        )?;
        cdef.absorb_tile(
            decoded.sink.cdef_grid(),
            bounds.mi_row_start,
            bounds.mi_row_end,
            bounds.mi_col_start,
            bounds.mi_col_end,
        );
        merge_lr_params(&mut lr, decoded.sink.lr_params());
        segments = Some(decoded.sink.segment_carry());
        segment_map = Some(decoded.sink.segment_map());
        blocks.extend(decoded.sink.blocks);
        if range.tile == header.context_update_tile_id as usize {
            cdfs = decoded.cdfs;
        }
    }
    let segment_map =
        segment_map.unwrap_or_else(|| SegmentMap::new(header.mi_cols, header.mi_rows));
    Ok(IntraTileGroup {
        blocks,
        cdef,
        lr,
        segment_map,
        cdfs,
    })
}

/// Merge one tile's restoration parameters into the frame grid. A restoration
/// unit straddling a tile boundary is read by both tiles, and the last tile in
/// decode order is the one whose parameters libaom keeps.
pub(crate) fn merge_lr_params(frame: &mut LrFrameParams, tile: &LrFrameParams) {
    for (frame_plane, tile_plane) in frame.planes.iter_mut().zip(&tile.planes) {
        for (slot, value) in frame_plane.iter_mut().zip(tile_plane) {
            if value.is_some() {
                *slot = *value;
            }
        }
    }
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
            &decoded.sink.blocks,
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
            (0x68fc6d8755c7d35b, 0xcf3144357b71c9c7, 0x4857460a905f7092)
        );
        assert_eq!(
            deblock,
            (0xe7379bd094ffe54e, 0x75d6e8936c34e186, 0xcecd3e9c6c037bc8)
        );
        assert_eq!(
            cdef,
            (0xb84936b022b0f93a, 0x60a6b7d4f51e600c, 0xec0da83a8ba1394e)
        );
        assert_eq!(restored, cdef);
    }

    #[test]
    fn real_palette_and_delta_lf_keyframe_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_delta_lf_320x240.ivf");
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
        assert!(frame.header.delta_q_present);
        assert!(frame.header.delta_lf_present);

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
        let hash = |plane: &PlaneBuffer| {
            plane
                .data
                .iter()
                .fold(0xcbf29ce484222325u64, |hash, sample| {
                    (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
                })
        };
        let hashes = |picture: &IntraFrame| {
            (
                hash(&picture.y),
                hash(picture.u.as_ref().unwrap()),
                hash(picture.v.as_ref().unwrap()),
            )
        };

        assert_eq!(
            hashes(&picture),
            (0xffe3910c45a2ece7, 0xbf7b5fb6fb9f7e39, 0x0f9c50fac95c7c2c)
        );
        apply_intra_deblock(sequence, &frame.header, &decoded.sink.blocks, &mut picture);
        let deblocked = picture.clone();
        assert_eq!(
            hashes(&picture),
            (0xe741b59de826134e, 0xe20d5256a0dd85cd, 0x8c0e1a73fbd21dd1)
        );
        apply_intra_cdef(
            sequence,
            &frame.header,
            &decoded.sink.blocks,
            decoded.sink.cdef_grid(),
            &mut picture,
        );
        assert_eq!(
            hashes(&picture),
            (0xfa2f3c46bb96aa22, 0x4ec746390246903d, 0x6b11349dffa07825)
        );
        apply_intra_loop_restoration(
            sequence,
            &frame.header,
            decoded.sink.lr_params(),
            &deblocked,
            &mut picture,
        )
        .unwrap();
        assert_eq!(
            hashes(&picture),
            (0x249c4589ab9f3602, 0xa05707ca85dc367d, 0x58d1046d57bc1b52)
        );
    }

    #[test]
    fn real_delta_lf_multi_keyframe_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_delta_lf_multi_key_320x240.ivf");
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
        assert!(frame.header.delta_q_present);
        assert!(frame.header.delta_lf_present);
        assert!(frame.header.delta_lf_multi);

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
            .blocks
            .iter()
            .any(|block| block.delta_lf.iter().any(|&level| level != 0)));

        let mut picture =
            reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks).unwrap();
        let hash = |plane: &PlaneBuffer| {
            plane
                .data
                .iter()
                .fold(0xcbf29ce484222325u64, |hash, sample| {
                    (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
                })
        };
        let hashes = |picture: &IntraFrame| {
            (
                hash(&picture.y),
                hash(picture.u.as_ref().unwrap()),
                hash(picture.v.as_ref().unwrap()),
            )
        };

        assert_eq!(
            hashes(&picture),
            (0xeb9b2cc39c06674e, 0xc5ee9b96833e35f2, 0x96b616e2e5cdcb47)
        );
        apply_intra_deblock(sequence, &frame.header, &decoded.sink.blocks, &mut picture);
        let deblocked = picture.clone();
        assert_eq!(
            hashes(&picture),
            (0xdb03a8f1bb5fb80b, 0x6c2d4841f33c2335, 0xba08c50fdef95767)
        );
        apply_intra_cdef(
            sequence,
            &frame.header,
            &decoded.sink.blocks,
            decoded.sink.cdef_grid(),
            &mut picture,
        );
        assert_eq!(
            hashes(&picture),
            (0x299169e913b55a11, 0x9859f6acc76be850, 0x50ac86fa68ba1155)
        );
        apply_intra_loop_restoration(
            sequence,
            &frame.header,
            decoded.sink.lr_params(),
            &deblocked,
            &mut picture,
        )
        .unwrap();
        assert_eq!(
            hashes(&picture),
            (0x299169e913b55a11, 0x9859f6acc76be850, 0x50ac86fa68ba1155)
        );
    }

    #[test]
    fn real_monochrome_delta_lf_multi_keyframe_matches_libaom_y() {
        let data = include_bytes!("../../../tests/data/av1_delta_lf_multi_mono_key_320x240.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let packets = stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        assert!(sequence.color.mono_chrome);
        assert_eq!(sequence.color.num_planes, 1);
        let frame = packets
            .iter()
            .flatten()
            .find_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .unwrap();
        assert!(frame.header.delta_q_present);
        assert!(frame.header.delta_lf_present);
        assert!(frame.header.delta_lf_multi);

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
            .blocks
            .iter()
            .any(|block| block.delta_lf[..2].iter().any(|&level| level != 0)));
        assert!(decoded
            .sink
            .blocks
            .iter()
            .all(|block| block.delta_lf[2..] == [0, 0]));

        let mut picture =
            reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks).unwrap();
        assert!(picture.u.is_none());
        assert!(picture.v.is_none());
        let hash = |plane: &PlaneBuffer| {
            plane
                .data
                .iter()
                .fold(0xcbf29ce484222325u64, |hash, sample| {
                    (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
                })
        };

        assert_eq!(hash(&picture.y), 0xec18ba5f6a8f7560);
        apply_intra_deblock(sequence, &frame.header, &decoded.sink.blocks, &mut picture);
        let deblocked = picture.clone();
        assert_eq!(hash(&picture.y), 0xd64b69724edf8fb8);
        apply_intra_cdef(
            sequence,
            &frame.header,
            &decoded.sink.blocks,
            decoded.sink.cdef_grid(),
            &mut picture,
        );
        assert_eq!(hash(&picture.y), 0x5aad36d5f317ba64);
        apply_intra_loop_restoration(
            sequence,
            &frame.header,
            decoded.sink.lr_params(),
            &deblocked,
            &mut picture,
        )
        .unwrap();
        assert_eq!(hash(&picture.y), 0x5aad36d5f317ba64);
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
            &decoded.sink.blocks,
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
            (0x4ef4205df5509374, 0x7672c012a85d151a, 0xea2a1ba10c16ea31)
        );
        assert_eq!(
            deblock,
            (0x2244082b97a94d31, 0x966ede6f91454c70, 0x00f43f7954179382)
        );
        assert_eq!(
            cdef_hash,
            (0x6894001dae9173bb, 0x01282f6d0e8d5445, 0x2c54df968c17024b)
        );
        assert_eq!(
            restored,
            (0x9a98c493231c827b, 0xb2c1b028f85d848c, 0x255f3517f39effd7)
        );
        assert_ne!(picture.y, cdef.y);
        assert_ne!(picture.u, cdef.u);
        assert_ne!(picture.v, cdef.v);
    }

    #[test]
    fn real_two_by_two_tile_keyframe_matches_libaom() {
        let data = include_bytes!("../../../tests/data/av1_tiles_320x240_10.ivf");
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
        assert!(frame.header.frame_is_intra);
        assert_eq!((frame.header.tile_cols, frame.header.tile_rows), (2, 2));
        assert_eq!(frame.header.context_update_tile_id, 3);
        assert!(!frame.header.uses_lr);
        assert_eq!(frame.layout.tiles.len(), 4);

        let group = decode_intra_tile_group(
            sequence,
            &frame.header,
            &frame.layout,
            frame.payload,
            CdfState::new(),
        )
        .unwrap();

        // Each tile owns one leaf block at its own origin, so four origins
        // prove all four tile bitstreams were traversed. `decode_intra_tile_group`
        // verifies the exit symbol per tile, so no tile may have read past or
        // short of its own payload.
        for tile in 0..4 {
            let bounds = tile_bounds(&frame.header, tile).unwrap();
            assert!(
                group
                    .blocks
                    .iter()
                    .any(|block| block.block.row == bounds.mi_row_start
                        && block.block.col == bounds.mi_col_start),
                "tile {tile} must contribute a block at its own origin",
            );
        }

        let visible = |plane: &PlaneBuffer| {
            plane
                .data
                .chunks_exact(plane.stride)
                .take(plane.height)
                .flat_map(|row| &row[..plane.width])
                .fold(0xcbf29ce484222325u64, |hash, sample| {
                    (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
                })
        };
        let mut picture = reconstruct_intra_frame(sequence, &frame.header, &group.blocks).unwrap();
        apply_intra_deblock(sequence, &frame.header, &group.blocks, &mut picture);
        apply_intra_cdef(
            sequence,
            &frame.header,
            &group.blocks,
            &group.cdef,
            &mut picture,
        );
        // Locked against `aomdec --rawvideo` frame 0 (whole-frame MD5
        // 26ba5556975579b24a5cb591844f6226).
        assert_eq!(visible(&picture.y), 0xe4c15c7e09f4c973);
        assert_eq!(visible(picture.u.as_ref().unwrap()), 0x7f75e286318ce612);
        assert_eq!(visible(picture.v.as_ref().unwrap()), 0xb3d6a9d8663b3bc1);
    }

    #[test]
    fn real_superres_keyframe_matches_libaom_visible_pixels() {
        let data = include_bytes!("../../../tests/data/av1_superres_320x240.ivf");
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
        assert!(frame.header.use_superres);
        assert_eq!(frame.header.superres_denom, 12);
        assert_eq!(
            (
                frame.header.frame_width,
                frame.header.upscaled_width,
                frame.header.frame_height
            ),
            (213, 320, 240)
        );

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
        let visible_hash = |plane: &PlaneBuffer, width: usize, height: usize| {
            plane
                .data
                .chunks_exact(plane.stride)
                .take(height)
                .flat_map(|row| &row[..width])
                .fold(0xcbf29ce484222325u64, |hash, sample| {
                    (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
                })
        };
        let hashes = |picture: &IntraFrame, width: usize, height: usize| {
            let sub_x = u32::from(sequence.color.subsampling_x);
            let sub_y = u32::from(sequence.color.subsampling_y);
            let chroma_width = (width + (1 << sub_x) - 1) >> sub_x;
            let chroma_height = (height + (1 << sub_y) - 1) >> sub_y;
            (
                visible_hash(&picture.y, width, height),
                visible_hash(picture.u.as_ref().unwrap(), chroma_width, chroma_height),
                visible_hash(picture.v.as_ref().unwrap(), chroma_width, chroma_height),
            )
        };

        let mut picture =
            reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks).unwrap();
        assert_eq!(
            hashes(
                &picture,
                frame.header.frame_width as usize,
                frame.header.frame_height as usize
            ),
            (0x35b16aec62f2f764, 0x7ca6039e9e84c9fe, 0x794e83e36b04acea)
        );
        apply_intra_deblock(sequence, &frame.header, &decoded.sink.blocks, &mut picture);
        let deblocked = picture.clone();
        apply_intra_cdef(
            sequence,
            &frame.header,
            &decoded.sink.blocks,
            decoded.sink.cdef_grid(),
            &mut picture,
        );
        apply_intra_loop_restoration(
            sequence,
            &frame.header,
            decoded.sink.lr_params(),
            &deblocked,
            &mut picture,
        )
        .unwrap();
        assert_eq!(
            (
                picture.viewport_width,
                picture.viewport_height,
                picture.y.width,
                picture.y.height
            ),
            (320, 240, 320, 240)
        );
        assert_eq!(
            hashes(
                &picture,
                frame.header.upscaled_width as usize,
                frame.header.frame_height as usize
            ),
            (0xf09348412b37d47c, 0x83d310531bebf8ae, 0x0d5e3aa6ca67a8de)
        );
    }
}
