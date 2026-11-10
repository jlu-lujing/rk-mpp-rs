use std::ffi::c_int;

use crate::service::RegOffsetInfo;

pub const REGION_COUNT: usize = 8;
pub const PALETTE_COUNT: usize = 256;
pub const REGISTER_WORDS: usize = 288;

const CONFIG_WORD: usize = 2;
const POSITION_WORD: usize = 4;
const ADDRESS_WORD: usize = 20;
const PALETTE_WORD: usize = 32;
const INVERSE_THRESHOLD: u32 = 15;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub enable: u32,
    pub inverse: u32,
    pub start_mb_x: u32,
    pub start_mb_y: u32,
    pub num_mb_x: u32,
    pub num_mb_y: u32,
    pub buffer_offset: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub buffer_fd: c_int,
    pub buffer_size: usize,
    pub regions: Vec<Region>,
    pub user_palette: Option<[u32; PALETTE_COUNT]>,
}

impl Config {
    pub fn offsets(&self) -> Vec<RegOffsetInfo> {
        self.regions
            .iter()
            .enumerate()
            .filter(|(_, region)| {
                region.enable != 0
                    && region.num_mb_x != 0
                    && region.num_mb_y != 0
                    && region.buffer_offset != 0
            })
            .map(|(index, region)| RegOffsetInfo {
                reg_idx: 3092 + index as u32,
                offset: region.buffer_offset,
            })
            .collect()
    }

    pub fn apply(&self, registers: &mut [u32; REGISTER_WORDS]) -> Result<(), String> {
        if self.regions.len() > REGION_COUNT {
            return Err(format!(
                "OSD accepts at most {REGION_COUNT} regions, got {}",
                self.regions.len()
            ));
        }
        registers.fill(0);
        let mut enable = 0u32;
        let mut luminance_inverse = 0u32;
        let mut chroma_inverse = 0u32;
        for (index, region) in self.regions.iter().enumerate() {
            let bit = 1u32 << index;
            enable |= u32::from(region.enable != 0) * bit;
            if region.inverse != 0 {
                luminance_inverse |= bit;
                chroma_inverse |= bit;
                let shift = index * 4;
                registers[1] = (registers[1] & !(0xf << shift)) | (INVERSE_THRESHOLD << shift);
            }
            if region.enable == 0 || region.num_mb_x == 0 || region.num_mb_y == 0 {
                continue;
            }
            let length = (region.num_mb_x as usize)
                .checked_mul(region.num_mb_y as usize)
                .and_then(|value| value.checked_mul(256))
                .ok_or_else(|| "OSD block length overflow".to_string())?;
            let end = (region.buffer_offset as usize)
                .checked_add(length)
                .ok_or_else(|| "OSD buffer range overflow".to_string())?;
            if region.buffer_offset & 0xf != 0 {
                return Err(format!(
                    "OSD region {index} offset {} is not 16-byte aligned",
                    region.buffer_offset
                ));
            }
            if end > self.buffer_size {
                return Err(format!(
                    "OSD region {index} ends at {end}, buffer has {} bytes",
                    self.buffer_size
                ));
            }
            let position = POSITION_WORD + index * 2;
            registers[position] = region.start_mb_x | region.start_mb_y.wrapping_shl(16);
            registers[position + 1] = region
                .start_mb_x
                .saturating_add(region.num_mb_x)
                .saturating_sub(1)
                | region
                    .start_mb_y
                    .saturating_add(region.num_mb_y)
                    .saturating_sub(1)
                    .wrapping_shl(16);
            registers[ADDRESS_WORD + index] = self.buffer_fd as u32;
        }
        registers[0] = luminance_inverse | chroma_inverse.wrapping_shl(8);
        registers[CONFIG_WORD] = enable
            | if self.user_palette.is_some() {
                1 << 16
            } else {
                1 << 17
            };
        if let Some(palette) = &self.user_palette {
            registers[PALETTE_WORD..].copy_from_slice(palette);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_palette_and_region_positions_match_the_vepu580_layout() {
        let mut palette = [0u32; PALETTE_COUNT];
        palette[0] = 0xaabbccdd;
        let config = Config {
            buffer_fd: 17,
            buffer_size: 2048,
            regions: vec![Region {
                enable: 1,
                inverse: 1,
                start_mb_x: 2,
                start_mb_y: 3,
                num_mb_x: 4,
                num_mb_y: 1,
                buffer_offset: 16,
            }],
            user_palette: Some(palette),
        };
        let mut regs = [0u32; REGISTER_WORDS];
        config.apply(&mut regs).unwrap();
        assert_eq!(regs[0], 0x101);
        assert_eq!(regs[1] & 0xf, 15);
        assert_eq!(regs[2], 0x1_0001);
        assert_eq!(regs[4], 0x0003_0002);
        assert_eq!(regs[5], 0x0003_0005);
        assert_eq!(regs[20], 17);
        assert_eq!(regs[32], 0xaabbccdd);
        assert_eq!(
            config.offsets(),
            [RegOffsetInfo {
                reg_idx: 3092,
                offset: 16
            }]
        );
    }

    #[test]
    fn fixed_palette_selects_the_hardware_default_type() {
        let config = Config {
            buffer_fd: 9,
            buffer_size: 256,
            regions: vec![Region {
                enable: 1,
                ..Region::default()
            }],
            user_palette: None,
        };
        let mut regs = [0u32; REGISTER_WORDS];
        config.apply(&mut regs).unwrap();
        assert_eq!(regs[CONFIG_WORD], 0x0002_0001);
    }
}
