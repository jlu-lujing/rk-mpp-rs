//! AV1 block-level delta-q and delta-LF syntax.

use crate::reference::av1::cdf_defaults::{
    CdfTable, DEFAULT_DELTA_LF_CDF, DEFAULT_DELTA_LF_MULTI_CDF, DEFAULT_DELTA_Q_CDF,
};
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

/// Update the frame's delta-LF state. Non-multi mode carries one base level;
/// multi mode carries four independent levels whose final two are absent for
/// monochrome frames, matching libaom's `av1_num_planes` count.
#[allow(clippy::too_many_arguments)]
pub fn read_delta_lf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    current: [i32; 4],
    delta_lf_present: bool,
    multi: bool,
    read_deltas: bool,
    delta_lf_res: u8,
    skip_at_superblock_size: bool,
    disable_cdf_update: bool,
    has_chroma: bool,
) -> Result<[i32; 4], DeltaError> {
    if !delta_lf_present || !read_deltas || skip_at_superblock_size {
        return Ok(current);
    }
    let mut next = current;
    if multi {
        let count = if has_chroma { 4 } else { 2 };
        for (index, value) in next.iter_mut().enumerate().take(count) {
            let delta = read_delta_lf_symbol(
                decoder,
                cdfs,
                &DEFAULT_DELTA_LF_MULTI_CDF,
                index * (DELTA_LF_SMALL as usize + 2),
                disable_cdf_update,
            )?;
            *value =
                (current[index] + (delta << delta_lf_res)).clamp(-MAX_LOOP_FILTER, MAX_LOOP_FILTER);
        }
    } else {
        let delta =
            read_delta_lf_symbol(decoder, cdfs, &DEFAULT_DELTA_LF_CDF, 0, disable_cdf_update)?;
        next[0] = (current[0] + (delta << delta_lf_res)).clamp(-MAX_LOOP_FILTER, MAX_LOOP_FILTER);
    }
    Ok(next)
}

fn read_delta_lf_symbol(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    table: &CdfTable,
    start: usize,
    disable_cdf_update: bool,
) -> Result<i32, DeltaError> {
    let mut abs = cdfs.read_symbol(
        decoder,
        table,
        start,
        DELTA_LF_SMALL as usize + 2,
        disable_cdf_update,
    )? as u32;
    if abs == DELTA_LF_SMALL {
        let n = decoder.read_literal(3)? + 1;
        let bits = decoder.read_literal(n)?;
        abs = bits + (1 << n) + 1;
    }
    if abs == 0 {
        return Ok(0);
    }
    let sign = decoder.read_literal(1)?;
    Ok(if sign != 0 { -(abs as i32) } else { abs as i32 })
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
                [7; 4],
                true,
                false,
                true,
                0,
                true,
                false,
                true,
            )
            .unwrap(),
            [7; 4]
        );
    }

    #[test]
    fn multi_delta_lf_reads_one_independent_level_per_active_filter() {
        let input = [0x5au8; 32];
        let mut color_decoder = SymbolDecoder::init(&input).unwrap();
        let mut color_cdfs = CdfState::new();
        let color = read_delta_lf(
            &mut color_decoder,
            &mut color_cdfs,
            [2, -1, 3, -4],
            true,
            true,
            true,
            0,
            false,
            false,
            true,
        )
        .unwrap();
        assert!(color.iter().all(|level| level.abs() <= MAX_LOOP_FILTER));

        let mut mono_decoder = SymbolDecoder::init(&input).unwrap();
        let mut mono_cdfs = CdfState::new();
        let mono = read_delta_lf(
            &mut mono_decoder,
            &mut mono_cdfs,
            [2, -1, 3, -4],
            true,
            true,
            true,
            0,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(&mono[2..], &[3, -4], "monochrome keeps chroma slots idle");
        assert_ne!(
            color_cdfs, mono_cdfs,
            "color multi-LF adapts all four independent rows"
        );
    }
}
