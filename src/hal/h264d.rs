//! rkvdec (vdpu34x) H.264 decode HAL for RK3588.
//!
//! Mirrors `hal/rkdec/h264d/hal_h264d_vdpu34x.c` + `common/vdpu34x_com.c`:
//! the driver only gets a 276-word register file, three bit-packed syntax
//! tables, a RCB scratch area and the addresses of the current/ref frames.
//! See `docs/h264d_hw.md` for the contract.

use std::io;

use crate::codec::h264::cabac_table::RKV_CABAC_TABLE;
use crate::codec::h264::dpb::{FrameStore, PictureOrderCount, RefListEntry, ReferenceFlags};
use crate::codec::h264::hw;
use crate::codec::h264::parser::{Pps, SliceHeader, Sps};
use crate::hal::vdpu34x_regs as reg;
use crate::service::dma_heap::DmaBuf;
use crate::service::{
    Batch, Client, MppService, RcbInfo as RcbConfig, RegOffsetInfo, MPP_CMD_SET_RCB_INFO,
};

pub const CABAC_SIZE: usize = 928 * 4 + 128;
pub const SPSPPS_UNIT: usize = 48;
pub const SPSPPS_SIZE: usize = 256 * SPSPPS_UNIT + 128;
pub const RPS_SIZE: usize = 384;
pub const SCLST_SIZE: usize = 6 * 16 + 2 * 64 + 128;
pub const ERROR_INFO_SIZE: usize = 256 * 144 * 4;
const REG_READBACK_WORDS: usize = 278;

static CMV_DUMP_INDEX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One decoded picture: NV12 planes plus its colmv buffer.
pub struct Frame {
    pub nv12: DmaBuf,
    pub colmv: DmaBuf,
    pub width: usize,
    pub height: usize,
    pub hor_stride: usize,
    pub ver_stride: usize,
}

impl Frame {
    pub fn alloc(width: usize, height: usize, frame_mbs_only: bool) -> io::Result<Frame> {
        let hor_stride = align(width, 16);
        let ver_stride = align(height, 16);
        let y_size = hor_stride * ver_stride;
        let nv12 = DmaBuf::alloc("system", y_size * 3 / 2, false)?;
        let colmv = DmaBuf::alloc("system", colmv_size(width, height, frame_mbs_only), false)?;
        Ok(Frame {
            nv12,
            colmv,
            width,
            height,
            hor_stride,
            ver_stride,
        })
    }

    /// Y plane as bytes (NV12 layout, 16-aligned stride).
    pub fn y_plane(&self) -> &[u8] {
        &self.nv12.as_slice()[..self.hor_stride * self.ver_stride]
    }

    /// UV plane (interleaved) as bytes.
    pub fn uv_plane(&self) -> &[u8] {
        let y = self.hor_stride * self.ver_stride;
        &self.nv12.as_slice()[y..y + y / 2]
    }
}

fn align(v: usize, a: usize) -> usize {
    (v + a - 1) / a * a
}

/// Compressed colmv layout used when `frame_mbs_only` is set.
pub fn colmv_size(width: usize, height: usize, compress: bool) -> usize {
    if compress {
        let seg_w = 64 * 4 * 4 / 16; // 64
        let seg_h = 16usize;
        let cnt_w = align(width, seg_w) / seg_w;
        let cnt_h = align(height, seg_h) / seg_h;
        let head = align(cnt_w, 16) * cnt_h;
        let payload = cnt_w * cnt_h * 64 * 16;
        head + payload
    } else {
        let bw = align(width, 64) / 4;
        let bh = align(height, 64) / 4;
        bw * bh * 16 * 2
    }
}

const RCB_ALIGNMENT: usize = 64;
const RCB_REGION_COUNT: usize = 10;

/// RCB regions, in the order MPP concatenates them.
///
/// MPP computes the fixed vdpu34x offsets before refining each region for the
/// current picture. The refined sizes must not be used to repack the regions.
#[derive(Debug, Default, Clone, Copy)]
pub struct RcbInfo {
    pub dblk: usize,
    pub intra: usize,
    pub transd_row: usize,
    pub strmd_row: usize,
    pub inter_row: usize,
    pub sao: usize,
    pub fbc: usize,
    pub transd_col: usize,
    pub inter_col: usize,
    pub filt_col: usize,
    offsets: [u32; RCB_REGION_COUNT],
    allocation_size: usize,
}

fn rcb_bytes(bits: usize) -> usize {
    align((bits + 7) / 8, RCB_ALIGNMENT)
}

fn rcb_aligned_size(size: usize) -> usize {
    align(size, RCB_ALIGNMENT)
}

impl RcbInfo {
    /// `h264d_refine_rcb_size` + `vdpu34x_get_rcb_buf_size`.
    pub fn compute(width: usize, height: usize, bit_depth: usize, mbaff: bool) -> RcbInfo {
        let w = align(width, 16);
        let h = align(height, 16);
        let layout_w = align(width, 64);
        let layout_h = align(height, 64);
        let common_sizes = [
            rcb_aligned_size(layout_w * 22),
            rcb_aligned_size(layout_w * 6),
            rcb_aligned_size(layout_w),
            rcb_aligned_size(layout_w * 3),
            rcb_aligned_size(layout_w * 6),
            rcb_aligned_size(layout_w * 6),
            rcb_aligned_size(layout_w * 11),
            rcb_aligned_size(layout_h),
            rcb_aligned_size(layout_h * 3),
            rcb_aligned_size(layout_h * 67),
        ];
        let mut offsets = [0u32; RCB_REGION_COUNT];
        let mut allocation_size = 0usize;
        for (offset, size) in offsets.iter_mut().zip(common_sizes) {
            *offset = allocation_size as u32;
            allocation_size += size;
        }
        RcbInfo {
            dblk: rcb_bytes(w * (2 + if mbaff { 12 } else { 6 } * bit_depth)),
            intra: rcb_bytes(w * 44),
            transd_row: rcb_bytes(if w > 8192 {
                ((w - 8192 + 3) / 4) * 2
            } else {
                0
            }),
            strmd_row: rcb_bytes(if w > 4096 {
                ((w + 15) / 16) * 154 * if mbaff { 2 } else { 1 }
            } else {
                0
            }),
            inter_row: rcb_bytes(w * 42),
            sao: 0,
            fbc: 0,
            transd_col: rcb_bytes(if h > 8192 {
                ((h - 8192 + 3) / 4) * 2
            } else {
                0
            }),
            inter_col: 0,
            filt_col: 0,
            offsets,
            allocation_size,
        }
    }

    pub fn total(&self) -> usize {
        self.allocation_size
    }

    /// (register index, byte offset) pairs the hardware expects.
    pub fn offsets(&self) -> Vec<RegOffsetInfo> {
        let mut out = Vec::with_capacity(10);
        self.write_offsets(&mut out);
        out
    }

    /// Same entries as [`Self::offsets`], appended into a caller-owned
    /// buffer so the decode path does not allocate per frame.
    pub fn write_offsets(&self, out: &mut Vec<RegOffsetInfo>) {
        out.extend(
            [139u32, 133, 134, 136, 137, 140, 141, 135, 138, 142]
                .iter()
                .zip(self.offsets)
                .map(|(&reg_idx, offset)| RegOffsetInfo { reg_idx, offset }),
        );
    }

    #[cfg(test)]
    fn hardware_sizes(&self) -> Vec<RcbConfig> {
        let mut out = Vec::new();
        self.write_hardware_sizes(&mut out);
        out
    }

    /// Same entries as [`Self::hardware_sizes`], appended into a
    /// caller-owned buffer so the decode path does not allocate per frame.
    fn write_hardware_sizes(&self, out: &mut Vec<RcbConfig>) {
        // Mirrors the RV3588 order in vdpu34x_set_rcbinfo. MPP deliberately
        // omits INTER_ROW because putting it in SRAM conflicts with DDR.
        out.extend(
            [
                (139u32, self.dblk),
                (133, self.intra),
                (140, self.sao),
                (141, self.fbc),
                (134, self.transd_row),
                (136, self.strmd_row),
                (138, self.inter_col),
                (142, self.filt_col),
                (135, self.transd_col),
            ]
            .into_iter()
            .filter(|(_, size)| *size > 0)
            .map(|(reg_idx, size)| RcbConfig {
                reg_idx,
                size: size as u32,
            }),
        );
    }
}

pub struct H264d {
    svc: MppService,
    cabac: DmaBuf,
    spspps: DmaBuf,
    rps: DmaBuf,
    sclst: DmaBuf,
    errinfo: DmaBuf,
    rcb: DmaBuf,
    rcb_info: RcbInfo,
    rcb_info_supported: bool,
    pub regs: reg::Regs,
    /// (reg224, reg226, reg227) read back after the last poll.
    pub last_status: (u32, u32, u32),
    pub last_hardware_error: bool,
    /// Reused across frames: the ioctl needs owned scratch but not fresh
    /// allocations, and the entries are rebuilt in full every job.
    offsets_scratch: Vec<RegOffsetInfo>,
    rcb_scratch: Vec<RcbConfig>,
    batch: Batch,
}

impl H264d {
    pub fn open() -> io::Result<H264d> {
        let svc = MppService::open(Client::RkvDec as u32)?;
        let rcb_info_supported = svc.cmd_supported(MPP_CMD_SET_RCB_INFO);
        let cabac = DmaBuf::alloc("system", CABAC_SIZE, false)?;
        let spspps = DmaBuf::alloc("system", SPSPPS_SIZE, false)?;
        let rps = DmaBuf::alloc("system", RPS_SIZE, false)?;
        let sclst = DmaBuf::alloc("system", SCLST_SIZE, false)?;
        let errinfo = DmaBuf::alloc("system", ERROR_INFO_SIZE, false)?;
        let rcb = DmaBuf::alloc("system", 1 << 20, false)?;
        let mut hw = H264d {
            svc,
            cabac,
            spspps,
            rps,
            sclst,
            errinfo,
            rcb,
            rcb_info: RcbInfo::default(),
            rcb_info_supported,
            regs: reg::Regs::new(),
            last_status: (0, 0, 0),
            last_hardware_error: false,
            offsets_scratch: Vec::with_capacity(10),
            rcb_scratch: Vec::with_capacity(9),
            batch: Batch::new(),
        };
        hw.cabac.sync_write_begin()?;
        hw.cabac
            .as_mut_slice()
            .iter_mut()
            .zip(RKV_CABAC_TABLE.iter().flat_map(|v| v.to_le_bytes()))
            .for_each(|(d, s)| *d = s);
        hw.cabac.sync_write_end()?;
        hw.init_common();
        Ok(hw)
    }

    fn init_common(&mut self) {
        let r = &mut self.regs;
        r.set(reg::w9::DEC_MODE, 1) // H.264
            .set(reg::w15::RLC_MODE, 0)
            .set(reg::w11::BUF_EMPTY_EN, 1)
            .set(reg::w11::DEC_TIMEOUT_E, 1)
            .set(reg::w11::DEC_CLKGATE_E, 1)
            .set(reg::w11::DEC_E_STRMD_CLKGATE_DIS, 0)
            .set(reg::w10::DEC_E, 1)
            .set(reg::w17::SLICE_NUM, 0x3fff)
            .set(reg::w12::WAIT_RESET_EN, 1)
            .set(reg::w13::H26X_ERROR_MODE, 1)
            .set(reg::w13::COLMV_ERROR_MODE, 1)
            .set(reg::w13::H26X_STREAMD_ERROR_MODE, 1)
            .set(reg::w13::TIMEOUT_MODE, 1)
            .set(reg::w21::ERROR_DEB_EN, 1)
            .set(reg::w21::INTER_ERROR_PRC_MODE, 0)
            .set(reg::w21::ERROR_INTRA_MODE, 1)
            .set(reg::w24::CABAC_ERR_EN_LOWBITS, 0)
            .set(reg::w25::CABAC_ERR_EN_HIGHBITS, 0)
            .set(reg::w26::SWREG_BLOCK_GATING_E, 0xfffef)
            .set(reg::w26::REG_CFG_GATING_EN, 1)
            .set(reg::w32::REG032_TIMEOUT_THRESHOLD, 0x3ffff)
            .set(reg::w28::SW_POC_ARB_FLAG, 0);
        r.w[65] = 0;
        r.w[66] = 0;
        // vdpu34x_setup_statistic: perf counters + AXI QoS hints.
        r.set(reg::w11::PIX_RANGE_DETECTION_E, 1)
            .set(reg::w256::AXI_PERF_WORK_E, 1)
            .set(reg::w256::AXI_PERF_CLR_E, 1)
            .set(reg::w256::AXI_CNT_TYPE, 1)
            .set(reg::w257::ADDR_ALIGN_TYPE, 1)
            .set(reg::w270::BUS2MC_BUFFER_QOS_LEVEL, 255)
            .set(reg::w270::AXI_RD_HURRY_LEVEL, 3)
            .set(reg::w270::AXI_WR_HURRY_LEVEL, 1)
            .set(reg::w270::AXI_WR_QOS, 1)
            .set(reg::w270::AXI_RD_QOS, 3)
            .set(reg::w271::REG271_WR_WAIT_CYCLE_QOS, 0);
    }

    /// Fill the register file for one frame.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_regs(
        &mut self,
        sps: &Sps,
        pps: &Pps,
        sh: &SliceHeader,
        order: PictureOrderCount,
        cur: &Frame,
        refs: &[Option<&Frame>],
        ref_pocs: &[i32],
        ref_flags: &[Option<ReferenceFlags>],
        ref_frame_nums: &[u32],
        stream_len: usize,
        idr: bool,
    ) {
        let r = &mut self.regs;
        r.set(reg::w16::REG016_STR_LEN, stream_len as u32)
            .set(reg::w13::CUR_PIC_IS_IDR, idr as u32)
            .set(reg::w12::COLMV_COMPRESS_EN, sps.frame_mbs_only as u32)
            .set(reg::w18::Y_HOR_VIRSTRIDE, (cur.hor_stride / 16) as u32)
            .set(reg::w19::UV_HOR_VIRSTRIDE, (cur.hor_stride / 16) as u32)
            .set(
                reg::w20::REG020_Y_VIRSTRIDE_Y_VIRSTRIDE,
                ((cur.hor_stride * cur.ver_stride) / 16) as u32,
            );
        write_current_picture_poc(r, sh, order);
        r.set(reg::w130::REG130_DECOUT_BASE, cur.nv12.fd() as u32)
            .set(reg::w131::REG131_COLMV_CUR_BASE, cur.colmv.fd() as u32)
            .set(reg::w128::REG128_RLC_BASE, 0)
            .set(reg::w197::REG197_CABACTBL_BASE, self.cabac.fd() as u32);
        // POC list (words 67..98) and per-ref flags (99..102), ref bases.
        for i in 0..16usize {
            r.w[67 + 2 * i] = ref_pocs.get(2 * i).copied().unwrap_or(0) as u32;
            r.w[67 + 2 * i + 1] = ref_pocs.get(2 * i + 1).copied().unwrap_or(0) as u32;
        }
        // The C HAL addresses these through SET_REF_INFO macros with literal
        // register indices (reg99..reg102), so take the words from there
        // rather than from declaration order.
        let present = std::array::from_fn(|i| refs.get(i).copied().flatten().is_some());
        let resolved = resolve_reference_slots(&present);
        write_reference_poc_high_bits(r, &resolved, sh);
        for i in 0..16usize {
            if let Some(flags) = ref_flags.get(i).copied().flatten() {
                write_reference_fields(r, i, flags);
            }
            let fallback = if resolved[i] == 16 {
                cur
            } else {
                refs[resolved[i]].expect("resolved reference slot")
            };
            r.w[164 + i] = fallback.nv12.fd() as u32;
            r.w[181 + i] = fallback.colmv.fd() as u32;
        }
        let error_ref_index = select_error_reference(ref_frame_nums, sh.frame_num)
            .filter(|&i| refs.get(i).copied().flatten().is_some());
        let error_ref = error_ref_index
            .and_then(|i| refs.get(i).copied().flatten())
            .unwrap_or(cur);
        r.set(reg::w132::REG132_ERROR_REF_BASE, error_ref.nv12.fd() as u32)
            .set(
                reg::w21::ERROR_INTRA_MODE,
                error_intra_mode(error_ref_index.is_some(), pps.weighted_pred),
            );
        let _ = sps;
    }

    fn prepare_tables(
        &mut self,
        sps: &Sps,
        pps: &Pps,
        sh: &SliceHeader,
        dpb: &[Option<&FrameStore>],
        lists: [&[RefListEntry]; 3],
    ) -> io::Result<()> {
        let unit = hw::build_sps_pps(sps, pps, dpb, sh);
        self.spspps.sync_write_begin()?;
        let buf = self.spspps.as_mut_slice();
        for i in 0..256 {
            let off = i * SPSPPS_UNIT;
            let n = unit.len().min(SPSPPS_UNIT);
            buf[off..off + n].copy_from_slice(&unit[..n]);
        }
        self.spspps.sync_write_end()?;
        if std::env::var("RFI_SPSPPS_DUMP").is_ok() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/our_spspps.bin")
            {
                let _ = writeln!(f, "TASK {}", sh.frame_num);
                let _ = f.write_all(&self.spspps.as_slice()[..SPSPPS_UNIT]);
            }
        }
        let rps = hw::build_rps_entries(sps, sh, dpb, lists);
        let n = rps.len().min(RPS_SIZE);
        self.rps.sync_write_begin()?;
        self.rps.as_mut_slice()[..n].copy_from_slice(&rps[..n]);
        self.rps.sync_write_end()?;
        let scan = hw::build_scanlist(None, None);
        let n = scan.len().min(SCLST_SIZE);
        if n > 0 {
            self.sclst.sync_write_begin()?;
            self.sclst.as_mut_slice()[..n].copy_from_slice(&scan[..n]);
            self.sclst.sync_write_end()?;
        }
        Ok(())
    }

    /// Decode one frame's worth of slices and wait for the hardware.
    #[allow(clippy::too_many_arguments)]
    pub fn decode(
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
        if std::env::var("RFI_DUMP_LEN").is_ok() {
            eprintln!("SUBMIT len={} idr={} fn={}", stream_len, idr, sh.frame_num);
        }
        self.prepare_tables(sps, pps, sh, dpb, lists)?;
        if std::env::var("RFI_TABLE_DUMP").is_ok() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/our_tables.bin")
            {
                let _ = writeln!(f, "TASK {}", order.top as u32);
                let _ = f.write_all(&self.rps.as_slice()[..RPS_SIZE]);
            }
        }

        let mbaff = !sps.frame_mbs_only && sps.mb_adaptive_frame_field && !sh.field_pic;
        let info = RcbInfo::compute(cur.width, cur.height, sps.bit_depth_luma as usize, mbaff);
        let total = info.total().max(4096);
        if total > self.rcb.len() {
            self.rcb = DmaBuf::alloc("system", align(total, 4096), false)?;
        }
        self.rcb_info = info;

        self.fill_regs(
            sps,
            pps,
            sh,
            order,
            cur,
            refs,
            ref_pocs,
            ref_flags,
            ref_frame_nums,
            stream_len,
            idr,
        );

        let r = &mut self.regs;
        r.set(reg::w128::REG128_RLC_BASE, stream.fd() as u32)
            .set(reg::w129::REG129_RLCWRITE_BASE, stream.fd() as u32)
            .set(reg::w161::REG161_PPS_HEAD_BASE, self.spspps.fd() as u32)
            .set(reg::w163::REG163_RPS_BASE, self.rps.fd() as u32)
            .set(reg::w180::REG180_SCANLIST_BASE, self.sclst.fd() as u32)
            .set(reg::w12::SCANLIST_ADDR_VALID_EN, 1);
        let rcb_fd = self.rcb.fd() as u32;
        r.set(reg::w139::REG139_RCB_DBLK_BASE, rcb_fd)
            .set(reg::w133::REG133_RCB_INTRA_BASE, rcb_fd)
            .set(reg::w134::REG134_RCB_TRANSD_ROW_BASE, rcb_fd)
            .set(reg::w136::REG136_RCB_STREAMD_ROW_BASE, rcb_fd)
            .set(reg::w137::REG137_RCB_INTER_ROW_BASE, rcb_fd)
            .set(reg::w140::REG140_RCB_SAO_BASE, rcb_fd)
            .set(reg::w141::REG141_RCB_FBC_BASE, rcb_fd)
            .set(reg::w135::REG135_RCB_TRANSD_COL_BASE, rcb_fd)
            .set(reg::w138::REG138_RCB_INTER_COL_BASE, rcb_fd)
            .set(reg::w142::REG142_RCB_FILTER_COL_BASE, rcb_fd);

        // MPP writes four sections and never the interrupt-status words.
        // They are contiguous in the register file, so the batch points
        // straight into it: same bytes, no per-frame copies.
        if std::env::var("RFI_REG_DUMP").is_ok() {
            use std::fs::OpenOptions;
            use std::io::Write;
            if let Ok(mut f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/our_regs.txt")
            {
                let _ = writeln!(f, "FRAME {}", order.frame as u32);
                for i in 8..284 {
                    let v = if i < r.w.len() { r.w[i] } else { 0 };
                    let _ = writeln!(f, "{:08x}", v);
                }
            }
        }
        self.offsets_scratch.clear();
        self.rcb_info.write_offsets(&mut self.offsets_scratch);
        self.rcb_scratch.clear();
        self.rcb_info.write_hardware_sizes(&mut self.rcb_scratch);
        let mut readback = [0u32; REG_READBACK_WORDS];
        self.batch.clear();
        self.batch
            .reg_write(&r.w[8..64], 32)
            .reg_write(&r.w[64..128], 256)
            .reg_write(&r.w[128..200], 512)
            .reg_write(&r.w[200..205], 800)
            .reg_write(&r.w[256..276], 1024)
            .reg_read(&mut readback, 0);
        self.batch.reg_offsets(&self.offsets_scratch);
        if self.rcb_info_supported && !self.rcb_scratch.is_empty() {
            self.batch.rcb_info(&self.rcb_scratch);
        }
        self.svc.submit_batch(&mut self.batch)?;
        self.svc.poll_hw_finish()?;
        self.last_status = (readback[224], readback[226], readback[227]);
        self.last_hardware_error =
            hardware_status_error(readback[224], readback[225], readback[226], readback[227]);
        if std::env::var_os("RFI_COLMV_DUMP").is_some() {
            use std::io::Write;

            let task = CMV_DUMP_INDEX.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = format!("/tmp/rust_cmv_{task:02}.bin");
            let _ = cur.colmv.sync_read_begin();
            if let Ok(mut file) = std::fs::File::create(path) {
                let _ = file.write_all(cur.colmv.as_slice());
            }
            let _ = cur.colmv.sync_read_end();
        }
        Ok(())
    }
}

fn hardware_status_error(common: u32, stream_detect: u32, stream: u32, colmv: u32) -> bool {
    common & (1 << 4) != 0
        || common & (1 << 2) == 0
        || common & (1 << 6) != 0
        || stream_detect & (1 << 1) != 0
        || stream & ((1 << 28) - 1) != 0
        || colmv & 0xf != 0
}

fn write_current_poc(r: &mut reg::Regs, top: i32, bottom: i32) {
    r.w[65] = top as u32;
    r.w[66] = bottom as u32;
}

fn write_current_picture_poc(r: &mut reg::Regs, sh: &SliceHeader, order: PictureOrderCount) {
    let (top, bottom) = order.hardware_fields(sh);
    write_current_poc(r, top, bottom);
    r.w[204] = u32::from(sh.field_pic && sh.bottom_field);
}

fn write_reference_fields(r: &mut reg::Regs, index: usize, flags: ReferenceFlags) {
    let field = reg::Field {
        word: 99 + index / 4,
        lsb: ((index % 4) * 8) as u8,
        width: 1,
    };
    r.set(field, flags.field_coded as u32)
        .set(
            reg::Field {
                lsb: field.lsb + 1,
                ..field
            },
            flags.top_used as u32,
        )
        .set(
            reg::Field {
                lsb: field.lsb + 2,
                ..field
            },
            flags.bottom_used as u32,
        )
        .set(
            reg::Field {
                lsb: field.lsb + 3,
                ..field
            },
            flags.colmv_valid as u32,
        );
}

fn resolve_reference_slots(present: &[bool; 16]) -> [usize; 16] {
    let mut nearest = 16;
    std::array::from_fn(|i| {
        if present[i] {
            nearest = i;
        }
        nearest
    })
}

fn write_reference_high_pocs(r: &mut reg::Regs, resolved: &[usize; 16]) {
    write_reference_poc_high_bits(r, resolved, &SliceHeader::default());
}

fn write_reference_poc_high_bits(r: &mut reg::Regs, resolved: &[usize; 16], sh: &SliceHeader) {
    r.w[200..204].fill(0);
    for (index, &fallback) in resolved.iter().enumerate() {
        if sh.field_pic {
            // For field pictures the nibble selects the top (0) or bottom (1)
            // POC slot rather than carrying an overflow bit.
            r.w[200 + index / 4] |= 1 << (8 * (index % 4) + 4);
        } else if fallback == 16 {
            // MPP substitutes the current picture for a missing reference and
            // encodes its POC high bit as 3 for both fields.
            r.w[200 + index / 4] |= 0x33 << (8 * (index % 4));
        }
    }
}

fn select_error_reference(frame_nums: &[u32], current: u32) -> Option<usize> {
    frame_nums
        .iter()
        .enumerate()
        .filter(|(_, frame_num)| **frame_num != 0 && **frame_num < current)
        .min_by_key(|(_, frame_num)| **frame_num)
        .map(|(i, _)| i)
}

fn error_intra_mode(has_error_reference: bool, weighted_pred: bool) -> u32 {
    if has_error_reference && !weighted_pred {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_poc_is_written_to_top_and_bottom_fields() {
        let mut regs = reg::Regs::new();
        write_current_poc(&mut regs, 37, 35);
        assert_eq!(regs.w[65], 37);
        assert_eq!(regs.w[66], 35);
    }

    #[test]
    fn field_picture_writes_only_its_coded_field_poc() {
        let mut regs = reg::Regs::new();
        let sh = SliceHeader {
            field_pic: true,
            bottom_field: true,
            ..Default::default()
        };
        let order = PictureOrderCount {
            top: 37,
            bottom: 39,
            frame: 39,
        };

        write_current_picture_poc(&mut regs, &sh, order);

        assert_eq!((regs.w[65], regs.w[66]), (0, 39));
    }

    #[test]
    fn bottom_field_sets_current_poc_high_bit() {
        let mut regs = reg::Regs::new();
        let sh = SliceHeader {
            field_pic: true,
            bottom_field: true,
            ..Default::default()
        };

        write_current_picture_poc(
            &mut regs,
            &sh,
            PictureOrderCount {
                top: 37,
                bottom: 39,
                frame: 39,
            },
        );

        assert_eq!(regs.w[204], 1);
    }

    #[test]
    fn reference_fields_encode_top_only_state() {
        let mut regs = reg::Regs::new();
        write_reference_fields(
            &mut regs,
            0,
            ReferenceFlags {
                field_coded: true,
                top_used: true,
                bottom_used: false,
                colmv_valid: true,
            },
        );

        assert_eq!(regs.w[99] & 0xf, 0b1011);
    }

    #[test]
    fn missing_reference_slots_fall_back_to_current_or_nearest() {
        let mut present = [false; 16];
        present[3] = true;
        present[7] = true;

        let resolved = resolve_reference_slots(&present);

        assert_eq!(&resolved[..3], &[16, 16, 16]);
        assert_eq!(&resolved[3..7], &[3, 3, 3, 3]);
        assert_eq!(&resolved[7..], &[7; 9]);
    }

    #[test]
    fn missing_reference_slots_use_the_current_picture_poc_high_bit() {
        let mut regs = reg::Regs::new();
        let resolved = [16; 16];

        write_reference_high_pocs(&mut regs, &resolved);

        assert_eq!(&regs.w[200..204], &[0x3333_3333; 4]);
        assert_eq!(regs.w[204], 0);
    }

    #[test]
    fn field_references_tag_top_and_bottom_poc_slots() {
        let mut regs = reg::Regs::new();
        let sh = SliceHeader {
            field_pic: true,
            ..Default::default()
        };

        write_reference_poc_high_bits(&mut regs, &[16; 16], &sh);

        assert_eq!(&regs.w[200..204], &[0x1010_1010; 4]);
    }

    #[test]
    fn error_reference_uses_oldest_nonzero_frame_before_current() {
        let mut frame_nums = [0u32; 16];
        frame_nums[0] = 2;
        frame_nums[1] = 1;
        frame_nums[2] = 4;

        assert_eq!(select_error_reference(&frame_nums, 4), Some(1));
        assert_eq!(select_error_reference(&frame_nums, 1), None);
        assert_eq!(select_error_reference(&frame_nums, 0), None);
    }

    #[test]
    fn error_intra_mode_is_disabled_for_an_unweighted_older_reference() {
        assert_eq!(error_intra_mode(true, false), 0);
    }

    #[test]
    fn rcb_sizes_follow_the_formula() {
        let info = RcbInfo::compute(1920, 1080, 8, false);
        assert_eq!(info.dblk, rcb_bytes(1920 * (2 + 6 * 8)));
        assert_eq!(info.intra, rcb_bytes(1920 * 44));
        assert_eq!(info.inter_row, rcb_bytes(1920 * 42));
        assert_eq!(info.strmd_row, 0, "only above 4096 wide");
        assert!(info.total() > 0);
    }

    #[test]
    fn rcb_offsets_use_the_fixed_aligned_layout() {
        let info = RcbInfo::compute(1920, 1080, 8, false);
        let offs = info.offsets();
        assert_eq!(offs.len(), 10);
        assert_eq!((offs[0].reg_idx, offs[9].reg_idx), (139, 142));
        assert_eq!(
            offs.iter().map(|info| info.offset).collect::<Vec<_>>(),
            [0, 42240, 53760, 55680, 61440, 72960, 84480, 105600, 106688, 109952]
        );
        assert_eq!(info.total(), 182848);
        assert_ne!(
            info.dblk, 42240,
            "refined size must not define the RCB layout"
        );
    }

    #[test]
    fn rcb_sram_config_uses_mpp_priority_order() {
        let info = RcbInfo::compute(1920, 1080, 8, false);
        let regs: Vec<u32> = info
            .hardware_sizes()
            .into_iter()
            .map(|cfg| cfg.reg_idx)
            .collect();
        assert_eq!(regs, [139, 133]);
    }

    #[test]
    fn colmv_grows_with_resolution() {
        assert!(colmv_size(1920, 1080, true) > colmv_size(1280, 720, true));
        assert!(colmv_size(1920, 1080, false) >= colmv_size(1920, 1080, true));
    }

    #[test]
    fn hardware_status_matches_vdpu34x_error_checks() {
        assert!(!hardware_status_error(1 << 2, 0, 0, 0));
        assert!(hardware_status_error(0, 0, 0, 0));
        assert!(hardware_status_error((1 << 2) | (1 << 4), 0, 0, 0));
        assert!(hardware_status_error((1 << 2) | (1 << 6), 0, 0, 0));
        assert!(hardware_status_error(1 << 2, 1 << 1, 0, 0));
        assert!(hardware_status_error(1 << 2, 0, 1, 0));
        assert!(hardware_status_error(1 << 2, 0, 0, 1));
    }

    /// The reused-scratch writers must reproduce the original allocating
    /// lists exactly, for resolutions where the width-dependent zero-size
    /// regions change which entries survive the filter.
    #[test]
    fn rcb_scratch_lists_match_the_original_allocating_lists() {
        for (width, height, depth, mbaff) in [
            (1920usize, 1080usize, 8usize, false),
            (320, 240, 8, false),
            (4096, 2304, 8, true),
        ] {
            let info = RcbInfo::compute(width, height, depth, mbaff);

            let expected = info.offsets();
            let mut got = vec![RegOffsetInfo {
                reg_idx: 0,
                offset: u32::MAX,
            }];
            got.clear();
            info.write_offsets(&mut got);
            assert_eq!(got, expected, "{width}x{height} offsets");

            let expected = info.hardware_sizes();
            let mut got = vec![RcbConfig {
                reg_idx: 0,
                size: u32::MAX,
            }];
            got.clear();
            info.write_hardware_sizes(&mut got);
            assert_eq!(got.len(), expected.len(), "{width}x{height} sizes");
            for (got, want) in got.iter().zip(&expected) {
                assert_eq!((got.reg_idx, got.size), (want.reg_idx, want.size));
            }
        }
    }
}
