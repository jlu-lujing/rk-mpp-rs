//! AV1 block-level delta-q and delta-LF syntax.

use crate::reference::av1::cdf_defaults::{DEFAULT_DELTA_LF_CDF, DEFAULT_DELTA_Q_CDF};
use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::symbol::SymbolDecoder;
use crate::reference::av1::symbol::SymbolError;

pub const DELTA_Q_SMALL: u32 = 3;
pub const DELTA_LF_SMALL: u32 = 3;
pub const MAX_LOOP_FILTER: i32 = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaError {
    Cdf(CdfStateError),
    Symbol(SymbolError),
    MultiDeltaLfUnsupported,
}

impl From<CdfStateError> for DeltaError {
    fn from(error: CdfStateError) -> Self {
        Self::Cdf(error)
    }
}

impl From<SymbolError> for DeltaError {
    fn from(error: SymbolError) -> Self {
        Self::Symbol(error)
    }
}

/// Update `CurrentQIndex` after `read_delta_qindex()`. `skip_at_superblock_size`
/// is the syntax's early return condition.
#[allow(clippy::too_many_arguments)]
pub fn read_delta_qindex(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    current_qindex: u8,
    delta_q_present: bool,
    read_deltas: bool,
    delta_q_res: u8,
    skip_at_superblock_size: bool,
    disable_cdf_update: bool,
) -> Result<u8, DeltaError> {
    if !delta_q_present || !read_deltas || skip_at_superblock_size {
        return Ok(current_qindex);
    }
    let mut abs = cdfs.read_symbol(decoder, &DEFAULT_DELTA_Q_CDF, 0, 5, disable_cdf_update)? as u32;
    if abs == DELTA_Q_SMALL {
        let rem = decoder.read_literal(3)? + 1;
        let bits = decoder.read_literal(rem)?;
        abs = bits + (1 << rem) + 1;
    }
    if abs == 0 {
        return Ok(current_qindex);
    }
    let sign = decoder.read_literal(1)?;
    let reduced = if sign != 0 { -(abs as i32) } else { abs as i32 };
    Ok((i32::from(current_qindex) + (reduced << delta_q_res)).clamp(1, 255) as u8)
}

/// Update one `DeltaLF` value. The generated defaults contain the single
/// `TileDeltaLFCdf`; multi-LF uses different tables and is rejected until those
/// are represented rather than decoding against the wrong distribution.
#[allow(clippy::too_many_arguments)]
pub fn read_delta_lf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    delta_lf: i32,
    delta_lf_present: bool,
    multi: bool,
    read_deltas: bool,
    delta_lf_res: u8,
    skip_at_superblock_size: bool,
    disable_cdf_update: bool,
) -> Result<i32, DeltaError> {
    if !delta_lf_present || !read_deltas || skip_at_superblock_size {
        return Ok(delta_lf);
    }
    if multi {
        return Err(DeltaError::MultiDeltaLfUnsupported);
    }
    let mut abs =
        cdfs.read_symbol(decoder, &DEFAULT_DELTA_LF_CDF, 0, 5, disable_cdf_update)? as u32;
    if abs == DELTA_LF_SMALL {
        let n = decoder.read_literal(3)? + 1;
        let bits = decoder.read_literal(n)?;
        abs = bits + (1 << n) + 1;
    }
    if abs == 0 {
        return Ok(delta_lf);
    }
    let sign = decoder.read_literal(1)?;
    let reduced = if sign != 0 { -(abs as i32) } else { abs as i32 };
    Ok((delta_lf + (reduced << delta_lf_res)).clamp(-MAX_LOOP_FILTER, MAX_LOOP_FILTER))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_delta_symbol_keeps_the_current_qindex() {
        let mut decoder = SymbolDecoder::init(&[0u8; 16]).unwrap();
        let mut cdfs = CdfState::new();
        assert_eq!(
            read_delta_qindex(&mut decoder, &mut cdfs, 96, true, true, 2, false, false,).unwrap(),
            96
        );
    }

    #[test]
    fn disabled_or_early_return_deltas_consume_nothing() {
        let mut decoder = SymbolDecoder::init(&[0xffu8; 16]).unwrap();
        let mut cdfs = CdfState::new();
        assert_eq!(
            read_delta_qindex(&mut decoder, &mut cdfs, 80, false, true, 0, false, false,).unwrap(),
            80
        );
        assert_eq!(
            read_delta_lf(
                &mut decoder,
                &mut cdfs,
                7,
                true,
                false,
                true,
                0,
                true,
                false,
            )
            .unwrap(),
            7
        );
    }

    #[test]
    fn multi_delta_lf_is_rejected() {
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        let mut cdfs = CdfState::new();
        assert_eq!(
            read_delta_lf(
                &mut decoder,
                &mut cdfs,
                0,
                true,
                true,
                true,
                0,
                false,
                false,
            ),
            Err(DeltaError::MultiDeltaLfUnsupported)
        );
    }
}
