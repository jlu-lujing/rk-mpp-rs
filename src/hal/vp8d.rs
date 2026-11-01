//! VP8 decode on VDPU2 (`mpp/hal/vpu/vp8d/hal_vp8d_vdpu2.c`).
//!
//! The same 159-word window and client 1 as the other legacy codecs (see
//! [`crate::hal::vdpu2`]). Two buffers live for the life of the channel: the
//! 64 KiB probability table the C HAL calls `probe_table`, and the 3072-byte
//! segment map. Both are program-order inputs to every job: the probability
//! image is rewritten from the parsed header each picture, and the segment map
//! is cleared **only** on a key frame, because an inter picture that enables
//! segmentation without updating the map continues to consume what the hardware
//! wrote last time.
//!
//! `hal_vp8_init_hwcfg` memsets the whole register file at the start of every
//! job, so unlike the MPEG codecs nothing carries over in the register image:
//! [`build_registers`] starts from zero, exactly like the C code.

use std::io;

use crate::codec::vp8::parser::EntropyProbs;
use crate::hal::vdpu2::{self, Word, BUS_BURST_16, REG_NUM};
use crate::service::dma_heap::DmaBuf;
use crate::service::{Client, MppService, RegOffsetInfo};

pub use crate::hal::vdpu2::{
    frame_layout, Frame, IRQ_FAILURE, REG_BUS, REG_CUR_PIC, REG_DEC_MODE, REG_ENDIAN, REG_FILTER,
    REG_IRQ, REG_PIC_CTRL, REG_PIC_EXT, REG_PIC_SIZE, REG_START_MB, REG_STREAM_LEN,
};

/// `DEC_MODE_VP8`. The VP7 mode is unreachable; see [`crate::codec::vp8::parser`].
pub const DEC_MODE_VP8: u32 = 10;

/// `VP8D_PROB_TABLE_SIZE`
pub const PROB_TABLE_SIZE: usize = 1 << 16;
/// The probability image actually written by `hal_vp8hw_asic_probe_update`.
/// Everything past it is never read by the decoder and stays zeroed here, where
/// the C buffer is simply uninitialised past that point.
pub const PROB_TABLE_WRITTEN: usize = 8 * 151;

/// `VP8D_MAX_SEGMAP_SIZE`
pub const SEG_MAP_SIZE: usize = 2048 + 1024;

/// Words the VP8 path adds to the shared set.
pub const REG_FILTER_LEVELS: usize = 129;
pub const REG_QUANT_0: usize = 130;
pub const REG_REF0: usize = 131;
pub const REG_FILTER_MODE: usize = 132;
pub const REG_REF_DELTAS: usize = 133;
pub const REG_GOLDEN: usize = 136;
pub const REG_ALT_REF: usize = 137;
pub const REG_DCT_STREAM_0: usize = 140;
/// `reg61_qtable_base`: the probability image.
pub const REG_PROB_TABLE: usize = 61;
/// `reg64_input_stream_base`: the first partition.
pub const REG_STREAM_0: usize = 64;
pub const REG_STREAM: usize = 145;
pub const REG_DCT_STREAM_1: usize = 146;
pub const REG_SEG_MAP: usize = 149;
pub const REG_DCT_START_BITS: usize = 150;
pub const REG_QUANT_2: usize = 151;
pub const REG_QUANT_4: usize = 152;
pub const REG_TAP_0_3: usize = 153;

/// `mcFilter` of `hal_vp8d_vdpu2.c`: the six-tap sub-pel banks, indexed by the
/// quarter-pel phase. Written only when `version` is zero.
pub const MC_FILTER: [[i32; 6]; 8] = [
    [0, 0, 128, 0, 0, 0],
    [0, -6, 123, 12, -1, 0],
    [2, -11, 108, 36, -8, 1],
    [0, -9, 93, 50, -6, 0],
    [3, -16, 77, 77, -16, 3],
    [0, -6, 50, 93, -9, 0],
    [1, -8, 36, 108, -11, 2],
    [0, -1, 12, 123, -6, 0],
];

/// Everything the register file needs for one picture, with the three reference
/// buffers already resolved to dma-buf fds.
#[derive(Debug, Clone)]
pub struct PictureJob {
    pub mb_width: u32,
    pub mb_height: u32,
    /// `frame_type`: zero for a key frame.
    pub frame_type: u32,
    pub version: u8,

    pub loop_filter_type: u32,
    pub loop_filter_level: u32,
    pub sharpness: u32,
    pub mode_ref_lf_delta_enabled: bool,
    pub ref_lf_deltas: [i8; 4],
    pub mode_lf_deltas: [i8; 4],

    pub segmentation_enabled: bool,
    pub segmentation_map_update: bool,
    pub segment_feature_mode: bool,
    pub segment_qp: [i8; 4],
    pub segment_loopfilter: [i8; 4],

    /// `base_qindex`, which is also what `sw130.sw_quant_0` takes in the
    /// non-segmented case.
    pub y1ac_delta_q: i8,
    pub y1dc_delta_q: i8,
    pub y2dc_delta_q: i8,
    pub y2ac_delta_q: i8,
    pub uvdc_delta_q: i8,
    pub uvac_delta_q: i8,

    pub mb_no_coeff_skip: bool,
    pub bool_value: u32,
    pub bool_range: u32,

    pub stream_start_offset: u32,
    pub stream_start_bit: u32,
    pub stream_end_pos: u32,
    pub frame_tag_size: u32,
    pub offset_to_dct_parts: u32,
    pub log2_dct_partitions: u8,
    pub dct_partition_offsets: [u32; 8],

    /// The probabilities this picture is decoded with.
    pub entropy: EntropyProbs,
    pub prob_skip_false: u8,
    pub prob_intra: u8,
    pub prob_last: u8,
    pub prob_golden: u8,
    pub segment_tree_probs: [u8; 3],
    /// `ref_frame_sign_bias_golden` / `..._altref`, which the C HAL programs as
    /// one-byte address offsets on the two reference base registers.
    pub sign_bias_golden: u32,
    pub sign_bias_altref: u32,

    pub stream_fd: u32,
    pub cur_fd: u32,
    /// `sw131` / `sw136` / `sw137`.
    pub refs: [u32; 3],
    pub probe_fd: u32,
    pub seg_map_fd: u32,
}

impl PictureJob {
    /// The stream base the DCT partitions are addressed from, in the same
    /// 8-byte-aligned form the register offsets require.
    fn partition_base(&self, index: usize) -> u32 {
        let extra: u32 = if self.frame_type == 0 { 7 } else { 0 };
        let address = extra.wrapping_add(self.dct_partition_offsets[index]);
        address & !0x7
    }

    /// The low three bits of that base, in bits, for the per-partition start
    /// word fields.
    fn partition_start_bit(&self, index: usize) -> u32 {
        let extra: u32 = if self.frame_type == 0 { 7 } else { 0 };
        ((extra.wrapping_add(self.dct_partition_offsets[index])) & 0x7) * 8
    }

    /// The start word of a partition that the picture does not carry is never
    /// written, so it keeps the zero the register file starts from.
    fn partition_start_bit_if(&self, index: usize) -> u32 {
        if index < (1 << self.log2_dct_partitions) {
            self.partition_start_bit(index)
        } else {
            0
        }
    }

    /// `sw51.sw_stream_len`: how far the primary stream reader may run. Every
    /// step is the C expression, which is unsigned arithmetic that is allowed to
    /// wrap on a damaged stream, so it is written as wrapping arithmetic here.
    pub fn stream_len(&self) -> u32 {
        let extra: u32 = if self.frame_type == 0 { 7 } else { 0 };
        let partitions = (1 << self.log2_dct_partitions) - 1;
        self.stream_end_pos
            .wrapping_add(self.frame_tag_size)
            .wrapping_sub(self.dct_partition_offsets[0])
            .wrapping_add(partitions * 3)
            .wrapping_add((extra.wrapping_add(self.dct_partition_offsets[0])) & 0x7)
    }

    /// `sw124.sw_stream1_len`: the first partition, from where the boolean
    /// coder left off.
    pub fn first_partition_len(&self) -> u32 {
        let extra: u32 = if self.frame_type == 0 { 7 } else { 0 };
        self.offset_to_dct_parts
            .wrapping_add(self.frame_tag_size)
            .wrapping_sub(self.stream_start_offset.wrapping_sub(extra))
            .wrapping_add(1)
    }
}

/// The probability image the hardware reads through `sw61`, in the byte order
/// `hal_vp8hw_asic_probe_update` lays it out.
pub fn build_prob_table(job: &PictureJob) -> [u8; PROB_TABLE_WRITTEN] {
    let mut table = [0u8; PROB_TABLE_WRITTEN];
    let probs = &job.entropy;
    let mut at = 0usize;
    let mut put = |value: u8| {
        table[at] = value;
        at += 1;
    };

    put(job.prob_skip_false);
    put(job.prob_intra);
    put(job.prob_last);
    put(job.prob_golden);
    put(job.segment_tree_probs[0]);
    put(job.segment_tree_probs[1]);
    put(job.segment_tree_probs[2]);
    put(0);

    for value in job.entropy.luma16x16_pred {
        put(value);
    }
    for value in job.entropy.chroma_pred {
        put(value);
    }
    put(0);

    // The is-short and sign probabilities of both components, then the two
    // widest buckets of each, which is what the header of the mv tree reads.
    put(probs.mv[0][0]);
    put(probs.mv[1][0]);
    put(probs.mv[0][1]);
    put(probs.mv[1][1]);
    put(probs.mv[0][17]);
    put(probs.mv[0][18]);
    put(probs.mv[1][17]);
    put(probs.mv[1][18]);

    for component in 0..2 {
        for low in (0..8).step_by(4) {
            for index in 0..4 {
                put(probs.mv[component][low + 9 + index]);
            }
        }
    }
    // The remaining buckets of each component, with an unused byte each. These
    // sit at 40 and 48, just below where the coefficient probabilities start, so
    // the tree heads above and the coefficients below leave no room for them.
    for component in 0..2 {
        for index in 2..9 {
            put(probs.mv[component][index]);
        }
        put(0);
    }

    // `vp8hwdReadBits`-style header coefficients: the first four tokens of each
    // plane / class / context, four bytes apiece at offset 56.
    let mut at = 8 * 7;
    for context in contexts(probs) {
        table[at..at + 4].copy_from_slice(&context[..4]);
        at += 4;
    }

    // The remaining seven tokens, eight bytes each with a pad, at offset 440.
    let mut at = 8 * 55;
    for context in contexts(probs) {
        table[at] = context[4];
        table[at + 1] = context[5];
        table[at + 2] = context[6];
        table[at + 3] = context[7];
        table[at + 4] = context[8];
        table[at + 5] = context[9];
        table[at + 6] = context[10];
        at += 8;
    }
    table
}

/// The 96 coefficient contexts of one probability set, in the plane, token-tree
/// and context order both coefficient halves of the probe table walk.
fn contexts(probs: &EntropyProbs) -> impl Iterator<Item = &[u8; 11]> {
    probs
        .coeffs
        .iter()
        .flat_map(|plane| plane.iter())
        .flat_map(|tree| tree.iter())
}

/// The interpolation filter banks, word by word, exactly as
/// `hal_vp8d_pre_filter_tap_set` distributes `mcFilter`.
///
/// The field names and the array indices are offset by one: `sw_pred_bc_tap_1_1`
/// takes `mcFilter[1][2]`, because column zero of each bank is the `-1` phase and
/// column five is the `+4` phase. The words disagree about which tap comes first
/// as well - `sw153` ends with the tail of bank zero and `sw127` with the tail of
/// bank six - so every field is written by name rather than by a loop.
fn write_filter_taps(regs: &mut [u32; REG_NUM]) {
    // The field `tap_<bank>_<k>` of the C register file.
    let tap = |bank: usize, phase: i32| -> u32 {
        u32::from_ne_bytes(MC_FILTER[bank][(phase + 1) as usize].to_ne_bytes())
    };

    // sw59: the head of bank zero.
    regs[59] = Word::default()
        .field(0, 2)
        .field(tap(0, 2), 10)
        .field(tap(0, 1), 10)
        .field(tap(0, 0), 10)
        .finish();
    regs[REG_TAP_0_3] = Word::default()
        .field(0, 2)
        .field(tap(1, 1), 10)
        .field(tap(1, 0), 10)
        .field(tap(0, 3), 10)
        .finish();
    regs[154] = Word::default()
        .field(0, 2)
        .field(tap(2, 0), 10)
        .field(tap(1, 3), 10)
        .field(tap(1, 2), 10)
        .finish();
    regs[155] = Word::default()
        .field(0, 2)
        .field(tap(2, 3), 10)
        .field(tap(2, 2), 10)
        .field(tap(2, 1), 10)
        .finish();
    regs[156] = Word::default()
        .field(0, 2)
        .field(tap(3, 2), 10)
        .field(tap(3, 1), 10)
        .field(tap(3, 0), 10)
        .finish();
    regs[157] = Word::default()
        .field(0, 2)
        .field(tap(4, 1), 10)
        .field(tap(4, 0), 10)
        .field(tap(3, 3), 10)
        .finish();
    regs[158] = Word::default()
        .field(0, 2)
        .field(tap(5, 0), 10)
        .field(tap(4, 3), 10)
        .field(tap(4, 2), 10)
        .finish();
    regs[125] = Word::default()
        .field(0, 2)
        .field(tap(5, 3), 10)
        .field(tap(5, 2), 10)
        .field(tap(5, 1), 10)
        .finish();
    regs[126] = Word::default()
        .field(0, 2)
        .field(tap(6, 2), 10)
        .field(tap(6, 1), 10)
        .field(tap(6, 0), 10)
        .finish();
    regs[127] = Word::default()
        .field(0, 2)
        .field(tap(7, 1), 10)
        .field(tap(7, 0), 10)
        .field(tap(6, 3), 10)
        .finish();
    // sw128 puts the six two-bit extreme phases below the last ten-bit pair.
    regs[128] = Word::default()
        .field(tap(6, 4), 2)
        .field(tap(6, -1), 2)
        .field(tap(4, 4), 2)
        .field(tap(4, -1), 2)
        .field(tap(2, 4), 2)
        .field(tap(2, -1), 2)
        .field(tap(7, 3), 10)
        .field(tap(7, 2), 10)
        .finish();
}

/// Build the 159-word register file for one picture.
pub fn build_registers(job: &PictureJob) -> [u32; REG_NUM] {
    let mut regs = [0u32; REG_NUM];

    // sw50: deblocking is off only when the loop filter level is zero, which is
    // the one control the C HAL sets conditionally here.
    regs[REG_FILTER] = Word::default()
        .field(0, 1) // dec_out_tiled_e
        .field(0, 6) // dec_latency
        .field(0, 1) // pic_fixed_quant
        .field(u32::from(job.loop_filter_level == 0), 1) // filtering_dis
        .field(u32::from(!job.mb_no_coeff_skip), 1) // skip_mode
        .field(0, 1) // dec_scmd_dis
        .field(0, 1) // dec_adv_pre_dis
        .field(0, 1) // priority_mode
        .field(0, 12) // refbu2_thr
        .field(0, 5) // refbu2_picid
        .field(0, 2)
        .finish();

    regs[REG_STREAM_LEN] = Word::default()
        .field(job.stream_len(), 24)
        .field(0, 1)
        .field(0, 6) // init_qp: never written for VP8
        .field(0, 1)
        .finish();

    // sw52 stays at zero; the MPEG codecs put their prefetch threshold here.
    regs[REG_START_MB] = 0;
    regs[REG_DEC_MODE] = DEC_MODE_VP8;

    regs[REG_ENDIAN] = Word::default()
        .field(1, 1)
        .field(1, 1)
        .field(1, 1)
        .field(1, 1)
        .field(1, 1)
        .field(1, 1)
        .field(0, 26)
        .finish();

    regs[REG_IRQ] = 0;

    regs[REG_BUS] = Word::default()
        .field(0, 8) // axi_rn_id
        .field(0, 8) // axi_wr_id
        .field(BUS_BURST_16, 5)
        .field(0, 1)
        .field(0, 1) // data_disc_e
        .field(0, 9)
        .finish();

    regs[REG_PIC_CTRL] = Word::default()
        .field(1, 1) // dec_e, armed by hand below as well
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
        .field(job.frame_type, 1) // pic_inter_e
        .field(0, 1) // pic_b_e
        .field(0, 1) // pic_fieldmode_e
        .field(0, 1) // pic_interlace_e
        .field(0, 1) // pjpeg_e
        .field(0, 1) // divx3_e
        .field(0, 1) // rlc_mode_e
        .field(0, 1) // ch_8pix_ileav_e
        .field(0, 1) // start_code_e
        .field(0, 8)
        .field(0, 1)
        .finish();

    if job.version & 3 == 0 {
        write_filter_taps(&mut regs);
    }

    // sw61 is the probability image and sw149 the segment map; sw62, the direct
    // motion-vector buffer the MPEG codecs use, stays unwritten.
    regs[REG_CUR_PIC] = job.cur_fd;
    regs[REG_PROB_TABLE] = job.probe_fd;
    regs[62] = 0;
    regs[REG_STREAM_0] = job.stream_fd;

    regs[REG_PIC_SIZE] = Word::default()
        .field(job.mb_height >> 8, 3) // mb_h_ext
        .field(job.mb_width >> 9, 3) // mb_w_ext
        .field(0, 1) // alt_scan_e
        .field(0, 4) // mb_height_off
        .field(job.mb_height & 0xff, 8)
        .field(0, 4) // mb_width_off
        .field(job.mb_width & 0x1ff, 9)
        .finish();

    regs[121] = Word::default()
        .field(0, 5)
        .field(0, 1) // vp7_version
        .field(0, 3) // dc_match0
        .field(0, 3) // dc_match1
        .field(u32::from(job.version & 3 != 0), 1) // eable_bilinear
        .field(u32::from(job.version != 3), 1) // romain_mv
        .field(0, 6)
        // The extra start words belong to partitions that may not exist.
        .field(job.partition_start_bit_if(2), 6)
        .field(job.partition_start_bit_if(1), 6)
        .finish();

    regs[REG_PIC_EXT] = Word::default()
        .field(job.bool_range, 8)
        .field(job.bool_value, 8)
        .field(0, 1) // multistream_e
        .field(0, 1) // huffman_e
        .field(job.stream_start_bit, 6) // strm1_start_bit
        .field(0, 2)
        .field(job.partition_start_bit(0), 6) // strm_start_bit
        .finish();

    regs[124] = Word::default()
        .field(job.first_partition_len(), 24)
        .field((1 << job.log2_dct_partitions) - 1, 4) // coeffs_part_am
        .field(0, 4)
        .finish();

    // Loop filter strengths, three cases: no segmentation, absolute segment
    // features, and segment features as deltas of the picture level.
    if !job.segmentation_enabled {
        regs[REG_FILTER_LEVELS] = Word::default()
            .field(0, 6)
            .field(0, 6)
            .field(0, 6)
            .field(job.loop_filter_level, 6)
            .field(0, 8)
            .finish();
    } else if job.segment_feature_mode {
        let levels = job.segment_loopfilter.map(|value| value as u32);
        regs[REG_FILTER_LEVELS] = Word::default()
            .field(levels[3], 6)
            .field(levels[2], 6)
            .field(levels[1], 6)
            .field(levels[0], 6)
            .field(0, 8)
            .finish();
    } else {
        let levels = job
            .segment_loopfilter
            .map(|value| (job.loop_filter_level as i32 + i32::from(value)).clamp(0, 63) as u32);
        regs[REG_FILTER_LEVELS] = Word::default()
            .field(levels[3], 6)
            .field(levels[2], 6)
            .field(levels[1], 6)
            .field(levels[0], 6)
            .field(0, 8)
            .finish();
    }

    let quants = if !job.segmentation_enabled {
        [job.y1ac_delta_q as u32, 0, 0, 0]
    } else if job.segment_feature_mode {
        job.segment_qp.map(|value| value as u32)
    } else {
        job.segment_qp
            .map(|value| (i32::from(job.y1ac_delta_q) + i32::from(value)).clamp(0, 127) as u32)
    };

    regs[REG_QUANT_0] = Word::default()
        .field(quants[1], 11)
        .field(quants[0], 11)
        .field(job.y2dc_delta_q as u32, 5) // quant_delta_1
        .field(job.y1dc_delta_q as u32, 5) // quant_delta_0
        .finish();

    regs[REG_REF0] = job.refs[0];

    regs[REG_FILTER_MODE] = Word::default()
        .field(job.mode_lf_deltas[3] as u32, 7)
        .field(job.mode_lf_deltas[2] as u32, 7)
        .field(job.mode_lf_deltas[1] as u32, 7)
        .field(job.mode_lf_deltas[0] as u32, 7)
        .field(job.sharpness, 3)
        .field(job.loop_filter_type, 1)
        .finish();

    if job.mode_ref_lf_delta_enabled {
        regs[REG_REF_DELTAS] = Word::default()
            .field(job.ref_lf_deltas[3] as u32, 7)
            .field(job.ref_lf_deltas[2] as u32, 7)
            .field(job.ref_lf_deltas[1] as u32, 7)
            .field(job.ref_lf_deltas[0] as u32, 7)
            .field(0, 4)
            .finish();
    }

    regs[REG_GOLDEN] = job.refs[1];
    regs[REG_ALT_REF] = job.refs[2];

    // One dma-buf handle per DCT partition, in the order the hardware reads
    // them: partition 0 in sw64, 1 to 5 in sw140..144, 6 and 7 in sw146..147.
    for partition in 1..(1usize << job.log2_dct_partitions) {
        if partition <= 5 {
            regs[REG_DCT_STREAM_0 + partition - 1] = job.stream_fd;
        } else if partition <= 7 {
            regs[REG_DCT_STREAM_1 + partition - 6] = job.stream_fd;
        }
    }
    regs[REG_STREAM] = job.stream_fd;
    regs[REG_SEG_MAP] = job.seg_map_fd;

    // sw150 carries the start words of partitions 3 to 7, declared from the most
    // significant one down. A partition the picture does not carry keeps zero.
    regs[REG_DCT_START_BITS] = Word::default()
        .field(job.partition_start_bit_if(7), 6)
        .field(job.partition_start_bit_if(6), 6)
        .field(job.partition_start_bit_if(5), 6)
        .field(job.partition_start_bit_if(4), 6)
        .field(job.partition_start_bit_if(3), 6)
        .field(0, 2)
        .finish();

    regs[REG_QUANT_2] = Word::default()
        .field(quants[3], 11)
        .field(quants[2], 11)
        .field(job.uvdc_delta_q as u32, 5) // quant_delta_3
        .field(job.y2ac_delta_q as u32, 5) // quant_delta_2
        .finish();

    regs[REG_QUANT_4] = Word::default()
        .field(0, 11) // quant_5
        .field(0, 11) // quant_4
        .field(0, 5)
        .field(job.uvac_delta_q as u32, 5) // quant_delta_4
        .finish();

    regs
}

/// The address offsets one picture arms. The C HAL reaches
/// `mpp_dev_set_reg_offset` for seven different registers, and only when the
/// offset is not zero, which is also what the driver expects: an offset of zero
/// is simply not sent.
pub fn build_offsets(job: &PictureJob) -> Vec<RegOffsetInfo> {
    let mut offsets = Vec::new();
    write_offsets(job, &mut offsets);
    offsets
}

/// Same entries as [`build_offsets`], appended into caller-owned scratch so
/// the job path does not allocate; the caller clears first, so the list is
/// rebuilt in full every job.
pub fn write_offsets(job: &PictureJob, offsets: &mut Vec<RegOffsetInfo>) {
    let mut arm = |reg: usize, offset: u32| {
        if offset != 0 {
            offsets.push(RegOffsetInfo {
                reg_idx: reg as u32,
                offset,
            });
        }
    };

    arm(REG_STREAM, job.stream_start_offset);
    arm(REG_STREAM_0, job.partition_base(0));
    for partition in 1..(1usize << job.log2_dct_partitions) {
        if partition <= 5 {
            arm(
                REG_DCT_STREAM_0 + partition - 1,
                job.partition_base(partition),
            );
        } else if partition <= 7 {
            arm(
                REG_DCT_STREAM_1 + partition - 6,
                job.partition_base(partition),
            );
        }
    }

    // A key frame has no reference, and the C HAL points `sw131` at the current
    // picture one macroblock-count past its start rather than at the same place
    // the decoder is writing.
    if job.frame_type == 0 {
        arm(REG_REF0, job.mb_width * job.mb_height);
    }
    arm(REG_GOLDEN, job.sign_bias_golden);
    arm(REG_ALT_REF, job.sign_bias_altref);
    arm(
        REG_SEG_MAP,
        u32::from(job.segmentation_enabled) + (u32::from(job.segmentation_map_update) << 1),
    );
}

/// Append a register image to `path` in the text `hal_vp8d_debug=2` prints, so
/// a Rust and a C run diff without reformatting either one. Unlike the shared
/// MPEG dump helper this covers words 0 to 49 as well, because that is what the
/// VP8 HAL writes.
pub fn dump_registers(path: &str, label: &str, registers: &[u32; REG_NUM]) {
    use std::fmt::Write as _;
    use std::io::Write;

    let mut text = String::with_capacity(REG_NUM * 20);
    let _ = writeln!(text, "# {label}");
    for (index, word) in registers.iter().enumerate() {
        let _ = writeln!(text, "reg[{}] {:#08x}", index, word);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(text.as_bytes());
    }
}

/// Persistent VP8 resources plus the one-job submission path.
pub struct Vp8d {
    svc: MppService,
    prob_table: DmaBuf,
    seg_map: DmaBuf,
    /// `sw55` of the last completed job.
    pub last_irq: u32,
    jobs: usize,
    /// Reused across jobs; the list is rebuilt in full every job.
    offsets_scratch: Vec<RegOffsetInfo>,
}

impl Vp8d {
    pub fn open() -> io::Result<Self> {
        let mut prob_table = DmaBuf::alloc("system", PROB_TABLE_SIZE, false)?;
        // The C buffer is whatever the allocator handed back beyond the written
        // prefix; zeroing it makes the run reproducible instead.
        prob_table.sync_write_begin()?;
        prob_table.as_mut_slice().fill(0);
        prob_table.sync_write_end()?;
        Ok(Self {
            svc: MppService::open(Client::Vdpu2 as u32)?,
            prob_table,
            seg_map: DmaBuf::alloc("system", SEG_MAP_SIZE, false)?,
            last_irq: 0,
            jobs: 0,
            offsets_scratch: Vec::with_capacity(11),
        })
    }

    pub fn prob_fd(&self) -> u32 {
        self.prob_table.fd() as u32
    }

    pub fn seg_map_fd(&self) -> u32 {
        self.seg_map.fd() as u32
    }

    /// Program the probability image, clear the segment map for a key frame,
    /// submit the picture and wait for the hardware.
    pub fn decode(&mut self, job: &PictureJob) -> io::Result<u32> {
        let table = build_prob_table(job);
        self.prob_table.sync_write_begin()?;
        {
            // The dma-heap mapping is page rounded, so address the image by its
            // own length rather than by the buffer's.
            let window = self.prob_table.as_mut_slice();
            window[..table.len()].copy_from_slice(&table);
        }
        self.prob_table.sync_write_end()?;

        if job.frame_type == 0 {
            self.seg_map.sync_write_begin()?;
            let window = self.seg_map.as_mut_slice();
            window[..SEG_MAP_SIZE].fill(0);
            self.seg_map.sync_write_end()?;
        }

        let registers = build_registers(job);
        self.offsets_scratch.clear();
        write_offsets(job, &mut self.offsets_scratch);
        self.last_irq = vdpu2::run_job(&mut self.svc, &registers, &self.offsets_scratch)?;
        self.jobs += 1;
        if std::env::var_os("RFI_VP8_REG_DUMP").is_some() {
            dump_registers(
                "/tmp/our_vp8_regs.txt",
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
    use crate::codec::vp8::parser::{parse_ivf, Parser};

    /// The dma-buf handle words, which two processes can never agree on.
    const HANDLE_WORDS: [usize; 15] = [
        61, 63, 64, 131, 136, 137, 140, 141, 142, 143, 144, 145, 146, 147, 149,
    ];

    /// The C HAL's own register image for the key frame of
    /// `tests/data/vp8_320x240_12_ip.ivf`, dumped on t6 with `hal_vp8d_debug=4`.
    fn c_registers() -> [u32; REG_NUM] {
        let text = std::fs::read_to_string("tests/data/vp8/c_keyframe_registers.txt")
            .expect("C register dump");
        let mut words = [0u32; REG_NUM];
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let Some(name) = fields.next() else { continue };
            let Some(value) = fields.next() else { continue };
            let index: usize = name
                .trim_start_matches("reg[")
                .trim_end_matches(']')
                .parse()
                .expect("register index");
            words[index] =
                u32::from_str_radix(value.trim_start_matches("0x"), 16).expect("register value");
        }
        words
    }

    /// The picture `index` of `path`, with the parser state as it is by then, and
    /// the frame buffers the C run happened to hold replaced by stand-ins.
    fn job_for(path: &str, index: usize) -> PictureJob {
        let data = std::fs::read(path).expect("test fixture");
        let frames = parse_ivf(&data).expect("parse container");
        let mut parser = Parser::new();
        let mut picture: Option<crate::codec::vp8::parser::Picture> = None;
        for frame in frames.iter().take(index + 1) {
            picture = Some(
                parser
                    .parse_picture(frame.data)
                    .expect("parse picture")
                    .expect("key frame present"),
            );
        }
        let picture = picture.expect("at least one picture");
        // The reference buffers of a key frame all point at the picture being
        // written, which is what the C dump shows for job zero.
        let current_fd = 18;
        PictureJob {
            mb_width: picture.width.div_ceil(16),
            mb_height: picture.height.div_ceil(16),
            frame_type: u32::from(!picture.key_frame),
            version: picture.version,
            loop_filter_type: u32::from(picture.loop_filter_type),
            loop_filter_level: u32::from(picture.loop_filter_level),
            sharpness: u32::from(picture.sharpness),
            mode_ref_lf_delta_enabled: picture.mode_ref_lf_delta_enabled,
            ref_lf_deltas: picture.ref_lf_deltas,
            mode_lf_deltas: picture.mode_lf_deltas,
            segmentation_enabled: picture.segmentation_enabled,
            segmentation_map_update: picture.segmentation_map_update,
            segment_feature_mode: picture.segment_feature_mode,
            segment_qp: picture.segment_qp,
            segment_loopfilter: picture.segment_loopfilter,
            y1ac_delta_q: picture.y1ac_delta_q,
            y1dc_delta_q: picture.y1dc_delta_q,
            y2dc_delta_q: picture.y2dc_delta_q,
            y2ac_delta_q: picture.y2ac_delta_q,
            uvdc_delta_q: picture.uvdc_delta_q,
            uvac_delta_q: picture.uvac_delta_q,
            mb_no_coeff_skip: picture.mb_no_coeff_skip,
            bool_value: u32::from(picture.bool_value),
            bool_range: u32::from(picture.bool_range),
            stream_start_offset: picture.stream_start_offset,
            stream_start_bit: picture.stream_start_bit,
            stream_end_pos: picture.stream_end_pos,
            frame_tag_size: picture.frame_tag_size,
            offset_to_dct_parts: picture.offset_to_dct_parts,
            log2_dct_partitions: picture.log2_dct_partitions,
            dct_partition_offsets: picture.dct_partition_offsets,
            entropy: picture.entropy,
            prob_skip_false: picture.prob_skip_false,
            prob_intra: picture.prob_intra,
            prob_last: picture.prob_last,
            prob_golden: picture.prob_golden,
            segment_tree_probs: picture.segment_tree_probs,
            sign_bias_golden: u32::from(picture.ref_frame_sign_bias_golden),
            sign_bias_altref: u32::from(picture.ref_frame_sign_bias_altref),
            stream_fd: 17,
            cur_fd: current_fd,
            refs: [current_fd, current_fd, current_fd],
            probe_fd: 16,
            seg_map_fd: 15,
        }
    }

    /// Every word of the key-frame register file, against the C HAL's own dump.
    /// The handle words are checked separately against the stand-ins, because a
    /// dma-buf fd means nothing outside the process that allocated it.
    #[test]
    fn the_keyframe_register_file_matches_the_c_decoder() {
        let c = c_registers();
        let ours = build_registers(&job_for("tests/data/vp8_320x240_12_ip.ivf", 0));
        let mismatched: Vec<String> = (0..REG_NUM)
            .filter(|index| !HANDLE_WORDS.contains(index))
            .filter(|index| c[*index] != ours[*index])
            .map(|index| format!("sw{index}: c={:#010x} rust={:#010x}", c[index], ours[index]))
            .collect();
        assert!(mismatched.is_empty(), "differing words: {mismatched:?}");

        let job = job_for("tests/data/vp8_320x240_12_ip.ivf", 0);
        assert_eq!(ours[REG_PROB_TABLE], job.probe_fd);
        assert_eq!(ours[REG_CUR_PIC], job.cur_fd);
        assert_eq!(ours[REG_STREAM_0], job.stream_fd);
        assert_eq!(ours[REG_STREAM], job.stream_fd);
        assert_eq!(ours[REG_SEG_MAP], job.seg_map_fd);
        assert_eq!(
            (ours[REG_REF0], ours[REG_GOLDEN], ours[REG_ALT_REF]),
            (job.cur_fd, job.cur_fd, job.cur_fd),
            "a key frame points every reference at the picture it writes"
        );
    }

    /// The words the dump pins that are worth naming: decode mode ten, the six
    /// endian bits, burst 16, the macroblock count of 20 by 15, and the stream
    /// lengths the first partition of the key frame occupies.
    #[test]
    fn the_fixed_words_are_the_ones_the_c_hal_writes() {
        let c = c_registers();
        let ours = build_registers(&job_for("tests/data/vp8_320x240_12_ip.ivf", 0));
        assert_eq!(ours[REG_DEC_MODE], DEC_MODE_VP8);
        assert_eq!(ours[REG_ENDIAN], c[REG_ENDIAN]);
        assert_eq!(ours[REG_BUS], c[REG_BUS]);
        assert_eq!(ours[REG_PIC_SIZE], c[REG_PIC_SIZE]);
        assert_eq!(ours[REG_PIC_CTRL], c[REG_PIC_CTRL]);
        assert_eq!(
            ours[REG_FILTER], c[REG_FILTER],
            "level 0 turns filtering off"
        );
        assert_eq!(ours[REG_STREAM_LEN], c[REG_STREAM_LEN]);
        assert_eq!(ours[124], c[124]);
        assert_eq!(ours[REG_PIC_EXT], c[REG_PIC_EXT], "boolean state 37:250");
        assert_eq!(ours[REG_START_MB], 0, "the prefetch threshold stays home");
        assert_eq!(ours[62], 0, "no direct motion-vector buffer");
        assert_eq!(ours[58], 0, "no soft reset word");
        assert_eq!(ours[..50], [0u32; 50], "the post-processor window is idle");
    }

    /// Every interpolation tap word, from the C dump. The bank and field indices
    /// are offset by one and the words disagree about the tap order, so this is
    /// the table a mistake in `write_filter_taps` has to be caught by.
    #[test]
    fn the_filter_taps_are_distributed_like_the_c_hal() {
        let c = c_registers();
        let ours = build_registers(&job_for("tests/data/vp8_320x240_12_ip.ivf", 0));
        for word in [59, 125, 126, 127, 128, REG_TAP_0_3, 154, 155, 156, 157, 158] {
            assert_eq!(ours[word], c[word], "sw{word}");
        }
        assert_eq!(c[59], 0x0008_0000);
        assert_eq!(c[128], 0x1eff_a9f6);
    }

    /// A version with any low bits set switches on the bilinear filter and drops
    /// the extra motion-vector range bit; only version zero gets the tap words.
    #[test]
    fn versions_select_the_taps_and_the_motion_vector_range() {
        let base = job_for("tests/data/vp8_320x240_12_ip.ivf", 0);
        // `sw121` declares the two controls after the dc match fields, so they sit
        // at bits 12 and 13.
        const BILINEAR: u32 = 1 << 12;
        const EXTRA_MV_RANGE: u32 = 1 << 13;
        assert_eq!(build_registers(&base)[121] & BILINEAR, 0, "bilinear off");
        assert_ne!(
            build_registers(&base)[121] & EXTRA_MV_RANGE,
            0,
            "the extra range bit is on for version zero"
        );

        let mut version_three = base.clone();
        version_three.version = 3;
        let regs = build_registers(&version_three);
        assert_eq!(
            regs[121] & EXTRA_MV_RANGE,
            0,
            "version three has no extra bit"
        );
        assert_eq!(
            (regs[59], regs[154], regs[128]),
            (0, 0, 0),
            "and no taps at all"
        );

        // Only version zero gets a tap bank at all: the condition is on the low
        // two bits, and version one turns the bilinear filter on instead.
        let mut version_one = base.clone();
        version_one.version = 1;
        let regs = build_registers(&version_one);
        assert_ne!(regs[121] & BILINEAR, 0, "bilinear for version one");
        assert_eq!(
            (regs[59], regs[154], regs[128]),
            (0, 0, 0),
            "no taps for a profile this block cannot interpolate with"
        );

        // The full derivation table for every frame-tag version, transcribed
        // from the three conditions in `hal_vp8d_vdpu2_gen_regs`: the bilinear
        // filter is on for any set low bit, the tap bank is written only when
        // the low two bits are clear, and the extra motion-vector range bit is
        // off for version three alone. Versions four to seven are reserved by
        // the RFC and C treats them by the same bit tests; version four has to
        // look exactly like version zero and version five like version one.
        for (version, bilinear, taps, extra_mv_range) in [
            (0u8, false, true, true),
            (1, true, false, true),
            (2, true, false, true),
            (3, true, false, false),
            (4, false, true, true),
            (5, true, false, true),
            (6, true, false, true),
            (7, true, false, true),
        ] {
            let mut job = base.clone();
            job.version = version;
            let regs = build_registers(&job);
            assert_eq!(
                regs[121] & BILINEAR != 0,
                bilinear,
                "version {version} bilinear"
            );
            assert_eq!(
                regs[121] & EXTRA_MV_RANGE != 0,
                extra_mv_range,
                "version {version} extra motion-vector range"
            );
            assert_eq!(
                regs[59] != 0 || regs[128] != 0 || regs[154] != 0,
                taps,
                "version {version} tap bank"
            );
        }
    }

    /// One dma-buf handle exists per DCT partition beyond the first, and a
    /// single-partition picture must leave all eight extra bases at zero - the
    /// off-by-one that wrote `sw140` for every picture is what this pins.
    #[test]
    fn one_stream_base_exists_per_extra_partition() {
        let single = build_registers(&job_for("tests/data/vp8_320x240_12_ip.ivf", 0));
        assert_eq!(single[REG_STREAM_0], 17);
        assert_eq!(single[REG_STREAM], 17);
        assert_eq!(
            single[REG_DCT_STREAM_0..REG_DCT_STREAM_1 + 2],
            [0, 0, 0, 0, 0, single[REG_STREAM], 0, 0],
            "no partition, no extra bases - only the stream base at sw145 is set"
        );
        assert_eq!(single[REG_DCT_START_BITS], 0, "and no extra start words");

        let mut job = job_for("tests/data/vp8_320x240_12_ip.ivf", 0);
        job.log2_dct_partitions = 2;
        for (index, offset) in job.dct_partition_offsets.iter_mut().enumerate() {
            *offset = 900 + 100 * index as u32;
        }
        let regs = build_registers(&job);
        assert_eq!(
            regs[REG_DCT_STREAM_0..REG_DCT_STREAM_0 + 3],
            [17, 17, 17],
            "partitions one to three"
        );
        assert_eq!(regs[REG_DCT_STREAM_0 + 3], 0, "there is no fourth base");
        assert_eq!(
            regs[REG_DCT_STREAM_1..REG_DCT_STREAM_1 + 2],
            [0u32; 2],
            "and none in the second pair either"
        );
        // 8055 bytes of picture, minus the 900-byte base of partition zero, plus
        // three bytes per extra partition and the three bits of head room the
        // aligned base gives up.
        assert_eq!(job.stream_len() as usize, 8055 + 3 - 900 + 3 * 3 + 3);
        assert_eq!(job.first_partition_len() as usize, 894 + 3 - (440 - 7) + 1);

        let offsets = build_offsets(&job);
        assert_eq!(
            offsets
                .iter()
                .map(|offset| offset.reg_idx)
                .collect::<Vec<u32>>(),
            vec![145, 64, 140, 141, 142, 131],
            "the stream, one base per extra partition, and the key-frame self reference"
        );
    }

    /// The reused-scratch offset writer must emit exactly the list the
    /// original allocating `build_offsets` produced, including the cases
    /// where zero offsets must stay absent (a P picture arms no self
    /// reference, and an unsegmented picture arms no map offset).
    #[test]
    fn the_offset_scratch_matches_the_allocating_builder() {
        for (path, index) in [
            ("tests/data/vp8_320x240_12_ip.ivf", 0usize),
            ("tests/data/vp8_320x240_12_ip.ivf", 2),
            ("tests/data/vp8_320x240_12_parts.ivf", 0),
            ("tests/data/vp8_320x240_12_parts.ivf", 2),
        ] {
            let job = job_for(path, index);
            let expected = build_offsets(&job);
            let mut got = vec![RegOffsetInfo {
                reg_idx: 0xffff_ffff,
                offset: u32::MAX,
            }]; // dirty scratch must be fully rebuilt
            got.clear();
            write_offsets(&job, &mut got);
            assert_eq!(got, expected, "{path} picture {index}");
        }
    }

    /// The probability image, byte for byte, against the buffer the C decoder left
    /// in memory. The defaults of a key frame and the updated set of a later P
    /// picture together cover every block of the layout.
    #[test]
    fn the_probability_image_matches_the_c_decoder() {
        for (picture, golden) in [
            (0usize, "tests/data/vp8/c_keyframe_prob_table.bin"),
            (2usize, "tests/data/vp8/c_p_frame_prob_table.bin"),
        ] {
            let expected = std::fs::read(golden).expect("C probability dump");
            let job = job_for("tests/data/vp8_320x240_12_ip.ivf", picture);
            let ours = build_prob_table(&job);
            assert_eq!(ours.len(), expected.len(), "{golden} length");
            let mismatched: Vec<usize> = (0..expected.len())
                .filter(|index| ours[*index] != expected[*index])
                .collect();
            assert!(
                mismatched.is_empty(),
                "{golden}: first difference at byte {} ({:#02x} vs {:#02x}), {} bytes differ",
                mismatched[0],
                expected[mismatched[0]],
                ours[mismatched[0]],
                mismatched.len()
            );
        }
    }

    /// Both loop filter forms the segment feature can take, and the quantiser
    /// deltas that sit next to them.
    #[test]
    fn segment_features_reach_the_filter_and_quantiser_words() {
        let mut job = job_for("tests/data/vp8_320x240_12_ip.ivf", 0);
        job.loop_filter_level = 40;
        job.segmentation_enabled = true;

        job.segment_feature_mode = false;
        job.segment_loopfilter = [4, -8, 30, 63];
        let regs = build_registers(&job);
        // `sw129` declares segment three first, so segment zero is the top field.
        let level = |segment: usize| (regs[REG_FILTER_LEVELS] >> (18 - 6 * segment)) & 0b11_1111;
        let levels = [level(0), level(1), level(2), level(3)];
        assert_eq!(
            levels,
            [44, 32, 63, 63],
            "delta levels are added to the picture level and clamped"
        );

        job.segment_feature_mode = true;
        let regs = build_registers(&job);
        let level = |segment: usize| (regs[REG_FILTER_LEVELS] >> (18 - 6 * segment)) & 0b11_1111;
        let levels = [level(0), level(1), level(2), level(3)];
        // The absolute values reach the six-bit field unclamped, so a negative
        // segment feature wraps the way the C bitfield assignment does.
        assert_eq!(levels, [4, 56, 30, 63], "absolute levels pass through");
    }
}
