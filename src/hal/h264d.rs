//! rkvdec (vdpu34x) H.264 decode HAL for RK3588.
//!
//! Mirrors `hal/rkdec/h264d/hal_h264d_vdpu34x.c` + `common/vdpu34x_com.c`:
//! the driver only gets a 276-word register file, three bit-packed syntax
//! tables, a RCB scratch area and the addresses of the current/ref frames.
//! See `docs/h264d_hw.md` for the contract.

use std::io;

use crate::codec::h264::cabac_table::RKV_CABAC_TABLE;
use crate::codec::h264::dpb::FrameStore;
use crate::codec::h264::hw;
use crate::codec::h264::parser::{Pps, SliceHeader, Sps};
use crate::hal::vdpu34x_regs as reg;
use crate::service::dma_heap::DmaBuf;
use crate::service::{Batch, Client, MppService, RegOffsetInfo};

pub const CABAC_SIZE: usize = 928 * 4 + 128;
pub const SPSPPS_UNIT: usize = 48;
pub const SPSPPS_SIZE: usize = 256 * SPSPPS_UNIT + 128;
pub const RPS_SIZE: usize = 384;
pub const SCLST_SIZE: usize = 6 * 16 + 2 * 64 + 128;
pub const ERROR_INFO_SIZE: usize = 256 * 144 * 4;

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
    pub fn alloc(width: usize, height: usize) -> io::Result<Frame> {
        let hor_stride = align(width, 16);
        let ver_stride = align(height, 16);
        let y_size = hor_stride * ver_stride;
        let nv12 = DmaBuf::alloc("system", y_size * 3 / 2, false)?;
        let colmv = DmaBuf::alloc("system", colmv_size(width, height, true), false)?;
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

/// RCB regions, in the order MPP concatenates them.
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
}

fn rcb_bytes(bits: usize) -> usize {
    align((bits + 7) / 8, 256)
}

impl RcbInfo {
    /// `h264d_refine_rcb_size` + `vdpu34x_get_rcb_buf_size`.
    pub fn compute(width: usize, height: usize, bit_depth: usize) -> RcbInfo {
        let w = align(width, 16);
        let h = align(height, 16);
        RcbInfo {
            dblk: rcb_bytes(w * (2 + 6 * bit_depth)),
            intra: rcb_bytes(w * 44),
            transd_row: rcb_bytes(if w > 8192 { ((w - 8192 + 3) / 4) * 2 } else { 0 }),
            strmd_row: rcb_bytes(if w > 4096 { ((w + 15) / 16) * 154 } else { 0 }),
            inter_row: rcb_bytes(w * 42),
            sao: 0,
            fbc: 0,
            transd_col: rcb_bytes(if h > 8192 { ((h - 8192 + 3) / 4) * 2 } else { 0 }),
            inter_col: 0,
            filt_col: 0,
        }
    }

    pub fn total(&self) -> usize {
        self.dblk
            + self.intra
            + self.transd_row
            + self.strmd_row
            + self.inter_row
            + self.sao
            + self.fbc
            + self.transd_col
            + self.inter_col
            + self.filt_col
    }

    /// (register index, byte offset) pairs the hardware expects.
    pub fn offsets(&self) -> Vec<RegOffsetInfo> {
        let order = [
            (139u32, self.dblk),
            (133, self.intra),
            (134, self.transd_row),
            (136, self.strmd_row),
            (137, self.inter_row),
            (140, self.sao),
            (141, self.fbc),
            (135, self.transd_col),
            (138, self.inter_col),
            (142, self.filt_col),
        ];
        let mut off = 0u32;
        order
            .iter()
            .map(|&(idx, size)| {
                let info = RegOffsetInfo {
                    reg_idx: idx,
                    offset: off,
                };
                off += size as u32;
                info
            })
            .collect()
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
    pub regs: reg::Regs,
    /// (reg001, reg045, reg076) read back after the last poll.
    pub last_status: (u32, u32, u32),
}

impl H264d {
    pub fn open() -> io::Result<H264d> {
        let svc = MppService::open(Client::RkvDec as u32)?;
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
            regs: reg::Regs::new(),
            last_status: (0, 0, 0),
        };
        hw.cabac
            .as_mut_slice()
            .iter_mut()
            .zip(RKV_CABAC_TABLE.iter().flat_map(|v| v.to_le_bytes()))
            .for_each(|(d, s)| *d = s);
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
        sh: &SliceHeader,
        poc: i32,
        cur: &Frame,
        refs: &[Option<&Frame>],
        ref_pocs: &[i32],
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
        r.w[65] = poc as u32;
        r.w[66] = 0;
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
        let flag_fields: Vec<reg::Field> = (0..16)
            .map(|i| reg::Field {
                word: 99 + i / 4,
                lsb: ((i % 4) * 8) as u8,
                width: 1,
            })
            .collect();
        let mut nearest: Option<u32> = None;
        for i in 0..16usize {
            let f = refs.get(i).copied().flatten();
            let fd = f.map(|f| f.nv12.fd() as u32).unwrap_or(0);
            r.set(flag_fields[i], (f.is_some()) as u32);
            r.w[164 + i] = fd;
            r.w[181 + i] = f.map(|f| f.colmv.fd() as u32).unwrap_or(0);
            if f.is_some() && nearest.is_none() {
                nearest = Some(fd);
            }
        }
        r.set(
            reg::w132::REG132_ERROR_REF_BASE,
            nearest.unwrap_or(cur.nv12.fd() as u32),
        );
        let _ = sps;
        let _ = sh;
    }

    fn prepare_tables(
        &mut self,
        sps: &Sps,
        pps: &Pps,
        sh: &SliceHeader,
        dpb: &[Option<&FrameStore>],
        lists: [&[u8]; 3],
    ) -> io::Result<()> {
        let unit = hw::build_sps_pps(sps, pps, dpb);
        let buf = self.spspps.as_mut_slice();
        for i in 0..256 {
            let off = i * SPSPPS_UNIT;
            let n = unit.len().min(SPSPPS_UNIT);
            buf[off..off + n].copy_from_slice(&unit[..n]);
        }
        let rps = hw::build_rps(sps, sh, dpb, lists);
        let n = rps.len().min(RPS_SIZE);
        self.rps.as_mut_slice()[..n].copy_from_slice(&rps[..n]);
        let scan = hw::build_scanlist(None, None);
        let n = scan.len().min(SCLST_SIZE);
        if n > 0 {
            self.sclst.as_mut_slice()[..n].copy_from_slice(&scan[..n]);
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
        dpb: &[Option<&FrameStore>],
        lists: [&[u8]; 3],
        cur: &Frame,
        refs: &[Option<&Frame>],
        stream: &DmaBuf,
        idr: bool,
    ) -> io::Result<()> {
        self.prepare_tables(sps, pps, sh, dpb, lists)?;

        let info = RcbInfo::compute(cur.width, cur.height, sps.bit_depth_luma as usize);
        let total = info.total().max(4096);
        if total > self.rcb.len() {
            self.rcb = DmaBuf::alloc("system", align(total, 4096), false)?;
        }
        self.rcb_info = info;

        let ref_pocs: Vec<i32> = (0..32)
            .map(|i| dpb.get(i).copied().flatten().map(|f| f.poc).unwrap_or(0))
            .collect();
        let mut refs16: Vec<Option<&Frame>> = vec![None; 16];
        for (i, slot) in dpb.iter().enumerate().take(16) {
            if let Some(f) = slot {
                if let Some(r) = refs.get(f.slot as usize).copied().flatten() {
                    refs16[i] = Some(r);
                }
            }
        }
        self.fill_regs(sps, sh, 0, cur, &refs16, &ref_pocs, stream.len(), idr);

        let r = &mut self.regs;
        r.set(reg::w128::REG128_RLC_BASE, stream.fd() as u32)
            .set(reg::w129::REG129_RLCWRITE_BASE, stream.fd() as u32)
            .set(reg::w161::REG161_PPS_HEAD_BASE, self.spspps.fd() as u32)
            .set(reg::w163::REG163_RPS_BASE, self.rps.fd() as u32)
            .set(reg::w180::REG180_SCANLIST_BASE, self.sclst.fd() as u32)
            .set(reg::w12::SCANLIST_ADDR_VALID_EN, 1)
            .set(reg::w132::REG132_ERROR_REF_BASE, cur.nv12.fd() as u32);
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
        let comm_gen: Vec<u32> = r.w[8..64].to_vec();
        let comm_paras: Vec<u32> = r.w[64..128].to_vec();
        let comm_addr: Vec<u32> = r.w[128..200].to_vec();
        let highpoc: Vec<u32> = r.w[200..205].to_vec();
        let statistic: Vec<u32> = r.w[256..276].to_vec();
        if std::env::var("RFI_REG_DUMP").is_ok() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::File::create("/tmp/our_regs.txt") {
                for i in 8..284 {
                    let v = if i < r.w.len() { r.w[i] } else { 0 };
                    let _ = writeln!(f, "{:08x}", v);
                }
            }
        }
        let offs = self.rcb_info.offsets();
        let mut batch = Batch::new();
        batch
            .reg_write(&comm_gen, 32)
            .reg_write(&comm_paras, 256)
            .reg_write(&comm_addr, 512)
            .reg_write(&highpoc, 800)
            .reg_write(&statistic, 1024)
            .reg_offsets(&offs);
        self.svc.submit_batch(&mut batch)?;
        self.svc.poll_hw_finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rcb_sizes_follow_the_formula() {
        let info = RcbInfo::compute(1920, 1080, 8);
        assert_eq!(info.dblk, align((1920 * (2 + 6 * 8) + 7) / 8, 256));
        assert_eq!(info.intra, rcb_bytes(1920 * 44));
        assert_eq!(info.inter_row, rcb_bytes(1920 * 42));
        assert_eq!(info.strmd_row, 0, "only above 4096 wide");
        assert!(info.total() > 0);
    }

    #[test]
    fn rcb_offsets_are_contiguous() {
        let info = RcbInfo::compute(1920, 1080, 8);
        let offs = info.offsets();
        assert_eq!(offs.len(), 10);
        assert_eq!((offs[0].reg_idx, offs[9].reg_idx), (139, 142));
        let sizes = [
            info.dblk,
            info.intra,
            info.transd_row,
            info.strmd_row,
            info.inter_row,
            info.sao,
            info.fbc,
            info.transd_col,
            info.inter_col,
            info.filt_col,
        ];
        let mut expect = 0u32;
        for (o, size) in offs.iter().zip(sizes) {
            assert_eq!(o.offset, expect, "reg {} offset", o.reg_idx);
            expect += size as u32;
        }
        assert_eq!(expect as usize, info.total());
    }

    #[test]
    fn colmv_grows_with_resolution() {
        assert!(colmv_size(1920, 1080, true) > colmv_size(1280, 720, true));
        assert!(colmv_size(1920, 1080, false) >= colmv_size(1920, 1080, true));
    }
}
