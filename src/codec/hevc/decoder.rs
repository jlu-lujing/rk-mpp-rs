//! End-to-end HEVC decoder: Annex-B in, planar yuv420p out, rkvdec underneath.

use std::io;

use crate::hal::hevc::{Frame, HevcHw};
use crate::service::dma_heap::DmaBuf;

use super::dpb::{Dpb, FrameStore};
use super::parser::{self, Pps, SliceHeader, Sps};

pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub yuv: Vec<u8>,
}

pub struct Decoder {
    sps: Vec<Option<Sps>>,
    pps: Vec<Option<Pps>>,
    dpb: Dpb,
    hw: HevcHw,
    frames: Vec<Option<Frame>>,
    width: usize,
    height: usize,
    ctu_size: usize,
}

impl Decoder {
    pub fn new() -> io::Result<Decoder> {
        Ok(Decoder {
            sps: (0..16).map(|_| None).collect(),
            pps: (0..64).map(|_| None).collect(),
            dpb: Dpb::new(),
            hw: HevcHw::open()?,
            frames: (0..17).map(|_| None).collect(),
            width: 0,
            height: 0,
            ctu_size: 16,
        })
    }

    fn ensure_frame(&mut self, slot: usize) -> io::Result<()> {
        if self.frames[slot].is_none() {
            self.frames[slot] = Some(Frame::alloc(self.width, self.height, self.ctu_size)?);
        }
        Ok(())
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
                    if self.width != s.width as usize || self.height != s.height as usize {
                        self.width = s.width as usize;
                        self.height = s.height as usize;
                        self.frames = (0..17).map(|_| None).collect();
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
        // The slice header needs the PPS id, which is parsed from the header
        // itself; HEVC puts it after the two IRAP flags.
        let pid = slice_pps_id(&first.rbsp, first.unit_type).map_err(|e| e.to_string())?;
        let pps = self
            .pps
            .get(pid as usize)
            .and_then(|p| p.clone())
            .ok_or("missing PPS")?;
        let sps = self
            .sps
            .get((pps.sps_id as usize) % 16)
            .and_then(|s| s.clone())
            .ok_or("missing SPS")?;
        let mut sh = parser::parse_slice_header(&first.rbsp, &sps, &pps, first.unit_type)
            .map_err(|e| e.to_string())?;
        if slices.len() > 1 {
            // Aggregate the inline RPS of later slices is not needed for the
            // hardware tables, but the slice count is.
            sh.first_slice = true;
        }

        let is_irap = first.is_irap();
        let no_rasl = first.unit_type == parser::NAL_IDR_W_RADL
            || first.unit_type == parser::NAL_IDR_N_LP
            || (21..=23).contains(&first.unit_type);
        let poc = self.dpb.compute_poc(&sh, &sps, no_rasl);

        // Output buffer: fresh per picture, never reused (learned from H.264).
        let slot = if is_irap {
            self.frames.push(None);
            self.frames.len() - 1
        } else {
            match self.frames.iter().position(|f| f.is_none()) {
                Some(b) => b,
                None => {
                    self.frames.push(None);
                    self.frames.len() - 1
                }
            }
        };
        self.ensure_frame(slot).map_err(|e| e.to_string())?;

        let l0 = self.dpb.build_ref_list(&sh.rps, poc);
        let mut refs: Vec<Option<&Frame>> = vec![None; 16];
        let mut ref_pocs: Vec<i32> = vec![0; 16];
        for (i, &s) in l0.iter().enumerate().take(16) {
            if let Some(f) = self.dpb.frames().iter().find(|f| f.slot == s) {
                refs[i] = self.frames.get(f.buf).and_then(|x| x.as_ref());
                ref_pocs[i] = f.poc;
            }
        }

        if std::env::var("RFI_HEVC_LISTS").is_ok() {
            eprintln!(
                "AU type={} poc={} rps s0={:?} s1={:?} l0={:?} dpb={:?}",
                first.unit_type,
                poc,
                sh.rps.s0,
                sh.rps.s1,
                l0,
                self.dpb.frames().iter().map(|f| (f.slot, f.poc)).collect::<Vec<_>>()
            );
        }

        // The decoder reads from the first byte, so the access unit is fed from
        // the first NAL header (start code stripped) like the H.264 path.
        let mut payload: Vec<u8> = Vec::with_capacity(aub.len());
        for n in slices.iter() {
            let code_len = if aub.len() >= n.offset + 4 && aub[n.offset..n.offset + 4] == [0, 0, 0, 1]
            {
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
        stream.as_mut_slice()[..payload.len()].copy_from_slice(&payload);

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
                &stream,
                raw_len,
                slice_count,
                first.unit_type,
            )
            .map_err(|e| e.to_string())?;

        if !sh.dependent {
            // Bound memory by the SPS picture-buffering budget instead of the
            // current L0: a picture missing from this frame's list may still be
            // needed by a later RPS.
            self.dpb
                .trim(sps.max_dec_pic_buffering as usize + 2);
            self.dpb.push(FrameStore {
                poc,
                slot: slot as u8,
                buf: slot,
                referenced: true,
            });
        }

        let frame = self.frames[slot].as_ref().unwrap();
        let _ = frame.nv12.sync_read_begin();
        let y = frame.y_plane();
        let uv = frame.uv_plane();
        let mut yuv = Vec::with_capacity(self.width * self.height * 3 / 2);
        for row in 0..self.height {
            let off = row * frame.hor_stride;
            yuv.extend_from_slice(&y[off..off + self.width]);
        }
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
        let _ = frame.nv12.sync_read_end();
        Ok(Some(Picture {
            width: self.width,
            height: self.height,
            yuv,
        }))
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
    for n in &nals {
        let boundary = match n.unit_type {
            parser::NAL_AUD => true,
            t if t <= 31 => first_slice(&n.rbsp),
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

fn first_slice(rbsp: &[u8]) -> bool {
    matches!(rbsp.first(), Some(b) if *b & 0x80 != 0)
}
