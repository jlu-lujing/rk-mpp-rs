//! First RK3576 VEPU510 H.264 hardware path.
//!
//! The current slice supports a fixed-QP 320x240 Baseline IDR followed by one
//! P picture. Both register images are captured from C MPP; only DMA fds are
//! patched at runtime.

use crate::codec::h264::encoder::{
    self, EncoderConfig, GdrMode, RcMode, SplitMode, MPP_ENC_SPLIT_OUT_LOWDELAY, PROFILE_HIGH,
};
use crate::codec::rc::model as rc_model;
use crate::encoder::{ExternalRateControl, RateControlFeedback, RateControlReencode};
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
const MD_INFO_SIZE: usize = 1200;
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

#[derive(Clone, Copy)]
struct PictureLayout {
    current_slot: usize,
    reference_slot: usize,
    non_reference: bool,
    temporal_id: u8,
    frame_num: u32,
    poc: u32,
    reference_reorder: Option<crate::encoder::ReferenceReorder>,
    current_long_term_index: Option<u32>,
}

pub struct Vepu510H264 {
    config: EncoderConfig,
    svc: MppService,
    input: DmaBuf,
    output: DmaBuf,
    recon: [DmaBuf; 3],
    thumb: [DmaBuf; 3],
    smear: [DmaBuf; 3],
    md_info: DmaBuf,
    idr_done: bool,
    idr_pic_id: u32,
    frame_number: u32,
    input_fd_override: Option<i32>,
    next_layout: Option<PictureLayout>,
    force_next_idr: bool,
    headers_each_idr: bool,
    last_status: Option<[u32; STATUS_WORDS]>,
    rate_model: Option<rc_model::Model>,
    last_quality_target: Option<i32>,
    last_rt_bits: Option<i32>,
    last_encoded_bits: Option<i32>,
}

impl Vepu510H264 {
    pub fn open() -> Result<Self, String> {
        let mut config = EncoderConfig::new(WIDTH, HEIGHT);
        config.vui_log2_max_mv_length = 15;
        Self::open_with_config(config)
    }

    pub fn open_with_config(config: EncoderConfig) -> Result<Self, String> {
        config.validate().map_err(|error| error.to_string())?;
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
            DmaBuf::alloc("system", RECON_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 reconstruction 2: {e}"))?,
        ];
        let mut thumb = [
            DmaBuf::alloc("system", THUMB_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 thumbnail 0: {e}"))?,
            DmaBuf::alloc("system", THUMB_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 thumbnail 1: {e}"))?,
            DmaBuf::alloc("system", THUMB_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 thumbnail 2: {e}"))?,
        ];
        let mut smear = [
            DmaBuf::alloc("system", SMEAR_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 smear 0: {e}"))?,
            DmaBuf::alloc("system", SMEAR_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 smear 1: {e}"))?,
            DmaBuf::alloc("system", SMEAR_SIZE, false)
                .map_err(|e| format!("allocate VEPU510 smear 2: {e}"))?,
        ];
        let mut md_info = DmaBuf::alloc("system", MD_INFO_SIZE, true)
            .map_err(|e| format!("allocate VEPU510 motion info: {e}"))?;
        for buffer in recon
            .iter_mut()
            .chain(thumb.iter_mut())
            .chain(smear.iter_mut())
            .chain(std::iter::once(&mut md_info))
        {
            zero_buffer(buffer)?;
        }
        let rate_model = if config.rc_mode == RcMode::FixedQp {
            None
        } else {
            Some(rc_model::Model::new(h264e::rc_model_config(&config)?))
        };
        Ok(Self {
            config,
            svc: MppService::open(Client::RkvEnc as u32)
                .map_err(|e| format!("open RK3576 video encoder: {e}"))?,
            input,
            output,
            recon,
            thumb,
            smear,
            md_info,
            idr_done: false,
            idr_pic_id: 0,
            frame_number: 0,
            input_fd_override: None,
            next_layout: None,
            force_next_idr: false,
            headers_each_idr: false,
            last_status: None,
            rate_model,
            last_quality_target: None,
            last_rt_bits: None,
            last_encoded_bits: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn force_next_picture_layout(
        &mut self,
        current_slot: usize,
        reference_slot: usize,
        non_reference: bool,
        temporal_id: u8,
        frame_num: u32,
        poc: u32,
        reference_reorder: Option<crate::encoder::ReferenceReorder>,
        current_long_term_index: Option<u32>,
    ) -> Result<(), String> {
        if current_slot >= self.recon.len() || reference_slot >= self.recon.len() {
            return Err("VEPU510 picture layout uses a slot outside its recon pool".into());
        }
        self.next_layout = Some(PictureLayout {
            current_slot,
            reference_slot,
            non_reference,
            temporal_id,
            frame_num,
            poc,
            reference_reorder,
            current_long_term_index,
        });
        Ok(())
    }

    pub(crate) fn force_idr(&mut self) {
        self.force_next_idr = true;
    }

    pub(crate) fn set_headers_each_idr(&mut self, each_idr: bool) {
        self.headers_each_idr = each_idr;
    }

    pub(crate) fn set_sei_enabled(&mut self, _enabled: bool) {}

    /// Streaming variant of the captured fixed-QP path: one IDR followed by P
    /// pictures for a 320x240 QP26 Baseline sequence.
    pub fn encode_fixed_qp_next_320x240(
        &mut self,
        nv12: &[u8],
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        self.last_quality_target = Some(i32::from(self.config.qp_init));
        self.last_rt_bits = Some(0);
        let layout = self.next_layout.take();
        let force_idr = std::mem::take(&mut self.force_next_idr);
        let direct_input = self.input_fd_override.is_some();
        if !direct_input && nv12.len() < INPUT_SIZE {
            return Err(format!(
                "VEPU510 input is {} bytes, expected at least {INPUT_SIZE}",
                nv12.len()
            ));
        }
        let input_data = if direct_input {
            &[]
        } else {
            &nv12[..INPUT_SIZE]
        };
        if !self.idr_done || force_idr {
            if self.idr_done {
                self.idr_pic_id = self.idr_pic_id.saturating_add(1);
            }
            let include_header = !self.idr_done || self.headers_each_idr;
            let encoded = self.encode_idr_with_layout(
                input_data,
                &[],
                SplitMode::None,
                0,
                0,
                layout,
                include_header,
            )?;
            self.idr_done = true;
            self.frame_number = 0;
            return Ok((encoded.stream, true, 0, 0));
        }
        let frame_num = layout.map_or(self.frame_number.saturating_add(1), |layout| {
            layout.frame_num
        });
        let poc = layout.map_or(frame_num as i32 * 2, |layout| layout.poc as i32);
        self.frame_number = frame_num;
        let encoded = self.encode_p_with_layout(
            input_data,
            frame_num,
            &[],
            SplitMode::None,
            0,
            0,
            &[],
            None,
            layout,
        )?;
        Ok((encoded.stream, false, frame_num, poc))
    }

    pub fn encode_next_320x240(
        &mut self,
        nv12: &[u8],
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        if self.rate_model.is_some() {
            self.encode_rate_next_320x240(nv12)
        } else {
            self.encode_fixed_qp_next_320x240(nv12)
        }
    }

    pub fn encode_next_with_rate_320x240(
        &mut self,
        nv12: &[u8],
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        self.encode_next_with_rate_policy_320x240(
            nv12,
            ExternalRateControl {
                bit_target,
                qp_target,
                qp_min,
                qp_max,
            },
            None,
        )
    }

    pub(crate) fn encode_next_with_rate_reencode_320x240(
        &mut self,
        nv12: &[u8],
        initial: ExternalRateControl,
        reencode: &mut dyn FnMut(
            RateControlFeedback,
        ) -> Result<Option<RateControlReencode>, String>,
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        self.encode_next_with_rate_policy_320x240(nv12, initial, Some(reencode))
    }

    fn encode_next_with_rate_policy_320x240(
        &mut self,
        nv12: &[u8],
        mut target: ExternalRateControl,
        mut reencode: Option<
            &mut dyn FnMut(RateControlFeedback) -> Result<Option<RateControlReencode>, String>,
        >,
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        let force_idr = std::mem::take(&mut self.force_next_idr);
        let direct_input = self.input_fd_override.take();
        if direct_input.is_none() && nv12.len() < INPUT_SIZE {
            return Err(format!(
                "VEPU510 external rate-control input is {} bytes, expected at least {INPUT_SIZE}",
                nv12.len()
            ));
        }
        let is_idr = !self.idr_done || force_idr;
        if force_idr && self.idr_done {
            self.idr_pic_id = self.idr_pic_id.saturating_add(1);
        }
        let frame_number = if is_idr {
            0
        } else {
            self.frame_number.saturating_add(1)
        };
        let prefix = if is_idr && (!self.idr_done || self.headers_each_idr) {
            encoder::build_headers(&self.config).map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };
        let input_data = if direct_input.is_some() {
            &[]
        } else {
            &nv12[..INPUT_SIZE]
        };
        let output = loop {
            let input_fd = direct_input.unwrap_or_else(|| self.input.fd()) as u32;
            let mut registers = if is_idr {
                RegisterFile::captured_fixqp_idr_320x240(
                    input_fd,
                    self.output.fd() as u32,
                    self.recon[0].fd() as u32,
                    self.thumb[0].fd() as u32,
                    self.smear[0].fd() as u32,
                )?
            } else {
                let current = (frame_number & 1) as usize;
                let reference = 1 - current;
                RegisterFile::captured_fixqp_p_320x240(
                    input_fd,
                    self.output.fd() as u32,
                    self.recon[current].fd() as u32,
                    self.recon[reference].fd() as u32,
                    self.thumb[current].fd() as u32,
                    self.thumb[reference].fd() as u32,
                    self.smear[current].fd() as u32,
                    self.smear[reference].fd() as u32,
                    frame_number,
                )?
            };
            registers.apply_cbr_rate_control(
                target.bit_target,
                target.qp_target,
                target.qp_min,
                target.qp_max,
                is_idr,
            )?;
            registers.apply_roi_regions(&self.config.rois)?;
            let output = self.run_job_with_status(input_data, &registers, &prefix, 0)?;
            let measured = measured_from_status(&output.status);
            let next = reencode
                .as_deref_mut()
                .map(|callback| {
                    callback(RateControlFeedback {
                        bit_real: measured.bits,
                        quality_real: measured.quality,
                    })
                })
                .transpose()?
                .flatten();
            if let Some(next) = next {
                target = next.target;
                continue;
            }
            break output;
        };
        self.last_quality_target = Some(target.qp_target as i32);
        self.last_rt_bits = Some(0);
        self.idr_done = true;
        self.frame_number = frame_number;
        Ok((output.stream, is_idr, frame_number, frame_number as i32 * 2))
    }

    pub fn encode_next_with_rate_320x240_fd(
        &mut self,
        input_fd: i32,
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        if input_fd < 0 {
            return Err("VEPU510 H.264 input fd is invalid".to_string());
        }
        self.input_fd_override = Some(input_fd);
        self.encode_next_with_rate_320x240(&[], bit_target, qp_target, qp_min, qp_max)
    }

    fn encode_rate_next_320x240(
        &mut self,
        nv12: &[u8],
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        let force_idr = std::mem::take(&mut self.force_next_idr);
        let direct_input = self.input_fd_override.is_some();
        if !direct_input && nv12.len() < INPUT_SIZE {
            return Err(format!(
                "VEPU510 rate-control input is {} bytes, expected at least {INPUT_SIZE}",
                nv12.len()
            ));
        }
        let is_idr = !self.idr_done || force_idr;
        if force_idr && self.idr_done {
            self.idr_pic_id = self.idr_pic_id.saturating_add(1);
        }
        let frame_number = if is_idr {
            0
        } else {
            self.frame_number.saturating_add(1)
        };
        let frame_type = if is_idr {
            rc_model::FrameType::Intra
        } else {
            rc_model::FrameType::InterP
        };
        let (bit_target, quality_target, rt_bits) = {
            let model = self.rate_model.as_mut().expect("rate-control model");
            let (bit_target, quality_target) = model.start(frame_type);
            (bit_target, quality_target, model.ins_bps())
        };
        let prefix = if is_idr && (!self.idr_done || self.headers_each_idr) {
            encoder::build_headers(&self.config).map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };
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
                frame_number,
            )?
        };
        registers.apply_cbr_rate_control(
            bit_target,
            quality_target as u32,
            self.config.rc_qp_min,
            self.config.rc_qp_max,
            is_idr,
        )?;
        registers.apply_roi_regions(&self.config.rois)?;
        let input_data = if direct_input {
            &[]
        } else {
            &nv12[..INPUT_SIZE]
        };
        let output = self.run_job_with_status(input_data, &registers, &prefix, 0)?;
        let measured = measured_from_status(&output.status);
        let model = self.rate_model.as_mut().expect("rate-control model");
        model.end_with(frame_type, bit_target, measured);
        model.accept_frame();
        self.last_quality_target = Some(quality_target);
        self.last_rt_bits = Some(rt_bits);
        self.idr_done = true;
        self.frame_number = frame_number;
        Ok((output.stream, is_idr, frame_number, frame_number as i32 * 2))
    }

    pub fn encode_next_320x240_fd(
        &mut self,
        input_fd: i32,
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        if input_fd < 0 {
            return Err("VEPU510 H.264 input fd is invalid".to_string());
        }
        self.input_fd_override = Some(input_fd);
        self.encode_next_320x240(&[])
    }

    pub fn read_motion_info(&mut self) -> Result<Vec<u8>, String> {
        self.md_info
            .sync_read_begin()
            .map_err(|e| format!("VEPU510 motion info invalidate: {e}"))?;
        let data = self.md_info.as_slice()[..MD_INFO_SIZE].to_vec();
        self.md_info
            .sync_read_end()
            .map_err(|e| format!("VEPU510 motion info sync end: {e}"))?;
        Ok(data)
    }

    pub fn last_packet_meta(&self, is_intra: bool) -> Option<crate::encoder::PacketMeta> {
        let status = self.last_status.as_ref()?;
        let macroblocks = (WIDTH * HEIGHT / 256) as u32;
        let average_qp = if macroblocks == 0 {
            0
        } else {
            (status[3] / macroblocks) as i32
        };
        let mut meta = crate::encoder::PacketMeta::from_vepu510_status(
            status,
            is_intra,
            self.last_quality_target
                .unwrap_or(i32::from(self.config.qp_init)),
            average_qp,
        );
        meta.rt_bits = self.last_rt_bits;
        Some(meta)
    }

    pub fn last_encoded_bits(&self) -> Option<i32> {
        self.last_encoded_bits
    }

    pub fn encode_fixed_qp_next_320x240_fd(
        &mut self,
        input_fd: i32,
    ) -> Result<(Vec<u8>, bool, u32, i32), String> {
        if input_fd < 0 {
            return Err("VEPU510 H.264 input fd is invalid".to_string());
        }
        self.input_fd_override = Some(input_fd);
        self.encode_fixed_qp_next_320x240(&[])
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
        let include_header = !self.idr_done;
        self.encode_idr_with_layout(
            nv12,
            rois,
            split_mode,
            split_arg,
            split_out,
            None,
            include_header,
        )
    }

    fn encode_idr_with_layout(
        &mut self,
        nv12: &[u8],
        rois: &[encoder::RoiRegion],
        split_mode: SplitMode,
        split_arg: u32,
        split_out: u32,
        layout: Option<PictureLayout>,
        include_header: bool,
    ) -> Result<EncodedFrame, String> {
        let config = &self.config;
        let mut prefix = if include_header {
            let header = encoder::build_headers(config).map_err(|e| e.to_string())?;
            if !matches!(header.len(), 36..=38) {
                return Err(format!(
                    "unexpected no-SEI VEPU510 header length {}",
                    header.len()
                ));
            }
            header
        } else {
            Vec::new()
        };
        let current_slot = layout.map_or(0, |layout| layout.current_slot);
        let mut registers = if config.profile >= PROFILE_HIGH {
            RegisterFile::captured_fixqp_idr_320x240_high(
                self.input.fd() as u32,
                self.output.fd() as u32,
                self.recon[current_slot].fd() as u32,
                self.thumb[current_slot].fd() as u32,
                self.smear[current_slot].fd() as u32,
                self.md_info.fd() as u32,
            )?
        } else {
            RegisterFile::captured_fixqp_idr_320x240(
                self.input.fd() as u32,
                self.output.fd() as u32,
                self.recon[current_slot].fd() as u32,
                self.thumb[current_slot].fd() as u32,
                self.smear[current_slot].fd() as u32,
            )?
        };
        registers.set(vepu510_regs::H264_SYNT_SLI1_IDR_PID, self.idr_pic_id)?;
        if let Some(layout) = layout {
            registers.set(
                vepu510_regs::H264_SYNT_REFM0_LTRF_FLG,
                u32::from(layout.current_long_term_index.is_some()),
            )?;
        }
        registers.apply_roi_regions(rois)?;
        registers.apply_slice_split(split_mode, split_arg, split_out)?;
        if let Some(layout) = layout {
            prefix.extend(h264e::svc_prefix_nal(
                true,
                if layout.non_reference { 0 } else { 3 },
                layout.temporal_id,
            ));
        }
        let output = self.run_job_with_status(nv12, &registers, &prefix, split_out)?;
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
        self.encode_p_with_layout(
            nv12,
            frame_number,
            rois,
            split_mode,
            split_arg,
            split_out,
            prefix,
            refresh,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_p_with_layout(
        &mut self,
        nv12: &[u8],
        frame_number: u32,
        rois: &[encoder::RoiRegion],
        split_mode: SplitMode,
        split_arg: u32,
        split_out: u32,
        prefix: &[u8],
        refresh: Option<GdrMode>,
        layout: Option<PictureLayout>,
    ) -> Result<EncodedFrame, String> {
        let current = layout.map_or((frame_number & 1) as usize, |layout| layout.current_slot);
        let reference = layout.map_or(1 - current, |layout| layout.reference_slot);
        let mut registers = if self.config.profile >= PROFILE_HIGH {
            RegisterFile::captured_fixqp_p_320x240_high(
                self.input.fd() as u32,
                self.output.fd() as u32,
                self.recon[current].fd() as u32,
                self.recon[reference].fd() as u32,
                self.thumb[current].fd() as u32,
                self.thumb[reference].fd() as u32,
                self.smear[current].fd() as u32,
                self.smear[reference].fd() as u32,
                frame_number,
                self.md_info.fd() as u32,
            )?
        } else {
            RegisterFile::captured_fixqp_p_320x240(
                self.input.fd() as u32,
                self.output.fd() as u32,
                self.recon[current].fd() as u32,
                self.recon[reference].fd() as u32,
                self.thumb[current].fd() as u32,
                self.thumb[reference].fd() as u32,
                self.smear[current].fd() as u32,
                self.smear[reference].fd() as u32,
                frame_number,
            )?
        };
        if let Some(layout) = layout {
            registers.set(vepu510_regs::H264_SYNT_SLI0_FRM_NUM, layout.frame_num)?;
            registers.set(vepu510_regs::H264_SYNT_SLI1_POC_LSB, layout.poc)?;
            registers.set(
                vepu510_regs::H264_SYNT_NAL_NAL_REF_IDC,
                if layout.non_reference { 0 } else { 2 },
            )?;
            registers.set(
                vepu510_regs::FRAME_ENC_PIC_CUR_FRM_REF,
                u32::from(!layout.non_reference),
            )?;
            apply_reference_reorder(
                &mut registers,
                layout.reference_reorder,
                layout.frame_num,
                self.config.log2_max_frame_num_minus4,
            )?;
            if let Some(long_term_index) = layout.current_long_term_index {
                registers.set(vepu510_regs::H264_SYNT_REFM0_ARPM_FLG, 1)?;
                registers.set(vepu510_regs::H264_SYNT_REFM0_MMCO_TYPE0, 6)?;
                registers.set(vepu510_regs::H264_SYNT_REFM0_MMCO_PARM0, long_term_index)?;
            }
        }
        registers.apply_roi_regions(rois)?;
        registers.apply_slice_split(split_mode, split_arg, split_out)?;
        registers.set(
            vepu510_regs::SQI_SMEAR_OPT_CFG_STATED_MODE,
            if frame_number == 1 { 1 } else { 2 },
        )?;
        if let Some(mode) = refresh {
            registers.apply_gdr_refresh(mode)?;
        }
        let mut slice_prefix = prefix.to_vec();
        if let Some(layout) = layout {
            slice_prefix.extend(h264e::svc_prefix_nal(
                false,
                if layout.non_reference { 0 } else { 2 },
                layout.temporal_id,
            ));
        }
        let output = self.run_job_with_status(nv12, &registers, &slice_prefix, split_out)?;
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
        let input_fd = if let Some(input_fd) = self.input_fd_override.take() {
            input_fd
        } else {
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
            self.input.fd()
        };

        let high_profile = self.config.profile >= PROFILE_HIGH;
        if !prefix.is_empty() && !high_profile {
            self.output
                .sync_write_begin()
                .map_err(|e| format!("VEPU510 output sync begin: {e}"))?;
            self.output.as_mut_slice()[..prefix.len()].copy_from_slice(prefix);
            self.output
                .sync_write_end()
                .map_err(|e| format!("VEPU510 output prefix flush: {e}"))?;
        }

        let luma_size = WIDTH * HEIGHT;
        let output_capacity = if high_profile {
            self.output.len() - FBC_HEADER_SIZE
        } else {
            self.output.len() - 1
        };
        let output_start = if high_profile { 0 } else { prefix.len() };
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
                offset: output_capacity as u32,
            },
            RegOffsetInfo {
                reg_idx: 174,
                offset: output_start as u32,
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
        batch.reg_read(&mut int_state, HW_STATUS_OFFSET);
        batch.reg_read(&mut status, STATUS_OFFSET);
        batch.reg_offsets(&offsets);
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
        let expected_len = prefix
            .len()
            .checked_add(payload_len)
            .ok_or_else(|| "VEPU510 stream length overflow".to_string())?;
        if payload_len == 0 || expected_len > self.output.len() {
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
            if slice_len != expected_len {
                let _ = self.svc.restart_after_hardware_error();
                return Err(format!(
                    "VEPU510 slice lengths total {slice_len}, STATUS stream length is {expected_len}"
                ));
            }
        }

        self.output
            .sync_read_begin()
            .map_err(|e| format!("VEPU510 output invalidate: {e}"))?;
        let stream = if high_profile {
            let mut stream = Vec::with_capacity(expected_len);
            stream.extend_from_slice(prefix);
            stream.extend_from_slice(&self.output.as_slice()[..payload_len]);
            stream
        } else {
            self.output.as_slice()[..expected_len].to_vec()
        };
        self.output
            .sync_read_end()
            .map_err(|e| format!("VEPU510 output sync end: {e}"))?;
        self.last_status = Some(status);
        self.last_encoded_bits = Some(status[0] as i32 * 8);
        Ok(JobOutput {
            stream,
            status,
            packet_lengths,
        })
    }
}

fn apply_reference_reorder(
    registers: &mut RegisterFile,
    reorder: Option<crate::encoder::ReferenceReorder>,
    frame_num: u32,
    log2_max_frame_num_minus4: u8,
) -> Result<(), String> {
    registers.set(vepu510_regs::H264_SYNT_SLI2_REF_LIST0_RODR, 0)?;
    registers.set(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_IDX, 0)?;
    registers.set(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_NUM, 0)?;
    let Some(reorder) = reorder else {
        return Ok(());
    };
    registers.set(vepu510_regs::H264_SYNT_SLI2_REF_LIST0_RODR, 1)?;
    match reorder {
        crate::encoder::ReferenceReorder::LongTerm { long_term_pic_num } => {
            registers.set(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_IDX, 2)?;
            registers.set(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_NUM, long_term_pic_num)?;
        }
        crate::encoder::ReferenceReorder::ShortTerm(order) => {
            let max_pic_num = 1u32 << (u32::from(log2_max_frame_num_minus4) + 4);
            let forward = frame_num
                .wrapping_add(max_pic_num)
                .wrapping_sub(order.reference_frame_num())
                % max_pic_num;
            let (direction, distance) = if forward <= max_pic_num / 2 {
                (0, forward)
            } else {
                (1, max_pic_num - forward)
            };
            if distance == 0 {
                return Err("VEPU510 reference reorder resolves to the current frame".into());
            }
            registers.set(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_IDX, direction)?;
            registers.set(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_NUM, distance - 1)?;
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::{ReferenceReorder, ShortTermReorder};

    fn p_registers() -> RegisterFile {
        RegisterFile::captured_fixqp_p_320x240(9, 10, 16, 13, 17, 14, 18, 15, 2).unwrap()
    }

    #[test]
    fn long_term_reorder_sets_the_vendor_register_fields() {
        let mut registers = p_registers();
        apply_reference_reorder(
            &mut registers,
            Some(ReferenceReorder::LongTerm {
                long_term_pic_num: 3,
            }),
            4,
            12,
        )
        .unwrap();
        assert_eq!(
            registers
                .get(vepu510_regs::H264_SYNT_SLI2_REF_LIST0_RODR)
                .unwrap(),
            1
        );
        assert_eq!(
            registers
                .get(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_IDX)
                .unwrap(),
            2
        );
        assert_eq!(
            registers
                .get(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_NUM)
                .unwrap(),
            3
        );
    }

    #[test]
    fn short_term_reorder_uses_direction_and_pic_num_delta() {
        let mut registers = p_registers();
        apply_reference_reorder(
            &mut registers,
            Some(ReferenceReorder::ShortTerm(ShortTermReorder::new(2, 0))),
            2,
            12,
        )
        .unwrap();
        assert_eq!(
            registers
                .get(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_IDX)
                .unwrap(),
            0
        );
        assert_eq!(
            registers
                .get(vepu510_regs::H264_SYNT_SLI2_RODR_PIC_NUM)
                .unwrap(),
            1
        );
    }
}
