//! First RK3576 VEPU510 H.265 hardware path.
//!
//! The captured job is a fixed-QP 320x240 Main-profile IDR. Software supplies
//! VPS/SPS/PPS and the two prefix SEIs; the hardware writes the slice payload.

use crate::codec::h265::encoder::{self, EncoderConfig};
use crate::codec::h265::sei;
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{require_video_encode, VideoEncodeHal};
use crate::service::{Batch, Client, MppService, RegOffsetInfo};

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
}

impl Vepu510H265 {
    pub fn open() -> Result<Self, String> {
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
        })
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
        self.encode_fixed_qp_gop_320x240(nv12, 2)
    }

    pub fn encode_fixed_qp_gop6_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        self.encode_fixed_qp_gop_320x240(nv12, 6)
    }

    fn encode_fixed_qp_gop_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
    ) -> Result<Vec<u8>, String> {
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
        let idr = RegisterFile::captured_fixqp_idr_320x240(
            self.input.fd() as u32,
            self.output.fd() as u32,
            self.recon[0].fd() as u32,
            self.colmv[0].fd() as u32,
            self.thumb[0].fd() as u32,
            self.smear[0].fd() as u32,
            self.md_info.fd() as u32,
        )?;
        let mut stream = self.run_job(&nv12[..INPUT_SIZE], &idr, &prefix)?;
        for frame_number in 1..frames {
            let current = frame_number & 1;
            let reference = 1 - current;
            let p = RegisterFile::captured_fixqp_p_320x240(
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
            let start = frame_number * INPUT_SIZE;
            let end = start + INPUT_SIZE;
            stream.extend(self.run_job(&nv12[start..end], &p, &[])?);
        }
        self.idr_done = true;
        Ok(stream)
    }

    fn run_job(
        &mut self,
        nv12: &[u8],
        registers: &RegisterFile,
        prefix: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.input
            .sync_write_begin()
            .map_err(|e| format!("VEPU510 H.265 input sync begin: {e}"))?;
        self.input.as_mut_slice()[..INPUT_SIZE].copy_from_slice(nv12);
        self.input
            .sync_write_end()
            .map_err(|e| format!("VEPU510 H.265 input flush: {e}"))?;

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
        self.svc
            .poll_hw_finish_recovering()
            .map_err(|e| format!("VEPU510 H.265 poll: {e}"))?;

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
        self.output
            .sync_read_begin()
            .map_err(|e| format!("VEPU510 H.265 output invalidate: {e}"))?;
        let stream = self.output.as_slice()[..stream_len].to_vec();
        self.output
            .sync_read_end()
            .map_err(|e| format!("VEPU510 H.265 output sync end: {e}"))?;
        Ok(stream)
    }
}

fn lf1_fixed_qp_prefix(config: &EncoderConfig, qp: u32, gop: u32) -> Result<Vec<u8>, String> {
    let mut out = encoder::build_headers(config).map_err(|e| e.to_string())?;
    out.extend(sei::user_data_unregistered(
        &sei::VERSION_UUID,
        LF1_VERSION_TEXT,
    ));
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
    out.extend(sei::user_data_unregistered(
        &sei::RC_UUID,
        sei::rc_cfg_line(&line).as_bytes(),
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
    fn lf1_prefix_has_the_c_packet_prefix_length() {
        let prefix = lf1_fixed_qp_prefix(&lf1_h265_config().unwrap(), QP, GOP).unwrap();
        assert_eq!(prefix.len(), 270);
    }
}
