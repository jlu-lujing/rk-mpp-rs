//! End-to-end H.264 decoder: Annex-B in, NV12 out, rkvdec underneath.

use std::io;

use crate::hal::h264d::{Frame, H264d};
use crate::service::dma_heap::DmaBuf;

use super::dpb::{Dpb, FrameStore};
use super::parser::{self, Pps, SliceHeader, Sps};

/// One decoded picture, cropped to the display size.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// Planar YUV 4:2:0, tightly packed.
    pub yuv: Vec<u8>,
    /// Raw interleaved chroma as the hardware wrote it (stride = width).
    pub raw_uv: Vec<u8>,
}

pub struct Decoder {
    sps: Vec<Option<Sps>>,
    pps: Vec<Option<Pps>>,
    dpb: Dpb,
    hw: H264d,
    frames: Vec<Option<Frame>>,
    width: usize,
    height: usize,
}

impl Decoder {
    pub fn new() -> io::Result<Decoder> {
        Ok(Decoder {
            sps: (0..32).map(|_| None).collect(),
            pps: (0..256).map(|_| None).collect(),
            dpb: Dpb::new(1, 0),
            hw: H264d::open()?,
            frames: (0..17).map(|_| None).collect(),
            width: 0,
            height: 0,
        })
    }

    /// Register read-back after the last job: (reg001, reg045, reg076).
    pub fn status(&self) -> (u32, u32, u32) {
        self.hw.last_status
    }

    fn ensure_frame(&mut self, slot: usize) -> io::Result<()> {
        if self.frames[slot].is_none() {
            self.frames[slot] = Some(Frame::alloc(self.width, self.height)?);
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
                    if self.width != s.width as usize || self.height != s.height as usize {
                        self.width = s.width as usize;
                        self.height = s.height as usize;
                        self.frames = (0..17).map(|_| None).collect();
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
        let sps = self.sps[(pps.sps_id as usize) % 32].clone().ok_or("missing SPS")?;
        let idr = first_vcl.unit_type == parser::NAL_IDR;
        let sh: SliceHeader = parser::parse_slice_header(
            &first_vcl.rbsp,
            &sps,
            &pps,
            first_vcl.unit_type,
            first_vcl.ref_idc,
        )
        .map_err(|e| e.to_string())?;

        // 3. DPB state
        if self.dpb.max_size != (sps.max_num_ref_frames as usize + 1) {
            self.dpb = Dpb::new(sps.max_num_ref_frames, 0);
        }
        if idr {
            self.dpb.clear();
        }
        let fno = self.dpb.frame_num_offset(&sh, &sps, idr, false);
        let poc = self.dpb.compute_poc(&sh, &sps, idr, fno);
        self.dpb.set_frame_num_offset(fno);
        let (l0, l1) = self.dpb.build_ref_lists(&sh, poc);

        // 4. buffers
        let slot = if idr {
            0
        } else {
            self.frames
                .iter()
                .position(|f| f.is_none())
                .unwrap_or((poc.rem_euclid(17)) as usize)
        };
        self.ensure_frame(slot).map_err(|e| e.to_string())?;
        let ref_refs: Vec<Option<&Frame>> = self
            .dpb
            .frames()
            .iter()
            .take(16)
            .map(|e| self.frames.get(e.buf).and_then(|f| f.as_ref()))
            .collect();

        // 5. submit. MPP feeds the slice NALs *with* their start codes
        // (verified against the reference decoder: the first bytes it hands to
        // the hardware are `00 00 01 65 ...`), parameter sets excluded.
        let mut payload: Vec<u8> = Vec::with_capacity(aub.len());
        for n in nals.iter().filter(|n| n.is_vcl()) {
            let end = (n.offset + n.len).min(aub.len());
            payload.extend_from_slice(&aub[n.offset..end]);
        }
        if payload.is_empty() {
            return Ok(None);
        }
        let mut stream =
            DmaBuf::alloc("system", payload.len().max(64), false).map_err(|e| e.to_string())?;
        stream.as_mut_slice()[..payload.len()].copy_from_slice(&payload);

        let dpb_slots: Vec<Option<&FrameStore>> = (0..16)
            .map(|i| self.dpb.frames().get(i))
            .collect();
        let lists: [&[u8]; 3] = [&l0, &l1, &[]];
        let cur = self.frames[slot].as_ref().unwrap();
        self.hw
            .decode(
                &sps,
                &pps,
                &sh,
                &dpb_slots,
                lists,
                cur,
                &ref_refs,
                &stream,
                idr,
            )
            .map_err(|e| e.to_string())?;

        // 6. mark the DPB
        if first_vcl.ref_idc != 0 {
            if self.dpb.frames().len() >= self.dpb.max_size {
                self.dpb.sliding_window();
            }
            self.dpb.push(FrameStore {
                frame_num: sh.frame_num,
                poc,
                long_term: idr && sh.long_term_reference,
                long_term_frame_idx: 0,
                slot: slot as u8,
                buf: slot,
                referenced: true,
            });
            if !sh.mmco.is_empty() {
                self.dpb.apply_mmco(&sh.mmco, sh.frame_num, 0);
            }
            self.dpb.update_poc_state(&sh, &sps, poc);
        }

        // 7. copy out (the dma-buf is CPU-cached, so sync before reading)
        let frame = self.frames[slot].as_ref().unwrap();
        let _ = frame.nv12.sync_read_begin();
        let y = frame.y_plane();
        let uv = frame.uv_plane();
        let mut yuv = Vec::with_capacity(self.width * self.height * 3 / 2);
        for row in 0..self.height {
            let off = row * frame.hor_stride;
            yuv.extend_from_slice(&y[off..off + self.width]);
        }
        // The hardware writes NV12 (interleaved U/V); the output is planar
        // yuv420p, so de-interleave into a full U plane followed by a full V
        // plane (per-row interleaving would mix the planes).
        let pairs = self.width / 2;
        for row in 0..self.height / 2 {
            let off = row * frame.hor_stride;
            for j in 0..pairs {
                yuv.push(uv[off + 2 * j]);
            }
        }
        for row in 0..self.height / 2 {
            let off = row * frame.hor_stride;
            for j in 0..pairs {
                yuv.push(uv[off + 2 * j + 1]);
            }
        }
        let raw_uv: Vec<u8> = (0..self.height / 2)
            .flat_map(|row| {
                let off = row * frame.hor_stride;
                uv[off..off + self.width].to_vec()
            })
            .collect();
        Ok(Some(Picture {
            width: self.width,
            height: self.height,
            yuv,
            raw_uv,
        }))
    }
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
    for n in &nals {
        let boundary = match n.unit_type {
            parser::NAL_AUD => true,
            1..=5 => first_mb_zero(&n.rbsp),
            _ => false,
        };
        if boundary && has_vcl {
            out.push(&data[start.unwrap()..n.offset]);
            start = Some(n.offset);
            has_vcl = n.is_vcl();
            continue;
        }
        if start.is_none() {
            start = Some(n.offset);
        }
        if n.is_vcl() {
            has_vcl = true;
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


