//! VDPU383 H.265 decode HAL for RK3576.
//!
//! The codec parser and RPS tables are shared with the RK3588 path. VDPU383
//! changes the common register/RCB layout and uses a 176-byte dynamic
//! SPS/PPS/POC packet.

use std::io;

use crate::codec::hevc::hw::{self, PPS_PACKET_383_SIZE};
use crate::codec::hevc::parser::{Pps, SliceHeader, Sps};
use crate::hal::hevc::Frame;
use crate::hal::vdpu383_regs as reg;
use crate::service::dma_heap::DmaBuf;
use crate::service::{
    Batch, Client, MppService, RcbInfo as RcbConfig, RegOffsetInfo, MPP_CMD_SET_RCB_INFO,
};

const CABAC_TABLE: &[u8; 27456] = include_bytes!("../../assets/hevc_cabac_table.bin");
const CABAC_ALIGNED: usize = 7 * 4096;
const SPSPPS_ALIGNED: usize = 4096;
const RPS_SIZE: usize = 400 * 8;
const RPS_ALIGNED: usize = 4096;
const SCALING_LIST_BUFFER_SIZE: usize = 81 * hw::SCALING_LIST_TABLE_SIZE;
const SCALING_LIST_ALIGNED: usize = (SCALING_LIST_BUFFER_SIZE + 4095) / 4096 * 4096;
const SPSPPS_OFFSET: usize = CABAC_ALIGNED;
const RPS_OFFSET: usize = SPSPPS_OFFSET + SPSPPS_ALIGNED;
const SCALING_LIST_OFFSET: usize = RPS_OFFSET + RPS_ALIGNED;
const BUFS_SIZE: usize = SCALING_LIST_OFFSET + SCALING_LIST_ALIGNED;
const RCB_ALIGNMENT: usize = 64;
const REG_READBACK_WORDS: usize = 360;
pub const JOB_SLOTS: usize = 4;

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
        chroma_format_idc: u32,
        bit_depth: usize,
        ctu: usize,
        tile_rows: u32,
        tile_cols: u32,
    ) -> Self {
        let width = align(width, ctu);
        let height = align(height, ctu);
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

        let row_uv = match chroma_format_idc {
            1 | 2 => 1,
            3 => 3,
            _ => 0,
        };
        let col_uv = match chroma_format_idc {
            1 => 1,
            2 | 3 => 3,
            _ => 0,
        };
        let tile_row_cut = tile_rows.saturating_sub(1) as usize;
        let tile_col_cut = tile_cols.saturating_sub(1) as usize;
        let ext_row = tile_row_cut * 64 * 8;
        let ext_col = tile_col_cut * 64 * 8;
        let filterd_append = if width > 4096 { 27648 } else { 8192 };

        let mut sizes = [0usize; 11];
        sizes[0] = 0;
        sizes[1] = 0;
        let inter = rcb_bits(((width + 7) / 8 * 174) as f64);
        sizes[2] = inter;
        sizes[3] = if tile_row_cut > 0 {
            rcb_bits((((width + 7) / 8 * 174) + ext_row) as f64)
        } else {
            0
        };
        let mut intra_bits = align(width, 512) * (bit_depth + 2) * 4;
        sizes[4] = rcb_bits(intra_bits as f64);
        intra_bits += ext_row;
        sizes[5] = if tile_row_cut > 0 {
            rcb_bits(intra_bits as f64)
        } else {
            0
        };
        let filterd_bits =
            align(width, 64) as f64 * (1.6 * bit_depth as f64 + 0.5) * (8.0 + 5.0 * row_uv as f64);
        sizes[6] = rcb_bits(filterd_bits / 2.0) + filterd_append;
        sizes[7] = sizes[6];
        sizes[8] = if tile_row_cut > 0 {
            rcb_bits(filterd_bits + ext_row as f64)
        } else {
            0
        };
        sizes[9] = if tile_col_cut > 0 {
            let bits = align(height, 64) as f64
                * (1.6 * bit_depth as f64 + 0.5)
                * (16.5 + 5.0 * col_uv as f64)
                + ext_col as f64;
            rcb_bits(bits)
        } else {
            0
        };
        sizes[10] = 0;

        Self {
            sizes,
            upper_sizes,
            offsets,
            total: align(total.max(1), 4096),
        }
    }

    fn write_regs(&self, r: &mut reg::Regs, fd: u32) {
        for (index, &size) in self.sizes.iter().enumerate() {
            let base = 140 + index * 2;
            r.w[base] = fd;
            r.w[base + 1] = size as u32;
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

fn rcb_bits(bits: f64) -> usize {
    let bits = bits.ceil() as usize;
    align((bits + 7) / 8, RCB_ALIGNMENT)
}

fn rcb_upper(length: usize, coeff: usize) -> usize {
    2 * align(length * coeff, RCB_ALIGNMENT)
}

fn scaling_list_slot(sps: &Sps, pps: &Pps) -> usize {
    if pps.scaling_list.is_some() {
        pps.pps_id as usize + 16
    } else {
        sps.sps_id as usize
    }
}

fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

pub struct Hevc383 {
    svc: MppService,
    slots: Vec<JobSlot383>,
    slot: usize,
    armed: Vec<usize>,
    depth: usize,
    pipeline: bool,
    rcb_info_supported: bool,
    dump_index: usize,
    regs: reg::Regs,
    pub last_status: (u32, u32, u32),
    pub last_hardware_error: bool,
}

struct JobSlot383 {
    bufs: DmaBuf,
    rcb: DmaBuf,
    scaling_list_offset: Option<u32>,
    reg_words: Vec<u32>,
    offsets: Vec<RegOffsetInfo>,
    rcb_scratch: Vec<RcbConfig>,
    batch: Batch,
    readback: [u32; REG_READBACK_WORDS],
}

impl Hevc383 {
    pub fn open() -> io::Result<Self> {
        crate::service::soc::require_video_decode(crate::service::soc::VideoDecodeHal::Vdpu383)?;
        let svc = MppService::open(Client::RkvDec as u32)?;
        let rcb_info_supported = svc.cmd_supported(MPP_CMD_SET_RCB_INFO);
        let slots = (0..JOB_SLOTS)
            .map(|_| {
                let mut bufs = DmaBuf::alloc("system", BUFS_SIZE, false)?;
                bufs.sync_write_begin()?;
                bufs.as_mut_slice()[..CABAC_TABLE.len()].copy_from_slice(CABAC_TABLE);
                bufs.sync_write_end()?;
                Ok(JobSlot383 {
                    bufs,
                    rcb: DmaBuf::alloc("system", 1 << 20, false)?,
                    scaling_list_offset: None,
                    reg_words: vec![0u32; reg::N_WORDS],
                    offsets: Vec::with_capacity(16),
                    rcb_scratch: Vec::with_capacity(11),
                    batch: Batch::new(),
                    readback: [0u32; REG_READBACK_WORDS],
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let mut hw = Self {
            svc,
            slots,
            slot: 0,
            armed: Vec::with_capacity(JOB_SLOTS),
            depth: 3,
            pipeline: false,
            rcb_info_supported,
            dump_index: 0,
            regs: reg::Regs::new(),
            last_status: (0, 0, 0),
            last_hardware_error: false,
        };
        hw.init_common();
        Ok(hw)
    }

    fn init_common(&mut self) {
        let r = &mut self.regs;
        r.set(reg::w8::REG8_DEC_MODE, 0)
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
        r.w[13] = 0xffff;
        r.w[20] = 0xffff_ffff;
        r.w[21] = 0x3ff3_f9ff;
    }

    fn prepare_tables(
        &mut self,
        slot: usize,
        sps: &Sps,
        pps: &Pps,
        poc: i32,
        ref_pocs: &[i32],
        ref_errors: &[bool],
        ref_present: &[bool],
        task: usize,
    ) {
        let packet = hw::build_pps_packet_383(sps, pps, poc, ref_pocs, ref_errors, ref_present);
        let rps = hw::build_sps_rps_from_sps(sps);
        let scaling = hw::build_scaling_list_packet_383(sps, pps);
        let buf = self.slots[slot].bufs.as_mut_slice();
        buf[SPSPPS_OFFSET..SPSPPS_OFFSET + PPS_PACKET_383_SIZE].copy_from_slice(&packet);
        buf[RPS_OFFSET..RPS_OFFSET + RPS_SIZE].copy_from_slice(&rps[..RPS_SIZE]);
        self.slots[slot].scaling_list_offset = scaling.map(|table| {
            let slot = scaling_list_slot(sps, pps);
            let offset = SCALING_LIST_OFFSET + slot * hw::SCALING_LIST_TABLE_SIZE;
            buf[offset..offset + table.len()].copy_from_slice(&table);
            offset as u32
        });
        if std::env::var_os("RFI_HEVC383_DUMP").is_some() {
            let _ = std::fs::write(format!("/tmp/rust_h265383_pps_{task:02}.bin"), packet);
            let _ = std::fs::write(format!("/tmp/rust_h265383_rps_{task:02}.bin"), rps);
        }
    }

    pub fn decode(
        &mut self,
        sps: &Sps,
        pps: &Pps,
        _sh: &SliceHeader,
        poc: i32,
        cur: &Frame,
        refs: &[Option<&Frame>],
        ref_pocs: &[i32],
        ref_errors: &[bool],
        stream: &DmaBuf,
        stream_len: usize,
        _slice_count: u32,
        _nal_type: u8,
    ) -> io::Result<()> {
        super::check_decode_geometry(cur.width, cur.height)?;
        if self.armed.len() >= self.depth {
            self.reap()?;
        }
        let slot = self.slot;
        let task = self.dump_index;
        self.dump_index = self.dump_index.saturating_add(1);
        let present: [bool; 16] = std::array::from_fn(|i| refs.get(i).copied().flatten().is_some());
        self.slots[slot].bufs.sync_write_begin()?;
        self.prepare_tables(slot, sps, pps, poc, ref_pocs, ref_errors, &present, task);
        self.slots[slot].bufs.sync_write_end()?;

        let ctu = 1usize << (sps.log2_min_cb + sps.log2_diff_max_min_cb);
        let bit_depth = sps.bit_depth_luma.max(sps.bit_depth_chroma) as usize;
        let rcb = Rcb383::compute(
            cur.width,
            cur.height,
            sps.chroma_format_idc,
            bit_depth,
            ctu,
            pps.tile_rows,
            pps.tile_columns,
        );
        if rcb.total > self.slots[slot].rcb.len() {
            self.slots[slot].rcb = DmaBuf::alloc("system", rcb.total, false)?;
        }
        let rcb_fd = self.slots[slot].rcb.fd() as u32;

        let error_ref = (0..16)
            .filter(|&i| present[i] && !ref_errors.get(i).copied().unwrap_or(true))
            .min_by_key(|&i| (ref_pocs.get(i).copied().unwrap_or(0) - poc).abs())
            .and_then(|i| refs.get(i).copied().flatten())
            .unwrap_or(cur);

        let r = &mut self.regs;
        rcb.write_regs(r, rcb_fd);
        let padded = align(stream_len, 16) + 64;
        let uv_stride = if sps.chroma_format_idc == 3 {
            cur.hor_stride * 2
        } else {
            cur.hor_stride
        };
        r.set(reg::w66::REG66_STREAM_LEN, padded as u32)
            .set(reg::w67::REG67_GLOBAL_LEN, 12)
            .set(reg::w68::REG68_HOR_VIRSTRIDE, (cur.hor_stride / 16) as u32)
            .set(
                reg::w69::REG69_RASTER_UV_HOR_VIRSTRIDE,
                (uv_stride / 16) as u32,
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
                (uv_stride / 16) as u32,
            )
            .set(
                reg::w82::REG82_ERROR_REF_VIRSTRIDE,
                (cur.hor_stride * cur.ver_stride / 16) as u32,
            )
            .set(reg::w128::REG128_STRM_BASE, stream.fd() as u32)
            .set(
                reg::w129::REG129_RPS_BASE,
                self.slots[slot].bufs.fd() as u32,
            )
            .set(reg::w130::REG130_CABACTBL_BASE, 0)
            .set(
                reg::w131::REG131_GBL_BASE,
                self.slots[slot].bufs.fd() as u32,
            )
            .set(
                reg::w132::REG132_SCANLIST_ADDR,
                self.slots[slot]
                    .scaling_list_offset
                    .map_or(0, |_| self.slots[slot].bufs.fd() as u32),
            )
            .set(reg::w168::REG168_DECOUT_BASE, cur.video_fd())
            .set(reg::w192::REG192_PAYLOAD_ST_CUR_BASE, cur.video_fd())
            .set(reg::w216::REG216_COLMV_CUR_BASE, cur.colmv_fd())
            .set(reg::w169::REG169_ERROR_REF_BASE, error_ref.video_fd());
        for i in 0..16 {
            let source = refs
                .get(i)
                .copied()
                .flatten()
                .filter(|_| !ref_errors.get(i).copied().unwrap_or(true))
                .unwrap_or(error_ref);
            r.w[170 + i] = source.video_fd();
            r.w[195 + i] = source.video_fd();
            r.w[217 + i] = source.colmv_fd();
        }
        if std::env::var_os("RFI_HEVC383_DUMP").is_some() {
            let text: String =
                r.w.iter()
                    .enumerate()
                    .map(|(index, word)| format!("{index:03} {word:08x}\n"))
                    .collect();
            let _ = std::fs::write(format!("/tmp/rust_h265383_regs_{task:02}.txt"), text);
        }

        self.slots[slot].reg_words.copy_from_slice(&r.w);
        let scaling_list_offset = self.slots[slot].scaling_list_offset;
        let rcb_info_supported = self.rcb_info_supported;
        let JobSlot383 {
            reg_words,
            offsets,
            rcb_scratch,
            batch,
            readback,
            ..
        } = &mut self.slots[slot];
        offsets.clear();
        offsets.extend(rcb.offsets());
        offsets.push(RegOffsetInfo {
            reg_idx: 129,
            offset: RPS_OFFSET as u32,
        });
        offsets.push(RegOffsetInfo {
            reg_idx: 131,
            offset: SPSPPS_OFFSET as u32,
        });
        if let Some(offset) = scaling_list_offset {
            offsets.push(RegOffsetInfo {
                reg_idx: 132,
                offset,
            });
        }
        rcb_scratch.clear();
        rcb_scratch.extend(rcb.hardware_sizes());
        batch.clear();
        batch
            .reg_write(&reg_words[8..64], 8 * 4)
            .reg_write(&reg_words[64..128], 64 * 4)
            .reg_write(&reg_words[128..233], 128 * 4)
            .reg_read(&mut readback[15..16], 15 * 4)
            .reg_read(&mut readback[320..360], 320 * 4);
        batch.reg_offsets(offsets);
        if rcb_info_supported && !rcb_scratch.is_empty() {
            batch.rcb_info(rcb_scratch);
        }
        self.svc
            .submit_batch_recovering(&mut self.slots[slot].batch)?;
        self.armed.push(slot);
        self.slot = (slot + 1) % JOB_SLOTS;
        if !self.pipeline {
            self.reap()?;
        }
        Ok(())
    }

    pub fn set_pipeline(&mut self, enabled: bool) {
        self.pipeline = enabled;
    }

    pub fn set_depth(&mut self, depth: usize) {
        self.depth = depth.clamp(1, JOB_SLOTS);
    }

    pub fn in_flight(&self) -> usize {
        self.armed.len()
    }

    pub fn reap(&mut self) -> io::Result<bool> {
        if self.armed.is_empty() {
            return Ok(false);
        }
        static BLOCKING: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let blocking = *BLOCKING.get_or_init(|| {
            std::env::var("RKMPP_POLL_BLOCKING").as_deref() != Ok("0") && self.pipeline
                || std::env::var("RKMPP_POLL_BLOCKING").as_deref() == Ok("1")
        });
        if blocking {
            self.svc.poll_hw_finish_blocking()?;
        } else {
            self.svc.poll_hw_finish_recovering()?;
        }
        let slot = self.armed.remove(0);
        let readback = self.slots[slot].readback;
        self.last_status = (readback[15], readback[320], readback[356]);
        self.last_hardware_error = readback[15] & 1 == 0 || readback[15] & 0x7e != 0;
        if self.last_hardware_error {
            let _ = self.svc.restart_after_hardware_error();
        }
        Ok(self.last_hardware_error)
    }

    pub fn flush(&mut self) -> io::Result<bool> {
        let mut failed = false;
        while !self.armed.is_empty() {
            failed |= self.reap()?;
        }
        Ok(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hevc_rcb_uses_the_vdpu383_upper_layout_and_refined_sizes() {
        let rcb = Rcb383::compute(320, 240, 1, 8, 64, 1, 1);
        assert_eq!(rcb.sizes[2], rcb_bits((((320 + 7) / 8) * 174) as f64));
        assert_eq!(rcb.sizes[3], 0);
        assert_eq!(rcb.sizes[5], 0);
        assert_eq!(rcb.sizes[8], 0);
        assert_eq!(rcb.sizes[9], 0);
        assert!(rcb.upper_sizes.iter().sum::<usize>() <= rcb.total);
    }

    #[test]
    fn hevc_scaling_lists_use_separate_sps_and_pps_slots() {
        let mut sps = Sps {
            sps_id: 7,
            ..Default::default()
        };
        let mut pps = Pps {
            pps_id: 5,
            ..Default::default()
        };
        assert_eq!(scaling_list_slot(&sps, &pps), 7);

        pps.scaling_list = Some(crate::codec::hevc::parser::ScalingList::default_hevc());
        assert_eq!(scaling_list_slot(&sps, &pps), 21);
        sps.sps_id = 8;
        assert_eq!(scaling_list_slot(&sps, &pps), 21);
    }
}
