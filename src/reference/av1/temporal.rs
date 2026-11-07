//! Reference-frame motion fields used by temporal MV prediction.

use crate::reference::av1::inter::MotionVector;

pub(crate) const MAX_FRAME_DISTANCE: i32 = 31;
pub(crate) const REFMVS_LIMIT: i32 = (1 << 12) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameMv {
    pub ref_frame: i8,
    pub mv: MotionVector,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameMotionField {
    pub mi_rows: usize,
    pub mi_cols: usize,
    pub ref_order_hints: [u8; 7],
    pub cells: Vec<Option<FrameMv>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TemporalMv {
    pub mv: MotionVector,
    pub ref_frame_offset: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalMotionField {
    pub(crate) mi_rows: usize,
    pub(crate) mi_cols: usize,
    pub(crate) mvs_rows: usize,
    pub(crate) mvs_cols: usize,
    pub(crate) current_order_hint: u8,
    pub(crate) enable_order_hint: bool,
    pub(crate) order_hint_bits: u8,
    pub(crate) ref_order_hints: [u8; 7],
    pub(crate) cells: Vec<Option<TemporalMv>>,
}

impl TemporalMotionField {
    pub(crate) fn candidate(&self, row: usize, col: usize) -> Option<TemporalMv> {
        self.cells
            .get(row.checked_mul(self.mvs_cols)?.checked_add(col)?)
            .copied()
            .flatten()
    }
}

pub(crate) fn project_mv(mv: MotionVector, numerator: i32, denominator: i32) -> MotionVector {
    const DIV_MULT: [i32; 32] = [
        0, 16384, 8192, 5461, 4096, 3276, 2730, 2340, 2048, 1820, 1638, 1489, 1365, 1260, 1170,
        1092, 1024, 963, 910, 862, 819, 780, 744, 712, 682, 655, 630, 606, 585, 564, 546, 528,
    ];
    let denominator = denominator.min(MAX_FRAME_DISTANCE).max(1) as usize;
    let numerator = numerator.clamp(-MAX_FRAME_DISTANCE, MAX_FRAME_DISTANCE);
    let scale = |value: i32| {
        let scaled = i64::from(value) * i64::from(numerator) * i64::from(DIV_MULT[denominator]);
        let rounded = if scaled < 0 {
            -((scaled.unsigned_abs() + (1u64 << 13)) as i64 >> 14)
        } else {
            (scaled + (1i64 << 13)) >> 14
        };
        rounded.clamp(-((1 << 14) - 1), (1 << 14) - 1) as i32
    };
    MotionVector::new(scale(mv.row), scale(mv.col))
}

pub(crate) fn block_position(
    block_row: usize,
    block_col: usize,
    mv: MotionVector,
    sign_bias: i32,
    mi_rows: usize,
    mi_cols: usize,
) -> Option<(usize, usize)> {
    const MAX_OFFSET_WIDTH: isize = 64;
    const MAX_OFFSET_HEIGHT: isize = 0;
    let base_row = (block_row >> 3) << 3;
    let base_col = (block_col >> 3) << 3;
    let row_offset = if mv.row >= 0 {
        mv.row >> 6
    } else {
        -((-mv.row) >> 6)
    } as isize;
    let col_offset = if mv.col >= 0 {
        mv.col >> 6
    } else {
        -((-mv.col) >> 6)
    } as isize;
    let row = if sign_bias == 1 {
        block_row as isize - row_offset
    } else {
        block_row as isize + row_offset
    };
    let col = if sign_bias == 1 {
        block_col as isize - col_offset
    } else {
        block_col as isize + col_offset
    };
    if row < 0 || row >= (mi_rows >> 1) as isize || col < 0 || col >= (mi_cols >> 1) as isize {
        return None;
    }
    if row < base_row as isize - MAX_OFFSET_HEIGHT
        || row >= base_row as isize + 8 + MAX_OFFSET_HEIGHT
        || col < base_col as isize - MAX_OFFSET_WIDTH
        || col >= base_col as isize + 8 + MAX_OFFSET_WIDTH
    {
        return None;
    }
    Some((row as usize, col as usize))
}
