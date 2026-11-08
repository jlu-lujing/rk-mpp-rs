//! First RK3576 VEPU510 H.264 hardware path.
//!
//! The current slice supports a fixed-QP 320x240 Baseline IDR followed by one
//! P picture. Both register images are captured from C MPP; only DMA fds are
//! patched at runtime.

use crate::codec::h264::encoder::{
    self, EncoderConfig, GdrMode, RcMode, SplitMode, MPP_ENC_SPLIT_OUT_LOWDELAY,
};
use crate::codec::rc::model as rc_model;
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{require_video_encode, VideoEncodeHal};
use crate::service::{Batch, Client, MppService, RegOffsetInfo, SliceInfo};

use super::h264e;
use super::vepu510::RegisterFile;
use super::vepu510_regs;

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const INPUT_SIZE: usize = WIDTH * HEIGHT * 3 / 2;
const OUTPUT_SIZE: usize = 128 * 1024;
const RECON_SIZE: usize = 128 * 1024;
const THUMB_SIZE: usize = 8 * 1024;
const SMEAR_SIZE: usize = 256;
const FBC_HEADER_SIZE: usize = 8 * 1024;
const CTL_OFFSET: u32 = 0;
const FRAME_OFFSET: u32 = 624;
const RC_OFFSET: u32 = 4096;
const PARAM_OFFSET: u32 = 5888;
const SQI_OFFSET: u32 = 8192;
const SCL_OFFSET: u32 = 8704;
const HW_STATUS_OFFSET: u32 = 0x2c;
const STATUS_OFFSET: u32 = 16 * 1024;
const STATUS_WORDS: usize = 148;

struct EncodedFrame {
    stream: Vec<u8>,
    packet_lengths: Vec<usize>,
}

struct EncodedGop {
    stream: Vec<u8>,
    packet_lengths: Vec<usize>,
}

struct JobOutput {
    stream: Vec<u8>,
    status: [u32; STATUS_WORDS],
    packet_lengths: Vec<usize>,
}

pub struct Vepu510H264 {
    svc: MppService,
    input: DmaBuf,
    output: DmaBuf,
    recon: [DmaBuf; 2],
    thumb: [DmaBuf; 2],
    smear: [DmaBuf; 2],
    idr_done: bool,
}

impl Vepu510H264 {
    pub fn open() -> Result<Self, String> {
        require_video_encode(VideoEncodeHal::Vepu510)?;
        let input = DmaBuf::alloc("system", INPUT_SIZE, false)
            .map_err(|e| format!("allocate VEPU510 input: {e}"))?;
        let output = DmaBuf::alloc("system", OUTPUT_SIZE, false)
            .map_err(|e| format!("allocate VEPU510 output: {e}"))?;
        let mut recon = [
            DmaBuf::alloc("system", RECON_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 reconstruction 0: {e}"))?,
            DmaBuf::alloc("system", RECON_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 reconstruction 1: {e}"))?,
        ];
        let mut thumb = [
            DmaBuf::alloc("system", THUMB_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 thumbnail 0: {e}"))?,
            DmaBuf::alloc("system", THUMB_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 thumbnail 1: {e}"))?,
        ];
        let mut smear = [
            DmaBuf::alloc("system", SMEAR_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 smear 0: {e}"))?,
            DmaBuf::alloc("system", SMEAR_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 smear 1: {e}"))?,
        ];
        for buffer in recon
            .iter_mut()
            .chain(thumb.iter_mut())
            .chain(smear.iter_mut())
        {
            zero_buffer(buffer)?;
        }
        Ok(Self {
            svc: MppService::open(Client::RkvEnc as u32)
                .map_err(|e| format!("open RK3576 video encoder: {e}"))?,
            input,
            output,
            recon,
            thumb,
            smear,
            idr_done: false,
        })
    }

    pub fn encode_fixed_qp_idr_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        if self.idr_done {
            return Err("this captured VEPU510 session already encoded its IDR".into());
        }
        let stream = self.encode_idr(nv12, &[], SplitMode::None, 0, 0)?.stream;
        self.idr_done = true;
        Ok(stream)
    }

    pub fn encode_fixed_qp_gop2_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        self.encode_fixed_qp_gop_320x240(nv12, 2)
    }

    pub fn encode_fixed_qp_gop_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
    ) -> Result<Vec<u8>, String> {
        self.encode_fixed_qp_gop_with_options(nv12, frames, &[], SplitMode::None, 0, 0)
            .map(|encoded| encoded.stream)
    }

    pub fn encode_fixed_qp_gop_roi_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
        rois: &[encoder::RoiRegion],
    ) -> Result<Vec<u8>, String> {
        self.encode_fixed_qp_gop_with_options(nv12, frames, rois, SplitMode::None, 0, 0)
            .map(|encoded| encoded.stream)
    }

    pub fn encode_fixed_qp_gop_split_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
        split_mode: SplitMode,
        split_arg: u32,
    ) -> Result<Vec<u8>, String> {
        self.encode_fixed_qp_gop_with_options(nv12, frames, &[], split_mode, split_arg, 0)
            .map(|encoded| encoded.stream)
    }

    pub fn encode_fixed_qp_gop_split_lowdelay_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
        split_mode: SplitMode,
        split_arg: u32,
    ) -> Result<(Vec<u8>, Vec<usize>), String> {
        let encoded = self.encode_fixed_qp_gop_with_options(
            nv12,
            frames,
            &[],
            split_mode,
            split_arg,
            MPP_ENC_SPLIT_OUT_LOWDELAY,
        )?;
        Ok((encoded.stream, encoded.packet_lengths))
    }

    pub fn encode_fixed_qp_gdr_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
        gdr_mode: GdrMode,
        gdr_num: u32,
        gop: u32,
    ) -> Result<(Vec<u8>, Vec<usize>), String> {
        if frames == 0 || frames > 32_767 {
            return Err(format!("VEPU510 GDR length {frames} is outside 1..=32767"));
        }
        let expected = INPUT_SIZE
            .checked_mul(frames)
            .ok_or_else(|| "VEPU510 GDR input size overflow".to_string())?;
        if nv12.len() < expected {
            return Err(format!(
                "VEPU510 GDR input is {} bytes, expected {expected}",
                nv12.len()
            ));
        }
        if self.idr_done {
            return Err("this captured VEPU510 session already encoded its IDR".into());
        }

        let mut config = EncoderConfig::new(WIDTH, HEIGHT);
        config.vui_log2_max_mv_length = 15;
        config.gop = gop;
        config.gdr = true;
        config.gdr_mode = gdr_mode;
        config.gdr_num = gdr_num;
        config.validate().map_err(|error| error.to_string())?;
        let refresh_length = config.gdr_refresh_length();
        let header = encoder::build_headers(&config).map_err(|e| e.to_string())?;
        if header.len() != 36 {
            return Err(format!(
                "unexpected no-SEI VEPU510 header length {}",
                header.len()
            ));
        }

        let idr = self.encode_idr(&nv12[..INPUT_SIZE], &[], SplitMode::None, 0, 0)?;
        let mut stream = idr.stream;
        let mut packet_lengths = vec![stream.len()];
        self.idr_done = true;
        let mut state = h264e::FrameState::default();
        for frame_number in 1..frames {
            state = state.next(gop, refresh_length);
            let start = frame_number * INPUT_SIZE;
            let end = start + INPUT_SIZE;
            let refresh = state.is_i_refresh.then_some(gdr_mode);
            let prefix = if state.is_i_recovery {
                header.as_slice()
            } else {
                &[]
            };
            let encoded = self.encode_p(
                &nv12[start..end],
                frame_number as u32,
                &[],
                SplitMode::None,
                0,
                0,
                prefix,
                refresh,
            )?;
            packet_lengths.push(encoded.stream.len());
            stream.extend(encoded.stream);
        }
        Ok((stream, packet_lengths))
    }

    fn encode_fixed_qp_gop_with_options(
        &mut self,
        nv12: &[u8],
        frames: usize,
        rois: &[encoder::RoiRegion],
        split_mode: SplitMode,
        split_arg: u32,
        split_out: u32,
    ) -> Result<EncodedGop, String> {
        if !(1..=6).contains(&frames) {
            return Err(format!(
                "captured VEPU510 GOP length {frames} is outside 1..=6"
            ));
        }
        let expected = INPUT_SIZE
            .checked_mul(frames)
            .ok_or_else(|| "VEPU510 GOP input size overflow".to_string())?;
        if nv12.len() < expected {
            return Err(format!(
                "VEPU510 GOP input is {} bytes, expected {}",
                nv12.len(),
                expected
            ));
        }
        if self.idr_done {
            return Err("this captured VEPU510 session already encoded its IDR".into());
        }
        let idr = self.encode_idr(&nv12[..INPUT_SIZE], rois, split_mode, split_arg, split_out)?;
        let mut encoded = EncodedGop {
            stream: idr.stream,
            packet_lengths: idr.packet_lengths,
        };
        self.idr_done = true;
        for frame_number in 1..frames {
            let start = frame_number * INPUT_SIZE;
            let end = start + INPUT_SIZE;
            let p = self.encode_p(
                &nv12[start..end],
                frame_number as u32,
                rois,
                split_mode,
                split_arg,
                split_out,
                &[],
                None,
            )?;
            encoded.stream.extend(p.stream);
            encoded.packet_lengths.extend(p.packet_lengths);
        }
        Ok(encoded)
    }

    pub fn encode_cbr_gop_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
    ) -> Result<Vec<u8>, String> {
        self.encode_rate_gop_320x240(nv12, frames, RcMode::Cbr)
    }

    pub fn encode_rate_gop_320x240(
        &mut self,
        nv12: &[u8],
        frames: usize,
        rc_mode: RcMode,
    ) -> Result<Vec<u8>, String> {
        if rc_mode == RcMode::FixedQp {
            return Err("fixed-QP GOP uses encode_fixed_qp_gop_320x240".into());
        }
        if !(1..=6).contains(&frames) {
            return Err(format!(
                "captured VEPU510 rate-control GOP length {frames} is outside 1..=6"
            ));
        }
        let expected = INPUT_SIZE
            .checked_mul(frames)
            .ok_or_else(|| "VEPU510 CBR input size overflow".to_string())?;
        if nv12.len() < expected {
            return Err(format!(
                "VEPU510 rate-control input is {} bytes, expected {expected}",
                nv12.len()
            ));
        }
        if self.idr_done {
            return Err("this captured VEPU510 session already encoded its IDR".into());
        }

        let mut config = EncoderConfig::new(WIDTH, HEIGHT);
        config.vui_log2_max_mv_length = 15;
        config.rc_mode = rc_mode;
        config.gop = 60;
        config.quality_auto = true;
        config.bps_target = 300_000;
        config.bps_min = 250_000;
        config.bps_max = 350_000;
        config.rc_qp_min = 8;
        config.rc_qp_max = 48;
        let mut model = rc_model::Model::new(h264e::rc_model_config(&config)?);
        let header = encoder::build_headers(&config).map_err(|e| e.to_string())?;
        if header.len() != 36 {
            return Err(format!(
                "unexpected no-SEI VEPU510 header length {}",
                header.len()
            ));
        }

        let mut stream = Vec::new();
        for frame_number in 0..frames {
            let is_idr = frame_number == 0;
            let frame_type = if is_idr {
                rc_model::FrameType::Intra
            } else {
                rc_model::FrameType::InterP
            };
            let (bit_target, quality_target) = model.start(frame_type);
            let mut registers = if is_idr {
                RegisterFile::captured_fixqp_idr_320x240(
                    self.input.fd() as u32,
                    self.output.fd() as u32,
                    self.recon[0].fd() as u32,
                    self.thumb[0].fd() as u32,
                    self.smear[0].fd() as u32,
                )?
            } else {
                let current = (frame_number & 1) as usize;
                let reference = 1 - current;
                RegisterFile::captured_fixqp_p_320x240(
                    self.input.fd() as u32,
                    self.output.fd() as u32,
                    self.recon[current].fd() as u32,
                    self.recon[reference].fd() as u32,
                    self.thumb[current].fd() as u32,
                    self.thumb[reference].fd() as u32,
                    self.smear[current].fd() as u32,
                    self.smear[reference].fd() as u32,
                    frame_number as u32,
                )?
            };
            registers.apply_cbr_rate_control(
                bit_target,
                quality_target as u32,
                config.rc_qp_min,
                config.rc_qp_max,
                is_idr,
            )?;
            let start = frame_number * INPUT_SIZE;
            let end = start + INPUT_SIZE;
            let prefix = if is_idr { header.as_slice() } else { &[] };
            registers.apply_roi_regions(&[])?;
            let output = self.run_job_with_status(&nv12[start..end], &registers, prefix, 0)?;
            let measured = measured_from_status(&output.status);
            model.end_with(frame_type, bit_target, measured);
            model.accept_frame();
            stream.extend(output.stream);
        }
        self.idr_done = true;
        Ok(stream)
    }

    fn encode_idr(
        &mut self,
        nv12: &[u8],
        rois: &[encoder::RoiRegion],
        split_mode: SplitMode,
        split_arg: u32,
        split_out: u32,
    ) -> Result<EncodedFrame, String> {
        let mut config = EncoderConfig::new(WIDTH, HEIGHT);
        config.vui_log2_max_mv_length = 15;
        let header = encoder::build_headers(&config).map_err(|e| e.to_string())?;
        if header.len() != 36 {
            return Err(format!(
                "unexpected no-SEI VEPU510 header length {}",
                header.len()
            ));
        }
        let mut registers = RegisterFile::captured_fixqp_idr_320x240(
            self.input.fd() as u32,
            self.output.fd() as u32,
            self.recon[0].fd() as u32,
            self.thumb[0].fd() as u32,
            self.smear[0].fd() as u32,
        )?;
        registers.apply_roi_regions(rois)?;
        registers.apply_slice_split(split_mode, split_arg, split_out)?;
        let output = self.run_job_with_status(nv12, &registers, &header, split_out)?;
        Ok(EncodedFrame {
            stream: output.stream,
            packet_lengths: output.packet_lengths,
        })
    }

    fn encode_p(
        &mut self,
        nv12: &[u8],
        frame_number: u32,
        rois: &[encoder::RoiRegion],
        split_mode: SplitMode,
        split_arg: u32,
        split_out: u32,
        prefix: &[u8],
        refresh: Option<GdrMode>,
    ) -> Result<EncodedFrame, String> {
        let current = (frame_number & 1) as usize;
        let reference = 1 - current;
        let mut registers = RegisterFile::captured_fixqp_p_320x240(
            self.input.fd() as u32,
            self.output.fd() as u32,
            self.recon[current].fd() as u32,
            self.recon[reference].fd() as u32,
            self.thumb[current].fd() as u32,
            self.thumb[reference].fd() as u32,
            self.smear[current].fd() as u32,
            self.smear[reference].fd() as u32,
            frame_number,
        )?;
        registers.apply_roi_regions(rois)?;
        registers.apply_slice_split(split_mode, split_arg, split_out)?;
        registers.set(
            vepu510_regs::SQI_SMEAR_OPT_CFG_STATED_MODE,
            if frame_number == 1 { 1 } else { 2 },
        )?;
        if let Some(mode) = refresh {
            registers.apply_gdr_refresh(mode)?;
        }
        let output = self.run_job_with_status(nv12, &registers, prefix, split_out)?;
        Ok(EncodedFrame {
            stream: output.stream,
            packet_lengths: output.packet_lengths,
        })
    }

    fn run_job_with_status(
        &mut self,
        nv12: &[u8],
        registers: &RegisterFile,
        prefix: &[u8],
        split_out: u32,
    ) -> Result<JobOutput, String> {
        if nv12.len() < INPUT_SIZE {
            return Err(format!(
                "VEPU510 input is {} bytes, expected {INPUT_SIZE}",
                nv12.len()
            ));
        }
        self.input
            .sync_write_begin()
            .map_err(|e| format!("VEPU510 input sync begin: {e}"))?;
        self.input.as_mut_slice()[..INPUT_SIZE].copy_from_slice(&nv12[..INPUT_SIZE]);
        self.input
            .sync_write_end()
            .map_err(|e| format!("VEPU510 input flush: {e}"))?;

        if !prefix.is_empty() {
            self.output
                .sync_write_begin()
                .map_err(|e| format!("VEPU510 output sync begin: {e}"))?;
            self.output.as_mut_slice()[..prefix.len()].copy_from_slice(prefix);
            self.output
                .sync_write_end()
                .map_err(|e| format!("VEPU510 output prefix flush: {e}"))?;
        }

        let luma_size = WIDTH * HEIGHT;
        let offsets = vec![
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
        let mut status = [0u32; STATUS_WORDS];
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
            .map_err(|e| format!("VEPU510 submit: {e}"))?;
        let mut packet_lengths = Vec::new();
        if split_out & MPP_ENC_SPLIT_OUT_LOWDELAY != 0 {
            let mut complete = false;
            for _ in 0..=4096 {
                let mut info = [SliceInfo::default()];
                let count = match self.svc.poll_irq(&mut info) {
                    Ok(count) => count,
                    Err(error) => {
                        let _ = self.svc.restart_after_hardware_error();
                        return Err(format!("VEPU510 slice poll: {error}"));
                    }
                };
                for slice in &info[..count] {
                    let (length, last) = slice.vepu510();
                    if length == 0 {
                        let _ = self.svc.restart_after_hardware_error();
                        return Err("VEPU510 slice FIFO returned a zero-length slice".into());
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
                return Err("VEPU510 slice FIFO did not report the last slice".into());
            }
        } else {
            self.svc
                .poll_hw_finish_recovering()
                .map_err(|e| format!("VEPU510 poll: {e}"))?;
        }

        if int_state[0] & 1 == 0 {
            let error = status_execution_error(&status);
            let _ = self.svc.restart_after_hardware_error();
            return Err(format!(
                "VEPU510 completed without enc_done (int_sta=0x{:08x}, status={error})",
                int_state[0]
            ));
        }
        let payload_len = status[0] as usize;
        let stream_len = prefix
            .len()
            .checked_add(payload_len)
            .ok_or_else(|| "VEPU510 stream length overflow".to_string())?;
        if payload_len == 0 || stream_len > self.output.len() {
            let _ = self.svc.restart_after_hardware_error();
            return Err(format!(
                "invalid VEPU510 payload length {payload_len} (capacity {})",
                self.output.len()
            ));
        }
        if !packet_lengths.is_empty() {
            if !prefix.is_empty() {
                packet_lengths[0] = packet_lengths[0]
                    .checked_add(prefix.len())
                    .ok_or_else(|| "VEPU510 first packet length overflow".to_string())?;
            }
            let slice_len = packet_lengths.iter().try_fold(0usize, |total, length| {
                total
                    .checked_add(*length)
                    .ok_or_else(|| "VEPU510 packet length overflow".to_string())
            })?;
            if slice_len != stream_len {
                let _ = self.svc.restart_after_hardware_error();
                return Err(format!(
                    "VEPU510 slice lengths total {slice_len}, STATUS stream length is {stream_len}"
                ));
            }
        }

        self.output
            .sync_read_begin()
            .map_err(|e| format!("VEPU510 output invalidate: {e}"))?;
        let stream = self.output.as_slice()[..stream_len].to_vec();
        self.output
            .sync_read_end()
            .map_err(|e| format!("VEPU510 output sync end: {e}"))?;
        Ok(JobOutput {
            stream,
            status,
            packet_lengths,
        })
    }
}

fn zero_buffer(buffer: &mut DmaBuf) -> Result<(), String> {
    buffer
        .sync_write_begin()
        .map_err(|e| format!("VEPU510 scratch sync begin: {e}"))?;
    buffer.as_mut_slice().fill(0);
    buffer
        .sync_write_end()
        .map_err(|e| format!("VEPU510 scratch flush: {e}"))
}

fn status_execution_error(status: &[u32]) -> u32 {
    let field = vepu510_regs::STATUS_ST_ENC_ST_ENC;
    let word = status[field.word - 4096];
    (word >> field.shift) & ((1u32 << field.width) - 1)
}

fn measured_from_status(status: &[u32]) -> rc_model::Measured {
    let mbs = (WIDTH * HEIGHT / 256) as i32;
    let p4 = (status[39] & 0x007f_ffff) as i32;
    let p8 = (status[38] & 0x007f_ffff) as i32;
    let p16 = (status[37] & 0x001f_ffff) as i32;
    rc_model::Measured {
        bits: status[0] as i32 * 8,
        quality: status[3] as i32 / mbs,
        madi: 0,
        madp: 0,
        iblk4_prop: (p4 + p8 + p16) * 256 / mbs,
    }
}
