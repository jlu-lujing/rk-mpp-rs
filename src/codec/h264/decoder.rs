//! End-to-end H.264 decoder: Annex-B in, NV12 out, rkvdec underneath.

use std::io;

use crate::hal::h264d::{Frame, H264d};
use crate::hal::h264d383::H264d383;
use crate::service::buffer_pool::BufferPool;
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{SocProfile, VideoDecodeHal};

use super::dpb::{Dpb, FrameStore, PictureOrderCount, RefListEntry, ReferenceFlags};
use super::parser::{self, Pps, SliceHeader, Sps};

pub const FRAME_ERR_DEC_INVALID: u32 = 0x10;
pub const FRAME_ERR_DEC_HW_ERR: u32 = 0x100;

/// One decoded picture, cropped to the display size.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// Picture order count used for display-order output.
    pub poc: i32,
    /// IDR starts a new POC epoch.
    pub idr: bool,
    /// Planar YUV 4:2:0, tightly packed.
    pub yuv: Vec<u8>,
    /// Tightly packed NV12 when `Decoder::set_output_nv12(true)` is enabled.
    /// `yuv` is empty in that mode.
    pub nv12: Option<Vec<u8>>,
    /// Strided hardware NV12 surface when DMA-BUF output is enabled.
    pub dmabuf: Option<DmaBufPicture>,
    /// Raw interleaved chroma as the hardware wrote it (stride = width).
    pub raw_uv: Vec<u8>,
    /// MPP-compatible frame error mask (`0x10` invalid syntax/reference,
    /// `0x100` hardware decode failure).
    pub err_info: u32,
    /// MPP drops this frame from the display path instead of exposing it as a
    /// retained error reference.
    pub discard: bool,
}

pub type DmaBufPicture = crate::codec::DmaBufSurface;

/// Bounded display-order reorder buffer for streaming output.
pub struct DisplayReorder {
    pending: Vec<Picture>,
    max_reorder: usize,
}

impl DisplayReorder {
    pub fn new(max_reorder: usize) -> Self {
        Self {
            pending: Vec::with_capacity(max_reorder.saturating_add(1)),
            max_reorder: max_reorder.min(16),
        }
    }

    pub fn push<F>(&mut self, picture: Picture, mut emit: F)
    where
        F: FnMut(Picture),
    {
        if picture.idr && !self.pending.is_empty() {
            self.flush(&mut emit);
        }
        if !picture_is_output(&picture) {
            return;
        }
        self.pending.push(picture);
        self.pending.sort_by_key(|picture| picture.poc);
        while self.pending.len() > self.max_reorder {
            emit(self.pending.remove(0));
        }
    }

    pub fn set_max_reorder(&mut self, max_reorder: usize) {
        self.max_reorder = max_reorder.min(16);
    }

    pub fn flush<F>(&mut self, mut emit: F)
    where
        F: FnMut(Picture),
    {
        self.pending.sort_by_key(|picture| picture.poc);
        for picture in self.pending.drain(..) {
            emit(picture);
        }
    }
}

#[derive(Clone)]
struct PendingTopField {
    sh: SliceHeader,
    order: PictureOrderCount,
    poc: i32,
    slot: usize,
    idr: bool,
    err_info: u32,
}

pub struct Decoder {
    sps: Vec<Option<Sps>>,
    pps: Vec<Option<Pps>>,
    dpb: Dpb,
    hw: H264Hardware,
    vdpu383: bool,
    frames: Vec<Option<Frame>>,
    nv12_pool: BufferPool<DmaBuf>,
    colmv_pool: BufferPool<DmaBuf>,
    output_injection: Option<DmaBuf>,
    width: usize,
    height: usize,
    coded_height: usize,
    frame_mbs_only: bool,
    pending_top: Option<PendingTopField>,
    /// Slice payload and staging buffer, reused and grown across pictures.
    payload: Vec<u8>,
    stream: Option<DmaBuf>,
    // Keep the interleaved chroma of every picture, for the debug dump only.
    capture_raw_uv: bool,
    max_num_reorder_frames: usize,
    output_nv12: bool,
    output_dmabuf: bool,
}

enum H264Hardware {
    Vdpu34x(H264d),
    Vdpu383(H264d383),
}

impl H264Hardware {
    fn decode(
        &mut self,
        sps: &Sps,
        pps: &Pps,
        sh: &SliceHeader,
        order: PictureOrderCount,
        dpb: &[Option<&FrameStore>],
        lists: [&[RefListEntry]; 3],
        cur: &Frame,
        refs: &[Option<&Frame>],
        ref_pocs: &[i32],
        ref_flags: &[Option<ReferenceFlags>],
        ref_frame_nums: &[u32],
        stream: &DmaBuf,
        stream_len: usize,
        idr: bool,
    ) -> io::Result<()> {
        match self {
            Self::Vdpu34x(hw) => hw.decode(
                sps,
                pps,
                sh,
                order,
                dpb,
                lists,
                cur,
                refs,
                ref_pocs,
                ref_flags,
                ref_frame_nums,
                stream,
                stream_len,
                idr,
            ),
            Self::Vdpu383(hw) => hw.decode(
                sps,
                pps,
                sh,
                order,
                dpb,
                lists,
                cur,
                refs,
                ref_pocs,
                ref_flags,
                ref_frame_nums,
                stream,
                stream_len,
                idr,
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

    fn recover_session(&mut self) -> io::Result<()> {
        match self {
            Self::Vdpu34x(hw) => hw.recover_session(),
            Self::Vdpu383(hw) => hw.recover_session(),
        }
    }
}

impl Decoder {
    pub fn new() -> io::Result<Decoder> {
        let profile = SocProfile::detect()?;
        let vdpu383 = profile.video_decode == VideoDecodeHal::Vdpu383;
        let hw = match profile.video_decode {
            VideoDecodeHal::Vdpu34x => H264Hardware::Vdpu34x(H264d::open()?),
            VideoDecodeHal::Vdpu383 => H264Hardware::Vdpu383(H264d383::open()?),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("no H.264 decode HAL for {:?}", other),
                ));
            }
        };
        Ok(Decoder {
            sps: (0..32).map(|_| None).collect(),
            pps: (0..256).map(|_| None).collect(),
            dpb: Dpb::new(1, 0),
            hw,
            vdpu383,
            frames: (0..17).map(|_| None).collect(),
            nv12_pool: BufferPool::new(17),
            colmv_pool: BufferPool::new(17),
            output_injection: None,
            width: 0,
            height: 0,
            coded_height: 0,
            frame_mbs_only: true,
            pending_top: None,
            payload: Vec::new(),
            stream: None,
            capture_raw_uv: std::env::var("RFI_DUMP_UV").is_ok(),
            max_num_reorder_frames: 16,
            output_nv12: false,
            output_dmabuf: false,
        })
    }

    /// Register read-back after the last job: (reg001, reg045, reg076).
    pub fn status(&self) -> (u32, u32, u32) {
        self.hw.status()
    }

    /// Recover the underlying hardware session after a watchdog/ioctl/status
    /// failure. Decoder syntax and DPB state are intentionally retained; reset
    /// only drops the failed kernel job.
    pub fn recover_session(&mut self) -> io::Result<()> {
        H264Hardware::recover_session(&mut self.hw)
    }

    fn ensure_frame(&mut self, slot: usize, sps: &Sps) -> io::Result<()> {
        if self.frames[slot].is_none() {
            if let Some(buffer) = self.output_injection.take() {
                self.nv12_pool.clear();
                self.nv12_pool
                    .import(buffer)
                    .map_err(|error| io::Error::other(error.to_string()))?;
            }
            let frame = if self.vdpu383 {
                Frame::acquire_vdpu383(
                    &self.nv12_pool,
                    &self.colmv_pool,
                    sps.coded_width as usize,
                    sps.coded_height as usize,
                    sps.frame_mbs_only,
                )
            } else {
                Frame::acquire(
                    &self.nv12_pool,
                    &self.colmv_pool,
                    sps.coded_width as usize,
                    sps.coded_height as usize,
                    sps.frame_mbs_only,
                )
            };
            self.frames[slot] = Some(
                frame.map_err(|error| io::Error::other(format!("H.264 frame pool: {error}")))?,
            );
        }
        Ok(())
    }

    /// Decode one access unit (one picture worth of NAL units).
    pub fn decode_access_unit(&mut self, aub: &[u8]) -> Result<Option<Picture>, String> {
        let nals = parser::split_annex_b(aub);
        if nals.is_empty() {
            return Ok(None);
        }
        // 1. parameter sets
        for n in &nals {
            match n.unit_type {
                parser::NAL_SPS => {
                    let s = parser::parse_sps(&n.rbsp).map_err(|e| e.to_string())?;
                    let id = (s.sps_id as usize) % 32;
                    if self.width != s.width as usize
                        || self.height != s.height as usize
                        || self.coded_height != s.coded_height as usize
                        || self.frame_mbs_only != s.frame_mbs_only
                    {
                        self.width = s.width as usize;
                        self.height = s.height as usize;
                        self.coded_height = s.coded_height as usize;
                        self.frame_mbs_only = s.frame_mbs_only;
                        self.frames = (0..17).map(|_| None).collect();
                        self.nv12_pool.clear();
                        self.colmv_pool.clear();
                        self.pending_top = None;
                    }
                    self.sps[id] = Some(s);
                }
                parser::NAL_PPS => {
                    let p = parser::parse_pps(&n.rbsp).map_err(|e| e.to_string())?;
                    let id = p.pps_id as usize;
                    self.pps[id] = Some(p);
                }
                _ => {}
            }
        }

        // 2. slice headers
        let first_vcl = nals
            .iter()
            .find(|n| n.is_vcl())
            .ok_or_else(|| "no slice in access unit".to_string())?;
        let pid = parser::slice_pps_id(&first_vcl.rbsp).map_err(|e| e.to_string())? as usize;
        let pps = self.pps[pid].clone().ok_or("missing PPS")?;
        let sps = self.sps[(pps.sps_id as usize) % 32]
            .clone()
            .ok_or("missing SPS")?;
        self.max_num_reorder_frames = sps.max_num_reorder_frames.unwrap_or(16).min(16) as usize;
        let idr = first_vcl.unit_type == parser::NAL_IDR;
        let sh: SliceHeader = parser::parse_slice_header(
            &first_vcl.rbsp,
            &sps,
            &pps,
            first_vcl.unit_type,
            first_vcl.ref_idc,
        )
        .map_err(|e| e.to_string())?;
        let paired_top = self
            .pending_top
            .as_ref()
            .filter(|top| complementary_fields(&top.sh, &sh))
            .cloned();

        // 3. DPB state
        if self.dpb.max_size != sps.max_num_ref_frames.clamp(1, 16) as usize {
            self.dpb = Dpb::new(sps.max_num_ref_frames, 0);
            self.pending_top = None;
        }
        if idr && paired_top.is_none() {
            self.dpb.clear();
            self.frames = (0..17).map(|_| None).collect();
            self.nv12_pool.clear();
            self.colmv_pool.clear();
        }
        let fno = self.dpb.frame_num_offset(&sh, &sps, idr, false);
        let order = self.dpb.compute_picture_order(&sh, &sps, idr, fno);
        let poc = order.frame;
        self.dpb.set_frame_num_offset(fno);
        let reference_error = (paired_top.is_none() && self.dpb.frame_num_gap(&sh, &sps))
            || self
                .dpb
                .reference_list_error(&sh, poc, sps.log2_max_frame_num);
        // 4. buffers. A fresh buffer for every IDR: reusing one that was a
        // reference makes the hardware's first (reference-less) frame come out
        // slightly wrong.
        let slot = if let Some(top) = &paired_top {
            top.slot
        } else if idr {
            0
        } else {
            // Reuse a buffer no DPB entry points at any more; otherwise grow.
            // A buffer that still backs a reference must never be the output.
            let live: Vec<usize> = self.dpb.frames().iter().map(|f| f.buf).collect();
            match (0..self.frames.len()).find(|b| !live.contains(b) && self.frames[*b].is_some()) {
                Some(b) => {
                    if self.output_dmabuf
                        && self.frames[b]
                            .as_ref()
                            .and_then(|frame| frame.nv12.refs())
                            .is_some_and(|refs| refs > 1)
                    {
                        // A reordered Picture still owns this surface. Drop the
                        // decoder slot's reference and acquire a different
                        // pooled buffer instead of overwriting display data.
                        self.frames[b] = None;
                    }
                    b
                }
                None => match self.frames.iter().position(|f| f.is_none()) {
                    Some(b) => b,
                    None => {
                        self.frames.push(None);
                        self.frames.len() - 1
                    }
                },
            }
        };
        self.ensure_frame(slot, &sps).map_err(|e| e.to_string())?;
        // rkvdec indexes its reference-base and POC tables by the compact
        // reference-table row (short-term rows first, then long-term rows).
        // RPS list entries must therefore contain table rows, not FrameStore
        // slot ids.
        let reference_table = self.dpb.reference_table();
        let dpb_slots: Vec<Option<&FrameStore>> =
            (0..16).map(|i| reference_table.get(i).copied()).collect();
        let mut ref_refs: Vec<Option<&Frame>> = vec![None; 16];
        let mut ref_pocs: Vec<i32> = vec![0; 32];
        let mut ref_flags: Vec<Option<ReferenceFlags>> = vec![None; 16];
        let mut ref_frame_nums = vec![0u32; 16];
        for (row, entry) in dpb_slots.iter().enumerate() {
            let Some(f) = entry else { continue };
            ref_refs[row] = self.frames.get(f.buf).and_then(|fr| fr.as_ref());
            ref_pocs[2 * row] = f.top_poc;
            ref_pocs[2 * row + 1] = f.bottom_poc;
            ref_flags[row] = Some(f.reference_flags());
            ref_frame_nums[row] = f.frame_num;
        }

        // 5. submit. MPP feeds the slice NALs *with* their start codes
        // (verified against the reference decoder: the first bytes it hands to
        // the hardware are `00 00 01 65 ...`), parameter sets excluded.
        self.payload.clear();
        build_slice_payload(aub, &nals, &mut self.payload);
        let payload_len = self.payload.len();
        if payload_len == 0 {
            return Ok(None);
        }
        if std::env::var("RFI_STREAM_DUMP").is_ok() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/our_streams.bin")
            {
                let _ = writeln!(f, "TASK {} {}", order.top, payload_len);
                let _ = f.write_all(&self.payload);
            }
        }
        load_stream_payload(&mut self.stream, &self.payload).map_err(|e| e.to_string())?;

        let table_list = |list: &[RefListEntry]| {
            list.iter()
                .filter_map(|entry| {
                    reference_table
                        .iter()
                        .position(|f| f.slot == entry.slot)
                        .map(|row| RefListEntry {
                            slot: row as u8,
                            bottom: entry.bottom,
                        })
                })
                .collect::<Vec<RefListEntry>>()
        };
        let p_list = self
            .dpb
            .build_p_ref_list_entries(&sh, sps.log2_max_frame_num);
        let (b_list0, b_list1) =
            self.dpb
                .build_auxiliary_b_ref_list_entries(&sh, poc, sps.log2_max_frame_num);
        let table_p = table_list(&p_list);
        let table_l0 = table_list(&b_list0);
        let table_l1 = table_list(&b_list1);
        let lists = hardware_rps_lists(&sh, &table_p, &table_l0, &table_l1);
        if std::env::var_os("RFI_LIST_TRACE").is_some() {
            let rows: Vec<String> = reference_table
                .iter()
                .enumerate()
                .map(|(row, f)| {
                    format!(
                        "{row}:fn{}:poc{}/{}:slot{}:field{}/top{}/bottom{}",
                        f.frame_num,
                        f.top_poc,
                        f.bottom_poc,
                        f.slot,
                        f.field_coded,
                        f.top_present,
                        f.bottom_present,
                    )
                })
                .collect();
            let fmt = |name: &str, list: &[RefListEntry]| {
                format!(
                    "{name}=[{}]",
                    list.iter()
                        .map(|e| format!("{}:{}", e.slot, if e.bottom { "B" } else { "T" }))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            };
            eprintln!(
                "LIST fn={} bottom={} rows=[{}] {} {} {}",
                sh.frame_num,
                sh.bottom_field,
                rows.join(","),
                fmt("p", &table_p),
                fmt("b0", &table_l0),
                fmt("b1", &table_l1),
            );
        }
        let cur = self.frames[slot].as_ref().unwrap();
        let err_info = finish_hardware_job(reference_error, || {
            self.hw.decode(
                &sps,
                &pps,
                &sh,
                order,
                &dpb_slots,
                lists,
                cur,
                &ref_refs,
                &ref_pocs,
                &ref_flags,
                &ref_frame_nums,
                self.stream.as_ref().expect("stream was just loaded"),
                payload_len,
                idr,
            )?;
            Ok(self.hw.last_hardware_error())
        })?;
        if std::env::var_os("RFI_FIELD_TRACE").is_some() {
            eprintln!(
                "FIELD fn={} field={} bottom={} top={} bottom_poc={} frame_poc={} slot={} paired_top={}",
                sh.frame_num,
                sh.field_pic,
                sh.bottom_field,
                order.top,
                order.bottom,
                order.frame,
                slot,
                paired_top.is_some(),
            );
        }
        if std::env::var_os("RFI_FIELD_DUMP").is_some() {
            use std::io::Write;

            let field = if sh.field_pic {
                if sh.bottom_field {
                    'B'
                } else {
                    'T'
                }
            } else {
                'F'
            };
            let path = format!("/tmp/rust_field_{:03}_{field}.nv12", order.frame);
            let view = cur
                .nv12
                .data()
                .map_err(|error| format!("H.264 field dump pool: {error}"))?;
            let _ = view.buffer().sync_read_begin();
            if let Ok(mut file) = std::fs::File::create(path) {
                let _ = file.write_all(&view);
            }
            let _ = view.buffer().sync_read_end();
        }

        // 6. mark the DPB. Complementary fields occupy one frame store; the
        // bottom field replaces the top-only entry after completing the frame.
        let is_top_field = sh.field_pic && !sh.bottom_field;
        let is_bottom_field = sh.field_pic && sh.bottom_field;
        let current_reference = first_vcl.ref_idc != 0;
        let top_reference = paired_top
            .as_ref()
            .map(|top| top.sh.nal_ref_idc != 0)
            .unwrap_or(false);
        let top_err_info = paired_top.as_ref().map(|top| top.err_info).unwrap_or(0);
        let combined_err_info = err_info | top_err_info;
        if let Some(top) = &paired_top {
            if top_reference || current_reference {
                if !top_reference && current_reference {
                    self.dpb
                        .prepare_reference_insertion(sh.adaptive_ref_pic_buffering);
                }
                self.dpb.push(FrameStore {
                    frame_num: sh.frame_num,
                    poc: top.poc.min(poc),
                    top_poc: top.order.top,
                    bottom_poc: order.bottom,
                    long_term: (idr && top.sh.long_term_reference)
                        || (idr && sh.long_term_reference),
                    long_term_frame_idx: 0,
                    slot: slot as u8,
                    buf: slot,
                    referenced: top_reference || current_reference,
                    colmv_valid: true,
                    field_coded: true,
                    top_present: true,
                    bottom_present: true,
                    top_used: top_reference,
                    bottom_used: current_reference,
                    error: combined_err_info,
                });
            }
        } else if current_reference {
            self.dpb
                .prepare_reference_insertion(sh.adaptive_ref_pic_buffering);
            let reference = current_reference_state(sh.field_pic, is_bottom_field);
            self.dpb.push(FrameStore {
                frame_num: sh.frame_num,
                poc,
                top_poc: order.top,
                bottom_poc: order.bottom,
                long_term: idr && sh.long_term_reference,
                long_term_frame_idx: 0,
                slot: slot as u8,
                buf: slot,
                referenced: true,
                // Compression is disabled for MBAFF, but the uncompressed
                // colmv buffer is still valid for later motion prediction.
                colmv_valid: reference.colmv_valid,
                field_coded: reference.field_coded,
                top_present: reference.top_used,
                bottom_present: reference.bottom_used,
                top_used: reference.top_used,
                bottom_used: reference.bottom_used,
                error: err_info,
            });
        }
        if current_reference && !sh.mmco.is_empty() {
            self.dpb
                .apply_mmco(&sh.mmco, sh.frame_num, 0, 1u32 << sps.log2_max_frame_num);
        }
        self.dpb.update_picture_order_state(&sh, &sps, order);
        if is_top_field {
            self.pending_top = Some(PendingTopField {
                sh: sh.clone(),
                order,
                poc,
                slot,
                idr,
                err_info,
            });
        } else {
            self.pending_top = None;
        }
        if is_top_field {
            return Ok(None);
        }
        let output_poc = paired_top
            .as_ref()
            .map(|top| top.poc.min(poc))
            .unwrap_or(poc);
        let output_idr = paired_top.as_ref().map(|top| top.idr).unwrap_or(idr);

        // 7. copy out (the dma-buf is CPU-cached, so sync before reading).
        // MPP never exposes pixel data for an invalid or hardware-errored job.
        let (yuv, nv12, raw_uv, dmabuf) = if combined_err_info == 0 {
            let frame = self.frames[slot].as_ref().unwrap();
            if self.output_dmabuf {
                let lease = frame
                    .nv12
                    .try_clone()
                    .map_err(|error| format!("H.264 DMA-BUF lease: {error}"))?;
                (
                    Vec::new(),
                    None,
                    Vec::new(),
                    Some(DmaBufPicture::pooled(
                        lease,
                        frame.hor_stride,
                        frame.ver_stride,
                    )),
                )
            } else {
                let view = frame
                    .nv12
                    .data()
                    .map_err(|error| format!("H.264 output pool: {error}"))?;
                let y_size = frame.hor_stride * frame.ver_stride;
                let y = &view[..y_size];
                let uv = &view[y_size..y_size + y_size / 2];
                let _ = view.buffer().sync_read_begin();
                let (yuv, nv12) = if self.output_nv12 {
                    (
                        Vec::new(),
                        Some(crate::codec::yuv::nv12_to_packed(
                            y,
                            uv,
                            self.width,
                            self.height,
                            frame.hor_stride,
                        )),
                    )
                } else {
                    (
                        crate::codec::yuv::nv12_to_planar(
                            y,
                            uv,
                            self.width,
                            self.height,
                            frame.hor_stride,
                        ),
                        None,
                    )
                };
                // The interleaved chroma is only ever dumped by the debug path; the
                // hardware wrote it into the same buffer the planar copy just read.
                let raw_uv: Vec<u8> = if self.capture_raw_uv {
                    (0..self.height / 2)
                        .flat_map(|row| {
                            let off = row * frame.hor_stride;
                            uv[off..off + self.width].to_vec()
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let _ = view.buffer().sync_read_end();
                (yuv, nv12, raw_uv, None)
            }
        } else {
            (Vec::new(), None, Vec::new(), None)
        };
        let output_reference = current_reference || top_reference;
        Ok(Some(Picture {
            width: self.width,
            height: self.height,
            poc: output_poc,
            idr: output_idr,
            yuv,
            nv12,
            dmabuf,
            raw_uv,
            err_info: combined_err_info,
            discard: combined_err_info != 0 && !output_reference,
        }))
    }

    /// VUI display reorder depth for the active SPS.
    pub fn max_num_reorder_frames(&self) -> usize {
        self.max_num_reorder_frames
    }

    /// Select packed NV12 instead of planar I420 output. This removes the
    /// chroma deinterleave pass for NV12-capable display/conversion consumers.
    pub fn set_output_nv12(&mut self, enabled: bool) {
        self.output_nv12 = enabled;
    }

    pub fn set_output_dmabuf(&mut self, enabled: bool) {
        self.output_dmabuf = enabled;
        if enabled {
            self.output_nv12 = true;
        }
    }

    pub fn import_output_dmabuf(&mut self, buffer: DmaBuf) -> Result<(), String> {
        self.set_output_dmabuf(true);
        self.output_injection = Some(buffer);
        Ok(())
    }
}

/// Copy the assembled payload into the staging buffer, reusing and growing
/// it across pictures the way the VP8 decoder's stream buffer does. Zeroing
/// everything past the payload keeps the bytes the hardware reads identical
/// to the fresh zeroed allocation this path used per picture.
fn load_stream_payload(stream: &mut Option<DmaBuf>, payload: &[u8]) -> io::Result<()> {
    let needed = payload.len().max(64);
    if stream.as_ref().is_none_or(|buffer| buffer.len() < needed) {
        *stream = None;
        *stream = Some(DmaBuf::alloc("system", needed, false)?);
    }
    let buffer = stream.as_mut().expect("stream was just created");
    buffer.sync_write_begin()?;
    {
        let window = buffer.as_mut_slice();
        window[..payload.len()].copy_from_slice(payload);
        let padding_end = needed.min(window.len());
        window[payload.len()..padding_end].fill(0);
    }
    buffer.sync_write_end()
}

fn pad_h264_payload(payload: &mut Vec<u8>) {
    payload.resize((payload.len() + 7) & !7, 0);
}

/// Assemble the access unit's VCL slices into the payload MPP hands to the
/// hardware. Appended into caller-owned scratch; the entry sequence is the
/// original per-frame build, byte for byte.
fn build_slice_payload(aub: &[u8], nals: &[parser::NalUnit], payload: &mut Vec<u8>) {
    payload.reserve(aub.len());
    for n in nals.iter().filter(|n| n.is_vcl()) {
        // MPP hands the hardware a normalised 3-byte start code (a 4-byte
        // code loses its first byte) and pads the end with three zero
        // bytes; verified byte-for-byte against its packet dump.
        let code_len = if aub.len() >= n.offset + 4 && aub[n.offset..n.offset + 4] == [0, 0, 0, 1] {
            4
        } else {
            3
        };
        let start = n.offset + (code_len - 3);
        let end = (n.offset + n.len).min(aub.len());
        payload.extend_from_slice(&aub[start..end]);
    }
    payload.extend_from_slice(&[0, 0, 0]);
    pad_h264_payload(payload);
}

fn current_reference_state(field_pic: bool, bottom_field: bool) -> ReferenceFlags {
    ReferenceFlags {
        field_coded: field_pic,
        top_used: !field_pic || !bottom_field,
        bottom_used: !field_pic || bottom_field,
        colmv_valid: true,
    }
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

fn finish_hardware_job<F>(reference_error: bool, job: F) -> Result<u32, String>
where
    F: FnOnce() -> io::Result<bool>,
{
    if reference_error {
        return Ok(FRAME_ERR_DEC_INVALID);
    }
    let hardware_error = job().map_err(|e| e.to_string())?;
    Ok(frame_error(false, hardware_error))
}

fn hardware_rps_lists<'a>(
    sh: &SliceHeader,
    table_p: &'a [RefListEntry],
    table_l0: &'a [RefListEntry],
    table_l1: &'a [RefListEntry],
) -> [&'a [RefListEntry]; 3] {
    // The rkvdec RPS carries MPP's P list first, followed by its auxiliary
    // B lists 0 and 1. MPP materialises those B lists for every picture kind.
    if sh.is_b() || sh.is_p() {
        [table_p, table_l0, table_l1]
    } else {
        [&[], &[], &[]]
    }
}

/// Two field pictures form one complementary frame only when they describe
/// the top and bottom halves of the same coded frame.
fn complementary_fields(top: &SliceHeader, bottom: &SliceHeader) -> bool {
    top.field_pic
        && !top.bottom_field
        && bottom.field_pic
        && bottom.bottom_field
        && top.frame_num == bottom.frame_num
        && (top.nal_ref_idc != 0) == (bottom.nal_ref_idc != 0)
}

/// Whether MPP would deliver this picture to an application. Error frames are
/// retained internally only when needed for DPB error propagation.
pub fn picture_is_output(picture: &Picture) -> bool {
    picture.err_info == 0 && !picture.discard
}

/// Sort decoded pictures by POC within each IDR epoch.
///
/// This is the offline equivalent of the decoder's display reorder buffer.
/// Keeping IDR boundaries separate prevents a reset POC from being compared
/// with the previous POC epoch.
pub fn sort_pictures_by_poc(pictures: &mut [Picture]) {
    let mut start = 0;
    for i in 0..pictures.len() {
        if i > start && pictures[i].idr {
            pictures[start..i].sort_by_key(|picture| picture.poc);
            start = i;
        }
    }
    pictures[start..].sort_by_key(|picture| picture.poc);
}

/// Split an Annex-B stream into access units at picture boundaries.
///
/// A new picture begins at a VCL NAL with `first_mb_in_slice == 0` or at an
/// access-unit delimiter. Parameter sets and SEI before the first slice stay
/// attached to it.
pub fn split_access_units(data: &[u8]) -> Vec<&[u8]> {
    let nals = parser::split_annex_b(data);
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut has_vcl = false;
    let mut trailing_non_vcl: Option<usize> = None;
    for n in &nals {
        let boundary = match n.unit_type {
            parser::NAL_AUD => true,
            1..=5 => first_mb_zero(&n.rbsp),
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

fn first_mb_zero(rbsp: &[u8]) -> bool {
    // first_mb_in_slice is ue(v); 0 <=> the first bit is 1.
    matches!(rbsp.first(), Some(b) if *b & 0x80 != 0)
}

#[cfg(test)]
mod tests {
    use super::{
        build_slice_payload, complementary_fields, current_reference_state, finish_hardware_job,
        frame_error, hardware_rps_lists, pad_h264_payload, picture_is_output, sort_pictures_by_poc,
        split_access_units, DisplayReorder, Picture, FRAME_ERR_DEC_HW_ERR, FRAME_ERR_DEC_INVALID,
    };
    use crate::codec::h264::dpb::{RefListEntry, ReferenceFlags};
    use crate::codec::h264::parser::{split_annex_b, SliceHeader};

    fn picture(poc: i32, idr: bool) -> Picture {
        Picture {
            width: 1,
            height: 1,
            poc,
            idr,
            yuv: Vec::new(),
            nv12: None,
            dmabuf: None,
            raw_uv: Vec::new(),
            err_info: 0,
            discard: false,
        }
    }

    #[test]
    fn multi_slice_picture_stays_in_one_access_unit() {
        // Two VCL NALs of the same picture: first_mb 0 then first_mb > 0.
        let mut data = vec![0u8, 0, 0, 1, 0x65, 0x88];
        data.extend_from_slice(&[0, 0, 0, 1, 0x65, 0x44]);
        let aus = split_access_units(&data);
        assert_eq!(aus.len(), 1);
    }

    #[test]
    fn new_picture_starts_a_new_access_unit() {
        let mut data = vec![0u8, 0, 0, 1, 0x65, 0x88];
        data.extend_from_slice(&[0, 0, 0, 1, 0x41, 0x88]);
        let aus = split_access_units(&data);
        assert_eq!(aus.len(), 2);
    }

    #[test]
    fn parameter_sets_between_pictures_join_the_next_access_unit() {
        let mut data = vec![0u8, 0, 0, 1, 0x67, 0x42];
        data.extend_from_slice(&[0, 0, 0, 1, 0x65, 0x80]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x68, 0x00]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x41, 0x80]);

        let aus = split_access_units(&data);
        assert_eq!(aus.len(), 2);
        assert_eq!(&aus[1][4..5], &[0x68]);
    }

    #[test]
    fn picture_output_is_sorted_by_poc() {
        let mut pictures = [
            picture(0, true),
            picture(6, false),
            picture(2, false),
            picture(4, false),
        ];
        sort_pictures_by_poc(&mut pictures);
        assert_eq!(pictures.map(|picture| picture.poc), [0, 2, 4, 6]);
    }

    #[test]
    fn streaming_reorder_is_bounded_and_display_ordered() {
        let mut reorder = DisplayReorder::new(2);
        let mut emitted = Vec::new();
        for (poc, idr) in [(0, true), (2, false), (1, false), (4, false), (3, false)] {
            reorder.push(picture(poc, idr), |picture| emitted.push(picture.poc));
            assert!(reorder.pending.len() <= 2, "reorder exceeded its bound");
        }
        reorder.flush(|picture| emitted.push(picture.poc));
        assert_eq!(emitted, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn streaming_reorder_flushes_the_previous_idr_epoch() {
        let mut reorder = DisplayReorder::new(2);
        let mut emitted = Vec::new();
        reorder.push(picture(10, true), |picture| emitted.push(picture.poc));
        reorder.push(picture(12, false), |picture| emitted.push(picture.poc));
        reorder.push(picture(0, true), |picture| emitted.push(picture.poc));
        reorder.flush(|picture| emitted.push(picture.poc));
        assert_eq!(emitted, [10, 12, 0]);
    }

    #[test]
    fn idr_resets_the_display_order_epoch() {
        let mut pictures = [
            picture(0, true),
            picture(4, false),
            picture(2, false),
            picture(0, true),
            picture(4, false),
            picture(2, false),
        ];
        sort_pictures_by_poc(&mut pictures);
        assert_eq!(
            pictures
                .iter()
                .map(|picture| picture.poc)
                .collect::<Vec<_>>(),
            [0, 2, 4, 0, 2, 4]
        );
        assert_eq!(
            pictures
                .iter()
                .map(|picture| picture.idr)
                .collect::<Vec<_>>(),
            [true, false, false, true, false, false]
        );
    }

    #[test]
    fn complementary_fields_share_one_picture() {
        let top = SliceHeader {
            frame_num: 7,
            field_pic: true,
            bottom_field: false,
            ..Default::default()
        };
        let bottom = SliceHeader {
            frame_num: 7,
            field_pic: true,
            bottom_field: true,
            ..Default::default()
        };

        assert!(complementary_fields(&top, &bottom));
        assert!(!complementary_fields(&bottom, &top));
    }

    #[test]
    fn different_frame_numbers_are_not_a_field_pair() {
        let top = SliceHeader {
            frame_num: 7,
            field_pic: true,
            bottom_field: false,
            ..Default::default()
        };
        let bottom = SliceHeader {
            frame_num: 8,
            field_pic: true,
            bottom_field: true,
            ..Default::default()
        };

        assert!(!complementary_fields(&top, &bottom));
    }

    #[test]
    fn complementary_reference_fields_do_not_require_equal_ref_idc() {
        let top = SliceHeader {
            frame_num: 0,
            field_pic: true,
            bottom_field: false,
            nal_ref_idc: 3,
            idr_pic_id: Some(0),
            ..Default::default()
        };
        let bottom = SliceHeader {
            frame_num: 0,
            field_pic: true,
            bottom_field: true,
            nal_ref_idc: 1,
            idr_pic_id: None,
            ..Default::default()
        };

        assert!(complementary_fields(&top, &bottom));
    }

    #[test]
    fn full_frame_reference_uses_both_fields() {
        assert_eq!(
            current_reference_state(false, false),
            ReferenceFlags {
                field_coded: false,
                top_used: true,
                bottom_used: true,
                colmv_valid: true,
            }
        );
    }

    #[test]
    fn hardware_payload_is_eight_byte_aligned() {
        let mut payload = vec![0x41; 197];
        pad_h264_payload(&mut payload);
        assert_eq!(payload.len(), 200);
        assert_eq!(&payload[197..], &[0, 0, 0]);
    }

    /// The reused-scratch payload builder must emit exactly the bytes the
    /// original per-frame inline build emitted: VCL NALs only, 4-byte start
    /// codes normalised to 3, the three-zero tail and the 8-byte pad.
    #[test]
    fn slice_payload_matches_the_original_inline_build() {
        // SPS (non-VCL), an IDR slice behind a 4-byte start code, and a
        // non-IDR slice behind a 3-byte start code.
        let aub: Vec<u8> = [
            0u8, 0, 0, 1, 0x67, 0xaa, 0xbb, // SPS, must not reach hardware
            0, 0, 0, 1, 0x65, 0x88, 0x99, // IDR with a 4-byte start code
            0, 0, 1, 0x41, 0x8a, 0xb0, 0xc1, 0xd2, // non-IDR, 3-byte code
        ]
        .to_vec();
        let nals = split_annex_b(&aub);

        // The original sequence, spelled out exactly as the pre-optimisation
        // decode_access_unit() built it into a fresh Vec.
        let mut expected: Vec<u8> = Vec::with_capacity(aub.len());
        for n in nals.iter().filter(|n| n.is_vcl()) {
            let code_len =
                if aub.len() >= n.offset + 4 && aub[n.offset..n.offset + 4] == [0, 0, 0, 1] {
                    4
                } else {
                    3
                };
            let start = n.offset + (code_len - 3);
            let end = (n.offset + n.len).min(aub.len());
            expected.extend_from_slice(&aub[start..end]);
        }
        expected.extend_from_slice(&[0, 0, 0]);
        pad_h264_payload(&mut expected);

        let mut got = vec![0u8]; // dirty scratch: reserve-extend must not mix
                                 // in the old contents; the caller clears first
        got.clear();
        build_slice_payload(&aub, &nals, &mut got);
        assert_eq!(got, expected);
        assert_eq!(&got[..4], &[0, 0, 1, 0x65], "the 4-byte code is normalised");
        assert_eq!(
            got.iter().filter(|b| **b == 0x67).count(),
            0,
            "the SPS never reaches the hardware"
        );
    }

    #[test]
    fn p_slice_uses_auxiliary_b_lists_for_rps_lists_one_and_two() {
        let sh = SliceHeader {
            slice_type: 0,
            ..Default::default()
        };
        let entry = |slot| RefListEntry {
            slot,
            bottom: false,
        };
        let p = [entry(3), entry(1), entry(0)];
        let l0 = [entry(2), entry(1)];
        let l1 = [entry(0), entry(2)];

        assert_eq!(
            hardware_rps_lists(&sh, &p, &l0, &l1),
            [&p[..], &l0[..], &l1[..]]
        );
    }

    #[test]
    fn reference_errors_use_dec_invalid_and_hardware_errors_use_dec_hw_err() {
        assert_eq!(frame_error(true, true), FRAME_ERR_DEC_INVALID);
        assert_eq!(frame_error(false, true), FRAME_ERR_DEC_HW_ERR);
        assert_eq!(frame_error(false, false), 0);
    }

    #[test]
    fn hardware_job_reports_the_status_returned_after_submission() {
        assert_eq!(
            finish_hardware_job(false, || Ok(true)).unwrap(),
            FRAME_ERR_DEC_HW_ERR
        );
    }

    #[test]
    fn errored_or_discarded_pictures_are_not_output() {
        let mut picture = picture(0, true);
        assert!(picture_is_output(&picture));
        picture.err_info = FRAME_ERR_DEC_INVALID;
        assert!(!picture_is_output(&picture));
        picture.err_info = 0;
        picture.discard = true;
        assert!(!picture_is_output(&picture));
    }
}
