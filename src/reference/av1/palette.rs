//! AV1 palette-mode info for the no-palette path.
//!
//! Palette color maps and index tokens are a separate syntax family. This
//! module consumes the `has_palette_y/uv` decisions and maintains the
//! neighbour palette-size grid; selecting a palette is reported explicitly so
//! the tile driver cannot silently desynchronise.

use crate::reference::av1::cdf_defaults::{
    DEFAULT_PALETTE_UV_MODE_CDF, DEFAULT_PALETTE_Y_MODE_CDF,
};
use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::intra::{IntraMode, UvMode};
use crate::reference::av1::partition::BlockSize;
use crate::reference::av1::symbol::SymbolDecoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteError {
    Cdf(CdfStateError),
    UnsupportedPalette { y: bool, uv: bool },
}

impl From<CdfStateError> for PaletteError {
    fn from(error: CdfStateError) -> Self {
        Self::Cdf(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteState {
    pub mi_cols: usize,
    pub mi_rows: usize,
    y_sizes: Vec<u8>,
    uv_sizes: Vec<u8>,
}

impl PaletteState {
    pub fn new(mi_cols: usize, mi_rows: usize) -> Self {
        Self {
            mi_cols,
            mi_rows,
            y_sizes: vec![0; mi_cols * mi_rows],
            uv_sizes: vec![0; mi_cols * mi_rows],
        }
    }

    pub fn y_size(&self, row: usize, col: usize) -> u8 {
        if row < self.mi_rows && col < self.mi_cols {
            self.y_sizes[row * self.mi_cols + col]
        } else {
            0
        }
    }

    pub fn uv_size(&self, row: usize, col: usize) -> u8 {
        if row < self.mi_rows && col < self.mi_cols {
            self.uv_sizes[row * self.mi_cols + col]
        } else {
            0
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn read_mode_info(
        &mut self,
        decoder: &mut SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        row: usize,
        col: usize,
        size: BlockSize,
        y_mode: IntraMode,
        uv_mode: Option<UvMode>,
        has_chroma: bool,
        allow_screen_content_tools: bool,
        disable_cdf_update: bool,
    ) -> Result<(), PaletteError> {
        if !allow_screen_content_tools
            || size.width < 8
            || size.height < 8
            || size.width > 64
            || size.height > 64
        {
            return Ok(());
        }
        let bsize_ctx = (size.mi_width_log2() + size.mi_height_log2() - 2) as usize;
        let mut has_y = false;
        if y_mode == IntraMode::Dc {
            let above = row > 0 && self.y_size(row - 1, col) > 0;
            let left = col > 0 && self.y_size(row, col - 1) > 0;
            let ctx = usize::from(above) + usize::from(left);
            let start = (bsize_ctx * 3 + ctx) * 3;
            has_y = cdfs.read_symbol(
                decoder,
                &DEFAULT_PALETTE_Y_MODE_CDF,
                start,
                3,
                disable_cdf_update,
            )? != 0;
        }
        let mut has_uv = false;
        if has_chroma && uv_mode == Some(UvMode::Dc) {
            let ctx = usize::from(has_y);
            has_uv = cdfs.read_symbol(
                decoder,
                &DEFAULT_PALETTE_UV_MODE_CDF,
                ctx * 3,
                3,
                disable_cdf_update,
            )? != 0;
        }
        if has_y || has_uv {
            return Err(PaletteError::UnsupportedPalette {
                y: has_y,
                uv: has_uv,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::partition::BLOCK_SIZES;

    #[test]
    fn zero_stream_selects_no_palette() {
        let mut state = PaletteState::new(8, 8);
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        let mut cdfs = CdfState::new();
        assert_eq!(
            state.read_mode_info(
                &mut decoder,
                &mut cdfs,
                0,
                0,
                BLOCK_SIZES[6],
                IntraMode::Dc,
                Some(UvMode::Dc),
                true,
                true,
                false,
            ),
            Ok(())
        );
        assert_eq!(state.y_size(0, 0), 0);
    }

    #[test]
    fn non_dc_or_small_blocks_consume_nothing() {
        let mut state = PaletteState::new(8, 8);
        let mut decoder = SymbolDecoder::init(&[0xffu8; 8]).unwrap();
        let mut cdfs = CdfState::new();
        assert_eq!(
            state.read_mode_info(
                &mut decoder,
                &mut cdfs,
                0,
                0,
                BLOCK_SIZES[6],
                IntraMode::V,
                Some(UvMode::V),
                true,
                true,
                false,
            ),
            Ok(())
        );
    }
}
