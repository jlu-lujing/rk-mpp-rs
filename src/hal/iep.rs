//! IEP2 (client 28) register programming for the deinterlace path.
//!
//! The `mpp_service` client 28 is the IEP2 engine, not the legacy IEP:
//! `mpp/vproc/iep2/iep2.c` is what announces `IEP_CLIENT_TYPE` (= 28) on
//! `/dev/mpp_service`, whereas the legacy `iep/iep.c` opens a separate
//! `/dev/iep` node with its own `IEP_SET_PARAMETER` ioctl and never touches
//! client 28. Everything the client-28 session can do is *deinterlace*:
//! there is no scaler in the IEP2 register block, and the YUV-scale API
//! (`IepCmdParamScale`, `IEP_SCALE_ALG_*`) lives only in the legacy IEP
//! header. So this module derives the IEP2 parameter block for a
//! deinterlace job.
//!
//! IEP2 has no per-command protocol: `iep2_start()` submits the whole
//! `struct iep2_params` as one `MPP_CMD_SET_REG_WRITE` and the
//! `struct iep2_output` as the paired `MPP_CMD_SET_REG_READ`, then polls
//! `MPP_CMD_POLL_HW_FINISH`. The parameter struct *is* the register bank -
//! the kernel maps it word for word - so the field table below is the byte
//! layout of `struct iep2_params` (`iep2/iep2.h`), in word (u32) indices.
//!
//! Buffer registers carry the standard MPP `(byte_offset << 10) | dma-buf fd`
//! encoding: `set_addr()` in `iep2.c` sets y/cbcr/cr from the IepImg address
//! fields, which the test fills as `fd + (offset << 10)`.

/// Width of `struct iep2_params` in words, i.e. the size of the SET_REG_WRITE
/// buffer `iep2_start()` sends (`sizeof(struct iep2_params) / 4` = 656/4).
pub const PARAMS_WORDS: usize = 164;

/// Width of `struct iep2_output` in words, the paired SET_REG_READ buffer.
pub const OUTPUT_WORDS: usize = 103;

/// Tile grid the engine processes pictures in (`iep2.h` TILE_W / TILE_H).
pub const TILE_W: usize = 16;
pub const TILE_H: usize = 4;

/// `mv_hist` histogram depth in the output block: `MVL + MVR + 1`.
pub const MVL: usize = 28;
pub const MVR: usize = 27;

// --- struct iep2_params field word offsets (from iep2/iep2.h order) ---------

pub const SRC_FMT: usize = 0;
pub const SRC_YUV_SWAP: usize = 1;
pub const DST_FMT: usize = 2;
pub const DST_YUV_SWAP: usize = 3;
pub const TILE_COLS: usize = 4;
pub const TILE_ROWS: usize = 5;
pub const SRC_Y_STRIDE: usize = 6;
pub const SRC_UV_STRIDE: usize = 7;
pub const DST_Y_STRIDE: usize = 8;
/// `src[3]` of `iep2_addr { y, cbcr, cr }`: current, next, previous frame.
pub const SRC0_Y: usize = 9;
pub const SRC1_Y: usize = 12;
pub const SRC2_Y: usize = 15;
/// `dst[2]` of `iep2_addr`: top/bottom reconstructed frame (right after src).
pub const DST0_Y: usize = 18;
pub const DST1_Y: usize = 21;
pub const MV_ADDR: usize = 24;
pub const MD_ADDR: usize = 25;
pub const DIL_MODE: usize = 26;
pub const DIL_OUT_MODE: usize = 27;
pub const DIL_FIELD_ORDER: usize = 28;
pub const MD_THETA: usize = 29;
pub const MD_R: usize = 30;
pub const MD_LAMBDA: usize = 31;
pub const DECT_RESI_THR: usize = 32;
pub const OSD_AREA_NUM: usize = 33;
pub const OSD_GRADH_THR: usize = 34;
pub const OSD_GRADV_THR: usize = 35;
pub const OSD_POS_LIMIT_EN: usize = 36;
pub const OSD_POS_LIMIT_NUM: usize = 37;
pub const OSD_LIMIT_AREA: usize = 38; // [2]
pub const OSD_LINE_NUM: usize = 40;
pub const OSD_PEC_THR: usize = 41;
pub const OSD_X_STA: usize = 42; // [8]
pub const OSD_X_END: usize = 50; // [8]
pub const OSD_Y_STA: usize = 58; // [8]
pub const OSD_Y_END: usize = 66; // [8]
pub const ME_PENA: usize = 74;
pub const MV_BONUS: usize = 75;
pub const MV_SIMILAR_THR: usize = 76;
pub const MV_SIMILAR_NUM_THR0: usize = 77;
pub const ME_THR_OFFSET: usize = 78;
pub const MV_LEFT_LIMIT: usize = 79;
pub const MV_RIGHT_LIMIT: usize = 80;
/// `int8_t mv_tru_list[8]` - eight bytes, i.e. two words.
pub const MV_TRU_LIST: usize = 81;
pub const MV_TRU_VLD: usize = 83; // [8]
pub const EEDI_THR0: usize = 91;
pub const BLE_BACKTOMA_NUM: usize = 92;
pub const COMB_CNT_THR: usize = 93;
pub const COMB_FEATURE_THR: usize = 94;
pub const COMB_T_THR: usize = 95;
pub const COMB_OSD_VLD: usize = 96; // [8]
pub const MTN_EN: usize = 104;
pub const MTN_TAB: usize = 105; // [16]
pub const PD_MODE: usize = 121;
pub const ROI_EN: usize = 122;
pub const ROI_LAYER_NUM: usize = 123;
pub const ROI_MODE: usize = 124; // [8]
pub const XSTA: usize = 132; // [8]
pub const XEND: usize = 140; // [8]
pub const YSTA: usize = 148; // [8]
pub const YEND: usize = 156; // [8]

// --- struct iep2_output field word offsets (read-back) ----------------------

pub const OUT_MV_HIST: usize = 0; // [MVL + MVR + 1]
pub const OUT_DECT_PD_TCNT: usize = OUT_MV_HIST + (MVL + MVR + 1); // 56
pub const OUT_DECT_PD_BCNT: usize = OUT_DECT_PD_TCNT + 1;
pub const OUT_DECT_FF_CUR_TCNT: usize = OUT_DECT_PD_TCNT + 2;
pub const OUT_DECT_FF_CUR_BCNT: usize = OUT_DECT_PD_TCNT + 3;
pub const OUT_DECT_FF_NXT_TCNT: usize = OUT_DECT_PD_TCNT + 4;
pub const OUT_DECT_FF_NXT_BCNT: usize = OUT_DECT_PD_TCNT + 5;
pub const OUT_DECT_FF_BLE_TCNT: usize = OUT_DECT_PD_TCNT + 6;
pub const OUT_DECT_FF_BLE_BCNT: usize = OUT_DECT_PD_TCNT + 7;
pub const OUT_DECT_FF_NZ: usize = OUT_DECT_PD_TCNT + 8;
pub const OUT_DECT_FF_COMB_F: usize = OUT_DECT_PD_TCNT + 9;
pub const OUT_DECT_OSD_CNT: usize = OUT_DECT_PD_TCNT + 10;
pub const OUT_COMB_CNT: usize = OUT_DECT_PD_TCNT + 11;
pub const OUT_OSD_COMB_CNT: usize = OUT_DECT_PD_TCNT + 12;
pub const OUT_FF_GRADT_TCNT: usize = OUT_DECT_PD_TCNT + 13;
pub const OUT_FF_GRADT_BCNT: usize = OUT_DECT_PD_TCNT + 14;

/// `enum IEP2_FMT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Iep2Fmt {
    Yuv422 = 2,
    Yuv420 = 3,
}

impl Iep2Fmt {
    /// Bytes in a `width x height` picture: 4:2:2 is 2 bytes per luma sample
    /// total, 4:2:0 is 1.5 (`get_frm_size` in `iep2_test.c`).
    pub const fn frame_bytes(self, width: usize, height: usize) -> usize {
        match self {
            Iep2Fmt::Yuv422 => width * height * 2,
            Iep2Fmt::Yuv420 => width * height * 3 / 2,
        }
    }

    /// Allocation bytes for a picture whose rows are `hor_stride` apart.
    ///
    /// The luma plane always occupies `hor_stride * height`. Planar chroma
    /// uses the same half-width rounding as [`strides`], while semi-planar
    /// chroma keeps the full luma stride.
    pub const fn allocation_bytes(
        self,
        ver_stride: usize,
        hor_stride: usize,
        swap: Iep2YuvSwap,
    ) -> usize {
        let luma = hor_stride * ver_stride;
        let planar_stride = (hor_stride / 2 + 15) / 16 * 16;
        match (self, swap) {
            (Iep2Fmt::Yuv422, Iep2YuvSwap::P) => luma + 2 * planar_stride * ver_stride,
            (Iep2Fmt::Yuv422, _) => luma + hor_stride * ver_stride,
            (Iep2Fmt::Yuv420, Iep2YuvSwap::P) => luma + 2 * planar_stride * ver_stride.div_ceil(2),
            (Iep2Fmt::Yuv420, _) => luma + hor_stride * ver_stride.div_ceil(2),
        }
    }
}

/// `enum IEP2_YUV_SWAP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Iep2YuvSwap {
    SpUv = 0,
    SpVu = 1,
    P0 = 2,
    P = 3,
}

/// `enum IEP2_DIL_MODE` (de-interlace mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Iep2DilMode {
    Disable = 0,
    /// 5 fields in, 2 frames out (the mode `iep2_test.c` runs).
    I5O2 = 1,
    I5O1T = 2,
    I5O1B = 3,
    I2O2 = 4,
    I1O1T = 5,
    I1O1B = 6,
    Pd = 7,
    Bypass = 8,
    Dect = 9,
}

impl Iep2DilMode {
    /// Destination slots the mode writes, in `dst[0], dst[1]` order.
    pub const fn output_slots(self) -> (bool, bool) {
        match self {
            Self::I5O2 | Self::I2O2 => (true, true),
            Self::I5O1T | Self::I1O1T | Self::Bypass | Self::Disable => (true, false),
            Self::I5O1B | Self::I1O1B => (false, true),
            Self::Pd | Self::Dect => (false, false),
        }
    }
}

/// `enum IEP2_OUT_MODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Iep2OutMode {
    Line = 0,
    Tile = 1,
}

/// `enum IEP2_FIELD_ORDER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Iep2FieldOrder {
    Tff = 0,
    Bff = 1,
    Und = 2,
}

/// Default motion-noise table (`iep2_mtn_tab` in `iep2.c`).
pub const DEFAULT_MTN_TAB: [u32; 16] = [
    0x0000_0000,
    0x0000_0000,
    0x0000_0000,
    0x0000_0000,
    0x0101_0000,
    0x0605_0302,
    0x0f0d_0a08,
    0x1c19_1512,
    0x2b28_2420,
    0x3634_312e,
    0x3d3c_3a38,
    0x4040_3f3e,
    0x4040_4040,
    0x4040_4040,
    0x4040_4040,
    0x4040_4040,
];

/// `(byte_offset << 10) | fd`, the MPP dma-buf register encoding.
///
/// The kernel's translation table reads the low 10 bits as the dma-buf fd and
/// the rest as a byte offset into it, matching `ADD_OFFSET` / `set_addr` in
/// the C test. dma-buf fds are small, so the low-10-bit mask is only a guard.
pub const fn buf_reg(fd: i32, byte_offset: usize) -> u32 {
    ((byte_offset as u32) << 10) | ((fd as u32) & 0x3ff)
}

/// `tile_cols = (width + 15) / 16` from `IEP2_PARAM_TYPE_COM`.
pub const fn tile_cols(width: usize) -> usize {
    (width + 15) / TILE_W
}

/// `tile_rows = (height + 3) / 4` from `IEP2_PARAM_TYPE_COM`.
pub const fn tile_rows(height: usize) -> usize {
    (height + 3) / TILE_H
}

/// `osd_pec_thr = (width * 26) >> 7` from `IEP2_PARAM_TYPE_COM`.
pub const fn osd_pec_thr(width: usize) -> u32 {
    ((width * 26) >> 7) as u32
}

/// Source/destination luma and chroma strides in 4-byte units, exactly the
/// divisions `iep2_set_param(IEP2_PARAM_TYPE_COM)` performs: `hor_stride / 4`
/// for the Y planes, and for planar (SWAP_P) chroma a half-width stride rounded
/// up to 16 pixels first. Returns `(src_y, src_uv, dst_y)`.
pub fn strides(hor_stride: usize, swap: Iep2YuvSwap) -> (u32, u32, u32) {
    let y_stride = (hor_stride / 4) as u32;
    let uv_pixels = if swap == Iep2YuvSwap::P {
        (hor_stride / 2 + 15) / 16 * 16
    } else {
        hor_stride
    };
    (y_stride, (uv_pixels / 4) as u32, y_stride)
}

/// The mv scratch buffer size the engine expects (a byte per 16x4 tile over
/// the maximum picture the driver sizes for): `IEP2_TILE_W_MAX * IEP2_TILE_H_MAX`.
pub const MV_BUF_BYTES: usize = 120 * 480;
/// The md scratch buffer size: the driver allocates `1920 * 1088` unconditionally.
pub const MD_BUF_BYTES: usize = 1920 * 1088;

/// One deinterlace job: the three source frames and two output frames, as
/// dma-buf fds, plus the format/mode knobs the C library sets through
/// `IEP2_PARAM_TYPE_COM` and `IEP2_PARAM_TYPE_MODE`.
#[derive(Debug, Clone, Copy)]
pub struct DeinterlaceJob<'a> {
    pub width: usize,
    pub height: usize,
    /// Horizontal stride in bytes (the C library uses `com.width` as the stride).
    pub hor_stride: usize,
    /// Vertical stride in rows. Decoder-owned surfaces may allocate more rows
    /// than the visible height.
    pub ver_stride: usize,
    pub src_fmt: Iep2Fmt,
    pub src_swap: Iep2YuvSwap,
    pub dst_fmt: Iep2Fmt,
    pub dst_swap: Iep2YuvSwap,
    pub dil_mode: Iep2DilMode,
    pub out_mode: Iep2OutMode,
    pub field_order: Iep2FieldOrder,
    /// [current, next, previous] frame dma-buf fds, matching `src[0..3]`.
    pub src_fds: [i32; 3],
    /// [frame A, frame B] output dma-buf fds, matching `dst[0..2]`.
    pub dst_fds: [i32; 2],
    /// mv and md scratch dma-buf fds.
    pub mv_fd: i32,
    pub md_fd: i32,
    /// Motion-detection lambda; pass the platform default (4 on RK3588).
    pub md_lambda: u32,
    /// `pd_mode`: the inverse-telecine compose flag the caller's pulldown state
    /// machine derived for this picture (`iep2_pd_get_output`). Zero unless the
    /// session runs PD/DECT.
    pub pd_mode: u32,
    /// `mv_tru_list[8]` and `mv_tru_vld[8]` updated from the previous job's MV
    /// histogram.
    pub mv_tru_list: [i8; 8],
    pub mv_tru_vld: [u32; 8],
    pub _marker: std::marker::PhantomData<&'a ()>,
}

/// `set_addr(&params.src[i], img)` in `iep2.c`: y = fd, cb = fd + y_size,
/// cr = fd + y_size + chroma_size. For semi-planar inputs the cr word is set
/// but ignored by the engine; we reproduce it faithfully so the block matches
/// what the C library submits.
fn set_img_addr(
    words: &mut [u32; PARAMS_WORDS],
    base: usize,
    fd: i32,
    fmt: Iep2Fmt,
    y_size: usize,
) {
    let chroma = match fmt {
        Iep2Fmt::Yuv422 => y_size / 2,
        Iep2Fmt::Yuv420 => y_size / 4,
    };
    words[base] = buf_reg(fd, 0);
    words[base + 1] = buf_reg(fd, y_size);
    words[base + 2] = buf_reg(fd, y_size + chroma);
}

/// Build the 164-word IEP2 parameter block for one deinterlace job.
///
/// The defaults replicate `iep2_init()` (the non-RK3528 branch) and the
/// `IEP2_PARAM_TYPE_COM` + `IEP2_PARAM_TYPE_MODE` derivations from
/// `iep2_set_param()`. Only the deinterlace-relevant subset is set: everything
/// else (OSD boxes, ROI windows, mv_tru lists) the C library leaves zeroed for
/// the `iep2_test` path and we do the same.
pub fn build_deinterlace(job: &DeinterlaceJob) -> [u32; PARAMS_WORDS] {
    let mut w = [0u32; PARAMS_WORDS];

    // iep2_init() defaults (the parts the I5O2 test keeps).
    w[PD_MODE] = job.pd_mode;
    w[MD_THETA] = 1;
    w[MD_R] = 6;
    w[MD_LAMBDA] = job.md_lambda;
    w[DECT_RESI_THR] = 30;
    w[OSD_GRADH_THR] = 60;
    w[OSD_GRADV_THR] = 60;
    w[OSD_LINE_NUM] = 2;
    w[ME_PENA] = 5;
    w[MV_BONUS] = 10;
    w[MV_SIMILAR_THR] = 4;
    w[MV_SIMILAR_NUM_THR0] = 4;
    w[ME_THR_OFFSET] = 20;
    w[MV_LEFT_LIMIT] = 28;
    w[MV_RIGHT_LIMIT] = 27;
    w[EEDI_THR0] = 12;
    w[BLE_BACKTOMA_NUM] = 1;
    w[COMB_T_THR] = 4;
    w[COMB_FEATURE_THR] = 16;
    w[COMB_CNT_THR] = 0;
    // memset(comb_osd_vld, 1, sizeof) -> every byte 1 -> 0x01010101 per word.
    for i in 0..8 {
        w[COMB_OSD_VLD + i] = 0x0101_0101;
    }
    w[MTN_EN] = 1;
    for (i, v) in DEFAULT_MTN_TAB.iter().enumerate() {
        w[MTN_TAB + i] = *v;
    }

    // IEP2_PARAM_TYPE_COM derivation.
    w[SRC_FMT] = job.src_fmt as u32;
    w[SRC_YUV_SWAP] = job.src_swap as u32;
    w[DST_FMT] = job.dst_fmt as u32;
    w[DST_YUV_SWAP] = job.dst_swap as u32;
    let (src_y_stride, src_uv_stride, dst_y_stride) = strides(job.hor_stride, job.src_swap);
    w[SRC_Y_STRIDE] = src_y_stride;
    w[SRC_UV_STRIDE] = src_uv_stride;
    w[DST_Y_STRIDE] = dst_y_stride;
    w[TILE_COLS] = tile_cols(job.width) as u32;
    w[TILE_ROWS] = tile_rows(job.height) as u32;
    w[OSD_PEC_THR] = osd_pec_thr(job.width);

    // IEP2_PARAM_TYPE_MODE derivation.
    w[DIL_MODE] = job.dil_mode as u32;
    w[DIL_OUT_MODE] = job.out_mode as u32;
    w[DIL_FIELD_ORDER] = job.field_order as u32;
    w[MV_TRU_LIST] = u32::from_le_bytes([
        job.mv_tru_list[0] as u8,
        job.mv_tru_list[1] as u8,
        job.mv_tru_list[2] as u8,
        job.mv_tru_list[3] as u8,
    ]);
    w[MV_TRU_LIST + 1] = u32::from_le_bytes([
        job.mv_tru_list[4] as u8,
        job.mv_tru_list[5] as u8,
        job.mv_tru_list[6] as u8,
        job.mv_tru_list[7] as u8,
    ]);
    w[MV_TRU_VLD..MV_TRU_VLD + 8].copy_from_slice(&job.mv_tru_vld);

    // Buffer addresses: three source frames, two output frames, mv/md scratch.
    set_img_addr(
        &mut w,
        SRC0_Y,
        job.src_fds[0],
        job.src_fmt,
        job.hor_stride * job.ver_stride,
    );
    set_img_addr(
        &mut w,
        SRC1_Y,
        job.src_fds[1],
        job.src_fmt,
        job.hor_stride * job.ver_stride,
    );
    set_img_addr(
        &mut w,
        SRC2_Y,
        job.src_fds[2],
        job.src_fmt,
        job.hor_stride * job.ver_stride,
    );
    set_img_addr(
        &mut w,
        DST0_Y,
        job.dst_fds[0],
        job.dst_fmt,
        job.hor_stride * job.ver_stride,
    );
    set_img_addr(
        &mut w,
        DST1_Y,
        job.dst_fds[1],
        job.dst_fmt,
        job.hor_stride * job.ver_stride,
    );
    w[MV_ADDR] = buf_reg(job.mv_fd, 0);
    w[MD_ADDR] = buf_reg(job.md_fd, 0);

    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> DeinterlaceJob<'static> {
        DeinterlaceJob {
            width: 720,
            height: 480,
            hor_stride: 720,
            ver_stride: 480,
            src_fmt: Iep2Fmt::Yuv420,
            src_swap: Iep2YuvSwap::SpUv,
            dst_fmt: Iep2Fmt::Yuv420,
            dst_swap: Iep2YuvSwap::SpUv,
            dil_mode: Iep2DilMode::I5O2,
            out_mode: Iep2OutMode::Line,
            field_order: Iep2FieldOrder::Tff,
            src_fds: [3, 4, 5],
            dst_fds: [6, 7],
            mv_fd: 8,
            md_fd: 9,
            md_lambda: 4,
            pd_mode: 0,
            mv_tru_list: [0; 8],
            mv_tru_vld: [0; 8],
            _marker: std::marker::PhantomData,
        }
    }

    #[test]
    fn field_table_covers_the_whole_struct() {
        // sizeof(struct iep2_params) == 656 bytes == 164 words.
        assert_eq!(PARAMS_WORDS * 4, 656);
        // The last field (yend[8]) must reach exactly the end of the bank.
        assert_eq!(YEND + 8, PARAMS_WORDS);
        // src[3], dst[2], mv, md, mode are contiguous from the strides.
        assert_eq!(SRC0_Y, DST_Y_STRIDE + 1);
        assert_eq!(DST0_Y, SRC2_Y + 3);
        assert_eq!(MV_ADDR, DST1_Y + 3);
        assert_eq!(DIL_MODE, MD_ADDR + 1);
        // mv_tru_list is 8 bytes (2 words), mv_tru_vld is 8 words, then eedi.
        assert_eq!(MV_TRU_VLD, MV_TRU_LIST + 2);
        assert_eq!(EEDI_THR0, MV_TRU_VLD + 8);
    }

    #[test]
    fn output_field_table_covers_the_whole_struct() {
        assert_eq!(OUT_DECT_PD_TCNT, MVL + MVR + 1);
        // mv_hist[56] + 15 scalars + four 8-word arrays == 103 words.
        assert_eq!(OUT_DECT_PD_TCNT + 15 + 4 * 8, OUTPUT_WORDS);
    }

    #[test]
    fn buf_reg_encodes_offset_in_high_bits_fd_in_low_ten() {
        assert_eq!(buf_reg(7, 0), 7);
        assert_eq!(buf_reg(7, 345600), (345600u32 << 10) | 7);
        // A stray fd beyond 10 bits is masked, never carried into the offset.
        assert_eq!(buf_reg(1024 + 3, 4096), (4096u32 << 10) | 3);
    }

    #[test]
    fn com_derivation_matches_the_c_divisions() {
        // 720x480 yuv420 SP_UV, hor_stride 720 (the iep2_test default).
        let (y, uv, dst_y) = strides(720, Iep2YuvSwap::SpUv);
        assert_eq!((y, uv, dst_y), (180, 180, 180)); // 720/4
        assert_eq!(tile_cols(720), 45); // (720+15)/16
        assert_eq!(tile_rows(480), 120); // (480+3)/4
        assert_eq!(osd_pec_thr(720), ((720usize * 26) >> 7) as u32);
    }

    #[test]
    fn allocation_and_plane_offsets_use_the_horizontal_stride() {
        assert_eq!(
            Iep2Fmt::Yuv420.allocation_bytes(32, 64, Iep2YuvSwap::SpUv),
            3_072
        );
        assert_eq!(
            Iep2Fmt::Yuv420.allocation_bytes(32, 64, Iep2YuvSwap::P),
            3_072
        );
        assert_eq!(
            Iep2Fmt::Yuv422.allocation_bytes(32, 64, Iep2YuvSwap::SpUv),
            4_096
        );
        assert_eq!(
            Iep2Fmt::Yuv420.allocation_bytes(48, 64, Iep2YuvSwap::SpUv),
            4_608
        );

        let mut padded = job();
        padded.hor_stride = 768;
        padded.ver_stride = 496;
        let words = build_deinterlace(&padded);
        assert_eq!(words[SRC0_Y + 1], buf_reg(3, 768 * 496));
        assert_eq!(words[SRC0_Y + 2], buf_reg(3, 768 * 496 + 768 * 496 / 4));
        assert_eq!(words[DST0_Y + 1], buf_reg(6, 768 * 496));
    }

    #[test]
    fn planar_stride_rounds_half_width_up_to_sixteen() {
        // For SWAP_P the C rounds (hor_stride/2 + 15)/16*16 before /4.
        let (_, uv, _) = strides(721, Iep2YuvSwap::P);
        assert_eq!(uv, (((721 / 2 + 15) / 16 * 16) / 4) as u32);
    }

    #[test]
    fn deinterlace_block_fills_expected_fields() {
        let w = build_deinterlace(&job());
        assert_eq!(w[SRC_FMT], 3);
        assert_eq!(w[DST_FMT], 3);
        assert_eq!(w[SRC_YUV_SWAP], 0);
        assert_eq!(w[TILE_COLS], 45);
        assert_eq!(w[TILE_ROWS], 120);
        assert_eq!(w[DIL_MODE], Iep2DilMode::I5O2 as u32);
        assert_eq!(w[DIL_FIELD_ORDER], Iep2FieldOrder::Tff as u32);
        assert_eq!(w[MD_THETA], 1);
        assert_eq!(w[MD_R], 6);
        assert_eq!(w[MD_LAMBDA], 4);
        assert_eq!(w[OSD_LINE_NUM], 2);
        assert_eq!(w[COMB_OSD_VLD], 0x0101_0101);
        assert_eq!(w[MTN_TAB + 4], 0x0101_0000);
        assert_eq!(w[MTN_EN], 1);

        // Addresses: current frame y = fd 3, cb at +y_size, cr at +y+y/4.
        let y_size = 720 * 480;
        assert_eq!(w[SRC0_Y], 3);
        assert_eq!(w[SRC0_Y + 1], buf_reg(3, y_size));
        assert_eq!(w[SRC0_Y + 2], buf_reg(3, y_size + y_size / 4));
        // Previous frame is src[2]; output frame A is dst[0], B is dst[1].
        assert_eq!(w[SRC1_Y], 4);
        assert_eq!(w[SRC2_Y], 5);
        assert_eq!(w[DST0_Y], 6);
        assert_eq!(w[DST1_Y], 7);
        assert_eq!(w[MV_ADDR], 8);
        assert_eq!(w[MD_ADDR], 9);
    }

    #[test]
    fn no_default_overlaps_the_buffer_block() {
        // Every default scalar must sit outside src[3]/dst[2]/mv/md addresses,
        // i.e. at or after DIL_MODE - so set_img_addr never clobbers a default.
        assert!(MD_THETA >= DIL_MODE);
        assert!(MTN_TAB >= DIL_MODE);
        // And the buffer block must sit at or below DIL_MODE.
        assert!(MD_ADDR < DIL_MODE);
    }

    #[test]
    fn gmv_candidates_pack_into_the_c_byte_arrays() {
        let mut job = job();
        job.mv_tru_list = [-1, 2, 0, 0, 3, 0, 0, 0];
        job.mv_tru_vld = [1, 1, 0, 0, 0, 0, 0, 0];
        let w = build_deinterlace(&job);
        assert_eq!(w[MV_TRU_LIST], 0x0000_02ff);
        assert_eq!(w[MV_TRU_LIST + 1], 0x0000_0003);
        assert_eq!(w[MV_TRU_VLD], 1);
        assert_eq!(w[MV_TRU_VLD + 1], 1);
    }
}
