//! AV1 block syntax for intra blocks: skip, y mode and uv mode.
//!
//! `decode_block` starts with the segment id, then `skip` on
//! `TileSkipCdf[above_skip + left_skip]`; a non-skipped intra block reads its
//! luma mode (`intra_frame_y_mode` on key frames, `y_mode` otherwise) and its
//! chroma mode (`uv_mode`, with the CFL alphabet when allowed).

use crate::reference::av1::cdf_defaults::DEFAULT_SKIP_CDF;
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
    if skip {
        return Ok(IntraBlock {
            skip,
            y_mode: None,
            uv_mode: None,
        });
    }
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
    Ok(IntraBlock {
        skip,
        y_mode: Some(y_mode),
        uv_mode,
    })
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
    fn skipped_block_reads_no_modes() {
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
        assert_eq!(block.y_mode, None);
        assert_eq!(block.uv_mode, None);
    }
}
