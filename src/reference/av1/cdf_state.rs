//! Mutable AV1 tile CDF state.
//!
//! The generated `cdf_defaults` tables are immutable provenance. A real tile
//! copies them once, then successive `read_symbol()` calls adapt the selected
//! rows until the tile ends or a frame context is restored.

use crate::reference::av1::cdf_defaults::{
    CdfTable, DEFAULT_MV_BIT_CDF, DEFAULT_MV_CLASS0_BIT_CDF, DEFAULT_MV_CLASS0_FR_CDF,
    DEFAULT_MV_CLASS0_HP_CDF, DEFAULT_MV_CLASS_CDF, DEFAULT_MV_FR_CDF, DEFAULT_MV_HP_CDF,
    DEFAULT_MV_JOINT_CDF, DEFAULT_MV_SIGN_CDF, TABLES,
};
use crate::reference::av1::symbol::{SymbolDecoder, SymbolError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdfStateError {
    UnknownTable,
    BadRow {
        table: &'static str,
        start: usize,
        width: usize,
        len: usize,
    },
    Symbol(SymbolError),
    SymbolInRow {
        table: &'static str,
        start: usize,
        width: usize,
        error: SymbolError,
    },
}

impl From<SymbolError> for CdfStateError {
    fn from(error: SymbolError) -> Self {
        Self::Symbol(error)
    }
}

impl core::fmt::Display for CdfStateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

fn copy_row<const N: usize>(table: &CdfTable, row: usize) -> [u16; N] {
    table.values[row * N..(row + 1) * N].try_into().unwrap()
}

/// The two-component MV CDFs. These arrays are not suffix-indexed by qcontext,
/// but each component and each MV context still has an independent adaptation
/// counter and saved value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MvCdfs {
    pub(crate) joint: [u16; 5],
    pub(crate) class: [[u16; 12]; 2],
    pub(crate) class0_bit: [[u16; 3]; 2],
    pub(crate) class0_fr: [[[u16; 5]; 2]; 2],
    pub(crate) class0_hp: [[u16; 3]; 2],
    pub(crate) sign: [[u16; 3]; 2],
    pub(crate) bits: [[[u16; 3]; 10]; 2],
    pub(crate) fr: [[u16; 5]; 2],
    pub(crate) hp: [[u16; 3]; 2],
}

impl Default for MvCdfs {
    fn default() -> Self {
        let mut bits = [[[0u16; 3]; 10]; 2];
        for component in &mut bits {
            for index in 0..10 {
                component[index] = copy_row(&DEFAULT_MV_BIT_CDF, index);
            }
        }
        Self {
            joint: copy_row(&DEFAULT_MV_JOINT_CDF, 0),
            class: [
                copy_row(&DEFAULT_MV_CLASS_CDF, 0),
                copy_row(&DEFAULT_MV_CLASS_CDF, 1),
            ],
            class0_bit: [
                copy_row(&DEFAULT_MV_CLASS0_BIT_CDF, 0),
                copy_row(&DEFAULT_MV_CLASS0_BIT_CDF, 0),
            ],
            class0_fr: [
                [
                    copy_row(&DEFAULT_MV_CLASS0_FR_CDF, 0),
                    copy_row(&DEFAULT_MV_CLASS0_FR_CDF, 1),
                ],
                [
                    copy_row(&DEFAULT_MV_CLASS0_FR_CDF, 2),
                    copy_row(&DEFAULT_MV_CLASS0_FR_CDF, 3),
                ],
            ],
            class0_hp: [
                copy_row(&DEFAULT_MV_CLASS0_HP_CDF, 0),
                copy_row(&DEFAULT_MV_CLASS0_HP_CDF, 0),
            ],
            sign: [
                copy_row(&DEFAULT_MV_SIGN_CDF, 0),
                copy_row(&DEFAULT_MV_SIGN_CDF, 0),
            ],
            bits,
            fr: [
                copy_row(&DEFAULT_MV_FR_CDF, 0),
                copy_row(&DEFAULT_MV_FR_CDF, 1),
            ],
            hp: [
                copy_row(&DEFAULT_MV_HP_CDF, 0),
                copy_row(&DEFAULT_MV_HP_CDF, 0),
            ],
        }
    }
}

impl MvCdfs {
    fn reset_symbol_counters(&mut self) {
        self.joint[4] = 0;
        for row in &mut self.class {
            row[11] = 0;
        }
        for row in &mut self.class0_bit {
            row[2] = 0;
        }
        for component in &mut self.class0_fr {
            for row in component {
                row[4] = 0;
            }
        }
        for row in &mut self.class0_hp {
            row[2] = 0;
        }
        for row in &mut self.sign {
            row[2] = 0;
        }
        for component in &mut self.bits {
            for row in component {
                row[2] = 0;
            }
        }
        for row in &mut self.fr {
            row[4] = 0;
        }
        for row in &mut self.hp {
            row[2] = 0;
        }
    }
}

/// A tile's complete adaptive CDF set. Tables stay in `cdf_defaults::TABLES`
/// order and `read_symbol` updates the copied row in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdfState {
    tables: Vec<Vec<u16>>,
    pub mv: MvCdfs,
}

impl Default for CdfState {
    fn default() -> Self {
        Self::new()
    }
}

impl CdfState {
    pub fn new() -> Self {
        Self {
            tables: TABLES.iter().map(|table| table.values.to_vec()).collect(),
            mv: MvCdfs::default(),
        }
    }

    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    pub fn value_count(&self) -> usize {
        self.tables.iter().map(Vec::len).sum()
    }

    fn table_index(table: &CdfTable) -> Result<usize, CdfStateError> {
        TABLES
            .iter()
            .position(|entry| std::ptr::eq(*entry, table))
            .ok_or(CdfStateError::UnknownTable)
    }

    /// Mutable row without reading a symbol. `start` is the row's first value
    /// in the flat generated table.
    pub fn row_mut(
        &mut self,
        table: &CdfTable,
        start: usize,
        width: usize,
    ) -> Result<&mut [u16], CdfStateError> {
        let index = Self::table_index(table)?;
        let values = &mut self.tables[index];
        let end = start
            .checked_add(width)
            .filter(|end| *end <= values.len())
            .ok_or(CdfStateError::BadRow {
                table: table.name,
                start,
                width,
                len: values.len(),
            })?;
        Ok(&mut values[start..end])
    }

    /// Decode one symbol from a persistent row.
    pub fn read_symbol(
        &mut self,
        decoder: &mut SymbolDecoder<'_>,
        table: &CdfTable,
        start: usize,
        width: usize,
        disable_cdf_update: bool,
    ) -> Result<usize, CdfStateError> {
        let row = self.row_mut(table, start, width)?;
        decoder
            .read_symbol_with(row, disable_cdf_update)
            .map_err(|error| CdfStateError::SymbolInRow {
                table: table.name,
                start,
                width,
                error,
            })
    }

    /// Re-copy one table from its specification defaults.
    pub fn reset_table(&mut self, table: &CdfTable) -> Result<(), CdfStateError> {
        let index = Self::table_index(table)?;
        self.tables[index].copy_from_slice(table.values);
        Ok(())
    }

    /// Reset the adaptation counters stored after each row's terminal
    /// probability. AV1 does this when it saves a tile context as the
    /// reference frame's backward CDF context.
    pub fn reset_symbol_counters(&mut self) {
        for table in &mut self.tables {
            for index in 1..table.len() {
                if table[index - 1] == (1u16 << 15) {
                    table[index] = 0;
                }
            }
        }
        self.mv.reset_symbol_counters();
    }

    /// AV1 keeps one active coefficient-CDF q-context per frame. When a tile
    /// context is saved for `load_cdfs()`, copy that active group into all four
    /// q-context slots so the next frame can use its own q index.
    pub fn normalize_coeff_q_context(&mut self, q_context: usize) {
        for (table, meta) in self.tables.iter_mut().zip(TABLES.iter()) {
            let is_coeff_table = meta.name.starts_with("Default_Txb_Skip_Cdf")
                || meta.name.starts_with("Default_Eob_Pt_")
                || meta.name.starts_with("Default_Eob_Extra_Cdf")
                || meta.name.starts_with("Default_Dc_Sign_Cdf")
                || meta.name.starts_with("Default_Coeff_Base_Eob_Cdf")
                || meta.name.starts_with("Default_Coeff_Base_Cdf")
                || meta.name.starts_with("Default_Coeff_Br_Cdf");
            if !is_coeff_table {
                continue;
            }
            let group = table.len() / 4;
            let source = q_context.min(3) * group..(q_context.min(3) + 1) * group;
            let row = table[source].to_vec();
            for q in 0..4 {
                table[q * group..(q + 1) * group].copy_from_slice(&row);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::cdf_defaults::DEFAULT_SKIP_CDF;

    #[test]
    fn state_copies_every_generated_table() {
        let state = CdfState::new();
        assert_eq!(state.table_count(), 96);
        assert_eq!(state.value_count(), 19_677);
        for (actual, default) in state.tables.iter().zip(TABLES.iter()) {
            assert_eq!(actual.as_slice(), default.values, "{}", default.name);
        }
    }

    #[test]
    fn symbol_reads_persist_and_adapt_the_selected_row() {
        let mut state = CdfState::new();
        let before = state.row_mut(&DEFAULT_SKIP_CDF, 0, 3).unwrap().to_vec();
        let mut decoder = SymbolDecoder::init(&[0u8; 32]).unwrap();
        let symbol = state
            .read_symbol(&mut decoder, &DEFAULT_SKIP_CDF, 0, 3, false)
            .unwrap();
        assert_eq!(symbol, 0);
        let after = state.row_mut(&DEFAULT_SKIP_CDF, 0, 3).unwrap();
        assert_ne!(after, before);
        assert_eq!(after[2], 1);
        // Another default row is untouched.
        assert_eq!(
            state.row_mut(&DEFAULT_SKIP_CDF, 3, 3).unwrap(),
            &DEFAULT_SKIP_CDF.values[3..6]
        );
    }

    #[test]
    fn disabled_update_reads_without_changing_the_row() {
        let mut state = CdfState::new();
        let before = state.row_mut(&DEFAULT_SKIP_CDF, 0, 3).unwrap().to_vec();
        let mut decoder = SymbolDecoder::init(&[0xffu8; 16]).unwrap();
        assert_eq!(
            state
                .read_symbol(&mut decoder, &DEFAULT_SKIP_CDF, 0, 3, true)
                .unwrap(),
            1
        );
        assert_eq!(
            state.row_mut(&DEFAULT_SKIP_CDF, 0, 3).unwrap(),
            before.as_slice()
        );
    }

    #[test]
    fn reset_restores_defaults_and_bad_rows_are_rejected() {
        let mut state = CdfState::new();
        state.row_mut(&DEFAULT_SKIP_CDF, 0, 3).unwrap()[0] = 1;
        state.reset_table(&DEFAULT_SKIP_CDF).unwrap();
        assert_eq!(
            state.row_mut(&DEFAULT_SKIP_CDF, 0, 3).unwrap(),
            &DEFAULT_SKIP_CDF.values[..3]
        );
        assert!(matches!(
            state.row_mut(&DEFAULT_SKIP_CDF, usize::MAX, 3),
            Err(CdfStateError::BadRow { .. })
        ));
    }

    #[test]
    fn reset_symbol_counters_zeroes_terminal_row_counters() {
        let mut state = CdfState::new();
        let mut decoder = SymbolDecoder::init(&[0xffu8; 16]).unwrap();
        state
            .read_symbol(&mut decoder, &DEFAULT_SKIP_CDF, 3, 3, false)
            .unwrap();
        assert_eq!(state.row_mut(&DEFAULT_SKIP_CDF, 3, 3).unwrap()[2], 1);
        state.reset_symbol_counters();
        assert_eq!(state.row_mut(&DEFAULT_SKIP_CDF, 3, 3).unwrap()[2], 0);
    }

    #[test]
    fn coefficient_q_context_is_collapsed_when_a_frame_is_saved() {
        use crate::reference::av1::cdf_defaults::DEFAULT_TXB_SKIP_CDF;

        let mut state = CdfState::new();
        let group = DEFAULT_TXB_SKIP_CDF.values.len() / 4;
        state
            .row_mut(&DEFAULT_TXB_SKIP_CDF, 2 * group, group)
            .unwrap()[0] = 1234;
        state.normalize_coeff_q_context(2);
        for q in 0..4 {
            assert_eq!(
                state
                    .row_mut(&DEFAULT_TXB_SKIP_CDF, q * group, group)
                    .unwrap()[0],
                1234
            );
        }
    }
}
