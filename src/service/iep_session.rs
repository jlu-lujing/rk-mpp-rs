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
}

impl IepDeinterlace {
    /// Announce client 28 and reserve every buffer the deinterlace path needs.
    pub fn open(cfg: DeinterlaceConfig) -> io::Result<Self> {
        let svc = MppService::open(IEP_CLIENT)?;
        let frame = cfg.src_fmt.frame_bytes(cfg.width, cfg.height);
        let dst_frame = cfg.dst_fmt.frame_bytes(cfg.width, cfg.height);
        let alloc = |len: usize| DmaBuf::alloc("system", len, false);
        Ok(Self {
            svc,
            src: [alloc(frame)?, alloc(frame)?, alloc(frame)?],
            dst: [alloc(dst_frame)?, alloc(dst_frame)?],
            mv: alloc(iep::MV_BUF_BYTES)?,
            md: alloc(iep::MD_BUF_BYTES)?,
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

    /// Program the engine, run one deinterlace, and return the hardware's
    /// `struct iep2_output` as raw words (indices from [`crate::hal::iep`]).
    ///
    /// On return the output dma-bufs are already invalidated for CPU reads.
    pub fn run(&mut self) -> Result<[u32; iep::OUTPUT_WORDS], String> {
        for i in 0..3 {
            self.src[i]
                .sync_read_begin()
                .map_err(|e| format!("src{i} invalidate-before-DMA: {e}"))?;
            self.src[i]
                .sync_read_end()
                .map_err(|e| format!("src{i} sync end: {e}"))?;
        }

        let c = self.cfg;
        let job = DeinterlaceJob {
            width: c.width,
            height: c.height,
            hor_stride: c.hor_stride,
            src_fmt: c.src_fmt,
            src_swap: c.src_swap,
            dst_fmt: c.dst_fmt,
            dst_swap: c.dst_swap,
            dil_mode: c.dil_mode,
            out_mode: c.out_mode,
            field_order: c.field_order,
            src_fds: [self.src[0].fd(), self.src[1].fd(), self.src[2].fd()],
            dst_fds: [self.dst[0].fd(), self.dst[1].fd()],
            mv_fd: self.mv.fd(),
            md_fd: self.md.fd(),
            md_lambda: c.md_lambda,
            _marker: std::marker::PhantomData,
        };
        let params = iep::build_deinterlace(&job);

        let mut output = [0u32; iep::OUTPUT_WORDS];
        let mut batch = Batch::new();
        batch.reg_write(&params, 0);
        batch.reg_read(&mut output, 0);
        self.svc
            .submit_batch(&mut batch)
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
