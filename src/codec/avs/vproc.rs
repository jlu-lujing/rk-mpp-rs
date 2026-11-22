//! Stateful IEP2 post-processing matching C MPP's AVS decoder vproc thread.
//!
//! The first frame runs I1O1T with the same source repeated. The next frame
//! only fills the second anchor. From then on each three-frame window runs
//! I5O2 and emits its two destination slots in field-order order.

use std::os::fd::{AsFd, OwnedFd};

use crate::hal::iep::{Iep2DilMode, Iep2FieldOrder, Iep2Fmt, Iep2OutMode, Iep2YuvSwap};
use crate::service::dma_heap::DmaBuf;
use crate::service::iep_session::{DeinterlaceConfig, IepDeinterlace};

pub struct Vproc {
    iep: IepDeinterlace,
    frame_bytes: usize,
    prev0: Option<OwnedFd>,
    prev1: Option<OwnedFd>,
}

impl Vproc {
    pub fn new(
        width: usize,
        height: usize,
        hor_stride: usize,
        ver_stride: usize,
    ) -> Result<Self, String> {
        let cfg = DeinterlaceConfig {
            width,
            height,
            hor_stride,
            ver_stride,
            src_fmt: Iep2Fmt::Yuv420,
            src_swap: Iep2YuvSwap::SpUv,
            dst_fmt: Iep2Fmt::Yuv420,
            dst_swap: Iep2YuvSwap::SpUv,
            dil_mode: Iep2DilMode::I1O1T,
            out_mode: Iep2OutMode::Line,
            field_order: Iep2FieldOrder::Tff,
            md_lambda: 4,
        };
        let iep = IepDeinterlace::open(cfg).map_err(|error| error.to_string())?;
        Ok(Self {
            iep,
            frame_bytes: width * height * 3 / 2,
            prev0: None,
            prev1: None,
        })
    }

    pub fn push(&mut self, frame: &DmaBuf, top_field_first: bool) -> Result<Vec<Vec<u8>>, String> {
        let mut frames = Vec::new();
        self.push_with(frame, top_field_first, |bytes| {
            frames.push(bytes.to_vec());
            Ok(())
        })?;
        Ok(frames)
    }

    /// Run the state machine and visit each compact output frame.
    ///
    /// Packed destinations are passed directly from their dma-buf mapping.
    /// The retained anchors keep only duplicated descriptors, not additional
    /// CPU mappings.
    pub fn push_with(
        &mut self,
        frame: &DmaBuf,
        top_field_first: bool,
        mut visit: impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<usize, String> {
        if frame.len() < self.frame_bytes {
            return Err(format!(
                "AVS vproc frame is {} bytes, needs {}",
                frame.len(),
                self.frame_bytes
            ));
        }

        if self.prev0.is_none() {
            let current = retain(frame)?;
            self.iep.set_mode(Iep2DilMode::I1O1T);
            self.iep.set_field_order(if top_field_first {
                Iep2FieldOrder::Tff
            } else {
                Iep2FieldOrder::Bff
            });
            self.iep.run_imported_fds([frame.as_fd(); 3])?;
            self.prev0 = Some(current);
            self.iep.visit_dst_frame(0, visit)?;
            return Ok(1);
        }

        if self.prev1.is_none() {
            let current = retain(frame)?;
            self.prev1 = self.prev0.take();
            self.prev0 = Some(current);
            return Ok(0);
        }

        let current = retain(frame)?;
        let prev0 = self.prev0.take().expect("prev0 checked above");
        let prev1 = self.prev1.take().expect("prev1 checked above");
        self.iep.set_mode(Iep2DilMode::I5O2);
        self.iep.set_field_order(if top_field_first {
            Iep2FieldOrder::Tff
        } else {
            Iep2FieldOrder::Bff
        });
        self.iep
            .run_imported_fds([prev0.as_fd(), frame.as_fd(), prev1.as_fd()])?;

        let order = self.iep.field_order();
        if std::env::var_os("RFI_AVS_VPROC_TRACE").is_some() {
            let (frm_mode, ratio) = self.iep.frame_mode();
            eprintln!(
                "vproc order={:?} pd={} frm_mode={} ratio={}",
                order,
                self.iep.pd_mode(),
                frm_mode,
                ratio
            );
        }
        let first = if order == Iep2FieldOrder::Bff { 1 } else { 0 };
        let second = 1 - first;
        self.iep.visit_dst_frame(first, &mut visit)?;
        self.iep.visit_dst_frame(second, visit)?;
        self.prev1 = Some(prev0);
        self.prev0 = Some(current);
        Ok(2)
    }
}

fn retain(frame: &DmaBuf) -> Result<OwnedFd, String> {
    frame
        .dup_fd()
        .map_err(|error| format!("retain AVS vproc frame: {error}"))
}
