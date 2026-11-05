//! AV1 tile-level coefficient contexts.
//!
//! A tile keeps, per plane, the `Above`/`Left` level and DC-context arrays that
//! every coefficient block updates after its residual. `dc_sign` picks its CDF
//! context from the DC categories of the above/left neighbours:
//!
//! ```text
//! dcSign = sum(above categories == 1 ? -1 : above == 2 ? +1 : 0)
//!        + sum(left  categories == 1 ? -1 : left  == 2 ? +1 : 0)
//! ctx    = dcSign < 0 ? 1 : dcSign > 0 ? 2 : 0
//! ```
//!
//! and a finished block writes its `culLevel`/`dcCategory` into every 4x4
//! position it covers.

use crate::reference::av1::partition::BlockSize;
use crate::reference::av1::tx::TxSize;

/// Levels and DC categories for the three planes.
#[derive(Debug, Clone)]
pub struct TileContext {
    pub above_level: [Vec<u8>; 3],
    pub left_level: [Vec<u8>; 3],
    pub above_dc: [Vec<u8>; 3],
    pub left_dc: [Vec<u8>; 3],
}

impl TileContext {
    /// Blank contexts for a tile whose picture is `mi_cols` x `mi_rows` 4x4
    /// units.
    pub fn new(mi_cols: usize, mi_rows: usize) -> Self {
        Self {
            above_level: [vec![0; mi_cols], vec![0; mi_cols], vec![0; mi_cols]],
            left_level: [vec![0; mi_rows], vec![0; mi_rows], vec![0; mi_rows]],
            above_dc: [vec![0; mi_cols], vec![0; mi_cols], vec![0; mi_cols]],
            left_dc: [vec![0; mi_rows], vec![0; mi_rows], vec![0; mi_rows]],
        }
    }

    /// `dc_sign`'s context (0..2) for a block at `(x4, y4)` of `w4` x `h4` units.
    pub fn dc_sign_context(
        &self,
        plane: usize,
        x4: usize,
        y4: usize,
        w4: usize,
        h4: usize,
        max_x4: usize,
        max_y4: usize,
    ) -> usize {
        let mut sign = 0i32;
        for k in 0..w4 {
            if x4 + k < max_x4 {
                match self.above_dc[plane][x4 + k] {
                    1 => sign -= 1,
                    2 => sign += 1,
                    _ => {}
                }
            }
        }
        for k in 0..h4 {
            if y4 + k < max_y4 {
                match self.left_dc[plane][y4 + k] {
                    1 => sign -= 1,
                    2 => sign += 1,
                    _ => {}
                }
            }
        }
        if sign < 0 {
            1
        } else if sign > 0 {
            2
        } else {
            0
        }
    }

    /// Record a finished block's `culLevel` and DC category across its extent.
    pub fn update(
        &mut self,
        plane: usize,
        x4: usize,
        y4: usize,
        w4: usize,
        h4: usize,
        cul_level: u8,
        dc_category: u8,
    ) {
        for k in 0..w4 {
            if let Some(slot) = self.above_level[plane].get_mut(x4 + k) {
                *slot = cul_level;
            }
            if let Some(slot) = self.above_dc[plane].get_mut(x4 + k) {
                *slot = dc_category;
            }
        }
        for k in 0..h4 {
            if let Some(slot) = self.left_level[plane].get_mut(y4 + k) {
                *slot = cul_level;
            }
            if let Some(slot) = self.left_dc[plane].get_mut(y4 + k) {
                *slot = dc_category;
            }
        }
    }

    /// A skipped transform block updates both coefficient contexts to zero.
    pub fn clear_block(&mut self, plane: usize, x4: usize, y4: usize, w4: usize, h4: usize) {
        self.update(plane, x4, y4, w4, h4, 0, 0);
    }

    /// `all_zero` context from the current block's above/left levels.
    #[allow(clippy::too_many_arguments)]
    pub fn txb_skip_context(
        &self,
        plane: usize,
        x4: usize,
        y4: usize,
        tx: TxSize,
        residual_size: BlockSize,
        max_x4: usize,
        max_y4: usize,
    ) -> usize {
        let w4 = (tx.width() / 4) as usize;
        let h4 = (tx.height() / 4) as usize;
        if plane == 0 {
            let mut top = 0u8;
            let mut left = 0u8;
            for k in 0..w4 {
                if x4 + k < max_x4 {
                    top = top.max(self.above_level[plane][x4 + k]);
                }
            }
            for k in 0..h4 {
                if y4 + k < max_y4 {
                    left = left.max(self.left_level[plane][y4 + k]);
                }
            }
            if residual_size.width == tx.width() && residual_size.height == tx.height() {
                return 0;
            }
            if top == 0 && left == 0 {
                return 1;
            }
            if top == 0 || left == 0 {
                return 2 + usize::from(top.max(left) > 3);
            }
            if top.max(left) <= 3 {
                4
            } else if top.min(left) <= 3 {
                5
            } else {
                6
            }
        } else {
            let mut above = false;
            let mut left = false;
            for k in 0..w4 {
                if x4 + k < max_x4 {
                    above |= self.above_level[plane][x4 + k] != 0;
                    above |= self.above_dc[plane][x4 + k] != 0;
                }
            }
            for k in 0..h4 {
                if y4 + k < max_y4 {
                    left |= self.left_level[plane][y4 + k] != 0;
                    left |= self.left_dc[plane][y4 + k] != 0;
                }
            }
            let mut ctx = 7 + usize::from(above) + usize::from(left);
            let block_area = u64::from(residual_size.width) * u64::from(residual_size.height);
            let tx_area = u64::from(tx.width()) * u64::from(tx.height());
            if block_area > tx_area {
                ctx += 3;
            }
            ctx
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::partition::BLOCK_SIZES;

    #[test]
    fn dc_sign_context_counts_neighbour_categories() {
        let mut ctx = TileContext::new(8, 8);
        assert_eq!(ctx.dc_sign_context(0, 0, 0, 2, 2, 8, 8), 0);
        // A negative DC above and to the left sums to a negative sign.
        ctx.above_dc[0][0] = 1;
        ctx.left_dc[0][0] = 1;
        assert_eq!(ctx.dc_sign_context(0, 0, 0, 2, 2, 8, 8), 1);
        // One positive leaves the two negatives net negative: ctx 1.
        ctx.left_dc[0][1] = 2;
        assert_eq!(ctx.dc_sign_context(0, 0, 0, 2, 2, 8, 8), 1);
        // Balanced neighbours (two positive, two negative) cancel out.
        ctx.above_dc[0][1] = 2;
        assert_eq!(ctx.dc_sign_context(0, 0, 0, 2, 2, 8, 8), 0);
        // Making the above pair positive tips the sum positive.
        ctx.above_dc[0][0] = 2;
        assert_eq!(ctx.dc_sign_context(0, 0, 0, 2, 2, 8, 8), 2);
    }

    #[test]
    fn clearing_a_skipped_block_drops_stale_level_and_dc_contexts() {
        let mut ctx = TileContext::new(16, 16);
        ctx.update(0, 2, 3, 2, 2, 7, 1);
        ctx.clear_block(0, 2, 3, 2, 2);
        assert_eq!(ctx.above_level[0][2..4], [0, 0]);
        assert_eq!(ctx.left_level[0][3..5], [0, 0]);
        assert_eq!(ctx.above_dc[0][2..4], [0, 0]);
        assert_eq!(ctx.left_dc[0][3..5], [0, 0]);
    }

    #[test]
    fn update_writes_every_covered_position() {
        let mut ctx = TileContext::new(8, 8);
        ctx.update(1, 2, 3, 3, 2, 17, 1);
        assert_eq!(&ctx.above_level[1][2..5], &[17, 17, 17]);
        assert_eq!(&ctx.above_dc[1][2..5], &[1, 1, 1]);
        assert_eq!(&ctx.left_level[1][3..5], &[17, 17]);
        assert_eq!(&ctx.left_dc[1][3..5], &[1, 1]);
        // Positions outside the block stay untouched, and planes are separate.
        assert_eq!(ctx.above_level[1][1], 0);
        assert_eq!(ctx.above_level[0][2], 0);
    }

    #[test]
    fn txb_skip_context_uses_levels_and_residual_area() {
        let mut ctx = TileContext::new(8, 8);
        ctx.above_level[0][0] = 9;
        ctx.left_level[0][0] = 2;
        assert_eq!(
            ctx.txb_skip_context(0, 0, 0, TxSize::Tx4x4, BLOCK_SIZES[0], 8, 8),
            0
        );
        assert_eq!(
            ctx.txb_skip_context(0, 0, 0, TxSize::Tx4x4, BLOCK_SIZES[3], 8, 8),
            5
        );
        ctx.above_level[1][0] = 1;
        ctx.above_dc[1][0] = 0;
        ctx.left_level[1][0] = 0;
        ctx.left_dc[1][0] = 0;
        assert_eq!(
            ctx.txb_skip_context(1, 0, 0, TxSize::Tx4x4, BLOCK_SIZES[3], 8, 8),
            11
        );
    }
}
