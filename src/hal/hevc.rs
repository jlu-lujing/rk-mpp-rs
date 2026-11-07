//! rkvdec (vdpu34x) HEVC decode HAL for RK3588.
//!
//! Port of `hal/rkdec/h265d/hal_h265d_vdpu34x.c::hal_h265d_vdpu34x_gen_regs`:
//! the driver receives the 276-word register file, the 112-byte SPS/PPS packet
//! replicated 64 times, the RPS table and the bitstream. Register semantics
//! follow the C source; the tables come from `codec::hevc::hw`.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::codec::hevc::hw;
use crate::codec::hevc::parser::{Pps, SliceHeader, Sps};
use crate::hal::vdpu34x_regs as reg;
use crate::service::buffer_pool::{BufferLease, BufferPool, PoolError};
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{require_video_decode, VideoDecodeHal};
use crate::service::{
    Client, MppService, RcbInfo as RcbConfig, RegOffsetInfo, MPP_CMD_SET_RCB_INFO,
};

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
const SCALING_LIST_OFFSET: usize = (RPS_OFFSET + hw::RPS_TABLE_SIZE + 4095) & !4095;
const BUFS_SIZE: usize = SCALING_LIST_OFFSET + hw::SCALING_LIST_TABLE_SIZE + 128;

/// HEVC chroma sampling. The numeric values are the SPS
/// `chroma_format_idc` values consumed by the vdpu34x packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ChromaFormat {
    Monochrome = 0,
    Yuv420 = 1,
    Yuv422 = 2,
    Yuv444 = 3,
}

impl ChromaFormat {
    pub fn from_idc(idc: u32) -> Option<Self> {
        match idc {
            0 => Some(Self::Monochrome),
            1 => Some(Self::Yuv420),
            2 => Some(Self::Yuv422),
            3 => Some(Self::Yuv444),
            _ => None,
        }
    }

    pub(crate) fn uv_size_factor(self) -> usize {
        match self {
            Self::Monochrome => 0,
            Self::Yuv420 => 1,
            Self::Yuv422 => 2,
            Self::Yuv444 => 4,
        }
    }
}

/// One decoded picture plus its colmv buffer. `video` contains the luma plane
/// followed by the chroma planes in the layout emitted by vdpu34x.
pub struct Frame {
    pub video: BufferLease<DmaBuf>,
    pub colmv: BufferLease<DmaBuf>,
    pub width: usize,
    pub height: usize,
    pub hor_stride: usize,
    pub ver_stride: usize,
    pub bit_depth: u32,
    pub chroma_format: ChromaFormat,
}

impl Frame {
    pub fn acquire(
        video_pool: &BufferPool<DmaBuf>,
        colmv_pool: &BufferPool<DmaBuf>,
        width: usize,
        height: usize,
        ctu: usize,
        bit_depth: u32,
        chroma_format: ChromaFormat,
    ) -> Result<Frame, PoolError> {
        let (hor_stride, ver_stride, frame_size) =
            frame_layout(width, height, bit_depth, chroma_format);
        let video = video_pool.acquire_lease_reclaiming_idle(frame_size)?;
        let colmv = colmv_pool.acquire_lease_reclaiming_idle(colmv_size(width, height, ctu))?;
        Ok(Frame {
            video,
            colmv,
            width,
            height,
            hor_stride,
            ver_stride,
            bit_depth,
            chroma_format,
        })
    }

    pub fn video_fd(&self) -> u32 {
        self.video
            .fd()
            .expect("pooled HEVC frame has a dma-buf fd") as u32
    }

    pub fn colmv_fd(&self) -> u32 {
        self.colmv
            .fd()
            .expect("pooled HEVC colmv has a dma-buf fd") as u32
    }

    pub fn alloc(
        width: usize,
        height: usize,
        ctu: usize,
        bit_depth: u32,
        chroma_format: ChromaFormat,
    ) -> io::Result<Frame> {
        let video_pool = BufferPool::new(1);
        let colmv_pool = BufferPool::new(1);
        Frame::acquire(
            &video_pool,
            &colmv_pool,
            width,
            height,
            ctu,
            bit_depth,
            chroma_format,
        )
        .map_err(|error| io::Error::other(format!("HEVC frame pool: {error}")))
    }
}

/// MPP aligns coded width to 64 pixels, then converts the bit depth to a byte
/// stride. Ten-bit pixels are packed at 10 bits each, not expanded to P010.
fn frame_layout(
    width: usize,
    height: usize,
    bit_depth: u32,
    chroma_format: ChromaFormat,
) -> (usize, usize, usize) {
    let hor_stride = (align(width, 64) * bit_depth as usize + 7) / 8;
    let ver_stride = align(height, 16);
    let y_size = hor_stride * ver_stride;
    let frame_size = y_size + y_size * chroma_format.uv_size_factor() / 2;
    (hor_stride, ver_stride, frame_size)
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

const RCB_ALIGNMENT: usize = 64;
const RCB_REGION_COUNT: usize = 10;
const RCB_BUFFER_COUNT: usize = 3;
static COLMV_DUMP_INDEX: AtomicUsize = AtomicUsize::new(0);

/// RCB regions in `vdpu34x_get_rcb_buf_size` order.
///
/// `size` fields are refined per picture, while `offsets` and
/// `allocation_size` are fixed by the common vdpu34x layout. MPP computes the
/// latter first and only then shrinks each region, so offsets must never be
/// re-packed from the refined sizes.
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
        let inter_col = if tile_col_cut > 0 {
            rcb_bytes(h * 22)
        } else {
            0
        };
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
        let common_sizes = [
            rcb_aligned_size(width * 22),
            rcb_aligned_size(width * 6),
            rcb_aligned_size(width),
            rcb_aligned_size(width * 3),
            rcb_aligned_size(width * 6),
            rcb_aligned_size(width * 6),
            rcb_aligned_size(width * 11),
            rcb_aligned_size(height),
            rcb_aligned_size(height * 3),
            rcb_aligned_size(height * 67),
        ];
        let mut offsets = [0u32; RCB_REGION_COUNT];
        let mut allocation_size = 0usize;
        for (offset, size) in offsets.iter_mut().zip(common_sizes) {
            *offset = allocation_size as u32;
            allocation_size += size;
        }
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
            offsets,
            allocation_size,
        }
    }

    pub fn total(&self) -> usize {
        self.allocation_size
    }

    /// (register index, byte offset) pairs, identical ordering to H.264.
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

pub struct HevcHw {
    svc: MppService,
    bufs: DmaBuf,
    rcbs: Vec<DmaBuf>,
    rcb_index: usize,
    rcb_info: RcbInfo,
    rcb_info_supported: bool,
    pub regs: reg::Regs,
    pub last_status: (u32, u32, u32),
    pub last_hardware_error: bool,
    scaling_list_offset: Option<u32>,
    /// Reused across frames: the ioctl needs owned scratch but not fresh
    /// allocations, and the entries are rebuilt in full every job.
    offsets_scratch: Vec<RegOffsetInfo>,
    rcb_scratch: Vec<RcbConfig>,
    batch: crate::service::Batch,
}

impl HevcHw {
    pub fn open() -> io::Result<HevcHw> {
        require_video_decode(VideoDecodeHal::Vdpu34x)?;
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
        let rcb_info_supported = svc.cmd_supported(MPP_CMD_SET_RCB_INFO);
        let mut bufs = DmaBuf::alloc("system", BUFS_SIZE, false)?;
        let n = bufs.as_mut_slice().len().min(CABAC_TABLE.len());
        bufs.as_mut_slice()[CABAC_OFFSET..CABAC_OFFSET + n].copy_from_slice(&CABAC_TABLE[..n]);
        let mut rcbs = Vec::with_capacity(RCB_BUFFER_COUNT);
        for _ in 0..RCB_BUFFER_COUNT {
            rcbs.push(DmaBuf::alloc("system", 1 << 20, false)?);
        }
        let mut hw = HevcHw {
            svc,
            bufs,
            rcbs,
            rcb_index: 0,
            rcb_info: RcbInfo::default(),
            rcb_info_supported,
            regs: reg::Regs::new(),
            last_status: (0, 0, 0),
            last_hardware_error: false,
            scaling_list_offset: None,
            offsets_scratch: Vec::with_capacity(13),
            rcb_scratch: Vec::with_capacity(9),
            batch: crate::service::Batch::new(),
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
        setup_statistic(r);
    }

    fn prepare_tables(&mut self, sps: &Sps, pps: &Pps) {
        let pkt = hw::build_pps_packet(sps, pps);
        let buf = self.bufs.as_mut_slice();
        for i in 0..PPS_PACKET_COPIES {
            let off = SPSPPS_OFFSET + i * hw::PPS_PACKET_SIZE;
            buf[off..off + hw::PPS_PACKET_SIZE].copy_from_slice(&pkt);
        }
        let rps = hw::build_sps_rps_from_sps(sps);
        let n = rps.len().min(hw::RPS_TABLE_SIZE);
        buf[RPS_OFFSET..RPS_OFFSET + n].copy_from_slice(&rps[..n]);
        self.scaling_list_offset = hw::build_scaling_list_packet(sps, pps).map(|table| {
            let end = SCALING_LIST_OFFSET + hw::SCALING_LIST_TABLE_SIZE;
            buf[SCALING_LIST_OFFSET..end].copy_from_slice(&table);
            if std::env::var_os("RFI_HEVC_SCALING_DUMP").is_some() {
                let _ = std::fs::write("/tmp/our_hevc_scaling_table.bin", &table);
            }
            SCALING_LIST_OFFSET as u32
        });
    }

    /// Fill the register file for one picture.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_regs(
        &mut self,
        _sps: &Sps,
        pps: &Pps,
        sh: &SliceHeader,
        poc: i32,
        cur: &Frame,
        refs: &[Option<&Frame>],
        ref_pocs: &[i32],
        ref_errors: &[bool],
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

        r.set(reg::w130::REG130_DECOUT_BASE, cur.video_fd())
            .set(reg::w131::REG131_COLMV_CUR_BASE, cur.colmv_fd())
            .set(reg::w161::REG161_PPS_HEAD_BASE, self.bufs.fd() as u32)
            .set(reg::w163::REG163_RPS_BASE, self.bufs.fd() as u32)
            .set(reg::w197::REG197_CABACTBL_BASE, self.bufs.fd() as u32)
            .set(reg::w180::REG180_SCANLIST_BASE, self.bufs.fd() as u32)
            .set(
                reg::w12::SCANLIST_ADDR_VALID_EN,
                self.scaling_list_offset.is_some() as u32,
            );

        let present: Vec<bool> = (0..16)
            .map(|i| refs.get(i).copied().flatten().is_some())
            .collect();
        let error_ref_index = select_error_reference(&ref_pocs, &present, ref_errors, poc);
        let error_ref = error_ref_index
            .and_then(|i| refs.get(i).copied().flatten())
            .unwrap_or(cur);
        // When the current picture is the error reference, MPP tags every
        // unused reference slot with POC high bit 3. The hardware carries
        // this state into the following inter picture.
        let error_ref_is_current = error_ref_index.is_none();
        let mut fallback = cur;
        for i in 0..16usize {
            r.w[67 + i] = ref_pocs.get(i).copied().unwrap_or(0) as u32;
            let flag = reference_valid_field(i);
            let high_poc = reference_high_poc_field(i);
            match refs.get(i).copied().flatten() {
                Some(f) => {
                    r.set(flag, 1);
                    r.set(high_poc, 0);
                    fallback = f;
                }
                None => {
                    r.set(flag, 0);
                    r.set(high_poc, error_ref_is_current as u32 * 3);
                }
            }
            // A present reference keeps its own address unless its error info
            // makes it unusable while another valid error reference exists.
            let source = match refs.get(i).copied().flatten() {
                Some(_) if ref_errors.get(i).copied().unwrap_or(true) => error_ref,
                Some(f) => f,
                None if error_ref_index.is_some() => error_ref,
                None => fallback,
            };
            r.w[164 + i] = source.video_fd();
            r.w[181 + i] = source.colmv_fd();
        }
        r.set(
            reg::w132::REG132_ERROR_REF_BASE,
            error_ref.video_fd(),
        )
        .set(
            reg::w21::ERROR_INTRA_MODE,
            error_intra_mode(error_ref_index.is_some()),
        )
        .set(reg::w128::REG128_RLC_BASE, 0)
        .set(reg::w129::REG129_RLCWRITE_BASE, 0)
        .set(reg::w13::TIMEOUT_MODE, 1)
        .set(reg::w11::BUF_EMPTY_EN, 1);
        // IRAP (IDR_W_RADL / IDR_N_LP) tells the decoder to reset its state.
        let idr = sh.slice_type == 2 && matches!(nal_type, 19 | 20);
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
        ref_errors: &[bool],
        stream: &DmaBuf,
        stream_len: usize,
        slice_count: u32,
        nal_type: u8,
    ) -> io::Result<()> {
        super::check_decode_geometry(cur.width, cur.height)?;
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
        for rcb in &mut self.rcbs {
            if total > rcb.len() {
                *rcb = DmaBuf::alloc("system", align(total, 4096), false)?;
            }
        }
        let rcb_index = self.rcb_index;
        self.rcb_index = (self.rcb_index + 1) % RCB_BUFFER_COUNT;
        self.rcb_info = rcb;
        self.fill_regs(
            sps,
            pps,
            sh,
            poc,
            cur,
            refs,
            ref_pocs,
            ref_errors,
            stream_len,
            slice_count,
            nal_type,
        );

        let r = &mut self.regs;
        if std::env::var_os("RFI_HEVC_REG_DUMP").is_some() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/our_hevc_regs.txt")
            {
                let _ = writeln!(f, "FRAME {poc}");
                for (i, word) in r.w.iter().enumerate().take(205) {
                    let _ = writeln!(f, "{i:02} {word:08x}");
                }
            }
        }
        r.set(reg::w128::REG128_RLC_BASE, stream.fd() as u32)
            .set(reg::w129::REG129_RLCWRITE_BASE, stream.fd() as u32);
        // MPP's fast parser cycles through three RCB buffers. The hardware
        // leaves decoder state in this scratch area, so reusing one buffer
        // immediately for the next picture corrupts the second job.
        let rcb_fd = self.rcbs[rcb_index].fd() as u32;
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

        // The register sections are contiguous in the register file, so the
        // batch points straight into it: same bytes, no per-frame copies.
        self.offsets_scratch.clear();
        self.rcb_info.write_offsets(&mut self.offsets_scratch);
        // cabac sits at offset 0 of the shared buffer, the SPS/PPS packet and
        // the RPS follow at their layout offsets.
        self.offsets_scratch.push(RegOffsetInfo {
            reg_idx: 161,
            offset: SPSPPS_OFFSET as u32,
        });
        self.offsets_scratch.push(RegOffsetInfo {
            reg_idx: 163,
            offset: RPS_OFFSET as u32,
        });
        if let Some(offset) = self.scaling_list_offset {
            self.offsets_scratch.push(RegOffsetInfo {
                reg_idx: 180,
                offset,
            });
        }
        if std::env::var_os("RFI_HEVC_FULL_DUMP").is_some() {
            dump_submission(poc, r, &self.bufs, stream, stream_len);
        }
        let mut readback = [0u32; 278];
        self.batch.clear();
        self.rcb_scratch.clear();
        self.rcb_info.write_hardware_sizes(&mut self.rcb_scratch);
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
        self.svc.submit_batch_recovering(&mut self.batch)?;
        self.svc.poll_hw_finish_recovering()?;
        self.last_status = (readback[224], readback[225], readback[230]);
        self.last_hardware_error = hardware_status_error(readback[224]);
        if self.last_hardware_error {
            let _ = self.svc.restart_after_hardware_error();
        }
        if std::env::var_os("RFI_HEVC_FRAME_DUMP").is_some() {
            let task = COLMV_DUMP_INDEX.fetch_add(1, Ordering::Relaxed);
            let video = cur.video.data().expect("pooled video stays live");
            let _ = video.buffer().sync_read_begin();
            let _ = std::fs::write(
                format!("/tmp/rust_hevc_frame_{task:02}.yuv"),
                &video[..],
            );
            let _ = video.buffer().sync_read_end();
        } else if std::env::var_os("RFI_HEVC_COLMV_DUMP").is_some() {
            use std::io::Write;

            let task = COLMV_DUMP_INDEX.fetch_add(1, Ordering::Relaxed);
            let path = format!("/tmp/rust_hevc_cmv_{task:02}.bin");
            let colmv = cur.colmv.data().expect("pooled colmv stays live");
            let _ = colmv.buffer().sync_read_begin();
            if let Ok(mut file) = std::fs::File::create(path) {
                let _ = file.write_all(&colmv);
            }
            let _ = colmv.buffer().sync_read_end();
        }
        if std::env::var("RFI_HEVC_STATUS").is_ok() {
            eprintln!(
                "STATUS reg224={:08x} reg225={:08x} reg226={:08x} reg227={:08x} reg228={:08x} reg230={:08x} reg231={:08x} reg232={:08x}",
                readback[224],
                readback[225],
                readback[226],
                readback[227],
                readback[228],
                readback[230],
                readback[231],
                readback[232],
            );
        }
        Ok(())
    }
}

fn dump_submission(poc: i32, regs: &reg::Regs, bufs: &DmaBuf, stream: &DmaBuf, stream_len: usize) {
    use std::io::Write;

    let padded = align(stream_len, 16) + 64;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/our_hevc_full_regs.txt")
    {
        let _ = writeln!(
            f,
            "FRAME {poc} raw_len={stream_len} padded={padded} fd={}",
            stream.fd()
        );
        for (i, word) in regs.w.iter().enumerate() {
            let _ = writeln!(f, "{i:03} {word:08x}");
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/our_hevc_stream.bin")
    {
        let _ = writeln!(f, "FRAME {poc} raw_len={stream_len} padded={padded}");
        let _ = f.write_all(&stream.as_slice()[..padded]);
    }
    if std::env::var_os("RFI_HEVC_STREAM_DUMP").is_some() {
        let _ = std::fs::write(
            format!("/tmp/our_hevc_stream_{poc}.bin"),
            &stream.as_slice()[..padded],
        );
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/our_hevc_tables.bin")
    {
        let _ = writeln!(f, "FRAME {poc}");
        let buf = bufs.as_slice();
        let _ = f.write_all(&buf[SPSPPS_OFFSET..SPSPPS_OFFSET + hw::PPS_PACKET_SIZE]);
        let _ = f.write_all(&buf[RPS_OFFSET..RPS_OFFSET + hw::RPS_TABLE_SIZE]);
    }
}

/// MPP's vdpu34x HEVC completion check.
fn hardware_status_error(common: u32) -> bool {
    common & (1 << 4) != 0
        || common & (1 << 6) != 0
        || common & (1 << 3) != 0
        || common & (1 << 2) == 0
}

/// MPP selects the closest POC among present references without error info.
/// Slot order is the DPB order used to populate the hardware reference list.
fn select_error_reference(
    ref_pocs: &[i32],
    present: &[bool],
    errors: &[bool],
    current_poc: i32,
) -> Option<usize> {
    ref_pocs
        .iter()
        .zip(present)
        .zip(errors)
        .enumerate()
        .filter(|(_, ((_, present), error))| **present && !**error)
        .min_by_key(|(_, ((poc, _), _))| poc.abs_diff(current_poc))
        .map(|(index, _)| index)
}

fn error_intra_mode(has_error_reference: bool) -> u32 {
    if has_error_reference {
        0
    } else {
        1
    }
}

/// `vdpu34x_setup_statistic`: pixel-range detection and AXI performance/QoS.
fn setup_statistic(r: &mut reg::Regs) {
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

fn reference_valid_field(index: usize) -> reg::Field {
    reg::Field {
        word: 99 + index / 8,
        lsb: (((index % 8) / 4) * 8 + (index % 4)) as u8,
        width: 1,
    }
}

fn reference_high_poc_field(index: usize) -> reg::Field {
    reg::Field {
        word: 200 + index / 8,
        lsb: ((index % 8) * 4) as u8,
        width: 4,
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
    fn rcb_offsets_do_not_repack_refined_sizes() {
        let r = RcbInfo::compute(1920, 1080, 1, 8, 64, 0);
        let offs = r.offsets();
        assert_eq!(offs.len(), 10);
        assert_eq!((offs[0].reg_idx, offs[9].reg_idx), (139, 142));
        assert_eq!(
            offs.iter().map(|info| info.offset).collect::<Vec<_>>(),
            [0, 42240, 53760, 55680, 61440, 72960, 84480, 105600, 106688, 109952]
        );
        assert_eq!(r.total(), 182336);
        assert_ne!(r.dblk, 42240, "refined size must not define the RCB layout");
    }

    #[test]
    fn rcb_offsets_use_the_fixed_vdpu34x_layout() {
        // vdpu34x_get_rcb_buf_size computes offsets from the fixed per-pixel
        // coefficients before h265d_refine_rcb_size shrinks each region.
        let r = RcbInfo::compute(256, 256, 1, 8, 64, 0);
        let offsets: Vec<u32> = r.offsets().iter().map(|info| info.offset).collect();
        assert_eq!(
            offsets,
            [0, 5632, 7168, 7424, 8192, 9728, 11264, 14080, 14336, 15104]
        );
        assert_eq!(r.total(), 32256);
    }

    #[test]
    fn rcb_sram_config_uses_mpp_priority_order() {
        let r = RcbInfo::compute(256, 256, 1, 8, 64, 0);
        let regs: Vec<u32> = r
            .hardware_sizes()
            .into_iter()
            .map(|cfg| cfg.reg_idx)
            .collect();
        assert_eq!(regs, [139, 133, 140]);
    }

    #[test]
    fn high_bit_depth_frame_layout_matches_the_mpp_packed_stride() {
        assert_eq!(
            frame_layout(256, 256, 8, ChromaFormat::Yuv420),
            (256, 256, 98304)
        );
        assert_eq!(
            frame_layout(256, 256, 10, ChromaFormat::Yuv420),
            (320, 256, 122880)
        );
        assert_eq!(
            frame_layout(1920, 1080, 10, ChromaFormat::Yuv420),
            (2400, 1088, 3916800)
        );
    }

    #[test]
    fn frame_layout_uses_the_mpp_chroma_size_multiplier() {
        assert_eq!(
            frame_layout(256, 256, 8, ChromaFormat::Monochrome),
            (256, 256, 65536)
        );
        assert_eq!(
            frame_layout(256, 256, 8, ChromaFormat::Yuv420),
            (256, 256, 98304)
        );
        assert_eq!(
            frame_layout(256, 256, 8, ChromaFormat::Yuv422),
            (256, 256, 131072)
        );
        assert_eq!(
            frame_layout(256, 256, 8, ChromaFormat::Yuv444),
            (256, 256, 196608)
        );
    }

    #[test]
    fn reference_valid_bits_use_the_hevc_reg99_layout() {
        assert_eq!(
            reference_valid_field(0),
            reg::Field {
                word: 99,
                lsb: 0,
                width: 1
            }
        );
        assert_eq!(
            reference_valid_field(7),
            reg::Field {
                word: 99,
                lsb: 11,
                width: 1
            }
        );
        assert_eq!(
            reference_valid_field(8),
            reg::Field {
                word: 100,
                lsb: 0,
                width: 1
            }
        );
        assert_eq!(
            reference_valid_field(15),
            reg::Field {
                word: 100,
                lsb: 11,
                width: 1
            }
        );
    }

    #[test]
    fn reference_high_poc_bits_use_the_mpp_nibble_layout() {
        assert_eq!(
            reference_high_poc_field(0),
            reg::Field {
                word: 200,
                lsb: 0,
                width: 4,
            }
        );
        assert_eq!(
            reference_high_poc_field(7),
            reg::Field {
                word: 200,
                lsb: 28,
                width: 4,
            }
        );
        assert_eq!(
            reference_high_poc_field(8),
            reg::Field {
                word: 201,
                lsb: 0,
                width: 4,
            }
        );
        assert_eq!(
            reference_high_poc_field(15),
            reg::Field {
                word: 201,
                lsb: 28,
                width: 4,
            }
        );
    }

    #[test]
    fn hardware_status_matches_h265_vdpu34x_error_checks() {
        assert!(!hardware_status_error(1 << 2));
        assert!(hardware_status_error(0));
        assert!(hardware_status_error((1 << 2) | (1 << 3)));
        assert!(hardware_status_error((1 << 2) | (1 << 4)));
        assert!(hardware_status_error((1 << 2) | (1 << 6)));
    }

    #[test]
    fn error_reference_is_the_nearest_valid_picture() {
        let pocs = [0, 2, 0, 0];
        let present = [true, true, false, false];
        let errors = [false, false, false, false];

        assert_eq!(
            select_error_reference(&pocs, &present, &errors, 5),
            Some(1),
            "MPP selects POC 2, not the first or an absent slot"
        );
    }

    #[test]
    fn error_reference_ignores_pictures_with_error_info() {
        let pocs = [0, 2, 4, 0];
        let present = [true, true, true, false];
        let errors = [false, true, true, false];

        assert_eq!(select_error_reference(&pocs, &present, &errors, 5), Some(0));
        assert_eq!(
            select_error_reference(&pocs, &present, &[true, true, true, false], 5),
            None
        );
    }

    #[test]
    fn error_intra_mode_is_disabled_only_with_a_valid_error_reference() {
        assert_eq!(error_intra_mode(true), 0);
        assert_eq!(error_intra_mode(false), 1);
    }

    #[test]
    fn statistic_setup_matches_vdpu34x_mpp_values() {
        let mut regs = reg::Regs::new();

        setup_statistic(&mut regs);

        assert_eq!(regs.get(reg::w11::PIX_RANGE_DETECTION_E), 1);
        assert_eq!(regs.w[256], 0b1011);
        assert_eq!(regs.get(reg::w257::ADDR_ALIGN_TYPE), 1);
        assert_eq!(regs.w[270], 0x3113_00ff);
        assert_eq!(regs.w[271], 0);
    }

    /// The reused-scratch writers must reproduce the original allocating
    /// lists exactly, including the tiles case where the sao and filt_col
    /// regions stop being zero.
    #[test]
    fn rcb_scratch_lists_match_the_original_allocating_lists() {
        for (w, h, chroma, depth, ctu, tiles) in [
            (1920usize, 1080usize, 1u32, 8u32, 64usize, 0usize),
            (320, 240, 1, 8, 32, 0),
            (3840, 2160, 1, 10, 64, 3),
        ] {
            let r = RcbInfo::compute(w, h, chroma, depth, ctu, tiles);

            let expected = r.offsets();
            let mut got = vec![RegOffsetInfo {
                reg_idx: 0,
                offset: u32::MAX,
            }];
            got.clear();
            r.write_offsets(&mut got);
            assert_eq!(got, expected, "{w}x{h} offsets");

            let expected = r.hardware_sizes();
            let mut got = vec![RcbConfig {
                reg_idx: 0,
                size: u32::MAX,
            }];
            got.clear();
            r.write_hardware_sizes(&mut got);
            assert_eq!(got.len(), expected.len(), "{w}x{h} sizes");
            for (got, want) in got.iter().zip(&expected) {
                assert_eq!((got.reg_idx, got.size), (want.reg_idx, want.size));
            }
        }
    }
}
