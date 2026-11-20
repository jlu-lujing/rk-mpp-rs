//! H.263 decode on VDPU2 (`mpp/hal/vpu/h263d/hal_h263d_vdpu2.c`).
//!
//! The register window is the MPEG-2 one word for word - `hal_h263d_vdpu2_reg.h`
//! differs from `hal_m2vd_vdpu2_reg.h` only in what the fields are called - so
//! the shared plumbing of [`crate::hal::vdpu2`] covers it and the interesting
//! part is what H.263 leaves at zero: no quantiser table (`sw61`), because the
//! profile has no custom matrices; no direct-motion-vector buffer (`sw62`) and
//! no backward reference pair (`sw134`/`sw135`), because there are no B
//! pictures and no advanced prediction.
//!
//! `sw122`'s time increment is not the picture's own: `h263_fill_picture_parameters`
//! hard-codes 30000, and the hardware only needs a value there for the rounding
//! of advanced-prediction vectors, which the parser rules out by insisting the
//! PTYPE tail is zero.

use std::io;

use crate::codec::h263::parser::VOP_P;
use crate::hal::vdpu2::{self, Word, BUS_BURST_16, REG_NUM};
use crate::service::{Client, MppService, RegOffsetInfo};

pub use crate::hal::vdpu2::{
    frame_layout, Frame, IRQ_FAILURE, REG_BUS, REG_CUR_PIC, REG_DEC_MODE, REG_DIRECT_MV,
    REG_ENDIAN, REG_FILTER, REG_IRQ, REG_MV_ACC, REG_PIC_CTRL, REG_PIC_EXT, REG_PIC_SIZE,
    REG_QP_TABLE, REG_REF0, REG_REF1, REG_REF2, REG_REF3, REG_START_MB, REG_STREAM_LEN,
    REG_VLC_BASE,
};

/// Words the H.263 path names differently from MPEG-2.
pub const REG_TIME_INCR: usize = REG_PIC_EXT;
pub const REG_TAP_0_3: usize = 153;

/// `vop_time_increment_resolution` of `h263_fill_picture_parameters`: a constant,
/// not a field of the stream.
pub const TIME_INCREMENT_RESOLUTION: u32 = 30_000;

/// Everything the register file needs for one picture, with the reference
/// already resolved to a dma-buf fd.
#[derive(Debug, Clone, Copy)]
pub struct PictureJob {
    /// `(vop_width + 15) >> 4`
    pub mb_width: u32,
    /// `(vop_height + 15) >> 4`
    pub mb_height: u32,
    /// The unaligned remainder of the coded size, which the C HAL writes for
    /// every H.263 picture rather than only for a DivX build.
    pub mb_width_off: u32,
    pub mb_height_off: u32,
    /// `vop_quant`, the initial quantiser of the picture.
    pub quant: u32,
    /// `VOP_I` or `VOP_P`.
    pub coding_type: u32,
    /// Whether the forward reference of this picture was itself a P picture.
    pub prev_pic_type: bool,
    /// Bytes the hardware may read, from `stream_offset`.
    pub stream_len: u32,
    /// Byte offset added to the stream fd, always 8-byte aligned.
    pub stream_offset: u32,
    /// Bit position of the first macroblock inside `stream_offset`.
    pub stream_start_bit: u32,
    pub stream_fd: u32,
    pub cur_fd: u32,
    /// `sw131` and `sw148` together; an I picture is pointed at its own buffer.
    pub ref_fd: u32,
}

/// Build the 159-word register file for one picture.
pub fn build_registers(job: &PictureJob) -> [u32; REG_NUM] {
    let mut regs = [0u32; REG_NUM];

    // sw50: the deblocking filter is off and everything else stays at reset.
    regs[REG_FILTER] = Word::default()
        .field(0, 1) // dec_out_tiled_e
        .field(0, 6) // dec_latency
        .field(0, 1) // pic_fixed_quant
        .field(1, 1) // filtering_dis
        .field(0, 1) // divx_enable
        .field(0, 1) // dec_scmd_dis
        .field(0, 1) // dec_adv_pre_dis
        .field(0, 1) // priority_mode
        .field(0, 12) // refbu2_thr
        .field(0, 5) // refbu2_picid
        .field(0, 2)
        .finish();

    regs[REG_STREAM_LEN] = Word::default()
        .field(job.stream_len, 24)
        .field(0, 1)
        .field(job.quant, 6) // init_qp
        .field(0, 1)
        .finish();

    // sw52: start at macroblock 0, with the address-fault threshold at 1.
    regs[REG_START_MB] = Word::default()
        .field(0, 8) // startmb_y
        .field(0, 9) // startmb_x
        .field(1, 14) // apf_threshold
        .field(0, 1)
        .finish();

    // sw53 selects the H.263 variable-length tables.
    regs[REG_DEC_MODE] = 2;

    regs[REG_ENDIAN] = Word::default()
        .field(1, 1) // dec_in_endian
        .field(1, 1) // dec_out_endian
        .field(1, 1) // dec_inswap32_e
        .field(1, 1) // dec_outswap32_e
        .field(1, 1) // dec_strswap32_e
        .field(1, 1) // dec_strendian_e
        .field(0, 26)
        .finish();

    regs[REG_IRQ] = 0;

    regs[REG_BUS] = Word::default()
        .field(0, 8) // dec_axi_rn_id
        .field(0, 8) // dec_axi_wr_id
        .field(BUS_BURST_16, 5) // dec_max_burst
        .field(0, 1)
        .field(0, 1) // dec_data_disc_e
        .field(0, 9)
        .finish();

    let inter = u32::from(job.coding_type == VOP_P);
    regs[REG_PIC_CTRL] = Word::default()
        .field(1, 1) // dec_e
        .field(0, 1) // refbu2_buf_e
        .field(0, 1) // dec_out_dis
        .field(0, 1)
        .field(1, 1) // dec_clk_gate_e
        .field(1, 1) // dec_timeout_e
        .field(0, 1) // picord_count_e
        .field(0, 1) // seq_mbaff_e
        .field(0, 1) // reftopfirst_e
        .field(0, 1) // ref_topfield_e
        .field(0, 1) // write_mvs_e
        .field(0, 1) // sorenson_e
        .field(0, 1) // fwd_interlace_e
        .field(0, 1) // pic_topfield_e
        .field(inter, 1) // pic_inter_e
        .field(0, 1) // pic_b_e
        .field(0, 1) // pic_fieldmode_e
        .field(0, 1) // pic_interlace_e
        .field(0, 1) // pjpeg_e
        .field(0, 1) // divx3_e
        .field(0, 1) // rlc_mode_e
        .field(0, 1) // ch_8pix_ileav_e
        .field(0, 1) // start_code_e
        .field(0, 8)
        .field(0, 1) // dec_ahb_hlock_e
        .finish();

    // sw59 and sw153 are the interpolation filter taps, written as two's
    // complement ten-bit values: -1, 3, -6 and 20.
    regs[59] = Word::default()
        .field(0, 2)
        .field((-6i32) as u32, 10) // pred_bc_tap_0_2
        .field(3, 10) // pred_bc_tap_0_1
        .field((-1i32) as u32, 10) // pred_bc_tap_0_0
        .finish();

    // sw61 and sw62 stay zero: no quantiser table, no direct vectors.
    regs[REG_CUR_PIC] = job.cur_fd;
    regs[REG_VLC_BASE] = job.stream_fd;

    regs[REG_PIC_SIZE] = Word::default()
        .field(0, 5) // reserve
        .field(0, 1) // topfieldfirst_e
        .field(0, 1) // alt_scan_e
        .field(job.mb_height_off, 4)
        .field(job.mb_height, 8)
        .field(job.mb_width_off, 4)
        .field(job.mb_width, 9)
        .finish();

    regs[REG_TIME_INCR] = Word::default()
        .field(TIME_INCREMENT_RESOLUTION, 16)
        .field(0, 3) // intradc_vlc_thr
        .field(0, 5) // ch_qp_offset
        .field(0, 1) // quant_type_1_en
        .field(1, 1) // sync_markers_en
        .field(job.stream_start_bit, 6)
        .finish();

    // One forward reference, and the hardware is told to read it twice over;
    // sw134 and sw135 are never written, so they stay pointed at nothing.
    regs[REG_REF0] = job.ref_fd;
    regs[REG_REF1] = job.ref_fd;

    // The one-bit motion-vector resolutions of the short video header: half-pel
    // in both directions, forward only.
    regs[REG_MV_ACC] = Word::default()
        .field(u32::from(job.prev_pic_type), 1)
        .field(0, 1) // rounding
        .field(0, 1) // fwd_mv_y_resolution
        .field(0, 4) // vrz_bit_of_bwd_mv
        .field(0, 4) // hrz_bit_of_bwd_mv
        .field(1, 4) // vrz_bit_of_fwd_mv
        .field(1, 4) // hrz_bit_of_fwd_mv
        .field(0, 1) // alt_scan
        .field(0, 12)
        .finish();

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

/// Persistent VDPU2 resources plus the one-job submission path. H.263 needs no
/// buffer of its own beyond the register file.
pub struct H263d {
    svc: MppService,
    /// `sw55` of the last completed job.
    pub last_irq: u32,
    jobs: usize,
}

impl H263d {
    pub fn open() -> io::Result<Self> {
        Ok(Self {
            svc: MppService::open(Client::Vdpu2 as u32)?,
            last_irq: 0,
            jobs: 0,
        })
    }

    /// Submit one picture and wait for the hardware.
    pub fn decode(&mut self, job: &PictureJob) -> io::Result<u32> {
        let registers = build_registers(job);
        let offsets = build_offsets(job);
        self.last_irq = vdpu2::run_job(
            &mut self.svc,
            &registers,
            &offsets,
            (job.mb_width as usize) * 16,
            (job.mb_height as usize) * 16,
        )?;
        self.jobs += 1;
        if std::env::var_os("RFI_H263_REG_DUMP").is_some() {
            vdpu2::dump_registers_to(
                "/tmp/our_h263_regs.txt",
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
    use crate::codec::h263::parser::VOP_I;

    /// The words below come from `hal_h263d_debug=1` dumps of
    /// `tests/data/h263_cif_12_ip.263` on RK3588, with the fd words replaced by
    /// stand-ins. They pin every field the C HAL writes, for the I picture that
    /// opens the stream and the P picture that follows it.
    fn job(coding_type: u32, quant: u32, start_bit: u32, stream_len: u32) -> PictureJob {
        PictureJob {
            mb_width: 22,
            mb_height: 18,
            mb_width_off: 0,
            mb_height_off: 0,
            quant,
            coding_type,
            prev_pic_type: false,
            stream_len,
            stream_offset: 0,
            stream_start_bit: start_bit,
            stream_fd: 16,
            cur_fd: 17,
            ref_fd: 17,
        }
    }

    #[test]
    fn the_common_words_match_the_c_register_file() {
        for coding in [VOP_I, VOP_P] {
            let regs = build_registers(&job(coding, 4, 50, 15_877));
            assert_eq!(regs[REG_FILTER], 0x0000_0100, "filtering_dis");
            assert_eq!(
                regs[REG_STREAM_LEN],
                (4 << 25) | 15_877,
                "init_qp above the 24-bit stream length"
            );
            assert_eq!(regs[REG_START_MB], 0x0002_0000, "apf_threshold 1");
            assert_eq!(regs[REG_DEC_MODE], 2, "the H.263 decode mode");
            assert_eq!(regs[REG_ENDIAN], 0x0000_003f);
            assert_eq!(regs[REG_BUS], 0x0010_0000, "max burst 16");
            assert_eq!(regs[REG_IRQ], 0, "no interrupt is armed");
            assert_eq!(regs[58], 0, "sw58 holds no soft reset");
            assert_eq!(regs[59], 0xffc0_3fe8, "filter taps -1, 3, -6");
            assert_eq!(regs[REG_QP_TABLE], 0, "no quantiser table");
            assert_eq!(regs[REG_DIRECT_MV], 0, "no direct motion vectors");
            assert_eq!(regs[REG_CUR_PIC], 17);
            assert_eq!(regs[REG_VLC_BASE], 16);
            assert_eq!(regs[REG_PIC_SIZE], 0x0b00_9000, "22 by 18 macroblocks");
            assert_eq!(regs[REG_TIME_INCR], 0xca00_7530, "30000 and word 50");
            assert_eq!(regs[REG_REF2], 0, "no backward reference");
            assert_eq!(regs[REG_REF3], 0, "no backward reference");
            assert_eq!(regs[REG_TAP_0_3], 0x0500_0000, "pred_bc_tap_0_3 is 20");
            // The post-processor window stays at reset.
            assert_eq!(regs[..50], [0u32; 50]);
        }
    }

    #[test]
    fn picture_type_moves_the_reference_and_the_inter_bit() {
        let intra = build_registers(&job(VOP_I, 4, 50, 15_877));
        assert_eq!(intra[REG_PIC_CTRL], 0x0000_0031, "dec_e, clk gate, timeout");

        let mut inter = job(VOP_P, 2, 50, 11_081);
        inter.ref_fd = 17;
        let regs = build_registers(&inter);
        assert_eq!(regs[REG_PIC_CTRL], 0x0000_4031, "pic_inter_e joins");
        assert_eq!(
            regs[REG_STREAM_LEN],
            (2 << 25) | 11_081,
            "the P picture's own quantiser and length"
        );
        // One forward reference is programmed into both halves of the pair.
        assert_eq!(regs[REG_REF0], 17);
        assert_eq!(regs[REG_REF1], 17);
    }

    /// The last two bits of `sw136`'s neighbour tell the hardware how to round
    /// forward vectors, and `sw_prev_pic_type` only sets after a P reference.
    #[test]
    fn motion_vector_resolution_is_half_pel_forward_only() {
        let regs = build_registers(&job(VOP_P, 2, 50, 11_081));
        assert_eq!(regs[REG_MV_ACC], 0x0000_8800, "hrz and vrz of fwd at 1");

        let mut prev_p = job(VOP_P, 2, 50, 11_081);
        prev_p.prev_pic_type = true;
        assert_eq!(build_registers(&prev_p)[REG_MV_ACC], 0x0000_8801);
    }

    /// QCIF and 4CIF are what the source format table allows besides CIF, and
    /// neither is a whole number of macroblocks wide except CIF.
    #[test]
    fn macroblock_counts_and_remainders_are_both_written() {
        let mut qcif = job(VOP_I, 4, 50, 7570);
        qcif.mb_width = 11;
        qcif.mb_height = 9;
        assert_eq!(build_registers(&qcif)[REG_PIC_SIZE], 0x0580_4800);

        let mut four_cif = job(VOP_I, 4, 50, 41_277);
        four_cif.mb_width = 44;
        four_cif.mb_height = 36;
        assert_eq!(build_registers(&four_cif)[REG_PIC_SIZE], 0x1601_2000);

        // A picture that is not a multiple of 16 carries its remainder here, and
        // 16CIF is the widest size the format table allows.
        let mut odd = job(VOP_P, 2, 50, 1000);
        odd.mb_width = 11;
        odd.mb_height = 9;
        odd.mb_width_off = 15;
        odd.mb_height_off = 1;
        assert_eq!(build_registers(&odd)[REG_PIC_SIZE], 0x05f8_4880);
    }

    #[test]
    fn only_the_stream_base_ever_moves() {
        assert!(build_offsets(&job(VOP_I, 4, 50, 15_877)).is_empty());
        let mut moved = job(VOP_P, 2, 50, 11_081);
        moved.stream_offset = 8;
        let offsets = build_offsets(&moved);
        assert_eq!(offsets.len(), 1);
        assert_eq!(offsets[0].reg_idx, REG_VLC_BASE as u32);
        assert_eq!(offsets[0].offset, 8);
    }
}
