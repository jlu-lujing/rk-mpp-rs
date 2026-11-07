//! AV1 intra/inter segment-id syntax and frame segment maps.

use crate::reference::av1::cdf_defaults::{
    DEFAULT_SEGMENT_ID_CDF, DEFAULT_SEGMENT_ID_PREDICTED_CDF,
};
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

/// A completed frame segment map, copied into reference-frame slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentMap {
    pub mi_cols: usize,
    pub mi_rows: usize,
    ids: Vec<u8>,
}

impl SegmentMap {
    pub fn new(mi_cols: usize, mi_rows: usize) -> Self {
        Self {
            mi_cols,
            mi_rows,
            ids: vec![0; mi_cols * mi_rows],
        }
    }

    pub fn get(&self, row: usize, col: usize) -> Option<usize> {
        if row >= self.mi_rows || col >= self.mi_cols {
            return None;
        }
        Some(usize::from(self.ids[row * self.mi_cols + col]))
    }

    fn block_min(&self, row: usize, col: usize, bw4: usize, bh4: usize) -> Option<usize> {
        if row >= self.mi_rows || col >= self.mi_cols {
            return None;
        }
        let mut id = usize::MAX;
        for r in row..(row + bh4).min(self.mi_rows) {
            for c in col..(col + bw4).min(self.mi_cols) {
                id = id.min(usize::from(self.ids[r * self.mi_cols + c]));
            }
        }
        (id != usize::MAX).then_some(id)
    }
}

/// Per-frame segment-id grid, temporal-prediction flags and `LastActiveSegId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentState {
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub last_active: usize,
    previous: Option<SegmentMap>,
    ids: Vec<u8>,
    predicted: Vec<bool>,
}

impl SegmentState {
    pub fn new(mi_cols: usize, mi_rows: usize, last_active: usize) -> Self {
        Self::with_previous(mi_cols, mi_rows, last_active, None)
    }

    pub fn with_previous(
        mi_cols: usize,
        mi_rows: usize,
        last_active: usize,
        previous: Option<SegmentMap>,
    ) -> Self {
        let previous = previous.filter(|map| map.mi_cols == mi_cols && map.mi_rows == mi_rows);
        Self {
            mi_cols,
            mi_rows,
            last_active,
            previous,
            ids: vec![u8::MAX; mi_cols * mi_rows],
            predicted: vec![false; mi_cols * mi_rows],
        }
    }

    pub fn get(&self, row: usize, col: usize) -> Option<usize> {
        if row >= self.mi_rows || col >= self.mi_cols {
            return None;
        }
        let id = self.ids[row * self.mi_cols + col];
        (id != u8::MAX).then_some(usize::from(id))
    }

    pub fn was_predicted(&self, row: usize, col: usize) -> Option<bool> {
        if row >= self.mi_rows || col >= self.mi_cols {
            return None;
        }
        Some(self.predicted[row * self.mi_cols + col])
    }

    pub fn snapshot(&self) -> SegmentMap {
        let ids = self
            .ids
            .iter()
            .map(|id| if *id == u8::MAX { 0 } else { *id })
            .collect();
        SegmentMap {
            mi_cols: self.mi_cols,
            mi_rows: self.mi_rows,
            ids,
        }
    }

    fn neighbor(&self, row: isize, col: isize) -> Option<usize> {
        if row < 0 || col < 0 {
            return None;
        }
        self.get(row as usize, col as usize)
    }

    fn predicted_neighbor(&self, row: isize, col: isize) -> bool {
        if row < 0 || col < 0 {
            return false;
        }
        self.was_predicted(row as usize, col as usize)
            .unwrap_or(false)
    }

    fn spatial_prediction(&self, row: usize, col: usize) -> (usize, usize) {
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
        let ctx = match (prev_ul, prev_u, prev_l) {
            (None, _, _) => 0,
            (Some(ul), Some(up), Some(left)) if ul == up && ul == left => 2,
            (Some(ul), Some(up), Some(left)) if ul == up || ul == left || up == left => 1,
            _ => 0,
        };
        (pred, ctx)
    }

    fn read_spatial_segment_id(
        &self,
        decoder: &mut SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        row: usize,
        col: usize,
        skip: bool,
        disable_cdf_update: bool,
    ) -> Result<usize, CdfStateError> {
        let (pred, ctx) = self.spatial_prediction(row, col);
        if skip {
            return Ok(pred);
        }
        let symbol = cdfs.read_symbol(
            decoder,
            &DEFAULT_SEGMENT_ID_CDF,
            ctx * 9,
            9,
            disable_cdf_update,
        )?;
        Ok(neg_deinterleave(symbol, pred, self.last_active + 1))
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
        let segment =
            self.read_spatial_segment_id(decoder, cdfs, row, col, skip, disable_cdf_update)?;
        self.fill(row, col, bw4, bh4, segment as u8);
        self.fill_predicted(row, col, bw4, bh4, false);
        Ok(segment)
    }

    /// Inter-frame `read_inter_segment_id()`.
    ///
    /// `preskip` mirrors the two call sites in AV1 and determines whether the
    /// skip/temporal syntax branch is active.
    #[allow(clippy::too_many_arguments)]
    pub fn read_inter_segment_id(
        &mut self,
        decoder: &mut SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        row: usize,
        col: usize,
        bw4: usize,
        bh4: usize,
        preskip: bool,
        skip: bool,
        segmentation_enabled: bool,
        update_map: bool,
        temporal_update: bool,
        segment_id_pre_skip: bool,
        disable_cdf_update: bool,
    ) -> Result<usize, CdfStateError> {
        if !segmentation_enabled {
            self.fill(row, col, bw4, bh4, 0);
            self.fill_predicted(row, col, bw4, bh4, false);
            return Ok(0);
        }
        if !update_map {
            let mut segment_id = usize::MAX;
            for r in row..(row + bh4).min(self.mi_rows) {
                for c in col..(col + bw4).min(self.mi_cols) {
                    let id = self
                        .previous
                        .as_ref()
                        .and_then(|map| map.get(r, c))
                        .unwrap_or(0);
                    self.ids[r * self.mi_cols + c] = id as u8;
                    self.predicted[r * self.mi_cols + c] = false;
                    segment_id = segment_id.min(id);
                }
            }
            return Ok(segment_id);
        }
        if preskip && !segment_id_pre_skip {
            return Ok(0);
        }
        if !preskip && skip {
            let segment_id =
                self.read_spatial_segment_id(decoder, cdfs, row, col, true, disable_cdf_update)?;
            self.fill(row, col, bw4, bh4, segment_id as u8);
            self.fill_predicted(row, col, bw4, bh4, false);
            return Ok(segment_id);
        }

        let (segment_id, predicted) = if temporal_update {
            let ctx = usize::from(self.predicted_neighbor(row as isize - 1, col as isize))
                + usize::from(self.predicted_neighbor(row as isize, col as isize - 1));
            let predicted = cdfs.read_symbol(
                decoder,
                &DEFAULT_SEGMENT_ID_PREDICTED_CDF,
                ctx * 3,
                3,
                disable_cdf_update,
            )? != 0;
            let id = if predicted {
                self.previous
                    .as_ref()
                    .and_then(|map| map.block_min(row, col, bw4, bh4))
                    .unwrap_or(0)
            } else {
                self.read_spatial_segment_id(decoder, cdfs, row, col, skip, disable_cdf_update)?
            };
            (id, predicted)
        } else {
            (
                self.read_spatial_segment_id(decoder, cdfs, row, col, skip, disable_cdf_update)?,
                false,
            )
        };
        self.fill(row, col, bw4, bh4, segment_id as u8);
        self.fill_predicted(row, col, bw4, bh4, predicted);
        Ok(segment_id)
    }

    fn fill(&mut self, row: usize, col: usize, bw4: usize, bh4: usize, id: u8) {
        for r in row..(row + bh4).min(self.mi_rows) {
            for c in col..(col + bw4).min(self.mi_cols) {
                self.ids[r * self.mi_cols + c] = id;
            }
        }
    }

    fn fill_predicted(&mut self, row: usize, col: usize, bw4: usize, bh4: usize, value: bool) {
        for r in row..(row + bh4).min(self.mi_rows) {
            for c in col..(col + bw4).min(self.mi_cols) {
                self.predicted[r * self.mi_cols + c] = value;
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

    #[test]
    fn inter_update_map_false_copies_the_primary_reference_map() {
        let mut previous = SegmentMap::new(4, 2);
        previous.ids.copy_from_slice(&[6, 6, 4, 5, 6, 4, 5, 2]);
        let mut state = SegmentState::with_previous(4, 2, 7, Some(previous));
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        let mut cdfs = CdfState::new();
        let id = state
            .read_inter_segment_id(
                &mut decoder,
                &mut cdfs,
                0,
                0,
                2,
                2,
                false,
                false,
                true,
                false,
                false,
                false,
                false,
            )
            .unwrap();
        assert_eq!(id, 4);
        assert_eq!(state.get(0, 0), Some(6));
        assert_eq!(state.get(0, 1), Some(6));
        assert_eq!(state.get(1, 1), Some(4));
    }

    #[test]
    fn temporal_prediction_copies_the_block_minimum_from_the_reference_map() {
        let mut previous = SegmentMap::new(4, 4);
        for row in 0..2 {
            for col in 0..2 {
                previous.ids[row * 4 + col] = 5;
            }
        }
        previous.ids[1 * 4 + 1] = 2;
        let mut state = SegmentState::with_previous(4, 4, 7, Some(previous));
        let mut decoder = SymbolDecoder::init(&[0x80; 8]).unwrap();
        let mut cdfs = CdfState::new();
        let id = state
            .read_inter_segment_id(
                &mut decoder,
                &mut cdfs,
                0,
                0,
                2,
                2,
                false,
                false,
                true,
                true,
                true,
                false,
                false,
            )
            .unwrap();
        assert_eq!(id, 2);
        assert_eq!(state.get(0, 0), Some(2));
        assert_eq!(state.get(1, 1), Some(2));
        assert!(state.was_predicted(0, 0).unwrap());
    }

    #[test]
    fn temporal_prediction_is_not_signalled_for_a_spatially_skipped_block() {
        let previous = SegmentMap::new(4, 4);
        let mut state = SegmentState::with_previous(4, 4, 7, Some(previous));
        let mut decoder = SymbolDecoder::init(&[0x80; 8]).unwrap();
        let mut cdfs = CdfState::new();
        let id = state
            .read_inter_segment_id(
                &mut decoder,
                &mut cdfs,
                0,
                0,
                2,
                2,
                false,
                true,
                true,
                true,
                true,
                false,
                false,
            )
            .unwrap();
        assert_eq!(id, 0);
        assert!(!state.was_predicted(0, 0).unwrap());
    }
}
