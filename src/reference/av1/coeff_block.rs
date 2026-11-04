//! Complete coefficient-block decoding for one transform block.
//!
//! This composes the syntax pieces already pinned independently:
//! `all_zero`, EOB, the level loop, signs and the tile above/left contexts.
//! Prediction and inverse transform are intentionally outside this layer.

use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::coeff::{
    decode_eob_with_cdf, decode_levels_with_cdf, decode_signs_with_cdf, read_all_zero_with_cdf,
    seg_eob, TxClass,
};
use crate::reference::av1::context::TileContext;
use crate::reference::av1::partition::BlockSize;
use crate::reference::av1::scan::get_scan;
use crate::reference::av1::symbol::SymbolDecoder;
use crate::reference::av1::tx::TxSize;
use crate::reference::av1::tx_type::TxType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedCoeffs {
    pub all_zero: bool,
    pub eob: u32,
    pub quant: Vec<i32>,
    pub tx_type: TxType,
    pub dc_category: i32,
    pub cul_level: u32,
}

/// Decode one coefficient block and update the tile's coefficient contexts.
#[allow(clippy::too_many_arguments)]
pub fn decode_coefficients(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    tile: &mut TileContext,
    plane: usize,
    x4: usize,
    y4: usize,
    max_x4: usize,
    max_y4: usize,
    residual_size: BlockSize,
    tx: TxSize,
    tx_type: TxType,
    q_ctx: usize,
    disable_cdf_update: bool,
) -> Result<DecodedCoeffs, CdfStateError> {
    let txb_ctx = tile.txb_skip_context(plane, x4, y4, tx, residual_size, max_x4, max_y4);
    let all_zero = read_all_zero_with_cdf(decoder, cdfs, tx, q_ctx, txb_ctx, disable_cdf_update)?;
    let w4 = (tx.width() / 4) as usize;
    let h4 = (tx.height() / 4) as usize;
    if all_zero {
        tile.update(plane, x4, y4, w4, h4, 0, 0);
        return Ok(DecodedCoeffs {
            all_zero,
            eob: 0,
            quant: vec![0; seg_eob(tx) as usize],
            tx_type,
            dc_category: 0,
            cul_level: 0,
        });
    }

    decode_coefficient_payload(
        decoder,
        cdfs,
        tile,
        plane,
        x4,
        y4,
        max_x4,
        max_y4,
        tx,
        tx_type,
        q_ctx,
        disable_cdf_update,
    )
}

/// Decode the EOB/level/sign payload after the caller has established that
/// `all_zero == 0` and selected the final transform type.
#[allow(clippy::too_many_arguments)]
pub fn decode_coefficient_payload(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    tile: &mut TileContext,
    plane: usize,
    x4: usize,
    y4: usize,
    max_x4: usize,
    max_y4: usize,
    tx: TxSize,
    tx_type: TxType,
    q_ctx: usize,
    disable_cdf_update: bool,
) -> Result<DecodedCoeffs, CdfStateError> {
    let w4 = (tx.width() / 4) as usize;
    let h4 = (tx.height() / 4) as usize;

    let class = tx_type.class();
    let scan = get_scan(tx, tx_type.scan_direction());
    let eob = decode_eob_with_cdf(
        decoder,
        cdfs,
        tx,
        q_ctx,
        usize::from(plane > 0),
        class == TxClass::TwoD,
        disable_cdf_update,
    )?;
    let mut quant = vec![0i32; seg_eob(tx) as usize];
    decode_levels_with_cdf(
        decoder,
        cdfs,
        tx,
        &mut quant,
        &scan.order,
        eob,
        q_ctx,
        usize::from(plane > 0),
        class,
        disable_cdf_update,
    )?;
    let dc_ctx = tile.dc_sign_context(plane, x4, y4, w4, h4, max_x4, max_y4);
    let (dc_category, cul_level) = decode_signs_with_cdf(
        decoder,
        cdfs,
        &mut quant,
        &scan.order,
        eob,
        q_ctx,
        usize::from(plane > 0),
        dc_ctx,
        disable_cdf_update,
    )?;
    tile.update(plane, x4, y4, w4, h4, cul_level as u8, dc_category as u8);
    Ok(DecodedCoeffs {
        all_zero: false,
        eob,
        quant,
        tx_type,
        dc_category,
        cul_level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::partition::BLOCK_SIZES;

    #[test]
    fn an_all_zero_block_updates_context_to_zero() {
        let mut decoder = SymbolDecoder::init(&[0xffu8; 16]).unwrap();
        let mut cdfs = CdfState::new();
        let mut tile = TileContext::new(8, 8);
        let out = decode_coefficients(
            &mut decoder,
            &mut cdfs,
            &mut tile,
            0,
            0,
            0,
            8,
            8,
            BLOCK_SIZES[0],
            TxSize::Tx4x4,
            TxType::DctDct,
            0,
            false,
        )
        .unwrap();
        assert!(out.all_zero);
        assert_eq!(out.eob, 0);
        assert_eq!(out.quant, [0; 16]);
        assert_eq!(tile.above_level[0][0], 0);
    }

    #[test]
    fn a_nonzero_block_decodes_eob_levels_and_sign_syntax() {
        let mut decoder = SymbolDecoder::init(&[0u8; 32]).unwrap();
        let mut cdfs = CdfState::new();
        let mut tile = TileContext::new(8, 8);
        let out = decode_coefficients(
            &mut decoder,
            &mut cdfs,
            &mut tile,
            0,
            0,
            0,
            8,
            8,
            BLOCK_SIZES[0],
            TxSize::Tx4x4,
            TxType::DctDct,
            0,
            false,
        )
        .unwrap();
        assert!(!out.all_zero);
        assert!(out.eob >= 1);
        assert_ne!(out.quant[0], 0);
        assert_ne!(tile.above_level[0][0], 0);
    }
}
