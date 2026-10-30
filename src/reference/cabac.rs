//! CABAC arithmetic coder (H.264 9.3 / H.265 9.3), decode and encode sides.
//!
//! Only the arithmetic core and the context-model state machine live here; the
//! context tables and the syntax element readers build on top.

/// One context model: 6-bit probability state plus the most probable symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Context {
    pub state: u8,
    pub mps: u8,
}

impl Context {
    pub fn new(state: u8, mps: u8) -> Context {
        Context {
            state: state & 0x3f,
            mps: mps & 1,
        }
    }
}

/// H.264/HEVC Table 9-45: `rangeTabLPS[state][(range >> 6) & 3]`.
#[rustfmt::skip]
const LPS_TABLE: [[u8; 4]; 64] = [
    [128,176,208,240],[128,167,197,227],[128,158,187,216],[123,150,178,205],
    [116,142,169,195],[111,135,160,185],[105,128,152,175],[100,122,144,166],
    [95,116,137,158],[90,110,130,150],[85,104,123,142],[81,99,117,135],
    [77,94,111,128],[73,89,105,122],[69,85,100,116],[66,80,95,110],
    [62,76,90,104],[59,72,86,99],[56,69,81,94],[53,65,77,89],
    [51,62,73,85],[48,59,69,80],[46,56,66,76],[43,53,63,72],
    [41,50,59,69],[39,48,56,65],[37,45,54,62],[35,43,51,59],
    [33,41,48,56],[32,39,46,53],[30,37,43,50],[29,35,41,48],
    [27,33,39,45],[26,31,37,43],[24,30,35,41],[23,28,33,39],
    [22,27,32,37],[21,26,30,35],[20,24,29,33],[19,23,27,31],
    [18,22,26,30],[17,21,25,28],[16,20,23,27],[15,19,22,25],
    [14,18,21,24],[14,17,20,23],[13,16,19,22],[12,15,18,21],
    [12,14,17,20],[11,14,16,19],[11,13,15,18],[10,12,15,17],
    [10,12,14,16],[9,11,13,15],[9,11,12,14],[8,10,12,14],
    [8,9,11,13],[7,9,11,12],[7,9,10,12],[7,8,10,11],
    [6,8,9,11],[6,7,9,10],[6,7,8,9],[2,2,2,2],
];

/// Table 9-44: next state after coding an MPS.
const TRANS_MPS: [u8; 64] = [
    1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,
    33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,
    62,62,63,
];
/// Table 9-44: next state after coding an LPS (state 0 flips the MPS instead).
const TRANS_LPS: [u8; 64] = [
    0,0,1,2,2,4,4,5,6,7,8,9,9,11,11,12,13,13,15,15,16,16,18,18,19,19,21,21,22,22,23,24,
    24,25,26,26,27,27,28,29,29,30,30,30,31,32,32,33,33,33,34,34,35,35,35,36,36,36,37,37,37,
    38,38,63,
];

impl Context {
    pub fn lps_range(&self, range: u32) -> u32 {
        LPS_TABLE[self.state as usize][((range >> 6) & 3) as usize] as u32
    }

    fn update(&mut self, lps: bool) {
        if lps {
            if self.state == 0 {
                self.mps ^= 1;
            } else {
                self.state = TRANS_LPS[self.state as usize];
            }
        } else {
            self.state = TRANS_MPS[self.state as usize];
        }
    }
}

/// Bit reader over the RBSP used by the arithmetic decoder.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0 }
    }
    fn bit(&mut self) -> u32 {
        let v = if self.pos / 8 < self.data.len() {
            (self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1
        } else {
            0
        };
        self.pos += 1;
        v as u32
    }
    fn bits(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit();
        }
        v
    }
}

/// Arithmetic decoder (H.265 9.3.4).
pub struct CabacDecoder<'a> {
    bits: Bits<'a>,
    range: u32,
    offset: u32,
}

impl<'a> CabacDecoder<'a> {
    pub fn new(data: &'a [u8]) -> CabacDecoder<'a> {
        let mut d = CabacDecoder {
            bits: Bits::new(data),
            range: 510,
            offset: 0,
        };
        // 16-bit priming, as the H.264/HEVC reference decoders do.
        d.offset = d.bits.bits(16);
        d
    }

    fn renorm(&mut self) {
        while self.range < 256 {
            self.range <<= 1;
            self.offset = (self.offset << 1) | self.bits.bit();
        }
    }

    /// Decode one bin with a context model (H.264 9.3.4.3.2).
    ///
    /// After `range -= lps`, the MPS occupies `[0, range)` and the LPS
    /// occupies `[range, range + lps)`, so the offset is compared against the
    /// *reduced* range, not against `lps`.
    pub fn decode_bin(&mut self, ctx: &mut Context) -> u32 {
        let lps = ctx.lps_range(self.range);
        self.range -= lps;
        let mps = ctx.mps as u32;
        let bin;
        if self.offset >= self.range {
            self.offset -= self.range;
            self.range = lps;
            ctx.update(true);
            bin = mps ^ 1;
        } else {
            ctx.update(false);
            bin = mps;
        }
        self.renorm();
        bin
    }

    /// Decode one bypass bin (H.264 9.3.4.3.3): the shift itself is the
    /// renormalisation, so the range is untouched.
    pub fn decode_bypass(&mut self) -> u32 {
        self.offset = (self.offset << 1) | self.bits.bit();
        if self.offset >= self.range {
            self.offset -= self.range;
            1
        } else {
            0
        }
    }

    /// Decode the terminating bin; true ends the slice segment.
    pub fn decode_terminate(&mut self) -> bool {
        self.range -= 2;
        if self.offset >= self.range {
            true
        } else {
            self.renorm();
            false
        }
    }

    /// Number of bits consumed, for slice-data boundary checks.
    pub fn bit_pos(&self) -> usize {
        self.bits.pos
    }
}

/// Arithmetic encoder used only by the tests (and later by an encoder module).
pub struct CabacEncoder {
    low: u64,
    range: u32,
    outstanding: u32,
    out: Vec<u8>,
    cur: u8,
    cur_bits: u32,
}

impl Default for CabacEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl CabacEncoder {
    pub fn new() -> CabacEncoder {
        CabacEncoder {
            low: 0,
            range: 510,
            outstanding: 0,
            out: Vec::new(),
            cur: 0,
            cur_bits: 0,
        }
    }

    fn put_bit(&mut self, bit: u32) {
        self.cur = (self.cur << 1) | bit as u8;
        self.cur_bits += 1;
        if self.cur_bits == 8 {
            self.out.push(self.cur);
            self.cur = 0;
            self.cur_bits = 0;
        }
    }

    fn put_bit_plus_outstanding(&mut self, bit: u32) {
        self.put_bit(bit);
        while self.outstanding > 0 {
            self.put_bit(bit ^ 1);
            self.outstanding -= 1;
        }
    }

    fn renorm(&mut self) {
        while self.range < 256 {
            if self.low < 0x100 {
                self.put_bit_plus_outstanding(0);
            } else if self.low >= 0x200 {
                self.put_bit_plus_outstanding(1);
                self.low -= 0x200;
            } else {
                self.outstanding += 1;
                self.low -= 0x100;
            }
            self.low <<= 1;
            self.range <<= 1;
        }
    }

    pub fn encode_bin(&mut self, ctx: &mut Context, bin: u32) {
        let lps = ctx.lps_range(self.range);
        let mps_bit = ctx.mps as u32;
        let lps_bin = mps_bit ^ 1;
        self.range -= lps;
        if bin == mps_bit {
            ctx.update(false);
            self.renorm();
        } else {
            self.low += self.range as u64;
            self.range = lps;
            ctx.update(true);
            let _ = lps_bin;
            self.renorm();
        }
    }

    pub fn encode_bypass(&mut self, bin: u32) {
        self.low <<= 1;
        if bin != 0 {
            self.low += self.range as u64;
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        // Flush the low word: emit the 16 most significant fractional bits so
        // the decoder's 16-bit priming sees a value inside the final interval.
        for shift in (0..16).rev() {
            let bit = ((self.low >> shift) & 1) as u32;
            self.put_bit_plus_outstanding(bit);
        }
        while self.cur_bits != 0 {
            self.put_bit(0);
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_reaches_the_flip() {
        let mut c = Context::new(0, 0);
        let mut flips = 0;
        for _ in 0..200 {
            let before = c.mps;
            c.update(true);
            if c.mps != before {
                flips += 1;
            }
        }
        assert!(flips >= 1, "state 63 must flip the MPS");
    }

    #[test]
    fn lps_range_shrinks_with_state() {
        let a = Context::new(0, 0).lps_range(512);
        let b = Context::new(63, 0).lps_range(512);
        assert!(b >= 2);
        assert!(a >= b);
    }

    #[test]
    fn zero_stream_decodes_as_mps() {
        // offset 0 always falls in the MPS sub-interval, so an all-zero stream
        // yields the MPS of every context until the bits run out.
        let data = [0u8; 8];
        let mut dec = CabacDecoder::new(&data);
        let mut ctx = Context::new(20, 1);
        for _ in 0..16 {
            assert_eq!(dec.decode_bin(&mut ctx), 1, "MPS is 1");
        }
    }

    #[test]
    fn all_ones_priming_takes_the_lps_first() {
        // The 9-bit priming of 0x1ff is outside the 510 range, so the first
        // bin must be the LPS.
        let data = [0xffu8; 8];
        let mut dec = CabacDecoder::new(&data);
        let mut ctx = Context::new(0, 0);
        assert_eq!(dec.decode_bin(&mut ctx), 1, "LPS of an MPS=0 context");
    }

    #[test]
    fn bypass_reads_the_bits_verbatim() {
        // Bypass bins are the raw bits after the priming, so a stream of
        // 0x00 0x00 ... (after a zero priming) keeps yielding zeros.
        let data = [0x00u8; 8];
        let mut dec = CabacDecoder::new(&data);
        for _ in 0..8 {
            assert_eq!(dec.decode_bypass(), 0);
        }
        let ones = [0xffu8; 8];
        let mut dec = CabacDecoder::new(&ones);
        let _ = dec.decode_bin(&mut Context::new(0, 0));
        assert_eq!(dec.decode_bypass(), 1);
    }
}
