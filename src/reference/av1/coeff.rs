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
    CdfTable, DEFAULT_EOB_EXTRA_CDF, DEFAULT_EOB_PT_1024_CDF, DEFAULT_EOB_PT_128_CDF,
    DEFAULT_EOB_PT_16_CDF, DEFAULT_EOB_PT_256_CDF, DEFAULT_EOB_PT_32_CDF, DEFAULT_EOB_PT_512_CDF,
    DEFAULT_EOB_PT_64_CDF,
};
use crate::reference::av1::symbol::{SymbolDecoder, SymbolError};
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

/// `eob_pt` symbol count for a multisize: 5 for 16, one more per class up to
/// 11 for 1024 (`CDF_SIZE(5..11)` in the reference implementation).
pub fn eob_symbol_count(multisize: u32) -> usize {
    5 + multisize.min(6) as usize
}

/// The selected EOB row. The row already includes the adaptation counter.
///
/// `TileEobPt{16,32,64,128,256}Cdf` are indexed `[ q ][ ptype ][ tx_class ]`
/// (the specification's selection is `[ ptype ][ ctx ]` on the q-selected
/// copy); 512 and 1024 have no tx-class dimension, so they are
/// `[ q ][ ptype ]`.
pub fn eob_row(
    table: &CdfTable,
    q: usize,
    plane_type: usize,
    tx_class: usize,
    multisize: u32,
) -> &'static [u16] {
    let width = eob_symbol_count(multisize) + 1;
    let index = if multisize <= 4 {
        (((q * 2 + plane_type) * 2) + tx_class) * width
    } else {
        (q * 2 + plane_type) * width
    };
    &table.values[index..index + width]
}

/// `eob_pt = S(...) + 1` for the block's class, plane and transform class.
pub fn read_eob_pt(
    decoder: &mut SymbolDecoder<'_>,
    tx: TxSize,
    q: usize,
    plane_type: usize,
    tx_class_2d: bool,
) -> Result<u32, SymbolError> {
    let multisize = eob_multisize(tx);
    let table = eob_pt_table(multisize);
    let tx_class = usize::from(!tx_class_2d);
    let mut cdf = eob_row(table, q, plane_type, tx_class, multisize).to_vec();
    let symbol = decoder.read_symbol(&mut cdf)?;
    Ok(symbol as u32 + 1)
}

/// `Tx_Size_Sqr[ txSz ]`: the square transform index of the shorter side.
pub fn tx_sqr_index(tx: TxSize) -> usize {
    (log2(tx.width().min(tx.height())) - 2) as usize
}

/// `Tx_Size_Sqr_Up[ txSz ]`: the square transform index of the longer side.
pub fn tx_sqr_up_index(tx: TxSize) -> usize {
    (log2(tx.width().max(tx.height())) - 2) as usize
}

/// `txSzCtx = ( Tx_Size_Sqr[txSz] + Tx_Size_Sqr_Up[txSz] + 1 ) >> 1`.
pub fn tx_size_ctx(tx: TxSize) -> usize {
    (tx_sqr_index(tx) + tx_sqr_up_index(tx) + 1) >> 1
}

/// `TileEobExtraCdf[ txSzCtx ][ ptype ][ eobPt - 3 ]` row (three values:
/// two probabilities plus the adaptation counter).
pub fn eob_extra_row(q: usize, tx: TxSize, plane_type: usize, eob_pt: u32) -> &'static [u16] {
    let ctx = (eob_pt - 3) as usize;
    let index = ((((q * 5 + tx_size_ctx(tx)) * 2 + plane_type) * 9) + ctx) * 3;
    &DEFAULT_EOB_EXTRA_CDF.values[index..index + 3]
}

/// The full `eob` of a non-zero coefficient block: the EOB class plus its
/// `eob_extra` symbol and the `eob_extra_bit` literals.
pub fn decode_eob(
    decoder: &mut SymbolDecoder<'_>,
    tx: TxSize,
    q: usize,
    plane_type: usize,
    tx_class_2d: bool,
) -> Result<u32, SymbolError> {
    let eob_pt = read_eob_pt(decoder, tx, q, plane_type, tx_class_2d)?;
    let mut eob = eob_from_pt(eob_pt);
    let mut shift = (eob_pt as i32 - 3).max(-1);
    if shift >= 0 {
        let mut cdf = eob_extra_row(q, tx, plane_type, eob_pt).to_vec();
        if decoder.read_symbol(&mut cdf)? != 0 {
            eob += 1 << shift;
        }
        for i in 1..(eob_pt as i32 - 2).max(0) {
            shift = (eob_pt as i32 - 2).max(0) - 1 - i;
            if decoder.read_bool()? != 0 {
                eob += 1 << shift;
            }
        }
    }
    let limit = seg_eob(tx);
    if eob > limit {
        return Err(SymbolError::EobOutOfRange { eob, limit });
    }
    Ok(eob)
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
    fn eob_tables_hold_one_more_symbol_per_class() {
        assert_eq!(eob_symbol_count(0), 5);
        assert_eq!(eob_symbol_count(1), 6);
        assert_eq!(eob_symbol_count(3), 8);
        assert_eq!(eob_symbol_count(6), 11);
        assert_eq!(eob_symbol_count(9), 11);
        for multisize in 0..=6 {
            let table = eob_pt_table(multisize);
            let per_row = eob_symbol_count(multisize) + 1;
            // 16..256 carry a tx-class dimension; 512/1024 do not.
            let rows = if multisize <= 4 { 4 * 2 * 2 } else { 4 * 2 };
            assert_eq!(table.values.len(), rows * per_row, "multisize {multisize}");
        }
    }

    #[test]
    fn eob_pt_decodes_per_plane_and_class() {
        // Expected symbols from the specification pseudocode over the 16 row.
        for (bytes, expected) in [
            ([0u8; 8], 1u32),
            ([0xffu8; 8], 5),
            ([0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78], 4),
        ] {
            let mut decoder = SymbolDecoder::init(&bytes).unwrap();
            assert_eq!(
                read_eob_pt(&mut decoder, TxSize::Tx4x4, 0, 0, true).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn tx_size_context_averages_the_square_indices() {
        use crate::reference::av1::tx::TxSize;
        assert_eq!(tx_size_ctx(TxSize::Tx4x4), 0);
        assert_eq!(tx_size_ctx(TxSize::Tx4x8), 1);
        assert_eq!(tx_size_ctx(TxSize::Tx8x8), 1);
        assert_eq!(tx_size_ctx(TxSize::Tx16x8), 2);
        assert_eq!(tx_size_ctx(TxSize::Tx16x16), 2);
        assert_eq!(tx_size_ctx(TxSize::Tx16x32), 3);
        assert_eq!(tx_size_ctx(TxSize::Tx32x32), 3);
        assert_eq!(tx_size_ctx(TxSize::Tx64x64), 4);
        // (sqr 0 + sqr_up 2 + 1) >> 1 = 1, not the larger side.
        assert_eq!(tx_size_ctx(TxSize::Tx4x16), 1);
    }

    #[test]
    fn decode_eob_adds_the_extra_bits() {
        // Expected values from the specification pseudocode.
        for (bytes, expected) in [
            ([0u8; 8], 1u32),
            ([0xffu8; 8], 16),
            ([0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78], 5),
        ] {
            let mut decoder = SymbolDecoder::init(&bytes).unwrap();
            assert_eq!(
                decode_eob(&mut decoder, TxSize::Tx4x4, 0, 0, true).unwrap(),
                expected
            );
        }
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
