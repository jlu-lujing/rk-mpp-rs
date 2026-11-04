//! AV1 loop restoration frame/tile driver, specification section 7.17.
//!
//! `restoration.rs` filters one rectangle; this module supplies the geometry
//! around it: the restoration-unit grid, the 64-luma-line stripe order, the 4x4
//! block walk and the write-back into `LrFrame`. `LrFrame` starts as a copy of
//! `UpscaledCdefFrame`, so a frame with `UsesLr == 0` returns that copy without
//! reading `UpscaledCurrFrame` at all.
//!
//! The per-unit filters are injected through [`LrFrameParams`]. The bitstream
//! half (`read_lr` / `read_lr_unit`) is a separate slice; keeping the driver
//! independent of the CDF tables lets the geometry be checked on its own.

use crate::reference::av1::cdef::PlaneView;
use crate::reference::av1::restoration::{loop_restore_block, LrFilter, LrSource, RestorationType};

/// `RESTORATION_TILESIZE_MAX`.
pub const RESTORATION_TILESIZE_MAX: usize = 256;
/// `MI_SIZE`.
pub const MI_SIZE: usize = 4;
/// Stripe height in luma samples.
pub const LR_STRIPE_HEIGHT: usize = 64;
/// Stripes are shifted up by eight luma lines.
pub const LR_STRIPE_OFFSET: usize = 8;

/// `Round2(x, n)`: `(x + (1 << (n-1))) >> n`, with `n == 0` returning `x`.
pub const fn round2(x: usize, n: u32) -> usize {
    if n == 0 {
        x
    } else {
        (x + (1usize << (n - 1))) >> n
    }
}

/// `count_units_in_frame(unitSize, frameSize)`.
///
/// This deliberately rounds to *nearest* rather than up: the specification
/// adds `unitSize >> 1` before dividing, then clamps the result to at least 1.
pub const fn count_units_in_frame(unit_size: usize, frame_size: usize) -> usize {
    let units = (frame_size + (unit_size >> 1)) / unit_size;
    if units > 0 {
        units
    } else {
        1
    }
}

/// `LoopRestorationSize[plane]` from `lr_unit_shift` and `lr_uv_shift`.
pub const fn loop_restoration_size(lr_unit_shift: u32, lr_uv_shift: u32) -> [usize; 3] {
    assert!(lr_unit_shift <= 2);
    let y = RESTORATION_TILESIZE_MAX >> (2 - lr_unit_shift);
    let c = y >> lr_uv_shift;
    [y, c, c]
}

/// Geometry of one plane for the loop restoration process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LrPlaneGeometry {
    pub unit_size: usize,
    pub unit_rows: usize,
    pub unit_cols: usize,
    pub sub_x: u32,
    pub sub_y: u32,
    pub plane_end_x: isize,
    pub plane_end_y: isize,
    pub frame_type: RestorationType,
}

impl LrPlaneGeometry {
    /// Build the geometry the `loop_restore_block` process derives from the
    /// frame header and the plane's subsampling.
    pub fn new(
        frame_height: usize,
        upscaled_width: usize,
        unit_size: usize,
        sub_x: u32,
        sub_y: u32,
        frame_type: RestorationType,
    ) -> Self {
        let width = round2(upscaled_width, sub_x);
        let height = round2(frame_height, sub_y);
        Self {
            unit_size,
            unit_rows: count_units_in_frame(unit_size, height),
            unit_cols: count_units_in_frame(unit_size, width),
            sub_x,
            sub_y,
            plane_end_x: width as isize - 1,
            plane_end_y: height as isize - 1,
            frame_type,
        }
    }
}

/// Everything `loop_restore_block(plane, row, col)` derives for one 4x4 block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LrBlockPosition {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub unit_row: usize,
    pub unit_col: usize,
    pub stripe_num: u32,
    pub stripe_start_y: isize,
    pub stripe_end_y: isize,
}

impl LrPlaneGeometry {
    /// The specification's `loop_restore_block` prelude for `row`/`col` in
    /// 4x4 luma units.
    pub fn block(&self, row: usize, col: usize) -> LrBlockPosition {
        let luma_y = row * MI_SIZE;
        let stripe_num = ((luma_y + LR_STRIPE_OFFSET) / LR_STRIPE_HEIGHT) as u32;
        let x = (col * MI_SIZE) >> self.sub_x;
        let y = (row * MI_SIZE) >> self.sub_y;
        let w = ((MI_SIZE >> self.sub_x) as isize)
            .min(self.plane_end_x - x as isize + 1)
            .max(0) as usize;
        let h = ((MI_SIZE >> self.sub_y) as isize)
            .min(self.plane_end_y - y as isize + 1)
            .max(0) as usize;
        let unit_row = (((row * MI_SIZE + LR_STRIPE_OFFSET) >> self.sub_y) / self.unit_size)
            .min(self.unit_rows - 1);
        let unit_col = (x / self.unit_size).min(self.unit_cols - 1);
        let stripe_start_y = ((-(LR_STRIPE_OFFSET as i32)
            + stripe_num as i32 * LR_STRIPE_HEIGHT as i32)
            >> self.sub_y) as isize;
        let stripe_end_y = stripe_start_y + ((LR_STRIPE_HEIGHT >> self.sub_y) - 1) as isize;
        LrBlockPosition {
            x,
            y,
            w,
            h,
            unit_row,
            unit_col,
            stripe_num,
            stripe_start_y,
            stripe_end_y,
        }
    }
}

/// Per-plane `LrType` / Wiener / SGRPROJ parameters, flattened to
/// `unit_rows * unit_cols` entries. `None` means `RESTORE_NONE` for that unit.
#[derive(Debug, Clone, Default)]
pub struct LrFrameParams {
    pub planes: Vec<Vec<Option<LrFilter>>>,
}

impl LrFrameParams {
    pub fn filter(
        &self,
        plane: usize,
        unit_row: usize,
        unit_col: usize,
        geometry: &LrPlaneGeometry,
    ) -> Option<LrFilter> {
        let grid = self.planes.get(plane)?;
        grid.get(unit_row * geometry.unit_cols + unit_col)
            .copied()
            .flatten()
    }
}

/// Run the section 7.17 walk over every plane and return `LrFrame`.
///
/// `cdef` is `UpscaledCdefFrame` (deringed), `current` is `UpscaledCurrFrame`
/// (deblocked, not CDEFed). Samples inside the current stripe come from
/// `cdef`, samples outside from `current`, as the get-source-sample process
/// requires. Output buffers keep each input plane's stride.
#[allow(clippy::too_many_arguments)]
pub fn apply_loop_restoration(
    cdef: &[PlaneView<'_>],
    current: &[PlaneView<'_>],
    geometry: &[LrPlaneGeometry],
    params: &LrFrameParams,
    frame_height: usize,
    upscaled_width: usize,
    bit_depth: u32,
) -> Vec<Vec<u16>> {
    assert_eq!(cdef.len(), current.len());
    assert_eq!(cdef.len(), geometry.len());

    let mut out: Vec<Vec<u16>> = cdef
        .iter()
        .map(|p| p.samples[..p.stride * p.height].to_vec())
        .collect();
    if !geometry
        .iter()
        .any(|g| g.frame_type != RestorationType::None)
    {
        return out;
    }

    let rows = (frame_height + MI_SIZE - 1) / MI_SIZE;
    let cols = (upscaled_width + MI_SIZE - 1) / MI_SIZE;
    for row in 0..rows {
        for col in 0..cols {
            for plane in 0..cdef.len() {
                let geom = &geometry[plane];
                if geom.frame_type == RestorationType::None {
                    continue;
                }
                let pos = geom.block(row, col);
                if pos.w == 0 || pos.h == 0 {
                    continue;
                }
                let filter = params.filter(plane, pos.unit_row, pos.unit_col, geom);
                let source =
                    LrSource::for_stripe(cdef[plane], current[plane], pos.stripe_num, geom.sub_y);
                let block =
                    loop_restore_block(source, pos.x, pos.y, pos.w, pos.h, filter, bit_depth);
                for r in 0..pos.h {
                    let dst = (pos.y + r) * cdef[plane].stride + pos.x;
                    out[plane][dst..dst + pos.w]
                        .copy_from_slice(&block[r * pos.w..(r + 1) * pos.w]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma_frame(width: usize, height: usize, unit_size: usize) -> LrPlaneGeometry {
        LrPlaneGeometry::new(height, width, unit_size, 0, 0, RestorationType::Wiener)
    }

    #[test]
    fn round2_matches_the_integer_definition() {
        assert_eq!(round2(7, 0), 7);
        assert_eq!(round2(7, 1), 4);
        assert_eq!(round2(7, 2), 2);
        assert_eq!(round2(8, 1), 4);
        assert_eq!(round2(9, 1), 5);
    }

    #[test]
    fn units_round_to_nearest_not_up() {
        assert_eq!(count_units_in_frame(64, 64), 1);
        assert_eq!(count_units_in_frame(64, 65), 1);
        assert_eq!(count_units_in_frame(64, 96), 2);
        assert_eq!(count_units_in_frame(64, 97), 2);
        assert_eq!(count_units_in_frame(64, 160), 3);
        assert_eq!(count_units_in_frame(64, 1), 1);
        assert_eq!(count_units_in_frame(64, 0), 1);
    }

    #[test]
    fn unit_sizes_follow_the_frame_header_shifts() {
        assert_eq!(loop_restoration_size(0, 0), [64, 64, 64]);
        assert_eq!(loop_restoration_size(1, 0), [128, 128, 128]);
        assert_eq!(loop_restoration_size(2, 0), [256, 256, 256]);
        assert_eq!(loop_restoration_size(2, 1), [256, 128, 128]);
    }

    #[test]
    fn first_block_matches_the_specification_prelude() {
        let geom = luma_frame(64, 64, 64);
        assert_eq!(geom.unit_rows, 1);
        assert_eq!(geom.unit_cols, 1);
        let pos = geom.block(0, 0);
        assert_eq!((pos.x, pos.y, pos.w, pos.h), (0, 0, 4, 4));
        assert_eq!((pos.unit_row, pos.unit_col), (0, 0));
        assert_eq!(pos.stripe_num, 0);
        assert_eq!((pos.stripe_start_y, pos.stripe_end_y), (-8, 55));
    }

    #[test]
    fn stripe_changes_at_the_shifted_boundary() {
        let geom = luma_frame(8, 128, 64);
        assert_eq!(geom.unit_rows, 2);
        let before = geom.block(15, 0);
        assert_eq!(before.y, 60);
        assert_eq!(before.stripe_num, 1);
        assert_eq!((before.stripe_start_y, before.stripe_end_y), (56, 119));
        assert_eq!(before.unit_row, 1);
        let at_boundary = geom.block(16, 0);
        assert_eq!(at_boundary.y, 64);
        assert_eq!(at_boundary.stripe_num, 1);
        assert_eq!(at_boundary.stripe_start_y, 56);
    }

    #[test]
    fn chroma_geometry_uses_the_plane_subsampling() {
        // 4:2:0, 64x64 luma -> 32x32 chroma, unit size 64 (lr_uv_shift = 0).
        let geom = LrPlaneGeometry::new(64, 64, 64, 1, 1, RestorationType::Switchable);
        assert_eq!((geom.plane_end_x, geom.plane_end_y), (31, 31));
        assert_eq!((geom.unit_rows, geom.unit_cols), (1, 1));
        let pos = geom.block(0, 0);
        assert_eq!((pos.x, pos.y, pos.w, pos.h), (0, 0, 2, 2));
        assert_eq!((pos.stripe_start_y, pos.stripe_end_y), (-4, 27));
        let last = geom.block(15, 15);
        assert_eq!((last.x, last.y, last.w, last.h), (30, 30, 2, 2));
    }

    fn plane<'a>(samples: &'a [u16], n: usize) -> PlaneView<'a> {
        PlaneView::new(samples, n, n, n)
    }

    #[test]
    fn uses_lr_zero_returns_the_cdef_copy() {
        let cdef: Vec<u16> = (0..64).collect();
        let current = vec![200u16; 64];
        let geom = LrPlaneGeometry::new(8, 8, 64, 0, 0, RestorationType::None);
        let out = apply_loop_restoration(
            &[plane(&cdef, 8)],
            &[plane(&current, 8)],
            &[geom],
            &LrFrameParams::default(),
            8,
            8,
            8,
        );
        assert_eq!(out[0], cdef);
    }

    #[test]
    fn all_none_units_leave_the_cdef_copy() {
        let cdef: Vec<u16> = (0..64).collect();
        let current = vec![200u16; 64];
        let geom = luma_frame(8, 8, 64);
        let params = LrFrameParams {
            planes: vec![vec![None]],
        };
        let out = apply_loop_restoration(
            &[plane(&cdef, 8)],
            &[plane(&current, 8)],
            &[geom],
            &params,
            8,
            8,
            8,
        );
        assert_eq!(out[0], cdef);
    }

    #[test]
    fn wieners_are_written_back_at_every_block_position() {
        let cdef: Vec<u16> = (0..64).map(|v| (v * 7 % 251) as u16).collect();
        let current = vec![200u16; 64];
        let geom = luma_frame(8, 8, 64);
        let filter = LrFilter::Wiener {
            vertical: [3, -7, 15],
            horizontal: [2, 4, -9],
        };
        let params = LrFrameParams {
            planes: vec![vec![Some(filter)]],
        };
        let out = apply_loop_restoration(
            &[plane(&cdef, 8)],
            &[plane(&current, 8)],
            &[geom],
            &params,
            8,
            8,
            8,
        );

        // Independently filter each 4x4 block and place it into a fresh frame.
        let mut expected = cdef.clone();
        for row in 0..2 {
            for col in 0..2 {
                let pos = geom.block(row, col);
                let source =
                    LrSource::for_stripe(plane(&cdef, 8), plane(&current, 8), pos.stripe_num, 0);
                let block = loop_restore_block(source, pos.x, pos.y, pos.w, pos.h, Some(filter), 8);
                for r in 0..pos.h {
                    let dst = (pos.y + r) * 8 + pos.x;
                    expected[dst..dst + pos.w].copy_from_slice(&block[r * pos.w..(r + 1) * pos.w]);
                }
            }
        }
        assert_eq!(out[0], expected);
        assert_ne!(out[0], cdef, "the filter must actually change samples");
    }

    #[test]
    fn three_planes_are_driven_independently() {
        let cdef: Vec<u16> = (0..16).collect();
        let current = vec![9u16; 16];
        let luma = luma_frame(4, 4, 64);
        let chroma = LrPlaneGeometry::new(4, 4, 64, 0, 0, RestorationType::None);
        let params = LrFrameParams {
            planes: vec![vec![None], vec![None], vec![None]],
        };
        let views = vec![plane(&cdef, 4), plane(&cdef, 4), plane(&cdef, 4)];
        let current_views = vec![plane(&current, 4), plane(&current, 4), plane(&current, 4)];
        let out = apply_loop_restoration(
            &views,
            &current_views,
            &[luma, chroma, chroma],
            &params,
            4,
            4,
            8,
        );
        assert_eq!(out.len(), 3);
        for plane_out in &out {
            assert_eq!(plane_out, &cdef);
        }
    }
}
