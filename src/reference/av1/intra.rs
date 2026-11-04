//! AV1 intra mode syntax: the `intra_frame_y_mode` and `y_mode` symbols.
//!
//! `Intra_Mode_Context` is the specification's thirteen-entry table
//! (`{0,1,2,3,4,4,4,4,3,0,1,2,0}`). Key frames code `intra_frame_y_mode` with
//! `TileIntraFrameYModeCdf[ above ][ left ]`; intra blocks inside other frame
//! types code `y_mode` with `TileYModeCdf[ Size_Group[ MiSize ] ]`.

use crate::reference::av1::cdf_defaults::{
    CdfTable, DEFAULT_INTRA_FRAME_Y_MODE_CDF, DEFAULT_UV_MODE_CFL_ALLOWED_CDF,
    DEFAULT_UV_MODE_CFL_NOT_ALLOWED_CDF, DEFAULT_Y_MODE_CDF,
};
use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::partition::{size_group, BlockSize};
use crate::reference::av1::symbol::SymbolDecoder;

/// `enum IntraMode` in specification order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IntraMode {
    Dc = 0,
    V = 1,
    H = 2,
    D45 = 3,
    D135 = 4,
    D113 = 5,
    D157 = 6,
    D203 = 7,
    D67 = 8,
    Smooth = 9,
    SmoothV = 10,
    SmoothH = 11,
    Paeth = 12,
}

impl IntraMode {
    fn from_symbol(symbol: usize) -> Option<Self> {
        Some(match symbol {
            0 => Self::Dc,
            1 => Self::V,
            2 => Self::H,
            3 => Self::D45,
            4 => Self::D135,
            5 => Self::D113,
            6 => Self::D157,
            7 => Self::D203,
            8 => Self::D67,
            9 => Self::Smooth,
            10 => Self::SmoothV,
            11 => Self::SmoothH,
            12 => Self::Paeth,
            _ => return None,
        })
    }
}

/// `Intra_Mode_Context[ INTRA_MODES ]`.
pub const INTRA_MODE_CONTEXT: [u8; 13] = [0, 1, 2, 3, 4, 4, 4, 4, 3, 0, 1, 2, 0];

/// The thirteen probabilities of one `CdfTable` row, dropping the counter.
fn row(table: &CdfTable, index: usize, width: usize) -> &'static [u16] {
    let start = index * width;
    &table.values[start..start + (width - 1)]
}

fn decode(
    decoder: &mut SymbolDecoder<'_>,
    row: &[u16],
) -> Result<IntraMode, crate::reference::av1::symbol::SymbolError> {
    let mut cdf = row.to_vec();
    cdf.push(0);
    let symbol = decoder.read_symbol(&mut cdf)?;
    Ok(IntraMode::from_symbol(symbol).expect("intra mode within range"))
}

/// `TileIntraFrameYModeCdf[ abovemode ][ leftmode ]`.
pub fn intra_frame_y_mode_cdf(above: Option<IntraMode>, left: Option<IntraMode>) -> &'static [u16] {
    let ctx = |mode: Option<IntraMode>| {
        let mode = mode.unwrap_or(IntraMode::Dc) as usize;
        INTRA_MODE_CONTEXT[mode] as usize
    };
    // Shape [13][13][INTRA_MODES + 1].
    row(
        &DEFAULT_INTRA_FRAME_Y_MODE_CDF,
        ctx(above) * 13 + ctx(left),
        14,
    )
}

/// `intra_frame_y_mode`: the key-frame block mode.
pub fn read_intra_frame_y_mode(
    decoder: &mut SymbolDecoder<'_>,
    above: Option<IntraMode>,
    left: Option<IntraMode>,
) -> Result<IntraMode, crate::reference::av1::symbol::SymbolError> {
    decode(decoder, intra_frame_y_mode_cdf(above, left))
}

/// Stateful key-frame luma mode read. The neighbour-context row stays in the
/// tile's adaptive CDF set.
pub fn read_intra_frame_y_mode_with_cdf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    above: Option<IntraMode>,
    left: Option<IntraMode>,
    disable_cdf_update: bool,
) -> Result<IntraMode, CdfStateError> {
    let ctx = |mode: Option<IntraMode>| {
        INTRA_MODE_CONTEXT[mode.unwrap_or(IntraMode::Dc) as usize] as usize
    };
    let row = ctx(above) * 13 + ctx(left);
    let symbol = cdfs.read_symbol(
        decoder,
        &DEFAULT_INTRA_FRAME_Y_MODE_CDF,
        row * 14,
        14,
        disable_cdf_update,
    )?;
    Ok(IntraMode::from_symbol(symbol).expect("intra mode within range"))
}

/// `TileYModeCdf[ Size_Group[ MiSize ] ]`.
pub fn y_mode_cdf(size: BlockSize) -> &'static [u16] {
    // Shape [BLOCK_SIZE_GROUPS][INTRA_MODES + 1].
    row(&DEFAULT_Y_MODE_CDF, size_group(size) as usize, 14)
}

/// `y_mode`: the intra block mode inside inter frame types.
pub fn read_y_mode(
    decoder: &mut SymbolDecoder<'_>,
    size: BlockSize,
) -> Result<IntraMode, crate::reference::av1::symbol::SymbolError> {
    decode(decoder, y_mode_cdf(size))
}

/// Stateful `y_mode` read for an intra block in an inter frame type.
pub fn read_y_mode_with_cdf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    size: BlockSize,
    disable_cdf_update: bool,
) -> Result<IntraMode, CdfStateError> {
    let row = size_group(size) as usize;
    let symbol = cdfs.read_symbol(
        decoder,
        &DEFAULT_Y_MODE_CDF,
        row * 14,
        14,
        disable_cdf_update,
    )?;
    Ok(IntraMode::from_symbol(symbol).expect("intra mode within range"))
}

/// `enum UvMode`: the thirteen intra modes plus CFL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UvMode {
    Dc = 0,
    V = 1,
    H = 2,
    D45 = 3,
    D135 = 4,
    D113 = 5,
    D157 = 6,
    D203 = 7,
    D67 = 8,
    Smooth = 9,
    SmoothV = 10,
    SmoothH = 11,
    Paeth = 12,
    Cfl = 13,
}

impl UvMode {
    fn from_symbol(symbol: usize) -> Option<Self> {
        Some(match symbol {
            0 => Self::Dc,
            1 => Self::V,
            2 => Self::H,
            3 => Self::D45,
            4 => Self::D135,
            5 => Self::D113,
            6 => Self::D157,
            7 => Self::D203,
            8 => Self::D67,
            9 => Self::Smooth,
            10 => Self::SmoothV,
            11 => Self::SmoothH,
            12 => Self::Paeth,
            13 => Self::Cfl,
            _ => return None,
        })
    }
}

/// `get_plane_residual_size( size, 1 )` for 4:2:0 (both subsampling flags set):
/// each dimension halves with a four-pixel floor.
pub fn residual_size_420(size: BlockSize) -> Option<BlockSize> {
    let width = (size.width / 2).max(4);
    let height = (size.height / 2).max(4);
    crate::reference::av1::partition::BLOCK_SIZES
        .iter()
        .copied()
        .find(|entry| entry.width == width && entry.height == height)
}

/// `uv_mode`'s CFL availability, spec 5.11.4.
pub fn cfl_allowed(size: BlockSize, lossless: bool) -> bool {
    if lossless {
        residual_size_420(size) == Some(crate::reference::av1::partition::BLOCK_SIZES[0])
    } else {
        size.width.max(size.height) <= 32
    }
}

/// `TileUVModeCfl{Allowed,NotAllowed}Cdf[ YMode ]`.
pub fn uv_mode_cdf(y_mode: IntraMode, allowed: bool) -> &'static [u16] {
    let table = if allowed {
        &DEFAULT_UV_MODE_CFL_ALLOWED_CDF
    } else {
        &DEFAULT_UV_MODE_CFL_NOT_ALLOWED_CDF
    };
    let width = if allowed { 15 } else { 14 };
    row(table, y_mode as usize, width)
}

/// `uv_mode`.
pub fn read_uv_mode(
    decoder: &mut SymbolDecoder<'_>,
    y_mode: IntraMode,
    allowed: bool,
) -> Result<UvMode, crate::reference::av1::symbol::SymbolError> {
    let mut cdf = uv_mode_cdf(y_mode, allowed).to_vec();
    cdf.push(0);
    let symbol = decoder.read_symbol(&mut cdf)?;
    Ok(UvMode::from_symbol(symbol).expect("uv mode within range"))
}

/// Stateful `uv_mode` read, selecting the CFL-allowed or ordinary table row.
pub fn read_uv_mode_with_cdf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    y_mode: IntraMode,
    allowed: bool,
    disable_cdf_update: bool,
) -> Result<UvMode, CdfStateError> {
    let (table, width) = if allowed {
        (&DEFAULT_UV_MODE_CFL_ALLOWED_CDF, 15)
    } else {
        (&DEFAULT_UV_MODE_CFL_NOT_ALLOWED_CDF, 14)
    };
    let symbol = cdfs.read_symbol(
        decoder,
        table,
        y_mode as usize * width,
        width,
        disable_cdf_update,
    )?;
    Ok(UvMode::from_symbol(symbol).expect("uv mode within range"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::partition::BLOCK_SIZES;

    fn named(name: &str) -> BlockSize {
        *BLOCK_SIZES.iter().find(|size| size.name == name).unwrap()
    }

    #[test]
    fn cfl_availability_follows_the_specification_rules() {
        let named = |name: &str| *BLOCK_SIZES.iter().find(|size| size.name == name).unwrap();
        // Lossless allows CFL only when the 4:2:0 residual is 4x4.
        assert!(cfl_allowed(named("BLOCK_4X4"), true));
        assert!(cfl_allowed(named("BLOCK_8X8"), true));
        assert!(cfl_allowed(named("BLOCK_4X8"), true));
        assert!(cfl_allowed(named("BLOCK_8X4"), true));
        // 4x16 halves to 4x8, so it is *not* allowed in lossless mode.
        assert!(!cfl_allowed(named("BLOCK_4X16"), true));
        assert!(!cfl_allowed(named("BLOCK_16X16"), true));
        assert!(!cfl_allowed(named("BLOCK_16X8"), true));
        // Otherwise max(width, height) <= 32.
        assert!(cfl_allowed(named("BLOCK_32X32"), false));
        assert!(cfl_allowed(named("BLOCK_32X8"), false));
        assert!(!cfl_allowed(named("BLOCK_32X64"), false));
        assert!(!cfl_allowed(named("BLOCK_64X16"), false));
    }

    #[test]
    fn uv_mode_decodes_with_and_without_cfl() {
        // Expected symbols from the specification pseudocode over the DC rows.
        let bytes = [0u8; 8];
        let mut decoder = SymbolDecoder::init(&bytes).unwrap();
        assert_eq!(
            read_uv_mode(&mut decoder, IntraMode::Dc, false).unwrap(),
            UvMode::Dc
        );
        let mut decoder = SymbolDecoder::init(&bytes).unwrap();
        assert_eq!(
            read_uv_mode(&mut decoder, IntraMode::Dc, true).unwrap(),
            UvMode::Dc
        );

        let bytes = [0xffu8; 8];
        let mut decoder = SymbolDecoder::init(&bytes).unwrap();
        assert_eq!(
            read_uv_mode(&mut decoder, IntraMode::Dc, false).unwrap(),
            UvMode::Paeth
        );
        let mut decoder = SymbolDecoder::init(&bytes).unwrap();
        assert_eq!(
            read_uv_mode(&mut decoder, IntraMode::Dc, true).unwrap(),
            UvMode::Cfl
        );
    }

    #[test]
    fn intra_mode_context_is_the_specification_table() {
        assert_eq!(INTRA_MODE_CONTEXT, [0, 1, 2, 3, 4, 4, 4, 4, 3, 0, 1, 2, 0]);
    }

    #[test]
    fn intra_frame_y_mode_decodes_from_the_defaults() {
        // Expected symbols from an independent run of the specification
        // pseudocode over Default_Intra_Frame_Y_Mode_Cdf[DC][DC].
        for (bytes, expected) in [
            ([0u8; 8], IntraMode::Dc),
            ([0xffu8; 8], IntraMode::Paeth),
            (
                [0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78],
                IntraMode::Dc,
            ),
        ] {
            let mut decoder = SymbolDecoder::init(&bytes).unwrap();
            assert_eq!(
                read_intra_frame_y_mode(&mut decoder, None, None).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn y_mode_decodes_per_size_group() {
        for name in ["BLOCK_4X4", "BLOCK_8X8", "BLOCK_16X16", "BLOCK_32X32"] {
            let mut decoder = SymbolDecoder::init(&[0xffu8; 8]).unwrap();
            assert_eq!(
                read_y_mode(&mut decoder, named(name)).unwrap(),
                IntraMode::Paeth
            );
            let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
            assert_eq!(
                read_y_mode(&mut decoder, named(name)).unwrap(),
                IntraMode::Dc
            );
        }
    }

    #[test]
    fn cdf_selection_uses_neighbour_contexts() {
        // DC and V map to contexts 0 and 1; H to 2.
        assert_eq!(
            intra_frame_y_mode_cdf(None, None)[..2],
            intra_frame_y_mode_cdf(Some(IntraMode::Dc), Some(IntraMode::Dc))[..2]
        );
        assert_ne!(
            intra_frame_y_mode_cdf(Some(IntraMode::Dc), Some(IntraMode::Dc)),
            intra_frame_y_mode_cdf(Some(IntraMode::V), Some(IntraMode::Dc))
        );
    }
}
