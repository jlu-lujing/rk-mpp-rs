use crate::codec::vp9::parser::Vp9Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbabilityUpdate {
    pub delta: u8,
    pub probability: u8,
}

pub struct RangeDecoder<'a> {
    data: &'a [u8],
    position: usize,
    range: u32,
    bits_left: i32,
    value: u32,
}

impl<'a> RangeDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, Vp9Error> {
        if data.len() < 3 {
            return Err(Vp9Error::Truncated { bit: 0, width: 24 });
        }

        Ok(Self {
            data,
            position: 3,
            range: 255,
            bits_left: -16,
            value: read_u24(data, 0),
        })
    }

    pub fn read_bool(&mut self, probability: u8) -> Result<bool, Vp9Error> {
        let shift = normalization_shift(self.range);
        self.range <<= shift;
        self.value <<= shift;
        self.bits_left += shift as i32;

        if self.bits_left >= 0 {
            let word = if self.position + 1 < self.data.len() {
                u16::from_be_bytes([self.data[self.position], self.data[self.position + 1]])
            } else if self.position < self.data.len() {
                u16::from(self.data[self.position]) << 8
            } else {
                0
            };
            self.position = (self.position + 2).min(self.data.len());
            self.value |= u32::from(word) << self.bits_left;
            self.bits_left -= 16;
        }

        let split = 1 + (((self.range - 1) * u32::from(probability)) >> 8);
        let split_scaled = split << 16;
        if self.value >= split_scaled {
            self.range -= split;
            self.value -= split_scaled;
            Ok(true)
        } else {
            self.range = split;
            Ok(false)
        }
    }

    pub fn read_bits(&mut self, width: u8) -> Result<u32, Vp9Error> {
        let mut value = 0;
        for _ in 0..width {
            value = (value << 1) | u32::from(self.read_bool(128)?);
        }
        Ok(value)
    }

    pub fn diff_update_prob(&mut self, probability: u8) -> Result<ProbabilityUpdate, Vp9Error> {
        let delta = if !self.read_bool(128)? {
            self.read_bits(4)?
        } else if !self.read_bool(128)? {
            self.read_bits(4)? + 16
        } else if !self.read_bool(128)? {
            self.read_bits(5)? + 32
        } else {
            let value = self.read_bits(7)?;
            if value < 65 {
                value + 64
            } else {
                (value << 1) - 1 + self.read_bits(1)?
            }
        };
        let delta = delta as usize;
        let mapped = INV_MAP_TABLE[delta] as i32;
        let probability = if probability <= 128 {
            1 + recenter_non_negative(mapped, i32::from(probability) - 1)
        } else {
            255 - recenter_non_negative(mapped, 255 - i32::from(probability))
        };
        Ok(ProbabilityUpdate {
            delta: delta as u8,
            probability: probability as u8,
        })
    }
}

fn normalization_shift(range: u32) -> u32 {
    range.leading_zeros().saturating_sub(24)
}

fn read_u24(data: &[u8], offset: usize) -> u32 {
    (u32::from(data[offset]) << 16)
        | (u32::from(data[offset + 1]) << 8)
        | u32::from(data[offset + 2])
}

fn recenter_non_negative(value: i32, center: i32) -> i32 {
    if value > 2 * center {
        value
    } else if value & 1 != 0 {
        center - ((value + 1) >> 1)
    } else {
        center + (value >> 1)
    }
}

const INV_MAP_TABLE: [u8; 255] = [
    7, 20, 33, 46, 59, 72, 85, 98, 111, 124, 137, 150, 163, 176, 189, 202, 215, 228, 241, 254, 1,
    2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 21, 22, 23, 24, 25, 26, 27, 28,
    29, 30, 31, 32, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 47, 48, 49, 50, 51, 52, 53, 54,
    55, 56, 57, 58, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 73, 74, 75, 76, 77, 78, 79, 80,
    81, 82, 83, 84, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 99, 100, 101, 102, 103, 104,
    105, 106, 107, 108, 109, 110, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 125,
    126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 138, 139, 140, 141, 142, 143, 144, 145,
    146, 147, 148, 149, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 164, 165, 166,
    167, 168, 169, 170, 171, 172, 173, 174, 175, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186,
    187, 188, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 203, 204, 205, 206, 207,
    208, 209, 210, 211, 212, 213, 214, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227,
    229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 242, 243, 244, 245, 246, 247, 248,
    249, 250, 251, 252, 253, 253,
];
