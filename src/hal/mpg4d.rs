//! MPEG-4 Part 2 decode on VDPU2 (`mpp/hal/vpu/mpg4d/hal_m4vd_vdpu2.c`).
//!
//! Same 159-word window, client 1 and frame layout as the other legacy codecs
//! (see [`crate::hal::vdpu2`]); what differs is how the words are filled. The
//! register names follow `M4vdVdpu2Regs_t`, so `REG_MV_ACC` is reg136, where the
//! MPEG-4 fcode nibbles and the `prev_pic_type` / rounding flags live.
//!
//! Two buffers are persistent for the life of the channel rather than per
//! picture: the 128-byte quantiser table and the 1080p-sized direct-motion-vector
//! buffer, which the hardware writes on P pictures and reads back on the next
//! one. The C HAL never clears the mv buffer between jobs and neither do we.

use std::io;

use crate::codec::mpg4::parser::{VOP_B, VOP_I, VOP_P};
use crate::hal::vdpu2::{self, Word, BUS_BURST_16, REG_NUM};
use crate::service::dma_heap::DmaBuf;
use crate::service::{Client, MppService, RegOffsetInfo};

pub use crate::hal::vdpu2::{
    frame_layout, Frame, IRQ_FAILURE, REG_BUS, REG_CUR_PIC, REG_DEC_MODE, REG_DIRECT_MV, REG_ENDIAN,
    REG_FILTER, REG_IRQ, REG_MV_ACC, REG_PIC_CTRL, REG_PIC_SIZE, REG_QP_TABLE, REG_REF0, REG_REF1, REG_REF2,
    REG_REF3, REG_START_MB, REG_STREAM_LEN, REG_VLC_BASE,
};

/// Words the MPEG-4 path touches that MPEG-2 leaves alone.
pub const REG_TIME_INCR: usize = 122;
pub const REG_TRB_D0: usize = 137;
pub const REG_TRB_DM1: usize = 138;
pub const REG_TRB_D1: usize = 139;
pub const REG_TAP_0_3: usize = 153;

/// `MPEG4_MAX_MV_BUF_SIZE`: fixed at 1080p macroblock count for convenience.
pub const MV_BUF_SIZE: usize = (1920 / 16) * (1088 / 16) * 4 * std::mem::size_of::<u32>();

/// `qp_table`: one intra and one inter matrix, no padding.
pub const QP_TABLE_SIZE: usize = 128;

/// `default_intra_matrix` of `hal_m4vd_com.c`. MPEG-4 matrices travel in raster
/// order, so unlike MPEG-2 there is no de-zigzag step here.
pub const DEFAULT_INTRA_MATRIX: [u8; 64] = [
    8, 17, 18, 19, 21, 23, 25, 27, 17, 18, 19, 21, 23, 25, 27, 28, 20, 21, 22, 23, 24, 26, 28, 30,
    21, 22, 23, 24, 26, 28, 30, 32, 22, 23, 24, 26, 28, 30, 32, 35, 23, 24, 26, 28, 30, 32, 35, 38,
    25, 26, 28, 30, 32, 35, 38, 41, 27, 28, 30, 32, 35, 38, 41, 45,
];

/// `default_inter_matrix` of `hal_m4vd_com.c`.
pub const DEFAULT_INTER_MATRIX: [u8; 64] = [
    16, 17, 18, 19, 20, 21, 22, 23, 17, 18, 19, 20, 21, 22, 23, 24, 18, 19, 20, 21, 22, 23, 24, 25,
    19, 20, 21, 22, 23, 24, 26, 27, 20, 21, 22, 23, 25, 26, 27, 28, 21, 22, 23, 24, 26, 27, 28, 30,
    22, 23, 24, 26, 27, 28, 30, 31, 23, 24, 25, 27, 28, 30, 31, 33,
];

/// Everything the register file needs for one picture, with the references
/// already resolved to dma-buf fds.
#[derive(Debug, Clone, Copy)]
pub struct PictureJob {
    /// `(vop_width + 15) >> 4`
    pub mb_width: u32,
    /// `(vop_height + 15) >> 4`
    pub mb_height: u32,
    /// DivX build 4 only: the unaligned remainder of the coded size.
    pub mb_width_off: u32,
    pub mb_height_off: u32,
    /// `vop_time_increment_resolution`
    pub time_inc_resolution: u32,
    pub quant_type: bool,
    pub quarter_sample: bool,
    pub alternate_vertical_scan: bool,
    /// `vop_rounding_type`, read on P pictures only.
    pub rounding: bool,
    pub top_field_first: bool,
    pub interlaced: bool,
    pub intra_dc_vlc_thr: u32,
    /// `vop_quant`, the initial quantiser of the picture.
    pub quant: u32,
    /// `prev_coding_type` of the current forward reference, truncated to one bit.
    pub prev_pic_type: u32,
    /// `VOP_I` / `VOP_P` / `VOP_B`.
    pub coding_type: u32,
    pub fcode_forward: u32,
    pub fcode_backward: u32,
    /// B picture interpolation ratios; ignored by every other picture type.
    pub time_bp: u32,
    pub time_pp: u32,
    /// Bytes the hardware may read, from the 8-byte aligned base.
    pub stream_len: u32,
    /// Byte offset added to the stream fd.
    pub stream_offset: u32,
    /// Bit position of the first coded macroblock inside `stream_offset`.
    pub stream_start_bit: u32,
    pub stream_fd: u32,
    pub cur_fd: u32,
    /// `sw131` / `sw148` / `sw134` / `sw135`.
    pub refs: [u32; 4],
    pub qp_fd: u32,
    pub mv_fd: u32,
}

/// `MPP_DIV`: integer division that leaves the numerator alone when the
/// denominator is zero.
fn div(num: i64, den: i64) -> u32 {
    if den == 0 {
        num
    } else {
        num / den
    }
    .try_into()
    .unwrap_or(u32::MAX)
}

/// The three `trb_per_trd_*` ratios of a B picture, as `(d0, dm1, d1)`.
pub fn b_time_ratios(time_bp: u32, time_pp: u32) -> (u32, u32, u32) {
    let (tbp, tpp) = (i64::from(time_bp), i64::from(time_pp));
    (
        div((tbp << 27) + (tpp - 1), tpp),
        div((((2 * tbp) - 1) << 27) + 2 * (tpp - 1), 2 * tpp - 1),
        div((((2 * tbp) + 1) << 27) + 2 * tpp, 2 * tpp + 1),
    )
}

/// Build the 159-word register file for one picture.
pub fn build_registers(job: &PictureJob) -> [u32; REG_NUM] {
    let mut regs = [0u32; REG_NUM];

    // sw50: the C HAL turns off the deblocking filter and leaves the rest alone.
    regs[REG_FILTER] = Word::default()
        .field(0, 1) // dec_out_tiled_msb
        .field(0, 6) // dec_latency
        .field(0, 1) // pic_fixed_quant
        .field(1, 1) // dblk_flt_dis
        .field(0, 1) // skip_sel
        .field(0, 1) // dec_ascmd0_dis
        .field(0, 1) // adv_pref_dis
        .field(0, 1) // dec_out_tiled_lsb
        .field(0, 12) // refbuf_thrd
        .field(0, 5) // refbuf_pid
        .field(0, 2)
        .finish();

    regs[REG_STREAM_LEN] = Word::default()
        .field(job.stream_len, 24)
        .field(0, 1)
        .field(job.quant, 6) // qp_init_val
        .field(0, 1)
        .finish();

    // sw52: start at macroblock 0, with the advanced prefetch threshold at 1.
    regs[REG_START_MB] = Word::default()
        .field(0, 8) // ydim_mbst
        .field(0, 9) // xdim_mbst
        .field(1, 14) // adv_pref_thrd
        .field(0, 1)
        .finish();

    regs[REG_DEC_MODE] = 1;

    regs[REG_ENDIAN] = Word::default()
        .field(1, 1) // dec_in_endian
        .field(1, 1) // dec_out_endian
        .field(1, 1) // dec_in_wordsp
        .field(1, 1) // dec_out_wordsp
        .field(1, 1) // dec_strswap32_e
        .field(1, 1) // dec_strendian_e
        .field(0, 26)
        .finish();

    regs[REG_IRQ] = 0;

    regs[REG_BUS] = Word::default()
        .field(0, 8) // axi_id_rd
        .field(0, 8) // axi_id_wr
        .field(BUS_BURST_16, 5) // max_burlen
        .field(0, 1)
        .field(0, 1) // data_discd_en
        .field(0, 9)
        .finish();

    let inter = u32::from(job.coding_type != VOP_I);
    let dmmv_wr = u32::from(job.coding_type == VOP_P);
    regs[REG_PIC_CTRL] = Word::default()
        .field(1, 1) // dec_st_work
        .field(0, 1) // refpic_buf2_en
        .field(0, 1) // dec_wr_extmem_dis
        .field(0, 1)
        .field(1, 1) // dec_clkgate_en
        .field(1, 1) // timeout_sts_en
        .field(0, 1) // rd_cnt_tab_en
        .field(0, 1) // sequ_mbaff_en
        .field(0, 1) // first_reftop_en
        .field(0, 1) // reftop_en
        .field(dmmv_wr, 1) // dmmv_wr_en
        .field(0, 1) // sorspa_en
        .field(0, 1) // fwd_refpic_mode_sel
        .field(0, 1) // pic_decfield_sel
        .field(inter, 1) // pic_type_sel0
        .field(u32::from(job.coding_type == VOP_B), 1) // pic_type_sel1
        .field(0, 1) // curpic_stru_sel
        .field(u32::from(job.interlaced), 1) // curpic_code_sel
        .field(0, 1) // prog_jpeg_en
        .field(0, 1) // divx3_en
        .field(0, 1) // rlc_mode_en
        .field(0, 1) // addit_ch_fmt_wen
        .field(0, 1) // st_code_exist
        .field(0, 8)
        .field(0, 1) // dec_timeout_mode
        .finish();

    // sw59 and sw153 are the interpolation filter taps, written as two's
    // complement ten-bit values: -1, 3, -6 and 20.
    regs[59] = Word::default()
        .field(0, 2)
        .field((-6i32) as u32, 10) // pflt_set0_tap2
        .field(3, 10) // pflt_set0_tap1
        .field((-1i32) as u32, 10) // pflt_set0_tap0
        .finish();

    regs[REG_QP_TABLE] = job.qp_fd;
    regs[REG_DIRECT_MV] = job.mv_fd;
    regs[REG_CUR_PIC] = job.cur_fd;
    regs[REG_VLC_BASE] = job.stream_fd;

    regs[REG_PIC_SIZE] = Word::default()
        .field(0, 5)
        .field(u32::from(job.interlaced && job.top_field_first), 1) // topfieldfirst_e
        .field(0, 1) // alt_scan_e: the C model wants only the flag in sw136
        .field(job.mb_height_off, 4)
        .field(job.mb_height, 8)
        .field(job.mb_width_off, 4)
        .field(job.mb_width, 9)
        .finish();

    regs[REG_TIME_INCR] = Word::default()
        .field(job.time_inc_resolution, 16)
        .field(job.intra_dc_vlc_thr, 3)
        .field(0, 5) // ch_qp_offset
        .field(u32::from(job.quant_type), 1) // quant_type_1_en
        .field(1, 1) // sync_markers_en
        .field(job.stream_start_bit, 6)
        .finish();

    // MPEG-4 puts the forward reference pair in sw131/sw148 and the backward
    // pair in sw134/sw135; for a B picture the older head is the forward one.
    regs[REG_REF0] = job.refs[0];
    regs[REG_REF1] = job.refs[1];
    regs[REG_REF2] = job.refs[2];
    regs[REG_REF3] = job.refs[3];

    // An I picture has no fcode, and the C HAL still gives the forward direction
    // a shift of one so the direct vectors stay unusable.
    let (fwd, bwd) = match job.coding_type {
        VOP_B => (job.fcode_forward, job.fcode_backward),
        VOP_P => (job.fcode_forward, 0),
        _ => (1, 0),
    };
    regs[REG_MV_ACC] = Word::default()
        .field(job.prev_pic_type & 1, 1)
        // The C HAL clears the rounding type for B pictures.
        .field(u32::from(job.coding_type != VOP_B && job.rounding), 1)
        .field(u32::from(job.quarter_sample), 1) // fwd_mv_y_resolution
        .field(bwd, 4) // vrz_bit_of_bwd_mv
        .field(bwd, 4) // hrz_bit_of_bwd_mv
        .field(fwd, 4) // vrz_bit_of_fwd_mv
        .field(fwd, 4) // hrz_bit_of_fwd_mv
        .field(u32::from(job.alternate_vertical_scan), 1) // alt_scan_flag_e
        .field(0, 12)
        .finish();

    if job.coding_type == VOP_B {
        let (d0, dm1, d1) = b_time_ratios(job.time_bp, job.time_pp);
        regs[REG_TRB_D0] = Word::default().field(d0, 27).field(0, 5).finish();
        regs[REG_TRB_DM1] = Word::default().field(dm1, 27).field(0, 5).finish();
        regs[REG_TRB_D1] = Word::default().field(d1, 27).field(0, 5).finish();
    }

    regs[REG_TAP_0_3] = Word::default()
        .field(0, 2)
        .field(0, 10) // pred_bc_tap_1_1
        .field(0, 10) // pred_bc_tap_1_0
        .field(20, 10) // pred_bc_tap_0_3
        .finish();

    regs
}

/// Address offset for one picture: the stream base moves to the 8-byte aligned
/// byte in front of the first macroblock.
pub fn build_offsets(job: &PictureJob) -> Vec<RegOffsetInfo> {
    if job.stream_offset == 0 {
        Vec::new()
    } else {
        vec![RegOffsetInfo {
            reg_idx: REG_VLC_BASE as u32,
            offset: job.stream_offset,
        }]
    }
}

/// Persistent VDPU2 resources plus the one-job submission path.
pub struct Mpg4d {
    svc: MppService,
    qp_table: DmaBuf,
    mv_table: DmaBuf,
    /// `sw55` of the last completed job.
    pub last_irq: u32,
    jobs: usize,
}

impl Mpg4d {
    pub fn open() -> io::Result<Self> {
        Ok(Self {
            svc: MppService::open(Client::Vdpu2 as u32)?,
            qp_table: DmaBuf::alloc("system", QP_TABLE_SIZE, false)?,
            mv_table: DmaBuf::alloc("system", MV_BUF_SIZE, false)?,
            last_irq: 0,
            jobs: 0,
        })
    }

    pub fn qp_fd(&self) -> u32 {
        self.qp_table.fd() as u32
    }

    /// fd of the direct-motion-vector buffer, which outlives every picture.
    pub fn mv_fd(&self) -> u32 {
        self.mv_table.fd() as u32
    }

    /// Write the intra and inter quantiser tables. The hardware reads exactly
    /// these 128 bytes, so there is no tail to clear.
    pub fn write_qp_table(&mut self, intra: &[u8; 64], inter: &[u8; 64]) -> io::Result<()> {
        self.qp_table.sync_write_begin()?;
        {
            let window = self.qp_table.as_mut_slice();
            // The dma-heap mapping is page rounded, so address the table by its
            // own size rather than the buffer's.
            window[..64].copy_from_slice(intra);
            window[64..QP_TABLE_SIZE].copy_from_slice(inter);
        }
        self.qp_table.sync_write_end()
    }

    /// Submit one picture and wait for the hardware.
    pub fn decode(&mut self, job: &PictureJob) -> io::Result<u32> {
        let registers = build_registers(job);
        let offsets = build_offsets(job);
        self.last_irq = vdpu2::run_job(&mut self.svc, &registers, &offsets)?;
        self.jobs += 1;
        if std::env::var_os("RFI_MP4_REG_DUMP").is_some() {
            vdpu2::dump_registers_to(
                "/tmp/our_mp4_regs.txt",
                &format!("PICTURE {}", self.jobs),
                &registers,
            );
        }
        Ok(self.last_irq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values below come from `hal_mpg4d_debug=1` register dumps of
    /// `tests/data/mp4_320x240_12_ip.m4v` and `mp4_320x240_12_ibpb.m4v` on t6,
    /// with the fd words replaced by stand-ins. They pin every field the C HAL
    /// writes for the three picture types.
    fn job(coding: u32, start_bit: u32, stream_len: u32, fcode: [u32; 2]) -> PictureJob {
        PictureJob {
            mb_width: 20,
            mb_height: 15,
            mb_width_off: 0,
            mb_height_off: 0,
            time_inc_resolution: 30,
            quant_type: false,
            quarter_sample: false,
            alternate_vertical_scan: false,
            rounding: false,
            top_field_first: false,
            interlaced: false,
            intra_dc_vlc_thr: 0,
            quant: 6,
            prev_pic_type: 0,
            coding_type: coding,
            fcode_forward: fcode[0],
            fcode_backward: fcode[1],
            time_bp: 1,
            time_pp: 3,
            stream_len,
            stream_offset: 0,
            stream_start_bit: start_bit,
            stream_fd: 17,
            cur_fd: 18,
            refs: [18, 18, 18, 18],
            qp_fd: 16,
            mv_fd: 15,
        }
    }

    #[test]
    fn the_common_words_match_the_c_register_file() {
        for coding in [VOP_I, VOP_P, VOP_B] {
            let regs = build_registers(&job(coding, 55, 8192, [1, 1]));
            assert_eq!(regs[REG_FILTER], 0x0000_0100, "dblk_flt_dis");
            assert_eq!(regs[REG_START_MB], 0x0002_0000, "adv_pref_thrd");
            assert_eq!(regs[REG_DEC_MODE], 1);
            assert_eq!(regs[REG_ENDIAN], 0x0000_003f);
            assert_eq!(regs[REG_BUS], 0x0010_0000, "max_burlen 16");
            assert_eq!(regs[59], 0xffc0_3fe8, "filter taps -1, 3, -6");
            assert_eq!(regs[REG_PIC_SIZE], 0x0a00_7800, "20 by 15 macroblocks");
            assert_eq!(regs[REG_TAP_0_3], 0x0500_0000, "pred_bc_tap_0_3 is 20");
            assert_eq!(regs[REG_QP_TABLE], 16);
            assert_eq!(REG_DIRECT_MV, 62);
            assert_eq!(regs[REG_DIRECT_MV], 15);
            assert_eq!(regs[REG_CUR_PIC], 18);
            assert_eq!(regs[REG_VLC_BASE], 17);
            // The post-processor window stays at reset.
            assert_eq!(regs[..50], [0u32; 50]);
            assert_eq!(regs[REG_IRQ], 0, "no interrupt is armed");
            assert_eq!(regs[58], 0, "sw58 holds no soft reset");
        }
    }

    #[test]
    fn picture_type_selects_the_control_bits() {
        let regs = build_registers(&job(VOP_I, 51, 8755, [0, 0]));
        assert_eq!(regs[REG_PIC_CTRL], 0x0000_0031);
        assert_eq!(regs[REG_STREAM_LEN], 0x0c00_2233, "len 8755, qp_init 6");
        // C's own dump of this picture reads start word 35 because its access
        // unit carries the 54 bytes of sequence headers in front of the VOP
        // start code; the absolute bit position is the same either way.
        assert_eq!(
            regs[REG_TIME_INCR], 0xce00_001e,
            "time increment 30, sync markers, start word 51"
        );
        assert_eq!(regs[REG_MV_ACC], 0x0000_8800, "an I picture shifts by one");

        let regs = build_registers(&job(VOP_P, 55, 2548, [1, 0]));
        assert_eq!(regs[REG_PIC_CTRL], 0x0000_4431, "pic_type_sel0 and dmmv_wr_en");
        assert_eq!(regs[REG_STREAM_LEN], 0x0c00_09f4);
        assert_eq!(regs[REG_TIME_INCR], 0xde00_001e, "start word 55");
        assert_eq!(regs[REG_MV_ACC], 0x0000_8800);

        let regs = build_registers(&job(VOP_B, 57, 2021, [1, 1]));
        assert_eq!(regs[REG_PIC_CTRL], 0x0000_c031, "both sel bits, no dmmv write");
        assert_eq!(regs[REG_TIME_INCR], 0xe600_001e, "start word 57");
    }

    /// `sw136` of the B pictures in `mp4_320x240_12_ibpb`: the `prev_pic_type`
    /// bit tracks the forward head and both directions carry their fcode.
    #[test]
    fn mv_accumulator_carries_the_prediction_state() {
        let mut picture = job(VOP_B, 57, 2021, [1, 1]);
        picture.prev_pic_type = 1;
        assert_eq!(build_registers(&picture)[REG_MV_ACC], 0x0000_8889);

        picture.prev_pic_type = 0;
        assert_eq!(build_registers(&picture)[REG_MV_ACC], 0x0000_8888);

        // The P picture that opens the group reads fcode 2 and rounding type 1.
        let mut picture = job(VOP_P, 55, 3793, [2, 0]);
        picture.rounding = true;
        assert_eq!(build_registers(&picture)[REG_MV_ACC], 0x0001_1002);
    }

    #[test]
    fn b_pictures_get_the_time_ratios() {
        let regs = build_registers(&job(VOP_B, 57, 2021, [1, 1]));
        assert_eq!(regs[REG_TRB_D0], 0x02aa_aaab);
        assert_eq!(regs[REG_TRB_DM1], 0x0199_999a);
        assert_eq!(regs[REG_TRB_D1], 0x036d_b6dc);
        assert_eq!(b_time_ratios(2, 3), (0x5555556, 0x4cccccd, 0x5b6db6e));
        assert_eq!(b_time_ratios(1, 2), (0x4000000, 0x2aaaaab, 0x4cccccd));

        let regs = build_registers(&job(VOP_P, 55, 2548, [1, 0]));
        assert_eq!(
            (regs[REG_TRB_D0], regs[REG_TRB_DM1], regs[REG_TRB_D1]),
            (0, 0, 0),
            "a P picture leaves the ratio words at reset"
        );
    }

    #[test]
    fn interlaced_pictures_mark_the_field_order() {
        let mut picture = job(VOP_I, 51, 4096, [0, 0]);
        picture.interlaced = true;
        picture.top_field_first = true;
        let regs = build_registers(&picture);
        assert_eq!(regs[REG_PIC_CTRL] & (1 << 17), 1 << 17, "curpic_code_sel");
        assert_eq!(regs[REG_PIC_CTRL] & (1 << 16), 0, "curpic_stru_sel");
        assert_eq!(regs[REG_PIC_SIZE] & (1 << 5), 1 << 5, "topfieldfirst_e");
    }

    #[test]
    fn divx4_reports_the_unaligned_macroblock_remainder() {
        let mut picture = job(VOP_I, 51, 4096, [0, 0]);
        picture.mb_width_off = 6;
        picture.mb_height_off = 8;
        assert_eq!(build_registers(&picture)[REG_PIC_SIZE], 0x0a30_7c00);
    }

    #[test]
    fn only_a_nonzero_stream_offset_is_sent() {
        let mut picture = job(VOP_P, 55, 2548, [1, 0]);
        assert!(build_offsets(&picture).is_empty());
        picture.stream_offset = 56;
        assert_eq!(
            build_offsets(&picture),
            vec![RegOffsetInfo {
                reg_idx: REG_VLC_BASE as u32,
                offset: 56
            }]
        );
    }
}
