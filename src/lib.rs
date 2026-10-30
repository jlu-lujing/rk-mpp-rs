//! rk-mpp-rs: pure-Rust hardware JPEG decode for Rockchip SoCs.
//!
//! The C API at the bottom is what `rknn-fastinfer` links against; the modules
//! are usable directly from Rust.
//!
//! `swjpeg` is a pure-Rust software decoder used when the (shared) VPU is busy.

pub mod decode;
pub mod dma_heap;
pub mod jpeg;
pub mod jpeg_regs;
pub mod mpp;
pub mod scale;
pub mod swjpeg;
pub mod tables;

use std::ffi::{c_char, CStr};

use dma_heap::DmaBuf;
use mpp::{MppService, VPU_CLIENT_JPEG_DEC};

/// Decoder state kept across frames (buffers, tables, scaler).
pub struct Decoder {
    svc: MppService,
    table: DmaBuf,
    stream: DmaBuf,
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
        let mut stream =
            DmaBuf::alloc("system", data.len(), false).map_err(|e| e.to_string())?;
        stream.as_mut_slice()[..data.len()].copy_from_slice(&data);

        let aw = (info.width + 15) / 16 * 16;
        let ah = (info.height + 15) / 8 * 8;
        let output = DmaBuf::alloc("system", aw * ah * 3, false).map_err(|e| e.to_string())?;

        let (regs, geom) = decode::build(&decode::Job {
            info: &info,
            table_fd: table.fd(),
            stream_fd: stream.fd(),
            stream_len: data.len(),
            output_fd: output.fd(),
            rgb: true,
        })
        .map_err(|e| e.to_string())?;

        Ok(Decoder {
            svc,
            table,
            stream,
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
            .submit_regs(&self.regs.words)
            .map_err(|e| format!("submit: {e}"))?;
        self.svc
            .poll_hw_finish()
            .map_err(|e| format!("poll: {e}"))?;
        let _ = self.output.sync_read_begin();
        let g = self.scaler.letterbox_strided(
            self.output.as_slice(),
            self.geom.stride_bytes,
            self.geom.out_width,
            self.geom.out_height,
            dst,
        );
        let _ = self.output.sync_read_end();
        Ok((g.pad_left, g.pad_top, g.scale))
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

/// Open a decoder for `path`, producing `dst_w` x `dst_h` RGB888 frames.
///
/// # Safety
/// `path` must be a NUL-terminated string.
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
    let p = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };
    match Decoder::open(p, dst_w as usize, dst_h as usize) {
        Ok(d) => {
            if !out_w.is_null() {
                *out_w = d.src_size().0 as u32;
            }
            if !out_h.is_null() {
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
/// `h` must come from `rk_jpeg_open` and `dst` must have room for the frame.
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
    let dec = &mut (*h).0;
    let n = dec.dst_w * dec.dst_h * 3;
    let slice = std::slice::from_raw_parts_mut(dst, n);
    match dec.frame(slice) {
        Ok((pl, pt, s)) => {
            if !pad_left.is_null() {
                *pad_left = pl;
            }
            if !pad_top.is_null() {
                *pad_top = pt;
            }
            if !scale.is_null() {
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
        drop(Box::from_raw(h));
    }
}
