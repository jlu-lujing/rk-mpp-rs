//! AV1 arithmetic-coded subexponential values, specification section 8.3 and
//! `read_lr_unit()` in section 7.17.
//!
//! Loop restoration codes the Wiener taps and the SGRPROJ xqd values with
//! `decode_signed_subexp_with_ref_bool`, the arithmetic-coded twin of
//! `decode_subexp`: `f(n)` becomes `L(n)` (`read_literal`) and `ns(n)` becomes
//! `NS(n)` (`read_literal` plus one `read_bool`). The non-bool variants are not
//! needed by restoration but are kept beside them so the two can be compared.

use crate::reference::av1::symbol::{floor_log2, SymbolDecoder, SymbolError};

/// `inverse_recenter( r, v )` from section 8.3.
pub fn inverse_recenter(r: i32, v: i32) -> i32 {
    if v > 2 * r {
        v
    } else if v & 1 != 0 {
        r - ((v + 1) >> 1)
    } else {
        r + (v >> 1)
    }
}

/// The `NS(n)` mapping after its `L(w-1)` part `v` and optional `L(1)` bit.
/// Kept separate so the worked example in the specification can be pinned
/// without constructing an arithmetic bitstream for each case.
///
/// ```text
/// w = FloorLog2(n) + 1
/// m = (1 << w) - n
/// if ( v < m ) return v
/// return (v << 1) - m + extra_bit
/// ```
pub fn ns_value(n: u32, v: u32, extra_bit: u32) -> u32 {
    debug_assert!(n > 0);
    if n <= 1 {
        return 0;
    }
    let w = floor_log2(n) + 1;
    let m = (1u32 << w) - n;
    if v < m {
        v
    } else {
        (v << 1) - m + extra_bit
    }
}

/// `NS(n)`: the `ns(n)` descriptor with every bit read arithmetically.
pub fn read_ns(decoder: &mut SymbolDecoder<'_>, n: u32) -> Result<u32, SymbolError> {
    debug_assert!(n > 0);
    if n <= 1 {
        return Ok(0);
    }
    let w = floor_log2(n) + 1;
    let m = (1u32 << w) - n;
    let v = decoder.read_literal(w - 1)?;
    if v < m {
        Ok(v)
    } else {
        Ok(ns_value(n, v, decoder.read_bool()?))
    }
}

/// `decode_subexp_bool( numSyms, k )`: the arithmetic-coded subexponential
/// Golomb code used by the loop restoration unit syntax.
pub fn decode_subexp_bool(
    decoder: &mut SymbolDecoder<'_>,
    num_syms: u32,
    k: u32,
) -> Result<u32, SymbolError> {
    let mut i = 0u32;
    let mut mk = 0u32;
    loop {
        let b2 = if i != 0 { k + i - 1 } else { k };
        let a = 1u32 << b2;
        if num_syms <= mk + 3 * a {
            let subexp_unif_bools = read_ns(decoder, num_syms - mk)?;
            return Ok(subexp_unif_bools + mk);
        }
        let subexp_more_bools = decoder.read_bool()?;
        if subexp_more_bools != 0 {
            i += 1;
            mk += a;
        } else {
            let subexp_bools = decoder.read_literal(b2)?;
            return Ok(subexp_bools + mk);
        }
    }
}

/// `decode_unsigned_subexp_with_ref_bool( mx, k, r )`.
pub fn decode_unsigned_subexp_with_ref_bool(
    decoder: &mut SymbolDecoder<'_>,
    mx: u32,
    k: u32,
    r: i32,
) -> Result<i32, SymbolError> {
    let v = decode_subexp_bool(decoder, mx, k)? as i32;
    let mx = mx as i32;
    if (r << 1) <= mx {
        Ok(inverse_recenter(r, v))
    } else {
        Ok(mx - 1 - inverse_recenter(mx - 1 - r, v))
    }
}

/// `decode_signed_subexp_with_ref_bool( low, high, k, r )`, returning a value
/// in `low..high`.
pub fn decode_signed_subexp_with_ref_bool(
    decoder: &mut SymbolDecoder<'_>,
    low: i32,
    high: i32,
    k: u32,
    r: i32,
) -> Result<i32, SymbolError> {
    let mx = (high - low) as u32;
    let x = decode_unsigned_subexp_with_ref_bool(decoder, mx, k, r - low)?;
    Ok(x + low)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::restoration::{
        WIENER_TAPS_K, WIENER_TAPS_MAX, WIENER_TAPS_MID, WIENER_TAPS_MIN,
    };

    /// All-zero bytes make every `read_bool` a zero; all-ones make it a one.
    const ZEROS: [u8; 64] = [0u8; 64];
    const ONES: [u8; 64] = [0xffu8; 64];

    #[test]
    fn inverse_recenter_matches_the_specification_branches() {
        // v > 2 * r is returned unchanged.
        assert_eq!(inverse_recenter(3, 7), 7);
        // Even values walk upwards from r, odd values downwards.
        assert_eq!(inverse_recenter(8, 0), 8);
        assert_eq!(inverse_recenter(8, 2), 9);
        assert_eq!(inverse_recenter(8, 15), 0);
        assert_eq!(inverse_recenter(16, 0), 16);
        assert_eq!(inverse_recenter(0, 0), 0);
    }

    #[test]
    fn ns_spans_the_whole_alphabet_for_zero_and_one_streams() {
        for n in 2..=33u32 {
            let mut dec = SymbolDecoder::init(&ZEROS).unwrap();
            assert_eq!(read_ns(&mut dec, n).unwrap(), 0, "n={n}");
            let mut dec = SymbolDecoder::init(&ONES).unwrap();
            assert_eq!(read_ns(&mut dec, n).unwrap(), n - 1, "n={n}");
        }
    }

    #[test]
    fn ns_uses_one_fewer_bit_for_the_lower_part() {
        // The specification's worked example for n = 5: w = 3, m = 3, so the
        // two-bit prefixes 0..2 pass through while 3 needs the third bit.
        assert_eq!(ns_value(5, 0, 0), 0);
        assert_eq!(ns_value(5, 1, 0), 1);
        assert_eq!(ns_value(5, 2, 0), 2);
        assert_eq!(ns_value(5, 3, 0), 3);
        assert_eq!(ns_value(5, 3, 1), 4);
        // A power of two has m = 2^(w-1), so the extra bit is never read.
        assert_eq!(ns_value(8, 0, 0), 0);
        assert_eq!(ns_value(8, 7, 0), 7);
        assert_eq!(ns_value(16, 15, 0), 15);
    }

    #[test]
    fn subexp_bool_spans_the_whole_alphabet() {
        for (num_syms, k) in [(16u32, 1u32), (32, 2), (96, 3), (128, 4)] {
            let mut dec = SymbolDecoder::init(&ZEROS).unwrap();
            assert_eq!(decode_subexp_bool(&mut dec, num_syms, k).unwrap(), 0);
            let mut dec = SymbolDecoder::init(&ONES).unwrap();
            assert_eq!(
                decode_subexp_bool(&mut dec, num_syms, k).unwrap(),
                num_syms - 1,
                "numSyms={num_syms} k={k}"
            );
        }
    }

    #[test]
    fn wiener_tap_zero_stream_decodes_the_reference_value() {
        // read_lr_unit uses low=Wiener_Taps_Min[0], high=Max[0]+1, k=K[0],
        // and the frame's RefLrWiener (initially Wiener_Taps_Mid[0]).
        let (low, high, k, r) = (
            WIENER_TAPS_MIN[0],
            WIENER_TAPS_MAX[0] + 1,
            WIENER_TAPS_K[0],
            WIENER_TAPS_MID[0],
        );
        let mut dec = SymbolDecoder::init(&ZEROS).unwrap();
        assert_eq!(
            decode_signed_subexp_with_ref_bool(&mut dec, low, high, k, r).unwrap(),
            r
        );
        let mut dec = SymbolDecoder::init(&ONES).unwrap();
        assert_eq!(
            decode_signed_subexp_with_ref_bool(&mut dec, low, high, k, r).unwrap(),
            low
        );
    }

    #[test]
    fn wiener_taps_stay_inside_their_transmitted_range() {
        for j in 0..3 {
            let (low, high, k, r) = (
                WIENER_TAPS_MIN[j],
                WIENER_TAPS_MAX[j] + 1,
                WIENER_TAPS_K[j],
                WIENER_TAPS_MID[j],
            );
            for data in [
                ZEROS.to_vec(),
                ONES.to_vec(),
                vec![0x5au8; 64],
                vec![0xa5u8; 64],
            ] {
                let mut dec = SymbolDecoder::init(&data).unwrap();
                let value = decode_signed_subexp_with_ref_bool(&mut dec, low, high, k, r).unwrap();
                assert!(
                    (low..high).contains(&value),
                    "j={j} value={value} range={low}..{high}"
                );
            }
        }
    }

    #[test]
    fn sgrproj_xqd_stays_inside_its_transmitted_range() {
        use crate::reference::av1::restoration::{
            SGRPROJ_PRJ_SUBEXP_K, SGRPROJ_XQD_MAX, SGRPROJ_XQD_MID, SGRPROJ_XQD_MIN,
        };
        for i in 0..2 {
            for data in [ZEROS.to_vec(), ONES.to_vec(), vec![0x37u8; 64]] {
                let mut dec = SymbolDecoder::init(&data).unwrap();
                let value = decode_signed_subexp_with_ref_bool(
                    &mut dec,
                    SGRPROJ_XQD_MIN[i],
                    SGRPROJ_XQD_MAX[i] + 1,
                    SGRPROJ_PRJ_SUBEXP_K,
                    SGRPROJ_XQD_MID[i],
                )
                .unwrap();
                assert!(
                    (SGRPROJ_XQD_MIN[i]..SGRPROJ_XQD_MAX[i] + 1).contains(&value),
                    "i={i} value={value}"
                );
            }
        }
    }
}
