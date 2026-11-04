//! AV1 block syntax for intra blocks: skip, y mode and uv mode.
//!
//! `decode_block` starts with the segment id, then `skip` on
//! `TileSkipCdf[above_skip + left_skip]`; a non-skipped intra block reads its
//! luma mode (`intra_frame_y_mode` on key frames, `y_mode` otherwise) and its
//! chroma mode (`uv_mode`, with the CFL alphabet when allowed).

use crate::reference::av1::cdf_defaults::DEFAULT_SKIP_CDF;
use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::intra::{
    cfl_allowed, read_intra_frame_y_mode, read_uv_mode, read_y_mode, IntraMode, UvMode,
};
use crate::reference::av1::partition::BlockSize;
use crate::reference::av1::symbol::{SymbolDecoder, SymbolError};

/// The syntax of one decoded intra block (residuals are a later stage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntraBlock {
    pub skip: bool,
    pub y_mode: Option<IntraMode>,
    pub uv_mode: Option<UvMode>,
}

/// `skip: S()` on `TileSkipCdf[ ctx ]`.
pub fn read_skip(
    decoder: &mut SymbolDecoder<'_>,
    above_skip: bool,
    left_skip: bool,
) -> Result<bool, SymbolError> {
    let ctx = usize::from(above_skip) + usize::from(left_skip);
    let start = ctx * 3;
    let mut cdf = DEFAULT_SKIP_CDF.values[start..start + 3].to_vec();
    Ok(decoder.read_symbol(&mut cdf)? != 0)
}

/// Stateful `skip` read using the tile's adaptive `TileSkipCdf` row.
pub fn read_skip_with_cdf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    above_skip: bool,
    left_skip: bool,
    disable_cdf_update: bool,
) -> Result<bool, CdfStateError> {
    let ctx = usize::from(above_skip) + usize::from(left_skip);
    Ok(cdfs.read_symbol(decoder, &DEFAULT_SKIP_CDF, ctx * 3, 3, disable_cdf_update)? != 0)
}

/// Decode one intra block's modes.
#[allow(clippy::too_many_arguments)]
pub fn decode_intra_block(
    decoder: &mut SymbolDecoder<'_>,
    size: BlockSize,
    key_frame: bool,
    above_mode: Option<IntraMode>,
    left_mode: Option<IntraMode>,
    have_chroma: bool,
    lossless: bool,
    above_skip: bool,
    left_skip: bool,
) -> Result<IntraBlock, SymbolError> {
    let skip = read_skip(decoder, above_skip, left_skip)?;
    let (y_mode, uv_mode) = decode_intra_modes(
        decoder,
        size,
        key_frame,
        above_mode,
        left_mode,
        have_chroma,
        lossless,
    )?;
    Ok(IntraBlock {
        skip,
        y_mode: Some(y_mode),
        uv_mode,
    })
}

/// Stateful intra block syntax used by a real tile decoder.
#[allow(clippy::too_many_arguments)]
pub fn decode_intra_block_with_cdf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    size: BlockSize,
    key_frame: bool,
    above_mode: Option<IntraMode>,
    left_mode: Option<IntraMode>,
    have_chroma: bool,
    lossless: bool,
    above_skip: bool,
    left_skip: bool,
    disable_cdf_update: bool,
) -> Result<IntraBlock, CdfStateError> {
    let skip = read_skip_with_cdf(decoder, cdfs, above_skip, left_skip, disable_cdf_update)?;
    let (y_mode, uv_mode) = decode_intra_modes_with_cdf(
        decoder,
        cdfs,
        size,
        key_frame,
        above_mode,
        left_mode,
        have_chroma,
        lossless,
        disable_cdf_update,
    )?;
    Ok(IntraBlock {
        skip,
        y_mode: Some(y_mode),
        uv_mode,
    })
}

/// Decode only `intra_frame_y_mode`/`y_mode` and `uv_mode`.
#[allow(clippy::too_many_arguments)]
pub fn decode_intra_modes(
    decoder: &mut SymbolDecoder<'_>,
    size: BlockSize,
    key_frame: bool,
    above_mode: Option<IntraMode>,
    left_mode: Option<IntraMode>,
    have_chroma: bool,
    lossless: bool,
) -> Result<(IntraMode, Option<UvMode>), SymbolError> {
    let y_mode = if key_frame {
        read_intra_frame_y_mode(decoder, above_mode, left_mode)?
    } else {
        read_y_mode(decoder, size)?
    };
    let uv_mode = if have_chroma {
        Some(read_uv_mode(decoder, y_mode, cfl_allowed(size, lossless))?)
    } else {
        None
    };
    Ok((y_mode, uv_mode))
}

#[allow(clippy::too_many_arguments)]
pub fn decode_intra_modes_with_cdf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    size: BlockSize,
    key_frame: bool,
    above_mode: Option<IntraMode>,
    left_mode: Option<IntraMode>,
    have_chroma: bool,
    lossless: bool,
    disable_cdf_update: bool,
) -> Result<(IntraMode, Option<UvMode>), CdfStateError> {
    let y_mode = if key_frame {
        crate::reference::av1::intra::read_intra_frame_y_mode_with_cdf(
            decoder,
            cdfs,
            above_mode,
            left_mode,
            disable_cdf_update,
        )?
    } else {
        crate::reference::av1::intra::read_y_mode_with_cdf(decoder, cdfs, size, disable_cdf_update)?
    };
    let uv_mode = if have_chroma {
        Some(crate::reference::av1::intra::read_uv_mode_with_cdf(
            decoder,
            cdfs,
            y_mode,
            cfl_allowed(size, lossless),
            disable_cdf_update,
        )?)
    } else {
        None
    };
    Ok((y_mode, uv_mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::partition::BLOCK_SIZES;

    fn block_16() -> BlockSize {
        *BLOCK_SIZES
            .iter()
            .find(|s| s.name == "BLOCK_16X16")
            .unwrap()
    }

    #[test]
    fn skip_context_adds_the_neighbours() {
        for (above, left) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut decoder = SymbolDecoder::init(&[0u8; 16]).unwrap();
            assert_eq!(read_skip(&mut decoder, above, left).unwrap(), false);
        }
        let mut decoder = SymbolDecoder::init(&[0xffu8; 16]).unwrap();
        assert_eq!(read_skip(&mut decoder, false, false).unwrap(), true);
    }

    #[test]
    fn intra_block_decodes_modes_after_skip() {
        let mut decoder = SymbolDecoder::init(&[0u8; 32]).unwrap();
        let block = decode_intra_block(
            &mut decoder,
            block_16(),
            true,
            None,
            None,
            true,
            false,
            false,
            false,
        )
        .unwrap();
        assert!(!block.skip);
        assert_eq!(block.y_mode, Some(IntraMode::Dc));
        assert_eq!(block.uv_mode, Some(UvMode::Dc));
    }

    #[test]
    fn skipped_intra_block_still_reads_modes() {
        let mut decoder = SymbolDecoder::init(&[0xffu8; 32]).unwrap();
        let block = decode_intra_block(
            &mut decoder,
            block_16(),
            false,
            None,
            None,
            true,
            false,
            false,
            false,
        )
        .unwrap();
        assert!(block.skip);
        assert!(block.y_mode.is_some());
        assert!(block.uv_mode.is_some());
    }

    #[test]
    fn stateful_intra_block_updates_all_selected_cdf_rows() {
        use crate::reference::av1::cdf_defaults::{
            DEFAULT_INTRA_FRAME_Y_MODE_CDF, DEFAULT_SKIP_CDF, DEFAULT_UV_MODE_CFL_ALLOWED_CDF,
        };

        let mut cdfs = CdfState::new();
        let mut decoder = SymbolDecoder::init(&[0u8; 32]).unwrap();
        let block = decode_intra_block_with_cdf(
            &mut decoder,
            &mut cdfs,
            block_16(),
            true,
            None,
            None,
            true,
            false,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(block.y_mode, Some(IntraMode::Dc));
        assert_eq!(block.uv_mode, Some(UvMode::Dc));
        assert_eq!(cdfs.row_mut(&DEFAULT_SKIP_CDF, 0, 3).unwrap()[2], 1);
        assert_eq!(
            cdfs.row_mut(&DEFAULT_INTRA_FRAME_Y_MODE_CDF, 0, 14)
                .unwrap()[13],
            1
        );
        assert_eq!(
            cdfs.row_mut(&DEFAULT_UV_MODE_CFL_ALLOWED_CDF, 0, 15)
                .unwrap()[14],
            1
        );
    }
}
