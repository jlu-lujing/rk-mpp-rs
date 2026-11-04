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
    CdfTable, DEFAULT_COEFF_BASE_CDF, DEFAULT_COEFF_BASE_EOB_CDF, DEFAULT_COEFF_BR_CDF,
    DEFAULT_EOB_EXTRA_CDF, DEFAULT_EOB_PT_1024_CDF, DEFAULT_EOB_PT_128_CDF, DEFAULT_EOB_PT_16_CDF,
    DEFAULT_EOB_PT_256_CDF, DEFAULT_EOB_PT_32_CDF, DEFAULT_EOB_PT_512_CDF, DEFAULT_EOB_PT_64_CDF,
};
use crate::reference::av1::coeff_tables::{
    ADJUSTED_TX_SIZE, COEFF_BASE_CTX_OFFSET, COEFF_BASE_POS_CTX_OFFSET, SIG_REF_DIFF_OFFSET,
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

/// `SIG_COEF_CONTEXTS`.
pub const SIG_COEF_CONTEXTS: usize = 42;
/// `SIG_COEF_CONTEXTS_2D`.
pub const SIG_COEF_CONTEXTS_2D: usize = 26;
/// The three `get_tx_class` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxClass {
    TwoD = 0,
    Vertical = 1,
    Horizontal = 2,
}

/// `Adjusted_Tx_Size[ txSz ]`.
pub fn adjusted_tx_size(tx: TxSize) -> TxSize {
    ADJUSTED_TX_SIZE[tx as usize]
}

/// `get_coeff_base_ctx(..., isEob = 1)`: the last-coefficient contexts.
pub fn coeff_base_context_eob(tx: TxSize, c: u32) -> usize {
    let adj = adjusted_tx_size(tx);
    let bwl = log2(adj.width());
    let height = adj.height();
    if c == 0 {
        return SIG_COEF_CONTEXTS - 4;
    }
    if c <= (height << bwl) / 8 {
        return SIG_COEF_CONTEXTS - 3;
    }
    if c <= (height << bwl) / 4 {
        return SIG_COEF_CONTEXTS - 2;
    }
    SIG_COEF_CONTEXTS - 1
}

fn coefficient_magnitude(tx: TxSize, pos: u32, quant: &[i32], class: TxClass) -> usize {
    let adj = adjusted_tx_size(tx);
    let bwl = log2(adj.width());
    let width = 1u32 << bwl;
    let height = adj.height();
    let row = (pos >> bwl) as i32;
    let col = (pos & (width - 1)) as i32;
    let mut mag = 0usize;
    for idx in 0..5 {
        let base = (class as usize * 5 + idx) * 2;
        let ref_row = row + i32::from(SIG_REF_DIFF_OFFSET[base]);
        let ref_col = col + i32::from(SIG_REF_DIFF_OFFSET[base + 1]);
        if ref_row >= 0 && ref_col >= 0 && ref_row < height as i32 && ref_col < width as i32 {
            let at = ((ref_row as u32) << bwl) + ref_col as u32;
            mag += quant
                .get(at as usize)
                .copied()
                .unwrap_or(0)
                .unsigned_abs()
                .min(3) as usize;
        }
    }
    mag
}

/// `get_coeff_base_ctx(..., isEob = 0)` for a 2D transform.
pub fn coeff_base_context_2d(tx: TxSize, pos: u32, quant: &[i32]) -> usize {
    let adj = adjusted_tx_size(tx);
    let bwl = log2(adj.width());
    let width = 1u32 << bwl;
    let row = pos >> bwl;
    let col = pos & (width - 1);
    let mag = coefficient_magnitude(tx, pos, quant, TxClass::TwoD);
    let ctx = ((mag + 1) >> 1).min(4);
    if row == 0 && col == 0 {
        return 0;
    }
    let offset =
        COEFF_BASE_CTX_OFFSET[(tx as usize * 25) + (row.min(4) as usize * 5) + col.min(4) as usize];
    ctx + usize::from(offset)
}

/// `get_coeff_base_ctx(..., isEob = 0)` for the 1D transform classes.
pub fn coeff_base_context_1d(tx: TxSize, pos: u32, quant: &[i32], class: TxClass) -> usize {
    let adj = adjusted_tx_size(tx);
    let bwl = log2(adj.width());
    let width = 1u32 << bwl;
    let row = pos >> bwl;
    let col = pos & (width - 1);
    let mag = coefficient_magnitude(tx, pos, quant, class);
    let ctx = ((mag + 1) >> 1).min(4);
    let idx = if class == TxClass::Vertical { row } else { col } as usize;
    ctx + usize::from(COEFF_BASE_POS_CTX_OFFSET[idx.min(2)])
}

/// `SIG_COEF_CONTEXTS_EOB`.
pub const SIG_COEF_CONTEXTS_EOB: usize = 4;
/// `LEVEL_CONTEXTS`.
pub const LEVEL_CONTEXTS: usize = 21;

/// `TileCoeffBaseCdf[ txSzCtx ][ ptype ][ ctx ]` row (four levels + counter).
pub fn coeff_base_cdf_row(q: usize, tx: TxSize, plane_type: usize, ctx: usize) -> &'static [u16] {
    let index = (((q * 5 + tx_size_ctx(tx)) * 2 + plane_type) * SIG_COEF_CONTEXTS) + ctx;
    let start = index * 5;
    &DEFAULT_COEFF_BASE_CDF.values[start..start + 5]
}

/// `TileCoeffBaseEobCdf[ txSzCtx ][ ptype ][ ctx ]` row (three levels + counter).
pub fn coeff_base_eob_cdf_row(
    q: usize,
    tx: TxSize,
    plane_type: usize,
    ctx: usize,
) -> &'static [u16] {
    let local = ctx - (SIG_COEF_CONTEXTS - SIG_COEF_CONTEXTS_EOB);
    debug_assert!(local < SIG_COEF_CONTEXTS_EOB);
    let index = (((q * 5 + tx_size_ctx(tx)) * 2 + plane_type) * SIG_COEF_CONTEXTS_EOB) + local;
    let start = index * 4;
    &DEFAULT_COEFF_BASE_EOB_CDF.values[start..start + 4]
}

/// `TileCoeffBrCdf[ txSzCtx ][ ptype ][ ctx ]` row (four values + counter).
pub fn coeff_br_cdf_row(q: usize, tx: TxSize, plane_type: usize, ctx: usize) -> &'static [u16] {
    // `Min( txSzCtx, TX_32X32 )`: the br table stops at index 3.
    let tx_ctx = tx_size_ctx(tx).min(3);
    let index = (((q * 5 + tx_ctx) * 2 + plane_type) * LEVEL_CONTEXTS) + ctx;
    let start = index * 5;
    &DEFAULT_COEFF_BR_CDF.values[start..start + 5]
}

/// `coeff_base: S()` for one coefficient (returns the base level 0..3).
pub fn read_coeff_base(
    decoder: &mut SymbolDecoder<'_>,
    tx: TxSize,
    q: usize,
    plane_type: usize,
    ctx: usize,
) -> Result<u32, SymbolError> {
    let mut cdf = coeff_base_cdf_row(q, tx, plane_type, ctx).to_vec();
    Ok(decoder.read_symbol(&mut cdf)? as u32)
}

/// `coeff_base_eob: S()` for the last coefficient (returns 1..3).
pub fn read_coeff_base_eob(
    decoder: &mut SymbolDecoder<'_>,
    tx: TxSize,
    q: usize,
    plane_type: usize,
    ctx: usize,
) -> Result<u32, SymbolError> {
    let mut cdf = coeff_base_eob_cdf_row(q, tx, plane_type, ctx).to_vec();
    Ok(decoder.read_symbol(&mut cdf)? as u32 + 1)
}

/// `coeff_br: S()` in the level loop.
pub fn read_coeff_br(
    decoder: &mut SymbolDecoder<'_>,
    tx: TxSize,
    q: usize,
    plane_type: usize,
    ctx: usize,
) -> Result<u32, SymbolError> {
    let mut cdf = coeff_br_cdf_row(q, tx, plane_type, ctx).to_vec();
    Ok(decoder.read_symbol(&mut cdf)? as u32)
}

/// `NUM_BASE_LEVELS`.
pub const NUM_BASE_LEVELS: u32 = 2;
/// `COEFF_BASE_RANGE`.
pub const COEFF_BASE_RANGE: u32 = 12;
/// `BR_CDF_SIZE`.
pub const BR_CDF_SIZE: u32 = 4;

/// `Mag_Ref_Offset_With_Tx_Class[ 3 ][ 3 ][ 2 ]`.
const MAG_REF_OFFSET: [[[i8; 2]; 3]; 3] = [
    [[0, 1], [1, 0], [1, 1]],
    [[0, 1], [1, 0], [0, 2]],
    [[0, 1], [1, 0], [2, 0]],
];

/// `get_coeff_br_ctx` (0..20).
pub fn coeff_br_context(tx: TxSize, pos: u32, quant: &[i32], class: TxClass) -> usize {
    let adj = adjusted_tx_size(tx);
    let bwl = log2(adj.width());
    let txw = 1u32 << bwl;
    let txh = adj.height();
    let row = pos >> bwl;
    let col = pos & (txw - 1);
    let mut mag = 0u32;
    for idx in 0..3 {
        let ref_row = row as i32 + i32::from(MAG_REF_OFFSET[class as usize][idx][0]);
        let ref_col = col as i32 + i32::from(MAG_REF_OFFSET[class as usize][idx][1]);
        if ref_row >= 0 && ref_col >= 0 && ref_row < txh as i32 && ref_col < txw as i32 {
            let at = (ref_row as u32 * txw + ref_col as u32) as usize;
            let value = quant.get(at).copied().unwrap_or(0).unsigned_abs();
            mag += value.min(COEFF_BASE_RANGE + NUM_BASE_LEVELS + 1);
        }
    }
    let mag = ((mag + 1) >> 1).min(6) as usize;
    if pos == 0 {
        return mag;
    }
    let near = match class {
        TxClass::TwoD => row < 2 && col < 2,
        TxClass::Vertical => col == 0,
        TxClass::Horizontal => row == 0,
    };
    mag + if near { 7 } else { 14 }
}

/// Decode the level loop of `coeffs()` into `quant` (the scan positions are the
/// specification's `scan[c]` order). Signs are a separate pass.
///
/// Returns the number of coefficients written (`eob`).
pub fn decode_levels(
    decoder: &mut SymbolDecoder<'_>,
    tx: TxSize,
    quant: &mut [i32],
    scan: &[u16],
    eob: u32,
    q: usize,
    plane_type: usize,
    class: TxClass,
) -> Result<u32, SymbolError> {
    for c in (0..eob).rev() {
        let pos = u32::from(scan[c as usize]);
        let level = if c == eob - 1 {
            let ctx = coeff_base_context_eob(tx, c);
            read_coeff_base_eob(decoder, tx, q, plane_type, ctx)?
        } else {
            let ctx = match class {
                TxClass::TwoD => coeff_base_context_2d(tx, pos, quant),
                other => coeff_base_context_1d(tx, pos, quant, other),
            };
            read_coeff_base(decoder, tx, q, plane_type, ctx)?
        };
        let mut level = level;
        if level > NUM_BASE_LEVELS {
            for _ in 0..(COEFF_BASE_RANGE / (BR_CDF_SIZE - 1)) {
                let ctx = coeff_br_context(tx, pos, quant, class);
                let br = read_coeff_br(decoder, tx, q, plane_type, ctx)?;
                level += br;
                if br < BR_CDF_SIZE - 1 {
                    break;
                }
            }
        }
        if let Some(slot) = quant.get_mut(pos as usize) {
            *slot = level as i32;
        }
    }
    Ok(eob)
}

/// The sign and Golomb pass of `coeffs()`.
///
/// `dc_cdf` is the `TileDcSignCdf[ ptype ][ ctx ]` row selected by the tile
/// driver from `AboveDcContext`/`LeftDcContext` (three values: probability,
/// scale, counter). Returns `(dcCategory, culLevel)`.
pub fn decode_signs(
    decoder: &mut SymbolDecoder<'_>,
    quant: &mut [i32],
    scan: &[u16],
    eob: u32,
    dc_cdf: &mut [u16],
) -> Result<(i32, u32), SymbolError> {
    let mut cul_level = 0u32;
    let mut dc_category = 0i32;
    for c in 0..eob {
        let pos = u32::from(scan[c as usize]);
        let level = quant.get(pos as usize).copied().unwrap_or(0);
        let sign = if level != 0 {
            if c == 0 {
                decoder.read_symbol(dc_cdf)? as i32
            } else {
                decoder.read_bool()? as i32
            }
        } else {
            0
        };
        let mut magnitude = level;
        if magnitude > (NUM_BASE_LEVELS + COEFF_BASE_RANGE) as i32 {
            let mut length = 0u32;
            loop {
                length += 1;
                if decoder.read_bool()? != 0 {
                    break;
                }
            }
            let mut x = 1u32;
            for _ in 0..length.saturating_sub(1) {
                x = (x << 1) | decoder.read_bool()?;
            }
            magnitude = (x + COEFF_BASE_RANGE + NUM_BASE_LEVELS) as i32;
        }
        if pos == 0 && magnitude > 0 {
            dc_category = if sign != 0 { 1 } else { 2 };
        }
        magnitude &= 0xFFFFF;
        cul_level += magnitude as u32;
        if let Some(slot) = quant.get_mut(pos as usize) {
            *slot = if sign != 0 { -magnitude } else { magnitude };
        }
    }
    Ok((dc_category, cul_level.min(63)))
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
    fn signs_and_golomb_finish_the_quant_buffer() {
        let scan = [0u16, 1, 4, 8];
        let mut dc_cdf = [16384u16, 32768, 0];

        // A 0xff stream gives sign 1 on the DC coefficient.
        let mut quant = [5i32, 0, 0, 0];
        let mut decoder = SymbolDecoder::init(&[0xff; 8]).unwrap();
        let (category, cul) =
            decode_signs(&mut decoder, &mut quant, &scan, 1, &mut dc_cdf).unwrap();
        assert_eq!(quant, [-5, 0, 0, 0]);
        assert_eq!(category, 1);
        assert_eq!(cul, 5);

        // A magnitude above the base range is rebuilt from the Golomb suffix.
        let mut quant = [20i32, 0, 0, 0];
        let mut decoder = SymbolDecoder::init(&[0xff; 8]).unwrap();
        let (category, cul) =
            decode_signs(&mut decoder, &mut quant, &scan, 1, &mut dc_cdf).unwrap();
        assert_eq!(quant, [-15, 0, 0, 0]);
        assert_eq!(category, 1);
        assert_eq!(cul, 15);

        // Zero levels consume nothing and report no DC category.
        let mut quant = [0i32; 4];
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        let (category, cul) =
            decode_signs(&mut decoder, &mut quant, &scan, 1, &mut dc_cdf).unwrap();
        assert_eq!((category, cul), (0, 0));
    }

    #[test]
    fn br_context_adds_the_position_offsets() {
        let quant = [0i32; 16];
        assert_eq!(coeff_br_context(TxSize::Tx4x4, 0, &quant, TxClass::TwoD), 0);
        // (0,1) is near the DC corner: mag 0 + 7.
        assert_eq!(coeff_br_context(TxSize::Tx4x4, 1, &quant, TxClass::TwoD), 7);
        // (3,3) is far: mag 0 + 14.
        assert_eq!(
            coeff_br_context(TxSize::Tx4x4, 15, &quant, TxClass::TwoD),
            14
        );
        // Vertical looks at the column, horizontal at the row.
        assert_eq!(
            coeff_br_context(TxSize::Tx4x4, 4, &quant, TxClass::Vertical),
            7
        );
        assert_eq!(
            coeff_br_context(TxSize::Tx4x4, 1, &quant, TxClass::Horizontal),
            7
        );
        assert_eq!(
            coeff_br_context(TxSize::Tx4x4, 4, &quant, TxClass::Horizontal),
            14
        );
    }

    #[test]
    fn decode_levels_fills_the_quant_buffer() {
        use crate::reference::av1::scan_defaults::DEFAULT_SCAN_4X4;
        let mut expected_zero = [0i32; 16];
        expected_zero[0] = 1;
        let expected_max = [15i32; 16];
        for (bytes, expected) in [([0u8; 8], expected_zero), ([0xffu8; 8], expected_max)] {
            let mut decoder = SymbolDecoder::init(&bytes).unwrap();
            let eob = decode_eob(&mut decoder, TxSize::Tx4x4, 0, 0, true).unwrap();
            let mut quant = [0i32; 16];
            decode_levels(
                &mut decoder,
                TxSize::Tx4x4,
                &mut quant,
                &DEFAULT_SCAN_4X4.order,
                eob,
                0,
                0,
                TxClass::TwoD,
            )
            .unwrap();
            assert_eq!(quant, expected, "{} bytes", bytes[0]);
        }
    }

    #[test]
    fn coefficient_cdf_rows_start_where_the_tables_do() {
        assert_eq!(
            coeff_base_cdf_row(0, TxSize::Tx4x4, 0, 0)[..4],
            [4034, 8930, 12727, 32768]
        );
        assert_eq!(
            coeff_base_eob_cdf_row(0, TxSize::Tx4x4, 0, 38)[..3],
            [17837, 29055, 32768]
        );
        assert_eq!(
            coeff_br_cdf_row(0, TxSize::Tx4x4, 0, 0)[..4],
            [14298, 20718, 24174, 32768]
        );
    }

    #[test]
    fn coefficient_levels_decode_from_the_defaults() {
        for (bytes, base, eob, br) in [
            ([0u8; 8], 0u32, 1u32, 0u32),
            ([0xffu8; 8], 3, 3, 3),
            ([0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78], 0, 1, 0),
        ] {
            let mut decoder = SymbolDecoder::init(&bytes).unwrap();
            assert_eq!(
                read_coeff_base(&mut decoder, TxSize::Tx4x4, 0, 0, 0).unwrap(),
                base
            );
            let mut decoder = SymbolDecoder::init(&bytes).unwrap();
            assert_eq!(
                read_coeff_base_eob(&mut decoder, TxSize::Tx4x4, 0, 0, 38).unwrap(),
                eob
            );
            let mut decoder = SymbolDecoder::init(&bytes).unwrap();
            assert_eq!(
                read_coeff_br(&mut decoder, TxSize::Tx4x4, 0, 0, 0).unwrap(),
                br
            );
        }
    }

    #[test]
    fn eob_contexts_split_the_coefficient_range() {
        // 4x4: height << bwl = 16, so c <= 2 -> 39, c <= 4 -> 40, else 41.
        assert_eq!(coeff_base_context_eob(TxSize::Tx4x4, 0), 38);
        assert_eq!(coeff_base_context_eob(TxSize::Tx4x4, 1), 39);
        assert_eq!(coeff_base_context_eob(TxSize::Tx4x4, 2), 39);
        assert_eq!(coeff_base_context_eob(TxSize::Tx4x4, 3), 40);
        assert_eq!(coeff_base_context_eob(TxSize::Tx4x4, 5), 41);
    }

    #[test]
    fn two_d_context_uses_the_neighbour_magnitudes() {
        let quant = [0i32; 64];
        assert_eq!(coeff_base_context_2d(TxSize::Tx4x4, 0, &quant), 0);
        // DC's right neighbour (0,1) counts: mag 1 -> ctx 1, offset[0][0]=0.
        let mut quant = [0i32; 64];
        quant[1] = 1;
        assert_eq!(coeff_base_context_2d(TxSize::Tx4x4, 0, &quant), 0);
        let mut quant = [0i32; 64];
        quant[1] = 3;
        // The coefficient at pos 1 is not its own neighbour; offset[0][1] = 1.
        assert_eq!(coeff_base_context_2d(TxSize::Tx4x4, 1, &quant), 1);
        // A saturated neighbour still caps ctx at 4.
        let mut quant = [0i32; 64];
        quant[1] = -9;
        quant[2] = 9;
        quant[3] = 9;
        // mag 6 -> ctx 3, plus offset[0][1] = 1.
        assert_eq!(coeff_base_context_2d(TxSize::Tx4x4, 1, &quant), 4);
    }

    #[test]
    fn one_d_context_uses_the_position_offsets() {
        let quant = [0i32; 64];
        assert_eq!(
            coeff_base_context_1d(TxSize::Tx4x4, 0, &quant, TxClass::Vertical),
            26
        );
        assert_eq!(
            coeff_base_context_1d(TxSize::Tx4x4, 4, &quant, TxClass::Vertical),
            31
        );
        // Horizontal uses the column: col 1 -> pos offset 31.
        assert_eq!(
            coeff_base_context_1d(TxSize::Tx4x4, 1, &quant, TxClass::Horizontal),
            31
        );
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
