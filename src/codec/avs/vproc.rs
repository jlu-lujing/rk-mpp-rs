//! Stateful IEP2 post-processing matching C MPP's AVS decoder vproc thread.
//!
//! The first frame runs I1O1T with the same source repeated. The next frame
//! only fills the second anchor. From then on each three-frame window runs
//! I5O2 and emits its two destination slots in field-order order.

use crate::hal::iep::{Iep2DilMode, Iep2FieldOrder, Iep2Fmt, Iep2OutMode, Iep2YuvSwap};
use crate::service::dma_heap::DmaBuf;
use crate::service::iep_session::{DeinterlaceConfig, IepDeinterlace};

pub struct Vproc {
    iep: IepDeinterlace,
    width: usize,
    height: usize,
    hor_stride: usize,
    ver_stride: usize,
    frame_bytes: usize,
    prev0: Option<DmaBuf>,
    prev1: Option<DmaBuf>,
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
            width,
            height,
            hor_stride,
            ver_stride,
            frame_bytes: width * height * 3 / 2,
            prev0: None,
            prev1: None,
        })
    }

    pub fn push(&mut self, frame: &DmaBuf, top_field_first: bool) -> Result<Vec<Vec<u8>>, String> {
        if frame.len() < self.frame_bytes {
            return Err(format!(
                "AVS vproc frame is {} bytes, needs {}",
                frame.len(),
                self.frame_bytes
            ));
        }

        if self.prev0.is_none() {
            for slot in 0..3 {
                self.import(slot, frame)?;
            }
            self.iep.set_mode(Iep2DilMode::I1O1T);
            self.iep.set_field_order(if top_field_first {
                Iep2FieldOrder::Tff
            } else {
                Iep2FieldOrder::Bff
            });
            self.iep.run()?;
            self.prev0 = Some(frame.try_clone().map_err(|error| error.to_string())?);
            return Ok(vec![self.iep.dst_frame_compact(0)?]);
        }

        if self.prev1.is_none() {
            self.prev1 = self.prev0.take();
            self.prev0 = Some(frame.try_clone().map_err(|error| error.to_string())?);
            return Ok(Vec::new());
        }

        let prev0 = self.prev0.take().expect("prev0 checked above");
        let prev1 = self.prev1.take().expect("prev1 checked above");
        self.import(0, &prev0)?;
        self.import(1, frame)?;
        self.import(2, &prev1)?;
        self.iep.set_mode(Iep2DilMode::I5O2);
        self.iep.set_field_order(if top_field_first {
            Iep2FieldOrder::Tff
        } else {
            Iep2FieldOrder::Bff
        });
        self.iep.run()?;

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
        let output = vec![
            self.iep.dst_frame_compact(first)?,
            self.iep.dst_frame_compact(second)?,
        ];
        self.prev1 = Some(prev0);
        self.prev0 = Some(frame.try_clone().map_err(|error| error.to_string())?);
        Ok(output)
    }

    fn import(&mut self, slot: usize, frame: &DmaBuf) -> Result<(), String> {
        self.iep.import_src_dmabuf(
            slot,
            frame,
            self.width,
            self.height,
            self.hor_stride,
            self.ver_stride,
        )
    }
}
