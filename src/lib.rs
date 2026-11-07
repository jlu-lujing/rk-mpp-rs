//! rk-mpp-rs: pure-Rust hardware JPEG decode for Rockchip SoCs.
//!
//! The C API at the bottom is what `rknn-fastinfer` links against; the modules
//! are usable directly from Rust.

pub mod codec;
pub mod hal;
pub mod reference;
pub mod scale;
pub mod service;

use std::ffi::{c_char, CStr};

use codec::jpeg::parser as jpeg;
use codec::jpeg::regs as jpeg_regs;
use codec::jpeg::tables;
use hal::jpegd as decode;
use service::dma_heap::DmaBuf;
use service::{MppService, VPU_CLIENT_JPEG_DEC};

/// Decoder state kept across frames (buffers, tables, scaler).
pub struct Decoder {
    svc: MppService,
    /// Kept alive because its fd is programmed into `regs`.
    _table: DmaBuf,
    /// Kept alive because its fd is programmed into `regs`.
    _stream: DmaBuf,
    output: DmaBuf,
    scaler: scale::Scaler,
    info: jpeg::JpegInfo,
    regs: jpeg_regs::Regs,
    geom: decode::Geometry,
    stream_len: usize,
    dst_w: usize,
    dst_h: usize,
}

impl Decoder {
    pub fn open(path: &str, dst_w: usize, dst_h: usize) -> Result<Decoder, String> {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        let info = jpeg::parse(&data).map_err(|e| e.to_string())?;
        let tables = tables::Tables::build(&info).map_err(|e| format!("{e:?}"))?;

        let svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;
        let mut table =
            DmaBuf::alloc("system", tables::RKD_TABLE_SIZE, false).map_err(|e| e.to_string())?;
        table.as_mut_slice()[..tables::RKD_TABLE_SIZE].copy_from_slice(tables.as_slice());
        let mut stream = DmaBuf::alloc("system", data.len(), false).map_err(|e| e.to_string())?;
        stream.as_mut_slice()[..data.len()].copy_from_slice(&data);

        let output_geometry = decode::geometry(&info, true, decode::JpegDownscale::Disabled)
            .map_err(|e| e.to_string())?;
        let output = DmaBuf::alloc("system", output_geometry.frame_size, false)
            .map_err(|e| e.to_string())?;

        let (regs, geom) = decode::build(&decode::Job {
            info: &info,
            table_fd: table.fd(),
            stream_fd: stream.fd(),
            stream_len: data.len(),
            output_fd: output.fd(),
            rgb: true,
            downscale: decode::JpegDownscale::Disabled,
        })
        .map_err(|e| e.to_string())?;

        Ok(Decoder {
            svc,
            _table: table,
            _stream: stream,
            output,
            scaler: scale::Scaler::new(dst_w, dst_h),
            stream_len: data.len(),
            info,
            regs,
            geom,
            dst_w,
            dst_h,
        })
    }

    /// Decode the frame and write the letterboxed RGB888 model input.
    pub fn frame(&mut self, dst: &mut [u8]) -> Result<(i32, i32, f32), String> {
        assert_eq!(dst.len(), self.dst_w * self.dst_h * 3);
        self.svc
            .submit_regs_recovering(&self.regs.words)
            .map_err(|e| format!("submit: {e}"))?;
        self.svc
            .poll_hw_finish_recovering()
            .map_err(|e| format!("poll: {e}"))?;
        self.output
            .sync_read_begin()
            .map_err(|e| format!("dma-buf read sync: {e}"))?;
        let geometry = self.scaler.letterbox_strided(
            self.output.as_slice(),
            self.geom.stride_bytes,
            self.geom.out_width,
            self.geom.out_height,
            dst,
        );
        self.output
            .sync_read_end()
            .map_err(|e| format!("dma-buf read sync end: {e}"))?;
        Ok((geometry.pad_left, geometry.pad_top, geometry.scale))
    }

    pub fn src_size(&self) -> (usize, usize) {
        (self.info.width, self.info.height)
    }

    pub fn stream_len(&self) -> usize {
        self.stream_len
    }
}

/* ------------------------------- C API -------------------------------- */

/// Opaque handle for the C side.
pub struct RkDec(Decoder);

fn jpeg_dst_len(width: usize, height: usize) -> Option<usize> {
    width.checked_mul(height)?.checked_mul(3)
}

/// Open a decoder for `path`, producing `dst_w` x `dst_h` RGB888 frames.
///
/// # Safety
/// `path` must be a NUL-terminated string. `out_w` and `out_h` must each be
/// null or point to writable `u32` storage for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn rk_jpeg_open(
    path: *const c_char,
    dst_w: u32,
    dst_h: u32,
    out_w: *mut u32,
    out_h: *mut u32,
) -> *mut RkDec {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller guarantees `path` is NUL-terminated; the null case
    // was checked above and CStr only borrows until the call returns.
    let p = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };
    match Decoder::open(p, dst_w as usize, dst_h as usize) {
        Ok(d) => {
            if !out_w.is_null() {
                // SAFETY: the caller guarantees non-null out pointers are
                // writable u32 storage.
                *out_w = d.src_size().0 as u32;
            }
            if !out_h.is_null() {
                // SAFETY: same contract as out_w above.
                *out_h = d.src_size().1 as u32;
            }
            Box::into_raw(Box::new(RkDec(d)))
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Decode one frame into `dst` (dst_w * dst_h * 3 bytes, RGB888, padded 114).
///
/// # Safety
/// `h` must come from `rk_jpeg_open` and remain exclusively borrowed for this
/// call. `dst` must address `dst_w * dst_h * 3` writable bytes. `pad_left`,
/// `pad_top` and `scale`, when non-null, must point to writable storage of
/// their declared types.
#[no_mangle]
pub unsafe extern "C" fn rk_jpeg_frame(
    h: *mut RkDec,
    dst: *mut u8,
    pad_left: *mut i32,
    pad_top: *mut i32,
    scale: *mut f32,
) -> i32 {
    if h.is_null() || dst.is_null() {
        return -1;
    }
    // SAFETY: the caller guarantees this is a live, exclusively-borrowed
    // handle returned by rk_jpeg_open.
    let dec = &mut (*h).0;
    let Some(n) = jpeg_dst_len(dec.dst_w, dec.dst_h) else {
        return -1;
    };
    // SAFETY: the caller guarantees dst points to n writable bytes and is not
    // aliased by another live reference for this call.
    let slice = std::slice::from_raw_parts_mut(dst, n);
    match dec.frame(slice) {
        Ok((pl, pt, s)) => {
            if !pad_left.is_null() {
                // SAFETY: non-null output pointers are writable per contract.
                *pad_left = pl;
            }
            if !pad_top.is_null() {
                // SAFETY: same contract as pad_left.
                *pad_top = pt;
            }
            if !scale.is_null() {
                // SAFETY: same contract as pad_left.
                *scale = s;
            }
            0
        }
        Err(_) => -2,
    }
}

/// # Safety
/// `h` must come from `rk_jpeg_open` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn rk_jpeg_close(h: *mut RkDec) {
    if !h.is_null() {
        // SAFETY: the caller transfers ownership of a handle created by
        // rk_jpeg_open and guarantees it is not used again.
        drop(Box::from_raw(h));
    }
}

#[cfg(test)]
mod tests {
    use super::jpeg_dst_len;

    #[test]
    fn c_api_output_size_checks_both_multiplications() {
        assert_eq!(jpeg_dst_len(640, 640), Some(1_228_800));
        assert_eq!(jpeg_dst_len(usize::MAX, 2), None);
        assert_eq!(jpeg_dst_len(usize::MAX / 2, 3), None);
    }
}
