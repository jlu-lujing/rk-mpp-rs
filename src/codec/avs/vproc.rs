//! Stateful IEP2 post-processing matching C MPP's AVS decoder vproc thread.
//!
//! The first frame runs I1O1T with the same source repeated. The next frame
//! only fills the second anchor. From then on each three-frame window runs
//! I5O2 and emits its two destination slots in field-order order.

use crate::hal::iep::{Iep2DilMode, Iep2FieldOrder, Iep2Fmt, Iep2OutMode, Iep2YuvSwap};
use crate::service::iep_session::{DeinterlaceConfig, IepDeinterlace};

pub struct Vproc {
    iep: IepDeinterlace,
    frame_bytes: usize,
    prev0: Option<Vec<u8>>,
    prev1: Option<Vec<u8>>,
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

    pub fn push(&mut self, frame: &[u8], top_field_first: bool) -> Result<Vec<Vec<u8>>, String> {
        if frame.len() != self.frame_bytes {
            return Err(format!(
                "AVS vproc frame is {} bytes, expected {}",
                frame.len(),
                self.frame_bytes
            ));
        }

        if self.prev0.is_none() {
            for slot in 0..3 {
                self.copy_src(slot, frame)?;
            }
            self.iep.set_mode(Iep2DilMode::I1O1T);
            self.iep.set_field_order(if top_field_first {
                Iep2FieldOrder::Tff
            } else {
                Iep2FieldOrder::Bff
            });
            self.iep.run()?;
            self.prev0 = Some(frame.to_vec());
            return Ok(vec![self.iep.dst_frame_compact(0)?]);
        }

        if self.prev1.is_none() {
            self.prev1 = self.prev0.take();
            self.prev0 = Some(frame.to_vec());
            return Ok(Vec::new());
        }

        let prev0 = self.prev0.take().expect("prev0 checked above");
        let prev1 = self.prev1.take().expect("prev1 checked above");
        self.copy_src(0, &prev0)?;
        self.copy_src(1, frame)?;
        self.copy_src(2, &prev1)?;
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
        self.prev0 = Some(frame.to_vec());
        Ok(output)
    }

    fn copy_src(&mut self, slot: usize, frame: &[u8]) -> Result<(), String> {
        let src = self.iep.src_frame_mut(slot);
        src.sync_write_begin()
            .map_err(|error| format!("AVS vproc src{slot} begin: {error}"))?;
        let count = frame.len().min(src.len());
        src.as_mut_slice()[..count].copy_from_slice(&frame[..count]);
        self.iep
            .flush_src(slot)
            .map_err(|error| format!("AVS vproc src{slot} flush: {error}"))
    }
}
