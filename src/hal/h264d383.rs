//! VDPU383 H.264 decode HAL for RK3576.
//!
//! The newer decoder still consumes the same parsed H.264 model, but addresses
//! move to VDPU383 word positions and the global/RPS tables have a different
//! packing. Register writers, RCB sizing and STATUS interpretation follow
//! `hal_h264d_vdpu383.c`, `common/vdpu383_com.c` and
//! `common/vdpu38x_com.c`.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::codec::h264::cabac_table::RKV_CABAC_TABLE;
use crate::codec::h264::dpb::{FrameStore, PictureOrderCount, RefListEntry, ReferenceFlags};
use crate::codec::h264::hw383;
use crate::codec::h264::parser::{Pps, SliceHeader, Sps};
use crate::hal::h264d::Frame;
use crate::hal::vdpu383_regs as reg;
use crate::service::dma_heap::DmaBuf;
use crate::service::{
    Batch, Client, MppService, RcbInfo as RcbConfig, RegOffsetInfo, MPP_CMD_SET_RCB_INFO,
};

const REG_READBACK_WORDS: usize = 360;
const CABAC_SIZE: usize = 928 * 4 + 128;
const RCB_ALIGNMENT: usize = 64;
static DUMP_INDEX: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Rcb383 {
    sizes: [usize; 11],
    upper_sizes: [usize; 11],
    offsets: [usize; 11],
    total: usize,
}

impl Rcb383 {
    fn compute(
        width: usize,
        height: usize,
        mbaff: bool,
        bit_depth: usize,
        chroma_format_idc: u32,
    ) -> Self {
        let width = align(width, 64);
        let height = align(height, 64);
        let extra_lines = usize::from(mbaff);
        let upper_sizes = [
            rcb_upper(width, 3),
            rcb_upper(width, 3),
            rcb_upper(width, 6),
            rcb_upper(width, 6),
            rcb_upper(width, 5),
            rcb_upper(width, 5),
            rcb_upper(width, 60),
            rcb_upper(width, 60),
            rcb_upper(width, 60),
            rcb_upper(height, 90),
            0,
        ];
        let mut offsets = [0usize; 11];
        let mut total = 0usize;
        for (offset, size) in offsets.iter_mut().zip(upper_sizes) {
            *offset = total;
            total += size;
        }

        let row_uv = usize::from(chroma_format_idc != 0);
        let mut sizes = [0usize; 11];
        let strmd_bits = if width > 4096 {
            ((width + 15) / 16) * 154 * (1 + extra_lines)
        } else {
            0
        };
        sizes[0] = rcb_bits(strmd_bits as f64);
        sizes[1] = sizes[0];
        let inter_bits = ((width + 3) / 4) * 92 * (1 + extra_lines);
        sizes[2] = rcb_bits(inter_bits as f64);
        sizes[3] = sizes[2];
        let mut intra_bits = align(width, 512) * (bit_depth + 2) * (1 + extra_lines);
        if chroma_format_idc == 1 || chroma_format_idc == 2 {
            intra_bits = intra_bits * 5 / 2;
        }
        sizes[4] = rcb_bits(intra_bits as f64);
        sizes[5] = 0;
        let filterd_bits = width as f64
            * 17.0
            * ((6.0 + 3.0 * row_uv as f64) * (1 + extra_lines) as f64 + 2.0 * row_uv as f64 + 1.5);
        let filterd_append = if width > 4096 { 27648 } else { 8192 };
        sizes[6] = filterd_append + rcb_bits(filterd_bits / 2.0);
        sizes[7] = sizes[6];
        sizes[8] = 0;
        sizes[9] = 0;
        sizes[10] = 0;
        Self {
            sizes,
            upper_sizes,
            offsets,
            total: align(total.max(1), 4096),
        }
    }

    fn write_regs(&self, r: &mut reg::Regs, fd: u32) {
        for (index, (&size, &offset)) in self.sizes.iter().zip(&self.offsets).enumerate() {
            let base = 140 + index * 2;
            r.w[base] = fd;
            r.w[base + 1] = size as u32;
            let _ = offset;
        }
    }

    fn offsets(&self) -> Vec<RegOffsetInfo> {
        self.offsets
            .iter()
            .zip(self.sizes)
            .enumerate()
            .filter(|(_, (offset, size))| **offset != 0 && *size != 0)
            .map(|(index, (offset, _))| RegOffsetInfo {
                reg_idx: (140 + index * 2) as u32,
                offset: *offset as u32,
            })
            .collect()
    }

    fn hardware_sizes(&self) -> Vec<RcbConfig> {
        const PRIORITY: [usize; 11] = [6, 2, 4, 0, 3, 5, 1, 8, 9, 10, 7];
        PRIORITY
            .iter()
            .filter_map(|&index| {
                (self.sizes[index] > 0).then_some(RcbConfig {
                    reg_idx: (140 + index * 2) as u32,
                    size: self.sizes[index] as u32,
                })
            })
            .collect()
    }
}

fn rcb_bytes(bit_size: Option<f64>) -> usize {
    let bits = bit_size.unwrap_or(0.0).ceil() as usize;
    align((bits + 7) / 8, RCB_ALIGNMENT)
}

fn rcb_bits(bits: f64) -> usize {
    rcb_bytes(Some(bits))
}

fn rcb_upper(length: usize, coeff: usize) -> usize {
    2 * align(length * coeff, RCB_ALIGNMENT)
}

fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

pub struct H264d383 {
    svc: MppService,
    cabac: DmaBuf,
    global: DmaBuf,
    rps: DmaBuf,
    rcb: DmaBuf,
    rcb_info_supported: bool,
    regs: reg::Regs,
    pub last_status: (u32, u32, u32),
    pub last_hardware_error: bool,
    offsets_scratch: Vec<RegOffsetInfo>,
    rcb_scratch: Vec<RcbConfig>,
    batch: Batch,
}

impl H264d383 {
    pub fn open() -> io::Result<Self> {
        crate::service::soc::require_video_decode(crate::service::soc::VideoDecodeHal::Vdpu383)?;
        let svc = MppService::open(Client::RkvDec as u32)?;
        let rcb_info_supported = svc.cmd_supported(MPP_CMD_SET_RCB_INFO);
        let mut cabac = DmaBuf::alloc("system", CABAC_SIZE, false)?;
        cabac.sync_write_begin()?;
        for (destination, source) in cabac
            .as_mut_slice()
            .iter_mut()
            .zip(RKV_CABAC_TABLE.iter().flat_map(|word| word.to_le_bytes()))
        {
            *destination = source;
        }
        cabac.sync_write_end()?;
        let mut hw = Self {
            svc,
            cabac,
            global: DmaBuf::alloc("system", hw383::GLOBAL_CFG_BYTES, false)?,
            rps: DmaBuf::alloc("system", hw383::RPS_BYTES, false)?,
            rcb: DmaBuf::alloc("system", 1 << 20, false)?,
            rcb_info_supported,
            regs: reg::Regs::new(),
            last_status: (0, 0, 0),
            last_hardware_error: false,
            offsets_scratch: Vec::with_capacity(10),
            rcb_scratch: Vec::with_capacity(11),
            batch: Batch::new(),
        };
        hw.init_common();
        Ok(hw)
    }

    pub fn recover_session(&mut self) -> io::Result<()> {
        self.svc.restart_after_hardware_error()
    }

    fn init_common(&mut self) {
        let r = &mut self.regs;
        r.set(reg::w8::REG8_DEC_MODE, 1)
            .set(reg::w9::FBC_E, 0)
            .set(reg::w9::TILE_E, 0)
            .set(reg::w9::BUF_EMPTY_EN, 0)
            .set(reg::w10::STRMD_AUTO_GATING_E, 1)
            .set(reg::w10::INTER_AUTO_GATING_E, 1)
            .set(reg::w10::INTRA_AUTO_GATING_E, 1)
            .set(reg::w10::TRANSD_AUTO_GATING_E, 1)
            .set(reg::w10::RECON_AUTO_GATING_E, 1)
            .set(reg::w10::FILTERD_AUTO_GATING_E, 1)
            .set(reg::w10::BUS_AUTO_GATING_E, 1)
            .set(reg::w10::CTRL_AUTO_GATING_E, 1)
            .set(reg::w10::RCB_AUTO_GATING_E, 1)
            .set(reg::w10::ERR_PRC_AUTO_GATING_E, 1)
            .set(reg::w16::ERROR_PROC_DISABLE, 1)
            .set(reg::w16::ERROR_SPREAD_DISABLE, 0)
            .set(reg::w16::ROI_ERROR_CTU_CAL_EN, 0)
            .set(reg::w28::AXI_PERF_WORK_E, 1)
            .set(reg::w28::AXI_CNT_TYPE, 1)
            .set(reg::w28::RD_LATENCY_ID, 11)
            .set(reg::w29::ADDR_ALIGN_TYPE, 1)
            .set(reg::w29::AR_CNT_ID_TYPE, 0)
            .set(reg::w29::AW_CNT_ID_TYPE, 1)
            .set(reg::w29::AR_COUNT_ID, 17)
            .set(reg::w29::AW_COUNT_ID, 0)
            .set(reg::w29::RD_BAND_WIDTH_MODE, 0)
            .set(reg::w30::AXI_WR_QOS, 0)
            .set(reg::w30::AXI_RD_QOS, 0);
        r.w[13] = 0x00ff_ffff;
        r.w[20] = 0xfffe_dfff;
        r.w[21] = 0x0ffb_f9ff;
    }

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
        _ref_pocs: &[i32],
        _ref_flags: &[Option<ReferenceFlags>],
        ref_frame_nums: &[u32],
        stream: &DmaBuf,
        stream_len: usize,
        _idr: bool,
    ) -> io::Result<()> {
        super::check_decode_geometry(cur.width, cur.height)?;
        let global = hw383::build_global_cfg(sps, pps, dpb, sh, order);
        let rps = hw383::build_rps(sps, sh, dpb, lists);
        self.global.sync_write_begin()?;
        self.global.as_mut_slice()[..global.len()].copy_from_slice(&global);
        self.global.sync_write_end()?;
        self.rps.sync_write_begin()?;
        self.rps.as_mut_slice()[..rps.len()].copy_from_slice(&rps);
        self.rps.sync_write_end()?;

        let mbaff = !sps.frame_mbs_only && sps.mb_adaptive_frame_field && !sh.field_pic;
        let rcb = Rcb383::compute(
            cur.width,
            cur.height,
            mbaff,
            sps.bit_depth_luma as usize,
            sps.chroma_format_idc,
        );
        if rcb.total > self.rcb.len() {
            self.rcb = DmaBuf::alloc("system", rcb.total, false)?;
        }

        let present = std::array::from_fn(|index| refs.get(index).copied().flatten().is_some());
        let resolved = resolve_reference_slots(&present);
        let mut error_ref = cur;
        let mut min_frame_num = 0u32;
        let r = &mut self.regs;
        for index in 0..16 {
            let reference = if resolved[index] == 16 {
                cur
            } else {
                refs[resolved[index]].expect("resolved VDPU383 reference")
            };
            r.w[170 + index] = reference.nv12_fd();
            r.w[195 + index] = reference.nv12_fd();
            r.w[217 + index] = reference.colmv_fd();
            if let Some(frame_num) = ref_frame_nums.get(index) {
                if *frame_num < sh.frame_num && *frame_num > min_frame_num {
                    min_frame_num = *frame_num;
                    error_ref = reference;
                }
            }
        }

        let rcb_fd = self.rcb.fd() as u32;
        rcb.write_regs(r, rcb_fd);
        r.set(reg::w66::REG66_STREAM_LEN, stream_len as u32)
            .set(reg::w67::REG67_GLOBAL_LEN, 11)
            .set(reg::w68::REG68_HOR_VIRSTRIDE, (cur.hor_stride / 16) as u32)
            .set(
                reg::w69::REG69_RASTER_UV_HOR_VIRSTRIDE,
                (cur.hor_stride / 16) as u32,
            )
            .set(
                reg::w70::REG70_Y_VIRSTRIDE,
                (cur.hor_stride * cur.ver_stride / 16) as u32,
            )
            .set(
                reg::w80::REG80_ERROR_REF_HOR_VIRSTRIDE,
                (cur.hor_stride / 16) as u32,
            )
            .set(
                reg::w81::REG81_ERROR_REF_RASTER_UV_HOR_VIRSTRIDE,
                (cur.hor_stride / 16) as u32,
            )
            .set(
                reg::w82::REG82_ERROR_REF_VIRSTRIDE,
                (cur.hor_stride * cur.ver_stride / 16) as u32,
            )
            .set(reg::w128::REG128_STRM_BASE, stream.fd() as u32)
            .set(reg::w129::REG129_RPS_BASE, self.rps.fd() as u32)
            .set(reg::w130::REG130_CABACTBL_BASE, self.cabac.fd() as u32)
            .set(reg::w131::REG131_GBL_BASE, self.global.fd() as u32)
            .set(reg::w132::REG132_SCANLIST_ADDR, 0)
            .set(reg::w168::REG168_DECOUT_BASE, cur.nv12_fd())
            .set(reg::w192::REG192_PAYLOAD_ST_CUR_BASE, cur.nv12_fd())
            .set(reg::w216::REG216_COLMV_CUR_BASE, cur.colmv_fd())
            .set(reg::w169::REG169_ERROR_REF_BASE, error_ref.nv12_fd())
            .set(reg::w193::REG193_FBC_PAYLOAD_OFFSET, 0);

        self.offsets_scratch.clear();
        self.offsets_scratch.extend(rcb.offsets());
        self.rcb_scratch.clear();
        self.rcb_scratch.extend(rcb.hardware_sizes());
        if std::env::var_os("RFI_H264383_DUMP").is_some() {
            let task = DUMP_INDEX.fetch_add(1, Ordering::Relaxed);
            let _ = std::fs::write(format!("/tmp/rust_h264383_global_{task:02}.bin"), &global);
            let _ = std::fs::write(format!("/tmp/rust_h264383_rps_{task:02}.bin"), &rps);
            let regs: String =
                r.w.iter()
                    .enumerate()
                    .map(|(index, word)| format!("{index:03} {word:08x}\n"))
                    .collect();
            let _ = std::fs::write(format!("/tmp/rust_h264383_regs_{task:02}.txt"), regs);
        }
        let mut readback = [0u32; REG_READBACK_WORDS];
        self.batch.clear();
        self.batch
            .reg_write(&r.w[8..64], 8 * 4)
            .reg_write(&r.w[64..107], 64 * 4)
            .reg_write(&r.w[128..233], 128 * 4)
            .reg_read(&mut readback[15..16], 15 * 4)
            .reg_read(&mut readback[320..360], 320 * 4);
        self.batch.reg_offsets(&self.offsets_scratch);
        if self.rcb_info_supported && !self.rcb_scratch.is_empty() {
            self.batch.rcb_info(&self.rcb_scratch);
        }
        self.svc.submit_batch_recovering(&mut self.batch)?;
        self.svc.poll_hw_finish_recovering()?;
        let status = readback[15];
        self.last_status = (status, readback[320], readback[356]);
        self.last_hardware_error = status & 1 == 0 || status & 0x7e != 0;
        if self.last_hardware_error {
            let _ = self.svc.restart_after_hardware_error();
        }
        Ok(())
    }
}

fn resolve_reference_slots(present: &[bool; 16]) -> [usize; 16] {
    let mut nearest = 16usize;
    std::array::from_fn(|index| {
        if present[index] {
            nearest = index;
        }
        nearest
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rcb_offsets_follow_the_vdpu383_region_order() {
        let rcb = Rcb383::compute(640, 480, false, 8, 1);
        assert!(rcb.total >= 4096);
        assert_eq!(
            rcb.sizes,
            [0, 0, 0x740, 0x740, 0xc80, 0, 0x4140, 0x4140, 0, 0, 0]
        );
        assert_eq!(
            rcb.offsets,
            [
                0, 0xf00, 0x1e00, 0x3c00, 0x5a00, 0x7300, 0x8c00, 0x1b800, 0x2e400, 0x41000,
                0x57800
            ]
        );
        assert_eq!(rcb.offsets[0], 0);
        for index in 1..rcb.offsets.len() {
            let expected = rcb.offsets[index - 1] + rcb.upper_sizes[index - 1];
            assert_eq!(rcb.offsets[index], expected);
        }
    }
}
