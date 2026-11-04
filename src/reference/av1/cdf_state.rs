//! Mutable AV1 tile CDF state.
//!
//! The generated `cdf_defaults` tables are immutable provenance. A real tile
//! copies them once, then successive `read_symbol()` calls adapt the selected
//! rows until the tile ends or a frame context is restored.

use crate::reference::av1::cdf_defaults::{CdfTable, TABLES};
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

/// A tile's complete adaptive CDF set. Tables stay in `cdf_defaults::TABLES`
/// order and `read_symbol` updates the copied row in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdfState {
    tables: Vec<Vec<u16>>,
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
}
