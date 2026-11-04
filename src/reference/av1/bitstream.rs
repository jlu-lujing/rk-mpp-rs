//! AV1 bitstream reader and the §8.1 syntax descriptors.
//!
//! Everything in this module reads bits **MSB first**, matching `f(n)` in the
//! specification. The reader is deliberately byte-slice based (no allocation)
//! so an OBU payload can be parsed in place. `leb128()`, `le(n)` and
//! `byte_alignment()` only make sense at byte boundaries and enforce that.
//!
//! Reference: AV1 specification §8.1 (General syntax elements and descriptors),
//! §5.2 (OBU syntax) and §7.15 (trailing bits).

/// Why a descriptor read failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitstreamError {
    /// `f(n)`/`le(n)`/`uvlc()` asked for more bits than remain in the slice.
    OutOfBits { requested: usize, available: usize },
    /// `f(n)` and `su(n)` are defined for at most 32 bits here.
    TooManyBits(usize),
    /// `le(n)` is defined for at most 4 bytes (32 bits).
    TooManyBytes(usize),
    /// `leb128()` did not see a terminating byte within 8 bytes.
    Leb128TooLong,
    /// `leb128()` produced a value above `(1 << 32) - 1`, which the
    /// specification forbids by bitstream conformance.
    Leb128Overflow(u64),
    /// A byte-aligned descriptor was invoked mid-byte.
    NotByteAligned { position: usize },
    /// `byte_alignment()` found a non-zero padding bit.
    NonZeroAlignmentBit { position: usize },
    /// `trailing_bits()` did not start with a one.
    MissingTrailingOneBit,
    /// `trailing_bits()` found a non-zero trailing bit.
    NonZeroTrailingBit { index: usize },
    /// `su(n)`/`ns(n)` need `n >= 1`.
    InvalidDescriptorRange(u32),
}

impl core::fmt::Display for BitstreamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A cursor over a byte slice, counting positions in **bits** so that
/// `get_position()` in the specification maps directly onto [`BitReader::position`].
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    /// Bit offset from the start of the slice (`get_position()`).
    pub const fn position(&self) -> usize {
        self.bit_pos
    }

    pub const fn bits_remaining(&self) -> usize {
        self.data.len() * 8 - self.bit_pos
    }

    pub const fn is_byte_aligned(&self) -> bool {
        self.bit_pos & 7 == 0
    }

    /// The unread tail of the slice, only valid at a byte boundary.
    pub fn remaining_bytes(&self) -> Result<&'a [u8], BitstreamError> {
        if !self.is_byte_aligned() {
            return Err(BitstreamError::NotByteAligned {
                position: self.bit_pos,
            });
        }
        Ok(&self.data[self.bit_pos / 8..])
    }

    /// `f(1)`.
    pub fn read_bit(&mut self) -> Result<u32, BitstreamError> {
        self.read_f(1)
    }

    /// `f(n)` with `n <= 32`, most significant bit first.
    pub fn read_f(&mut self, n: usize) -> Result<u32, BitstreamError> {
        if n > 32 {
            return Err(BitstreamError::TooManyBits(n));
        }
        if n > self.bits_remaining() {
            return Err(BitstreamError::OutOfBits {
                requested: n,
                available: self.bits_remaining(),
            });
        }
        let mut value = 0u32;
        for _ in 0..n {
            let byte = self.data[self.bit_pos / 8];
            let shift = 7 - (self.bit_pos & 7);
            value = (value << 1) | ((byte >> shift) & 1) as u32;
            self.bit_pos += 1;
        }
        Ok(value)
    }

    /// Advance without reading. Useful when a second pass has to resume at a
    /// syntax position recorded by a previous reader (e.g. trailing bits).
    pub fn skip_bits(&mut self, n: usize) -> Result<(), BitstreamError> {
        if n > self.bits_remaining() {
            return Err(BitstreamError::OutOfBits {
                requested: n,
                available: self.bits_remaining(),
            });
        }
        self.bit_pos += n;
        Ok(())
    }

    /// `le(n)`: little-endian `n`-byte integer, `n <= 4`.
    pub fn read_le(&mut self, n: usize) -> Result<u32, BitstreamError> {
        if n > 4 {
            return Err(BitstreamError::TooManyBytes(n));
        }
        if !self.is_byte_aligned() {
            return Err(BitstreamError::NotByteAligned {
                position: self.bit_pos,
            });
        }
        let mut value = 0u32;
        for i in 0..n {
            let byte = self.read_f(8)?;
            value |= byte << (i * 8);
        }
        Ok(value)
    }

    /// `uvlc()`: variable length unsigned number.
    ///
    /// ```text
    /// leadingZeros = 0
    /// while (1) { done = f(1); if (done) break; leadingZeros++ }
    /// if (leadingZeros >= 32) return (1 << 32) - 1
    /// value = f(leadingZeros)
    /// return value + (1 << leadingZeros) - 1
    /// ```
    pub fn read_uvlc(&mut self) -> Result<u32, BitstreamError> {
        let mut leading_zeros = 0usize;
        loop {
            if self.read_bit()? != 0 {
                break;
            }
            leading_zeros += 1;
        }
        // The terminating one is consumed even in the saturating case, so the
        // reader stays at the position the specification leaves it at.
        if leading_zeros >= 32 {
            return Ok(u32::MAX);
        }
        let value = if leading_zeros == 0 {
            0
        } else {
            self.read_f(leading_zeros)?
        };
        // leading_zeros <= 31, so the sum is at most 2^32 - 2 and fits u32.
        Ok((value as u64 + (1u64 << leading_zeros) - 1) as u32)
    }

    /// `leb128()`: returns `(value, Leb128Bytes)`, the latter being the number
    /// of bytes consumed (needed by `open_bitstream_unit(sz)` bookkeeping).
    pub fn read_leb128(&mut self) -> Result<(u32, usize), BitstreamError> {
        if !self.is_byte_aligned() {
            return Err(BitstreamError::NotByteAligned {
                position: self.bit_pos,
            });
        }
        let mut value = 0u64;
        for i in 0..8 {
            let byte = self.read_f(8)? as u8;
            value |= ((byte & 0x7f) as u64) << (i * 7);
            if byte & 0x80 == 0 {
                if value > u32::MAX as u64 {
                    return Err(BitstreamError::Leb128Overflow(value));
                }
                return Ok((value as u32, i + 1));
            }
        }
        Err(BitstreamError::Leb128TooLong)
    }

    /// `su(n)`: two's-complement value stored in the bottom `n` bits.
    pub fn read_su(&mut self, n: usize) -> Result<i32, BitstreamError> {
        if n == 0 {
            return Err(BitstreamError::InvalidDescriptorRange(0));
        }
        if n > 32 {
            return Err(BitstreamError::TooManyBits(n));
        }
        let value = self.read_f(n)? as i64;
        let sign_mask = 1i64 << (n - 1);
        Ok(if value & sign_mask != 0 {
            (value - 2 * sign_mask) as i32
        } else {
            value as i32
        })
    }

    /// `ns(n)`: non-symmetric unsigned value in `0..n-1`, bitstream form.
    /// This is the `f()`/`read_bit()` variant; the arithmetic-coded `NS(n)`
    /// variant lives in [`crate::reference::av1::subexp`].
    pub fn read_ns(&mut self, n: u32) -> Result<u32, BitstreamError> {
        if n == 0 {
            return Err(BitstreamError::InvalidDescriptorRange(n));
        }
        if n == 1 {
            return Ok(0);
        }
        let w = 32 - n.leading_zeros();
        let m = (1u32 << w) - n;
        let v = self.read_f((w - 1) as usize)?;
        if v < m {
            Ok(v)
        } else {
            let extra = self.read_bit()?;
            Ok((v << 1) - m + extra)
        }
    }

    /// `byte_alignment()`: consume padding bits and verify they are zero.
    pub fn align_to_byte(&mut self) -> Result<(), BitstreamError> {
        while !self.is_byte_aligned() {
            let position = self.bit_pos;
            if self.read_bit()? != 0 {
                return Err(BitstreamError::NonZeroAlignmentBit { position });
            }
        }
        Ok(())
    }

    /// `trailing_bits(nbBits)`: one bit equal to 1 followed by zero bits.
    pub fn read_trailing_bits(&mut self, nb_bits: usize) -> Result<(), BitstreamError> {
        if nb_bits == 0 {
            return Ok(());
        }
        if self.read_bit()? != 1 {
            return Err(BitstreamError::MissingTrailingOneBit);
        }
        for index in 1..nb_bits {
            if self.read_bit()? != 0 {
                return Err(BitstreamError::NonZeroTrailingBit { index });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_reads_bits_msb_first_across_bytes() {
        let mut r = BitReader::new(&[0b1011_0010, 0b0110_1101]);
        assert_eq!(r.read_f(1).unwrap(), 1);
        assert_eq!(r.read_f(3).unwrap(), 0b011);
        assert_eq!(r.read_f(8).unwrap(), 0b0010_0110); // straddles the boundary
        assert_eq!(r.position(), 12);
        assert_eq!(r.read_f(4).unwrap(), 0b1101);
        assert_eq!(r.bits_remaining(), 0);
    }

    #[test]
    fn f_zero_bits_and_overread() {
        let mut r = BitReader::new(&[0xff]);
        assert_eq!(r.read_f(0).unwrap(), 0);
        assert_eq!(r.read_f(8).unwrap(), 255);
        assert_eq!(
            r.read_f(1),
            Err(BitstreamError::OutOfBits {
                requested: 1,
                available: 0
            })
        );
        assert_eq!(r.read_f(33), Err(BitstreamError::TooManyBits(33)));
        let mut r = BitReader::new(&[0b1010_0101]);
        r.skip_bits(3).unwrap();
        assert_eq!(r.read_f(2).unwrap(), 0b00);
        assert_eq!(
            r.skip_bits(10),
            Err(BitstreamError::OutOfBits {
                requested: 10,
                available: 3
            })
        );
    }

    #[test]
    fn le_reads_little_endian_and_requires_alignment() {
        let mut r = BitReader::new(&[0x78, 0x56, 0x34, 0x12]);
        assert_eq!(r.read_le(4).unwrap(), 0x1234_5678);
        let mut r = BitReader::new(&[0x40, 0x80]);
        let _ = r.read_bit().unwrap();
        assert_eq!(
            r.read_le(1),
            Err(BitstreamError::NotByteAligned { position: 1 })
        );
    }

    #[test]
    fn uvlc_matches_the_worked_examples() {
        // "1" -> 0, "01 0" -> 1, "001 00" -> 3, "001 10" -> 5.
        for (byte, expected) in [(0x80u8, 0u32), (0x40, 1), (0x20, 3), (0x30, 5)] {
            let bytes = [byte, 0x00];
            let mut r = BitReader::new(&bytes);
            assert_eq!(r.read_uvlc().unwrap(), expected, "byte {byte:#04x}");
        }
        // 32 leading zeros saturate to (1 << 32) - 1 without reading a value.
        // The terminating one still consumes bit 33.
        let mut r = BitReader::new(&[0x00, 0x00, 0x00, 0x00, 0x80]);
        assert_eq!(r.read_uvlc().unwrap(), u32::MAX);
        assert_eq!(r.position(), 33);
    }

    #[test]
    fn leb128_counts_bytes_and_rejects_bad_encodings() {
        for (bytes, expected) in [
            (&[0x00u8][..], 0u32),
            (&[0x0a][..], 10),
            (&[0xac, 0x02][..], 300),
            (&[0x80, 0x80, 0x01][..], 1 << 14),
        ] {
            let mut r = BitReader::new(bytes);
            let (value, count) = r.read_leb128().unwrap();
            assert_eq!(value, expected);
            assert_eq!(count, bytes.len());
            assert_eq!(r.position(), count * 8);
        }
        let mut r = BitReader::new(&[0x80; 8]);
        assert_eq!(r.read_leb128(), Err(BitstreamError::Leb128TooLong));
        // 8 bytes with the last one terminating: value exceeds 2^32 - 1.
        let mut bytes = [0x80u8; 8];
        bytes[7] = 0x7f;
        let mut r = BitReader::new(&bytes);
        assert!(matches!(
            r.read_leb128(),
            Err(BitstreamError::Leb128Overflow(_))
        ));
    }

    #[test]
    fn su_decodes_twos_complement() {
        // n = 4: 0b0000..0b0111 are 0..7, 0b1000..0b1111 are -8..-1.
        let mut r = BitReader::new(&[0b0111_1000, 0b1111_0000]);
        assert_eq!(r.read_su(4).unwrap(), 7);
        assert_eq!(r.read_su(4).unwrap(), -8);
        assert_eq!(r.read_su(4).unwrap(), -1);
        assert_eq!(r.read_su(4).unwrap(), 0);
        assert_eq!(r.read_su(0), Err(BitstreamError::InvalidDescriptorRange(0)));
    }

    #[test]
    fn ns_matches_the_n_equals_five_table() {
        // n=5: w=3, m=3. The specification's encodings are
        // 0 -> 00, 1 -> 01, 2 -> 10, 3 -> 110, 4 -> 111. The first three
        // only consume two bits; the unused third bit is padding here.
        for (byte, expected) in [(0x00u8, 0u32), (0x40, 1), (0x80, 2), (0xc0, 3), (0xe0, 4)] {
            let bytes = [byte, 0x00];
            let mut r = BitReader::new(&bytes);
            assert_eq!(r.read_ns(5).unwrap(), expected, "byte {byte:#04x}");
        }
        let mut r = BitReader::new(&[0x00]);
        assert_eq!(r.read_ns(1).unwrap(), 0);
        assert_eq!(r.read_ns(0), Err(BitstreamError::InvalidDescriptorRange(0)));
    }

    #[test]
    fn byte_alignment_and_trailing_bits_verify_their_padding() {
        let mut r = BitReader::new(&[0b1000_0000]);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert!(r.align_to_byte().is_ok());
        assert_eq!(r.position(), 8);

        let mut bad = BitReader::new(&[0b1100_0000]);
        let _ = bad.read_bit().unwrap();
        assert_eq!(
            bad.align_to_byte(),
            Err(BitstreamError::NonZeroAlignmentBit { position: 1 })
        );

        let mut r = BitReader::new(&[0b1000_0000, 0x00]);
        assert!(r.read_trailing_bits(16).is_ok());
        assert_eq!(r.position(), 16);

        let mut missing_one = BitReader::new(&[0b0000_0000]);
        assert_eq!(
            missing_one.read_trailing_bits(8),
            Err(BitstreamError::MissingTrailingOneBit)
        );
        let mut dirty_zero = BitReader::new(&[0b1100_0000]);
        assert_eq!(
            dirty_zero.read_trailing_bits(8),
            Err(BitstreamError::NonZeroTrailingBit { index: 1 })
        );
    }

    #[test]
    fn remaining_bytes_only_at_byte_boundary() {
        let mut r = BitReader::new(&[0x12, 0x34, 0x56]);
        assert_eq!(r.remaining_bytes().unwrap(), &[0x12, 0x34, 0x56]);
        let _ = r.read_f(8).unwrap();
        assert_eq!(r.remaining_bytes().unwrap(), &[0x34, 0x56]);
        let _ = r.read_bit().unwrap();
        assert_eq!(
            r.remaining_bytes(),
            Err(BitstreamError::NotByteAligned { position: 9 })
        );
    }
}
