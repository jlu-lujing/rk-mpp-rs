//! AV1 chroma-from-luma alpha syntax (specification section 5.11.4).

use crate::reference::av1::cdf_defaults::{DEFAULT_CFL_ALPHA_CDF, DEFAULT_CFL_SIGN_CDF};
use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::symbol::SymbolDecoder;

pub const CFL_SIGN_ZERO: u8 = 0;
pub const CFL_SIGN_NEG: u8 = 1;
pub const CFL_SIGN_POS: u8 = 2;
pub const CFL_JOINT_SIGNS: usize = 8;
pub const CFL_ALPHABET_SIZE: usize = 16;
pub const CFL_ALPHABET_SIZE_LOG2: u8 = 4;
pub const CFL_ALPHA_CONTEXTS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CflAlphas {
    pub joint_sign: u8,
    pub alpha_u: i8,
    pub alpha_v: i8,
    pub index: u8,
}

pub fn sign_u(joint_sign: u8) -> u8 {
    (u16::from(joint_sign) + 1).div_euclid(3) as u8
}

pub fn sign_v(joint_sign: u8) -> u8 {
    ((u16::from(joint_sign) + 1) % 3) as u8
}

pub fn context_u(joint_sign: u8) -> usize {
    usize::from(joint_sign + 1 - 3)
}

pub fn context_v(joint_sign: u8) -> usize {
    usize::from(sign_v(joint_sign) * 3 + sign_u(joint_sign) - 3)
}

fn read_alpha(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    context: usize,
    sign: u8,
    disable_cdf_update: bool,
) -> Result<(i8, u8), CdfStateError> {
    let symbol = cdfs.read_symbol(
        decoder,
        &DEFAULT_CFL_ALPHA_CDF,
        context * (CFL_ALPHABET_SIZE + 1),
        CFL_ALPHABET_SIZE + 1,
        disable_cdf_update,
    )? as u8;
    let magnitude = (symbol + 1) as i8;
    Ok((
        if sign == CFL_SIGN_NEG {
            -magnitude
        } else {
            magnitude
        },
        symbol,
    ))
}

/// `read_cfl_alphas()`.
pub fn read_cfl_alphas(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    disable_cdf_update: bool,
) -> Result<CflAlphas, CdfStateError> {
    let joint_sign = cdfs.read_symbol(
        decoder,
        &DEFAULT_CFL_SIGN_CDF,
        0,
        CFL_JOINT_SIGNS + 1,
        disable_cdf_update,
    )? as u8;
    let su = sign_u(joint_sign);
    let sv = sign_v(joint_sign);

    let mut alpha_u = 0;
    let mut alpha_v = 0;
    let mut index_u = 0;
    let mut index_v = 0;
    if su != CFL_SIGN_ZERO {
        let (value, symbol) =
            read_alpha(decoder, cdfs, context_u(joint_sign), su, disable_cdf_update)?;
        alpha_u = value;
        index_u = symbol;
    }
    if sv != CFL_SIGN_ZERO {
        let (value, symbol) =
            read_alpha(decoder, cdfs, context_v(joint_sign), sv, disable_cdf_update)?;
        alpha_v = value;
        index_v = symbol;
    }
    Ok(CflAlphas {
        joint_sign,
        alpha_u,
        alpha_v,
        index: (index_u << CFL_ALPHABET_SIZE_LOG2) | index_v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joint_sign_mapping_matches_the_specification_table() {
        assert_eq!(
            (0..=7)
                .map(|sign| (sign_u(sign), sign_v(sign)))
                .collect::<Vec<_>>(),
            [
                (0, 1),
                (0, 2),
                (1, 0),
                (1, 1),
                (1, 2),
                (2, 0),
                (2, 1),
                (2, 2),
            ]
        );
    }

    #[test]
    fn zero_stream_selects_the_zero_alpha() {
        let mut decoder = SymbolDecoder::init(&[0u8; 32]).unwrap();
        let mut cdfs = CdfState::new();
        let alphas = read_cfl_alphas(&mut decoder, &mut cdfs, false).unwrap();
        assert_eq!(
            alphas,
            CflAlphas {
                joint_sign: 0,
                alpha_u: 0,
                alpha_v: -1,
                index: 0,
            }
        );
    }

    #[test]
    fn alpha_contexts_and_index_pack_both_planes() {
        for joint_sign in 0..=7 {
            let su = sign_u(joint_sign);
            let sv = sign_v(joint_sign);
            if su != CFL_SIGN_ZERO {
                assert!(context_u(joint_sign) < CFL_ALPHA_CONTEXTS);
            }
            if sv != CFL_SIGN_ZERO {
                assert!(context_v(joint_sign) < CFL_ALPHA_CONTEXTS);
            }
        }
        assert_eq!(context_u(2), 0);
        assert_eq!(context_u(7), 5);
        assert_eq!(context_v(1), 3);
        assert_eq!(context_v(7), 5);
    }
}
