//! AV1 symbol decoder: specification section 8.2.
//!
//! `init_symbol` seeds a 15-bit `SymbolValue` with the *complement* of the
//! first bits, `read_symbol` splits `SymbolRange` through a 15-bit CDF and
//! renormalises from the stream, and `exit_symbol` consumes the trailing bits.
//! The constants are the specification's: `EC_PROB_SHIFT = 6`,
//! `EC_MIN_PROB = 4`.
//!
//! The decoder is byte-position aware (`get_position` in the specification):
//! `init_symbol` and every renormalisation read through the same forward bit
//! cursor, so `exit_symbol` can check the trailing-one and padding rules
//! exactly as written.

/// `EC_PROB_SHIFT`.
pub const EC_PROB_SHIFT: u32 = 6;
/// `EC_MIN_PROB`.
pub const EC_MIN_PROB: u32 = 4;
/// `1 << 15`, the CDF scale and the initial range.
pub const CDF_SCALE: u32 = 1 << 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolError {
    /// `init_symbol` needs at least one byte.
    Empty,
    /// The renormalisation asked for bits the partition does not have.
    ReadPastEnd,
    /// `exit_symbol` found a padding bit that is not zero.
    PaddingNotZero { bit: usize },
    /// `exit_symbol` found the trailing-one marker clear.
    MissingTrailingOne { bit: usize },
    /// `SymbolMaxBits` below the specification's -14 bound at exit.
    TooManyBits,
    /// A decoded coefficient count exceeds what the transform can hold.
    EobOutOfRange { eob: u32, limit: u32 },
}

/// The specification's symbol decoder over one tile data partition.
pub struct SymbolDecoder<'a> {
    data: &'a [u8],
    /// `get_position()`: bits consumed from `data`, MSB first.
    position: usize,
    /// `SymbolValue`.
    value: u32,
    /// `SymbolRange`.
    range: u32,
    /// `SymbolMaxBits`; negative means padding zero bits are in use.
    max_bits: i32,
}

impl<'a> SymbolDecoder<'a> {
    /// `init_symbol(sz)`.
    pub fn init(data: &'a [u8]) -> Result<Self, SymbolError> {
        if data.is_empty() {
            return Err(SymbolError::Empty);
        }
        let mut decoder = Self {
            data,
            position: 0,
            value: 0,
            range: CDF_SCALE,
            max_bits: 0,
        };
        let num_bits = (data.len() * 8).min(15) as u32;
        let buf = decoder.read_bits(num_bits)?;
        let padded = buf << (15 - num_bits);
        decoder.value = (CDF_SCALE - 1) ^ padded;
        decoder.max_bits = data.len() as i32 * 8 - 15;
        Ok(decoder)
    }

    /// `get_position()`.
    pub fn position(&self) -> usize {
        self.position
    }

    /// Current arithmetic state for diagnostics and cross-decoder comparisons.
    pub fn debug_state(&self) -> (u32, u32, i32) {
        (self.value, self.range, self.max_bits)
    }

    /// `f(n)`: `n` bits, most significant first.
    fn read_bits(&mut self, n: u32) -> Result<u32, SymbolError> {
        let mut out = 0u32;
        for _ in 0..n {
            if self.position >= self.data.len() * 8 {
                return Err(SymbolError::ReadPastEnd);
            }
            let byte = self.data[self.position / 8];
            let bit = (byte >> (7 - (self.position % 8))) & 1;
            out = (out << 1) | u32::from(bit);
            self.position += 1;
        }
        Ok(out)
    }

    /// Section 8.2.4: decode one `read_symbol(cdf)` symbol.
    ///
    /// `cdf` has `N + 1` entries: `N` cumulative probabilities in `1 << 15`
    /// units followed by the adaptation counter, exactly the specification's
    /// array. It is updated in place unless `disable_cdf_update`.
    pub fn read_symbol(&mut self, cdf: &mut [u16]) -> Result<usize, SymbolError> {
        self.read_symbol_with(cdf, false)
    }

    /// `read_symbol` with the syntax structure's `disable_cdf_update`.
    pub fn read_symbol_with(
        &mut self,
        cdf: &mut [u16],
        disable_cdf_update: bool,
    ) -> Result<usize, SymbolError> {
        let n = cdf.len() - 1;
        debug_assert!(n > 1);
        let mut cur = self.range;
        let mut symbol = 0usize;
        let mut prev;
        loop {
            prev = cur;
            let f = CDF_SCALE - u32::from(cdf[symbol]);
            cur = ((self.range >> 8) * (f >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT);
            cur += EC_MIN_PROB * (n - symbol - 1) as u32;
            // The specification's do/while: stop once `SymbolValue >= cur`.
            if self.value >= cur {
                break;
            }
            symbol += 1;
            if symbol >= n {
                return Err(SymbolError::ReadPastEnd);
            }
        }

        self.range = prev - cur;
        self.value -= cur;

        let bits = 15 - floor_log2(self.range);
        self.range <<= bits;
        let num_bits = bits.min(self.max_bits.max(0) as u32);
        let new_data = self.read_bits(num_bits)?;
        let padded = new_data << (bits - num_bits);
        self.value = padded ^ (((self.value + 1) << bits) - 1);
        self.max_bits -= bits as i32;

        if !disable_cdf_update {
            let count = cdf[n];
            let rate =
                3 + u32::from(count > 15) + u32::from(count > 31) + floor_log2(n as u32).min(2);
            let mut tmp = 0u32;
            for (i, entry) in cdf.iter_mut().take(n).enumerate() {
                if i == symbol {
                    tmp = CDF_SCALE;
                }
                let current = u32::from(*entry);
                let updated = if tmp < current {
                    current - ((current - tmp) >> rate)
                } else {
                    current + ((tmp - current) >> rate)
                };
                *entry = updated as u16;
            }
            cdf[n] += u16::from(cdf[n] < 32);
        }
        Ok(symbol)
    }

    /// `read_literal(n)`: `n` pseudo-raw bits through [`Self::read_bool`].
    pub fn read_literal(&mut self, n: u32) -> Result<u32, SymbolError> {
        let mut x = 0u32;
        for _ in 0..n {
            x = (x << 1) | self.read_bool()?;
        }
        Ok(x)
    }

    /// `read_bool()`: a `read_symbol` with cdf `[1 << 14, 1 << 15, 0]`.
    /// The specification notes the update can be skipped because the array is
    /// rebuilt for every call.
    pub fn read_bool(&mut self) -> Result<u32, SymbolError> {
        let mut cdf = [1u16 << 14, 1u16 << 15, 0];
        Ok(self.read_symbol_with(&mut cdf, true)? as u32)
    }

    /// `exit_symbol()`: skip the unread trailing bits and check them.
    ///
    /// A conforming partition has a one at `trailingBitPosition` and zero bits
    /// up to the byte-aligned end.
    pub fn exit_symbol(&self) -> Result<(), SymbolError> {
        if self.max_bits < -14 {
            return Err(SymbolError::TooManyBits);
        }
        let trailing_back = (self.max_bits + 15).min(15) as usize;
        let trailing = self.position - trailing_back;
        let mut end = self.position + self.max_bits.max(0) as usize;
        end = (end + 7) / 8 * 8;
        if trailing >= end || self.bit_at(trailing) != Some(1) {
            return Err(SymbolError::MissingTrailingOne { bit: trailing });
        }
        for bit in trailing + 1..end {
            if self.bit_at(bit) == Some(1) {
                return Err(SymbolError::PaddingNotZero { bit });
            }
        }
        Ok(())
    }

    fn bit_at(&self, bit: usize) -> Option<u32> {
        if bit >= self.data.len() * 8 {
            return None;
        }
        Some(u32::from((self.data[bit / 8] >> (7 - bit % 8)) & 1))
    }
}

/// `FloorLog2`.
pub fn floor_log2(value: u32) -> u32 {
    31 - value.max(1).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_complements_the_first_fifteen_bits() {
        // Section 8.2.2: numBits = Min(sz * 8, 15),
        // SymbolValue = ((1 << 15) - 1) ^ (buf << (15 - numBits)).
        let decoder = SymbolDecoder::init(&[0x00, 0x00]).unwrap();
        assert_eq!(decoder.value, 0x7fff);
        assert_eq!(decoder.range, 1 << 15);
        assert_eq!(decoder.max_bits, 16 - 15);

        let decoder = SymbolDecoder::init(&[0xff, 0xff]).unwrap();
        assert_eq!(decoder.value, 0);
        assert_eq!(decoder.position, 15);
    }

    #[test]
    fn one_byte_initialisation_uses_all_eight_bits() {
        let decoder = SymbolDecoder::init(&[0b1000_0000]).unwrap();
        // buf = 0x80, padded = 0x80 << 7 = 0x4000, value = 0x7fff ^ 0x4000.
        assert_eq!(decoder.value, 0x3fff);
        assert_eq!(decoder.max_bits, 8 - 15);
        assert_eq!(decoder.position, 8);
    }

    #[test]
    fn uniform_cdf_halves_the_range_and_adapts_the_counter() {
        let mut decoder = SymbolDecoder::init(&[0x00, 0x00, 0x00, 0x00]).unwrap();
        let mut cdf = [1u16 << 14, 1u16 << 15, 0];
        let symbol = decoder.read_symbol(&mut cdf).unwrap();
        assert_eq!(symbol, 0, "value starts below the first split");
        // rate = 3 + 0 + 0 + min(2, floorLog2(3)) = 4;
        // cdf[0] = 0x4000 + ((0x8000 - 0x4000) >> 4) = 0x4400.
        assert_eq!(cdf, [0x4400, 0x8000, 1]);
    }

    /// Four 4-bit literals per buffer, computed by running the specification's
    /// pseudocode independently (init_symbol + read_bool + renormalisation) on
    /// the same bytes.
    fn literals(data: &[u8]) -> [u32; 4] {
        let mut decoder = SymbolDecoder::init(data).unwrap();
        std::array::from_fn(|_| decoder.read_literal(4).unwrap())
    }

    #[test]
    fn literal_bits_are_pseudo_raw_flags() {
        assert_eq!(literals(&[0; 8]), [0, 0, 0, 0]);
        assert_eq!(literals(&[0xff; 8]), [15, 15, 15, 15]);
        assert_eq!(
            literals(&[0xab, 0xcd, 0xab, 0xcd, 0xab, 0xcd, 0xab, 0xcd]),
            [10, 11, 13, 2]
        );
        assert_eq!(literals(&[1, 2, 3, 4, 5, 6, 7, 8]), [0, 0, 15, 12]);
    }

    #[test]
    fn exit_symbol_checks_the_trailing_one() {
        // Two bytes leave SymbolMaxBits = 1, so the specification's
        // trailingBitPosition is position(15) - Min(15, 16) = 0 and the padding
        // region ends at 16: bit 0 must be one and bits 1..15 zero.
        let decoder = SymbolDecoder::init(&[0x80, 0x00]).unwrap();
        assert_eq!(decoder.position(), 15);
        assert_eq!(decoder.exit_symbol(), Ok(()));

        let decoder = SymbolDecoder::init(&[0x00, 0x00]).unwrap();
        assert_eq!(
            decoder.exit_symbol(),
            Err(SymbolError::MissingTrailingOne { bit: 0 })
        );
    }

    #[test]
    fn exit_symbol_uses_the_remaining_max_bits_for_the_trailing_one() {
        let decoder = SymbolDecoder {
            data: &[0xff, 0xff, 0x04],
            position: 24,
            value: 0,
            range: 1 << 15,
            max_bits: -12,
        };
        assert_eq!(decoder.exit_symbol(), Ok(()));
    }

    #[test]
    fn exit_symbol_rejects_non_zero_padding() {
        let decoder = SymbolDecoder::init(&[0xc0, 0x00]).unwrap();
        assert_eq!(
            decoder.exit_symbol(),
            Err(SymbolError::PaddingNotZero { bit: 1 })
        );
    }

    #[test]
    fn floor_log2_matches_the_specification_table() {
        assert_eq!(floor_log2(0), 0);
        assert_eq!(floor_log2(1), 0);
        assert_eq!(floor_log2(2), 1);
        assert_eq!(floor_log2(3), 1);
        assert_eq!(floor_log2(1 << 15), 15);
    }
}
