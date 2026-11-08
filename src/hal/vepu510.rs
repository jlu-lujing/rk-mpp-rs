//! VEPU510 register-file scaffolding for RK3576 H.264 encoding.

use super::vepu510_idr_template;
use super::vepu510_p_template;
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
    pub fn captured_fixqp_idr_320x240(
        input_fd: u32,
        output_fd: u32,
        recon_fd: u32,
        thumb_fd: u32,
        smear_fd: u32,
    ) -> Result<Self, String> {
        let mut regs = Self {
            ctl: vepu510_idr_template::CTL,
            frame: vepu510_idr_template::FRAME,
            rc: vepu510_idr_template::RC,
            param: vepu510_idr_template::PARAM,
            sqi: vepu510_idr_template::SQI,
            scl: vepu510_idr_template::SCL,
        };

        for index in 4..=6 {
            regs.frame[index] = input_fd;
        }
        for index in 7..=10 {
            regs.frame[index] = recon_fd;
        }
        regs.frame[13] = thumb_fd;
        regs.frame[14] = thumb_fd;
        for index in 16..=19 {
            regs.frame[index] = output_fd;
        }
        regs.frame[28] = smear_fd;
        regs.frame[29] = smear_fd;
        regs.frame[15] = 0;

        for (field, value) in [
            (vepu510_regs::FRAME_ENC_PIC_PIC_QP, 26),
            (vepu510_regs::FRAME_ENC_RSL_PIC_WD8_M1, 39),
            (vepu510_regs::FRAME_ENC_RSL_PIC_HD8_M1, 29),
            (vepu510_regs::FRAME_SRC_STRD0_SRC_STRD0, 320),
            (vepu510_regs::FRAME_SRC_STRD1_SRC_STRD1, 320),
            (vepu510_regs::FRAME_RC_QP_RC_QP_RANGE, 0),
            (vepu510_regs::FRAME_RC_QP_RC_MAX_QP, 26),
            (vepu510_regs::FRAME_RC_QP_RC_MIN_QP, 26),
            (vepu510_regs::H264_SYNT_PPS_PIC_INIT_QP, 26),
        ] {
            regs.set(field, value)?;
        }
        Ok(regs)
    }

    pub fn captured_fixqp_p_320x240(
        input_fd: u32,
        output_fd: u32,
        recon_write_fd: u32,
        recon_read_fd: u32,
        thumb_write_fd: u32,
        thumb_read_fd: u32,
        smear_write_fd: u32,
        smear_read_fd: u32,
        frame_number: u32,
    ) -> Result<Self, String> {
        if frame_number == 0 || frame_number > 5 {
            return Err(format!(
                "captured VEPU510 P frame number {frame_number} is outside 1..=5"
            ));
        }
        let mut regs = Self {
            ctl: vepu510_p_template::CTL,
            frame: vepu510_p_template::FRAME,
            rc: vepu510_p_template::RC,
            param: vepu510_p_template::PARAM,
            sqi: vepu510_p_template::SQI,
            scl: vepu510_p_template::SCL,
        };
        for index in 4..=6 {
            regs.frame[index] = input_fd;
        }
        regs.frame[7] = recon_write_fd;
        regs.frame[8] = recon_write_fd;
        regs.frame[9] = recon_read_fd;
        regs.frame[10] = recon_read_fd;
        regs.frame[13] = thumb_write_fd;
        regs.frame[14] = thumb_read_fd;
        for index in 16..=19 {
            regs.frame[index] = output_fd;
        }
        regs.frame[28] = smear_read_fd;
        regs.frame[29] = smear_write_fd;
        regs.frame[15] = 0;
        regs.set(vepu510_regs::H264_SYNT_SLI0_FRM_NUM, frame_number)?;
        regs.set(vepu510_regs::H264_SYNT_SLI1_IDR_PID, 0xffff)?;
        regs.set(
            vepu510_regs::H264_SYNT_SLI1_POC_LSB,
            frame_number.saturating_mul(2),
        )?;
        regs.sqi[5] = if frame_number & 1 != 0 {
            0x0000_2810
        } else {
            0x0000_4810
        };
        Ok(regs)
    }

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
    use crate::hal::vepu510_idr_template;
    use crate::hal::vepu510_p_template;

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

    #[test]
    fn captured_idr_template_matches_the_window_sizes() {
        assert_eq!(vepu510_idr_template::CTL.len(), CTL_WORDS);
        assert_eq!(vepu510_idr_template::FRAME.len(), FRAME_WORDS);
        assert_eq!(vepu510_idr_template::RC.len(), RC_WORDS);
        assert_eq!(vepu510_idr_template::PARAM.len(), PARAM_WORDS);
        assert_eq!(vepu510_idr_template::SQI.len(), SQI_WORDS);
        assert_eq!(vepu510_idr_template::SCL.len(), SCL_WORDS);
        assert_eq!(
            vepu510_idr_template::STATUS.len(),
            vepu510_regs::STATUS_WORDS
        );
    }

    #[test]
    fn captured_fixqp_builder_reproduces_the_reference_job() {
        let regs = RegisterFile::captured_fixqp_idr_320x240(9, 10, 13, 14, 15).unwrap();
        assert_eq!(regs.ctl, vepu510_idr_template::CTL);
        assert_eq!(regs.frame, vepu510_idr_template::FRAME);
        assert_eq!(regs.rc, vepu510_idr_template::RC);
        assert_eq!(regs.param, vepu510_idr_template::PARAM);
        assert_eq!(regs.sqi, vepu510_idr_template::SQI);
        assert_eq!(regs.scl, vepu510_idr_template::SCL);
    }

    #[test]
    fn captured_fixqp_p_builder_reproduces_the_reference_job() {
        let regs =
            RegisterFile::captured_fixqp_p_320x240(9, 10, 16, 13, 17, 14, 18, 15, 1).unwrap();
        assert_eq!(regs.ctl, vepu510_p_template::CTL);
        assert_eq!(regs.frame, vepu510_p_template::FRAME);
        assert_eq!(regs.rc, vepu510_p_template::RC);
        assert_eq!(regs.param, vepu510_p_template::PARAM);
        assert_eq!(regs.sqi, vepu510_p_template::SQI);
        assert_eq!(regs.scl, vepu510_p_template::SCL);
    }
}
