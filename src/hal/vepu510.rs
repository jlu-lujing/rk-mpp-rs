//! VEPU510 register-file scaffolding for RK3576 H.264 encoding.

use super::vepu510_regs::{self, Field};

pub const CTL_WORDS: usize = 24;
pub const FRAME_WORDS: usize = 98;
pub const RC_WORDS: usize = 68;
pub const PARAM_WORDS: usize = 180;
pub const SQI_WORDS: usize = 47;
pub const SCL_WORDS: usize = 226;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterFile {
    pub ctl: [u32; CTL_WORDS],
    pub frame: [u32; FRAME_WORDS],
    pub rc: [u32; RC_WORDS],
    pub param: [u32; PARAM_WORDS],
    pub sqi: [u32; SQI_WORDS],
    pub scl: [u32; SCL_WORDS],
}

impl Default for RegisterFile {
    fn default() -> Self {
        Self {
            ctl: [0; CTL_WORDS],
            frame: [0; FRAME_WORDS],
            rc: [0; RC_WORDS],
            param: [0; PARAM_WORDS],
            sqi: [0; SQI_WORDS],
            scl: [0; SCL_WORDS],
        }
    }
}

impl RegisterFile {
    pub fn set(&mut self, field: Field, value: u32) -> Result<(), String> {
        if field.width == 0 || field.width > 32 {
            return Err(format!("{} has invalid width {}", field.name, field.width));
        }
        let value = if field.width == 32 {
            value
        } else {
            if value >= 1 << field.width {
                return Err(format!(
                    "{}={} does not fit {} bits",
                    field.name, value, field.width
                ));
            }
            value
        };
        let word = self
            .word_mut(field.word)
            .ok_or_else(|| format!("{} is outside the write sections", field.name))?;
        let mask = if field.width == 32 {
            u32::MAX
        } else {
            ((1u32 << field.width) - 1) << field.shift
        };
        *word = (*word & !mask) | (value << field.shift);
        Ok(())
    }

    pub fn get(&self, field: Field) -> Result<u32, String> {
        let word = self
            .word(field.word)
            .ok_or_else(|| format!("{} is outside the write sections", field.name))?;
        let mask = if field.width == 32 {
            u32::MAX
        } else {
            (1u32 << field.width) - 1
        };
        Ok((word >> field.shift) & mask)
    }

    pub fn word(&self, absolute: usize) -> Option<u32> {
        let (section, index) = locate(absolute)?;
        self.section(section).map(|words| words[index])
    }

    pub fn word_mut(&mut self, absolute: usize) -> Option<&mut u32> {
        let (section, index) = locate(absolute)?;
        self.section_mut(section).map(|words| &mut words[index])
    }

    pub fn section(&self, name: &str) -> Option<&[u32]> {
        Some(match name {
            "ctl" => &self.ctl,
            "frame" => &self.frame,
            "rc" => &self.rc,
            "param" => &self.param,
            "sqi" => &self.sqi,
            "scl" => &self.scl,
            _ => return None,
        })
    }

    pub fn section_mut(&mut self, name: &str) -> Option<&mut [u32]> {
        Some(match name {
            "ctl" => &mut self.ctl,
            "frame" => &mut self.frame,
            "rc" => &mut self.rc,
            "param" => &mut self.param,
            "sqi" => &mut self.sqi,
            "scl" => &mut self.scl,
            _ => return None,
        })
    }
}

fn locate(absolute: usize) -> Option<(&'static str, usize)> {
    vepu510_regs::SECTIONS
        .iter()
        .find_map(|&(name, first, count)| {
            absolute
                .checked_sub(first)
                .filter(|&index| index < count)
                .map(|index| (name, index))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_match_the_lf1_ioctl_trace() {
        assert_eq!(
            vepu510_regs::SECTIONS,
            [
                ("ctl", 0, CTL_WORDS),
                ("frame", 156, FRAME_WORDS),
                ("rc", 1024, RC_WORDS),
                ("param", 1472, PARAM_WORDS),
                ("sqi", 2048, SQI_WORDS),
                ("scl", 2176, SCL_WORDS),
            ]
        );
        assert_eq!(vepu510_regs::STATUS_OFFSET, 4096);
        assert_eq!(vepu510_regs::STATUS_WORDS, 148);
    }

    #[test]
    fn fields_cross_preserved_neighbours_correctly() {
        let mut regs = RegisterFile::default();
        regs.ctl[4] = 0xffff_ffff;
        regs.set(vepu510_regs::CTL_ENC_STRT_VEPU_CMD, 3).unwrap();
        assert_eq!(regs.ctl[4], 0xffff_fbff);
        assert_eq!(regs.get(vepu510_regs::CTL_ENC_STRT_VEPU_CMD).unwrap(), 3);
    }

    #[test]
    fn status_fields_are_not_part_of_the_write_file() {
        let mut regs = RegisterFile::default();
        let error = regs.set(vepu510_regs::STATUS_ST_ENC_ST_ENC, 1).unwrap_err();
        assert!(error.contains("outside the write sections"));
    }
}
