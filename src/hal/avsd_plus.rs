//! RK3588 AVS+ decoder on client 12.
//!
//! This is the 60-word `hal_avsd_plus_reg.h` register file, not the 159-word
//! VDPU2 legacy window. The first implementation targets I pictures; the same
//! file also carries the AVS+ weighting, AEC, motion-vector and reference fields
//! needed when PB picture scheduling is added.

use std::io;

use crate::codec::avs::parser::{
    PictureHeader, SequenceHeader, B_PICTURE, FRAME_PICTURE, I_PICTURE, P_PICTURE,
};
use crate::hal::vdpu2::Word;
use crate::service::dma_heap::DmaBuf;
use crate::service::{Batch, Client, MppService, RegOffsetInfo};

pub const REG_NUM: usize = 60;
pub const REG_STATUS: usize = 1;
pub const REG_STREAM_BASE: usize = 12;
pub const REG_OUTPUT: usize = 13;
pub const REG_REF0: usize = 14;
pub const REG_REF1: usize = 15;
pub const REG_REF2: usize = 16;
pub const REG_REF3: usize = 17;
pub const REG_MV_BASE: usize = 41;
pub const REG_MV_BASE2: usize = 45;

/// C's AVS+ HAL allocates `1920 * 1088 * 2` bytes for the shared MV buffer.
pub const MV_BUFFER_SIZE: usize = 1920 * 1088 * 2;

pub const IRQ_RDY: u32 = 1 << 12;
pub const IRQ_BUFFER: u32 = 1 << 14;
pub const IRQ_ERROR: u32 = 1 << 16;

#[derive(Debug, Clone)]
pub struct PictureJob {
    pub sequence: SequenceHeader,
    pub picture: PictureHeader,
    /// Bytes the hardware may read, starting at `stream_offset`.
    pub stream_len: u32,
    /// Eight-byte aligned byte below the first slice start code.
    pub stream_offset: u32,
    /// Bit position of the first slice inside `stream_offset`.
    pub stream_start_bit: u32,
    pub stream_fd: u32,
    pub output_fd: u32,
    pub mv_fd: u32,
    /// Resolved reference fds; I pictures use the output surface.
    pub refs: [u32; 4],
    pub prev_anchor_type: bool,
    /// Previous decoded distances, used by P/B pictures.
    pub work0_distance: Option<u32>,
    pub work1_distance: Option<u32>,
    pub future_to_previous_distance: u32,
}

impl PictureJob {
    pub fn is_i_picture(&self) -> bool {
        self.picture.picture_coding_type == I_PICTURE
    }
}

fn inverse(distance: u32) -> u32 {
    512 / distance.max(1)
}

fn inverse_column(distance: u32) -> u32 {
    16384 / distance.max(1)
}

pub fn build_registers(job: &PictureJob) -> [u32; REG_NUM] {
    let mut regs = [0u32; REG_NUM];
    let picture = &job.picture;
    let avs_plus = job.sequence.is_avs_plus();
    let frame = picture.picture_structure == FRAME_PICTURE;
    let b_picture = picture.picture_coding_type == B_PICTURE;
    let p_picture = picture.picture_coding_type == P_PICTURE;
    let i_picture = picture.picture_coding_type == I_PICTURE;
    let inter = !i_picture;

    regs[1] = Word::default().field(1, 1).field(0, 31).finish();
    regs[2] = Word::default()
        .field(16, 5) // dec_max_burst
        .field(0, 1) // dec_scmd_dis
        .field(0, 1) // dec_adv_pre_dis
        .field(0, 1) // tiled_mode_lsb
        .field(1, 1) // dec_out_endian
        .field(0, 1) // dec_in_endian
        .field(1, 1) // dec_clk_gate_e
        .field(0, 6) // dec_latency
        .field(0, 1) // dec_out_tiled_e
        .field(0, 1) // dec_data_disc_e
        .field(1, 1) // dec_outswap32_e
        .field(1, 1) // dec_inswap32_e
        .field(1, 1) // dec_strendian_e
        .field(1, 1) // dec_strswap32_e
        .field(0, 1) // dec_timeout_e
        .field(0xff, 8) // dec_axi_rd_id
        .finish();

    let write_mvs = if avs_plus {
        p_picture || i_picture
    } else {
        p_picture || (i_picture && !frame)
    };
    regs[3] = Word::default()
        .field(0, 8) // dec_axi_wr_id
        .field(0, 1) // dec_ahb_hlock_e
        .field(0, 1) // picord_count_e
        .field(0, 1) // reserve
        .field(0, 1) // reftopfirst_e
        .field(u32::from(write_mvs), 1)
        .field(u32::from(picture.fixed_picture_qp), 1)
        .field(u32::from(picture.loop_filter_disable), 1)
        .field(0, 1) // dec_out_dis
        .field(0, 1) // ref_topfield_e
        .field(0, 1) // reserve
        .field(0, 1) // fwd_interlace_e
        .field(u32::from(!frame && picture.top_field_first), 1)
        .field(u32::from(inter), 1)
        .field(u32::from(b_picture), 1)
        .field(u32::from(!frame), 1)
        .field(u32::from(!frame), 1)
        .field(0, 2) // reserve
        .field(u32::from(picture.skip_mode_flag), 1)
        .field(0, 1) // rlc_mode_e
        .field(11, 4) // DEC_MODE_AVS
        .finish();

    let mb_width = job.sequence.horizontal_size.div_ceil(16);
    let mb_height = job.sequence.vertical_size.div_ceil(16);
    regs[4] = Word::default()
        .field(u32::from(picture.picture_reference_flag), 1)
        .field(0, 10)
        .field(mb_height, 8)
        .field(0, 4) // mb_width_off
        .field(mb_width, 9)
        .finish();
    regs[5] = Word::default()
        .field(picture.beta_offset as u32, 5)
        .field(picture.alpha_offset as u32, 5)
        .field(0, 16)
        .field(job.stream_start_bit, 6)
        .finish();
    regs[6] = Word::default()
        .field(job.stream_len, 24)
        .field(0, 1) // stream_len_ext
        .field(picture.picture_qp, 6)
        .field(0, 1) // start_code_e
        .finish();
    regs[7] = Word::default()
        .field(0, 25)
        .field(mb_height >> 12, 1)
        .field(0, 6)
        .finish();

    regs[REG_STREAM_BASE] = job.stream_fd;
    regs[REG_OUTPUT] = job.output_fd;
    regs[REG_REF0] = job.refs[0];
    regs[REG_REF1] = job.refs[1];
    regs[REG_REF2] = job.refs[2];
    regs[REG_REF3] = job.refs[3];
    regs[18] = Word::default()
        .field(u32::from(job.prev_anchor_type), 1)
        .field(0, 31)
        .finish();

    let mut current_0 = 0u32;
    let mut current_1 = 0u32;
    let mut current_2 = 0u32;
    let mut current_3 = 0u32;
    let mut column_0 = 0u32;
    let mut column_1 = 0u32;
    let mut column_2 = 0u32;
    let mut column_3 = 0u32;
    if b_picture && frame {
        if let Some(work0) = job.work0_distance {
            current_2 = (2 * work0 + 512).wrapping_sub(2 * picture.picture_distance) & 0x1ff;
            if current_2 == 0 {
                current_2 = 2;
            }
            current_3 = current_2;
        }
        if let Some(work1) = job.work1_distance {
            current_0 = (2 * picture.picture_distance).wrapping_sub(2 * work1 + 512) & 0x1ff;
            if current_0 == 0 {
                current_0 = 2;
            }
            current_1 = current_0;
        }
        if let (Some(work0), Some(work1)) = (job.work0_distance, job.work1_distance) {
            let distance = (2 * work0).wrapping_sub(2 * work1 + 512) & 0x1ff;
            let distance = if distance == 0 { 2 } else { distance };
            column_0 = inverse_column(distance);
            column_1 = column_0;
        }
        column_2 = inverse_column(job.future_to_previous_distance);
        column_3 = column_2;
    } else if !b_picture && frame {
        if let Some(work0) = job.work0_distance {
            current_0 = (2 * picture.picture_distance).wrapping_sub(2 * work0 + 512) & 0x1ff;
        }
        if current_0 == 0 {
            current_0 = 2;
        }
        current_1 = current_0;
        if let Some(work1) = job.work1_distance {
            current_2 = (2 * picture.picture_distance).wrapping_sub(2 * work1 + 512) & 0x1ff;
        }
        if current_2 == 0 {
            current_2 = 2;
        }
        current_3 = current_2;
    }
    regs[28] = Word::default()
        .field(inverse(current_0), 16)
        .field(inverse(current_1), 16)
        .finish();
    regs[29] = Word::default()
        .field(inverse(current_2), 16)
        .field(inverse(current_3), 16)
        .finish();
    regs[30] = Word::default()
        .field(current_0, 16)
        .field(current_1, 16)
        .finish();
    regs[31] = Word::default()
        .field(current_2, 16)
        .field(current_3, 16)
        .finish();
    regs[32] = Word::default()
        .field(column_0, 16)
        .field(column_1, 16)
        .finish();
    regs[33] = Word::default()
        .field(column_2, 16)
        .field(column_3, 16)
        .finish();

    // The predictor taps are written as signed ten-bit fields in C.
    regs[34] = Word::default()
        .field(0, 2)
        .field(7, 10)
        .field(1, 10)
        .field((-1i32) as u32, 10)
        .finish();
    regs[35] = Word::default()
        .field(0, 12)
        .field(1, 10)
        .field(7, 10)
        .finish();
    regs[REG_MV_BASE] = job.mv_fd;
    regs[42] = Word::default()
        .field(0, 3)
        .field(0, 3)
        .field(0, 3)
        .field(0, 3)
        .field(0, 3)
        .field(0, 3)
        .field(0, 3)
        .field(0, 3)
        .field(
            if picture.weighting_quant_flag {
                picture.weighting_quant_param[1]
            } else {
                0
            },
            8,
        )
        .finish();
    regs[43] = Word::default()
        .field(
            if picture.weighting_quant_flag {
                picture.weighting_quant_param[5]
            } else {
                0
            },
            8,
        )
        .field(
            if picture.weighting_quant_flag {
                picture.weighting_quant_param[4]
            } else {
                0
            },
            8,
        )
        .field(
            if picture.weighting_quant_flag {
                picture.weighting_quant_param[3]
            } else {
                0
            },
            8,
        )
        .field(
            if picture.weighting_quant_flag {
                picture.weighting_quant_param[2]
            } else {
                0
            },
            8,
        )
        .finish();
    regs[44] = Word::default()
        .field(
            if picture.weighting_quant_flag {
                picture.weighting_quant_param[0]
            } else {
                0
            },
            8,
        )
        .field(
            if picture.weighting_quant_flag && !picture.chroma_quant_param_disable {
                picture.chroma_quant_param_delta_cr as u32
            } else {
                0
            },
            6,
        )
        .field(
            if picture.weighting_quant_flag && !picture.chroma_quant_param_disable {
                picture.chroma_quant_param_delta_cb as u32
            } else {
                0
            },
            6,
        )
        .field(u32::from(picture.pb_field_enhanced_flag), 1)
        .field(u32::from(picture.no_forward_reference_flag), 1)
        .field(u32::from(picture.aec_enable), 1)
        .field(picture.weighting_quant_model, 2)
        .field(u32::from(picture.weighting_quant_flag), 1)
        .field(u32::from(avs_plus), 1)
        .field(0, 5)
        .finish();
    if avs_plus {
        regs[REG_MV_BASE2] = job.mv_fd;
    }
    regs[48] = Word::default()
        .field(0, 14)
        .field(0, 9)
        .field(0, 9)
        .finish();
    regs[49] = Word::default()
        .field(0, 2)
        .field(5, 10)
        .field(5, 10)
        .field((-1i32) as u32, 10)
        .finish();
    regs[55] = Word::default()
        .field(8, 14) // apf_threshold
        .field(0, 5)
        .field(0, 12)
        .field(0, 1)
        .finish();
    regs
}

pub fn build_offsets(job: &PictureJob) -> Vec<RegOffsetInfo> {
    let mut offsets = Vec::with_capacity(3);
    if job.stream_offset != 0 {
        offsets.push(RegOffsetInfo {
            reg_idx: REG_STREAM_BASE as u32,
            offset: job.stream_offset,
        });
    }
    // C leaves `prev_pic_structure` at zero on the first frame and applies
    // these field offsets to the duplicated reference addresses.
    offsets.push(RegOffsetInfo {
        reg_idx: REG_REF2 as u32,
        offset: 2,
    });
    offsets.push(RegOffsetInfo {
        reg_idx: REG_REF3 as u32,
        offset: 3,
    });
    offsets
}

pub struct AvsdPlus {
    service: MppService,
    mv: DmaBuf,
    jobs: usize,
    pub last_irq: u32,
}

impl AvsdPlus {
    pub fn open() -> io::Result<Self> {
        Ok(Self {
            service: MppService::open(Client::AvsPlusDec as u32)?,
            mv: DmaBuf::alloc("system", MV_BUFFER_SIZE, false)?,
            jobs: 0,
            last_irq: 0,
        })
    }

    pub fn mv_fd(&self) -> u32 {
        self.mv.fd() as u32
    }

    pub fn decode(&mut self, job: &PictureJob) -> io::Result<u32> {
        let registers = build_registers(job);
        let offsets = build_offsets(job);
        let mut readback = [0u32; REG_NUM];
        let mut batch = Batch::new();
        batch
            .reg_write(&registers, 0)
            .reg_read(&mut readback, 0)
            .reg_offsets(&offsets);
        self.service.submit_batch_recovering(&mut batch)?;
        self.service.poll_hw_finish_recovering()?;
        self.jobs += 1;
        self.last_irq = readback[REG_STATUS];
        if std::env::var_os("RFI_AVS_REG_DUMP").is_some() {
            let mut text = String::new();
            text.push_str(&format!("JOB {}\n", self.jobs));
            for (index, word) in registers.iter().enumerate() {
                text.push_str(&format!("reg[{index:02}] = {word:08x}\n"));
            }
            text.push_str("READBACK\n");
            for (index, word) in readback.iter().enumerate() {
                text.push_str(&format!("reg[{index:02}] = {word:08x}\n"));
            }
            let _ = std::fs::write("/tmp/our_avs_regs.txt", text);
        }
        if self.last_irq & (IRQ_ERROR | IRQ_BUFFER) != 0 || self.last_irq & IRQ_RDY == 0 {
            self.service.restart_after_hardware_error()?;
        }
        Ok(self.last_irq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::avs::parser::FRAME_PICTURE;

    fn public_i_job() -> PictureJob {
        PictureJob {
            sequence: SequenceHeader {
                profile_id: 0x20,
                level_id: 0x10,
                progressive_sequence: true,
                horizontal_size: 352,
                vertical_size: 288,
                chroma_format: 1,
                sample_precision: 1,
                aspect_ratio: 1,
                frame_rate_code: 3,
                bit_rate: 0x9c4000,
                low_delay: false,
                bbv_buffer_size: 7,
            },
            picture: PictureHeader {
                picture_coding_type: I_PICTURE,
                bbv_delay: 0xffff,
                bbv_delay_extension: 0,
                time_code_flag: true,
                time_code: 0,
                picture_distance: 0,
                bbv_check_times: 0,
                progressive_frame: true,
                picture_structure: FRAME_PICTURE,
                top_field_first: false,
                repeat_first_field: false,
                fixed_picture_qp: true,
                picture_qp: 40,
                picture_reference_flag: false,
                skip_mode_flag: false,
                no_forward_reference_flag: false,
                pb_field_enhanced_flag: false,
                loop_filter_disable: false,
                loop_filter_parameter_flag: false,
                alpha_offset: 0,
                beta_offset: 0,
                weighting_quant_flag: false,
                chroma_quant_param_disable: false,
                chroma_quant_param_delta_cb: 0,
                chroma_quant_param_delta_cr: 0,
                weighting_quant_param_index: 0,
                weighting_quant_model: 0,
                weighting_quant_param: [128; 6],
                aec_enable: false,
                header_bits: 66,
            },
            stream_len: 6128,
            stream_offset: 128,
            stream_start_bit: 24,
            stream_fd: 0x16,
            output_fd: 0x17,
            mv_fd: 0x10,
            refs: [0x17; 4],
            prev_anchor_type: true,
            work0_distance: None,
            work1_distance: None,
            future_to_previous_distance: 2,
        }
    }

    #[test]
    fn public_i_picture_matches_c_register_file() {
        let regs = build_registers(&public_i_job());
        assert_eq!(
            regs,
            [
                0x00000000, 0x00000001, 0xff780510, 0xb0002000, 0x0b009000, 0x60000000, 0x500017f0,
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000016, 0x00000017,
                0x00000017, 0x00000017, 0x00000017, 0x00000017, 0x00000001, 0x00000000, 0x00000000,
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
                0x01000100, 0x01000100, 0x00020002, 0x00020002, 0x00000000, 0x00000000, 0xffc0101c,
                0x01c01000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000010,
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
                0xffc05014, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000008,
                0x00000000, 0x00000000, 0x00000000, 0x00000000,
            ]
        );
    }

    #[test]
    fn avs_plus_profile_sets_extension_and_second_mv_base() {
        let mut job = public_i_job();
        job.sequence.profile_id = 0x48;
        let regs = build_registers(&job);
        assert_eq!(regs[3] & (1 << 12), 1 << 12, "write_mvs_e");
        assert_eq!(regs[44] & (1 << 26), 1 << 26, "dec_avsp_ena");
        assert_eq!(regs[45], job.mv_fd);
    }
}
