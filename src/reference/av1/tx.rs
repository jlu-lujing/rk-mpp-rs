//! AV1 transform-size tables and the per-block `tx_depth` symbol.
//!
//! `Max_Tx_Size_Rect`/`Max_Tx_Depth` are the specification's
//! `BLOCK_SIZES` tables, `Split_Tx_Size` its `TX_SIZES_ALL` halving chain. The
//! block's `tx_depth` is `S()` on `TileTxfmSplitCdf[ctx]` where `ctx` compares
//! the available neighbour transform extents against the block's maximum
//! transform (`(aboveW >= maxTxWidth) + (leftH >= maxTxHeight)`). Neighbour
//! tracking lives in the tile driver; this module owns the tables and the
//! decode of the symbol itself.

use crate::reference::av1::cdf_defaults::{
    CdfTable, DEFAULT_TX_16X16_CDF, DEFAULT_TX_32X32_CDF, DEFAULT_TX_64X64_CDF, DEFAULT_TX_8X8_CDF,
};
use crate::reference::av1::partition::{BlockSize, BLOCK_SIZES};
use crate::reference::av1::symbol::{SymbolDecoder, SymbolError};

/// `enum TxSize`, `TX_SIZES_ALL` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TxSize {
    Tx4x4 = 0,
    Tx8x8 = 1,
    Tx16x16 = 2,
    Tx32x32 = 3,
    Tx64x64 = 4,
    Tx4x8 = 5,
    Tx8x4 = 6,
    Tx8x16 = 7,
    Tx16x8 = 8,
    Tx16x32 = 9,
    Tx32x16 = 10,
    Tx32x64 = 11,
    Tx64x32 = 12,
    Tx4x16 = 13,
    Tx16x4 = 14,
    Tx8x32 = 15,
    Tx32x8 = 16,
    Tx16x64 = 17,
    Tx64x16 = 18,
}

impl TxSize {
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Tx4x4 => (4, 4),
            Self::Tx8x8 => (8, 8),
            Self::Tx16x16 => (16, 16),
            Self::Tx32x32 => (32, 32),
            Self::Tx64x64 => (64, 64),
            Self::Tx4x8 => (4, 8),
            Self::Tx8x4 => (8, 4),
            Self::Tx8x16 => (8, 16),
            Self::Tx16x8 => (16, 8),
            Self::Tx16x32 => (16, 32),
            Self::Tx32x16 => (32, 16),
            Self::Tx32x64 => (32, 64),
            Self::Tx64x32 => (64, 32),
            Self::Tx4x16 => (4, 16),
            Self::Tx16x4 => (16, 4),
            Self::Tx8x32 => (8, 32),
            Self::Tx32x8 => (32, 8),
            Self::Tx16x64 => (16, 64),
            Self::Tx64x16 => (64, 16),
        }
    }

    pub fn width(self) -> u32 {
        self.dimensions().0
    }

    pub fn height(self) -> u32 {
        self.dimensions().1
    }
}

use TxSize::*;

/// `Max_Tx_Size_Rect[ BLOCK_SIZES ]` in `BLOCK_SIZES` order.
pub const BLOCK_MAX_TX_SIZE_RECT: [TxSize; 22] = [
    Tx4x4, Tx4x8, Tx8x4, Tx8x8, Tx8x16, Tx16x8, Tx16x16, Tx16x32, Tx32x16, Tx32x32, Tx32x64,
    Tx64x32, Tx64x64, Tx64x64, Tx64x64, Tx64x64, Tx4x16, Tx16x4, Tx8x32, Tx32x8, Tx16x64, Tx64x16,
];

/// `Max_Tx_Depth[ BLOCK_SIZES ]`.
pub const BLOCK_MAX_TX_DEPTH: [u8; 22] = [
    0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4, 4, 4, 4, 2, 2, 3, 3, 4, 4,
];

/// `Split_Tx_Size[ TX_SIZES_ALL ]`.
pub const SPLIT_TX_SIZE: [TxSize; 19] = [
    Tx4x4, Tx4x4, Tx8x8, Tx16x16, Tx32x32, Tx4x4, Tx4x4, Tx8x8, Tx8x8, Tx16x16, Tx16x16, Tx32x32,
    Tx32x32, Tx4x8, Tx8x4, Tx8x16, Tx16x8, Tx16x32, Tx32x16,
];

fn size_index(size: BlockSize) -> usize {
    BLOCK_SIZES
        .iter()
        .position(|entry| *entry == size)
        .expect("block size is one of BLOCK_SIZES")
}

/// `Max_Tx_Size_Rect[ MiSize ]`.
pub fn max_tx_size_rect(size: BlockSize) -> TxSize {
    BLOCK_MAX_TX_SIZE_RECT[size_index(size)]
}

/// `Max_Tx_Depth[ MiSize ]`.
pub fn max_tx_depth(size: BlockSize) -> u8 {
    BLOCK_MAX_TX_DEPTH[size_index(size)]
}

/// `Split_Tx_Size[ txSz ]`.
pub fn split_tx_size(tx: TxSize) -> TxSize {
    SPLIT_TX_SIZE[tx as usize]
}

/// `ctx = (aboveW >= maxTxWidth) + (leftH >= maxTxHeight)` for the `tx_depth`
/// symbol. The result is 0..2; the table itself is chosen by
/// `Max_Tx_Depth[ MiSize ]`.
pub fn tx_depth_context(above_width: u32, left_height: u32, max: TxSize) -> usize {
    usize::from(above_width >= max.width()) + usize::from(left_height >= max.height())
}

/// `TileTx{64x64,32x32,16x16,8x8}Cdf` selected by the block's max depth:
/// 4 -> 64, 3 -> 32, 2 -> 16, otherwise 8.
pub fn tx_depth_table(size: BlockSize) -> &'static CdfTable {
    match max_tx_depth(size) {
        4 => &DEFAULT_TX_64X64_CDF,
        3 => &DEFAULT_TX_32X32_CDF,
        2 => &DEFAULT_TX_16X16_CDF,
        _ => &DEFAULT_TX_8X8_CDF,
    }
}

/// `tx_depth: S()` on the table [`tx_depth_table`] selects.
pub fn read_tx_depth(
    decoder: &mut SymbolDecoder<'_>,
    size: BlockSize,
    above_width: u32,
    left_height: u32,
) -> Result<u32, SymbolError> {
    let table = tx_depth_table(size);
    let max = max_tx_size_rect(size);
    let ctx = tx_depth_context(above_width, left_height, max);
    let start = ctx * 3;
    let mut cdf = table.values[start..start + 3].to_vec();
    let depth = decoder.read_symbol(&mut cdf)?;
    Ok(depth as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> BlockSize {
        *BLOCK_SIZES.iter().find(|size| size.name == name).unwrap()
    }

    #[test]
    fn max_tx_tables_follow_the_specification() {
        assert_eq!(max_tx_size_rect(named("BLOCK_4X4")), Tx4x4);
        assert_eq!(max_tx_size_rect(named("BLOCK_16X8")), Tx16x8);
        assert_eq!(max_tx_size_rect(named("BLOCK_128X128")), Tx64x64);
        assert_eq!(max_tx_depth(named("BLOCK_4X4")), 0);
        assert_eq!(max_tx_depth(named("BLOCK_16X16")), 2);
        assert_eq!(max_tx_depth(named("BLOCK_128X128")), 4);
        assert_eq!(max_tx_depth(named("BLOCK_16X64")), 4);
    }

    #[test]
    fn split_chain_halves_square_transforms() {
        assert_eq!(split_tx_size(Tx64x64), Tx32x32);
        assert_eq!(split_tx_size(Tx32x32), Tx16x16);
        assert_eq!(split_tx_size(Tx16x16), Tx8x8);
        assert_eq!(split_tx_size(Tx8x8), Tx4x4);
        assert_eq!(split_tx_size(Tx4x4), Tx4x4);
        assert_eq!(split_tx_size(Tx16x32), Tx16x16);
    }

    #[test]
    fn tx_depth_context_compares_neighbour_extents() {
        let max = Tx16x16;
        assert_eq!(tx_depth_context(0, 0, max), 0);
        assert_eq!(tx_depth_context(16, 0, max), 1);
        assert_eq!(tx_depth_context(0, 16, max), 1);
        assert_eq!(tx_depth_context(32, 32, max), 2);
        assert_eq!(tx_depth_context(8, 8, max), 0);
    }

    #[test]
    fn tx_depth_tables_follow_the_max_depth() {
        assert_eq!(
            tx_depth_table(named("BLOCK_8X8")).name,
            "Default_Tx_8x8_Cdf"
        );
        assert_eq!(
            tx_depth_table(named("BLOCK_16X16")).name,
            "Default_Tx_16x16_Cdf"
        );
        assert_eq!(
            tx_depth_table(named("BLOCK_32X32")).name,
            "Default_Tx_32x32_Cdf"
        );
        assert_eq!(
            tx_depth_table(named("BLOCK_64X64")).name,
            "Default_Tx_64x64_Cdf"
        );
        assert_eq!(
            tx_depth_table(named("BLOCK_128X128")).name,
            "Default_Tx_64x64_Cdf"
        );
    }

    #[test]
    fn tx_depth_decodes_per_context() {
        // Expected symbols from the specification pseudocode over
        // Default_Tx_8x8_Cdf context 0 (BLOCK_8X8, no neighbours).
        for (bytes, expected) in [
            ([0u8; 8], 0u32),
            ([0xffu8; 8], 1),
            ([0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78], 0),
        ] {
            let mut decoder = SymbolDecoder::init(&bytes).unwrap();
            assert_eq!(
                read_tx_depth(&mut decoder, named("BLOCK_8X8"), 0, 0).unwrap(),
                expected
            );
        }
    }
}
