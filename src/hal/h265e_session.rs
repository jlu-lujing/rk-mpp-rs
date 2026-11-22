//! VEPU580 H.265 encoder session: one fixed-QP IDR, submitted to the hardware.
//!
//! This is the first path that actually submits an H.265 job. It owns every
//! surface the job points at - input, output, two FBC reconstructions (pixel,
//! display thumb, motion-vector thumb), and the motion-info buffer the C harness
//! passes - so the address words in `h265e`'s image are ours rather than the
//! oracle's.
//!
//! The layout and the multi-offset table are `vepu580_h265_set_hw_address`:
//! registers 160..162 are the input plane (one fd, chroma reached through the
//! 161/162 offsets), 163/164 the reconstructed surface (164 offset by the FBC
//! header length), 165/166 the reference, 167..170 the motion-vector and display
//! thumbs, 171 the motion-info buffer, and 172..175 the bitstream buffer.

use std::collections::VecDeque;

use crate::codec::h265::encoder::{SliceSplit, SyntaxOptions, MPP_ENC_SPLIT_OUT_LOWDELAY};
use crate::codec::h265::roi::{build_tables as build_roi_tables, RoiRegion};
use crate::codec::rc::model as rc_model;
use crate::encoder::{ExternalRateControl, RateControlFeedback, RateControlReencode};
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{require_video_encode, VideoEncodeHal};
use crate::service::{Batch, Client, MppService, RegOffsetInfo, SliceInfo};

use super::h264e::SceneTune;
use super::h265e::{
    build_registers, refresh_base_table, IdrJob, Picture, RateControl, RefreshJob, RefreshMode,
    RegisterFile,
};
use super::h265e_regs as regs;

const CTL_OFFSET: u32 = 0;
const BASE_OFFSET: u32 = 640;
const RCKLUT_OFFSET: u32 = 4096;
const WGT_OFFSET: u32 = 5888;
const RDO_OFFSET: u32 = 8192;
const OSD_OFFSET: u32 = 12288;
const STATUS_OFFSET: u32 = 16384;
/// `VEPU580_REG_BASE_HW_STATUS` = 0x2c bytes.
const HW_STATUS_OFFSET: u32 = 44;
const STATUS_WORDS: usize = 180;
const AGGREGATE_STATUS_WORDS: [usize; 23] = [
    0, 3, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84,
];
const HW_ERROR_MASK: u32 = 0x3f0;
const OUTPUT_MARGIN: usize = 4096;
/// `h265e_init`'s intra GOP default, which the harness keeps. The rate-control SEI
/// prints it as `-g:60`, so a different value would change the prefix bytes.
const DEFAULT_IGOP: u32 = 60;

/// One encoded H.265 picture with both its complete Annex-B access unit and the
/// low-delay packet view MPP exposes through `encode_get_packet`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub annex_b: Vec<u8>,
    pub packets: Vec<Vec<u8>>,
    pub payload: Vec<u8>,
    pub headers: Vec<u8>,
    pub is_idr: bool,
    pub frame_num: u32,
    pub quality_target: i32,
    pub rt_bits: i32,
    pub statistics: Statistics,
}

/// The VEPU580 H.265 STATUS fields copied into `EncRcTaskInfo` by
/// `vepu580_h265_set_feedback`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Statistics {
    pub bitstream_bytes: u32,
    pub qp_sum: u32,
    pub madi_sum: u32,
    pub madp_sum: u32,
    pub num_b16: u32,
    pub num_ctu: u32,
    pub md_sad_b16num: [u32; 4],
    pub madi_b16num: [u32; 4],
    pub sse: u64,
    pub pnum_p64: u32,
    pub pnum_p32: u32,
    pub pnum_p16: u32,
    pub pnum_p8: u32,
    pub pnum_i32: u32,
    pub pnum_i16: u32,
    pub pnum_i8: u32,
    pub pnum_i4: u32,
}

impl Statistics {
    fn from_words(words: &[u32]) -> Self {
        let at = |word: usize| words.get(word - 4096).copied().unwrap_or(0);
        Self {
            bitstream_bytes: at(4096),
            qp_sum: at(4099),
            madi_sum: at(4170),
            madp_sum: at(4168),
            num_b16: at(4171) & 0x7f_ffff,
            num_ctu: at(4169) & 0x1f_ffff,
            md_sad_b16num: [at(4173), at(4174), at(4175), at(4176)],
            madi_b16num: [at(4177), at(4178), at(4179), at(4180)],
            sse: (u64::from(at(4098)) << 16) | u64::from((at(4097) >> 16) & 0xffff),
            pnum_p64: at(4160) & 0x1ffff,
            pnum_p32: at(4161) & 0x7ffff,
            pnum_p16: at(4162) & 0x1f_ffff,
            pnum_p8: at(4163) & 0x7f_ffff,
            pnum_i32: at(4164) & 0x7ffff,
            pnum_i16: at(4165) & 0x1f_ffff,
            pnum_i8: at(4166) & 0x7f_ffff,
            pnum_i4: at(4167) & 0x7f_ffff,
        }
    }

    pub fn quality_real(&self, width: usize, height: usize) -> i32 {
        let mb64 = ((width + 63) / 64) * ((height + 63) / 64);
        if mb64 == 0 {
            0
        } else {
            (self.qp_sum / ((mb64 << 6) as u32)) as i32
        }
    }
}

/// The three buffers of one DPB slot: the FBC pixel surface (header + body), the
/// display thumb, and the motion-vector thumb.
struct Surface {
    pixel: DmaBuf,
    dsp: DmaBuf,
    cmv: DmaBuf,
}

impl Surface {
    fn alloc(sizes: (usize, usize, usize)) -> Result<Self, String> {
        Ok(Self {
            pixel: DmaBuf::alloc("system", sizes.0, false)
                .map_err(|e| format!("allocate H.265 reconstruction: {e}"))?,
            dsp: DmaBuf::alloc("system", sizes.1, false)
                .map_err(|e| format!("allocate H.265 display thumb: {e}"))?,
            cmv: DmaBuf::alloc("system", sizes.2, false)
                .map_err(|e| format!("allocate H.265 motion-vector thumb: {e}"))?,
        })
    }
}

/// Which controller runs, and what the SEI line calls it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RcMode {
    Cbr,
    Vbr,
    Avbr,
}

impl RcMode {
    fn name(self) -> &'static str {
        match self {
            RcMode::Cbr => "cbr",
            RcMode::Vbr => "vbr",
            RcMode::Avbr => "avbr",
        }
    }

    fn model(self) -> rc_model::Mode {
        match self {
            RcMode::Cbr => rc_model::Mode::Cbr,
            RcMode::Vbr => rc_model::Mode::Vbr,
            RcMode::Avbr => rc_model::Mode::Avbr,
        }
    }
}

pub struct RcSei {
    pub target: i64,
    pub min: i64,
    pub max: i64,
    pub init_quality: i32,
    pub qp_min: u32,
    pub qp_max: u32,
    pub qp_min_i: u32,
    pub qp_max_i: u32,
    pub i_quality_delta: i32,
}

impl RcSei {
    /// The strategy line `mpi_enc_test` installs with its explicit `-qc` band.
    pub const fn from_harness(
        target: i64,
        min: i64,
        max: i64,
        init_quality: i32,
        qp_min: u32,
        qp_max: u32,
    ) -> Self {
        Self {
            target,
            min,
            max,
            init_quality,
            qp_min,
            qp_max,
            qp_min_i: qp_min,
            qp_max_i: qp_max,
            i_quality_delta: 2,
        }
    }
}

pub struct Vepu580H265 {
    /// Present when the session runs rate control.
    rc_sei: Option<RcSei>,
    rc_mode: RcMode,
    /// The rate-control model, when the session picks QPs itself.
    rc: Option<rc_model::Model>,
    rc_band: (u32, u32),
    /// Intra-refresh (GDR): the sweep and band size, plus the table it rewrites.
    refresh: Option<(RefreshMode, u32)>,
    refresh_buf: Option<DmaBuf>,
    gop: u32,
    refresh_length: u32,
    /// The finished picture'sSTATUS measurements, taken by `encode_picture_rc`.
    last_measured: Option<rc_model::Measured>,
    /// `MppEncSliceSplit`, fixed for the session before the first picture.
    split: SliceSplit,
    /// Static ROI base and QP tables, allocated when the first frame is encoded.
    roi: Option<(DmaBuf, DmaBuf)>,
    syntax: SyntaxOptions,
    service: MppService,
    width: usize,
    height: usize,
    qp: u32,
    /// `general_level_idc` written into VPS/SPS. `h265e_init` starts at 4.0 and
    /// raises this by picture size; the object path may request a lower level.
    level: u8,
    temporal_mvp: bool,
    video_format: Option<u8>,
    video_full_range: bool,
    force_cu_qp_delta: bool,
    tune: SceneTune,
    qbias: Option<(bool, u32, u32)>,
    fixed_qp_header_override: Option<u32>,
    /// Emit the GOP SPS variant (long-term reference entry) and let `encode_frame`
    /// produce P pictures.
    long_term: bool,
    input: DmaBuf,
    input_padding_initialized: bool,
    input_prepared: bool,
    outputs: [DmaBuf; 2],
    pipeline: bool,
    pipeline_depth: usize,
    pipeline_output: usize,
    /// VEPU580 frame-parallel dual-core handshake state. C's `task_cnt == 2`
    /// path advances this on every `gen_regs`, independent of which task slot
    /// later completes. An intra picture restarts both IDs at zero.
    frame_parallel_curr: u32,
    frame_parallel_prev: u32,
    pending_jobs: VecDeque<PendingH265Job>,
    /// Slot 0 is the opening IDR's reconstruction. Synchronous paths use one
    /// other slot; the depth-2 compatibility path alternates two P output
    /// surfaces so an in-flight job never overwrites the next reconstruction.
    surfaces: [Surface; 3],
    slot: usize,
    /// Pictures this session has encoded; 0 means the next one is the IDR.
    frames: u32,
    /// Global frame index of the most recent IDR, used to reset POC.
    idr_frame: u32,
    md_info: DmaBuf,
    tile_line_bufs: Vec<DmaBuf>,
    tile_warmed: bool,
    /// The service reopen count observed when the tile warm-up state was made.
    /// A background recovery replaces the fd while this object stays alive, so
    /// the first tiled picture must be warmed again against the replacement.
    service_reopens: u64,
    /// A failed hardware job invalidates the reference epoch. The next frame
    /// must be an IDR carrying parameter sets.
    recovery_pending: bool,
    input_fd_override: Option<i32>,
    force_next_idr: bool,
    headers_each_idr: bool,
    sei_enabled: bool,
    rc_model_name: String,
    next_osd: Option<crate::hal::osd::Config>,
    active_osd: Option<crate::hal::osd::Config>,
    status_scratch: Vec<u32>,
    aggregate_status_scratch: Vec<u32>,
    batch_scratch: Batch,
}

struct PendingH265Job {
    _ctl: Vec<u32>,
    _base: Vec<u32>,
    _rc_klut: Vec<u32>,
    _wgt: Vec<u32>,
    _rdo: Vec<u32>,
    _osd: Vec<u32>,
    _offsets: Vec<RegOffsetInfo>,
    batch: Batch,
    status: Vec<u32>,
    hw_status: Vec<u32>,
    output_slot: usize,
    picture: Picture,
    frame_num: u32,
    with_prefix: bool,
    include_annex_b: bool,
}

fn apply_frame_parallel_handshake(
    registers: &mut RegisterFile,
    is_intra: bool,
    curr: &mut u32,
    prev: &mut u32,
) -> Result<(), String> {
    if is_intra {
        *curr = 0;
        *prev = 0;
    }
    let rxe = if is_intra { 0 } else { 1 };
    registers.set(&regs::BASE_DUAL_CORE_DCHS_TXID, *curr)?;
    registers.set(&regs::BASE_DUAL_CORE_DCHS_RXID, *prev)?;
    registers.set(&regs::BASE_DUAL_CORE_DCHS_TXE, 1)?;
    registers.set(&regs::BASE_DUAL_CORE_DCHS_RXE, rxe)?;
    registers.set(&regs::BASE_DUAL_CORE_DCHS_OFST, 9)?;

    *prev = *curr;
    *curr += 1;
    if *curr > 3 {
        *curr = 0;
    }
    Ok(())
}

impl Vepu580H265 {
    /// A single-picture session: the IDR gate.
    pub fn open(width: usize, height: usize, qp: u32) -> Result<Self, String> {
        Self::open_inner(width, height, qp, false)
    }

    /// A GOP session: the SPS carries the long-term reference entry the harness
    /// writes when it is given an explicit `-g`, and later pictures are P.
    pub fn open_gop(width: usize, height: usize, qp: u32) -> Result<Self, String> {
        Self::open_inner(width, height, qp, true)
    }

    /// A GOP session whose reference is short-term: the shape a GDR/refresh job
    /// takes, with no SPS long-term entry.
    pub fn open_gop_short_term(width: usize, height: usize, qp: u32) -> Result<Self, String> {
        Self::open_inner(width, height, qp, false)
    }

    /// A CBR session whose model picks the per-picture QP and bit target.
    pub fn open_rate(
        width: usize,
        height: usize,
        mode: RcMode,
        bps_target: i32,
        bps_min: i32,
        bps_max: i32,
        init_qp: i32,
        qp_min: u32,
        qp_max: u32,
    ) -> Result<Self, String> {
        let mut session = Self::open_inner(width, height, qp_max, false)?;
        let mb64 = ((width + 63) / 64) * ((height + 63) / 64);
        let mut config = rc_model::Config::cbr(bps_target, bps_min, bps_max, 60);
        config.quality_init = init_qp;
        config.quality_min = qp_min as i32;
        config.quality_max = qp_max as i32;
        // `mpi_enc_test` sets the I and P quality bands to the same caller range
        // and uses the harness I delta of two in the strategy SEI.
        config.i_quality_delta = 2;
        config.total_mb = (mb64 << 6) as i32;
        config.mode = mode.model();
        session.rc_mode = mode;
        session.rc = Some(rc_model::Model::new(config));
        session.rc_band = (qp_min, qp_max);
        session.set_rate_control_sei(RcSei::from_harness(
            bps_target as i64,
            bps_min as i64,
            bps_max as i64,
            init_qp,
            qp_min,
            qp_max,
        ));
        Ok(session)
    }

    fn open_inner(width: usize, height: usize, qp: u32, long_term: bool) -> Result<Self, String> {
        require_video_encode(VideoEncodeHal::Vepu580)?;
        if width == 0 || height == 0 {
            return Err("H.265 session needs a non-zero picture".into());
        }
        if qp > 51 {
            return Err(format!("H.265 QP {qp} is outside 0..=51"));
        }
        let sizes = surface_sizes(width, height);
        let input_size = luma_bytes(width, height)
            .checked_mul(3)
            .map(|v| v / 2)
            .ok_or_else(|| "H.265 input size overflow".to_string())?;
        let output_size = input_size
            .checked_add(OUTPUT_MARGIN)
            .ok_or_else(|| "H.265 output size overflow".to_string())?;
        let outputs = [
            DmaBuf::alloc("system-uncached", output_size, false)
                .map_err(|e| format!("allocate H.265 output 0: {e}"))?,
            DmaBuf::alloc("system-uncached", output_size, false)
                .map_err(|e| format!("allocate H.265 output 1: {e}"))?,
        ];
        Ok(Self {
            service: MppService::open(Client::RkvEnc as u32)
                .map_err(|e| format!("open rkv encoder: {e}"))?,
            width,
            height,
            qp,
            level: 120,
            temporal_mvp: false,
            video_format: None,
            video_full_range: false,
            force_cu_qp_delta: false,
            tune: SceneTune::new_h265(false),
            qbias: None,
            fixed_qp_header_override: None,
            long_term,
            rc_sei: None,
            rc_mode: RcMode::Cbr,
            rc: None,
            rc_band: (8, 48),
            refresh: None,
            refresh_buf: None,
            gop: DEFAULT_IGOP,
            refresh_length: 0,
            last_measured: None,
            split: SliceSplit::default(),
            roi: None,
            syntax: SyntaxOptions::default(),
            input: DmaBuf::alloc("system-uncached", input_size, false)
                .map_err(|e| format!("allocate H.265 input: {e}"))?,
            input_padding_initialized: false,
            input_prepared: false,
            // The packets are only a few KiB. Keeping them uncached makes the
            // hardware result CPU-visible without two sync ioctls per picture.
            outputs,
            pipeline: false,
            pipeline_depth: 2,
            pipeline_output: 0,
            frame_parallel_curr: 0,
            frame_parallel_prev: 0,
            pending_jobs: VecDeque::new(),
            surfaces: [
                Surface::alloc(sizes)?,
                Surface::alloc(sizes)?,
                Surface::alloc(sizes)?,
            ],
            slot: 0,
            frames: 0,
            idr_frame: 0,
            md_info: DmaBuf::alloc("system", md_info_size(width, height), false)
                .map_err(|e| format!("allocate H.265 motion info: {e}"))?,
            tile_line_bufs: Vec::new(),
            tile_warmed: false,
            service_reopens: 0,
            recovery_pending: false,
            input_fd_override: None,
            force_next_idr: false,
            headers_each_idr: false,
            sei_enabled: true,
            rc_model_name: "default".to_string(),
            next_osd: None,
            active_osd: None,
            status_scratch: vec![0; STATUS_WORDS],
            aggregate_status_scratch: vec![0; STATUS_WORDS],
            batch_scratch: Batch::new(),
        })
    }

    fn prepare_pipeline_job(
        &mut self,
        input_fd: i32,
        picture: Picture,
        frame_num: u32,
        with_prefix: bool,
        include_annex_b: bool,
    ) -> Result<PendingH265Job, String> {
        let output_slot = self.pipeline_output;
        self.pipeline_output ^= 1;
        let output_fd = self.outputs[output_slot].fd();
        let (current, reference) = if matches!(picture, Picture::Idr) {
            (self.slot, self.slot)
        } else if self.long_term {
            (1 + (frame_num.saturating_sub(1) as usize & 1), 0)
        } else {
            let current = frame_num as usize % self.surfaces.len();
            (
                current,
                (current + self.surfaces.len() - 1) % self.surfaces.len(),
            )
        };
        let current_pixel = self.surfaces[current].pixel.fd();
        let current_dsp = self.surfaces[current].dsp.fd();
        let reference_pixel = self.surfaces[reference].pixel.fd();
        let reference_dsp = self.surfaces[reference].dsp.fd();
        let job = IdrJob {
            width: self.width,
            height: self.height,
            qp: self.qp,
            qp_min: self.qp,
            qp_max: self.qp,
            picture,
            long_term: self.long_term,
            temporal_mvp: self.temporal_mvp,
            force_cu_qp_delta: self.force_cu_qp_delta,
            scene_motion_flag: self.tune.scene_motion_flag(),
            qbias: self.qbias,
            rc: None,
            refresh: None,
            split: self.split,
            syntax: self.syntax,
            tile: None,
            roi_fd: self.roi.as_ref().map_or(-1, |buffers| buffers.0.fd()),
            roi_qp_fd: self.roi.as_ref().map_or(-1, |buffers| buffers.1.fd()),
            input_fd,
            output_fd,
            current_fd: current_pixel,
            current_thumb_fd: current_dsp,
            reference_fd: reference_pixel,
            reference_thumb_fd: reference_dsp,
        };
        let mut registers = build_registers(&job)?;
        if self.pipeline_depth > 1 {
            apply_frame_parallel_handshake(
                &mut registers,
                matches!(picture, Picture::Idr | Picture::ForcedIdrFromP { .. }),
                &mut self.frame_parallel_curr,
                &mut self.frame_parallel_prev,
            )?;
        }
        let input = input_fd as u32;
        let output = output_fd as u32;
        let base = &mut registers.base;
        base[0] = input;
        base[1] = input;
        base[2] = input;
        base[3] = current_pixel as u32;
        base[4] = current_pixel as u32;
        base[5] = reference_pixel as u32;
        base[6] = reference_pixel as u32;
        base[7] = self.surfaces[current].cmv.fd() as u32;
        base[8] = self.surfaces[reference].cmv.fd() as u32;
        base[9] = current_dsp as u32;
        base[10] = reference_dsp as u32;
        base[11] = self.md_info.fd() as u32;
        base[12] = output;
        base[13] = output;
        base[14] = output;
        base[15] = output;
        registers.set(&regs::BASE_ENC_PIC_MEI_STOR, 1)?;
        let header = fbc_header_len(self.width, self.height);
        let luma = luma_bytes(self.width, self.height) as u32;
        registers.offsets.extend([
            RegOffsetInfo {
                reg_idx: 161,
                offset: luma,
            },
            RegOffsetInfo {
                reg_idx: 162,
                offset: luma,
            },
            RegOffsetInfo {
                reg_idx: 164,
                offset: header as u32,
            },
            RegOffsetInfo {
                reg_idx: 166,
                offset: header as u32,
            },
            RegOffsetInfo {
                reg_idx: 175,
                offset: 0,
            },
            RegOffsetInfo {
                reg_idx: 172,
                offset: self.outputs[output_slot].len() as u32,
            },
        ]);

        let ctl = registers.ctl.to_vec();
        let base = registers.base.to_vec();
        let rc_klut = registers.rc_klut.to_vec();
        let wgt = registers.wgt.to_vec();
        let rdo = registers.rdo.to_vec();
        let osd = registers.osd.to_vec();
        let offsets = registers.offsets;
        let mut status = vec![0u32; STATUS_WORDS];
        let mut hw_status = vec![0u32; 1];
        let mut batch = Batch::new();
        batch
            .reg_write(&ctl, CTL_OFFSET)
            .reg_write(&base, BASE_OFFSET)
            .reg_write(&rc_klut, RCKLUT_OFFSET)
            .reg_write(&wgt, WGT_OFFSET)
            .reg_write(&rdo, RDO_OFFSET)
            .reg_write(&osd, OSD_OFFSET)
            .reg_offsets(&offsets)
            .reg_read(&mut hw_status, HW_STATUS_OFFSET)
            .reg_read(&mut status, STATUS_OFFSET);
        Ok(PendingH265Job {
            _ctl: ctl,
            _base: base,
            _rc_klut: rc_klut,
            _wgt: wgt,
            _rdo: rdo,
            _osd: osd,
            _offsets: offsets,
            batch,
            status,
            hw_status,
            output_slot,
            picture,
            frame_num,
            with_prefix,
            include_annex_b,
        })
    }

    fn wait_pipeline_job(&mut self, pending: &PendingH265Job) -> Result<(), String> {
        if let Err(error) = self.service.poll_hw_finish_blocking() {
            let recovery = self.service.restart_after_hardware_error();
            return Err(format!(
                "H.265 pipeline blocking poll: {error}; recovery: {recovery:?}"
            ));
        }
        let errors = pending.hw_status[0] & HW_ERROR_MASK;
        if errors != 0 {
            let _ = self.service.restart_after_hardware_error();
            return Err(format!(
                "VEPU580 H.265 hardware error mask 0x{errors:03x} (int_sta=0x{:08x})",
                pending.hw_status[0]
            ));
        }
        if pending.hw_status[0] & 1 == 0 {
            let _ = self.service.restart_after_hardware_error();
            return Err(format!(
                "VEPU580 H.265 pipeline completed without enc_done (int_sta=0x{:08x})",
                pending.hw_status[0]
            ));
        }
        Ok(())
    }

    fn finish_pipeline_job(&mut self, pending: PendingH265Job) -> Result<EncodedFrame, String> {
        self.wait_pipeline_job(&pending)?;
        self.assemble_pipeline_job(pending)
    }

    fn assemble_pipeline_job(&mut self, pending: PendingH265Job) -> Result<EncodedFrame, String> {
        let output = &self.outputs[pending.output_slot];
        let length = pending.status[0] as usize;
        if length == 0 || length > output.len() {
            let _ = self.service.restart_after_hardware_error();
            return Err(format!(
                "invalid VEPU580 H.265 pipeline length {length} (capacity {})",
                output.len()
            ));
        }
        let payload = output.as_slice()[..length].to_vec();
        let statistics = Statistics::from_words(&pending.status);
        self.update_tune(&statistics);
        let mut config = crate::codec::h265::encoder::EncoderConfig::new(self.width, self.height);
        config.level = self.level;
        config.tmvp_enable = self.temporal_mvp;
        config.video_format = self.video_format;
        config.video_full_range = self.video_full_range;
        config.long_term_refs = self.long_term;
        config.apply_syntax_options(self.syntax);
        config.force_cu_qp_delta = self.force_cu_qp_delta;
        let headers = if !pending.with_prefix {
            Vec::new()
        } else if !self.sei_enabled {
            crate::codec::h265::encoder::build_headers(&config)
                .map_err(|e| format!("H.265 headers: {e:?}"))?
        } else {
            let line = crate::codec::h265::sei::RcCfgLine {
                model_name: &self.rc_model_name,
                ..crate::codec::h265::sei::RcCfgLine::fixed_qp(
                    self.fixed_qp_header_override.unwrap_or(self.qp),
                    self.gop,
                    crate::codec::h265::sei::default_bps_target(config.width, config.height, 30),
                )
            };
            crate::codec::h265::encoder::build_prefix_line(&config, &line)
                .map_err(|e| format!("H.265 prefix: {e:?}"))?
        };
        let packets = output_packets(&headers, &payload, &[])?;
        let annex_b = if pending.include_annex_b {
            let mut out = Vec::with_capacity(headers.len() + payload.len());
            out.extend_from_slice(&headers);
            out.extend_from_slice(&payload);
            out
        } else {
            Vec::new()
        };
        Ok(EncodedFrame {
            annex_b,
            packets,
            payload,
            headers,
            is_idr: matches!(
                pending.picture,
                Picture::Idr | Picture::ForcedIdrFromP { .. }
            ),
            frame_num: pending.frame_num,
            quality_target: self.qp as i32,
            rt_bits: 0,
            statistics,
        })
    }

    pub fn encode_frame_fd_pipeline(&mut self, input_fd: i32) -> Result<Vec<EncodedFrame>, String> {
        if !self.pipeline
            || self.rc.is_some()
            || self.refresh_length != 0
            || self.tile_widths().len() != 1
            || self.split.out & MPP_ENC_SPLIT_OUT_LOWDELAY != 0
        {
            return Err("H.265 pipeline is disabled or incompatible with this session".into());
        }
        if input_fd < 0 {
            return Err("H.265 input fd is invalid".to_string());
        }
        if self.next_osd.is_some() {
            return Err("H.265 pipeline does not support per-frame OSD".into());
        }
        self.synchronize_service_recovery()?;
        if self.recovery_pending {
            return Err("H.265 pipeline requires a synchronous recovery IDR".into());
        }
        let force_idr = std::mem::take(&mut self.force_next_idr);
        let gop_idr = self.frames != 0 && self.frames % self.gop == 0;
        let is_idr = self.frames == 0 || force_idr || gop_idr;
        if is_idr {
            self.slot = 0;
            self.idr_frame = self.frames;
        }
        let frame_num = self.frames;
        let (picture, with_prefix) = if is_idr {
            (Picture::Idr, self.frames == 0 || self.headers_each_idr)
        } else {
            (
                Picture::P {
                    poc_lsb: self.frames.saturating_sub(self.idr_frame),
                },
                false,
            )
        };
        let mut completed = Vec::new();
        if self.pending_jobs.len() >= self.pipeline_depth {
            let pending = self.pending_jobs.pop_front().unwrap();
            completed.push(self.finish_pipeline_job(pending)?);
        }
        let mut pending =
            self.prepare_pipeline_job(input_fd, picture, frame_num, with_prefix, false)?;
        self.service
            .submit_batch_recovering(&mut pending.batch)
            .map_err(|e| format!("H.265 pipeline submit: {e}"))?;
        self.pending_jobs.push_back(pending);
        self.slot = 1 - self.slot;
        self.frames += 1;
        self.recovery_pending = false;
        Ok(completed)
    }

    pub fn flush_pipeline(&mut self) -> Result<Vec<EncodedFrame>, String> {
        let mut completed = Vec::new();
        while let Some(pending) = self.pending_jobs.pop_front() {
            completed.push(self.finish_pipeline_job(pending)?);
        }
        Ok(completed)
    }

    /// Encode the next picture. The first call is an IDR and later calls are P
    /// pictures unless the caller requested another IDR. Parameter sets and SEIs
    /// are emitted on the opening IDR, or on every IDR in per-IDR header mode.
    pub fn encode_frame(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        Ok(self.encode_frame_packets(nv12)?.annex_b)
    }

    pub fn encode_frame_fd(&mut self, input_fd: i32) -> Result<Vec<u8>, String> {
        Ok(self.encode_frame_packets_fd(input_fd)?.annex_b)
    }

    /// Encode the next picture and retain MPP's packet view. With low-delay
    /// slicing this has one packet per completed slice; otherwise it is one
    /// complete access unit.
    pub fn encode_frame_packets(&mut self, nv12: &[u8]) -> Result<EncodedFrame, String> {
        self.encode_frame_packets_inner(nv12, true)
    }

    pub(crate) fn encode_frame_packets_for_queue(
        &mut self,
        nv12: &[u8],
    ) -> Result<EncodedFrame, String> {
        self.encode_frame_packets_inner(nv12, false)
    }

    pub(crate) fn encode_visible_frame_packets_for_queue(
        &mut self,
        nv12: &[u8],
    ) -> Result<EncodedFrame, String> {
        self.load_visible_input_frame(nv12)?;
        let result = self.encode_frame_packets_inner(&[], false);
        self.input_prepared = false;
        result
    }

    fn encode_frame_packets_inner(
        &mut self,
        nv12: &[u8],
        include_annex_b: bool,
    ) -> Result<EncodedFrame, String> {
        self.synchronize_service_recovery()?;
        let recovery_restart = self.recovery_pending;
        let force_idr = std::mem::take(&mut self.force_next_idr);
        let gop_idr = self.refresh_length == 0 && self.frames != 0 && self.frames % self.gop == 0;
        let is_idr = self.frames == 0 || force_idr || recovery_restart || gop_idr;
        if force_idr || recovery_restart || gop_idr {
            self.slot = 0;
        }
        if is_idr {
            self.idr_frame = self.frames;
        }
        if self.rc.is_some() {
            let frame = if is_idr {
                rc_model::FrameType::Intra
            } else {
                rc_model::FrameType::InterP
            };
            let (bit_target, qp) = self.rc.as_mut().unwrap().start(frame);
            if std::env::var_os("H265_RC_DEBUG").is_some() {
                eprintln!(
                    "h265 rc: frame {} bit_target {} qp {}",
                    self.frames, bit_target, qp
                );
            }
            let recovery_prefix =
                !is_idr && self.refresh_length != 0 && self.frames % self.gop == 0;
            let (picture, prefix) = if is_idr {
                (
                    Picture::Idr,
                    self.frames == 0 || self.headers_each_idr || recovery_restart,
                )
            } else {
                (
                    Picture::P {
                        poc_lsb: self.frames.saturating_sub(self.idr_frame),
                    },
                    false,
                )
            };
            let rc = RateControl {
                bit_target,
                qp_target: qp as u32,
                qp_min: self.rc_band.0,
                qp_max: self.rc_band.1,
                qp_delta_row: 2,
                qp_delta_row_i: 2,
            };
            let mut encoded = self.encode_picture_rc(
                nv12,
                picture,
                prefix,
                recovery_prefix,
                Some(rc),
                include_annex_b,
            )?;
            if let Some(measured) = self.last_measured.take() {
                if std::env::var_os("H265_RC_DEBUG").is_some() {
                    eprintln!("h265 rc measured: {:?}", measured);
                }
                self.rc
                    .as_mut()
                    .unwrap()
                    .end_with(frame, bit_target, measured);
                encoded.rt_bits = self.rc.as_ref().unwrap().ins_bps();
            }
            self.slot = 1 - self.slot;
            self.frames += 1;
            self.recovery_pending = false;
            return Ok(encoded);
        }
        let recovery_prefix = !is_idr && self.refresh_length != 0 && self.frames % self.gop == 0;
        let (picture, prefix) = if is_idr {
            (
                Picture::Idr,
                self.frames == 0 || self.headers_each_idr || recovery_restart,
            )
        } else {
            let poc = self.frames.saturating_sub(self.idr_frame);
            (Picture::P { poc_lsb: poc }, false)
        };
        let encoded = self.encode_picture_rc(
            nv12,
            picture,
            prefix,
            recovery_prefix,
            None,
            include_annex_b,
        )?;
        self.slot = 1 - self.slot;
        self.frames += 1;
        self.recovery_pending = false;
        Ok(encoded)
    }

    pub fn encode_frame_packets_fd(&mut self, input_fd: i32) -> Result<EncodedFrame, String> {
        if input_fd < 0 {
            return Err("H.265 input fd is invalid".to_string());
        }
        self.input_fd_override = Some(input_fd);
        self.encode_frame_packets(&[])
    }

    pub(crate) fn encode_frame_packets_fd_for_queue(
        &mut self,
        input_fd: i32,
    ) -> Result<EncodedFrame, String> {
        if input_fd < 0 {
            return Err("H.265 input fd is invalid".to_string());
        }
        self.input_fd_override = Some(input_fd);
        self.encode_frame_packets_for_queue(&[])
    }

    fn encode_picture_packets(
        &mut self,
        nv12: &[u8],
        picture: Picture,
        with_prefix: bool,
    ) -> Result<EncodedFrame, String> {
        self.encode_picture_rc(nv12, picture, with_prefix, false, None, true)
    }

    /// One picture with an explicit rate-control decision: the same job the model
    /// would produce, but decided by the caller. This is how the RC *path* is
    /// checked against a traced C stream before the model's own QP choice is.
    /// Turn on intra refresh. `refresh_num` is the refresh-band width in CTUs per
    /// picture; every picture after the IDR sweeps one band.
    pub fn enable_refresh(&mut self, mode: RefreshMode, refresh_num: u32) -> Result<(), String> {
        if refresh_num == 0 {
            return Err("H.265 refresh_num must be non-zero".into());
        }
        let ctu_w = (self.width + 63) / 64;
        let ctu_h = (self.height + 63) / 64;
        let size = ctu_w * ctu_h * 64;
        self.refresh_buf = Some(
            DmaBuf::alloc("system", size, false)
                .map_err(|e| format!("allocate H.265 refresh table: {e}"))?,
        );
        self.refresh = Some((mode, refresh_num));
        let lines = match mode {
            RefreshMode::Row => (self.height + 31) / 32,
            RefreshMode::Col => (self.width + 31) / 32,
        } as u32;
        self.refresh_length = lines.div_ceil(refresh_num).min(self.gop);
        Ok(())
    }

    pub fn set_gop(&mut self, gop: u32) -> Result<(), String> {
        if gop == 0 {
            return Err("H.265 GOP must be non-zero".into());
        }
        self.gop = gop;
        if let Some((mode, refresh_num)) = self.refresh {
            let lines = match mode {
                RefreshMode::Row => (self.height + 31) / 32,
                RefreshMode::Col => (self.width + 31) / 32,
            } as u32;
            self.refresh_length = lines.div_ceil(refresh_num).min(self.gop);
        }
        Ok(())
    }

    pub fn force_idr(&mut self) {
        self.force_next_idr = true;
    }

    pub fn set_long_term(&mut self, long_term: bool) {
        self.long_term = long_term;
    }

    pub fn set_level(&mut self, level: u8) {
        self.level = level;
    }

    pub fn set_temporal_mvp(&mut self, enabled: bool) {
        self.temporal_mvp = enabled;
    }

    pub fn set_video_signal(&mut self, format: Option<u8>, full_range: bool) {
        self.video_format = format;
        self.video_full_range = full_range;
    }

    pub fn set_force_cu_qp_delta(&mut self, enabled: bool) {
        self.force_cu_qp_delta = enabled;
    }

    pub fn set_scene_mode(&mut self, ipc_scene_mode: bool) {
        self.tune = SceneTune::new_h265(ipc_scene_mode);
    }

    pub fn set_qbias(&mut self, qbias: Option<(bool, u32, u32)>) {
        self.qbias = qbias;
    }

    fn update_tune(&mut self, statistics: &Statistics) {
        self.tune.update_values(
            statistics.madi_sum,
            statistics.madp_sum,
            statistics.num_b16,
            statistics.num_ctu,
            statistics.md_sad_b16num,
            i64::from(statistics.num_b16),
        );
    }

    pub fn set_pipeline(&mut self, enabled: bool) {
        self.pipeline = enabled;
    }

    pub fn set_pipeline_depth(&mut self, depth: usize) {
        self.pipeline_depth = depth.clamp(1, 2);
    }

    pub fn pipeline_enabled(&self) -> bool {
        self.pipeline
    }

    pub fn pipeline_empty(&self) -> bool {
        self.pending_jobs.is_empty()
    }

    pub fn set_headers_each_idr(&mut self, each_idr: bool) {
        self.headers_each_idr = each_idr;
    }

    pub fn set_sei_enabled(&mut self, enabled: bool) {
        self.sei_enabled = enabled;
    }

    pub fn set_rate_control_name(&mut self, name: String) {
        self.rc_model_name = name;
    }

    pub fn set_next_osd(&mut self, osd: Option<crate::hal::osd::Config>) {
        self.next_osd = osd;
    }

    /// Set `MppEncSliceSplit` before the opening picture. Low-delay output uses
    /// one-entry `POLL_HW_IRQ` batches, matching the C HAL when
    /// `MPP_ENC_SPLIT_OUT_LOWDELAY` is set.
    pub fn set_slice_split(&mut self, split: SliceSplit) -> Result<(), String> {
        if self.frames != 0 {
            return Err("H.265 slice split must be configured before the first picture".into());
        }
        if split.out & MPP_ENC_SPLIT_OUT_LOWDELAY != 0 && self.tile_columns() > 1 {
            return Err("H.265 tile + low-delay slice output is not verified".into());
        }
        split
            .validate()
            .map_err(|error| format!("H.265 slice split: {error}"))?;
        self.split = split;
        Ok(())
    }

    pub fn set_syntax_options(&mut self, syntax: SyntaxOptions) {
        self.syntax = syntax;
    }

    /// Recovery counters for the underlying `mpp_service` client.
    pub fn recovery_stats(&self) -> crate::service::RecoveryStats {
        self.service.recovery_stats()
    }

    /// A hardware failure closes and reopens the service fd underneath this
    /// session. The VEPU580's dual-core tile handshake is not reset by that
    /// replacement, so discard both the warm-up flag and the line-buffer state
    /// that belonged to the failed fd. The next tiled picture will perform the
    /// same discarded prepass as a freshly opened session.
    fn synchronize_service_recovery(&mut self) -> Result<(), String> {
        let reopens = self.service.recovery_stats().session_reopens;
        if reopens == self.service_reopens {
            return Ok(());
        }
        self.tile_warmed = false;
        for (index, buffer) in self.tile_line_bufs.iter_mut().enumerate() {
            buffer
                .sync_write_begin()
                .map_err(|e| format!("H.265 tile line buffer {index} recovery sync: {e}"))?;
            buffer.as_mut_slice().fill(0);
            buffer
                .sync_write_end()
                .map_err(|e| format!("H.265 tile line buffer {index} recovery flush: {e}"))?;
        }
        self.recovery_pending = true;
        self.service_reopens = reopens;
        Ok(())
    }

    fn poll_picture(
        &mut self,
        tile_job: bool,
        slice_lengths: &mut Vec<usize>,
    ) -> Result<(), String> {
        if self.split.out & MPP_ENC_SPLIT_OUT_LOWDELAY != 0 && !tile_job {
            let mut complete = false;
            for _ in 0..=4096 {
                let mut info = [SliceInfo::default()];
                let count = self
                    .service
                    .poll_irq(&mut info)
                    .map_err(|e| format!("H.265 slice poll: {e}"))?;
                for slice in &info[..count] {
                    let (length, last) = slice.vepu580();
                    slice_lengths.push(length as usize);
                    complete |= last;
                }
                if complete {
                    break;
                }
            }
            if !complete {
                return Err("H.265 slice FIFO did not report the last slice".into());
            }
        } else {
            self.service
                .poll_hw_finish_recovering()
                .map_err(|e| format!("H.265 poll: {e}"))?;
        }
        Ok(())
    }

    /// The session's tile column count, asked of the same derivation that writes
    /// `num_tile_columns_minus1` into the PPS. Duplicating the thresholds here is
    /// how a PPS and a job window would drift apart.
    fn tile_columns(&self) -> usize {
        let mut config = crate::codec::h265::encoder::EncoderConfig::new(self.width, self.height);
        config.apply_syntax_options(self.syntax);
        config.tile_columns_minus1() as usize + 1
    }

    fn tile_widths(&self) -> Vec<(u32, u32)> {
        uniform_tile_widths(self.width, self.tile_columns())
    }

    fn ensure_tile_line_bufs(&mut self, columns: usize) -> Result<(), String> {
        if self.tile_line_bufs.len() >= columns {
            return Ok(());
        }
        let size = align((align(self.height, 64) + 64) << 5, 256);
        while self.tile_line_bufs.len() < columns {
            let mut buffer = DmaBuf::alloc("system-uncached", size, false)
                .map_err(|e| format!("allocate H.265 tile line buffer: {e}"))?;
            buffer
                .sync_write_begin()
                .map_err(|e| format!("H.265 tile line buffer sync begin: {e}"))?;
            buffer.as_mut_slice().fill(0);
            buffer
                .sync_write_end()
                .map_err(|e| format!("H.265 tile line buffer sync end: {e}"))?;
            self.tile_line_bufs.push(buffer);
        }
        Ok(())
    }

    /// Configure the same regions on every picture, like the reference harness's
    /// `mpp_enc_roi_setup_meta`. The two tables are static for this session.
    pub fn set_roi_regions(&mut self, regions: &[RoiRegion]) -> Result<(), String> {
        if self.frames != 0 {
            return Err("H.265 ROI must be configured before the first picture".into());
        }
        if regions.is_empty() {
            self.roi = None;
            return Ok(());
        }
        let tables =
            build_roi_tables(self.width, self.height, regions).map_err(|e| e.to_string())?;
        let mut base = DmaBuf::alloc("system", tables.base.len(), false)
            .map_err(|e| format!("allocate H.265 ROI base: {e}"))?;
        let mut qp = DmaBuf::alloc("system", tables.qp.len(), false)
            .map_err(|e| format!("allocate H.265 ROI QP: {e}"))?;
        base.sync_write_begin()
            .map_err(|e| format!("H.265 ROI base sync: {e}"))?;
        base.as_mut_slice()[..tables.base.len()].copy_from_slice(&tables.base);
        base.sync_write_end()
            .map_err(|e| format!("H.265 ROI base flush: {e}"))?;
        qp.sync_write_begin()
            .map_err(|e| format!("H.265 ROI QP sync: {e}"))?;
        qp.as_mut_slice()[..tables.qp.len()].copy_from_slice(&tables.qp);
        qp.sync_write_end()
            .map_err(|e| format!("H.265 ROI QP flush: {e}"))?;
        self.roi = Some((base, qp));
        Ok(())
    }

    pub fn set_rate_control_sei(&mut self, sei: RcSei) {
        self.rc_sei = Some(sei);
    }

    pub fn encode_frame_with_rc(
        &mut self,
        nv12: &[u8],
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
    ) -> Result<Vec<u8>, String> {
        Ok(self
            .encode_frame_with_rc_packets(nv12, bit_target, qp_target, qp_min, qp_max)?
            .annex_b)
    }

    pub fn encode_frame_with_rc_packets(
        &mut self,
        nv12: &[u8],
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
    ) -> Result<EncodedFrame, String> {
        self.encode_frame_with_rc_packets_inner(nv12, bit_target, qp_target, qp_min, qp_max, true)
    }

    pub(crate) fn encode_frame_with_rc_packets_for_queue(
        &mut self,
        nv12: &[u8],
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
    ) -> Result<EncodedFrame, String> {
        self.encode_frame_with_rc_packets_inner(nv12, bit_target, qp_target, qp_min, qp_max, false)
    }

    pub(crate) fn encode_visible_frame_with_rc_packets_for_queue(
        &mut self,
        nv12: &[u8],
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
    ) -> Result<EncodedFrame, String> {
        self.load_visible_input_frame(nv12)?;
        let result = self.encode_frame_with_rc_packets_inner(
            &[],
            bit_target,
            qp_target,
            qp_min,
            qp_max,
            false,
        );
        self.input_prepared = false;
        result
    }

    fn encode_frame_with_rc_packets_inner(
        &mut self,
        nv12: &[u8],
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
        include_annex_b: bool,
    ) -> Result<EncodedFrame, String> {
        self.encode_frame_with_rc_policy(
            nv12,
            Some(ExternalRateControl {
                bit_target,
                qp_target,
                qp_min,
                qp_max,
            }),
            None,
            include_annex_b,
        )
    }

    pub fn encode_frame_with_rc_packets_fd(
        &mut self,
        input_fd: i32,
        bit_target: i32,
        qp_target: u32,
        qp_min: u32,
        qp_max: u32,
    ) -> Result<EncodedFrame, String> {
        if input_fd < 0 {
            return Err("H.265 input fd is invalid".to_string());
        }
        self.input_fd_override = Some(input_fd);
        self.encode_frame_with_rc_packets(&[], bit_target, qp_target, qp_min, qp_max)
    }

    pub(crate) fn encode_frame_with_rc_reencode(
        &mut self,
        nv12: &[u8],
        initial: ExternalRateControl,
        reencode: &mut dyn FnMut(
            RateControlFeedback,
        ) -> Result<Option<RateControlReencode>, String>,
    ) -> Result<EncodedFrame, String> {
        self.encode_frame_with_rc_policy(nv12, Some(initial), Some(reencode), false)
    }

    pub(crate) fn encode_visible_frame_with_rc_reencode(
        &mut self,
        nv12: &[u8],
        initial: ExternalRateControl,
        reencode: &mut dyn FnMut(
            RateControlFeedback,
        ) -> Result<Option<RateControlReencode>, String>,
    ) -> Result<EncodedFrame, String> {
        self.load_visible_input_frame(nv12)?;
        let result = self.encode_frame_with_rc_policy(&[], Some(initial), Some(reencode), false);
        self.input_prepared = false;
        result
    }

    pub(crate) fn encode_frame_with_fixed_qp_reencode(
        &mut self,
        nv12: &[u8],
        reencode: &mut dyn FnMut(
            RateControlFeedback,
        ) -> Result<Option<RateControlReencode>, String>,
    ) -> Result<EncodedFrame, String> {
        self.encode_frame_with_rc_policy(nv12, None, Some(reencode), false)
    }

    pub(crate) fn encode_visible_frame_with_fixed_qp_reencode(
        &mut self,
        nv12: &[u8],
        reencode: &mut dyn FnMut(
            RateControlFeedback,
        ) -> Result<Option<RateControlReencode>, String>,
    ) -> Result<EncodedFrame, String> {
        self.load_visible_input_frame(nv12)?;
        let result = self.encode_frame_with_rc_policy(&[], None, Some(reencode), false);
        self.input_prepared = false;
        result
    }

    fn encode_frame_with_rc_policy(
        &mut self,
        nv12: &[u8],
        mut target: Option<ExternalRateControl>,
        mut reencode: Option<
            &mut dyn FnMut(RateControlFeedback) -> Result<Option<RateControlReencode>, String>,
        >,
        include_annex_b: bool,
    ) -> Result<EncodedFrame, String> {
        self.synchronize_service_recovery()?;
        let recovery_restart = self.recovery_pending;
        let force_idr = std::mem::take(&mut self.force_next_idr);
        let gop_idr = self.refresh_length == 0 && self.frames != 0 && self.frames % self.gop == 0;
        let is_idr = self.frames == 0 || force_idr || recovery_restart || gop_idr;
        if force_idr || recovery_restart || gop_idr {
            self.slot = 0;
        }
        if is_idr {
            self.idr_frame = self.frames;
        }
        let recovery_prefix = !is_idr && self.refresh_length != 0 && self.frames % self.gop == 0;
        let (mut picture, prefix) = if is_idr {
            (
                Picture::Idr,
                self.frames == 0 || self.headers_each_idr || recovery_restart,
            )
        } else {
            (
                Picture::P {
                    poc_lsb: self.frames.saturating_sub(self.idr_frame),
                },
                false,
            )
        };
        let saved_osd = self.next_osd.clone();
        let original_fixed_qp = self.qp;
        let previous_header_qp = self.fixed_qp_header_override;
        let mut fixed_qp_override = None;
        let encoded = loop {
            let rc = target.map(|target| RateControl {
                bit_target: target.bit_target,
                qp_target: target.qp_target,
                qp_min: target.qp_min,
                qp_max: target.qp_max,
                qp_delta_row: 2,
                qp_delta_row_i: 2,
            });
            if target.is_none() {
                if let Some(qp) = fixed_qp_override {
                    self.qp = qp;
                }
            }
            self.next_osd = saved_osd.clone();
            self.fixed_qp_header_override = if target.is_none() {
                Some(original_fixed_qp)
            } else {
                previous_header_qp
            };
            let hardware_result =
                self.encode_picture_rc(nv12, picture, prefix, recovery_prefix, rc, include_annex_b);
            self.qp = original_fixed_qp;
            self.fixed_qp_header_override = previous_header_qp;
            let encoded = hardware_result?;
            let next = reencode
                .as_deref_mut()
                .map(|callback| {
                    callback(RateControlFeedback {
                        bit_real: encoded.statistics.bitstream_bytes as i32 * 8,
                        quality_real: encoded.statistics.quality_real(self.width, self.height),
                    })
                })
                .transpose()?
                .flatten();
            if let Some(next) = next {
                validate_recovery_reencode(recovery_restart, &next)?;
                if next.is_intra && !next.is_idr {
                    return Err("VEPU580 H.265 reencode cannot switch to non-IDR I".to_string());
                }
                if next.is_idr {
                    if !matches!(picture, Picture::Idr) {
                        picture = Picture::ForcedIdrFromP {
                            poc_lsb: self.frames.saturating_sub(self.idr_frame),
                        };
                    }
                } else if matches!(picture, Picture::Idr | Picture::ForcedIdrFromP { .. }) {
                    picture = Picture::P {
                        poc_lsb: self.frames.saturating_sub(self.idr_frame),
                    };
                }
                if target.is_some() {
                    target = Some(next.target);
                } else {
                    fixed_qp_override = Some(next.target.qp_target);
                }
                continue;
            }
            break encoded;
        };
        if matches!(picture, Picture::Idr) {
            self.idr_frame = self.frames;
        }
        self.slot = 1 - self.slot;
        self.frames += 1;
        self.recovery_pending = false;
        Ok(encoded)
    }

    /// One IDR and nothing else: the board gate's entry point.
    pub fn encode_idr(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        Ok(self.encode_idr_packets(nv12)?.annex_b)
    }

    pub fn read_motion_info(&mut self) -> Result<Vec<u8>, String> {
        self.md_info
            .sync_read_begin()
            .map_err(|e| format!("VEPU580 H.265 motion info invalidate: {e}"))?;
        let data = self.md_info.as_slice()[..md_info_size(self.width, self.height)].to_vec();
        self.md_info
            .sync_read_end()
            .map_err(|e| format!("VEPU580 H.265 motion info sync end: {e}"))?;
        Ok(data)
    }

    pub fn encode_idr_packets(&mut self, nv12: &[u8]) -> Result<EncodedFrame, String> {
        assert_eq!(self.frames, 0, "encode_idr is the opening picture");
        self.synchronize_service_recovery()?;
        let encoded = self.encode_picture_packets(nv12, Picture::Idr, true)?;
        self.frames = 1;
        self.recovery_pending = false;
        Ok(encoded)
    }

    fn load_visible_input_frame(&mut self, nv12: &[u8]) -> Result<(), String> {
        let coded_width = align(self.width, 16);
        let coded_height = align(self.height, 16);
        let visible_size = self
            .width
            .checked_mul(self.height)
            .and_then(|luma| luma.checked_mul(3).map(|bytes| bytes / 2))
            .ok_or_else(|| "H.265 visible input size overflow".to_string())?;
        if nv12.len() < visible_size {
            return Err(format!(
                "H.265 input is {} bytes, the {}x{} picture needs {visible_size}",
                nv12.len(),
                self.width,
                self.height
            ));
        }

        self.input_prepared = false;
        super::copy_visible_nv12_to_padded(
            self.input.as_mut_slice(),
            &nv12[..visible_size],
            self.width,
            self.height,
            coded_width,
            coded_height,
            !self.input_padding_initialized,
        )?;
        self.input_padding_initialized = true;
        self.input_prepared = true;
        Ok(())
    }

    fn encode_picture_rc(
        &mut self,
        nv12: &[u8],
        picture: Picture,
        with_prefix: bool,
        recovery_prefix: bool,
        rc: Option<RateControl>,
        include_annex_b: bool,
    ) -> Result<EncodedFrame, String> {
        self.active_osd = self.next_osd.take();
        let input_prepared = self.input_prepared;
        let input_fd = if let Some(input_fd) = self.input_fd_override.take() {
            input_fd
        } else if input_prepared {
            self.input.fd()
        } else {
            // dma-heap rounds the allocation up to a page, so the usable picture is
            // the logical size, not `self.input.len()`.
            let input_size = luma_bytes(self.width, self.height) * 3 / 2;
            if nv12.len() < input_size {
                return Err(format!(
                    "H.265 input is {} bytes, the {}x{} surface needs {}",
                    nv12.len(),
                    self.width,
                    self.height,
                    input_size
                ));
            }
            self.input_padding_initialized = false;
            self.input
                .as_mut_slice()
                .get_mut(..input_size)
                .ok_or_else(|| "H.265 input buffer shorter than the picture".to_string())?
                .copy_from_slice(&nv12[..input_size]);
            self.input.fd()
        };
        // The VEPU580 carries dual-core handshake state between processes, and
        // the first tiled picture of a session reads it. The reference is not
        // immune either: the same C probe at 320x240 with `auto_tile` produces
        // one of two byte patterns depending on which encoder used the block
        // last (three runs after our own process: `9724d029..`; three of its
        // own: `b27d68dc..`), and emptying the page allocator between runs does
        // not change that, so the state is inside the block, not in memory.
        // A discarded prepass makes our output independent of that history: the
        // picture that reaches the stream is always written by a job whose
        // predecessor is one of our own.
        if self.tile_widths().len() > 1 && !self.tile_warmed {
            self.tile_warmed = true;
            self.input_fd_override = Some(input_fd);
            self.encode_picture_rc(nv12, picture, false, false, rc, false)?;
        }

        // The GOP's long-term reference is the opening IDR (`used_by_lt_flg0` and
        // the SPS LT entry): every P predicts from slot 0 and reconstructs into
        // slot 1. An IDR reconstructs into and reads from slot 0.
        let refresh_due = self.refresh_length != 0
            && self.frames / self.gop > 0
            && self.frames % self.gop < self.refresh_length;
        let refresh_job = match (self.refresh, self.refresh_buf.as_mut()) {
            (Some((mode, refresh_num)), Some(table_buf)) if refresh_due => {
                let refresh_idx = self.frames % self.gop;
                let table =
                    refresh_base_table(self.width, self.height, refresh_idx, mode, refresh_num);
                table_buf
                    .sync_write_begin()
                    .map_err(|e| format!("refresh table sync: {e}"))?;
                table_buf
                    .as_mut_slice()
                    .get_mut(..table.len())
                    .ok_or_else(|| "refresh table buffer too small".to_string())?
                    .copy_from_slice(&table);
                table_buf
                    .sync_write_end()
                    .map_err(|e| format!("refresh table flush: {e}"))?;
                Some(RefreshJob {
                    table_fd: table_buf.fd(),
                    mode,
                    refresh_num,
                    refresh_idx,
                })
            }
            _ => None,
        };
        let (current, reference) = if matches!(picture, Picture::Idr) {
            (self.slot, self.slot)
        } else if self.long_term {
            // The regular LT GOP always predicts from the opening IDR.
            (1, 0)
        } else {
            // REF_TO_PREV_REF_FRM: alternate the two DPB surfaces.
            (self.slot, 1 - self.slot)
        };
        let current_pixel = self.surfaces[current].pixel.fd();
        let current_dsp = self.surfaces[current].dsp.fd();
        let reference_pixel = self.surfaces[reference].pixel.fd();
        let reference_dsp = self.surfaces[reference].dsp.fd();
        let header = fbc_header_len(self.width, self.height);
        let luma = luma_bytes(self.width, self.height) as u32;
        let tile_widths = self.tile_widths();
        if tile_widths.len() > 1 && self.split.out & MPP_ENC_SPLIT_OUT_LOWDELAY != 0 {
            return Err("H.265 tile + low-delay slice output is not verified".into());
        }
        self.ensure_tile_line_bufs(tile_widths.len())?;
        let mut payload = Vec::new();
        let mut slice_lengths = Vec::new();
        let mut aggregate_status = std::mem::take(&mut self.aggregate_status_scratch);
        aggregate_status.resize(STATUS_WORDS, 0);
        for &word in &AGGREGATE_STATUS_WORDS {
            aggregate_status[word] = 0;
        }
        let mut sse_sum = 0u64;
        let mut stream_len = 0usize;

        for (tile_index, &(tile_start, tile_width)) in tile_widths.iter().enumerate() {
            let tile = (tile_widths.len() > 1).then_some(crate::hal::h265e::TileJob {
                index: tile_index as u32,
                start_x: tile_start,
                width_mb: tile_width,
                height_mb: (self.height + 63) as u32 / 64,
                columns: tile_widths.len() as u32,
            });
            let job = IdrJob {
                width: self.width,
                height: self.height,
                qp: self.qp,
                qp_min: self.qp,
                qp_max: self.qp,
                picture,
                long_term: self.long_term,
                temporal_mvp: self.temporal_mvp,
                force_cu_qp_delta: self.force_cu_qp_delta,
                scene_motion_flag: self.tune.scene_motion_flag(),
                qbias: self.qbias,
                rc,
                refresh: refresh_job,
                split: self.split,
                syntax: self.syntax,
                tile,
                roi_fd: self.roi.as_ref().map_or(-1, |buffers| buffers.0.fd()),
                roi_qp_fd: self.roi.as_ref().map_or(-1, |buffers| buffers.1.fd()),
                input_fd,
                output_fd: self.outputs[0].fd(),
                current_fd: current_pixel,
                current_thumb_fd: current_dsp,
                reference_fd: reference_pixel,
                reference_thumb_fd: reference_dsp,
            };
            let mut registers = build_registers(&job)?;
            if let Some(osd) = self.active_osd.as_ref() {
                osd.apply(&mut registers.osd)?;
                registers.offsets.extend(osd.offsets());
            }

            // `vepu580_h265_set_hw_address`: input/output, FBC bodies, thumbs and
            // motion info are shared by every tile. The tile line buffers alternate
            // through regs 176/177.
            let base = &mut registers.base;
            let input = input_fd as u32;
            let output = self.outputs[0].fd() as u32;
            base[0] = input;
            base[1] = input;
            base[2] = input;
            base[3] = current_pixel as u32;
            base[4] = current_pixel as u32;
            base[5] = reference_pixel as u32;
            base[6] = reference_pixel as u32;
            base[7] = self.surfaces[current].cmv.fd() as u32;
            base[8] = self.surfaces[reference].cmv.fd() as u32;
            base[9] = current_dsp as u32;
            base[10] = reference_dsp as u32;
            base[11] = self.md_info.fd() as u32;
            base[12] = output;
            base[13] = output;
            base[14] = output;
            base[15] = output;
            if tile.is_some() {
                base[16] = self.tile_line_bufs[tile_index].fd() as u32;
                base[17] = self.tile_line_bufs[tile_index.saturating_sub(1)].fd() as u32;
                if tile_index == 0 && self.tile_line_bufs.len() > 1 {
                    base[17] = self.tile_line_bufs[1].fd() as u32;
                }
            }
            registers.set(&regs::BASE_ENC_PIC_MEI_STOR, 1)?;

            // `vepu580_h265_set_hw_address`'s two stream multi-offsets: 175 is
            // where the tile starts writing and 172 is where its buffer ends. The
            // C gives tile 0 the whole output packet (`mpp_buffer_get_size`) and
            // later tiles their own `frame_size / tile_num` slice, so the limit is
            // per-tile, not one shared end minus a spare byte.
            let output_offset = stream_len as u32;
            let output_limit = if tile.is_none() {
                self.outputs[0].len() as u32
            } else if tile_index == 0 {
                self.outputs[0].len() as u32
            } else {
                (stream_len + self.outputs[0].len() / tile_widths.len()).min(self.outputs[0].len())
                    as u32
            };
            registers.offsets.extend([
                RegOffsetInfo {
                    reg_idx: 161,
                    offset: luma,
                },
                RegOffsetInfo {
                    reg_idx: 162,
                    offset: luma,
                },
                RegOffsetInfo {
                    reg_idx: 164,
                    offset: header as u32,
                },
                RegOffsetInfo {
                    reg_idx: 166,
                    offset: header as u32,
                },
                RegOffsetInfo {
                    reg_idx: 175,
                    offset: output_offset,
                },
                RegOffsetInfo {
                    reg_idx: 172,
                    offset: output_limit,
                },
            ]);

            let mut status = std::mem::take(&mut self.status_scratch);
            // The full STATUS range is a SET_REG_READ destination, so every word
            // is overwritten by the kernel. Resize only restores a lost scratch.
            status.resize(STATUS_WORDS, 0);
            let mut hw_status = [0u32; 1];
            let mut batch = std::mem::take(&mut self.batch_scratch);
            batch.clear();
            batch.reg_write(&registers.ctl, CTL_OFFSET);
            batch.reg_write(&registers.base, BASE_OFFSET);
            batch.reg_write(&registers.rc_klut, RCKLUT_OFFSET);
            batch.reg_write(&registers.wgt, WGT_OFFSET);
            batch.reg_write(&registers.rdo, RDO_OFFSET);
            batch.reg_write(&registers.osd, OSD_OFFSET);
            batch.reg_offsets(&registers.offsets);
            batch.reg_read(&mut hw_status, HW_STATUS_OFFSET);
            batch.reg_read(&mut status, STATUS_OFFSET);
            let submitted = self.service.submit_batch_recovering(&mut batch);
            self.batch_scratch = batch;
            submitted.map_err(|e| format!("H.265 register submit: {e}"))?;
            self.poll_picture(tile.is_some(), &mut slice_lengths)?;

            let errors = hw_status[0] & HW_ERROR_MASK;
            if errors != 0 {
                let _ = self.service.restart_after_hardware_error();
                return Err(format!(
                    "VEPU580 H.265 hardware error mask 0x{errors:03x} (int_sta=0x{:08x})",
                    hw_status[0]
                ));
            }
            if hw_status[0] & 1 == 0 {
                let _ = self.service.restart_after_hardware_error();
                return Err(format!(
                    "VEPU580 H.265 completed ioctl without enc_done (int_sta=0x{:08x})",
                    hw_status[0]
                ));
            }

            let length = status[0] as usize;
            if length == 0 || length > self.outputs[0].len().saturating_sub(stream_len) {
                let _ = self.service.restart_after_hardware_error();
                return Err(format!(
                    "invalid VEPU580 H.265 length {length} at offset {stream_len} (capacity {})",
                    self.outputs[0].len()
                ));
            }
            sse_sum += (u64::from(status[2]) << 16) | u64::from((status[1] >> 16) & 0xffff);
            for &word in &AGGREGATE_STATUS_WORDS {
                aggregate_status[word] = aggregate_status[word].wrapping_add(status[word]);
            }
            stream_len += length;
            payload.resize(stream_len, 0);
            payload[stream_len - length..stream_len]
                .copy_from_slice(&self.outputs[0].as_slice()[stream_len - length..stream_len]);
            self.status_scratch = status;
        }
        let mut status = aggregate_status;
        status[1] = (sse_sum & 0xffff) as u32;
        status[2] = (sse_sum >> 16) as u32;
        // STATUS measurement, `vepu580_h265_set_feedback`: madp/num_ctu, madi/num_b16
        // and quality over the 8x8 block count (`mb64_num << 6`), not the 16x16
        // macroblock count the H.264 path divides by.
        self.last_measured = Some(measured_from_status(self.width, self.height, &status));
        self.aggregate_status_scratch = status;
        let statistics = Statistics::from_words(&self.aggregate_status_scratch);
        self.update_tune(&statistics);

        // The hardware writes the slice NAL; the parameter sets and the two SEIs
        // are software output, exactly as `mpp_enc_impl.cpp` prepends them ahead
        // of the opening IDR or every IDR in per-IDR header mode.
        let mut config = crate::codec::h265::encoder::EncoderConfig::new(self.width, self.height);
        config.level = self.level;
        config.tmvp_enable = self.temporal_mvp;
        config.video_format = self.video_format;
        config.video_full_range = self.video_full_range;
        config.long_term_refs = self.long_term;
        config.apply_syntax_options(self.syntax);
        config.force_cu_qp_delta = self.force_cu_qp_delta;
        if self.rc_sei.is_some() {
            config.rc_mode = match self.rc_mode {
                RcMode::Cbr => crate::codec::h265::encoder::RateControl::Cbr,
                RcMode::Vbr | RcMode::Avbr => crate::codec::h265::encoder::RateControl::Vbr,
            };
        }
        let headers = if recovery_prefix {
            let mut out = crate::codec::h265::encoder::build_headers(&config)
                .map_err(|e| format!("H.265 recovery headers: {e:?}"))?;
            out.extend(crate::codec::h265::sei::recovery_point(
                self.refresh_length as i32 - 1,
            ));
            out
        } else if !with_prefix {
            Vec::new()
        } else if !self.sei_enabled {
            crate::codec::h265::encoder::build_headers(&config)
                .map_err(|e| format!("H.265 headers: {e:?}"))?
        } else if let Some(sei) = self.rc_sei.as_ref() {
            let line = crate::codec::h265::sei::RcCfgLine {
                model_name: &self.rc_model_name,
                mode_name: self.rc_mode.name(),
                bps_target: sei.target,
                bps_min: sei.min,
                bps_max: sei.max,
                igop: self.gop as i64,
                init_quality: sei.init_quality,
                min_quality: sei.qp_min as i32,
                max_quality: sei.qp_max as i32,
                min_i_quality: sei.qp_min_i as i32,
                max_i_quality: sei.qp_max_i as i32,
                i_quality_delta: sei.i_quality_delta,
            };
            crate::codec::h265::encoder::build_prefix_line(&config, &line)
                .map_err(|e| format!("H.265 prefix: {e:?}"))?
        } else {
            let line = crate::codec::h265::sei::RcCfgLine {
                model_name: &self.rc_model_name,
                ..crate::codec::h265::sei::RcCfgLine::fixed_qp(
                    self.fixed_qp_header_override.unwrap_or(self.qp),
                    self.gop,
                    crate::codec::h265::sei::default_bps_target(config.width, config.height, 30),
                )
            };
            crate::codec::h265::encoder::build_prefix_line(&config, &line)
                .map_err(|e| format!("H.265 prefix: {e:?}"))?
        };
        let packets = output_packets(&headers, &payload, &slice_lengths)?;
        let annex_b = if include_annex_b {
            let mut annex_b = Vec::with_capacity(headers.len() + payload.len());
            annex_b.extend_from_slice(&headers);
            annex_b.extend_from_slice(&payload);
            annex_b
        } else {
            Vec::new()
        };
        self.active_osd = None;
        Ok(EncodedFrame {
            annex_b,
            packets,
            payload,
            headers,
            is_idr: matches!(picture, Picture::Idr | Picture::ForcedIdrFromP { .. }),
            frame_num: self.frames,
            quality_target: rc.map_or(self.qp as i32, |rc| rc.qp_target as i32),
            rt_bits: 0,
            statistics,
        })
    }
}

/// Lay out a completed picture like `mpp_enc_impl.c`: the software prefix rides
/// with the first slice, every hardware slice becomes one packet, and a trailing
/// zero-length FIFO entry remains an empty EOI packet.
fn output_packets(
    headers: &[u8],
    payload: &[u8],
    slice_lengths: &[usize],
) -> Result<Vec<Vec<u8>>, String> {
    if slice_lengths.is_empty() {
        let mut packet = Vec::with_capacity(headers.len() + payload.len());
        packet.extend_from_slice(headers);
        packet.extend_from_slice(payload);
        return Ok(vec![packet]);
    }

    let total = slice_lengths
        .iter()
        .try_fold(0usize, |total, length| total.checked_add(*length));
    match total {
        Some(total) if total == payload.len() => {}
        Some(total) => {
            return Err(format!(
                "H.265 slice lengths sum to {total}, STATUS says {}",
                payload.len()
            ))
        }
        None => return Err("H.265 slice length sum overflowed usize".into()),
    }

    let mut packets = Vec::with_capacity(slice_lengths.len());
    let mut offset = 0;
    for (index, length) in slice_lengths.iter().copied().enumerate() {
        let mut packet = if index == 0 {
            headers.to_vec()
        } else {
            Vec::new()
        };
        packet.extend_from_slice(&payload[offset..offset + length]);
        packets.push(packet);
        offset += length;
    }
    Ok(packets)
}

/// `vepu580_h265_set_feedback`'s STATUS translation. The divisor for `quality_real`
/// is the 8x8 block count (`mb64_num << 6`), not the 16x16 macroblock count the
/// H.264 HAL uses; `bits` counts only the hardware slice, not the software prefix.
fn measured_from_status(width: usize, height: usize, status: &[u32]) -> rc_model::Measured {
    let statistics = Statistics::from_words(status);
    let madi = {
        if statistics.num_b16 > 0 {
            (statistics.madi_sum / statistics.num_b16) as i32
        } else {
            0
        }
    };
    let madp = {
        if statistics.num_ctu > 0 {
            (statistics.madp_sum / statistics.num_ctu) as i32
        } else {
            0
        }
    };
    rc_model::Measured {
        bits: statistics.bitstream_bytes as i32 * 8,
        quality: statistics.quality_real(width, height),
        madi,
        madp,
        iblk4_prop: 0,
    }
}

fn align(value: usize, to: usize) -> usize {
    (value + to - 1) & !(to - 1)
}

/// `MPP_ALIGN(width, 16) * MPP_ALIGN(height, 16)`: the luma plane of the input,
/// which is also the u and v offset of a 4:2:0 semi-planar frame.
fn luma_bytes(width: usize, height: usize) -> usize {
    align(width, 16) * align(height, 16)
}

/// `vepu580_h265_get_buffer`'s three sizes. The block count adds a row
/// (`mb_h64 = (h + 63) / 64 + 1`), which is what makes the 320x240 header 8192:
/// 5x5 blocks of 64 bytes rounded up to 8 KiB.
fn surface_sizes(width: usize, height: usize) -> (usize, usize, usize) {
    let blocks = ((width + 63) / 64) * ((height + 63) / 64 + 1);
    let header = fbc_header_len(width, height);
    let body = (blocks << 12) * 3 / 2;
    (header + body, blocks << 8, align(blocks * 16 * 6, 256))
}

fn fbc_header_len(width: usize, height: usize) -> usize {
    let blocks = ((width + 63) / 64) * ((height + 63) / 64 + 1);
    align(blocks << 6, 8192)
}

/// `p->mdinfo_size` for HEVC: one 16-byte record per 32x32 block.
fn md_info_size(width: usize, height: usize) -> usize {
    (align(width, 32) >> 5) * (align(height, 32) >> 5) * 16
}

/// `num_tile_columns_minus1 + 1` uniform columns: `(first CTU column, width in
/// CTUs)` for each, the same split `hal_h265e_v580_set_uniform_tile` computes.
pub fn uniform_tile_widths(width: usize, columns: usize) -> Vec<(u32, u32)> {
    let mb_w = (width + 63) as u32 / 64;
    (0..columns)
        .map(|index| {
            let start = (index as u32 * mb_w) / columns as u32;
            let end = ((index as u32 + 1) * mb_w) / columns as u32;
            (start, end - start)
        })
        .collect()
}

fn validate_recovery_reencode(
    recovery_restart: bool,
    next: &RateControlReencode,
) -> Result<(), String> {
    if recovery_restart && !next.is_idr {
        Err("VEPU580 H.265 recovery IDR cannot be reencoded as a non-IDR picture".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_status_resets_only_feedback_words() {
        assert_eq!(AGGREGATE_STATUS_WORDS.len(), 23);
        assert_eq!(AGGREGATE_STATUS_WORDS[..2], [0, 3]);
        assert_eq!(AGGREGATE_STATUS_WORDS[2..], (64..=84).collect::<Vec<_>>());
    }

    #[test]
    fn the_captured_geometry_gets_the_caps_sizes() {
        // 320x240: 5x5 blocks of 64 -> header 8192, body 153600, pixel 161792,
        // display thumb 6400, mv thumb 2560 (C `vepu580_h265_get_buffer`).
        assert_eq!(fbc_header_len(320, 240), 8192);
        assert_eq!(surface_sizes(320, 240), (161792, 6400, 2560));
        assert_eq!(luma_bytes(320, 240), 76800);
        assert_eq!(md_info_size(320, 240), 1280);
    }

    #[test]
    fn the_status_translation_uses_the_h265_divisors() {
        let mut status = vec![0u32; 180];
        let mut put = |word: usize, value: u32| status[word - 4096] = value;
        put(4096, 847); // slice bytes
        put(4097, 0x2222_1111); // sse low + ignored reserve
        put(4098, 0x3333_4444); // sse high
        put(4099, 19 * 1280); // qp_sum over 5x4 CTUs -> 1280 8x8 blocks
        put(4160, 1);
        put(4161, 2);
        put(4162, 3);
        put(4163, 4);
        put(4164, 5);
        put(4165, 6);
        put(4166, 7);
        put(4167, 8);
        put(4168, 7 * 3); // madp sum, num_ctu
        put(4169, 3);
        put(4170, 2 * 10); // madi sum, num_b16
        put(4171, 10);
        let statistics = Statistics::from_words(&status);
        assert_eq!(statistics.sse, (0x3333_4444_u64 << 16) | 0x2222);
        assert_eq!(
            [
                statistics.pnum_p64,
                statistics.pnum_p32,
                statistics.pnum_p16,
                statistics.pnum_p8,
                statistics.pnum_i32,
                statistics.pnum_i16,
                statistics.pnum_i8,
                statistics.pnum_i4,
            ],
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
        let measured = measured_from_status(320, 240, &status);
        assert_eq!(measured.bits, 847 * 8);
        assert_eq!(measured.quality, 19);
        assert_eq!(measured.madi, 2);
        assert_eq!(measured.madp, 7);
    }

    #[test]
    fn odd_sizes_round_the_way_the_hal_rounds() {
        assert_eq!(luma_bytes(322, 242), 336 * 256);
        assert_eq!(md_info_size(322, 242), (352 >> 5) * (256 >> 5) * 16);
        // 11x13 blocks = 143; 143*64 = 9152 -> 16384.
        assert_eq!(fbc_header_len(640, 736), 16384);
    }

    #[test]
    fn harness_rate_control_reuses_the_caller_i_band_with_delta_two() {
        let sei = RcSei::from_harness(300_000, 250_000, 350_000, 26, 10, 51);
        assert_eq!(sei.init_quality, 26);
        assert_eq!(sei.qp_min_i, 10);
        assert_eq!(sei.qp_max_i, 51);
        assert_eq!(sei.i_quality_delta, 2);
    }

    #[test]
    fn uniform_tiles_follow_the_c_integer_partition() {
        assert_eq!(uniform_tile_widths(320, 2), vec![(0, 2), (2, 3)]);
        assert_eq!(
            uniform_tile_widths(8256, 4),
            vec![(0, 32), (32, 32), (64, 32), (96, 33)]
        );
    }

    #[test]
    fn frame_parallel_handshake_matches_the_c_trace() {
        let mut registers = RegisterFile::default();
        let mut curr = 0;
        let mut prev = 0;
        let mut words = Vec::new();
        for intra in [true, false, false, false, false] {
            apply_frame_parallel_handshake(&mut registers, intra, &mut curr, &mut prev).unwrap();
            words.push(registers.base[33]);
        }
        assert_eq!(
            words,
            [
                0x0009_0010,
                0x0009_0031,
                0x0009_0036,
                0x0009_003b,
                0x0009_003c
            ]
        );

        apply_frame_parallel_handshake(&mut registers, true, &mut curr, &mut prev).unwrap();
        assert_eq!(registers.base[33], 0x0009_0010);
    }

    #[test]
    fn low_delay_packets_keep_headers_with_the_first_slice() {
        let packets =
            output_packets(&[0xaa, 0xbb], &[1, 2, 3, 4, 5, 6, 7, 8, 9], &[4, 5, 0]).unwrap();
        assert_eq!(
            packets,
            vec![
                vec![0xaa, 0xbb, 1, 2, 3, 4],
                vec![5, 6, 7, 8, 9],
                Vec::new()
            ]
        );
    }

    #[test]
    fn slice_length_sum_must_match_status() {
        assert!(output_packets(&[], &[0; 9], &[4, 4]).is_err());
    }

    #[test]
    fn recovery_idr_cannot_be_downgraded_by_the_reencode_callback() {
        let next = RateControlReencode {
            target: ExternalRateControl {
                bit_target: 1,
                qp_target: 26,
                qp_min: 8,
                qp_max: 48,
            },
            is_intra: false,
            is_idr: false,
        };
        assert!(validate_recovery_reencode(true, &next).is_err());
        assert!(validate_recovery_reencode(false, &next).is_ok());

        let idr = RateControlReencode {
            is_idr: true,
            ..next
        };
        assert!(validate_recovery_reencode(true, &idr).is_ok());
    }
}
