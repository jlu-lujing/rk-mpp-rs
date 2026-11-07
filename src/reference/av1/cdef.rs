//! AV1 constrained directional enhancement filter (CDEF), specification
//! sections 7.15.1 through 7.15.3.
//!
//! The direction search reads the reconstructed luma block. The filter then
//! reads the same unfiltered plane and returns the filtered 8x8 luma block, or
//! the corresponding 4x4 or 8x4 chroma block for 4:2:0 or 4:2:2 input.

use crate::reference::av1::partition::BlockSize;
use crate::reference::av1::symbol::SymbolDecoder;

/// `Cdef_Directions[8][2][2]`.
pub const CDEF_DIRECTIONS: [[[i8; 2]; 2]; 8] = [
    [[-1, 1], [-2, 2]],
    [[0, 1], [-1, 2]],
    [[0, 1], [0, 2]],
    [[0, 1], [1, 2]],
    [[1, 1], [2, 2]],
    [[1, 0], [2, 1]],
    [[1, 0], [2, 0]],
    [[1, 0], [2, -1]],
];

/// `Cdef_Pri_Taps[2][2]`.
pub const CDEF_PRI_TAPS: [[i32; 2]; 2] = [[4, 2], [3, 3]];

/// `Cdef_Sec_Taps[2][2]`.
pub const CDEF_SEC_TAPS: [[i32; 2]; 2] = [[2, 1], [2, 1]];

/// `Cdef_Uv_Dir[2][2][8]`.
pub const CDEF_UV_DIR: [[[u8; 8]; 2]; 2] = [
    [[0, 1, 2, 3, 4, 5, 6, 7], [1, 2, 2, 2, 3, 4, 6, 0]],
    [[7, 0, 2, 4, 5, 6, 6, 6], [0, 1, 2, 3, 4, 5, 6, 7]],
];

/// Tile-wide `cdef_idx` grid in 4x4 luma units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdefIndexGrid {
    pub mi_cols: usize,
    pub mi_rows: usize,
    values: Vec<i16>,
}

impl CdefIndexGrid {
    pub fn new(mi_cols: usize, mi_rows: usize) -> Self {
        Self {
            mi_cols,
            mi_rows,
            values: vec![-1; mi_cols * mi_rows],
        }
    }

    pub fn get(&self, row: usize, col: usize) -> Option<u8> {
        if row >= self.mi_rows || col >= self.mi_cols {
            return None;
        }
        let value = self.values[row * self.mi_cols + col];
        (value >= 0).then_some(value as u8)
    }

    /// `read_cdef()`: read one aligned 64x64 CDEF index for the block, unless
    /// this aligned region already carries an index.
    #[allow(clippy::too_many_arguments)]
    pub fn read(
        &mut self,
        decoder: &mut SymbolDecoder<'_>,
        row: usize,
        col: usize,
        size: BlockSize,
        skip: bool,
        coded_lossless: bool,
        enabled: bool,
        allow_intrabc: bool,
        cdef_bits: u8,
    ) -> Result<Option<u8>, crate::reference::av1::symbol::SymbolError> {
        if skip || coded_lossless || !enabled || allow_intrabc {
            return Ok(None);
        }
        let base_row = row & !15usize;
        let base_col = col & !15usize;
        if let Some(value) = self.get(base_row, base_col) {
            return Ok(Some(value));
        }
        let value = decoder.read_literal(u32::from(cdef_bits))? as u8;
        let w4 = size.num_4x4_wide() as usize;
        let h4 = size.num_4x4_high() as usize;
        for r in (base_row..(base_row + h4).min(self.mi_rows)).step_by(16) {
            for c in (base_col..(base_col + w4).min(self.mi_cols)).step_by(16) {
                self.values[r * self.mi_cols + c] = i16::from(value);
            }
        }
        Ok(Some(value))
    }

    /// Copy the CDEF indices one tile transmitted into the frame-level grid.
    ///
    /// A 64x64 CDEF unit that straddles a tile boundary is transmitted once
    /// per tile: libaom tracks this with `xd->cdef_transmitted`, which is
    /// tile-local and reset at every superblock start (`decodemv.c:54`), and
    /// writes the value into the shared mi-grid entry of the unit's first
    /// block. Tiles are decoded in raster order, so the later tile is the one
    /// whose index survives.
    pub fn absorb_tile(
        &mut self,
        other: &Self,
        mi_row_start: usize,
        mi_row_end: usize,
        mi_col_start: usize,
        mi_col_end: usize,
    ) {
        for row in mi_row_start..mi_row_end.min(self.mi_rows) {
            for col in mi_col_start..mi_col_end.min(self.mi_cols) {
                if let Some(value) = other.get(row, col) {
                    self.values[row * self.mi_cols + col] = i16::from(value);
                }
            }
        }
    }
}

/// `Div_Table[9]`, scaled by 1024.
const DIV_TABLE: [i64; 9] = [0, 840, 420, 280, 210, 168, 140, 120, 105];

/// Output of the CDEF direction process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdefDirection {
    pub dir: u8,
    pub variance: i64,
}

/// Frame extent in 4x4 luma units. CDEF's availability check covers the whole
/// frame, not merely the current tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdefRegion {
    pub mi_cols: usize,
    pub mi_rows: usize,
}

impl CdefRegion {
    pub const fn new(mi_cols: usize, mi_rows: usize) -> Self {
        Self { mi_cols, mi_rows }
    }

    fn contains(self, candidate_r: isize, candidate_c: isize) -> bool {
        candidate_c >= 0
            && candidate_c < self.mi_cols as isize
            && candidate_r >= 0
            && candidate_r < self.mi_rows as isize
    }
}

/// An immutable view of one reconstructed plane. `width` and `height` include
/// the MI-grid padding used by the CDEF process.
#[derive(Debug, Clone, Copy)]
pub struct PlaneView<'a> {
    pub samples: &'a [u16],
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

impl<'a> PlaneView<'a> {
    pub fn new(samples: &'a [u16], stride: usize, width: usize, height: usize) -> Self {
        assert!(stride >= width);
        assert!(samples.len() >= stride * height);
        Self {
            samples,
            stride,
            width,
            height,
        }
    }

    fn get(self, x: isize, y: isize) -> Option<u16> {
        if x < 0 || y < 0 || x >= self.width as isize || y >= self.height as isize {
            return None;
        }
        Some(self.samples[y as usize * self.stride + x as usize])
    }
}

/// Run the direction search for the 8x8 luma block at `(x0, y0)`.
pub fn cdef_direction(luma: PlaneView<'_>, x0: usize, y0: usize, bit_depth: u32) -> CdefDirection {
    assert!((8..=12).contains(&bit_depth));
    assert!(x0 + 8 <= luma.width);
    assert!(y0 + 8 <= luma.height);

    let mut partial = [[0i64; 15]; 8];
    let mut cost = [0i64; 8];
    for i in 0..8 {
        for j in 0..8 {
            let x =
                (i64::from(luma.samples[(y0 + i) * luma.stride + x0 + j]) >> (bit_depth - 8)) - 128;
            partial[0][i + j] += x;
            partial[1][i + j / 2] += x;
            partial[2][i] += x;
            partial[3][3 + i - j / 2] += x;
            partial[4][7 + i - j] += x;
            partial[5][3 - i / 2 + j] += x;
            partial[6][j] += x;
            partial[7][i / 2 + j] += x;
        }
    }

    for i in 0..8 {
        cost[2] += partial[2][i] * partial[2][i];
        cost[6] += partial[6][i] * partial[6][i];
    }
    cost[2] *= DIV_TABLE[8];
    cost[6] *= DIV_TABLE[8];

    for i in 0..7 {
        cost[0] += (partial[0][i] * partial[0][i] + partial[0][14 - i] * partial[0][14 - i])
            * DIV_TABLE[i + 1];
        cost[4] += (partial[4][i] * partial[4][i] + partial[4][14 - i] * partial[4][14 - i])
            * DIV_TABLE[i + 1];
    }
    cost[0] += partial[0][7] * partial[0][7] * DIV_TABLE[8];
    cost[4] += partial[4][7] * partial[4][7] * DIV_TABLE[8];

    for i in (1..8).step_by(2) {
        for j in 0..=4 {
            cost[i] += partial[i][3 + j] * partial[i][3 + j];
        }
        cost[i] *= DIV_TABLE[8];
        for j in 0..3 {
            cost[i] += (partial[i][j] * partial[i][j] + partial[i][10 - j] * partial[i][10 - j])
                * DIV_TABLE[2 * j + 2];
        }
    }

    let mut best_cost = 0;
    let mut dir = 0;
    for (i, value) in cost.iter().enumerate() {
        if *value > best_cost {
            best_cost = *value;
            dir = i;
        }
    }
    CdefDirection {
        dir: dir as u8,
        variance: (best_cost - cost[(dir + 4) & 7]) >> 10,
    }
}

/// Apply the specification's variance-dependent adjustment to the luma primary
/// strength.
pub fn cdef_luma_primary_strength(primary: i32, variance: i64) -> i32 {
    if variance == 0 {
        return 0;
    }
    let scaled = variance >> 6;
    let var_str = if scaled == 0 {
        0
    } else {
        floor_log2(scaled as u64).min(12) as i32
    };
    (primary * (4 + var_str) + 8) >> 4
}

fn floor_log2(value: u64) -> u32 {
    63 - value.leading_zeros()
}

/// `constrain(diff, threshold, damping)`.
fn constrain(diff: i32, threshold: i32, damping: i32) -> i32 {
    if threshold == 0 {
        return 0;
    }
    let damping_adj = (damping - floor_log2(threshold as u64) as i32).max(0);
    let sign = if diff < 0 { -1 } else { 1 };
    let magnitude = diff.unsigned_abs() as i32;
    let constrained = (threshold - (magnitude >> damping_adj)).clamp(0, magnitude);
    sign * constrained
}

#[allow(clippy::too_many_arguments)]
fn cdef_get_at(
    input: PlaneView<'_>,
    region: CdefRegion,
    x0: usize,
    y0: usize,
    i: usize,
    j: usize,
    dir: u8,
    k: usize,
    sign: isize,
    sub_x: u32,
    sub_y: u32,
) -> Option<u16> {
    let offset = CDEF_DIRECTIONS[dir as usize][k];
    let y = y0 as isize + i as isize + sign * isize::from(offset[0]);
    let x = x0 as isize + j as isize + sign * isize::from(offset[1]);
    let candidate_r = (y << sub_y) >> 2;
    let candidate_c = (x << sub_x) >> 2;
    if !region.contains(candidate_r, candidate_c) {
        return None;
    }
    input.get(x, y)
}

/// Run the CDEF filter process for one 8x8 luma block or one chroma block at
/// the same MI position. The returned samples are in row-major order.
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block(
    input: PlaneView<'_>,
    region: CdefRegion,
    r: usize,
    c: usize,
    sub_x: u32,
    sub_y: u32,
    primary_strength: i32,
    secondary_strength: i32,
    damping: i32,
    dir: u8,
    bit_depth: u32,
) -> Vec<u16> {
    assert!(dir < 8);
    let x0 = (c * 4) >> sub_x;
    let y0 = (r * 4) >> sub_y;
    let width = 8 >> sub_x;
    let height = 8 >> sub_y;
    assert!(x0 + width <= input.width);
    assert!(y0 + height <= input.height);
    assert!((8..=12).contains(&bit_depth));

    let coeff_shift = bit_depth - 8;
    let clipping_required = primary_strength != 0 && secondary_strength != 0;
    let mut out = vec![0u16; width * height];
    for i in 0..height {
        for j in 0..width {
            let x = input.samples[(y0 + i) * input.stride + x0 + j];
            let mut sum = 0i32;
            let mut max = x;
            let mut min = x;
            for k in 0..2 {
                for sign in [-1isize, 1] {
                    if let Some(p) =
                        cdef_get_at(input, region, x0, y0, i, j, dir, k, sign, sub_x, sub_y)
                    {
                        sum += CDEF_PRI_TAPS[((primary_strength >> coeff_shift) & 1) as usize][k]
                            * constrain(i32::from(p) - i32::from(x), primary_strength, damping);
                        max = max.max(p);
                        min = min.min(p);
                    }
                    for dir_off in [-2i32, 2] {
                        let secondary_dir = ((i32::from(dir) + dir_off) & 7) as u8;
                        if let Some(s) = cdef_get_at(
                            input,
                            region,
                            x0,
                            y0,
                            i,
                            j,
                            secondary_dir,
                            k,
                            sign,
                            sub_x,
                            sub_y,
                        ) {
                            sum += CDEF_SEC_TAPS[((primary_strength >> coeff_shift) & 1) as usize]
                                [k]
                                * constrain(
                                    i32::from(s) - i32::from(x),
                                    secondary_strength,
                                    damping,
                                );
                            max = max.max(s);
                            min = min.min(s);
                        }
                    }
                }
            }
            let rounded = i32::from(x) + ((8 + sum - i32::from(sum < 0)) >> 4);
            out[i * width + j] = if clipping_required {
                rounded.clamp(i32::from(min), i32::from(max)) as u16
            } else {
                rounded as u16
            };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(samples: &[u16], width: usize, height: usize) -> PlaneView<'_> {
        PlaneView::new(samples, width, width, height)
    }

    #[test]
    fn direction_tables_match_the_specification() {
        assert_eq!(CDEF_DIRECTIONS[0], [[-1, 1], [-2, 2]]);
        assert_eq!(CDEF_DIRECTIONS[7], [[1, 0], [2, -1]]);
        assert_eq!(CDEF_PRI_TAPS, [[4, 2], [3, 3]]);
        assert_eq!(CDEF_SEC_TAPS, [[2, 1], [2, 1]]);
        assert_eq!(CDEF_UV_DIR[0][1], [1, 2, 2, 2, 3, 4, 6, 0]);
        assert_eq!(CDEF_UV_DIR[1][1], [0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn direction_finds_horizontal_and_vertical_edges() {
        // A vertical gradient is constant along each row, so the row-wise
        // projection has the strongest directional cost.
        let mut vertical_gradient = [0u16; 64];
        for i in 0..8 {
            for j in 0..8 {
                vertical_gradient[i * 8 + j] = 32 + (i as u16 * 16);
            }
        }
        let found = cdef_direction(plane(&vertical_gradient, 8, 8), 0, 0, 8);
        assert_eq!(found.dir, 2);
        assert_eq!(found.variance, 70_560);

        let mut horizontal_gradient = [0u16; 64];
        for i in 0..8 {
            for j in 0..8 {
                horizontal_gradient[i * 8 + j] = 32 + (j as u16 * 16);
            }
        }
        let found = cdef_direction(plane(&horizontal_gradient, 8, 8), 0, 0, 8);
        assert_eq!(found.dir, 6);
        assert_eq!(found.variance, 70_560);
    }

    #[test]
    fn direction_of_a_flat_block_has_zero_variance() {
        let flat = [128u16; 64];
        let found = cdef_direction(plane(&flat, 8, 8), 0, 0, 8);
        assert_eq!(found.variance, 0);
    }

    #[test]
    fn direction_reads_ten_bit_samples_at_eight_bit_precision() {
        let flat = [512u16; 64];
        assert_eq!(
            cdef_direction(plane(&flat, 8, 8), 0, 0, 10),
            CdefDirection {
                dir: 0,
                variance: 0
            }
        );
    }

    #[test]
    fn luma_strength_is_scaled_by_variance() {
        assert_eq!(cdef_luma_primary_strength(12, 0), 0);
        // (12 * (4 + floor_log2(1)) + 8) >> 4 = 3.
        assert_eq!(cdef_luma_primary_strength(12, 64), 3);
        // Variance is capped at a log2 of 12.
        assert_eq!(cdef_luma_primary_strength(12, i64::MAX), 12);
    }

    #[test]
    fn constrain_clamps_an_overdamped_difference_to_zero() {
        assert_eq!(constrain(100, 4, 4), 0);
        assert_eq!(constrain(-100, 4, 4), 0);
        assert_eq!(constrain(7, 4, 3), 1);
        assert_eq!(constrain(-7, 4, 3), -1);
    }

    #[test]
    fn zero_strength_or_flat_block_is_unchanged() {
        let flat = [100u16; 64];
        let region = CdefRegion::new(2, 2);
        assert_eq!(
            cdef_filter_block(plane(&flat, 8, 8), region, 0, 0, 0, 0, 0, 0, 7, 0, 8),
            flat
        );
    }

    #[test]
    fn filter_uses_the_primary_direction_and_constrains_the_edge() {
        // A vertical step in a direction-2 (horizontal) block: opposite taps on
        // each side are constrained, and adjacent taps are averaged.
        let mut samples = [80u16; 64];
        for i in 0..8 {
            for j in 4..8 {
                samples[i * 8 + j] = 160;
            }
        }
        let out = cdef_filter_block(
            plane(&samples, 8, 8),
            CdefRegion::new(2, 2),
            0,
            0,
            0,
            0,
            12,
            0,
            7,
            2,
            8,
        );
        let row = [80, 80, 81, 83, 157, 159, 160, 160];
        assert_eq!(out, row.repeat(8));
    }

    #[test]
    fn ten_bit_strength_uses_the_scaled_tap_row_and_sample_range() {
        let mut samples = [320u16; 64];
        for i in 0..8 {
            for j in 4..8 {
                samples[i * 8 + j] = 640;
            }
        }
        let out = cdef_filter_block(
            plane(&samples, 8, 8),
            CdefRegion::new(2, 2),
            0,
            0,
            0,
            0,
            48,
            0,
            9,
            2,
            10,
        );
        let row = [320, 320, 324, 331, 629, 636, 640, 640];
        assert_eq!(out, row.repeat(8));
    }

    #[test]
    fn chroma_sub_blocks_apply_the_uv_direction() {
        let flat = [128u16; 16];
        let region = CdefRegion::new(2, 2);
        let out = cdef_filter_block(plane(&flat, 4, 4), region, 0, 0, 1, 1, 0, 0, 6, 0, 8);
        assert_eq!(out, flat);
        assert_eq!(CDEF_UV_DIR[0][0][6], 6);
        assert_eq!(CDEF_UV_DIR[0][1][6], 6);
    }

    #[test]
    fn cdef_index_is_aligned_and_reused() {
        use crate::reference::av1::partition::BLOCK_SIZES;
        let mut grid = CdefIndexGrid::new(32, 32);
        let mut decoder = SymbolDecoder::init(&[0xffu8; 8]).unwrap();
        assert_eq!(
            grid.read(
                &mut decoder,
                17,
                18,
                BLOCK_SIZES[6],
                false,
                false,
                true,
                false,
                2,
            )
            .unwrap(),
            Some(3)
        );
        assert_eq!(grid.get(16, 16), Some(3));
        // The existing aligned value does not consume another literal.
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        assert_eq!(
            grid.read(
                &mut decoder,
                20,
                20,
                BLOCK_SIZES[0],
                false,
                false,
                true,
                false,
                2,
            )
            .unwrap(),
            Some(3)
        );
    }

    #[test]
    fn cdef_index_is_skipped_for_skip_and_lossless_blocks() {
        let mut grid = CdefIndexGrid::new(8, 8);
        let mut decoder = SymbolDecoder::init(&[0xffu8; 8]).unwrap();
        assert_eq!(
            grid.read(
                &mut decoder,
                0,
                0,
                crate::reference::av1::partition::BLOCK_SIZES[0],
                true,
                false,
                true,
                false,
                2,
            )
            .unwrap(),
            None
        );
        assert_eq!(grid.get(0, 0), None);
    }
}
