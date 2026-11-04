//! AV1 intra segment-id syntax.

use crate::reference::av1::cdf_defaults::DEFAULT_SEGMENT_ID_CDF;
use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::symbol::SymbolDecoder;

/// `neg_deinterleave(diff, ref, max)`.
pub fn neg_deinterleave(diff: usize, reference: usize, max: usize) -> usize {
    if reference == 0 {
        return diff;
    }
    if reference >= max - 1 {
        return max - diff - 1;
    }
    let span = if 2 * reference < max {
        2 * reference
    } else {
        2 * (max - reference - 1)
    };
    if diff <= span {
        if diff & 1 != 0 {
            reference + ((diff + 1) >> 1)
        } else {
            reference - (diff >> 1)
        }
    } else if 2 * reference < max {
        diff
    } else {
        max - (diff + 1)
    }
}

/// Per-frame segment-id grid and `LastActiveSegId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentState {
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub last_active: usize,
    ids: Vec<i8>,
}

impl SegmentState {
    pub fn new(mi_cols: usize, mi_rows: usize, last_active: usize) -> Self {
        Self {
            mi_cols,
            mi_rows,
            last_active,
            ids: vec![-1; mi_cols * mi_rows],
        }
    }

    pub fn get(&self, row: usize, col: usize) -> Option<usize> {
        if row >= self.mi_rows || col >= self.mi_cols {
            return None;
        }
        let id = self.ids[row * self.mi_cols + col];
        (id >= 0).then_some(id as usize)
    }

    fn neighbor(&self, row: isize, col: isize) -> Option<usize> {
        if row < 0 || col < 0 {
            return None;
        }
        self.get(row as usize, col as usize)
    }

    /// `read_segment_id()` followed by `intra_segment_id()`.
    #[allow(clippy::too_many_arguments)]
    pub fn read_intra_segment_id(
        &mut self,
        decoder: &mut SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        row: usize,
        col: usize,
        bw4: usize,
        bh4: usize,
        skip: bool,
        segmentation_enabled: bool,
        disable_cdf_update: bool,
    ) -> Result<usize, CdfStateError> {
        if !segmentation_enabled {
            self.fill(row, col, bw4, bh4, 0);
            return Ok(0);
        }
        let prev_ul = self.neighbor(row as isize - 1, col as isize - 1);
        let prev_u = self.neighbor(row as isize - 1, col as isize);
        let prev_l = self.neighbor(row as isize, col as isize - 1);
        let pred = match (prev_u, prev_l) {
            (None, None) => 0,
            (None, Some(left)) => left,
            (Some(up), None) => up,
            (Some(up), Some(left)) => {
                if prev_ul == Some(up) {
                    up
                } else {
                    left
                }
            }
        };
        let segment = if skip {
            pred
        } else {
            let ctx = match (prev_ul, prev_u, prev_l) {
                (None, _, _) => 0,
                (Some(ul), Some(up), Some(left)) if ul == up && ul == left => 2,
                (Some(ul), Some(up), Some(left)) if ul == up || ul == left || up == left => 1,
                _ => 0,
            };
            let symbol = cdfs.read_symbol(
                decoder,
                &DEFAULT_SEGMENT_ID_CDF,
                ctx * 9,
                9,
                disable_cdf_update,
            )?;
            neg_deinterleave(symbol, pred, self.last_active + 1)
        };
        self.fill(row, col, bw4, bh4, segment as i8);
        Ok(segment)
    }

    fn fill(&mut self, row: usize, col: usize, bw4: usize, bh4: usize, id: i8) {
        for r in row..(row + bh4).min(self.mi_rows) {
            for c in col..(col + bw4).min(self.mi_cols) {
                self.ids[r * self.mi_cols + c] = id;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neg_deinterleave_covers_the_spec_branches() {
        assert_eq!(neg_deinterleave(3, 0, 8), 3);
        assert_eq!(neg_deinterleave(2, 7, 8), 5);
        assert_eq!(neg_deinterleave(1, 2, 8), 3);
        assert_eq!(neg_deinterleave(2, 2, 8), 1);
        assert_eq!(neg_deinterleave(5, 2, 8), 5);
        assert_eq!(neg_deinterleave(1, 6, 8), 7);
        assert_eq!(neg_deinterleave(2, 6, 8), 5);
    }

    #[test]
    fn skipped_blocks_inherit_their_neighbour_segment() {
        let mut state = SegmentState::new(4, 4, 7);
        state.fill(0, 0, 1, 1, 3);
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        let mut cdfs = CdfState::new();
        let id = state
            .read_intra_segment_id(&mut decoder, &mut cdfs, 0, 1, 1, 1, true, true, false)
            .unwrap();
        assert_eq!(id, 3);
        assert_eq!(state.get(0, 1), Some(3));
    }

    #[test]
    fn disabled_segmentation_always_writes_zero() {
        let mut state = SegmentState::new(2, 2, 7);
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        let mut cdfs = CdfState::new();
        assert_eq!(
            state
                .read_intra_segment_id(&mut decoder, &mut cdfs, 0, 0, 1, 1, false, false, false)
                .unwrap(),
            0
        );
        assert_eq!(state.get(0, 0), Some(0));
    }
}
