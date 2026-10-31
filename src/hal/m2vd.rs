//! MPEG-2 decode on VDPU2 (`mpp/hal/vpu/m2vd/hal_m2vd_vdpu2.c`).
//!
//! VDPU2 is the legacy VPU block, client 1 on RK3588. Its register file is
//! 159 words: the 50-word post-processor window (`ppReg`, unused here because
//! the C HAL leaves it zeroed) followed by the sw50..sw158 decode window.
//! Address registers carry a dma-buf fd in the low bits and the driver rewrites
//! them at job time, exactly like the rkvdec path; a byte offset on top of an
//! address goes through `MPP_CMD_SET_REG_ADDR_OFFSET` rather than the word
//! itself, so the fd stays intact.
//!
//! The bitfield packing follows the declaration order of `M2vdVdpu2Reg` in
//! `hal_m2vd_vdpu2_reg.h`: the first declared field sits in the low bits.

use std::io;

use crate::service::dma_heap::DmaBuf;
use crate::service::{Client, MppService, RegOffsetInfo};

// The register window, the interrupt bits and the frame surface are shared with
// every other vdpu2 legacy codec; MPEG-4 and H.263 import them from here too.
pub use crate::hal::vdpu2::{
    frame_layout, Frame, Word, BUS_BURST_16, IRQ_ASO, IRQ_BUFFER, IRQ_ERROR, IRQ_FAILURE,
    IRQ_PIC_INF, IRQ_RDY, IRQ_SLICE, IRQ_TIMEOUT, REG_BUS, REG_CUR_PIC, REG_DEC_MODE, REG_ENDIAN,
    REG_FILTER, REG_IRQ, REG_MV_ACC, REG_NUM, REG_PIC_CTRL, REG_PIC_EXT, REG_PIC_SIZE,
    REG_QP_TABLE, REG_REF0, REG_REF1, REG_REF2, REG_REF3, REG_START_MB, REG_STREAM_LEN,
    REG_VLC_BASE,
};

/// `M2VD_VDPU2_REG_NUM`
/// `M2VD_BUF_SIZE_QPTAB`: 64 intra + 64 inter entries, zero padded to this.
pub const QP_TABLE_SIZE: usize = 256;
pub use crate::hal::vdpu2::REG_DIRECT_MV;

/// MPEG-2 picture coding types (`M2VDPicCodingType`).
pub const CODING_I: u32 = 1;
pub const CODING_P: u32 = 2;
pub const CODING_B: u32 = 3;
pub const CODING_D: u32 = 4;

/// `M2VDPicStruct`
pub const STRUCT_TOP_FIELD: u32 = 1;
pub const STRUCT_BOTTOM_FIELD: u32 = 2;
pub const STRUCT_FRAME: u32 = 3;

/// `DEC_LITTLE_ENDIAN`
const LITTLE_ENDIAN: u32 = 1;

/// Everything the register file needs for one picture, already resolved to
/// dma-buf fds by the caller.
#[derive(Debug, Clone, Copy)]
pub struct PictureJob {
    /// `decode_width / 16` rounded up (`pic_mb_width`).
    pub mb_width: u32,
    /// `decode_height / 16` rounded up (`pic_mb_height_p`).
    pub mb_height_p: u32,
    /// A sequence extension was seen, which selects decode mode 5 over 6.
    pub has_sequence_extension: bool,
    pub progressive_sequence: bool,
    /// 1 top field, 2 bottom field, 3 frame.
    pub picture_structure: u32,
    pub top_field_first: bool,
    /// `CODING_I` / `CODING_P` / `CODING_B` / `CODING_D`.
    pub picture_coding_type: u32,
    pub intra_dc_precision: u32,
    pub q_scale_type: bool,
    pub intra_vlc_format: bool,
    pub concealment_motion_vectors: bool,
    pub alternate_scan: bool,
    pub frame_pred_frame_dct: bool,
    /// The four `f_code` halves as the C parser leaves them: with a sequence
    /// extension all four come from the picture coding extension, otherwise the
    /// forward pair is the MPEG-1 `forward_f_code` and the full-pel flags scale
    /// the motion-vector accuracy.
    pub forward_full_pel: u32,
    pub forward_f_code: u32,
    pub backward_full_pel: u32,
    pub backward_f_code: u32,
    /// Bytes of packet data the hardware may read, from `stream_offset`.
    pub stream_len: u32,
    pub stream_offset: u32,
    /// Bit position of the first slice start code inside `stream_offset`.
    pub stream_start_bit: u32,
    pub stream_fd: u32,
    pub cur_fd: u32,
    /// Byte offset added to `cur_fd` for a bottom field.
    pub cur_offset: u32,
    /// Reference fds for sw131 / sw148 / sw134 / sw135.
    pub refs: [u32; 4],
    pub qp_fd: u32,
}

/// Build the 159-word register file for one picture.
pub fn build_registers(job: &PictureJob) -> [u32; REG_NUM] {
    let mut regs = [0u32; REG_NUM];

    // sw50: raster output, deblocking on, everything else at reset values.
    regs[REG_FILTER] = Word::default()
        .field(0, 1) // dec_out_tiled_e
        .field(0, 6) // dec_latency
        .field(0, 1) // pic_fixed_quant
        .field(1, 1) // filtering_dis
        .field(0, 1) // skip_mode
        .field(0, 1) // dec_scmd_dis
        .field(0, 1) // dec_adv_pre_dis
        .field(0, 1) // priority_mode
        .field(0, 12) // refbu2_thr
        .field(0, 5) // refbu2_picid
        .field(0, 2) // reserve1
        .finish();

    regs[REG_STREAM_LEN] = Word::default()
        .field(job.stream_len, 24)
        .field(0, 1) // reserve1
        .field(1, 6) // init_qp
        .field(0, 1) // reserve2
        .finish();

    regs[REG_START_MB] = Word::default()
        .field(0, 8) // startmb_y
        .field(0, 9) // startmb_x
        .field(8, 14) // apf_threshold
        .field(0, 1)
        .finish();

    // sw53: 5 with a sequence extension, 6 for MPEG-1 style syntax only.
    regs[REG_DEC_MODE] = if job.has_sequence_extension { 5 } else { 6 };

    regs[REG_ENDIAN] = Word::default()
        .field(LITTLE_ENDIAN, 1) // dec_in_endian
        .field(LITTLE_ENDIAN, 1) // dec_out_endian
        .field(1, 1) // dec_inswap32_e
        .field(1, 1) // dec_outswap32_e
        .field(1, 1) // dec_strswap32_e
        .field(LITTLE_ENDIAN, 1) // dec_strendian_e
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

    let interlace = u32::from(!job.progressive_sequence);
    let field_mode = u32::from(job.picture_structure != STRUCT_FRAME);
    // `pic_topfield_e` says which field this picture is, not the display order;
    // `top_field_first` is carried separately in sw120.
    let top_field = u32::from(job.picture_structure == STRUCT_TOP_FIELD);
    let inter = u32::from(job.picture_coding_type != CODING_I);
    let b_pic = u32::from(job.picture_coding_type == CODING_B);
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
        .field(top_field, 1) // pic_topfield_e
        .field(inter, 1) // pic_inter_e
        .field(b_pic, 1) // pic_b_e
        .field(field_mode, 1) // pic_fieldmode_e
        .field(interlace, 1) // pic_interlace_e
        .field(0, 1) // pjpeg_e
        .field(0, 1) // divx3_e
        .field(0, 1) // rlc_mode_e
        .field(0, 1) // ch_8pix_ileav_e
        .field(0, 1) // start_code_e
        .field(0, 8)
        .field(0, 1) // dec_ahb_hlock_e
        .finish();

    regs[REG_QP_TABLE] = job.qp_fd;
    regs[REG_DIRECT_MV] = 0;
    regs[REG_CUR_PIC] = job.cur_fd;
    regs[REG_VLC_BASE] = job.stream_fd;

    regs[REG_PIC_SIZE] = Word::default()
        .field(0, 5) // ref_frames
        .field(u32::from(job.top_field_first), 1) // topfieldfirst_e
        .field(u32::from(job.alternate_scan), 1) // alt_scan_e
        .field(0, 4) // mb_height_off
        .field(job.mb_height_p, 8)
        .field(0, 4) // mb_width_off
        .field(job.mb_width, 9)
        .finish();

    regs[REG_PIC_EXT] = Word::default()
        .field(u32::from(job.frame_pred_frame_dct), 1)
        .field(u32::from(job.intra_vlc_format), 1)
        .field(job.intra_dc_precision, 2)
        .field(u32::from(job.concealment_motion_vectors), 1)
        .field(0, 19)
        .field(u32::from(job.q_scale_type), 1)
        .field(0, 1)
        .field(job.stream_start_bit, 6)
        .finish();

    regs[REG_REF0] = job.refs[0];
    regs[REG_REF2] = job.refs[2];
    regs[REG_REF3] = job.refs[3];

    // The C parser overwrites the picture-header f_code fields with the picture
    // coding extension's four-bit values whenever a sequence extension is
    // present, so the sequence-extension branch hands those straight to the
    // hardware and only the MPEG-1 branch has to fold the full-pel flags in.
    let (fcode_fwd_hor, fcode_fwd_ver, fcode_bwd_hor, fcode_bwd_ver, mv_fwd, mv_bwd) =
        if job.has_sequence_extension {
            (
                job.forward_full_pel,
                job.forward_f_code,
                job.backward_full_pel,
                job.backward_f_code,
                1,
                1,
            )
        } else {
            (
                job.forward_f_code,
                job.forward_f_code,
                job.backward_f_code,
                job.backward_f_code,
                u32::from(job.forward_full_pel == 0),
                u32::from(job.backward_full_pel == 0),
            )
        };
    regs[REG_MV_ACC] = Word::default()
        .field(0, 1) // reserve
        .field(mv_bwd, 1) // mv_accuracy_bwd
        .field(mv_fwd, 1) // mv_accuracy_fwd
        .field(fcode_bwd_ver, 4)
        .field(fcode_bwd_hor, 4)
        .field(fcode_fwd_ver, 4)
        .field(fcode_fwd_hor, 4)
        .field(u32::from(job.alternate_scan), 1) // alt_scan_flag_e
        .field(0, 12)
        .finish();

    regs[REG_REF1] = job.refs[1];
    regs
}

/// Address offsets for one picture: the stream base moves to the 8-byte
/// aligned byte in front of the first slice, and a bottom field starts one
/// aligned row into the current picture.
pub fn build_offsets(job: &PictureJob) -> Vec<RegOffsetInfo> {
    let mut offsets = Vec::with_capacity(2);
    if job.stream_offset != 0 {
        offsets.push(RegOffsetInfo {
            reg_idx: REG_VLC_BASE as u32,
            offset: job.stream_offset,
        });
    }
    if job.cur_offset != 0 {
        offsets.push(RegOffsetInfo {
            reg_idx: REG_CUR_PIC as u32,
            offset: job.cur_offset,
        });
    }
    offsets
}

/// Persistent VDPU2 resources plus the one-job submission path.
pub struct M2vd {
    svc: MppService,
    qp_table: DmaBuf,
    /// `sw55` of the last completed job.
    pub last_irq: u32,
    /// Jobs submitted so far, so the register dump can name them.
    jobs: usize,
}

impl M2vd {
    pub fn open() -> io::Result<Self> {
        Ok(Self {
            svc: MppService::open(Client::Vdpu2 as u32)?,
            qp_table: DmaBuf::alloc("system", QP_TABLE_SIZE, false)?,
            last_irq: 0,
            jobs: 0,
        })
    }

    /// fd the hardware reads the quantiser tables from (`sw61.slice_table`; the
    /// name is legacy, MPEG-2 has no slice table).
    pub fn qp_fd(&self) -> u32 {
        self.qp_table.fd() as u32
    }

    /// Write the 64 intra and 64 inter entries the C HAL copies per picture.
    pub fn write_qp_table(&mut self, intra: &[u8; 64], inter: &[u8; 64]) -> io::Result<()> {
        self.qp_table.sync_write_begin()?;
        {
            let window = self.qp_table.as_mut_slice();
            window[..64].copy_from_slice(intra);
            window[64..128].copy_from_slice(inter);
            for byte in &mut window[128..QP_TABLE_SIZE] {
                *byte = 0;
            }
        }
        self.qp_table.sync_write_end()
    }

    /// Submit one picture and wait for the hardware.
    pub fn decode(&mut self, job: &PictureJob) -> io::Result<u32> {
        let registers = build_registers(job);
        let offsets = build_offsets(job);
        self.last_irq = crate::hal::vdpu2::run_job(&mut self.svc, &registers, &offsets)?;
        self.jobs += 1;
        // `RFI_HEVC_REG_DUMP` and `RFI_REG_DUMP` do the same for the other
        // decoders: append the image that was submitted, in the text
        // `hal_m2vd_debug=2` uses, so the two dumps diff.
        if std::env::var_os("RFI_M2V_REG_DUMP").is_some() {
            crate::hal::vdpu2::dump_registers_to(
                "/tmp/our_m2v_regs.txt",
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

    /// The values below are `hal_m2vd_debug=2` register dumps of
    /// `tests/data/m2v_320x240_12.m2v` on t6, with the fd words replaced by
    /// stand-ins. They pin every field the C HAL writes.
    fn job(coding: u32, stream_len: u32, start_bit: u32, f: [[u32; 2]; 2]) -> PictureJob {
        PictureJob {
            mb_width: 20,
            mb_height_p: 15,
            has_sequence_extension: true,
            progressive_sequence: true,
            picture_structure: STRUCT_FRAME,
            top_field_first: false,
            picture_coding_type: coding,
            intra_dc_precision: 0,
            q_scale_type: false,
            intra_vlc_format: false,
            concealment_motion_vectors: false,
            alternate_scan: false,
            frame_pred_frame_dct: true,
            forward_full_pel: f[0][0],
            forward_f_code: f[0][1],
            backward_full_pel: f[1][0],
            backward_f_code: f[1][1],
            stream_len,
            stream_offset: 40,
            stream_start_bit: start_bit,
            stream_fd: 16,
            cur_fd: 17,
            cur_offset: 0,
            refs: [17, 17, 17, 17],
            qp_fd: 15,
        }
    }

    #[test]
    fn i_picture_matches_the_c_register_file() {
        let regs = build_registers(&job(CODING_I, 8017, 8, [[15, 15], [15, 15]]));
        assert_eq!(regs[REG_FILTER], 0x0000_0100);
        assert_eq!(regs[REG_STREAM_LEN], 0x0200_1f51);
        assert_eq!(regs[REG_START_MB], 0x0010_0000);
        assert_eq!(regs[REG_DEC_MODE], 5);
        assert_eq!(regs[REG_ENDIAN], 0x3f);
        assert_eq!(regs[REG_BUS], 0x0010_0000);
        assert_eq!(regs[REG_PIC_CTRL], 0x0000_0031);
        assert_eq!(regs[REG_QP_TABLE], 15);
        assert_eq!(regs[REG_CUR_PIC], 17);
        assert_eq!(regs[REG_VLC_BASE], 16);
        assert_eq!(regs[REG_PIC_SIZE], 0x0a00_7800);
        assert_eq!(regs[REG_PIC_EXT], 0x2000_0001);
        assert_eq!(regs[REG_MV_ACC], 0x0007_fffe);
        assert_eq!(regs[REG_REF0], 17);
        assert_eq!(regs[REG_REF1], 17);
        assert_eq!(regs[REG_REF2], 17);
        assert_eq!(regs[REG_REF3], 17);
        // The post-processor window stays at reset.
        assert_eq!(regs[..50], [0u32; 50]);
    }

    #[test]
    fn p_and_b_pictures_set_the_prediction_flags() {
        let regs = build_registers(&job(CODING_P, 4757, 16, [[1, 1], [15, 15]]));
        assert_eq!(regs[REG_PIC_CTRL], 0x0000_4031);
        assert_eq!(regs[REG_PIC_EXT], 0x4000_0001);
        assert_eq!(regs[REG_MV_ACC], 0x0000_8ffe);

        let regs = build_registers(&job(CODING_B, 573, 16, [[1, 1], [1, 1]]));
        assert_eq!(regs[REG_PIC_CTRL], 0x0000_c031);
        assert_eq!(regs[REG_MV_ACC], 0x0000_888e);
    }

    #[test]
    fn mpeg1_jobs_fold_full_pel_into_the_vector_accuracy() {
        // `[full_pel, f_code]` per direction: forward is full-pel, backward is not.
        let mut job = job(CODING_P, 1024, 0, [[1, 2], [0, 4]]);
        job.has_sequence_extension = false;
        let regs = build_registers(&job);
        assert_eq!(regs[REG_DEC_MODE], 6);
        // forward_f_code 2 for both directions, full-pel set -> accuracy 0.
        assert_eq!(regs[REG_MV_ACC] & (1 << 1), 1 << 1, "mv_accuracy_bwd");
        assert_eq!(regs[REG_MV_ACC] & (1 << 2), 0, "mv_accuracy_fwd");
        assert_eq!((regs[REG_MV_ACC] >> 11) & 0xf, 2, "fcode_fwd_ver");
        assert_eq!((regs[REG_MV_ACC] >> 15) & 0xf, 2, "fcode_fwd_hor");
    }

    #[test]
    fn field_coded_pictures_select_the_field_mode() {
        let mut job = job(CODING_P, 512, 0, [[1, 1], [1, 1]]);
        job.progressive_sequence = false;
        job.top_field_first = true;

        job.picture_structure = STRUCT_TOP_FIELD;
        let regs = build_registers(&job);
        assert_eq!(regs[REG_PIC_CTRL] & (1 << 13), 1 << 13, "pic_topfield_e");
        assert_eq!(regs[REG_PIC_CTRL] & (1 << 16), 1 << 16, "pic_fieldmode_e");
        assert_eq!(regs[REG_PIC_CTRL] & (1 << 17), 1 << 17, "pic_interlace_e");

        job.picture_structure = STRUCT_BOTTOM_FIELD;
        let regs = build_registers(&job);
        assert_eq!(
            regs[REG_PIC_CTRL] & (1 << 13),
            0,
            "pic_topfield_e follows the picture structure, not top_field_first"
        );
        assert_eq!(regs[REG_PIC_CTRL] & (1 << 16), 1 << 16, "pic_fieldmode_e");
        assert_eq!(regs[REG_PIC_SIZE] & (1 << 5), 1 << 5, "topfieldfirst_e");
    }

    #[test]
    fn only_nonzero_address_offsets_are_sent() {
        let mut job = job(CODING_P, 512, 0, [[1, 1], [1, 1]]);
        job.stream_offset = 0;
        assert!(build_offsets(&job).is_empty());
        job.cur_offset = 320;
        assert_eq!(
            build_offsets(&job),
            vec![RegOffsetInfo {
                reg_idx: REG_CUR_PIC as u32,
                offset: 320
            }]
        );
    }

    #[test]
    fn frame_layout_aligns_to_sixteens() {
        assert_eq!(frame_layout(320, 240).0, 320);
        assert_eq!(frame_layout(350, 242).0, 352);
        assert_eq!(frame_layout(350, 242).1, 256);
        let (_, _, size) = frame_layout(320, 240);
        assert_eq!(size, 320 * 240 * 3 / 2 + 320 * 16);
    }
}
