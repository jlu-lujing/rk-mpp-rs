//! First RK3576 VEPU510 H.264 hardware path.
//!
//! This slice intentionally supports one fully captured configuration: a
//! 320x240, QP 26, Baseline IDR with no rate control. General syntax, P
//! pictures and RC are layered on after this job is byte-identical to C MPP.

use crate::codec::h264::encoder::{self, EncoderConfig};
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{require_video_encode, VideoEncodeHal};
use crate::service::{Batch, Client, MppService, RegOffsetInfo};

use super::vepu510::{self, RegisterFile};
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

pub struct Vepu510H264 {
    svc: MppService,
    input: DmaBuf,
    output: DmaBuf,
    recon: DmaBuf,
    thumb: DmaBuf,
    smear: DmaBuf,
}

impl Vepu510H264 {
    pub fn open() -> Result<Self, String> {
        require_video_encode(VideoEncodeHal::Vepu510)?;
        Ok(Self {
            svc: MppService::open(Client::RkvEnc as u32)
                .map_err(|e| format!("open RK3576 video encoder: {e}"))?,
            input: DmaBuf::alloc("system", INPUT_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 input: {e}"))?,
            output: DmaBuf::alloc("system", OUTPUT_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 output: {e}"))?,
            recon: DmaBuf::alloc("system", RECON_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 reconstruction: {e}"))?,
            thumb: DmaBuf::alloc("system", THUMB_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 thumbnail: {e}"))?,
            smear: DmaBuf::alloc("system", SMEAR_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 smear: {e}"))?,
        })
    }

    pub fn encode_fixed_qp_idr_320x240(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        if nv12.len() < INPUT_SIZE {
            return Err(format!(
                "VEPU510 input is {} bytes, expected {INPUT_SIZE}",
                nv12.len()
            ));
        }
        let mut config = EncoderConfig::new(WIDTH, HEIGHT);
        config.vui_log2_max_mv_length = 15;
        let header = encoder::build_headers(&config).map_err(|e| e.to_string())?;
        if header.len() != 36 {
            return Err(format!(
                "unexpected no-SEI VEPU510 header length {}",
                header.len()
            ));
        }

        self.input
            .sync_write_begin()
            .map_err(|e| format!("VEPU510 input sync begin: {e}"))?;
        self.input.as_mut_slice()[..INPUT_SIZE].copy_from_slice(&nv12[..INPUT_SIZE]);
        self.input
            .sync_write_end()
            .map_err(|e| format!("VEPU510 input flush: {e}"))?;

        for buffer in [&mut self.recon, &mut self.thumb, &mut self.smear] {
            buffer
                .sync_write_begin()
                .map_err(|e| format!("VEPU510 scratch sync begin: {e}"))?;
            buffer.as_mut_slice().fill(0);
            buffer
                .sync_write_end()
                .map_err(|e| format!("VEPU510 scratch flush: {e}"))?;
        }

        self.output
            .sync_write_begin()
            .map_err(|e| format!("VEPU510 output sync begin: {e}"))?;
        self.output.as_mut_slice()[..header.len()].copy_from_slice(&header);
        self.output
            .sync_write_end()
            .map_err(|e| format!("VEPU510 output header flush: {e}"))?;

        let registers = RegisterFile::captured_fixqp_idr_320x240(
            self.input.fd() as u32,
            self.output.fd() as u32,
            self.recon.fd() as u32,
            self.thumb.fd() as u32,
            self.smear.fd() as u32,
        )?;
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
                offset: header.len() as u32,
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
        self.svc
            .poll_hw_finish_recovering()
            .map_err(|e| format!("VEPU510 poll: {e}"))?;

        if int_state[0] & 1 == 0 {
            let error = status_execution_error(&status);
            let _ = self.svc.restart_after_hardware_error();
            return Err(format!(
                "VEPU510 completed without enc_done (int_sta=0x{:08x}, status={error})",
                int_state[0]
            ));
        }
        let payload_len = status[0] as usize;
        let stream_len = header
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

        self.output
            .sync_read_begin()
            .map_err(|e| format!("VEPU510 output invalidate: {e}"))?;
        let stream = self.output.as_slice()[..stream_len].to_vec();
        self.output
            .sync_read_end()
            .map_err(|e| format!("VEPU510 output sync end: {e}"))?;
        Ok(stream)
    }
}

fn status_execution_error(status: &[u32]) -> u32 {
    vepu510::RegisterFile::default()
        .get(vepu510_regs::STATUS_ST_ENC_ST_ENC)
        .ok()
        .map(|_| {
            let word = status[vepu510_regs::STATUS_ST_ENC_ST_ENC.word - 4096];
            (word >> vepu510_regs::STATUS_ST_ENC_ST_ENC.shift)
                & ((1u32 << vepu510_regs::STATUS_ST_ENC_ST_ENC.width) - 1)
        })
        .unwrap_or(0)
}
