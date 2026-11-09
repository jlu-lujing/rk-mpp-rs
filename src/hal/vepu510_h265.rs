//! First RK3576 VEPU510 H.265 hardware path.
//!
//! The captured job is a fixed-QP 320x240 Main-profile IDR. Software supplies
//! VPS/SPS/PPS and the two prefix SEIs; the hardware writes the slice payload.

use crate::codec::h264::encoder::RoiRegion;
use crate::codec::h265::encoder::{self, EncoderConfig};
use crate::codec::h265::sei;
use crate::codec::rc::model as rc_model;
use crate::hal::h265e_session::RcMode;
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{require_video_encode, VideoEncodeHal};
use crate::service::{Batch, Client, MppService, RegOffsetInfo, SliceInfo};

use super::vepu510_h265_idr_template as template;
use super::vepu510_h265_p_template as p_template;
use super::vepu510_h265_regs::{self as regs, Field};

pub const CTL_WORDS: usize = 24;
pub const FRAME_WORDS: usize = 98;
pub const RC_WORDS: usize = 68;
pub const PARAM_WORDS: usize = 180;
pub const SQI_WORDS: usize = 76;
pub const SCL_WORDS: usize = 226;

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const QP: u32 = 26;
const GOP: u32 = 60;
const CBR_TARGET: i32 = 300_000;
const CBR_MIN: i32 = 250_000;
const CBR_MAX: i32 = 350_000;
const INPUT_SIZE: usize = WIDTH * HEIGHT * 3 / 2;
const OUTPUT_SIZE: usize = 128 * 1024;
const FBC_HEADER_SIZE: usize = 8 * 1024;
const RECON_SIZE: usize = 161_792;
const COLMV_SIZE: usize = 28_672;
const THUMB_SIZE: usize = 6_400;
const SMEAR_SIZE: usize = 256;
const MD_INFO_SIZE: usize = 1_280;

const CTL_OFFSET: u32 = 0;
const FRAME_OFFSET: u32 = 624;
const RC_OFFSET: u32 = 4096;
const PARAM_OFFSET: u32 = 5888;
const SQI_OFFSET: u32 = 8192;
const SCL_OFFSET: u32 = 8704;
const HW_STATUS_OFFSET: u32 = 0x2c;
const STATUS_OFFSET: u32 = 16 * 1024;

const LF1_VERSION_TEXT: &[u8] =
    b"4ed4f778 author: Herman Chen   2025-09-10 docs: Update 1.0.11 CHANGELOG.md";

struct RateControlPlan {
    bit_target: i32,
    qp_target: u32,
    qp_min: u32,
    qp_max: u32,
    qp_delta: u32,
}

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
        colmv_fd: u32,
        thumb_fd: u32,
        smear_fd: u32,
        md_info_fd: u32,
    ) -> Result<Self, String> {
        let mut regs = Self {
            ctl: template::CTL,
            frame: template::FRAME,
            rc: template::RC,
            param: template::PARAM,
            sqi: template::SQI,
            scl: template::SCL,
        };
        for index in 4..=6 {
            regs.frame[index] = input_fd;
        }
        for index in 7..=10 {
            regs.frame[index] = recon_fd;
        }
        for index in 11..=12 {
            regs.frame[index] = colmv_fd;
        }
        regs.frame[13] = thumb_fd;
        regs.frame[14] = thumb_fd;
        regs.frame[15] = md_info_fd;
        for index in 16..=19 {
            regs.frame[index] = output_fd;
        }
        regs.frame[28] = smear_fd;
        regs.frame[29] = smear_fd;

        // These are already at QP26 in the capture. Setting them from the field
        // map keeps the template honest and catches a wrong struct overlay.
        regs.set(regs::FRAME_ENC_PIC_PIC_QP, QP)?;
        regs.set(regs::FRAME_RC_QP_RC_MAX_QP, QP)?;
        regs.set(regs::FRAME_RC_QP_RC_MIN_QP, QP)?;
        regs.set(regs::H265_SYNT_PPS_PIC_INIT_QP, QP)?;
        Ok(regs)
    }

    pub fn captured_fixqp_p_320x240(
        input_fd: u32,
        output_fd: u32,
        recon_write_fd: u32,
        recon_read_fd: u32,
        colmv_write_fd: u32,
        colmv_read_fd: u32,
        thumb_write_fd: u32,
        thumb_read_fd: u32,
        smear_write_fd: u32,
        smear_read_fd: u32,
        md_info_fd: u32,
        frame_number: u32,
    ) -> Result<Self, String> {
        if frame_number == 0 || frame_number > 32_767 {
            return Err(format!(
                "captured VEPU510 H.265 P frame number {frame_number} is outside 1..=32767"
            ));
        }
        let mut regs = Self {
            ctl: p_template::CTL,
            frame: p_template::FRAME,
            rc: p_template::RC,
            param: p_template::PARAM,
            sqi: p_template::SQI,
            scl: p_template::SCL,
        };
        for index in 4..=6 {
            regs.frame[index] = input_fd;
        }
        regs.frame[7] = recon_write_fd;
        regs.frame[8] = recon_write_fd;
        regs.frame[9] = recon_read_fd;
        regs.frame[10] = recon_read_fd;
        regs.frame[11] = colmv_write_fd;
        regs.frame[12] = colmv_read_fd;
        regs.frame[13] = thumb_write_fd;
        regs.frame[14] = thumb_read_fd;
        regs.frame[15] = md_info_fd;
        for index in 16..=19 {
            regs.frame[index] = output_fd;
        }
        regs.frame[28] = smear_read_fd;
        regs.frame[29] = smear_write_fd;
        regs.set(regs::H265_SYNT_SLI2_SLI_POC_LSB, frame_number)?;
        if frame_number >= 2 {
            regs.set(regs::SQI_SMEAR_OPT_CFG0_SMEAR_LOAD_EN, 1)?;
        }
        Ok(regs)
    }

    fn apply_rate_control(&mut self, rc: &RateControlPlan, is_intra: bool) -> Result<(), String> {
        let mb_w = (WIDTH + 31) / 32;
        let mb_h = (HEIGHT + 31) / 32;
        let mut target_16 = rc.bit_target.wrapping_shl(4) / (mb_w * mb_h) as i32;
        if target_16 >= 0x0010_0000 {
            target_16 = 0x0005_0000;
        }
        let ctu_bits = (target_16 * mb_w as i32) >> 4;
        let negative = 0 - 5 * ctu_bits / 16;
        let positive = 5 * ctu_bits / 16;

        self.set(regs::FRAME_ENC_PIC_PIC_QP, rc.qp_target)?;
        self.set(regs::H265_SYNT_SLI1_SLI_QP, rc.qp_target)?;
        self.set(regs::FRAME_RC_CFG_RC_EN, 1)?;
        self.set(regs::FRAME_RC_CFG_AQ_EN, 1)?;
        self.set(regs::FRAME_RC_CFG_RC_CTU_NUM, mb_w as u32)?;
        self.set(regs::FRAME_RC_QP_RC_MAX_QP, rc.qp_max)?;
        self.set(regs::FRAME_RC_QP_RC_MIN_QP, rc.qp_min)?;
        self.set(regs::FRAME_RC_QP_RC_QP_RANGE, rc.qp_delta)?;
        self.set(regs::FRAME_RC_TGT_CTU_EBIT, target_16 as u32)?;
        self.set(regs::H265_SYNT_PPS_CU_QP_DLT_EN, 1)?;

        let thresholds = [
            2 * negative,
            negative,
            positive,
            2 * positive,
            i32::MAX,
            i32::MAX,
            i32::MAX,
            i32::MAX,
            i32::MAX,
        ];
        let start = regs::RC_RC_DTHD_0_8.word;
        for (index, value) in thresholds.iter().enumerate() {
            *self.rc_word_mut(start + index)? = *value as u32;
        }

        for (field, value) in [
            (regs::RC_RC_ADJ0_QP_ADJ0, -2),
            (regs::RC_RC_ADJ0_QP_ADJ1, -1),
            (regs::RC_RC_ADJ0_QP_ADJ2, 0),
            (regs::RC_RC_ADJ0_QP_ADJ3, 1),
            (regs::RC_RC_ADJ0_QP_ADJ4, 2),
            (regs::RC_RC_ADJ1_QP_ADJ5, 0),
            (regs::RC_RC_ADJ1_QP_ADJ6, 0),
            (regs::RC_RC_ADJ1_QP_ADJ7, 0),
            (regs::RC_RC_ADJ1_QP_ADJ8, 0),
        ] {
            self.set_signed(field, value)?;
        }

        for (field, value) in [
            (regs::RC_ROI_QTHD0_QPMIN_AREA0, rc.qp_min),
            (regs::RC_ROI_QTHD0_QPMAX_AREA0, rc.qp_max),
            (regs::RC_ROI_QTHD0_QPMIN_AREA1, rc.qp_min),
            (regs::RC_ROI_QTHD0_QPMAX_AREA1, rc.qp_max),
            (regs::RC_ROI_QTHD0_QPMIN_AREA2, rc.qp_min),
            (regs::RC_ROI_QTHD1_QPMAX_AREA2, rc.qp_max),
            (regs::RC_ROI_QTHD1_QPMIN_AREA3, rc.qp_min),
            (regs::RC_ROI_QTHD1_QPMAX_AREA3, rc.qp_max),
            (regs::RC_ROI_QTHD1_QPMIN_AREA4, rc.qp_min),
            (regs::RC_ROI_QTHD1_QPMAX_AREA4, rc.qp_max),
            (regs::RC_ROI_QTHD2_QPMIN_AREA5, rc.qp_min),
            (regs::RC_ROI_QTHD2_QPMAX_AREA5, rc.qp_max),
            (regs::RC_ROI_QTHD2_QPMIN_AREA6, rc.qp_min),
            (regs::RC_ROI_QTHD2_QPMAX_AREA6, rc.qp_max),
            (regs::RC_ROI_QTHD2_QPMIN_AREA7, rc.qp_min),
            (regs::RC_ROI_QTHD3_QPMAX_AREA7, rc.qp_max),
        ] {
            self.set(field, value)?;
        }
        self.set(regs::RC_ROI_QTHD3_QPMAP_MODE, 1)?;
        if is_intra {
            self.set(regs::H265_SYNT_PPS_PIC_INIT_QP, 26)?;
        }
        Ok(())
    }

    fn apply_slice_split(
        &mut self,
        mode: encoder::SplitMode,
        arg: u32,
        split_out: u32,
    ) -> Result<(), String> {
        if split_out & !encoder::MPP_ENC_SPLIT_OUT_LOWDELAY != 0 {
            return Err(format!(
                "VEPU510 H.265 slice output bits 0x{split_out:x} are unsupported"
            ));
        }
        if split_out != 0 && mode == encoder::SplitMode::None {
            return Err("VEPU510 H.265 low-delay output needs a split mode".into());
        }
        self.set(regs::FRAME_SLI_SPLT_SLI_SPLT, 0)?;
        self.set(regs::FRAME_SLI_SPLT_SLI_SPLT_MODE, 0)?;
        self.set(regs::FRAME_SLI_SPLT_SLI_SPLT_CPST, 0)?;
        self.set(regs::FRAME_SLI_SPLT_SLI_MAX_NUM_M1, 0)?;
        self.set(regs::FRAME_SLI_SPLT_SLI_FLSH, 0)?;
        self.set(regs::FRAME_SLI_CNUM_SLI_SPLT_CNUM_M1, 0)?;
        self.set(regs::FRAME_SLI_BYTE_SLI_SPLT_BYTE, 0)?;
        self.set(regs::FRAME_ENC_PIC_SLEN_FIFO, 0)?;
        self.set(regs::CTL_INT_EN_VSLC_DONE_EN, 0)?;

        match mode {
            encoder::SplitMode::None => {}
            encoder::SplitMode::ByByte => {
                if arg == 0 || arg > 0x000f_ffff {
                    return Err(format!(
                        "VEPU510 H.265 slice byte target {arg} is outside 1..=1048575"
                    ));
                }
                self.set(regs::FRAME_SLI_SPLT_SLI_SPLT, 1)?;
                self.set(regs::FRAME_SLI_SPLT_SLI_MAX_NUM_M1, 500)?;
                self.set(regs::FRAME_SLI_SPLT_SLI_FLSH, 1)?;
                self.set(regs::FRAME_SLI_BYTE_SLI_SPLT_BYTE, arg)?;
                if split_out != 0 {
                    self.set(regs::FRAME_ENC_PIC_SLEN_FIFO, 1)?;
                    self.set(regs::CTL_INT_EN_VSLC_DONE_EN, 1)?;
                }
            }
            encoder::SplitMode::ByCtu => {
                if arg == 0 || arg > 0x0010_0000 {
                    return Err(format!(
                        "VEPU510 H.265 slice CTU count {arg} is outside 1..=1048576"
                    ));
                }
                self.set(regs::FRAME_SLI_SPLT_SLI_SPLT, 1)?;
                self.set(regs::FRAME_SLI_SPLT_SLI_SPLT_MODE, 1)?;
                self.set(regs::FRAME_SLI_SPLT_SLI_MAX_NUM_M1, 500)?;
                self.set(regs::FRAME_SLI_SPLT_SLI_FLSH, 1)?;
                self.set(regs::FRAME_SLI_CNUM_SLI_SPLT_CNUM_M1, arg - 1)?;
                if split_out != 0 {
                    self.set(regs::FRAME_ENC_PIC_SLEN_FIFO, 1)?;
                    self.set(regs::CTL_INT_EN_VSLC_DONE_EN, 1)?;
                }
            }
        }
        Ok(())
    }

    fn apply_gop_smear(&mut self, position: u32, gop: u32) -> Result<(), String> {
        let gop = gop.max(1);
        let load = u32::from(position % gop > 1);
        let store = u32::from(position % gop != 0 && position % gop != gop - 1);
        self.set(regs::SQI_SMEAR_OPT_CFG0_SMEAR_LOAD_EN, load)?;
        self.set(regs::SQI_SMEAR_OPT_CFG0_SMEAR_STOR_EN, store)
    }

    fn apply_roi_regions(&mut self, regions: &[RoiRegion]) -> Result<(), String> {
        if regions.len() > 8 {
            return Err(format!(
                "VEPU510 H.265 accepts at most 8 ROI regions, got {}",
                regions.len()
            ));
        }
        self.rc[36..68].fill(0);
        for (index, region) in regions.iter().enumerate() {
            let left = (region.x.clamp(0, WIDTH as i32) as usize).div_ceil(16);
            let top = (region.y.clamp(0, HEIGHT as i32) as usize).div_ceil(16);
            let right =
                (region.x.saturating_add(region.w).clamp(0, WIDTH as i32) as usize).div_ceil(16);
            let bottom =
                (region.y.saturating_add(region.h).clamp(0, HEIGHT as i32) as usize).div_ceil(16);
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

    pub fn set(&mut self, field: Field, value: u32) -> Result<(), String> {
        if field.width == 0 || field.width > 32 {
            return Err(format!("{} has invalid width {}", field.name, field.width));
        }
        let mask = if field.width == 32 {
            u32::MAX
        } else {
            (1u32 << field.width) - 1
        };
        if value & !mask != 0 {
            return Err(format!(
                "{}={} does not fit {} bits",
                field.name, value, field.width
            ));
        }
        let word = self
            .word_mut(field.word)
            .ok_or_else(|| format!("{} is outside the write sections", field.name))?;
        *word = (*word & !(mask << field.shift)) | (value << field.shift);
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

    fn word(&self, absolute: usize) -> Option<u32> {
        let (section, index) = locate(absolute)?;
        self.section(section).map(|words| words[index])
    }

    fn word_mut(&mut self, absolute: usize) -> Option<&mut u32> {
        let (section, index) = locate(absolute)?;
        self.section_mut(section).map(|words| &mut words[index])
    }

    fn rc_word_mut(&mut self, absolute: usize) -> Result<&mut u32, String> {
        if !(1024..1092).contains(&absolute) {
            return Err(format!("word {absolute} is outside the H.265 RC window"));
        }
        Ok(&mut self.rc[absolute - 1024])
    }

    fn set_signed(&mut self, field: Field, value: i32) -> Result<(), String> {
        let mask = (1u32 << field.width) - 1;
        self.set(field, value as u32 & mask)
    }

    fn section(&self, name: &str) -> Option<&[u32]> {
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

    fn section_mut(&mut self, name: &str) -> Option<&mut [u32]> {
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
    regs::SECTIONS.iter().find_map(|&(name, first, count)| {
        absolute
            .checked_sub(first)
            .filter(|&index| index < count)
            .map(|index| (name, index))
    })
}

pub struct Vepu510H265 {
    svc: MppService,
    input: DmaBuf,
    output: DmaBuf,
    recon: [DmaBuf; 2],
    colmv: [DmaBuf; 2],
    thumb: [DmaBuf; 2],
    smear: [DmaBuf; 2],
    md_info: DmaBuf,
    idr_done: bool,
    frame_number: u32,
    input_fd_override: Option<i32>,
}

impl Vepu510H265 {
    pub fn open() -> Result<Self, String> {
        Self::open_bit_depth(8)
    }

    pub fn open_bit_depth(bit_depth: u8) -> Result<Self, String> {
        if bit_depth != 8 {
            return Err(format!(
                "VEPU510 H.265 {bit_depth}-bit input is unsupported; lf1 C MPP fails format/color-space allocation and returns no packet"
            ));
        }
        require_video_encode(VideoEncodeHal::Vepu510)?;
        let input = DmaBuf::alloc("system", INPUT_SIZE, false)
            .map_err(|e| format!("allocate VEPU510 H.265 input: {e}"))?;
        let output = DmaBuf::alloc("system", OUTPUT_SIZE, false)
            .map_err(|e| format!("allocate VEPU510 H.265 output: {e}"))?;
        let mut recon = [
            DmaBuf::alloc("system", RECON_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 H.265 reconstruction 0: {e}"))?,
            DmaBuf::alloc("system", RECON_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 H.265 reconstruction 1: {e}"))?,
        ];
        let mut colmv = [
            DmaBuf::alloc("system", COLMV_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 H.265 colmv 0: {e}"))?,
            DmaBuf::alloc("system", COLMV_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 H.265 colmv 1: {e}"))?,
        ];
        let mut thumb = [
            DmaBuf::alloc("system", THUMB_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 H.265 thumbnail 0: {e}"))?,
            DmaBuf::alloc("system", THUMB_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 H.265 thumbnail 1: {e}"))?,
        ];
        let mut smear = [
            DmaBuf::alloc("system", SMEAR_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 H.265 smear 0: {e}"))?,
            DmaBuf::alloc("system", SMEAR_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 H.265 smear 1: {e}"))?,
        ];
        let mut md_info = DmaBuf::alloc("system", MD_INFO_SIZE, false)
            .map_err(|e| format!("allocate VEPU510 H.265 motion info: {e}"))?;
        for buffer in recon
            .iter_mut()
            .chain(colmv.iter_mut())
            .chain(thumb.iter_mut())
            .chain(smear.iter_mut())
            .chain(std::iter::once(&mut md_info))
        {
            zero_buffer(buffer)?;
        }
        Ok(Self {
            svc: MppService::open(Client::RkvEnc as u32)
                .map_err(|e| format!("open RK3576 H.265 encoder: {e}"))?,
            input,
            output,
            recon,
            colmv,
            thumb,
            smear,
            md_info,
            idr_done: false,
            frame_number: 0,
            input_fd_override: None,
        })
    }

    pub fn open_tile_columns(columns: u32) -> Result<Self, String> {
        if columns > 1 {
            return Err(format!(
                "VEPU510 H.265 tiles are unavailable on RK3576: C only enables PPS tiles on RK3566/RK3568/RK3588, requested {columns} columns"
            ));
        }
        Self::open()
    }

    /// Streaming form of the captured fixed-QP path: one IDR followed by P
    /// pictures for a 320x240 QP26 Main GOP6 sequence.
    pub fn encode_fixed_qp_next_320x240(
        &mut self,
        nv12: &[u8],
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        let direct_input = self.input_fd_override.is_some();
        if !direct_input && nv12.len() < INPUT_SIZE {
            return Err(format!(
                "VEPU510 H.265 input is {} bytes, expected at least {INPUT_SIZE}",
                nv12.len()
            ));
        }
        let config = lf1_h265_config()?;
        let prefix = lf1_fixed_qp_prefix(&config, QP, GOP)?;
        let input_fd = self.input_fd_override.unwrap_or_else(|| self.input.fd());
        let input_data = if direct_input {
            &[]
        } else {
            &nv12[..INPUT_SIZE]
        };
        if !self.idr_done {
            let registers = RegisterFile::captured_fixqp_idr_320x240(
                input_fd as u32,
                self.output.fd() as u32,
                self.recon[0].fd() as u32,
                self.colmv[0].fd() as u32,
                self.thumb[0].fd() as u32,
                self.smear[0].fd() as u32,
                self.md_info.fd() as u32,
            )?;
            let stream = self.run_job(input_data, &registers, &prefix)?;
            self.idr_done = true;
            self.frame_number = 0;
            return Ok((stream, true, 0, 0));
        }
        self.frame_number = self.frame_number.saturating_add(1);
        let current = self.frame_number as usize & 1;
        let reference = 1 - current;
        let registers = RegisterFile::captured_fixqp_p_320x240(
            input_fd as u32,
            self.output.fd() as u32,
            self.recon[current].fd() as u32,
            self.recon[reference].fd() as u32,
            self.colmv[current].fd() as u32,
            self.colmv[reference].fd() as u32,
            self.thumb[current].fd() as u32,
            self.thumb[reference].fd() as u32,
            self.smear[current].fd() as u32,
            self.smear[reference].fd() as u32,
            self.md_info.fd() as u32,
            self.frame_number,
        )?;
        let stream = self.run_job(input_data, &registers, &[])?;
        Ok((
            stream,
            false,
            self.frame_number,
            self.frame_number as i32 * 2,
        ))
    }

    pub fn encode_fixed_qp_next_320x240_fd(
        &mut self,
        input_fd: i32,
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        if input_fd < 0 {
            return Err("VEPU510 H.265 input fd is invalid".to_string());
        }
        self.input_fd_override = Some(input_fd);
        self.encode_fixed_qp_next_320x240(&[])
    }

    pub fn encode_fixed_qp_idr_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        if self.idr_done {
            return Err("this VEPU510 H.265 session already encoded its IDR".into());
        }
        if nv12.len() < INPUT_SIZE {
            return Err(format!(
                "VEPU510 H.265 input is {} bytes, expected {INPUT_SIZE}",
                nv12.len()
            ));
        }
        let config = lf1_h265_config()?;
        let prefix = lf1_fixed_qp_prefix(&config, QP, GOP)?;
        let registers = RegisterFile::captured_fixqp_idr_320x240(
            self.input.fd() as u32,
            self.output.fd() as u32,
            self.recon[0].fd() as u32,
            self.colmv[0].fd() as u32,
            self.thumb[0].fd() as u32,
            self.smear[0].fd() as u32,
            self.md_info.fd() as u32,
        )?;
        let stream = self.run_job(&nv12[..INPUT_SIZE], &registers, &prefix)?;
        self.idr_done = true;
        Ok(stream)
    }

    pub fn encode_fixed_qp_gop2_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        self.encode_fixed_qp_gop_with_split(nv12, 2, &[], encoder::SplitMode::None, 0, 0)
            .map(|(stream, _)| stream)
    }

    pub fn encode_fixed_qp_gop6_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        self.encode_fixed_qp_gop_with_split(nv12, 6, &[], encoder::SplitMode::None, 0, 0)
            .map(|(stream, _)| stream)
    }

    pub fn encode_fixed_qp_split_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
        mode: encoder::SplitMode,
        arg: u32,
        split_out: u32,
    ) -> Result<(Vec<u8>, Vec<usize>), String> {
        self.encode_fixed_qp_gop_with_split(nv12, frames, &[], mode, arg, split_out)
    }

    pub fn encode_fixed_qp_roi_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
        rois: &[RoiRegion],
    ) -> Result<Vec<u8>, String> {
        self.encode_fixed_qp_gop_with_split(nv12, frames, rois, encoder::SplitMode::None, 0, 0)
            .map(|(stream, _)| stream)
    }

    pub fn encode_fixed_qp_gdr_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
        gop: u32,
        refresh_mode: crate::hal::h265e::RefreshMode,
        refresh_num: u32,
    ) -> Result<Vec<u8>, String> {
        if self.idr_done {
            return Err("this VEPU510 H.265 session already encoded its IDR".into());
        }
        if !(1..=32_767).contains(&frames) {
            return Err(format!(
                "VEPU510 H.265 GDR length {frames} is outside 1..=32767"
            ));
        }
        if gop == 0 || refresh_num == 0 {
            return Err("VEPU510 H.265 GDR needs non-zero GOP and refresh count".into());
        }
        let expected = INPUT_SIZE * frames;
        if nv12.len() < expected {
            return Err(format!(
                "VEPU510 H.265 GDR input is {} bytes, expected {expected}",
                nv12.len()
            ));
        }
        let axis_lines = match refresh_mode {
            crate::hal::h265e::RefreshMode::Row => HEIGHT.div_ceil(64),
            crate::hal::h265e::RefreshMode::Col => WIDTH.div_ceil(64),
        } as u32;
        let refresh_length = axis_lines.div_ceil(refresh_num).min(gop);
        let config = lf1_h265_config()?;
        let prefix = lf1_fixed_qp_prefix(&config, QP, gop)?;
        let mut recovery_prefix = encoder::build_headers(&config).map_err(|e| e.to_string())?;
        recovery_prefix.extend(sei::recovery_point(refresh_length as i32 - 1));

        let mut idr = RegisterFile::captured_fixqp_idr_320x240(
            self.input.fd() as u32,
            self.output.fd() as u32,
            self.recon[0].fd() as u32,
            self.colmv[0].fd() as u32,
            self.thumb[0].fd() as u32,
            self.smear[0].fd() as u32,
            self.md_info.fd() as u32,
        )?;
        idr.apply_gop_smear(0, gop)?;
        let mut stream = self.run_job(&nv12[..INPUT_SIZE], &idr, &prefix)?;
        for frame_number in 1..frames {
            let current = frame_number & 1;
            let reference = 1 - current;
            let mut p = RegisterFile::captured_fixqp_p_320x240(
                self.input.fd() as u32,
                self.output.fd() as u32,
                self.recon[current].fd() as u32,
                self.recon[reference].fd() as u32,
                self.colmv[current].fd() as u32,
                self.colmv[reference].fd() as u32,
                self.thumb[current].fd() as u32,
                self.thumb[reference].fd() as u32,
                self.smear[current].fd() as u32,
                self.smear[reference].fd() as u32,
                self.md_info.fd() as u32,
                frame_number as u32,
            )?;
            p.apply_gop_smear(frame_number as u32, gop)?;
            let start = frame_number * INPUT_SIZE;
            let end = start + INPUT_SIZE;
            let frame_prefix = if frame_number as u32 % gop == 0 {
                recovery_prefix.as_slice()
            } else {
                &[]
            };
            stream.extend(self.run_job(&nv12[start..end], &p, frame_prefix)?);
        }
        self.idr_done = true;
        Ok(stream)
    }

    pub fn encode_cbr_gop6_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        self.encode_rate_gop6_320x240(nv12, RcMode::Cbr)
    }

    pub fn encode_vbr_gop6_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        self.encode_rate_gop6_320x240(nv12, RcMode::Vbr)
    }

    pub fn encode_avbr_gop6_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        self.encode_rate_gop6_320x240(nv12, RcMode::Avbr)
    }

    fn encode_rate_gop6_320x240(
        &mut self,
        nv12: &[u8],
        rc_mode: RcMode,
    ) -> Result<Vec<u8>, String> {
        if self.idr_done {
            return Err("this VEPU510 H.265 session already encoded its IDR".into());
        }
        let expected = INPUT_SIZE * 6;
        if nv12.len() < expected {
            return Err(format!(
                "VEPU510 H.265 rate-control input is {} bytes, expected {expected}",
                nv12.len()
            ));
        }
        let mut config = lf1_h265_config()?;
        config.rc_mode = match rc_mode {
            RcMode::Cbr => encoder::RateControl::Cbr,
            RcMode::Vbr | RcMode::Avbr => encoder::RateControl::Vbr,
        };
        let prefix = lf1_rate_prefix(&config, GOP, rc_mode)?;
        let mut model_config = rc_model::Config::cbr(CBR_TARGET, CBR_MIN, CBR_MAX, GOP as i32);
        model_config.quality_init = QP as i32;
        model_config.quality_min = 10;
        model_config.quality_max = 51;
        model_config.i_quality_delta = 4;
        model_config.total_mb = ((WIDTH + 63) / 64) as i32 * ((HEIGHT + 63) / 64) as i32 * 64;
        model_config.mode = match rc_mode {
            RcMode::Cbr => rc_model::Mode::Cbr,
            RcMode::Vbr => rc_model::Mode::Vbr,
            RcMode::Avbr => rc_model::Mode::Avbr,
        };
        let mut model = rc_model::Model::new(model_config);

        let mut stream = Vec::new();
        for frame_number in 0..6 {
            let is_intra = frame_number == 0;
            let frame_type = if is_intra {
                rc_model::FrameType::Intra
            } else {
                rc_model::FrameType::InterP
            };
            let (bit_target, qp_target) = model.start(frame_type);
            let plan = RateControlPlan {
                bit_target,
                qp_target: qp_target as u32,
                qp_min: if is_intra { 15 } else { 10 },
                qp_max: 51,
                qp_delta: 2,
            };
            let mut registers = if is_intra {
                RegisterFile::captured_fixqp_idr_320x240(
                    self.input.fd() as u32,
                    self.output.fd() as u32,
                    self.recon[0].fd() as u32,
                    self.colmv[0].fd() as u32,
                    self.thumb[0].fd() as u32,
                    self.smear[0].fd() as u32,
                    self.md_info.fd() as u32,
                )?
            } else {
                let current = frame_number & 1;
                let reference = 1 - current;
                RegisterFile::captured_fixqp_p_320x240(
                    self.input.fd() as u32,
                    self.output.fd() as u32,
                    self.recon[current].fd() as u32,
                    self.recon[reference].fd() as u32,
                    self.colmv[current].fd() as u32,
                    self.colmv[reference].fd() as u32,
                    self.thumb[current].fd() as u32,
                    self.thumb[reference].fd() as u32,
                    self.smear[current].fd() as u32,
                    self.smear[reference].fd() as u32,
                    self.md_info.fd() as u32,
                    frame_number as u32,
                )?
            };
            registers.apply_rate_control(&plan, is_intra)?;
            let start = frame_number * INPUT_SIZE;
            let end = start + INPUT_SIZE;
            let frame_prefix = if is_intra { prefix.as_slice() } else { &[] };
            let (frame_stream, status, _) =
                self.run_job_with_status(&nv12[start..end], &registers, frame_prefix, 0)?;
            let measured = measured_from_status(&status);
            model.end_with(frame_type, bit_target, measured);
            model.accept_frame();
            stream.extend(frame_stream);
        }
        self.idr_done = true;
        Ok(stream)
    }

    fn encode_fixed_qp_gop_with_split(
        &mut self,
        nv12: &[u8],
        frames: usize,
        rois: &[RoiRegion],
        split_mode: encoder::SplitMode,
        split_arg: u32,
        split_out: u32,
    ) -> Result<(Vec<u8>, Vec<usize>), String> {
        if self.idr_done {
            return Err("this VEPU510 H.265 session already encoded its IDR".into());
        }
        if !(1..=6).contains(&frames) {
            return Err(format!(
                "captured VEPU510 H.265 GOP length {frames} is outside 1..=6"
            ));
        }
        let expected = INPUT_SIZE * frames;
        if nv12.len() < expected {
            return Err(format!(
                "VEPU510 H.265 GOP input is {} bytes, expected {}",
                nv12.len(),
                expected
            ));
        }
        let config = lf1_h265_config()?;
        let prefix = lf1_fixed_qp_prefix(&config, QP, GOP)?;
        let mut idr = RegisterFile::captured_fixqp_idr_320x240(
            self.input.fd() as u32,
            self.output.fd() as u32,
            self.recon[0].fd() as u32,
            self.colmv[0].fd() as u32,
            self.thumb[0].fd() as u32,
            self.smear[0].fd() as u32,
            self.md_info.fd() as u32,
        )?;
        idr.apply_roi_regions(rois)?;
        idr.apply_slice_split(split_mode, split_arg, split_out)?;
        let (mut stream, _, mut packet_lengths) =
            self.run_job_with_status(&nv12[..INPUT_SIZE], &idr, &prefix, split_out)?;
        for frame_number in 1..frames {
            let current = frame_number & 1;
            let reference = 1 - current;
            let mut p = RegisterFile::captured_fixqp_p_320x240(
                self.input.fd() as u32,
                self.output.fd() as u32,
                self.recon[current].fd() as u32,
                self.recon[reference].fd() as u32,
                self.colmv[current].fd() as u32,
                self.colmv[reference].fd() as u32,
                self.thumb[current].fd() as u32,
                self.thumb[reference].fd() as u32,
                self.smear[current].fd() as u32,
                self.smear[reference].fd() as u32,
                self.md_info.fd() as u32,
                frame_number as u32,
            )?;
            p.apply_roi_regions(rois)?;
            p.apply_slice_split(split_mode, split_arg, split_out)?;
            let start = frame_number * INPUT_SIZE;
            let end = start + INPUT_SIZE;
            let (frame_stream, _, frame_packets) =
                self.run_job_with_status(&nv12[start..end], &p, &[], split_out)?;
            stream.extend(frame_stream);
            packet_lengths.extend(frame_packets);
        }
        self.idr_done = true;
        Ok((stream, packet_lengths))
    }

    fn run_job(
        &mut self,
        nv12: &[u8],
        registers: &RegisterFile,
        prefix: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.run_job_with_status(nv12, registers, prefix, 0)
            .map(|(stream, _, _)| stream)
    }

    fn run_job_with_status(
        &mut self,
        nv12: &[u8],
        registers: &RegisterFile,
        prefix: &[u8],
        split_out: u32,
    ) -> Result<(Vec<u8>, [u32; regs::STATUS_WORDS], Vec<usize>), String> {
        let input_fd = if let Some(input_fd) = self.input_fd_override.take() {
            input_fd
        } else {
            self.input
                .sync_write_begin()
                .map_err(|e| format!("VEPU510 H.265 input sync begin: {e}"))?;
            self.input.as_mut_slice()[..INPUT_SIZE].copy_from_slice(nv12);
            self.input
                .sync_write_end()
                .map_err(|e| format!("VEPU510 H.265 input flush: {e}"))?;
            self.input.fd()
        };

        self.output
            .sync_write_begin()
            .map_err(|e| format!("VEPU510 H.265 output sync begin: {e}"))?;
        self.output.as_mut_slice()[..prefix.len()].copy_from_slice(prefix);
        self.output
            .sync_write_end()
            .map_err(|e| format!("VEPU510 H.265 output prefix flush: {e}"))?;

        let luma_size = WIDTH * HEIGHT;
        let offsets = [
            RegOffsetInfo {
                reg_idx: 161,
                offset: luma_size as u32,
            },
            RegOffsetInfo {
                reg_idx: 162,
                offset: luma_size as u32,
            },
            RegOffsetInfo {
                reg_idx: 172,
                offset: (self.output.len() - 1) as u32,
            },
            RegOffsetInfo {
                reg_idx: 174,
                offset: prefix.len() as u32,
            },
            RegOffsetInfo {
                reg_idx: 164,
                offset: FBC_HEADER_SIZE as u32,
            },
            RegOffsetInfo {
                reg_idx: 166,
                offset: FBC_HEADER_SIZE as u32,
            },
        ];

        let mut int_state = [0u32; 1];
        let mut status = [0u32; regs::STATUS_WORDS];
        let mut batch = Batch::new();
        let mut registers = registers.clone();
        for index in 4..=6 {
            registers.frame[index] = input_fd as u32;
        }
        batch.reg_write(&registers.ctl, CTL_OFFSET);
        batch.reg_write(&registers.frame, FRAME_OFFSET);
        batch.reg_write(&registers.rc, RC_OFFSET);
        batch.reg_write(&registers.param, PARAM_OFFSET);
        batch.reg_write(&registers.sqi, SQI_OFFSET);
        batch.reg_write(&registers.scl, SCL_OFFSET);
        batch.reg_offsets(&offsets);
        batch.reg_read(&mut int_state, HW_STATUS_OFFSET);
        batch.reg_read(&mut status, STATUS_OFFSET);
        self.svc
            .submit_batch_recovering(&mut batch)
            .map_err(|e| format!("VEPU510 H.265 submit: {e}"))?;
        let mut packet_lengths = Vec::new();
        if split_out & encoder::MPP_ENC_SPLIT_OUT_LOWDELAY != 0 {
            let mut complete = false;
            for _ in 0..=4096 {
                let mut info = [SliceInfo::default()];
                let count = match self.svc.poll_irq(&mut info) {
                    Ok(count) => count,
                    Err(error) => {
                        let _ = self.svc.restart_after_hardware_error();
                        return Err(format!("VEPU510 H.265 slice poll: {error}"));
                    }
                };
                for slice in &info[..count] {
                    let (length, last) = slice.vepu510();
                    if length == 0 {
                        let _ = self.svc.restart_after_hardware_error();
                        return Err("VEPU510 H.265 slice FIFO returned an empty slice".into());
                    }
                    packet_lengths.push(length as usize);
                    complete |= last;
                }
                if complete {
                    break;
                }
            }
            if !complete {
                let _ = self.svc.restart_after_hardware_error();
                return Err("VEPU510 H.265 slice FIFO did not report the last slice".into());
            }
        } else {
            self.svc
                .poll_hw_finish_recovering()
                .map_err(|e| format!("VEPU510 H.265 poll: {e}"))?;
        }

        if int_state[0] & 1 == 0 {
            let error = status_execution_error(&status);
            let _ = self.svc.restart_after_hardware_error();
            return Err(format!(
                "VEPU510 H.265 completed without enc_done (int_sta=0x{:08x}, status={error})",
                int_state[0]
            ));
        }
        let payload_len = status[0] as usize;
        let stream_len = prefix
            .len()
            .checked_add(payload_len)
            .ok_or_else(|| "VEPU510 H.265 stream length overflow".to_string())?;
        if payload_len == 0 || stream_len > self.output.len() {
            let _ = self.svc.restart_after_hardware_error();
            return Err(format!(
                "invalid VEPU510 H.265 payload length {payload_len} (capacity {})",
                self.output.len()
            ));
        }
        if !packet_lengths.is_empty() {
            if !prefix.is_empty() {
                packet_lengths[0] = packet_lengths[0]
                    .checked_add(prefix.len())
                    .ok_or_else(|| "VEPU510 H.265 first packet length overflow".to_string())?;
            }
            let total = packet_lengths.iter().try_fold(0usize, |sum, length| {
                sum.checked_add(*length)
                    .ok_or_else(|| "VEPU510 H.265 packet length overflow".to_string())
            })?;
            if total != stream_len {
                let _ = self.svc.restart_after_hardware_error();
                return Err(format!(
                    "VEPU510 H.265 slice lengths sum to {total}, STATUS says {stream_len}"
                ));
            }
        }
        self.output
            .sync_read_begin()
            .map_err(|e| format!("VEPU510 H.265 output invalidate: {e}"))?;
        let stream = self.output.as_slice()[..stream_len].to_vec();
        self.output
            .sync_read_end()
            .map_err(|e| format!("VEPU510 H.265 output sync end: {e}"))?;
        Ok((stream, status, packet_lengths))
    }
}

fn lf1_fixed_qp_prefix(config: &EncoderConfig, qp: u32, gop: u32) -> Result<Vec<u8>, String> {
    let line = sei::RcCfgLine {
        model_name: "default",
        mode_name: "fixqp",
        bps_target: 300_000,
        bps_min: 250_000,
        bps_max: 350_000,
        igop: gop as i64,
        init_quality: qp as i32,
        min_quality: qp as i32,
        max_quality: qp as i32,
        min_i_quality: qp as i32,
        max_i_quality: qp as i32,
        i_quality_delta: 0,
    };
    lf1_prefix(config, &line)
}

fn lf1_rate_prefix(config: &EncoderConfig, gop: u32, mode: RcMode) -> Result<Vec<u8>, String> {
    let line = sei::RcCfgLine {
        model_name: "default",
        mode_name: match mode {
            RcMode::Cbr => "cbr",
            RcMode::Vbr => "vbr",
            RcMode::Avbr => "avbr",
        },
        bps_target: CBR_TARGET as i64,
        bps_min: CBR_MIN as i64,
        bps_max: CBR_MAX as i64,
        igop: gop as i64,
        init_quality: QP as i32,
        min_quality: 10,
        max_quality: 51,
        min_i_quality: 15,
        max_i_quality: 51,
        i_quality_delta: 4,
    };
    lf1_prefix(config, &line)
}

fn lf1_prefix(config: &EncoderConfig, line: &sei::RcCfgLine) -> Result<Vec<u8>, String> {
    let mut out = encoder::build_headers(config).map_err(|e| e.to_string())?;
    out.extend(sei::user_data_unregistered(
        &sei::VERSION_UUID,
        LF1_VERSION_TEXT,
    ));
    out.extend(sei::user_data_unregistered(
        &sei::RC_UUID,
        sei::rc_cfg_line(line).as_bytes(),
    ));
    Ok(out)
}

fn lf1_h265_config() -> Result<EncoderConfig, String> {
    let mut config = EncoderConfig::new(WIDTH, HEIGHT);
    config.max_cu_size = 32;
    config.tu_log2_max_extra = 1;
    config.force_cu_qp_delta = true;
    config.validate().map_err(|error| error.to_string())?;
    Ok(config)
}

fn zero_buffer(buffer: &mut DmaBuf) -> Result<(), String> {
    buffer
        .sync_write_begin()
        .map_err(|e| format!("VEPU510 H.265 scratch sync begin: {e}"))?;
    buffer.as_mut_slice().fill(0);
    buffer
        .sync_write_end()
        .map_err(|e| format!("VEPU510 H.265 scratch flush: {e}"))
}

fn status_execution_error(status: &[u32]) -> u32 {
    let field = regs::STATUS_ST_ENC_ST_ENC;
    let word = status[field.word - regs::STATUS_OFFSET];
    (word >> field.shift) & ((1u32 << field.width) - 1)
}

fn measured_from_status(status: &[u32]) -> rc_model::Measured {
    let at = |word: usize| status.get(word - regs::STATUS_OFFSET).copied().unwrap_or(0);
    let mb64 = ((WIDTH + 63) / 64) * ((HEIGHT + 63) / 64);
    let quality = (at(4099) / ((mb64 << 6) as u32)) as i32;
    let madi = {
        let count = at(4171) & 0x007f_ffff;
        if count > 0 {
            (at(4170) / count) as i32
        } else {
            0
        }
    };
    let madp = {
        let count = at(4169) & 0x001f_ffff;
        if count > 0 {
            (at(4168) / count) as i32
        } else {
            0
        }
    };
    rc_model::Measured {
        bits: at(4096) as i32 * 8,
        quality,
        madi,
        madp,
        iblk4_prop: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_match_the_lf1_h265_trace() {
        assert_eq!(template::CTL.len(), CTL_WORDS);
        assert_eq!(template::FRAME.len(), FRAME_WORDS);
        assert_eq!(template::RC.len(), RC_WORDS);
        assert_eq!(template::PARAM.len(), PARAM_WORDS);
        assert_eq!(template::SQI.len(), SQI_WORDS);
        assert_eq!(template::SCL.len(), SCL_WORDS);
        assert_eq!(template::STATUS.len(), regs::STATUS_WORDS);
        assert_eq!(p_template::FRAME.len(), FRAME_WORDS);
        assert_eq!(p_template::SQI.len(), SQI_WORDS);
    }

    #[test]
    fn captured_builder_patches_every_moving_address() {
        let regs = RegisterFile::captured_fixqp_idr_320x240(9, 10, 14, 16, 15, 17, 11).unwrap();
        assert_eq!(regs.ctl, template::CTL);
        assert_eq!(regs.frame, template::FRAME);
        assert_eq!(regs.rc, template::RC);
        assert_eq!(regs.param, template::PARAM);
        assert_eq!(regs.sqi, template::SQI);
        assert_eq!(regs.scl, template::SCL);
        assert_eq!(&regs.frame[4..=6], &[9, 9, 9]);
        assert_eq!(&regs.frame[7..=10], &[14, 14, 14, 14]);
        assert_eq!(&regs.frame[11..=12], &[16, 16]);
        assert_eq!(&regs.frame[13..=14], &[15, 15]);
        assert_eq!(regs.frame[15], 11);
        assert_eq!(&regs.frame[16..=19], &[10, 10, 10, 10]);
        assert_eq!(&regs.frame[28..=29], &[17, 17]);
    }

    #[test]
    fn p_builder_reproduces_the_lf1_job() {
        let regs =
            RegisterFile::captured_fixqp_p_320x240(9, 10, 18, 14, 20, 16, 19, 15, 21, 17, 11, 1)
                .unwrap();
        assert_eq!(regs.ctl, p_template::CTL);
        assert_eq!(regs.frame, p_template::FRAME);
        assert_eq!(regs.rc, p_template::RC);
        assert_eq!(regs.param, p_template::PARAM);
        assert_eq!(regs.sqi, p_template::SQI);
        assert_eq!(regs.scl, p_template::SCL);
    }

    #[test]
    fn later_p_pictures_advance_poc_and_load_smear() {
        let regs =
            RegisterFile::captured_fixqp_p_320x240(9, 10, 14, 18, 16, 20, 15, 19, 17, 21, 11, 2)
                .unwrap();
        assert_eq!(regs.get(regs::H265_SYNT_SLI2_SLI_POC_LSB).unwrap(), 2);
        assert_eq!(regs.get(regs::SQI_SMEAR_OPT_CFG0_SMEAR_LOAD_EN).unwrap(), 1);
    }

    #[test]
    fn cbr_plan_matches_the_lf1_idr_rc_words() {
        let mut regs = RegisterFile::captured_fixqp_idr_320x240(9, 10, 14, 16, 15, 17, 11).unwrap();
        regs.apply_rate_control(
            &RateControlPlan {
                bit_target: 86_955,
                qp_target: 26,
                qp_min: 15,
                qp_max: 51,
                qp_delta: 2,
            },
            true,
        )
        .unwrap();
        assert_eq!(regs.get(regs::FRAME_ENC_PIC_PIC_QP).unwrap(), 26);
        assert_eq!(regs.get(regs::H265_SYNT_SLI1_SLI_QP).unwrap(), 26);
        assert_eq!(regs.get(regs::FRAME_RC_QP_RC_QP_RANGE).unwrap(), 2);
        assert_eq!(regs.get(regs::FRAME_RC_QP_RC_MIN_QP).unwrap(), 15);
        assert_eq!(regs.get(regs::FRAME_RC_QP_RC_MAX_QP).unwrap(), 51);
        assert_eq!(regs.get(regs::FRAME_RC_TGT_CTU_EBIT).unwrap(), 17_391);
        assert_eq!(
            &regs.rc[..8],
            &[
                0x0020_83fe,
                0,
                0xffff_e578,
                0xffff_f2bc,
                0x0000_0d44,
                0x0000_1a88,
                0x7fff_ffff,
                0x7fff_ffff,
            ]
        );
    }

    #[test]
    fn split_and_lowdelay_match_the_c_vpu510_words() {
        let mut regs = RegisterFile::default();
        regs.apply_slice_split(encoder::SplitMode::ByCtu, 8, 0)
            .unwrap();
        assert_eq!(regs.get(regs::FRAME_SLI_SPLT_SLI_SPLT).unwrap(), 1);
        assert_eq!(regs.get(regs::FRAME_SLI_SPLT_SLI_SPLT_MODE).unwrap(), 1);
        assert_eq!(regs.get(regs::FRAME_SLI_CNUM_SLI_SPLT_CNUM_M1).unwrap(), 7);
        assert_eq!(regs.get(regs::FRAME_ENC_PIC_SLEN_FIFO).unwrap(), 0);
        assert_eq!(regs.get(regs::CTL_INT_EN_VSLC_DONE_EN).unwrap(), 0);

        let mut regs = RegisterFile::default();
        regs.apply_slice_split(
            encoder::SplitMode::ByByte,
            256,
            encoder::MPP_ENC_SPLIT_OUT_LOWDELAY,
        )
        .unwrap();
        assert_eq!(regs.get(regs::FRAME_SLI_BYTE_SLI_SPLT_BYTE).unwrap(), 256);
        assert_eq!(regs.get(regs::FRAME_ENC_PIC_SLEN_FIFO).unwrap(), 1);
        assert_eq!(regs.get(regs::CTL_INT_EN_VSLC_DONE_EN).unwrap(), 1);
    }

    #[test]
    fn gdr_smear_flags_follow_the_c_gop_positions() {
        let expected = [(0, 0), (0, 1), (1, 1), (1, 0)];
        for (position, (load, store)) in expected.into_iter().enumerate() {
            let mut regs = RegisterFile::default();
            regs.apply_gop_smear(position as u32, 4).unwrap();
            assert_eq!(
                regs.get(regs::SQI_SMEAR_OPT_CFG0_SMEAR_LOAD_EN).unwrap(),
                load
            );
            assert_eq!(
                regs.get(regs::SQI_SMEAR_OPT_CFG0_SMEAR_STOR_EN).unwrap(),
                store
            );
        }
        let mut prefix = encoder::build_headers(&lf1_h265_config().unwrap()).unwrap();
        prefix.extend(sei::recovery_point(1));
        assert_eq!(prefix.len(), 89);
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

    #[test]
    fn lf1_prefix_has_the_c_packet_prefix_length() {
        let prefix = lf1_fixed_qp_prefix(&lf1_h265_config().unwrap(), QP, GOP).unwrap();
        assert_eq!(prefix.len(), 270);
    }

    #[test]
    fn ten_bit_input_is_rejected_before_hardware_allocation() {
        let error = match Vepu510H265::open_bit_depth(10) {
            Ok(_) => panic!("10-bit unexpectedly opened a hardware session"),
            Err(error) => error,
        };
        assert!(error.contains("10-bit"));
        assert!(error.contains("unsupported"));
    }

    #[test]
    fn tile_requests_are_rejected_on_rk3576() {
        let error = match Vepu510H265::open_tile_columns(2) {
            Ok(_) => panic!("tiles unexpectedly opened a hardware session"),
            Err(error) => error,
        };
        assert!(error.contains("tiles are unavailable"));
        assert!(error.contains("RK3576"));
    }
}
