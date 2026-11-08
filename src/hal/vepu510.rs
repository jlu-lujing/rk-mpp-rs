//! VEPU510 register-file scaffolding for RK3576 H.264 encoding.

use super::vepu510_idr_template;
use super::vepu510_p_template;
use super::vepu510_regs::{self, Field};
use crate::codec::h264::encoder::RoiRegion;

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

    pub fn apply_cbr_rate_control(
        &mut self,
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
        is_idr: bool,
    ) -> Result<(), String> {
        if bit_target <= 0 {
            return Err(format!("invalid VEPU510 CBR bit target {bit_target}"));
        }
        let mb_w = 20i32;
        let mb_h = 15i32;
        let mut target_16 = bit_target.wrapping_shl(4) / (mb_w * mb_h);
        if target_16 >= 0x0010_0000 {
            target_16 = 0x0005_0000;
        }
        let mb_target = (target_16 * mb_w) >> 4;
        let negative = 0 - 5 * mb_target / 16;
        let positive = 5 * mb_target / 16;

        self.set(vepu510_regs::FRAME_ENC_PIC_PIC_QP, qp_target)?;
        self.set(vepu510_regs::FRAME_RC_CFG_RC_EN, 1)?;
        self.set(vepu510_regs::FRAME_RC_CFG_AQ_EN, 1)?;
        self.set(vepu510_regs::FRAME_RC_CFG_RC_CTU_NUM, mb_w as u32)?;
        self.set(
            vepu510_regs::FRAME_RC_QP_RC_QP_RANGE,
            if is_idr { 1 } else { 2 },
        )?;
        self.set(vepu510_regs::FRAME_RC_QP_RC_MAX_QP, qp_max)?;
        self.set(vepu510_regs::FRAME_RC_QP_RC_MIN_QP, qp_min)?;
        self.set(vepu510_regs::FRAME_RC_TGT_CTU_EBIT, target_16 as u32)?;

        for (field, value) in [
            (vepu510_regs::RC_RC_ADJ0_QP_ADJ0, -2),
            (vepu510_regs::RC_RC_ADJ0_QP_ADJ1, -1),
            (vepu510_regs::RC_RC_ADJ0_QP_ADJ2, 0),
            (vepu510_regs::RC_RC_ADJ0_QP_ADJ3, 1),
            (vepu510_regs::RC_RC_ADJ0_QP_ADJ4, 2),
            (vepu510_regs::RC_RC_ADJ1_QP_ADJ5, 0),
            (vepu510_regs::RC_RC_ADJ1_QP_ADJ6, 0),
            (vepu510_regs::RC_RC_ADJ1_QP_ADJ7, 0),
            (vepu510_regs::RC_RC_ADJ1_QP_ADJ8, 0),
        ] {
            self.set_signed(field, value)?;
        }

        let thresholds = [
            4 * negative,
            negative,
            positive,
            4 * positive,
            0x7fff_ffff,
            0x7fff_ffff,
            0x7fff_ffff,
            0x7fff_ffff,
            0x7fff_ffff,
        ];
        for (index, value) in thresholds.iter().enumerate() {
            self.rc[2 + index] = *value as u32;
        }

        for (field, value) in [
            (vepu510_regs::RC_ROI_QTHD0_QPMIN_AREA0, qp_min),
            (vepu510_regs::RC_ROI_QTHD0_QPMAX_AREA0, qp_max),
            (vepu510_regs::RC_ROI_QTHD0_QPMIN_AREA1, qp_min),
            (vepu510_regs::RC_ROI_QTHD0_QPMAX_AREA1, qp_max),
            (vepu510_regs::RC_ROI_QTHD0_QPMIN_AREA2, qp_min),
            (vepu510_regs::RC_ROI_QTHD1_QPMAX_AREA2, qp_max),
            (vepu510_regs::RC_ROI_QTHD1_QPMIN_AREA3, qp_min),
            (vepu510_regs::RC_ROI_QTHD1_QPMAX_AREA3, qp_max),
            (vepu510_regs::RC_ROI_QTHD1_QPMIN_AREA4, qp_min),
            (vepu510_regs::RC_ROI_QTHD1_QPMAX_AREA4, qp_max),
            (vepu510_regs::RC_ROI_QTHD2_QPMIN_AREA5, qp_min),
            (vepu510_regs::RC_ROI_QTHD2_QPMAX_AREA5, qp_max),
            (vepu510_regs::RC_ROI_QTHD2_QPMIN_AREA6, qp_min),
            (vepu510_regs::RC_ROI_QTHD2_QPMAX_AREA6, qp_max),
            (vepu510_regs::RC_ROI_QTHD2_QPMIN_AREA7, qp_min),
            (vepu510_regs::RC_ROI_QTHD3_QPMAX_AREA7, qp_max),
        ] {
            self.set(field, value)?;
        }
        self.set(vepu510_regs::RC_ROI_QTHD3_QPMAP_MODE, 1)
    }

    pub fn apply_roi_regions(&mut self, regions: &[RoiRegion]) -> Result<(), String> {
        if regions.len() > 8 {
            return Err(format!(
                "VEPU510 accepts at most 8 ROI regions, got {}",
                regions.len()
            ));
        }
        self.rc[36..68].fill(0);
        for (index, region) in regions.iter().enumerate() {
            let left = (region.x.clamp(0, 320) as usize).div_ceil(16);
            let top = (region.y.clamp(0, 240) as usize).div_ceil(16);
            let right = (region.x.saturating_add(region.w).clamp(0, 320) as usize).div_ceil(16);
            let bottom = (region.y.saturating_add(region.h).clamp(0, 240) as usize).div_ceil(16);
            let base = 36 + index * 4;
            self.rc[base] = left as u32 | (top as u32) << 16;
            self.rc[base + 1] = right as u32 | (bottom as u32) << 16;
            self.rc[base + 2] = (region.qp_val as u32 & 0x7f)
                | (u32::from(region.qp_absolute) << 7)
                | (0x1f << 8)
                | (1 << 13);
            if region.force_intra {
                self.rc[base + 3] = (1 << 8) | (1 << 20);
            }
        }
        Ok(())
    }

    fn set_signed(&mut self, field: Field, value: i32) -> Result<(), String> {
        let mask = (1u32 << field.width) - 1;
        self.set(field, value as u32 & mask)
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

    #[test]
    fn roi_region_matches_the_c_vpu510_words() {
        let mut regs = RegisterFile::default();
        regs.apply_roi_regions(&[RoiRegion {
            x: 0,
            y: 0,
            w: 160,
            h: 120,
            force_intra: true,
            qp_absolute: true,
            qp_val: 30,
        }])
        .unwrap();
        assert_eq!(regs.rc[37], 0x0008_000a);
        assert_eq!(regs.rc[38], 0x0000_3f9e);
        assert_eq!(regs.rc[39], 0x0010_0100);
    }
}
