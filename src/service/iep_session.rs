//! IEP2 deinterlace session: client 28 buffers plus the submit-and-poll
//! sequence, beside the JPEG path with it untouched.
//!
//! The ioctl choreography is exactly `iep2_start()` + `iep2_wait()` from
//! `mpp/vproc/iep2/iep2.c`: one `MPP_FLAGS_MULTI_MSG` batch carrying the whole
//! `struct iep2_params` as `SET_REG_WRITE` and the `struct iep2_output` as the
//! paired `SET_REG_READ`, then a separate `POLL_HW_FINISH`. The parameter and
//! output layouts come from [`crate::hal::iep`]; this file only owns dma-bufs
//! and drives the already-written [`MppService`] client. The register derivation
//! is host-testable on its own; the code here needs `/dev/mpp_service`,
//! `/dev/dma_heap/*` and the IEP2 driver, so it is exercised on-board by
//! `scripts/board_iep_dei_vs_mpp.sh`, never in a host unit test.

use std::io;

use super::dma_heap::DmaBuf;
use super::{Batch, Client, MppService};
use crate::hal::iep::{
    self, DeinterlaceJob, Iep2DilMode, Iep2FieldOrder, Iep2Fmt, Iep2OutMode, Iep2YuvSwap,
};
use crate::hal::iep_pd::{self, FieldOrderState, Iep2DectStats, PulldownState};

/// The `IEP_CLIENT_TYPE` the C library announces on `/dev/mpp_service`.
pub const IEP_CLIENT: u32 = Client::Iep as u32; // 28

/// Format, geometry and deinterlace mode for a session. Mirrors the values the
/// C library accumulates through `IEP2_PARAM_TYPE_COM` and `IEP2_PARAM_TYPE_MODE`.
#[derive(Debug, Clone, Copy)]
pub struct DeinterlaceConfig {
    pub width: usize,
    pub height: usize,
    pub hor_stride: usize,
    pub src_fmt: Iep2Fmt,
    pub src_swap: Iep2YuvSwap,
    pub dst_fmt: Iep2Fmt,
    pub dst_swap: Iep2YuvSwap,
    pub dil_mode: Iep2DilMode,
    pub out_mode: Iep2OutMode,
    pub field_order: Iep2FieldOrder,
    /// Motion-detection lambda: the C library uses 4 for every SoC except
    /// RK3528, where an env switch picks 4 or 8. On this board it is 4.
    pub md_lambda: u32,
}

impl Default for DeinterlaceConfig {
    fn default() -> Self {
        Self {
            width: 720,
            height: 480,
            hor_stride: 720,
            src_fmt: Iep2Fmt::Yuv420,
            src_swap: Iep2YuvSwap::SpUv,
            dst_fmt: Iep2Fmt::Yuv420,
            dst_swap: Iep2YuvSwap::SpUv,
            dil_mode: Iep2DilMode::I5O2,
            out_mode: Iep2OutMode::Line,
            field_order: Iep2FieldOrder::Tff,
            md_lambda: 4,
        }
    }
}

/// What client 28 reports about itself, both questions the block table asks:
/// the kernel's presence bit and whether a driver session actually bound.
#[derive(Debug)]
pub struct ProbeReport {
    /// Bit 28 of `MPP_CMD_PROBE_HW_SUPPORT`.
    pub present: bool,
    /// `MPP_CMD_INIT_CLIENT_TYPE` for 28 succeeded.
    pub binds: bool,
    /// `MPP_CMD_SET_REG_ADDR_OFFSET` is implemented.
    pub reg_offset: bool,
    /// `MPP_CMD_POLL_HW_FINISH` is implemented.
    pub poll: bool,
    pub hw_id: Option<u32>,
    /// errno from a failed open, when `binds` is false.
    pub open_errno: Option<i32>,
}

/// Open client 28 and report its probe answers without driving any job.
pub fn probe() -> ProbeReport {
    let mut report = ProbeReport {
        present: false,
        binds: false,
        reg_offset: false,
        poll: false,
        hw_id: None,
        open_errno: None,
    };
    match MppService::open(IEP_CLIENT) {
        Ok(svc) => {
            report.binds = true;
            if let Ok(mask) = svc.probe_hw_support() {
                report.present = (mask >> IEP_CLIENT) & 1 == 1;
            }
            report.reg_offset = svc.cmd_supported(super::MPP_CMD_SET_REG_ADDR_OFFSET);
            report.poll = svc.cmd_supported(super::MPP_CMD_POLL_HW_FINISH);
            report.hw_id = svc.hw_id().ok();
        }
        Err(e) => report.open_errno = e.raw_os_error(),
    }
    report
}

/// A bound IEP2 session with its three source frames, two output frames, and
/// the mv/md scratch buffers the C library allocates unconditionally.
pub struct IepDeinterlace {
    svc: MppService,
    cfg: DeinterlaceConfig,
    src: [DmaBuf; 3],
    dst: [DmaBuf; 2],
    mv: DmaBuf,
    md: DmaBuf,
    /// Effective mode: starts at the configured one and switches to PD for a
    /// frame the pulldown machine confirmed, then back to DECT.
    mode: Iep2DilMode,
    /// Effective field order, which `iep2_check_ffo` may override.
    field_order: Iep2FieldOrder,
    field_order_state: FieldOrderState,
    pulldown: PulldownState,
    /// `pd_mode` written into the next parameter block; never cleared, like
    /// `iep2.c`, so an unconfirmed frame keeps the last confirmed flag.
    pd_mode: u32,
    /// `pd_mode` handed to the hardware for this frame, which is what the
    /// reference reports as `pd_flag` (it is captured before `iep2_done`).
    submitted_pd: u32,
}

impl IepDeinterlace {
    /// Announce client 28 and reserve every buffer the deinterlace path needs.
    pub fn open(cfg: DeinterlaceConfig) -> io::Result<Self> {
        let svc = MppService::open(IEP_CLIENT)?;
        let frame = cfg.src_fmt.frame_bytes(cfg.width, cfg.height);
        let dst_frame = cfg.dst_fmt.frame_bytes(cfg.width, cfg.height);
        let heap = std::env::var("IEP_HEAP").unwrap_or_else(|_| "system".to_string());
        let alloc = |len: usize| DmaBuf::alloc(&heap, len, false);
        Ok(Self {
            svc,
            src: [alloc(frame)?, alloc(frame)?, alloc(frame)?],
            dst: [alloc(dst_frame)?, alloc(dst_frame)?],
            mv: alloc(iep::MV_BUF_BYTES)?,
            md: alloc(iep::MD_BUF_BYTES)?,
            mode: cfg.dil_mode,
            field_order: cfg.field_order,
            field_order_state: FieldOrderState::default(),
            pulldown: PulldownState::default(),
            pd_mode: 0,
            submitted_pd: 0,
            cfg,
        })
    }

    /// Bytes in one source (or, for 4:2:0 out, one destination) frame.
    pub fn src_frame_bytes(&self) -> usize {
        self.cfg
            .src_fmt
            .frame_bytes(self.cfg.width, self.cfg.height)
    }

    pub fn dst_frame_bytes(&self) -> usize {
        self.cfg
            .dst_fmt
            .frame_bytes(self.cfg.width, self.cfg.height)
    }

    /// Mutable view of source frame `i` ([current, next, previous]) for the
    /// caller to fill. The caller must flush it with `flush_src(i)` before `run`.
    pub fn src_frame_mut(&mut self, i: usize) -> &mut DmaBuf {
        &mut self.src[i]
    }

    /// Read a deinterlaced output frame after `run`.
    pub fn dst_frame(&self, i: usize) -> &[u8] {
        self.dst[i].as_slice()
    }

    /// Finish CPU writes to source frame `i` so the engine can read it.
    pub fn flush_src(&mut self, i: usize) -> io::Result<()> {
        self.src[i].sync_write_end()
    }

    /// Effective field order after `iep2_check_ffo` has had its say.
    pub fn field_order(&self) -> Iep2FieldOrder {
        self.field_order
    }

    /// Pulldown state, i.e. the reference's `iep2_api_info.pd_types`.
    pub fn pulldown(&self) -> &PulldownState {
        &self.pulldown
    }

    /// `pd_mode` of the last submitted picture (`iep2_api_info.pd_flag`).
    pub fn pd_mode(&self) -> u32 {
        self.submitted_pd
    }

    /// `(frm_mode, dil_order_confidence_ratio)` of `struct iep2_api_info`.
    pub fn frame_mode(&self) -> (i32, u32) {
        (
            self.field_order_state.is_frm,
            self.field_order_state.fo_ratio_avg,
        )
    }

    fn apply_stats(&mut self, stats: &Iep2DectStats) {
        if let Some(order) = iep_pd::check_field_order(stats, &mut self.field_order_state) {
            self.field_order = order;
        }
        iep_pd::check_pulldown(stats, &mut self.pulldown);
    }

    /// Clear both destinations and make the sources visible to the engine, once
    /// per picture - `iep2_test` memsets before `RUN_SYNC`, not before every job.
    fn prepare(&mut self) -> Result<(), String> {
        let dst_len = self.dst_frame_bytes();
        for dst in self.dst.iter_mut() {
            dst.sync_write_begin()
                .map_err(|e| format!("dst clear sync: {e}"))?;
            let len = dst_len.min(dst.len());
            dst.as_mut_slice()[..len].fill(0);
            dst.sync_write_end()
                .map_err(|e| format!("dst clear flush: {e}"))?;
        }

        for i in 0..3 {
            self.src[i]
                .sync_read_begin()
                .map_err(|e| format!("src{i} invalidate-before-DMA: {e}"))?;
            self.src[i]
                .sync_read_end()
                .map_err(|e| format!("src{i} sync end: {e}"))?;
        }
        Ok(())
    }

    /// One `iep2_start()` + `iep2_wait()` with the given mode.
    fn submit(&mut self, mode: Iep2DilMode) -> Result<[u32; iep::OUTPUT_WORDS], String> {
        let c = self.cfg;
        let job = DeinterlaceJob {
            width: c.width,
            height: c.height,
            hor_stride: c.hor_stride,
            src_fmt: c.src_fmt,
            src_swap: c.src_swap,
            dst_fmt: c.dst_fmt,
            dst_swap: c.dst_swap,
            dil_mode: mode,
            out_mode: c.out_mode,
            field_order: self.field_order,
            src_fds: [self.src[0].fd(), self.src[1].fd(), self.src[2].fd()],
            dst_fds: [self.dst[0].fd(), self.dst[1].fd()],
            mv_fd: self.mv.fd(),
            md_fd: self.md.fd(),
            md_lambda: c.md_lambda,
            pd_mode: self.pd_mode,
            _marker: std::marker::PhantomData,
        };
        self.submitted_pd = self.pd_mode;
        let params = iep::build_deinterlace(&job);

        let mut output = [0u32; iep::OUTPUT_WORDS];
        let mut batch = Batch::new();
        batch.reg_write(&params, 0);
        batch.reg_read(&mut output, 0);
        self.svc
            .submit_batch_plain(&mut batch)
            .map_err(|e| format!("IEP submit: {e}"))?;
        self.svc
            .poll_hw_finish()
            .map_err(|e| format!("IEP poll: {e}"))?;

        for d in self.dst.iter_mut() {
            d.sync_read_begin()
                .map_err(|e| format!("dst invalidate: {e}"))?;
        }
        Ok(output)
    }

    /// Program the engine and run one picture, following `IEP_CMD_RUN_SYNC` +
    /// `iep2_done` from `iep2.c`:
    ///
    /// 1. run the picture with the mode the previous frame's `done` chose;
    /// 2. a `PD` picture is followed by a second `DECT` job that only collects
    ///    counters (`dil_mode = DECT`, same `pd_mode`);
    /// 3. `iep2_done` folds the last job's counters into the field-order and
    ///    pulldown machines and, once a pulldown type is confirmed, leaves the
    ///    session in `PD` with the flag `iep2_pd_get_output` produced.
    ///
    /// The returned counters belong to the last job.
    pub fn run(&mut self) -> Result<[u32; iep::OUTPUT_WORDS], String> {
        let mode = self.mode;
        self.prepare()?;
        let first = self.submit(mode)?;
        let output = if mode == Iep2DilMode::Pd {
            self.mode = Iep2DilMode::Dect;
            self.submit(Iep2DilMode::Dect)?
        } else {
            first
        };

        // `iep2_done`: the I5O2 family and the DECT/PD pair all update the
        // machines here, then a confirmed pulldown moves the session to PD.
        if matches!(
            self.mode,
            Iep2DilMode::I5O2
                | Iep2DilMode::I5O1T
                | Iep2DilMode::I5O1B
                | Iep2DilMode::Dect
                | Iep2DilMode::Pd
        ) {
            let stats = Iep2DectStats::from_words(&output);
            self.apply_stats(&stats);
            if self.pulldown.pdtype != iep_pd::PD_TYPES_UNKNOWN {
                self.pd_mode = iep_pd::pulldown_mode(&self.pulldown).unwrap_or(-1) as u32;
                self.mode = Iep2DilMode::Pd;
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iep_client_is_28() {
        assert_eq!(IEP_CLIENT, 28);
        assert_eq!(IEP_CLIENT, Client::Iep as u32);
    }

    #[test]
    fn config_defaults_are_the_iep2_test_picture() {
        let c = DeinterlaceConfig::default();
        assert_eq!((c.width, c.height, c.hor_stride), (720, 480, 720));
        assert_eq!(c.dil_mode, Iep2DilMode::I5O2);
        assert_eq!(c.src_fmt.frame_bytes(720, 480), 720 * 480 * 3 / 2);
    }
}
