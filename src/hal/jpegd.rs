//! jpegd register programming for one baseline JPEG frame.
//!
//! Port of `jpegd_gen_regs()` + `setup_output_fmt()` from rockchip-linux/mpp
//! `mpp/hal/vpu/jpegd/hal_jpegd_rkv.c`, for the RGB888 output path.
//!
//! Buffer registers carry `(offset << 10) | fd`; the kernel's translation table
//! turns the low 10 bits into an IOVA and adds the byte offset.

use crate::codec::jpeg::parser::JpegInfo;
use crate::codec::jpeg::regs::*;
use crate::codec::jpeg::tables;

// Output format (hal_jpegd_vpu7xx_com.h)
pub const YUV_OUT_FMT_2_RGB888: u32 = 1;
pub const YUV_OUT_FMT_2_NV12: u32 = 3;
pub const YUV_OUT_FMT_NO_TRANS: u32 = 0;
pub const SCALEDOWN_DISABLE: u32 = 0;
pub const OUTPUT_RASTER: u32 = 0;
pub const BIT_DEPTH_8: u32 = 0;
pub const TBL_ENTRY_1: u32 = 1;
pub const TBL_ENTRY_2: u32 = 2;
pub const TBL_ENTRY_3: u32 = 3;
pub const RST_ENABLE: u32 = 1;
pub const YUV_MODE_400: u32 = 0;
pub const YUV_MODE_420: u32 = 2;
pub const YUV_MODE_422: u32 = 3;
pub const YUV_MODE_440: u32 = 4;
pub const YUV_MODE_444: u32 = 5;
pub const YUV_MODE_411: u32 = 6;

const ALIGN16: usize = 16;

fn align(v: usize, a: usize) -> usize {
    (v + a - 1) / a * a
}

/// How the decoder must pack this JPEG's components.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegMode {
    Yuv400,
    Yuv420,
    Yuv422,
    Yuv440,
    Yuv444,
    Yuv411,
}

impl JpegMode {
    pub fn from_info(info: &JpegInfo) -> Option<JpegMode> {
        if info.components.len() == 1 {
            let c = info.components[0];
            return if c.h == c.v && c.h != 0 {
                Some(JpegMode::Yuv400)
            } else {
                None
            };
        }
        if info.components.len() > 3 {
            return None;
        }
        if info.components.len() == 2 {
            return None; // not a JPEG colour model we handle
        }
        let (h0, v0) = (info.components[0].h, info.components[0].v);
        let rest_11 = info.components[1..].iter().all(|c| c.h == 1 && c.v == 1);
        if !rest_11 {
            return None;
        }
        match (h0, v0) {
            (2, 2) => Some(JpegMode::Yuv420),
            (2, 1) => Some(JpegMode::Yuv422),
            (1, 2) => Some(JpegMode::Yuv440),
            (1, 1) => Some(JpegMode::Yuv444),
            (4, 1) => Some(JpegMode::Yuv411),
            _ => None,
        }
    }

    fn hw_value(self) -> u32 {
        match self {
            JpegMode::Yuv400 => YUV_MODE_400,
            JpegMode::Yuv420 => YUV_MODE_420,
            JpegMode::Yuv422 => YUV_MODE_422,
            JpegMode::Yuv440 => YUV_MODE_440,
            JpegMode::Yuv444 => YUV_MODE_444,
            JpegMode::Yuv411 => YUV_MODE_411,
        }
    }
}

pub struct Job<'a> {
    pub info: &'a JpegInfo,
    /// ION/dma-buf fd of the 1280-byte table buffer.
    pub table_fd: i32,
    /// dma-buf fd of the whole JPEG file bytes.
    pub stream_fd: i32,
    pub stream_len: usize,
    /// dma-buf fd of the output frame (RGB888 when `rgb`, else NV12).
    pub output_fd: i32,
    /// Ask the post-processor for RGB888 instead of NV12.
    pub rgb: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub out_width: usize,
    pub out_height: usize,
    /// Register value: stride in 16-byte units.
    pub stride: usize,
    /// Real stride in bytes.
    pub stride_bytes: usize,
    /// Real frame size in bytes.
    pub frame_size: usize,
}

/// Build the 42-word command register file and report the output geometry.
pub fn build(job: &Job) -> Result<(Regs, Geometry), &'static str> {
    let info = job.info;
    if info.progressive {
        return Err("progressive JPEG is not supported by the hardware decoder");
    }
    let mode = JpegMode::from_info(info).ok_or("unsupported component sampling")?;
    let nb_components = info.components.len() as u32;

    // Number of quant tables defined in the file and the Huffman table bitmask,
    // both as the parser counts them.
    let qtbl_entry = info.quant.iter().filter(|q| q.is_some()).count() as u32;
    let mut htbl_entry = 0u32;
    for c in info.components.iter() {
        htbl_entry |= 1 << (c.td * 2);
        htbl_entry |= 1 << (c.ta * 2 + 1);
    }

    let mut r = Regs::default();

    // reg1: enable the decoder, timeout and output-buffer-empty interrupts
    r.set(DEC_E, 1);
    r.set(DEC_TIMEOUT_E, 1);
    r.set(BUF_EMPTY_E, 1);

    r.set(PIC_WIDTH_M1, (info.width - 1) as u32);
    r.set(PIC_HEIGHT_M1, (info.height - 1) as u32);

    // reg4: picture format
    let qtables_sel = if nb_components > 1 {
        if qtbl_entry > 1 { TBL_ENTRY_3 } else { TBL_ENTRY_2 }
    } else {
        TBL_ENTRY_1
    };
    let htables_sel = if nb_components > 1 {
        if htbl_entry > 0x0f { TBL_ENTRY_3 } else { TBL_ENTRY_2 }
    } else {
        TBL_ENTRY_1
    };
    r.set(PIXEL_DEPTH, BIT_DEPTH_8);
    r.set(QTABLES_SEL, qtables_sel);
    r.set(HTABLES_SEL, htables_sel);
    if info.restart_interval != 0 {
        r.set(DRI_E, RST_ENABLE);
        r.set(DRI_MCU_NUM_M1, info.restart_interval as u32 - 1);
    }
    r.set(JPEG_MODE, mode.hw_value());

    // reg2: output format (RGB888, full range, raster) + fill bits
    let fill_right = match mode {
        JpegMode::Yuv444 | JpegMode::Yuv440 | JpegMode::Yuv400 => {
            (info.width & 0xf) != 0 && (info.width & 0xf) <= 8
        }
        _ => false,
    };
    let fill_bottom = match mode {
        JpegMode::Yuv444 | JpegMode::Yuv422 | JpegMode::Yuv411 | JpegMode::Yuv400 => {
            (info.height & 0xf) != 0 && (info.height & 0xf) <= 8
        }
        _ => false,
    };
    r.set(SCALEDOWN_MODE, SCALEDOWN_DISABLE);
    r.set(DEC_OUT_SEQUENCE, OUTPUT_RASTER);
    r.set(
        YUV_OUT_FORMAT,
        if job.rgb { YUV_OUT_FMT_2_RGB888 } else { YUV_OUT_FMT_2_NV12 },
    );
    r.set(YUV2RGB_RANGE, 1); // full range, JPEG
    r.set(FILL_DOWN_E, fill_bottom as u32);
    r.set(FILL_RIGHT_E, fill_right as u32);

    // Geometry
    let mut out_width = align(info.width, ALIGN16);
    let mut out_height = if fill_bottom {
        align(info.height, 16)
    } else {
        align(info.height, 8)
    };
    out_width = align(out_width, ALIGN16);
    out_height = align(out_height, 8);
    let y_hor_stride = if job.rgb {
        (out_width >> 4) * 3 // RGB888: 3 bytes per pixel
    } else {
        out_width >> 4 // NV12: one byte per pixel
    };
    let uv_hor_stride = if job.rgb { 0u32 } else { y_hor_stride as u32 };
    let y_virstride = if job.rgb {
        y_hor_stride * out_height
    } else {
        y_hor_stride * out_height * 3 / 2
    };

    r.set(Y_HOR_VIRSTRIDE, (y_hor_stride & 0xffff) as u32);
    r.set(UV_HOR_VIRSTRIDE, uv_hor_stride & 0xffff);
    r.set(Y_VIRSTRIDE, y_virstride as u32);
    r.set(Y_HOR_VIRSTRIDE_H, ((y_hor_stride >> 16) & 1) as u32);

    r.set(QTBL_LEN, if nb_components > 0 { qtables_sel * 8 - 1 } else { 0 });
    r.set(HTBL_VALUE_LEN, htables_sel * 12 - 1); // 8-bit precision
    r.set(
        HTBL_MINCODE_LEN,
        match htables_sel {
            TBL_ENTRY_2 => (nb_components - 1) * 6 - 1,
            _ => nb_components * 6 - 1,
        },
    );

    // Stream window: hardware works on 16-byte aligned chunks
    let strm_offset = info.entropy_start;
    let hw_strm_offset = strm_offset - strm_offset % 16;
    let start_byte = strm_offset % 16;
    let pkt_len = job.stream_len;
    let stream_len = (align(pkt_len - hw_strm_offset, 16) - 1) >> 4;
    r.set(STREAM_LEN, stream_len as u32);
    r.set(STRM_START_BYTE, start_byte as u32);

    // Buffer registers: (offset << 10) | fd
    let fd_reg = |fd: i32, off: usize| ((off as u32) << 10) | ((fd as u32) & 0x3ff);
    r.words[9] = fd_reg(job.table_fd, 0);
    r.words[10] = fd_reg(job.table_fd, tables::RKD_HUFFMAN_MINCODE_TBL_OFFSET);
    r.words[11] = fd_reg(job.table_fd, tables::RKD_HUFFMAN_VALUE_TBL_OFFSET);
    r.words[12] = fd_reg(job.stream_fd, hw_strm_offset);
    r.words[13] = fd_reg(job.output_fd, 0);

    // Robustness: ignore stream errors, keep decoding
    r.set(ERROR_PRC_MODE, 1);
    r.set(STRM_FFFF_ERR_MODE, 2);
    r.set(STRM_OTHER_MARK_MODE, 2);
    r.set(STRM_DRI_SEQ_ERR_MODE, 0);

    // Clock gating: everything on
    r.set(DEC_CLKGATE_E, 1);
    r.set(DEC_STRM_GATE_E, 1);
    r.set(DEC_HUFFMAN_GATE_E, 1);
    r.set(DEC_IZQ_GATE_E, 1);
    r.set(DEC_IDCT_GATE_E, 1);
    r.set(BUSIFD_GATE_E, 1);
    r.set(POST_PRS_GATE_E, 1);
    r.set(DEC_SRAM_GATE_E, 1);

    // Performance counters (as the reference does)
    r.set(AXI_PERF_WORK_E, 1);
    r.set(AXI_PERF_CLR_E, 1);
    r.set(AXI_CNT_TYPE, 1);
    r.set(RD_LATENCY_ID, 0xa);

    // Stride registers are counted in 16-byte units.
    let geom = Geometry {
        out_width,
        out_height,
        stride: y_hor_stride,
        stride_bytes: y_hor_stride * 16,
        frame_size: y_virstride * 16,
    };
    Ok((r, geom))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::jpeg::parser as jpeg;

    const BUS: &str = "/home/user/projects/rknn-projects-test/baseline/images/bus.jpg";

    #[test]
    fn mode_from_sampling() {
        let Ok(data) = std::fs::read(BUS) else { return };
        let mut i = jpeg::parse(&data).unwrap();
        assert_eq!(JpegMode::from_info(&i), Some(JpegMode::Yuv420));
        i.components[0].h = 1;
        i.components[0].v = 1;
        assert_eq!(JpegMode::from_info(&i), Some(JpegMode::Yuv444));
        i.components[0].h = 2;
        i.components[0].v = 1;
        assert_eq!(JpegMode::from_info(&i), Some(JpegMode::Yuv422));
    }

    #[test]
    fn bus_register_values() {
        let Ok(data) = std::fs::read(BUS) else { return };
        let info = jpeg::parse(&data).unwrap();
        let (r, g) = build(&Job {
            info: &info,
            table_fd: 7,
            stream_fd: 9,
            stream_len: data.len(),
            output_fd: 11,
            rgb: true,
        })
        .unwrap();

        assert_eq!(r.get(PIC_WIDTH_M1), 639);
        assert_eq!(r.get(PIC_HEIGHT_M1), 639);
        assert_eq!(r.get(JPEG_MODE), YUV_MODE_420);
        assert_eq!(r.get(DEC_E), 1);
        assert_eq!(r.get(YUV_OUT_FORMAT), YUV_OUT_FMT_2_RGB888);
        assert_eq!(r.get(YUV2RGB_RANGE), 1);

        // 640 wide, RGB888: stride = (640/16)*3 = 120, frame = 120*640
        assert_eq!(g.out_width, 640);
        assert_eq!(g.out_height, 640);
        assert_eq!(g.stride, 120);
        assert_eq!(g.stride_bytes, 1920);
        assert_eq!(g.frame_size, 1920 * 640);

        let (rn, gn) = build(&Job {
            info: &info,
            table_fd: 7,
            stream_fd: 9,
            stream_len: data.len(),
            output_fd: 11,
            rgb: false,
        })
        .unwrap();
        assert_eq!(rn.get(YUV_OUT_FORMAT), YUV_OUT_FMT_2_NV12);
        assert_eq!(gn.stride, 40);
        assert_eq!(gn.frame_size, 640 * 640 * 3 / 2);
        assert_eq!(r.get(Y_HOR_VIRSTRIDE), 120);
        assert_eq!(r.get(Y_VIRSTRIDE), 120 * 640);
        assert_eq!(r.get(UV_HOR_VIRSTRIDE), 0);

        // buffer registers carry fd in the low 10 bits and the offset above
        assert_eq!(r.words[9], 7);
        assert_eq!(r.words[10], (tables::RKD_HUFFMAN_MINCODE_TBL_OFFSET as u32) << 10 | 7);
        assert_eq!(r.words[11], (tables::RKD_HUFFMAN_VALUE_TBL_OFFSET as u32) << 10 | 7);
        assert_eq!(r.words[13], 11);
        assert_eq!(r.words[12] & 0x3ff, 9);
        assert_eq!(r.words[12] >> 10, ((info.entropy_start / 16) * 16) as u32);

        assert_eq!(r.get(STREAM_LEN), ((data.len() - (info.entropy_start / 16) * 16 + 15) / 16 - 1) as u32);
        assert_eq!(r.get(STRM_START_BYTE), (info.entropy_start % 16) as u32);
    }
}
