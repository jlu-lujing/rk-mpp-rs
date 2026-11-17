//! End-to-end HEVC decoder: Annex-B in, packed planar output, rkvdec underneath.

use std::io;

use crate::codec::DmaBufSurface;
use crate::hal::hevc::{ChromaFormat, Frame, HevcHw};
use crate::hal::hevc383::Hevc383;
use crate::service::buffer_pool::BufferPool;
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{SocProfile, VideoDecodeHal};

use super::dpb::{Dpb, FrameStore};
use super::parser::{self, NalUnit, Pps, SliceHeader, Sps};

pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

struct PictureSlices {
    sps: Sps,
    pps: Pps,
    headers: Vec<SliceHeader>,
}

pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// Picture order count used for display-order output.
    pub poc: i32,
    /// IDR/BLA pictures start a new display-order epoch.
    pub epoch: u32,
    pub yuv: Vec<u8>,
    /// Strided hardware NV12 surface when DMA-BUF output is enabled.
    pub dmabuf: Option<DmaBufSurface>,
    pub chroma_format: ChromaFormat,
    pub bit_depth: u32,
    /// MPP-compatible frame error mask.
    pub err_info: u32,
}

pub struct Decoder {
    sps: Vec<Option<Sps>>,
    pps: Vec<Option<Pps>>,
    dpb: Dpb,
    hw: HevcHardware,
    vdpu383: bool,
    frames: Vec<Option<Frame>>,
    video_pool: BufferPool<DmaBuf>,
    colmv_pool: BufferPool<DmaBuf>,
    output_injection: Option<DmaBuf>,
    width: usize,
    height: usize,
    bit_depth: u32,
    chroma_format: ChromaFormat,
    ctu_size: usize,
    poc_epoch: u32,
    active_sps_id: usize,
    output_dmabuf: bool,
    /// Submit without waiting and hand back an older picture instead.
    pipeline: bool,
    depth: usize,
    /// One entry per submitted job, oldest first.
    pending: std::collections::VecDeque<Option<PendingHevcFrame>>,
    /// Pictures already materialised but not handed to the caller yet.
    ready: std::collections::VecDeque<Picture>,
    /// A queued job failed after later pictures were already submitted. Until
    /// the next IRAP those pictures are predicted from a corrupt reference,
    /// so they must be reported as reference errors like the blocking path
    /// (which learns the verdict before decoding them).
    pipeline_failed: bool,
}

/// A submitted picture whose pixels may only be read once the driver reports
/// its job. The leases keep every surface the job touches alive.
struct PendingHevcFrame {
    poc: i32,
    epoch: u32,
    err_info: u32,
    /// DPB buffer index, so a hardware failure can be written back.
    slot: usize,
    video: crate::service::buffer_pool::BufferLease<DmaBuf>,
    _colmv: crate::service::buffer_pool::BufferLease<DmaBuf>,
    _refs: Vec<crate::service::buffer_pool::BufferLease<DmaBuf>>,
    /// The video block reads the bitstream for the whole job.
    _stream: DmaBuf,
    width: usize,
    height: usize,
    hor_stride: usize,
    ver_stride: usize,
    bit_depth: u32,
    chroma_format: ChromaFormat,
}

enum HevcHardware {
    Vdpu34x(HevcHw),
    Vdpu383(Hevc383),
}

impl HevcHardware {
    #[allow(clippy::too_many_arguments)]
    fn decode(
        &mut self,
        sps: &Sps,
        pps: &Pps,
        sh: &SliceHeader,
        poc: i32,
        cur: &Frame,
        refs: &[Option<&Frame>],
        ref_pocs: &[i32],
        ref_errors: &[bool],
        stream: &DmaBuf,
        raw_len: usize,
        slice_count: u32,
        nal_type: u8,
    ) -> io::Result<()> {
        match self {
            Self::Vdpu34x(hw) => hw.decode(
                sps,
                pps,
                sh,
                poc,
                cur,
                refs,
                ref_pocs,
                ref_errors,
                stream,
                raw_len,
                slice_count,
                nal_type,
            ),
            Self::Vdpu383(hw) => hw.decode(
                sps,
                pps,
                sh,
                poc,
                cur,
                refs,
                ref_pocs,
                ref_errors,
                stream,
                raw_len,
                slice_count,
                nal_type,
            ),
        }
    }

    fn status(&self) -> (u32, u32, u32) {
        match self {
            Self::Vdpu34x(hw) => hw.last_status,
            Self::Vdpu383(hw) => hw.last_status,
        }
    }

    fn last_hardware_error(&self) -> bool {
        match self {
            Self::Vdpu34x(hw) => hw.last_hardware_error,
            Self::Vdpu383(hw) => hw.last_hardware_error,
        }
    }

    /// Turn on submit-without-wait where the HAL has a job ring.
    fn set_pipeline(&mut self, enabled: bool, depth: usize) -> bool {
        match self {
            Self::Vdpu34x(hw) => {
                hw.set_pipeline(enabled);
                hw.set_depth(depth);
                true
            }
            Self::Vdpu383(_) => false,
        }
    }

    fn in_flight(&self) -> usize {
        match self {
            Self::Vdpu34x(hw) => hw.in_flight(),
            Self::Vdpu383(_) => 0,
        }
    }

    fn reap(&mut self) -> io::Result<bool> {
        match self {
            Self::Vdpu34x(hw) => hw.reap(),
            Self::Vdpu383(_) => Ok(false),
        }
    }
}

impl Decoder {
    pub fn new() -> io::Result<Decoder> {
        let profile = SocProfile::detect()?;
        let vdpu383 = profile.video_decode == VideoDecodeHal::Vdpu383;
        let hw = match profile.video_decode {
            VideoDecodeHal::Vdpu34x => HevcHardware::Vdpu34x(HevcHw::open()?),
            VideoDecodeHal::Vdpu383 => HevcHardware::Vdpu383(Hevc383::open()?),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("no H.265 decode HAL for {:?}", other),
                ));
            }
        };
        Ok(Decoder {
            sps: (0..16).map(|_| None).collect(),
            pps: (0..64).map(|_| None).collect(),
            dpb: Dpb::new(),
            hw,
            vdpu383,
            frames: (0..17).map(|_| None).collect(),
            // 16 DPB references plus the current picture, and `JOB_SLOTS`
            // spare surfaces still leased by jobs in the submission ring.
            video_pool: BufferPool::new(17 + crate::hal::hevc::JOB_SLOTS),
            colmv_pool: BufferPool::new(17 + crate::hal::hevc::JOB_SLOTS),
            output_injection: None,
            width: 0,
            height: 0,
            bit_depth: 8,
            chroma_format: ChromaFormat::Yuv420,
            ctu_size: 16,
            poc_epoch: 0,
            active_sps_id: 0,
            output_dmabuf: false,
            pipeline: false,
            depth: 1,
            pending: std::collections::VecDeque::with_capacity(crate::hal::hevc::JOB_SLOTS),
            ready: std::collections::VecDeque::new(),
            pipeline_failed: false,
        })
    }

    /// Register read-back after the last job: (reg224, reg225, reg230).
    pub fn status(&self) -> (u32, u32, u32) {
        self.hw.status()
    }

    /// Bounded display delay announced by the active SPS.
    pub fn max_num_reorder_pics(&self) -> usize {
        self.sps
            .get(self.active_sps_id)
            .and_then(Option::as_ref)
            .map(|sps| sps.max_num_reorder_pics as usize)
            .unwrap_or(0)
            .min(16)
    }

    fn ensure_frame(&mut self, slot: usize) -> io::Result<()> {
        if self.frames[slot].is_none() {
            if let Some(buffer) = self.output_injection.take() {
                self.video_pool.clear();
                self.video_pool
                    .import(buffer)
                    .map_err(|error| io::Error::other(error.to_string()))?;
            }
            let frame = if self.vdpu383 {
                Frame::acquire_vdpu383(
                    &self.video_pool,
                    &self.colmv_pool,
                    self.width,
                    self.height,
                    self.ctu_size,
                    self.bit_depth,
                    self.chroma_format,
                )
            } else {
                Frame::acquire(
                    &self.video_pool,
                    &self.colmv_pool,
                    self.width,
                    self.height,
                    self.ctu_size,
                    self.bit_depth,
                    self.chroma_format,
                )
            };
            self.frames[slot] =
                Some(frame.map_err(|error| io::Error::other(format!("HEVC frame pool: {error}")))?);
        }
        Ok(())
    }

    fn release_frames_outside_dpb(&mut self) {
        let live: Vec<usize> = self.dpb.frames().iter().map(|frame| frame.buf).collect();
        for (index, frame) in self.frames.iter_mut().enumerate() {
            if frame.is_some() && !live.contains(&index) {
                *frame = None;
            }
        }
    }

    /// Decode one access unit (all NAL units of one picture).
    pub fn decode_access_unit(&mut self, aub: &[u8]) -> Result<Option<Picture>, String> {
        let nals = parser::split_annex_b(aub);
        if nals.is_empty() {
            return Ok(None);
        }
        for n in &nals {
            match n.unit_type {
                parser::NAL_SPS => {
                    let s = parser::parse_sps(&n.rbsp).map_err(|e| e.to_string())?;
                    let id = (s.sps_id as usize) % 16;
                    let bit_depth = s.bit_depth_luma.max(s.bit_depth_chroma);
                    let chroma_format = ChromaFormat::from_idc(s.chroma_format_idc)
                        .ok_or("invalid chroma_format_idc")?;
                    if self.width != s.width as usize
                        || self.height != s.height as usize
                        || self.bit_depth != bit_depth
                        || self.chroma_format != chroma_format
                    {
                        self.dpb.clear();
                        for frame in &mut self.frames {
                            *frame = None;
                        }
                        self.video_pool.clear();
                        self.colmv_pool.clear();
                        self.width = s.width as usize;
                        self.height = s.height as usize;
                        self.bit_depth = bit_depth;
                        self.chroma_format = chroma_format;
                        self.ctu_size = 1 << (s.log2_min_cb + s.log2_diff_max_min_cb);
                    }
                    self.sps[id] = Some(s);
                }
                parser::NAL_PPS => {
                    let p = parser::parse_pps(&n.rbsp).map_err(|e| e.to_string())?;
                    let id = (p.pps_id as usize) % 64;
                    self.pps[id] = Some(p);
                }
                _ => {}
            }
        }

        let slices: Vec<&parser::NalUnit<'_>> = nals.iter().filter(|n| n.is_vcl()).collect();
        if slices.is_empty() {
            return Ok(None);
        }
        let first = slices[0];
        let picture_slices = parse_picture_slices(&slices, &self.sps, &self.pps)?;
        let sps = picture_slices.sps;
        self.active_sps_id = (sps.sps_id as usize) % 16;
        if self.output_dmabuf && (self.bit_depth != 8 || self.chroma_format != ChromaFormat::Yuv420)
        {
            return Err(format!(
                "H.265 DMA-BUF output supports only 8-bit 4:2:0 NV12, got {}-bit {:?}",
                self.bit_depth, self.chroma_format
            ));
        }
        let pps = picture_slices.pps;
        let sh = picture_slices
            .headers
            .into_iter()
            .next()
            .ok_or("missing slice header")?;

        if starts_new_display_epoch(first.unit_type) {
            self.poc_epoch = self.poc_epoch.saturating_add(1);
        }
        let epoch = self.poc_epoch;
        let poc = self
            .dpb
            .compute_poc(&sh, &sps, resets_picture_order_count(first.unit_type));

        self.release_frames_outside_dpb();
        let slot = match self.frames.iter().position(Option::is_none) {
            Some(slot) => slot,
            None => {
                self.frames.push(None);
                self.frames.len() - 1
            }
        };
        // Backpressure: every deferred job keeps its surface leased, so when
        // the pool is nearly exhausted reap the oldest job early and queue its
        // picture. The caller drains it on a later access unit.
        if self.pipeline && self.pending.len() > 1 {
            let stats = self.video_pool.stats();
            if stats.in_use + 1 >= stats.limit {
                if let Some(entry) = self.pending.pop_front() {
                    let failed = self.hw.reap().map_err(|e| e.to_string())?;
                    if let Some(mut picture) = entry {
                        if failed {
                            picture.err_info |= FRAME_ERR_DEC_HW_ERR;
                        }
                        let picture = self.materialise(&picture)?;
                        self.ready.push_back(picture);
                    }
                }
            }
        }
        self.ensure_frame(slot).map_err(|e| e.to_string())?;

        let l0 = self.dpb.build_ref_list(
            &sh.rps,
            &sh.long_term_rps,
            poc,
            sps.log2_max_pic_order_cnt_lsb,
        );
        let is_irap = matches!(
            first.unit_type,
            16..=21 | parser::NAL_IDR_W_RADL | parser::NAL_IDR_N_LP | parser::NAL_CRA
        );
        if is_irap {
            // A clean random access point restarts the prediction chain.
            self.pipeline_failed = false;
        }
        let poisoned = self.pipeline && self.pipeline_failed && !is_irap;
        let reference_error = poisoned
            || self.dpb.reference_set_has_error(
                &sh.rps,
                &sh.long_term_rps,
                poc,
                sps.log2_max_pic_order_cnt_lsb,
            );
        let mut refs: Vec<Option<&Frame>> = vec![None; 16];
        let mut ref_pocs: Vec<i32> = vec![0; 16];
        let mut ref_errors: Vec<bool> = vec![true; 16];
        for (i, &s) in l0.iter().enumerate().take(16) {
            if let Some(f) = self.dpb.frames().iter().find(|f| f.slot == s) {
                refs[i] = self.frames.get(f.buf).and_then(|x| x.as_ref());
                ref_pocs[i] = f.poc;
                ref_errors[i] = f.error;
            }
        }

        if std::env::var("RFI_HEVC_LISTS").is_ok() {
            eprintln!(
                "AU type={} poc={} rps s0={:?}/{:?} s1={:?}/{:?} l0={:?} dpb={:?}",
                first.unit_type,
                poc,
                sh.rps.s0,
                sh.rps.used_s0,
                sh.rps.s1,
                sh.rps.used_s1,
                l0,
                self.dpb
                    .frames()
                    .iter()
                    .map(|f| (f.slot, f.poc))
                    .collect::<Vec<_>>()
            );
        }

        let mut job_stream = None;
        let mut submitted = false;
        let err_info = if reference_error {
            frame_error(true, false)
        } else {
            // The decoder reads from the first byte, so the access unit is fed
            // from the first NAL header (start code stripped).
            let mut payload: Vec<u8> = Vec::with_capacity(aub.len());
            for n in slices.iter() {
                let code_len =
                    if aub.len() >= n.offset + 4 && aub[n.offset..n.offset + 4] == [0, 0, 0, 1] {
                        4
                    } else {
                        3
                    };
                let start = n.offset + (code_len - 3);
                let end = (n.offset + n.len).min(aub.len());
                payload.extend_from_slice(&aub[start..end]);
            }
            let raw_len = payload.len();
            let pad16 = (16 - raw_len % 16) % 16;
            payload.extend(std::iter::repeat(0u8).take(pad16 + 64));
            let mut stream =
                DmaBuf::alloc("system", payload.len(), false).map_err(|e| e.to_string())?;
            stream.sync_write_begin().map_err(|e| e.to_string())?;
            stream.as_mut_slice()[..payload.len()].copy_from_slice(&payload);
            stream.sync_write_end().map_err(|e| e.to_string())?;

            let cur = self.frames[slot].as_ref().unwrap();
            let slice_count = slices.len() as u32;
            self.hw
                .decode(
                    &sps,
                    &pps,
                    &sh,
                    poc,
                    cur,
                    &refs,
                    &ref_pocs,
                    &ref_errors,
                    &stream,
                    raw_len,
                    slice_count,
                    first.unit_type,
                )
                .map_err(|e| e.to_string())?;
            submitted = true;
            job_stream = Some(stream);
            if self.pipeline {
                // The verdict belongs to the picture drained next.
                frame_error(false, false)
            } else {
                frame_error(false, self.hw.last_hardware_error())
            }
        };

        if !sh.dependent {
            if first.unit_type == parser::NAL_CRA {
                // An open-GOP CRA declares no current references, but its
                // following RASL pictures can still use pictures from before
                // the CRA. MPP keeps those DPB entries until the first RASL
                // RPS selects the subset it needs.
                self.dpb.clear_reference_marks();
            } else {
                self.dpb.refresh_references(
                    &sh.rps,
                    &sh.long_term_rps,
                    poc,
                    sps.log2_max_pic_order_cnt_lsb,
                );
            }
            self.dpb.push(FrameStore {
                poc,
                slot: slot as u8,
                buf: slot,
                referenced: false,
                error: err_info != 0,
                long_term: false,
            });
        }
        self.release_frames_outside_dpb();

        if self.pipeline {
            let current = if submitted {
                Some(self.defer(slot, poc, epoch, err_info, job_stream.take())?)
            } else {
                None
            };
            let step = self.pipeline_step(submitted, current)?;
            if !submitted {
                // Reference/parse error: this access unit armed no job, so its
                // picture is complete now.
                self.ready.push_back(Picture {
                    width: self.width,
                    height: self.height,
                    poc,
                    epoch,
                    yuv: Vec::new(),
                    dmabuf: None,
                    chroma_format: self.chroma_format,
                    bit_depth: self.bit_depth,
                    err_info,
                });
            }
            return Ok(step.or_else(|| self.ready.pop_front()));
        }

        let (yuv, dmabuf) = if err_info == 0 {
            let frame = self.frames[slot].as_ref().unwrap();
            if self.output_dmabuf {
                let lease = frame
                    .video
                    .try_clone()
                    .map_err(|error| format!("HEVC DMA-BUF lease: {error}"))?;
                (
                    Vec::new(),
                    Some(DmaBufSurface::pooled(
                        lease,
                        frame.hor_stride,
                        frame.ver_stride,
                    )),
                )
            } else {
                let video = frame
                    .video
                    .data()
                    .map_err(|error| format!("HEVC frame view: {error}"))?;
                let _ = video.buffer().sync_read_begin();
                let y_size = frame.hor_stride * frame.ver_stride;
                let uv_size = y_size * frame.chroma_format.uv_size_factor() / 2;
                let yuv = copy_planar(
                    &video[..y_size],
                    &video[y_size..y_size + uv_size],
                    self.width,
                    self.height,
                    frame.hor_stride,
                    frame.bit_depth,
                    frame.chroma_format,
                );
                let _ = video.buffer().sync_read_end();
                (yuv, None)
            }
        } else {
            (Vec::new(), None)
        };
        Ok(Some(Picture {
            width: self.width,
            height: self.height,
            poc,
            epoch,
            yuv,
            dmabuf,
            chroma_format: self.chroma_format,
            bit_depth: self.bit_depth,
            err_info,
        }))
    }

    /// Lease every surface this job touches and remember how to read it out.
    fn defer(
        &mut self,
        slot: usize,
        poc: i32,
        epoch: u32,
        err_info: u32,
        stream: Option<DmaBuf>,
    ) -> Result<PendingHevcFrame, String> {
        let frame = self.frames[slot].as_ref().expect("decoded frame slot");
        let mut held = Vec::new();
        for reference in self
            .dpb
            .frames()
            .iter()
            .filter_map(|entry| self.frames.get(entry.buf).and_then(Option::as_ref))
        {
            held.push(
                reference
                    .video
                    .try_clone()
                    .map_err(|error| format!("HEVC reference lease: {error}"))?,
            );
            held.push(
                reference
                    .colmv
                    .try_clone()
                    .map_err(|error| format!("HEVC reference lease: {error}"))?,
            );
        }
        Ok(PendingHevcFrame {
            poc,
            epoch,
            err_info,
            slot,
            video: frame
                .video
                .try_clone()
                .map_err(|error| format!("HEVC pipeline lease: {error}"))?,
            _colmv: frame
                .colmv
                .try_clone()
                .map_err(|error| format!("HEVC pipeline colmv lease: {error}"))?,
            _refs: held,
            _stream: stream.expect("submitted job owns its bitstream"),
            width: frame.width,
            height: frame.height,
            hor_stride: frame.hor_stride,
            ver_stride: frame.ver_stride,
            bit_depth: frame.bit_depth,
            chroma_format: frame.chroma_format,
        })
    }

    /// Reap the oldest job if the ring is full, then queue this picture.
    fn pipeline_step(
        &mut self,
        submitted: bool,
        mut current: Option<PendingHevcFrame>,
    ) -> Result<Option<Picture>, String> {
        let mut failed = false;
        if self.hw.in_flight() >= self.depth {
            failed = self.hw.reap().map_err(|e| e.to_string())?;
            if let Some(Some(mut picture)) = self.pending.pop_front() {
                if failed {
                    picture.err_info |= FRAME_ERR_DEC_HW_ERR;
                    self.pipeline_failed = true;
                    // The blocking path knows the verdict before the DPB entry
                    // is pushed; a deferred picture is already in the DPB, so
                    // propagate the failure here instead.
                    self.dpb.mark_error(picture.slot);
                    // Pictures already submitted after this one were predicted
                    // from the corrupt chain, exactly like the blocking path's
                    // reference-error verdict.
                    for entry in self.pending.iter_mut() {
                        if let Some(later) = entry {
                            later.err_info |= FRAME_ERR_DEC_INVALID;
                            self.dpb.mark_error(later.slot);
                        }
                    }
                }
                let picture = self.materialise(&picture)?;
                self.ready.push_back(picture);
            }
        }
        // This access unit was already submitted when the failure above was
        // discovered, so it also decoded from the corrupt chain.
        if failed {
            if let Some(picture) = current.as_mut() {
                picture.err_info |= FRAME_ERR_DEC_INVALID;
                self.dpb.mark_error(picture.slot);
            }
        }
        if submitted {
            self.pending.push_back(current);
        }
        Ok(self.ready.pop_front())
    }

    /// Build the caller-facing picture for a drained job.
    fn materialise(&self, pending: &PendingHevcFrame) -> Result<Picture, String> {
        let (yuv, dmabuf) = if pending.err_info == 0 {
            if self.output_dmabuf {
                let lease = pending
                    .video
                    .try_clone()
                    .map_err(|error| format!("HEVC DMA-BUF lease: {error}"))?;
                (
                    Vec::new(),
                    Some(DmaBufSurface::pooled(
                        lease,
                        pending.hor_stride,
                        pending.ver_stride,
                    )),
                )
            } else {
                let video = pending
                    .video
                    .data()
                    .map_err(|error| format!("HEVC frame view: {error}"))?;
                let _ = video.buffer().sync_read_begin();
                let y_size = pending.hor_stride * pending.ver_stride;
                let uv_size = y_size * pending.chroma_format.uv_size_factor() / 2;
                let yuv = copy_planar(
                    &video[..y_size],
                    &video[y_size..y_size + uv_size],
                    pending.width,
                    pending.height,
                    pending.hor_stride,
                    pending.bit_depth,
                    pending.chroma_format,
                );
                let _ = video.buffer().sync_read_end();
                (yuv, None)
            }
        } else {
            (Vec::new(), None)
        };
        Ok(Picture {
            width: pending.width,
            height: pending.height,
            poc: pending.poc,
            epoch: pending.epoch,
            yuv,
            dmabuf,
            chroma_format: pending.chroma_format,
            bit_depth: pending.bit_depth,
            err_info: pending.err_info,
        })
    }

    /// Keep `depth` jobs in the submission ring. Returns false on hardware
    /// without a ring (RK3576 VDPU383).
    pub fn set_pipeline_depth(&mut self, enabled: bool, depth: usize) -> bool {
        self.depth = depth.max(1);
        self.pipeline = enabled && self.hw.set_pipeline(true, self.depth);
        self.pipeline
    }

    /// Drain every in-flight job. Call once at end of stream.
    pub fn flush(&mut self) -> Result<Vec<Picture>, String> {
        let mut out = Vec::new();
        if !self.pipeline {
            return Ok(out);
        }
        // Reap one at a time so a failure is attributed to its own picture.
        while self.hw.in_flight() > 0 {
            let failed = self.hw.reap().map_err(|e| e.to_string())?;
            if let Some(entry) = self.pending.pop_front() {
                if let Some(mut picture) = entry {
                    if failed {
                        picture.err_info |= FRAME_ERR_DEC_HW_ERR;
                        self.pipeline_failed = true;
                        self.dpb.mark_error(picture.slot);
                    }
                    let picture = self.materialise(&picture)?;
                    self.ready.push_back(picture);
                }
            }
        }
        while let Some(picture) = self.ready.pop_front() {
            out.push(picture);
        }
        Ok(out)
    }

    /// True once a queued job failed. The caller can then re-decode the
    /// stream through the blocking path, which learns each verdict before the
    /// next picture and therefore reproduces the documented error semantics.
    pub fn pipeline_poisoned(&self) -> bool {
        self.pipeline && self.pipeline_failed
    }

    pub fn set_output_dmabuf(&mut self, enabled: bool) {
        self.output_dmabuf = enabled;
    }

    pub fn import_output_dmabuf(&mut self, buffer: DmaBuf) -> Result<(), String> {
        self.set_output_dmabuf(true);
        self.output_injection = Some(buffer);
        Ok(())
    }
}

fn parse_picture_slices(
    slices: &[&NalUnit<'_>],
    sps_by_id: &[Option<Sps>],
    pps_by_id: &[Option<Pps>],
) -> Result<PictureSlices, String> {
    let first = slices.first().ok_or("missing slice segment")?;
    let first_pps_id = slice_pps_id(&first.rbsp, first.unit_type).map_err(|e| e.to_string())?;
    let pps = pps_by_id
        .get(first_pps_id as usize)
        .and_then(|p| p.clone())
        .ok_or("missing PPS")?;
    let sps = sps_by_id
        .get((pps.sps_id as usize) % 16)
        .and_then(|s| s.clone())
        .ok_or("missing SPS")?;

    let mut headers = Vec::with_capacity(slices.len());
    for (index, slice) in slices.iter().enumerate() {
        if slice.unit_type != first.unit_type {
            return Err("NAL type changed between slices".to_string());
        }
        let pps_id =
            slice_pps_id(&slice.rbsp, slice.unit_type).map_err(|error| error.to_string())?;
        if pps_id != first_pps_id {
            return Err("PPS changed between slices".to_string());
        }
        let header = parser::parse_slice_header(&slice.rbsp, &sps, &pps, slice.unit_type)
            .map_err(|error| error.to_string())?;
        if index == 0 && !header.first_slice {
            return Err("first slice segment is missing".to_string());
        }
        if index > 0 && header.first_slice {
            return Err("multiple pictures in one access unit".to_string());
        }
        headers.push(header);
    }

    Ok(PictureSlices { sps, pps, headers })
}

/// Convert the hardware's semi-planar chroma layout to planar
/// yuv420p/yuv422p/yuv444p (or their 10-bit variants).
fn copy_planar(
    y: &[u8],
    uv: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    bit_depth: u32,
    chroma_format: ChromaFormat,
) -> Vec<u8> {
    let sample_bytes = if bit_depth > 8 { 2 } else { 1 };
    let plane_size = width * height * sample_bytes;
    let uv_factor = match chroma_format {
        ChromaFormat::Monochrome => 0,
        ChromaFormat::Yuv420 => 1,
        ChromaFormat::Yuv422 => 2,
        ChromaFormat::Yuv444 => 4,
    };
    let mut out = Vec::with_capacity(plane_size + plane_size * uv_factor / 2);
    if bit_depth == 8 && chroma_format == ChromaFormat::Yuv420 {
        // Hot path: the shared readback reads the uncached chroma plane once.
        return crate::codec::yuv::nv12_to_planar(y, uv, width, height, stride);
    }
    if bit_depth == 8 {
        for row in 0..height {
            let off = row * stride;
            out.extend_from_slice(&y[off..off + width]);
        }
        match chroma_format {
            ChromaFormat::Monochrome => {}
            ChromaFormat::Yuv420 | ChromaFormat::Yuv422 => {
                let chroma_height = if chroma_format == ChromaFormat::Yuv420 {
                    height / 2
                } else {
                    height
                };
                let pairs = width / 2;
                for row in 0..chroma_height {
                    let off = row * stride;
                    for sample in 0..pairs {
                        out.push(uv[off + 2 * sample]);
                    }
                }
                for row in 0..chroma_height {
                    let off = row * stride;
                    for sample in 0..pairs {
                        out.push(uv[off + 2 * sample + 1]);
                    }
                }
            }
            ChromaFormat::Yuv444 => {
                // 4:4:4 semi-planar has two interleaved bytes per pixel and
                // a chroma row stride twice the luma stride.
                for component in 0..2 {
                    for row in 0..height {
                        let off = row * stride * 2;
                        for sample in 0..width {
                            out.push(uv[off + 2 * sample + component]);
                        }
                    }
                }
            }
        }
    } else {
        for row in 0..height {
            let off = row * stride;
            for sample in 0..width {
                let value = read_packed_sample(&y[off..], sample, bit_depth);
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        match chroma_format {
            ChromaFormat::Monochrome => {}
            ChromaFormat::Yuv420 => {
                let pairs = width / 2;
                for row in 0..height / 2 {
                    let off = row * stride;
                    for sample in 0..pairs {
                        let value = read_packed_sample(&uv[off..], sample * 2, bit_depth);
                        out.extend_from_slice(&value.to_le_bytes());
                    }
                }
                for row in 0..height / 2 {
                    let off = row * stride;
                    for sample in 0..pairs {
                        let value = read_packed_sample(&uv[off..], sample * 2 + 1, bit_depth);
                        out.extend_from_slice(&value.to_le_bytes());
                    }
                }
            }
            ChromaFormat::Yuv422 => {
                // MPP stores the packed 4:2:2 chroma rows as interleaved
                // U/V sample pairs, with one chroma row per luma row.
                let pairs = width / 2;
                for component in 0..2 {
                    for row in 0..height {
                        let off = row * stride;
                        for sample in 0..pairs {
                            let value =
                                read_packed_sample(&uv[off..], sample * 2 + component, bit_depth);
                            out.extend_from_slice(&value.to_le_bytes());
                        }
                    }
                }
            }
            ChromaFormat::Yuv444 => {
                // The 10-bit 4:4:4 hardware layout is two packed component
                // planes (all U rows, then all V rows), each with the luma
                // byte stride.
                for row in 0..height * 2 {
                    let off = row * stride;
                    for sample in 0..width {
                        let value = read_packed_sample(&uv[off..], sample, bit_depth);
                        out.extend_from_slice(&value.to_le_bytes());
                    }
                }
            }
        }
    }
    out
}

fn read_packed_sample(row: &[u8], index: usize, bit_depth: u32) -> u16 {
    let bit = index * bit_depth as usize;
    let byte = bit / 8;
    let shift = bit % 8;
    let word = row[byte] as u32
        | (row.get(byte + 1).copied().unwrap_or(0) as u32) << 8
        | (row.get(byte + 2).copied().unwrap_or(0) as u32) << 16;
    ((word >> shift) & ((1u32 << bit_depth) - 1)) as u16
}

/// IDR and BLA pictures reset POC and start a new display-order epoch.
fn starts_new_display_epoch(unit_type: u8) -> bool {
    (16..=20).contains(&unit_type)
}

/// IDR and BLA pictures reset POC. CRA retains POC continuity for RASL.
fn resets_picture_order_count(unit_type: u8) -> bool {
    (16..=20).contains(&unit_type)
}

/// Sort decoded pictures by epoch and POC for display.
pub fn sort_pictures_for_display(pictures: &mut [Picture]) {
    // Stable sorting preserves decode order if a malformed stream repeats a POC.
    pictures.sort_by_key(|picture| (picture.epoch, picture.poc));
}

/// Error pictures remain usable as references but are not sent to the display.
pub fn picture_is_output(picture: &Picture) -> bool {
    picture.err_info == 0
}

fn frame_error(reference_error: bool, hardware_error: bool) -> u32 {
    if reference_error {
        FRAME_ERR_DEC_INVALID
    } else if hardware_error {
        FRAME_ERR_DEC_HW_ERR
    } else {
        0
    }
}

/// Read only `slice_pic_parameter_set_id` from a slice header.
pub fn slice_pps_id(rbsp: &[u8], nal_type: u8) -> Result<u32, parser::HevcError> {
    use crate::codec::h264::parser::BitReader;
    let mut r = BitReader::new(rbsp);
    let _first_slice = r.read_bit()?;
    // IRAP pictures carry no_output_of_prior_pics_flag before the PPS id.
    if (16..=23).contains(&nal_type) {
        let _ = r.read_bit()?;
    }
    r.read_ue().map_err(|_| parser::HevcError::Truncated)
}

/// Split an Annex-B stream into access units at picture boundaries.
pub fn split_access_units(data: &[u8]) -> Vec<&[u8]> {
    let nals = parser::split_annex_b(data);
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut has_vcl = false;
    let mut trailing_non_vcl: Option<usize> = None;
    for n in &nals {
        let boundary = match n.unit_type {
            parser::NAL_AUD => true,
            t if t <= 31 => first_slice(&n.rbsp),
            _ => false,
        };
        if boundary && has_vcl {
            let next_start = trailing_non_vcl.unwrap_or(n.offset);
            out.push(&data[start.unwrap()..next_start]);
            start = Some(next_start);
            has_vcl = n.is_vcl();
            trailing_non_vcl = None;
            continue;
        }
        if start.is_none() {
            start = Some(n.offset);
        }
        if n.is_vcl() {
            has_vcl = true;
            trailing_non_vcl = None;
        } else if has_vcl && trailing_non_vcl.is_none() {
            trailing_non_vcl = Some(n.offset);
        }
    }

    if let Some(s) = start {
        if has_vcl {
            out.push(&data[s..]);
        }
    }
    out
}

fn first_slice(rbsp: &[u8]) -> bool {
    matches!(rbsp.first(), Some(b) if *b & 0x80 != 0)
}

#[cfg(test)]
mod tests {
    use super::{
        copy_planar, parse_picture_slices, read_packed_sample, resets_picture_order_count,
        sort_pictures_for_display, split_access_units, starts_new_display_epoch, ChromaFormat,
        Picture,
    };
    use crate::codec::hevc::parser::{NalUnit, Pps, Sps};
    use std::borrow::Cow;

    struct BitWriter {
        bits: Vec<u8>,
    }

    impl BitWriter {
        fn new() -> Self {
            Self { bits: Vec::new() }
        }

        fn put(&mut self, value: u32, width: u32) -> &mut Self {
            for shift in (0..width).rev() {
                self.bits.push(((value >> shift) & 1) as u8);
            }
            self
        }

        fn ue(&mut self, value: u32) -> &mut Self {
            let code = value + 1;
            let width = 32 - code.leading_zeros();
            self.put(0, width - 1).put(code, width)
        }

        fn finish(&mut self) -> Vec<u8> {
            self.bits.push(1);
            while self.bits.len() % 8 != 0 {
                self.bits.push(0);
            }
            let mut bytes = vec![0u8; self.bits.len() / 8];
            for (index, bit) in self.bits.iter().copied().enumerate() {
                bytes[index / 8] |= bit << (7 - index % 8);
            }
            bytes
        }
    }

    fn nal(unit_type: u8, rbsp: Vec<u8>) -> NalUnit<'static> {
        let len = rbsp.len() + 2;
        NalUnit {
            unit_type,
            layer_id: 0,
            temporal_id_plus1: 1,
            rbsp: Cow::Owned(rbsp),
            offset: 0,
            len,
        }
    }

    fn picture(epoch: u32, poc: i32, marker: u8) -> Picture {
        Picture {
            width: 1,
            height: 1,
            poc,
            epoch,
            yuv: vec![marker],
            dmabuf: None,
            chroma_format: ChromaFormat::Yuv420,
            bit_depth: 8,
            err_info: 0,
        }
    }

    #[test]
    fn parameter_sets_between_pictures_join_the_next_access_unit() {
        let mut data = vec![0u8, 0, 0, 1, 0x40, 0x01, 0x00];
        data.extend_from_slice(&[0, 0, 0, 1, 0x26, 0x01, 0x80]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x44, 0x01, 0x00]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x26, 0x01, 0x80]);

        let aus = split_access_units(&data);
        assert_eq!(aus.len(), 2);
        assert!(aus[1].starts_with(&[0, 0, 0, 1, 0x44, 0x01]));
    }

    #[test]
    fn display_order_sorts_by_epoch_then_poc() {
        let mut pictures = [
            picture(1, 8, 4),
            picture(2, 0, 5),
            picture(1, 0, 1),
            picture(1, 4, 3),
            picture(1, 2, 2),
        ];

        sort_pictures_for_display(&mut pictures);

        assert_eq!(
            pictures.map(|picture| (picture.epoch, picture.poc)),
            [(1, 0), (1, 2), (1, 4), (1, 8), (2, 0)]
        );
    }

    #[test]
    fn idr_and_bla_start_display_epochs_but_cra_does_not() {
        assert!(starts_new_display_epoch(16));
        assert!(starts_new_display_epoch(19));
        assert!(starts_new_display_epoch(20));
        assert!(!starts_new_display_epoch(21));
    }

    #[test]
    fn only_idr_and_bla_reset_poc_for_irap_pictures() {
        for unit_type in 16..=20 {
            assert!(resets_picture_order_count(unit_type));
        }
        for unit_type in 21..=23 {
            assert!(!resets_picture_order_count(unit_type));
        }
    }

    #[test]
    fn hardware_failures_are_not_displayed() {
        let mut picture = picture(1, 0, 1);
        assert!(super::picture_is_output(&picture));
        picture.err_info = super::FRAME_ERR_DEC_HW_ERR;
        assert!(!super::picture_is_output(&picture));
    }

    #[test]
    fn reference_errors_take_precedence_over_hardware_errors() {
        assert_eq!(
            super::frame_error(true, false),
            super::FRAME_ERR_DEC_INVALID
        );
        assert_eq!(super::frame_error(false, true), super::FRAME_ERR_DEC_HW_ERR);
        assert_eq!(super::frame_error(false, false), 0);
    }

    #[test]
    fn packed_10_bit_samples_stay_little_endian_and_unshifted() {
        // [0x159, 0x001] packed LSB-first as used by the rkvdec P010_10
        // hardware format. MPP's dump expands these to 16-bit samples.
        let row = [0x59, 0x05, 0x00];
        assert_eq!(read_packed_sample(&row, 0, 10), 0x159);
        assert_eq!(read_packed_sample(&row, 1, 10), 0x001);
    }

    #[test]
    fn packed_row_becomes_planar_yuv420p10le() {
        fn pack(values: &[u16]) -> Vec<u8> {
            let bits = values
                .iter()
                .enumerate()
                .fold(0u128, |bits, (index, value)| {
                    bits | ((*value as u128) << (index * 10))
                });
            bits.to_le_bytes()[..(values.len() * 10).div_ceil(8)].to_vec()
        }

        let mut y = pack(&[1, 2, 3, 4]);
        y.extend_from_slice(&pack(&[9, 10, 11, 12]));
        let uv = pack(&[5, 6, 7, 8]);
        let out = copy_planar(&y, &uv, 4, 2, 5, 10, ChromaFormat::Yuv420);
        let samples: Vec<u16> = out
            .chunks_exact(2)
            .map(|sample| u16::from_le_bytes([sample[0], sample[1]]))
            .collect();
        assert_eq!(samples, [1, 2, 3, 4, 9, 10, 11, 12, 5, 7, 6, 8]);
    }

    #[test]
    fn semiplanar_422_becomes_planar_422() {
        let y = [1, 2, 3, 4, 5, 6, 7, 8];
        let uv = [10, 20, 11, 21, 12, 22, 13, 23];

        let out = copy_planar(&y, &uv, 4, 2, 4, 8, ChromaFormat::Yuv422);

        assert_eq!(
            out,
            [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 20, 21, 22, 23]
        );
    }

    #[test]
    fn semiplanar_444_becomes_planar_444() {
        let y = [1, 2, 3, 4, 5, 6, 7, 8];
        let uv = [
            10, 20, 11, 21, 12, 22, 13, 23, 14, 24, 15, 25, 16, 26, 17, 27,
        ];

        let out = copy_planar(&y, &uv, 4, 2, 4, 8, ChromaFormat::Yuv444);

        assert_eq!(
            out,
            [
                1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 16, 17, 20, 21, 22, 23, 24, 25, 26,
                27
            ]
        );
    }

    #[test]
    fn parses_dependent_segments_after_the_first_slice() {
        let sps = Sps {
            width: 1280,
            height: 720,
            log2_min_cb: 3,
            log2_diff_max_min_cb: 3,
            ..Default::default()
        };
        let pps = Pps {
            dependent_slice_segments: true,
            ..Default::default()
        };
        let mut first_bits = BitWriter::new();
        first_bits
            .put(1, 1) // first_slice_segment_in_pic_flag
            .put(0, 1) // no_output_of_prior_pics_flag
            .ue(0) // pps id
            .ue(2); // I slice
        let mut dependent_bits = BitWriter::new();
        dependent_bits
            .put(0, 1) // first_slice_segment_in_pic_flag
            .put(0, 1) // no_output_of_prior_pics_flag
            .ue(0) // pps id
            .put(1, 1) // dependent_slice_segment_flag
            .put(37, 8); // slice_segment_address
        let first = nal(19, first_bits.finish());
        let dependent = nal(19, dependent_bits.finish());
        let slices = [&first, &dependent];
        let mut sps_by_id: Vec<Option<Sps>> = (0..16).map(|_| None).collect();
        let mut pps_by_id: Vec<Option<Pps>> = (0..64).map(|_| None).collect();
        sps_by_id[0] = Some(sps);
        pps_by_id[0] = Some(pps);

        let picture = parse_picture_slices(&slices, &sps_by_id, &pps_by_id).unwrap();

        assert_eq!(picture.headers.len(), 2);
        assert!(picture.headers[0].first_slice);
        assert!(picture.headers[1].dependent);
        assert_eq!(picture.headers[1].segment_address, 37);
    }
}
