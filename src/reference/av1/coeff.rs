//! AV1 coefficient syntax scaffolding: the EOB class of a transform block.
//!
//! `coeffs()` starts with `segEob`, then `eobMultisize` selects which of the
//! seven `TileEobPt*Cdf` arrays codes the EOB class. Both are pure functions of
//! the transform size, so they are pinned here before the symbol loop is added.
//!
//! ```text
//! segEob = (txSz == TX_16X64 || txSz == TX_64X16) ? 512 : Min(1024, w * h)
//! eobMultisize = Min(Tx_Width_Log2, 5) + Min(Tx_Height_Log2, 5) - 4
//! ```

use crate::reference::av1::cdf_defaults::{
    CdfTable, DEFAULT_EOB_PT_1024_CDF, DEFAULT_EOB_PT_128_CDF, DEFAULT_EOB_PT_16_CDF,
    DEFAULT_EOB_PT_256_CDF, DEFAULT_EOB_PT_32_CDF, DEFAULT_EOB_PT_512_CDF, DEFAULT_EOB_PT_64_CDF,
};
use crate::reference::av1::tx::TxSize;

fn log2(value: u32) -> u32 {
    value.trailing_zeros()
}

/// `segEob`: how many coefficient positions the block can signal.
pub fn seg_eob(tx: TxSize) -> u32 {
    match tx {
        TxSize::Tx16x64 | TxSize::Tx64x16 => 512,
        _ => (tx.width() * tx.height()).min(1024),
    }
}

/// `eobMultisize`.
pub fn eob_multisize(tx: TxSize) -> u32 {
    log2(tx.width()).min(5) + log2(tx.height()).min(5) - 4
}

/// The `TileEobPt*Cdf` array selected by `eobMultisize` (values above 6 all use
/// the 1024 table, as the specification's if-chain does).
pub fn eob_pt_table(multisize: u32) -> &'static CdfTable {
    match multisize {
        0 => &DEFAULT_EOB_PT_16_CDF,
        1 => &DEFAULT_EOB_PT_32_CDF,
        2 => &DEFAULT_EOB_PT_64_CDF,
        3 => &DEFAULT_EOB_PT_128_CDF,
        4 => &DEFAULT_EOB_PT_256_CDF,
        5 => &DEFAULT_EOB_PT_512_CDF,
        _ => &DEFAULT_EOB_PT_1024_CDF,
    }
}

/// `eob` from the syntax's `eobPt` (after its `+1`): positions below the first
/// power of two are literal, above it a base plus `eobShift` extra bits.
pub fn eob_from_pt(eob_pt: u32) -> u32 {
    if eob_pt < 2 {
        eob_pt
    } else {
        (1 << (eob_pt - 2)) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seg_eob_caps_large_blocks_and_the_wide_rectangles() {
        assert_eq!(seg_eob(TxSize::Tx4x4), 16);
        assert_eq!(seg_eob(TxSize::Tx16x16), 256);
        assert_eq!(seg_eob(TxSize::Tx32x32), 1024);
        assert_eq!(seg_eob(TxSize::Tx64x64), 1024);
        assert_eq!(seg_eob(TxSize::Tx16x64), 512);
        assert_eq!(seg_eob(TxSize::Tx64x16), 512);
    }

    #[test]
    fn eob_multisize_matches_the_log2_sum() {
        assert_eq!(eob_multisize(TxSize::Tx4x4), 0);
        assert_eq!(eob_multisize(TxSize::Tx8x8), 2);
        assert_eq!(eob_multisize(TxSize::Tx16x16), 4);
        assert_eq!(eob_multisize(TxSize::Tx32x32), 6);
        assert_eq!(eob_multisize(TxSize::Tx64x64), 6);
        assert_eq!(eob_multisize(TxSize::Tx4x16), 2);
    }

    #[test]
    fn eob_pt_tables_follow_the_multisize() {
        assert_eq!(eob_pt_table(0).name, "Default_Eob_Pt_16_Cdf");
        assert_eq!(eob_pt_table(1).name, "Default_Eob_Pt_32_Cdf");
        assert_eq!(eob_pt_table(2).name, "Default_Eob_Pt_64_Cdf");
        assert_eq!(eob_pt_table(3).name, "Default_Eob_Pt_128_Cdf");
        assert_eq!(eob_pt_table(4).name, "Default_Eob_Pt_256_Cdf");
        assert_eq!(eob_pt_table(5).name, "Default_Eob_Pt_512_Cdf");
        assert_eq!(eob_pt_table(6).name, "Default_Eob_Pt_1024_Cdf");
        assert_eq!(eob_pt_table(9).name, "Default_Eob_Pt_1024_Cdf");
    }

    #[test]
    fn eob_from_pt_is_literal_below_two_and_power_plus_one_above() {
        assert_eq!(eob_from_pt(1), 1);
        assert_eq!(eob_from_pt(2), 2);
        assert_eq!(eob_from_pt(3), 3);
        assert_eq!(eob_from_pt(4), 5);
        assert_eq!(eob_from_pt(5), 9);
        assert_eq!(eob_from_pt(11), 513);
    }
}
