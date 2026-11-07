//! AV1 loop filter (deblocking), specification section 7.14.
//!
//! The frame walk, adaptive strength, filter masks and the narrow/wide sample
//! filters follow the specification pseudocode verbatim. The per-4x4 mode-info
//! arrays (`MiSizes`, `LoopfilterTxSizes`, `Skips`, `RefFrames`, `YModes`,
//! `SegmentIds`, `DeltaLFs`) are *injected* through [`DeblockGrid`]: decoding
//! them needs the frame header and tile syntax that this slice does not own.
//! [`build_cell`] turns their pixel geometry into the `isBlockEdge`/`isTxEdge`
//! decision and the filter size, so a tile driver only has to supply sizes.
//!
//! Sample reads outside a plane clamp to its border. The specification assumes
//! the frame has border samples for the 7-tap wide filter; edge replication is
//! the equivalent when working on a plain `Vec<u16>`.

use crate::reference::av1::cdef::PlaneView;
use crate::reference::av1::lr_syntax::clip3;

/// `MAX_LOOP_FILTER`.
pub const MAX_LOOP_FILTER: i32 = 63;
/// `MI_SIZE`.
pub const MI_SIZE: usize = 4;
/// `INTRA_FRAME`, the first entry of the reference-frame enumeration.
pub const INTRA_FRAME: usize = 0;
/// `TOTAL_REFS_PER_FRAME`.
pub const TOTAL_REFS_PER_FRAME: usize = 8;
/// `SEG_LVL_ALT_LF_Y_V`: index of the vertical luma filter segment feature.
pub const SEG_LVL_ALT_LF_Y_V: usize = 1;

/// `Round2(value, bits)` for non-negative filter accumulators and signed
/// narrow-filter taps: `(value + (1 << (bits - 1))) >> bits`.
pub const fn round2_i32(value: i32, bits: u32) -> i32 {
    if bits == 0 {
        value
    } else {
        (value + (1i32 << (bits - 1))) >> bits
    }
}

/// Frame-header loop filter parameters (`loop_filter_params()`), plus the
/// `delta_lf_multi` flag from the frame header. `level[i]` is indexed by
/// `(plane == 0) ? pass : plane + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopFilterParams {
    pub level: [i32; 4],
    pub sharpness: u32,
    pub delta_enabled: bool,
    pub ref_deltas: [i32; TOTAL_REFS_PER_FRAME],
    pub mode_deltas: [i32; 2],
    pub delta_lf_multi: bool,
}

impl LoopFilterParams {
    /// The values `loop_filter_params()` assigns for `CodedLossless` or
    /// `allow_intrabc`, where no syntax is read at all.
    pub fn lossless_defaults() -> Self {
        Self {
            level: [0; 4],
            sharpness: 0,
            delta_enabled: false,
            // INTRA, LAST, LAST2, LAST3, GOLDEN, BWDREF, ALTREF2, ALTREF
            ref_deltas: [1, 0, 0, 0, -1, 0, -1, -1],
            mode_deltas: [0, 0],
            delta_lf_multi: false,
        }
    }

    /// Index into `DeltaLFs[row][col][...]` used by the adaptive strength
    /// process: `DeltaLFs[...][0]` without `delta_lf_multi`, otherwise
    /// `pass` for luma and `plane + 1` for chroma.
    pub const fn delta_lf_index(self, plane: usize, pass: usize) -> usize {
        if !self.delta_lf_multi {
            0
        } else if plane == 0 {
            pass
        } else {
            plane + 1
        }
    }

    /// `adaptive_filter_strength_selection`: base level, segment feature, then
    /// the reference/mode deltas scaled by `lvlSeg >> 5`.
    pub fn select_level(self, plane: usize, pass: usize, info: &DeblockEdgeInfo) -> i32 {
        let i = if plane == 0 { pass } else { plane + 1 };
        let delta_lf = info.delta_lf[self.delta_lf_index(plane, pass)];
        let mut level = clip3(0, MAX_LOOP_FILTER, delta_lf + self.level[i]);
        if let Some(feature_data) = info.seg_lvl_alt_lf {
            level = clip3(0, MAX_LOOP_FILTER, feature_data[i] + level);
        }
        if self.delta_enabled {
            let n_shift = level >> 5;
            level += if info.ref_frame == INTRA_FRAME {
                self.ref_deltas[INTRA_FRAME] << n_shift
            } else {
                (self.ref_deltas[info.ref_frame] << n_shift)
                    + (self.mode_deltas[info.mode_type] << n_shift)
            };
            level = clip3(0, MAX_LOOP_FILTER, level);
        }
        level
    }

    /// `adaptive_filter_strength`: level plus `shift`, `limit`, `blimit` and
    /// `thresh` derived from `loop_filter_sharpness`.
    pub fn strength(self, plane: usize, pass: usize, info: &DeblockEdgeInfo) -> FilterStrength {
        let lvl = self.select_level(plane, pass, info);
        let shift = if self.sharpness > 4 {
            2
        } else if self.sharpness > 0 {
            1
        } else {
            0
        };
        let limit = if self.sharpness > 0 {
            clip3(1, 9 - self.sharpness as i32, lvl >> shift)
        } else {
            (lvl >> shift).max(1)
        };
        FilterStrength {
            lvl,
            limit,
            blimit: 2 * (lvl + 2) + limit,
            thresh: lvl >> 4,
        }
    }
}

/// Outputs of the adaptive filter strength process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterStrength {
    pub lvl: i32,
    pub limit: i32,
    pub blimit: i32,
    pub thresh: i32,
}

/// The mode-info values the strength process reads for one 4x4 luma block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeblockEdgeInfo {
    pub ref_frame: usize,
    pub mode_type: usize,
    pub delta_lf: [i32; 4],
    /// The result of `seg_feature_active_idx(segment, SEG_LVL_ALT_LF_Y_V+i)`:
    /// `Some(FeatureData[segment][...])` when active, `None` otherwise.
    pub seg_lvl_alt_lf: Option<[i32; 4]>,
}

impl Default for DeblockEdgeInfo {
    fn default() -> Self {
        Self {
            ref_frame: INTRA_FRAME,
            mode_type: 0,
            delta_lf: [0; 4],
            seg_lvl_alt_lf: None,
        }
    }
}

/// `filter_size` process: `Min(Tx_Width[prevTxSz], Tx_Width[txSz])` capped at
/// 16 for luma and 8 for chroma.
pub const fn filter_size(tx_extent: usize, prev_tx_extent: usize, plane: usize) -> usize {
    let base = if tx_extent < prev_tx_extent {
        tx_extent
    } else {
        prev_tx_extent
    };
    if plane == 0 {
        if base < 16 {
            base
        } else {
            16
        }
    } else if base < 8 {
        base
    } else {
        8
    }
}

/// One 4x4 cell of the injected mode-info grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeblockCell {
    pub info: DeblockEdgeInfo,
    /// `isTxEdge && (isBlockEdge || !skip || isIntra)`.
    pub apply_filter: bool,
    /// Output of the filter size process for this edge direction.
    pub filter_size: usize,
}

impl DeblockCell {
    /// Build a cell from the pixel geometry the edge process reads. Callers
    /// supply the extents for their edge direction: `xP`/`Block_Width`/
    /// `Tx_Width` for pass 0, `yP`/`Block_Height`/`Tx_Height` for pass 1.
    /// `tx_extent`/`prev_tx_extent` are `Tx_Width`/`Tx_Height` on either side
    /// of the edge.
    #[allow(clippy::too_many_arguments)]
    pub fn from_geometry(
        coordinate: usize,
        plane: usize,
        block_extent: usize,
        tx_extent: usize,
        prev_tx_extent: usize,
        skip: bool,
        is_intra: bool,
        info: DeblockEdgeInfo,
    ) -> Self {
        let is_block_edge = coordinate % block_extent == 0;
        let is_tx_edge = coordinate % tx_extent == 0;
        Self {
            info,
            apply_filter: is_tx_edge && (is_block_edge || !skip || is_intra),
            filter_size: filter_size(tx_extent, prev_tx_extent, plane),
        }
    }
}

/// `MiSizes` / strength metadata laid out in 4x4 luma units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeblockGrid {
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub cells: Vec<DeblockCell>,
}

impl DeblockGrid {
    pub fn uniform(mi_cols: usize, mi_rows: usize, cell: DeblockCell) -> Self {
        Self {
            mi_cols,
            mi_rows,
            cells: vec![cell; mi_cols * mi_rows],
        }
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&DeblockCell> {
        if row >= self.mi_rows || col >= self.mi_cols {
            return None;
        }
        self.cells.get(row * self.mi_cols + col)
    }
}

/// A mutable reconstructed plane. `get`/`set` clamp coordinates to the plane
/// so the wide filter's seven taps work at frame borders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeblockPlane {
    pub samples: Vec<u16>,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
    pub bit_depth: u32,
}

impl DeblockPlane {
    pub fn new(
        samples: &[u16],
        stride: usize,
        width: usize,
        height: usize,
        bit_depth: u32,
    ) -> Self {
        assert!(stride >= width);
        assert!(samples.len() >= stride * height);
        assert!((8..=12).contains(&bit_depth));
        Self {
            samples: samples.to_vec(),
            stride,
            width,
            height,
            bit_depth,
        }
    }

    pub fn get(&self, x: isize, y: isize) -> i32 {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        i32::from(self.samples[y * self.stride + x])
    }

    pub fn set(&mut self, x: isize, y: isize, value: i32) {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        let max = (1i32 << self.bit_depth) - 1;
        self.samples[y * self.stride + x] = value.clamp(0, max) as u16;
    }

    /// Hand the deblocked samples to CDEF / loop restoration.
    pub fn view(&self) -> PlaneView<'_> {
        PlaneView::new(&self.samples, self.stride, self.width, self.height)
    }
}

/// `p0..p6` and `q0..q6` across one edge, as the filter mask process names
/// them: `p[k]` is `-(k+1)` steps from the boundary and `q[k]` is `+k`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeSamples {
    pub p: [i32; 7],
    pub q: [i32; 7],
}

impl EdgeSamples {
    pub fn read(plane: &DeblockPlane, x: isize, y: isize, dx: isize, dy: isize) -> Self {
        let mut p = [0i32; 7];
        let mut q = [0i32; 7];
        for k in 0..7 {
            let step = k as isize + 1;
            p[k] = plane.get(x - dx * step, y - dy * step);
            let forward = k as isize;
            q[k] = plane.get(x + dx * forward, y + dy * forward);
        }
        Self { p, q }
    }
}

/// Outputs of the filter mask process. `flat`/`flat2` are `true` when the
/// corresponding region is flat (the specification's masks are 1 for flat).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterMasks {
    pub hev: bool,
    pub filter: bool,
    pub flat: bool,
    pub flat2: bool,
}

fn abs_diff(a: i32, b: i32) -> i32 {
    (a - b).abs()
}

/// `filter_mask` process (§7.14.6.2).
pub fn filter_mask(
    samples: &EdgeSamples,
    bit_depth: u32,
    strength: FilterStrength,
    filter_size: usize,
    plane: usize,
) -> FilterMasks {
    let (p, q) = (&samples.p, &samples.q);
    let bd_shift = bit_depth - 8;
    let thresh_bd = strength.thresh << bd_shift;
    let hev = abs_diff(p[1], p[0]) > thresh_bd || abs_diff(q[1], q[0]) > thresh_bd;
    let filter_len = if filter_size == 4 {
        4
    } else if plane != 0 {
        6
    } else if filter_size == 8 {
        8
    } else {
        16
    };

    let limit_bd = strength.limit << bd_shift;
    let blimit_bd = strength.blimit << bd_shift;
    let mut mask = false;
    mask |= abs_diff(p[1], p[0]) > limit_bd;
    mask |= abs_diff(q[1], q[0]) > limit_bd;
    mask |= abs_diff(p[0], q[0]) * 2 + abs_diff(p[1], q[1]) / 2 > blimit_bd;
    if filter_len >= 6 {
        mask |= abs_diff(p[2], p[1]) > limit_bd;
        mask |= abs_diff(q[2], q[1]) > limit_bd;
    }
    if filter_len >= 8 {
        mask |= abs_diff(p[3], p[2]) > limit_bd;
        mask |= abs_diff(q[3], q[2]) > limit_bd;
    }

    let threshold_bd = 1i32 << bd_shift;
    let mut flat = false;
    if filter_size >= 8 {
        let mut flat_mask = false;
        flat_mask |= abs_diff(p[1], p[0]) > threshold_bd;
        flat_mask |= abs_diff(q[1], q[0]) > threshold_bd;
        flat_mask |= abs_diff(p[2], p[0]) > threshold_bd;
        flat_mask |= abs_diff(q[2], q[0]) > threshold_bd;
        if filter_len >= 8 {
            flat_mask |= abs_diff(p[3], p[0]) > threshold_bd;
            flat_mask |= abs_diff(q[3], q[0]) > threshold_bd;
        }
        flat = !flat_mask;
    }
    let mut flat2 = false;
    if filter_size >= 16 {
        let mut flat_mask2 = false;
        for k in 4..=6 {
            flat_mask2 |= abs_diff(p[k], p[0]) > threshold_bd;
            flat_mask2 |= abs_diff(q[k], q[0]) > threshold_bd;
        }
        flat2 = !flat_mask2;
    }
    FilterMasks {
        hev,
        filter: !mask,
        flat,
        flat2,
    }
}

/// `narrow_filter` (§7.14.6.3), the `filter4` core.
pub fn narrow_filter(
    plane: &mut DeblockPlane,
    x: isize,
    y: isize,
    dx: isize,
    dy: isize,
    hev: bool,
) {
    let bd = plane.bit_depth;
    let half = 0x80i32 << (bd - 8);
    let half_range = 1i32 << (bd - 1);
    let clamp4 = |value: i32| clip3(-half_range, half_range - 1, value);

    let q0 = plane.get(x, y);
    let q1 = plane.get(x + dx, y + dy);
    let p0 = plane.get(x - dx, y - dy);
    let p1 = plane.get(x - dx * 2, y - dy * 2);
    let ps1 = p1 - half;
    let ps0 = p0 - half;
    let qs0 = q0 - half;
    let qs1 = q1 - half;

    let mut filter = if hev { clamp4(ps1 - qs1) } else { 0 };
    filter = clamp4(filter + 3 * (qs0 - ps0));
    let filter1 = clamp4(filter + 4) >> 3;
    let filter2 = clamp4(filter + 3) >> 3;
    let oq0 = clamp4(qs0 - filter1) + half;
    let op0 = clamp4(ps0 + filter2) + half;
    plane.set(x, y, oq0);
    plane.set(x - dx, y - dy, op0);

    if !hev {
        let filter = round2_i32(filter1, 1);
        let oq1 = clamp4(qs1 - filter) + half;
        let op1 = clamp4(ps1 + filter) + half;
        plane.set(x + dx, y + dy, oq1);
        plane.set(x - dx * 2, y - dy * 2, op1);
    }
}

/// `wide_filter` (§7.14.6.4), with `log2Size` 3 or 4.
pub fn wide_filter(
    plane: &mut DeblockPlane,
    x: isize,
    y: isize,
    dx: isize,
    dy: isize,
    plane_index: usize,
    log2_size: u32,
) {
    let n = if log2_size == 4 {
        6
    } else if plane_index == 0 {
        3
    } else {
        2
    };
    let n2 = if log2_size == 3 && plane_index == 0 {
        0
    } else {
        1
    };
    let mut filtered = [0i32; 12];
    for i in -n..n {
        let mut total = 0i32;
        for j in -n..=n {
            let p = clip3(-(n + 1), n, i + j) as isize;
            let tap = if j.abs() <= n2 { 2 } else { 1 };
            total += plane.get(x + p * dx, y + p * dy) * tap;
        }
        filtered[(i + n) as usize] = round2_i32(total, log2_size);
    }
    for i in -n..n {
        let step = i as isize;
        plane.set(x + step * dx, y + step * dy, filtered[(i + n) as usize]);
    }
}

/// `sample_filtering` (§7.14.6): choose the narrow or wide filter from the
/// masks and the maximum filter size.
pub fn sample_filtering(
    plane: &mut DeblockPlane,
    x: isize,
    y: isize,
    dx: isize,
    dy: isize,
    plane_index: usize,
    strength: FilterStrength,
    max_filter_size: usize,
) {
    let samples = EdgeSamples::read(plane, x, y, dx, dy);
    let masks = filter_mask(
        &samples,
        plane.bit_depth,
        strength,
        max_filter_size,
        plane_index,
    );
    if !masks.filter {
        return;
    }
    if max_filter_size == 4 || !masks.flat {
        narrow_filter(plane, x, y, dx, dy, masks.hev);
    } else if max_filter_size == 8 || !masks.flat2 {
        wide_filter(plane, x, y, dx, dy, plane_index, 3);
    } else {
        wide_filter(plane, x, y, dx, dy, plane_index, 4);
    }
}

/// `loop_filter_edge` (§7.14.2) for one plane/pass/4x4 position.
#[allow(clippy::too_many_arguments)]
pub fn filter_edge(
    plane: &mut DeblockPlane,
    plane_index: usize,
    pass: usize,
    row: usize,
    col: usize,
    params: &LoopFilterParams,
    grid: &DeblockGrid,
    frame_width: usize,
    frame_height: usize,
    sub_x: u32,
    sub_y: u32,
) {
    let (dx, dy) = if pass == 0 { (1isize, 0isize) } else { (0, 1) };
    let x = col * MI_SIZE;
    let y = row * MI_SIZE;
    // `onScreen`: both sides of the boundary must be visible.
    if x >= frame_width || y >= frame_height || (pass == 0 && x == 0) || (pass == 1 && y == 0) {
        return;
    }
    let x_p = (x >> sub_x) as isize;
    let y_p = (y >> sub_y) as isize;
    let row = row | sub_y as usize;
    let col = col | sub_x as usize;
    let Some(cell) = grid.cell(row, col) else {
        return;
    };

    let mut strength = params.strength(plane_index, pass, &cell.info);
    if strength.lvl == 0 {
        let prev_row = row.wrapping_sub((dy as usize) << sub_y);
        let prev_col = col.wrapping_sub((dx as usize) << sub_x);
        if let Some(prev) = grid.cell(prev_row, prev_col) {
            strength = params.strength(plane_index, pass, &prev.info);
        }
    }
    if !cell.apply_filter || strength.lvl <= 0 {
        return;
    }
    for i in 0..MI_SIZE as isize {
        let sx = x_p + dy * i;
        let sy = y_p + dx * i;
        sample_filtering(
            plane,
            sx,
            sy,
            dx,
            dy,
            plane_index,
            strength,
            cell.filter_size,
        );
    }
}

/// `loop_filter_frame` (§7.14.1): all vertical boundaries before all
/// horizontal boundaries, for every enabled plane.
pub fn loop_filter_frame(
    planes: &mut [DeblockPlane],
    params: &LoopFilterParams,
    grid: &DeblockGrid,
    frame_width: usize,
    frame_height: usize,
    subsampling_x: u32,
    subsampling_y: u32,
) {
    if params.level[0] == 0 && params.level[1] == 0 {
        return;
    }
    for (index, plane) in planes.iter_mut().enumerate() {
        if index != 0 && params.level[index + 1] == 0 {
            continue;
        }
        let (sub_x, sub_y) = if index == 0 {
            (0, 0)
        } else {
            (subsampling_x, subsampling_y)
        };
        let row_step = 1usize << sub_y;
        let col_step = 1usize << sub_x;
        for pass in 0..2 {
            let mut row = 0;
            while row < grid.mi_rows {
                let mut col = 0;
                while col < grid.mi_cols {
                    filter_edge(
                        plane,
                        index,
                        pass,
                        row,
                        col,
                        params,
                        grid,
                        frame_width,
                        frame_height,
                        sub_x,
                        sub_y,
                    );
                    col += col_step;
                }
                row += row_step;
            }
        }
    }
}

/// `loop_filter_frame` with separate mode-info grids for the vertical and
/// horizontal passes. Rectangular blocks need this because their block and
/// transform extents differ by direction.
pub fn loop_filter_frame_dual_grid(
    planes: &mut [DeblockPlane],
    params: &LoopFilterParams,
    vertical_grid: &DeblockGrid,
    horizontal_grid: &DeblockGrid,
    frame_width: usize,
    frame_height: usize,
    subsampling_x: u32,
    subsampling_y: u32,
) {
    if params.level[0] == 0 && params.level[1] == 0 {
        return;
    }
    assert_eq!(vertical_grid.mi_cols, horizontal_grid.mi_cols);
    assert_eq!(vertical_grid.mi_rows, horizontal_grid.mi_rows);
    for (index, plane) in planes.iter_mut().enumerate() {
        if index != 0 && params.level[index + 1] == 0 {
            continue;
        }
        let (sub_x, sub_y) = if index == 0 {
            (0, 0)
        } else {
            (subsampling_x, subsampling_y)
        };
        let row_step = 1usize << sub_y;
        let col_step = 1usize << sub_x;
        for pass in 0..2 {
            let grid = if pass == 0 {
                vertical_grid
            } else {
                horizontal_grid
            };
            let mut row = 0;
            while row < grid.mi_rows {
                let mut col = 0;
                while col < grid.mi_cols {
                    filter_edge(
                        plane,
                        index,
                        pass,
                        row,
                        col,
                        params,
                        grid,
                        frame_width,
                        frame_height,
                        sub_x,
                        sub_y,
                    );
                    col += col_step;
                }
                row += row_step;
            }
        }
    }
}

/// Like [`loop_filter_frame_dual_grid`], with separate mode-info grids for each
/// plane. Chroma transform sizes are derived after subsampling and can differ
/// from the luma grid.
#[allow(clippy::too_many_arguments)]
pub fn loop_filter_frame_plane_grids(
    planes: &mut [DeblockPlane],
    params: &LoopFilterParams,
    vertical_grids: &[DeblockGrid],
    horizontal_grids: &[DeblockGrid],
    frame_width: usize,
    frame_height: usize,
    subsampling_x: u32,
    subsampling_y: u32,
) {
    if params.level[0] == 0 && params.level[1] == 0 {
        return;
    }
    assert_eq!(vertical_grids.len(), planes.len());
    assert_eq!(horizontal_grids.len(), planes.len());
    for (index, plane) in planes.iter_mut().enumerate() {
        if index != 0 && params.level[index + 1] == 0 {
            continue;
        }
        let (sub_x, sub_y) = if index == 0 {
            (0, 0)
        } else {
            (subsampling_x, subsampling_y)
        };
        let row_step = 1usize << sub_y;
        let col_step = 1usize << sub_x;
        for pass in 0..2 {
            let grid = if pass == 0 {
                &vertical_grids[index]
            } else {
                &horizontal_grids[index]
            };
            let mut row = 0;
            while row < grid.mi_rows {
                let mut col = 0;
                while col < grid.mi_cols {
                    filter_edge(
                        plane,
                        index,
                        pass,
                        row,
                        col,
                        params,
                        grid,
                        frame_width,
                        frame_height,
                        sub_x,
                        sub_y,
                    );
                    col += col_step;
                }
                row += row_step;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::lr_frame::{apply_loop_restoration, LrFrameParams, LrPlaneGeometry};
    use crate::reference::av1::restoration::RestorationType;

    fn params(level: i32) -> LoopFilterParams {
        LoopFilterParams {
            level: [level, level, level, level],
            sharpness: 0,
            delta_enabled: false,
            ..LoopFilterParams::lossless_defaults()
        }
    }

    fn step_plane() -> DeblockPlane {
        // 16x16 luma with a ten-level step at x = 8.
        let mut samples = vec![0u16; 256];
        for y in 0..16 {
            for x in 0..16 {
                samples[y * 16 + x] = if x < 8 { 100 } else { 110 };
            }
        }
        DeblockPlane::new(&samples, 16, 16, 16, 8)
    }

    #[test]
    fn lossless_defaults_match_loop_filter_params() {
        let p = LoopFilterParams::lossless_defaults();
        assert_eq!(p.level, [0; 4]);
        assert_eq!(p.ref_deltas, [1, 0, 0, 0, -1, 0, -1, -1]);
        assert_eq!(p.mode_deltas, [0, 0]);
        assert!(!p.delta_enabled);
    }

    #[test]
    fn strength_follows_sharpness_and_level() {
        let info = DeblockEdgeInfo::default();
        let plain = params(20).strength(0, 0, &info);
        assert_eq!(plain.lvl, 20);
        assert_eq!(plain.limit, 20);
        assert_eq!(plain.blimit, 2 * 22 + 20);
        assert_eq!(plain.thresh, 1);

        let mut sharp3 = params(20);
        sharp3.sharpness = 3;
        assert_eq!(sharp3.strength(0, 0, &info).limit, clip3(1, 6, 10));
        assert_eq!(sharp3.strength(0, 0, &info).blimit, 44 + 6);

        let mut sharp5 = params(20);
        sharp5.sharpness = 5;
        assert_eq!(sharp5.strength(0, 0, &info).limit, clip3(1, 4, 5));
        assert_eq!(sharp5.strength(0, 0, &info).blimit, 44 + 4);
    }

    #[test]
    fn ref_and_mode_deltas_are_shifted_by_the_level() {
        // lvlSeg = 40 -> nShift = 1, so every delta doubles.
        let mut p = params(40);
        p.delta_enabled = true;
        p.ref_deltas = [3, 2, 0, 0, 0, 0, 0, 0];
        p.mode_deltas = [0, -1];

        let intra = DeblockEdgeInfo::default();
        assert_eq!(p.select_level(0, 0, &intra), 40 + (3 << 1));

        let inter = DeblockEdgeInfo {
            ref_frame: 1,
            mode_type: 1,
            ..DeblockEdgeInfo::default()
        };
        assert_eq!(p.select_level(0, 0, &inter), 40 + (2 << 1) + (-1 << 1));

        // The result is clamped to MAX_LOOP_FILTER.
        let mut high = params(60);
        high.delta_enabled = true;
        high.ref_deltas = [10, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(high.select_level(0, 0, &intra), MAX_LOOP_FILTER);
    }

    #[test]
    fn segment_feature_and_delta_lf_are_applied_in_order() {
        let mut p = params(10);
        let info = DeblockEdgeInfo {
            delta_lf: [3, 0, 0, 0],
            seg_lvl_alt_lf: Some([5, 0, 0, 0]),
            ..DeblockEdgeInfo::default()
        };
        // deltaLF is folded into the base level, then the segment feature.
        assert_eq!(p.select_level(0, 0, &info), 18);

        p.delta_lf_multi = true;
        assert_eq!(p.delta_lf_index(0, 0), 0);
        assert_eq!(p.delta_lf_index(0, 1), 1);
        assert_eq!(p.delta_lf_index(1, 0), 2);
        assert_eq!(p.delta_lf_index(2, 1), 3);
        let multi = DeblockEdgeInfo {
            delta_lf: [3, 1, 2, 4],
            ..DeblockEdgeInfo::default()
        };
        assert_eq!(p.select_level(0, 1, &multi), 11);
        assert_eq!(p.select_level(1, 0, &multi), 12);
    }

    #[test]
    fn filter_size_caps_luma_at_16_and_chroma_at_8() {
        assert_eq!(filter_size(16, 16, 0), 16);
        assert_eq!(filter_size(32, 8, 0), 8);
        assert_eq!(filter_size(64, 16, 0), 16);
        assert_eq!(filter_size(16, 16, 1), 8);
        assert_eq!(filter_size(4, 4, 0), 4);
    }

    #[test]
    fn cell_geometry_computes_the_edge_decision() {
        let info = DeblockEdgeInfo::default();
        // xP = 8 lies on an 8-wide block edge but not on a 16-wide transform
        // edge: without skip/intra there is no filter.
        let cell = DeblockCell::from_geometry(8, 0, 8, 16, 16, true, false, info);
        assert!(!cell.apply_filter);
        assert_eq!(cell.filter_size, 16);
        // A 4x4 transform edge at xP = 4 with a residual is a filter edge.
        let cell = DeblockCell::from_geometry(4, 0, 8, 4, 4, false, false, info);
        assert!(cell.apply_filter);
        assert_eq!(cell.filter_size, 4);
        // Intra blocks are filtered regardless of skip.
        let cell = DeblockCell::from_geometry(8, 0, 8, 16, 16, true, true, info);
        assert!(!cell.apply_filter, "isTxEdge still gates the decision");
        let cell = DeblockCell::from_geometry(8, 0, 8, 8, 8, true, true, info);
        assert!(cell.apply_filter);
    }

    #[test]
    fn masks_accept_flat_regions_and_reject_large_steps() {
        let strength = params(20).strength(0, 0, &DeblockEdgeInfo::default());
        let flat = EdgeSamples {
            p: [100; 7],
            q: [100; 7],
        };
        let masks = filter_mask(&flat, 8, strength, 16, 0);
        assert_eq!(
            masks,
            FilterMasks {
                hev: false,
                filter: true,
                flat: true,
                flat2: true,
            }
        );

        let mut step_samples = flat;
        step_samples.q = [200; 7];
        let masks = filter_mask(&step_samples, 8, strength, 16, 0);
        assert!(!masks.filter, "a large step must not be filtered");

        let mut high_variance = flat;
        high_variance.p[1] = 90;
        assert!(
            filter_mask(&high_variance, 8, strength, 4, 0).hev,
            "|p1 - p0| = 10 exceeds threshBd = 1"
        );
    }

    #[test]
    fn mask_thresholds_scale_with_bit_depth() {
        let strength = FilterStrength {
            lvl: 16,
            limit: 2,
            blimit: 40,
            thresh: 1,
        };
        let mut samples = EdgeSamples {
            p: [100; 7],
            q: [100; 7],
        };
        // 8-bit: a difference of 5 exceeds `limit` (2 << 0).
        samples.p[0] = 95;
        assert!(!filter_mask(&samples, 8, strength, 4, 0).filter);
        // 10-bit: the same difference is within `limit << 2` = 8.
        assert!(filter_mask(&samples, 10, strength, 4, 0).filter);
    }

    #[test]
    fn narrow_filter_matches_an_independently_computed_vector() {
        // p1,p0,q0,q1 = 100,100,140,140; BitDepth 8; hevMask = 0.
        // half = 128 -> ps1 = ps0 = -28, qs0 = qs1 = 12.
        // filter = 3 * (12 - (-28)) = 120; filter1 = 124 >> 3 = 15,
        // filter2 = 123 >> 3 = 15.
        // oq0 = 12 - 15 + 128 = 125; op0 = -28 + 15 + 128 = 115.
        // Round2(15,1) = 8 -> oq1 = 132, op1 = 108.
        let samples = [100u16, 100, 140, 140];
        let mut plane = DeblockPlane::new(&samples, 4, 4, 1, 8);
        narrow_filter(&mut plane, 2, 0, 1, 0, false);
        assert_eq!(plane.samples, [108, 115, 125, 132]);
    }

    #[test]
    fn narrow_filter_hev_only_changes_the_two_inner_samples() {
        let samples = [100u16, 100, 140, 140];
        let mut plane = DeblockPlane::new(&samples, 4, 4, 1, 8);
        narrow_filter(&mut plane, 2, 0, 1, 0, true);
        assert_eq!(plane.samples[0], samples[0]);
        assert_eq!(plane.samples[3], samples[3]);
        assert_ne!(plane.samples[1], samples[1]);
        assert_ne!(plane.samples[2], samples[2]);
    }

    #[test]
    fn wide_filter_has_unity_dc_gain_for_every_tap_set() {
        for (plane_index, log2_size) in [(0, 3), (0, 4), (1, 3)] {
            let mut plane = DeblockPlane::new(&[100u16; 64], 8, 8, 8, 8);
            wide_filter(&mut plane, 4, 4, 1, 0, plane_index, log2_size);
            assert!(
                plane.samples.iter().all(|v| *v == 100),
                "plane={plane_index} log2={log2_size} produced {:?}",
                plane.samples
            );
        }
    }

    #[test]
    fn sample_filtering_dispatches_to_the_right_filter() {
        let strength = params(20).strength(0, 0, &DeblockEdgeInfo::default());
        // filterSize == 4 always takes the narrow filter.
        let samples = [100u16, 100, 110, 110];
        let mut dispatched = DeblockPlane::new(&samples, 4, 4, 1, 8);
        sample_filtering(&mut dispatched, 2, 0, 1, 0, 0, strength, 4);
        let mut direct = DeblockPlane::new(&samples, 4, 4, 1, 8);
        narrow_filter(&mut direct, 2, 0, 1, 0, false);
        assert_eq!(dispatched.samples, direct.samples);

        // A flat region with filterSize == 16 reaches the 16-tap wide filter,
        // whose unity DC gain leaves the constant row untouched.
        let mut flat = DeblockPlane::new(&[100u16; 16], 16, 16, 1, 8);
        sample_filtering(&mut flat, 8, 0, 1, 0, 0, strength, 16);
        assert!(flat.samples.iter().all(|value| *value == 100));

        // The mask rejects a step far beyond `blimit`.
        let big = [0u16, 0, 200, 200];
        let mut rejected = DeblockPlane::new(&big, 4, 4, 1, 8);
        sample_filtering(&mut rejected, 2, 0, 1, 0, 0, strength, 4);
        assert_eq!(rejected.samples, big);
    }

    #[test]
    fn frame_with_zero_level_is_unchanged() {
        let original = step_plane();
        let cell = DeblockCell {
            info: DeblockEdgeInfo::default(),
            apply_filter: true,
            filter_size: 4,
        };
        let grid = DeblockGrid::uniform(4, 4, cell);
        let mut no_filter = params(0);
        no_filter.delta_enabled = true;
        no_filter.ref_deltas[0] = 63;
        let mut planes = [original.clone()];
        loop_filter_frame(&mut planes, &no_filter, &grid, 16, 16, 0, 0);
        assert_eq!(planes[0].samples, original.samples);
    }

    #[test]
    fn frame_walk_smooths_the_vertical_step() {
        let original = step_plane();
        let plane = original.clone();
        let cell = DeblockCell {
            info: DeblockEdgeInfo::default(),
            apply_filter: true,
            filter_size: 4,
        };
        let grid = DeblockGrid::uniform(4, 4, cell);
        let mut planes = [plane.clone()];
        loop_filter_frame(&mut planes, &params(20), &grid, 16, 16, 0, 0);
        let out = &planes[0];
        assert_ne!(out.samples, original.samples);
        // The step at x = 8 is reduced; the flat interior of each half stays.
        assert_eq!(out.get(0, 0), 100);
        assert_eq!(out.get(15, 0), 110);
        assert!(out.get(8, 0) - out.get(7, 0) < 10);
        assert!(out.get(7, 0) > 100);
        assert!(out.get(8, 0) < 110);
    }

    #[test]
    fn horizontal_edges_run_as_the_second_pass() {
        let mut samples = vec![0u16; 256];
        for y in 0..16 {
            for x in 0..16 {
                samples[y * 16 + x] = if y < 8 { 100 } else { 110 };
            }
        }
        let original = DeblockPlane::new(&samples, 16, 16, 16, 8);
        let cell = DeblockCell {
            info: DeblockEdgeInfo::default(),
            apply_filter: true,
            filter_size: 4,
        };
        let grid = DeblockGrid::uniform(4, 4, cell);
        let mut planes = [original.clone()];
        loop_filter_frame(&mut planes, &params(20), &grid, 16, 16, 0, 0);
        let out = &planes[0];
        assert!(
            out.get(0, 7) > 100,
            "pass 1 must filter the horizontal step"
        );
        assert!(out.get(0, 8) < 110);
        assert_ne!(out.samples, original.samples);
    }

    #[test]
    fn chroma_plane_is_gated_by_its_own_level() {
        let cell = DeblockCell {
            info: DeblockEdgeInfo::default(),
            apply_filter: true,
            filter_size: 4,
        };
        let grid = DeblockGrid::uniform(4, 4, cell);
        let chroma = step_plane();
        let mut planes = [step_plane(), chroma.clone()];
        let mut gated = params(20);
        gated.level[2] = 0;
        loop_filter_frame(&mut planes, &gated, &grid, 16, 16, 1, 1);
        assert_eq!(planes[1].samples, chroma.samples, "plane 1 level[2] == 0");
        assert_ne!(planes[0].samples, step_plane().samples);
    }

    #[test]
    fn deblocked_plane_feeds_loop_restoration() {
        // Evidence that the reconstructed/deblocked frame is the
        // `UpscaledCurrFrame` input consumed by the CDEF/LR stage.
        let mut plane = step_plane();
        let cell = DeblockCell {
            info: DeblockEdgeInfo::default(),
            apply_filter: true,
            filter_size: 4,
        };
        let grid = DeblockGrid::uniform(4, 4, cell);
        let mut planes = [plane.clone()];
        loop_filter_frame(&mut planes, &params(20), &grid, 16, 16, 0, 0);
        plane = planes[0].clone();

        let geometry = vec![LrPlaneGeometry::new(
            16,
            16,
            64,
            0,
            0,
            RestorationType::None,
        )];
        let out = apply_loop_restoration(
            &[plane.view()],
            &[plane.view()],
            &geometry,
            &LrFrameParams::for_geometry(&geometry),
            16,
            16,
            8,
        );
        assert_eq!(out[0], plane.samples);
    }
}
