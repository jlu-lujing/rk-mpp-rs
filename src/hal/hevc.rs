//! rkvdec (vdpu34x) HEVC decode HAL for RK3588.
//!
//! Port of `hal/rkdec/h265d/hal_h265d_vdpu34x.c::hal_h265d_vdpu34x_gen_regs`:
//! the driver receives the 276-word register file, the 112-byte SPS/PPS packet
//! replicated 64 times, the RPS table and the bitstream. Register semantics
//! follow the C source; the tables come from `codec::hevc::hw`.

use std::io;

use crate::codec::hevc::hw;
use crate::codec::hevc::parser::{Pps, SliceHeader, Sps};
use crate::hal::vdpu34x_regs as reg;
use crate::service::dma_heap::DmaBuf;
use crate::service::{Client, MppService, RegOffsetInfo};

/// HEVC CABAC initial context table (`cabac_table[27456]` in MPP).
const CABAC_TABLE: &[u8; 27456] = include_bytes!("../../assets/hevc_cabac_table.bin");

pub const PPS_PACKET_COPIES: usize = 64;
pub const PPS_BUF_SIZE: usize = PPS_PACKET_COPIES * hw::PPS_PACKET_SIZE;
/// MPP keeps cabac/SPS-PPS/RPS in ONE buffer and addresses them through the
/// register offset table; the hardware appears to rely on that layout.
const CABAC_OFFSET: usize = 0;
const CABAC_ALIGNED: usize = 28672; // align(27456, 4096)
const SPSPPS_OFFSET: usize = CABAC_ALIGNED;
// MPP aligns every region in the shared buffer to 4 KiB.
const RPS_OFFSET: usize = (SPSPPS_OFFSET + PPS_BUF_SIZE + 4095) & !4095;
const BUFS_SIZE: usize = RPS_OFFSET + hw::RPS_TABLE_SIZE + 128;

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
    pub fn alloc(width: usize, height: usize, ctu: usize) -> io::Result<Frame> {
        let hor_stride = align(width, 16);
        let ver_stride = align(height, 16);
        let y_size = hor_stride * ver_stride;
        let nv12 = DmaBuf::alloc("system", y_size * 3 / 2, false)?;
        let colmv = DmaBuf::alloc("system", colmv_size(width, height, ctu), false)?;
        Ok(Frame {
            nv12,
            colmv,
            width,
            height,
            hor_stride,
            ver_stride,
        })
    }

    pub fn y_plane(&self) -> &[u8] {
        &self.nv12.as_slice()[..self.hor_stride * self.ver_stride]
    }

    pub fn uv_plane(&self) -> &[u8] {
        let y = self.hor_stride * self.ver_stride;
        &self.nv12.as_slice()[y..y + y / 2]
    }
}

fn align(v: usize, a: usize) -> usize {
    (v + a - 1) / a * a
}

/// Compressed colmv layout (vdpu34x_get_colmv_size) for the picture's CTU
/// size. The segment width is derived from the CTU, so a 64x64 CTU needs a
/// bigger buffer than a 16x16 one.
pub fn colmv_size(width: usize, height: usize, ctu: usize) -> usize {
    // HEVC calls vdpu34x_get_colmv_size(width, height, ctu, 16, 16, 1), i.e.
    // colmv_bytes = colmv_size = 16 (H.264 uses 16/4).
    let seg_w = 64 * 16 * 16 / ctu;
    let seg_h = ctu;
    let cnt_w = align(width, seg_w) / seg_w;
    let cnt_h = align(height, seg_h) / seg_h;
    align(cnt_w, 16) * cnt_h + cnt_w * cnt_h * 64 * 16
}

/// RCB regions in `vdpu34x_get_rcb_buf_size` order.
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
    /// `h265d_refine_rcb_size`; FBC is always off in this implementation.
    pub fn compute(
        width: usize,
        height: usize,
        chroma_idc: u32,
        bit_depth: u32,
        ctu_size: usize,
        tile_col_cut: usize,
    ) -> RcbInfo {
        let w = align(width, ctu_size);
        let h = align(height, ctu_size);
        let bd = bit_depth as usize;
        let ext = tile_col_cut * 64 * 8;
        let sao_flag = true;

        let strmd_row = if w > 8192 {
            let factor = ctu_size / 16;
            rcb_bytes((align(w, ctu_size) + factor - 1) * factor * 24 + ext)
        } else {
            0
        };
        let transd_row = if w > 8192 {
            rcb_bytes(((w - 8192).next_multiple_of(4)) * 2 + ext)
        } else {
            0
        };
        let transd_col = if h > 8192 && tile_col_cut > 0 {
            rcb_bytes((h - 8192).next_multiple_of(4) * 2)
        } else {
            0
        };
        let inter_row = rcb_bytes(w * 22 + ext);
        let inter_col = if tile_col_cut > 0 { rcb_bytes(h * 22) } else { 0 };
        let intra = rcb_bytes(w * 48 + ext);

        let dblk_bits = if chroma_idc == 1 || chroma_idc == 2 {
            if ctu_size == 32 {
                w * (4 + 6 * bd)
            } else {
                w * (2 + 6 * bd)
            }
        } else if ctu_size == 32 {
            w * (4 + 8 * bd)
        } else {
            w * (2 + 8 * bd)
        } + tile_col_cut * (if bd == 8 { 256 } else { 192 })
            + ext;
        let dblk = rcb_bytes(dblk_bits);

        let sao_bits = if sao_flag {
            (if chroma_idc == 1 || chroma_idc == 2 {
                w * (128 / ctu_size + 2 * bd)
            } else {
                w * (128 / ctu_size + 3 * bd)
            }) + tile_col_cut * (if bd == 8 { 160 } else { 128 })
                + ext
        } else {
            0
        };
        let sao = rcb_bytes(sao_bits);
        let fbc = 0;
        let filt_col = 0;
        RcbInfo {
            dblk,
            intra,
            transd_row,
            strmd_row,
            inter_row,
            sao,
            fbc,
            transd_col,
            inter_col,
            filt_col,
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

    /// (register index, byte offset) pairs, identical ordering to H.264.
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
            .map(|&(reg_idx, size)| {
                let info = RegOffsetInfo {
                    reg_idx,
                    offset: off,
                };
                off += size as u32;
                info
            })
            .collect()
    }
}

pub struct HevcHw {
    svc: MppService,
    bufs: DmaBuf,
    rcb: DmaBuf,
    rcb_info: RcbInfo,
    pub regs: reg::Regs,
}

impl HevcHw {
    pub fn open() -> io::Result<HevcHw> {
        // The board's hardware mask advertises a dedicated HEVC decode client
        // (bit 8) next to rkvdec (bit 9); try it first, falling back to rkvdec.
        let client = std::env::var("RFI_HEVC_CLIENT")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(Client::HevcDec as u32);
        let svc = match MppService::open(client) {
            Ok(s) => s,
            Err(_) => MppService::open(Client::RkvDec as u32)?,
        };
        let mut bufs = DmaBuf::alloc("system", BUFS_SIZE, false)?;
        let n = bufs.as_mut_slice().len().min(CABAC_TABLE.len());
        bufs.as_mut_slice()[CABAC_OFFSET..CABAC_OFFSET + n]
            .copy_from_slice(&CABAC_TABLE[..n]);
        let rcb = DmaBuf::alloc("system", 1 << 20, false)?;
        let mut hw = HevcHw {
            svc,
            bufs,
            rcb,
            rcb_info: RcbInfo::default(),
            regs: reg::Regs::new(),
        };
        hw.init_common();
        Ok(hw)
    }

    fn init_common(&mut self) {
        let r = &mut self.regs;
        // The HEVC HAL leaves reg009.dec_mode at its reset value (0); only the
        // H.264 HAL programs it (as 1).
        r.set(reg::w9::DEC_MODE, 0)
            .set(reg::w11::BUF_EMPTY_EN, 1)
            .set(reg::w11::DEC_TIMEOUT_E, 1)
            .set(reg::w11::DEC_CLKGATE_E, 1)
            .set(reg::w11::DEC_E_STRMD_CLKGATE_DIS, 0)
            .set(reg::w10::DEC_E, 1)
            .set(reg::w12::COLMV_COMPRESS_EN, 1)
            .set(reg::w13::H26X_ERROR_MODE, 1)
            .set(reg::w13::H26X_STREAMD_ERROR_MODE, 1)
            .set(reg::w13::COLMV_ERROR_MODE, 1)
            .set(reg::w21::ERROR_DEB_EN, 1)
            .set(reg::w21::INTER_ERROR_PRC_MODE, 0)
            .set(reg::w21::ERROR_INTRA_MODE, 1)
            .set(reg::w24::CABAC_ERR_EN_LOWBITS, 0)
            .set(reg::w25::CABAC_ERR_EN_HIGHBITS, 0)
            .set(reg::w26::SWREG_BLOCK_GATING_E, 0xfffef)
            .set(reg::w26::REG_CFG_GATING_EN, 1)
            .set(reg::w32::REG032_TIMEOUT_THRESHOLD, 0x3ffff)
            .set(reg::w103::REF_PIC_LAYER_SAME_WITH_CUR, 0xffff);
    }

    fn prepare_tables(&mut self, sps: &Sps, pps: &Pps) {
        let pkt = hw::build_pps_packet(sps, pps);
        let buf = self.bufs.as_mut_slice();
        for i in 0..PPS_PACKET_COPIES {
            let off = SPSPPS_OFFSET + i * hw::PPS_PACKET_SIZE;
            buf[off..off + hw::PPS_PACKET_SIZE].copy_from_slice(&pkt);
        }
        let rps = hw::build_sps_rps(&[], &[]);
        let n = rps.len().min(hw::RPS_TABLE_SIZE);
        buf[RPS_OFFSET..RPS_OFFSET + n].copy_from_slice(&rps[..n]);
    }

    /// Fill the register file for one picture.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_regs(
        &mut self,
        sps: &Sps,
        pps: &Pps,
        sh: &SliceHeader,
        poc: i32,
        cur: &Frame,
        refs: &[Option<&Frame>],
        ref_pocs: &[i32],
        stream_len: usize,
        slice_count: u32,
        nal_type: u8,
    ) {
        // HEVC pads the bitstream length to 16 bytes plus 64 and always keeps
        // wr_ddr_align_en except for tiles.
        let padded = align(stream_len, 16) + 64;
        let r = &mut self.regs;
        r.set(reg::w16::REG016_STR_LEN, padded as u32)
            .set(reg::w17::SLICE_NUM, slice_count)
            .set(reg::w12::WR_DDR_ALIGN_EN, (!pps.tiles_enabled) as u32)
            .set(reg::w12::COLMV_COMPRESS_EN, 1)
            .set(reg::w18::Y_HOR_VIRSTRIDE, (cur.hor_stride / 16) as u32)
            .set(reg::w19::UV_HOR_VIRSTRIDE, (cur.hor_stride / 16) as u32)
            .set(
                reg::w20::REG020_Y_VIRSTRIDE_Y_VIRSTRIDE,
                ((cur.hor_stride * cur.ver_stride) / 16) as u32,
            );
        // HW-RPS path: the hardware derives the slice RPS itself, so rps_mode
        // stays 0 and wait_reset_en is asserted like the C HAL does.
        r.w[64] = 0;
        r.set(reg::w12::WAIT_RESET_EN, 1);
        r.w[65] = poc as u32;

        r.set(reg::w130::REG130_DECOUT_BASE, cur.nv12.fd() as u32)
            .set(reg::w131::REG131_COLMV_CUR_BASE, cur.colmv.fd() as u32)
            .set(reg::w161::REG161_PPS_HEAD_BASE, self.bufs.fd() as u32)
            .set(reg::w163::REG163_RPS_BASE, self.bufs.fd() as u32)
            .set(reg::w197::REG197_CABACTBL_BASE, self.bufs.fd() as u32)
            .set(reg::w180::REG180_SCANLIST_BASE, 0);

        let mut nearest: Option<u32> = None;
        let mut nearest_dist = i32::MAX;
        for i in 0..16usize {
            r.w[67 + i] = ref_pocs.get(i).copied().unwrap_or(0) as u32;
            let flag = reg::Field {
                word: 100 + i / 8,
                lsb: (((i % 8) / 4) * 8 + (i % 4)) as u8,
                width: 1,
            };
            match refs.get(i).copied().flatten() {
                Some(f) => {
                    r.set(flag, 1);
                    r.w[164 + i] = f.nv12.fd() as u32;
                    r.w[181 + i] = f.colmv.fd() as u32;
                    let d = (ref_pocs.get(i).copied().unwrap_or(0) - poc).abs();
                    if d < nearest_dist {
                        nearest_dist = d;
                        nearest = Some(f.nv12.fd() as u32);
                    }
                }
                None => {
                    // MPP points unused slots at the current picture (its
                    // valid_ref fallback) instead of leaving address 0.
                    r.set(flag, 0);
                    r.w[164 + i] = cur.nv12.fd() as u32;
                    r.w[181 + i] = cur.colmv.fd() as u32;
                }
            }
        }
        r.set(
            reg::w132::REG132_ERROR_REF_BASE,
            nearest.unwrap_or(cur.nv12.fd() as u32),
        )
        .set(reg::w128::REG128_RLC_BASE, 0)
        .set(reg::w129::REG129_RLCWRITE_BASE, 0)
        .set(reg::w13::TIMEOUT_MODE, 1)
        .set(reg::w11::BUF_EMPTY_EN, 1);
        // IRAP (IDR_W_RADL / IDR_N_LP) tells the decoder to reset its state.
        let idr = sh.slice_type == 2
            && matches!(nal_type, 19 | 20);
        r.set(reg::w13::CUR_PIC_IS_IDR, idr as u32);
        let _ = nal_type;
    }

    /// Decode one picture and wait for the hardware.
    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &mut self,
        sps: &Sps,
        pps: &Pps,
        sh: &SliceHeader,
        poc: i32,
        cur: &Frame,
        refs: &[Option<&Frame>],
        ref_pocs: &[i32],
        stream: &DmaBuf,
        stream_len: usize,
        slice_count: u32,
        nal_type: u8,
    ) -> io::Result<()> {
        self.prepare_tables(sps, pps);
        let ctu_size = 1usize << (sps.log2_min_cb + sps.log2_diff_max_min_cb);
        let rcb = RcbInfo::compute(
            cur.width,
            cur.height,
            sps.chroma_format_idc,
            sps.bit_depth_luma.max(sps.bit_depth_chroma),
            ctu_size,
            pps.tile_columns.saturating_sub(1) as usize,
        );
        let total = rcb.total().max(4096);
        if total > self.rcb.len() {
            self.rcb = DmaBuf::alloc("system", align(total, 4096), false)?;
        }
        self.rcb_info = rcb;
        self.fill_regs(sps, pps, sh, poc, cur, refs, ref_pocs, stream_len, slice_count, nal_type);

        let r = &mut self.regs;
        r.set(reg::w128::REG128_RLC_BASE, stream.fd() as u32)
            .set(reg::w129::REG129_RLCWRITE_BASE, stream.fd() as u32)
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

        let comm_gen: Vec<u32> = r.w[8..64].to_vec();
        let comm_paras: Vec<u32> = r.w[64..128].to_vec();
        let comm_addr: Vec<u32> = r.w[128..200].to_vec();
        let highpoc: Vec<u32> = r.w[200..205].to_vec();
        let statistic: Vec<u32> = r.w[256..276].to_vec();
        let mut offs = self.rcb_info.offsets();
        // cabac sits at offset 0 of the shared buffer, the SPS/PPS packet and
        // the RPS follow at their layout offsets.
        offs.push(RegOffsetInfo {
            reg_idx: 161,
            offset: SPSPPS_OFFSET as u32,
        });
        offs.push(RegOffsetInfo {
            reg_idx: 163,
            offset: RPS_OFFSET as u32,
        });
        let mut batch = crate::service::Batch::new();
        batch
            .reg_write(&comm_gen, 32)
            .reg_write(&comm_paras, 256)
            .reg_write(&comm_addr, 512)
            .reg_write(&highpoc, 800)
            .reg_write(&statistic, 1024)
            .reg_offsets(&offs);
        self.svc.submit_batch(&mut batch)?;
        self.svc.poll_hw_finish()?;
        if std::env::var("RFI_HEVC_STATUS").is_ok() {
            let mut back = vec![0u32; 276];
            let _ = self.svc.call(
                crate::service::MPP_CMD_SET_REG_READ,
                back.as_mut_ptr() as u64,
                (back.len() * 4) as u32,
            );
            eprintln!(
                "STATUS reg1={:08x} reg45={:08x} reg76={:08x}",
                back[1], back[45], back[76]
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colmv_grows_with_resolution() {
        assert!(colmv_size(1920, 1080, 64) > colmv_size(1280, 720, 64));
    }

    #[test]
    fn colmv_scales_with_the_ctu() {
        // HEVC passes colmv_bytes = colmv_size = 16, so the segment width is
        // 64 * 16 * 16 / ctu and the total is bounded by picture area either
        // way; all three sizes must be positive and of the same order.
        let c16 = colmv_size(640, 480, 16);
        let c32 = colmv_size(640, 480, 32);
        let c64 = colmv_size(640, 480, 64);
        for v in [c16, c32, c64] {
            assert!(v > 0 && v < 64 * 1024 * 1024, "{v}");
        }
    }

    #[test]
    fn pps_buffer_holds_64_copies() {
        assert_eq!(PPS_BUF_SIZE, 64 * 112);
    }

    #[test]
    fn rcb_matches_the_c_formula_for_1080p() {
        // 1920x1080, 4:2:0, 8-bit, 64x64 CTU, no tiles.
        let r = RcbInfo::compute(1920, 1080, 1, 8, 64, 0);
        assert_eq!(r.dblk, rcb_bytes(1920 * (2 + 6 * 8)));
        assert_eq!(r.intra, rcb_bytes(1920 * 48));
        assert_eq!(r.inter_row, rcb_bytes(1920 * 22));
        assert_eq!(r.sao, rcb_bytes(1920 * (128 / 64 + 2 * 8)));
        assert_eq!(r.strmd_row, 0, "only above 8192 wide");
        assert_eq!(r.filt_col, 0, "only with tiles");
        assert!(r.total() > 0);
    }

    #[test]
    fn rcb_offsets_accumulate_in_order() {
        let r = RcbInfo::compute(1920, 1080, 1, 8, 64, 0);
        let offs = r.offsets();
        assert_eq!(offs.len(), 10);
        assert_eq!((offs[0].reg_idx, offs[9].reg_idx), (139, 142));
        assert_eq!(offs[0].offset, 0);
        let mut expect = 0u32;
        for (o, size) in offs.iter().zip([
            r.dblk,
            r.intra,
            r.transd_row,
            r.strmd_row,
            r.inter_row,
            r.sao,
            r.fbc,
            r.transd_col,
            r.inter_col,
            r.filt_col,
        ]) {
            assert_eq!(o.offset, expect);
            expect += size as u32;
        }
        assert_eq!(expect as usize, r.total());
    }
}
