//! The bit-allocation half of `rc_model_v2.c` - `bits_model_*`, `calc_next_i_ratio`
//! and the frame-type scaling they apply.
//!
//! This is the part of MPP's rate control that decides *how many bits a picture is
//! allowed to spend*, and it is separable from the part that decides the quantiser:
//! `calc_cbr_ratio`, `calc_vbr_ratio` and `calc_avbr_ratio` only move `next_ratio`,
//! which the QP path consumes, while `bit_target` comes out of `bits_model_alloc`
//! whatever the mode. So the window arithmetic here is complete, and the controller
//! that turns it into a QP is still ahead of us.
//!
//! The numbers are transcribed in the C's order, including the integer divisions,
//! because this loop is compared against the reference's output rather than against
//! an idea of fairness: `i_scale` is `80 * i_sumbits / (2 * p_sumbits)` and loses
//! its fraction before the allocation divides by it.

use super::data::DataV2;

/// `tab_lnx`: the log-shaped lookup the CBR controller reads its instantaneous-rate
/// term out of, indexed by `(ins_bps << 5) / target_bps` clipped to 0..63.
/// `mean_qp2scale`: how much an intra picture's bit count is worth against the
/// current mean QP, indexed by `pre_mean_qp + 8 - current_mean_qp`.
const MEAN_QP2SCALE: [i32; 16] = [
    14, 15, 16, 18, 20, 22, 25, 28, 32, 36, 40, 44, 50, 56, 64, 72,
];

/// `bit2percent`: the still-to-moving mapping AVBR uses on the ratio of intra to
/// inter bits.
const BIT2PERCENT: [i32; 100] = [
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 95, 90, 85, 80, 75, 72, 69, 66, 64, 62, 59, 58, 57,
    56, 55, 54, 53, 52, 51, 50, 49, 48, 47, 47, 46, 46, 45, 45, 44, 44, 43, 43, 43, 42, 42, 42, 41,
    41, 41, 40, 40, 40, 39, 39, 39, 38, 38, 38, 37, 37, 37, 37, 37, 37, 37, 36, 36, 36, 36, 36, 36,
    36, 35, 35, 35, 34, 34, 34, 34, 34, 33, 33, 33, 33, 33, 33, 33, 33, 33, 32, 32, 32, 32, 32, 32,
    32, 32, 32, 32,
];

const TAB_LNX: [i32; 64] = [
    -1216, -972, -830, -729, -651, -587, -533, -486, -445, -408, -374, -344, -316, -290, -265,
    -243, -221, -201, -182, -164, -147, -131, -115, -100, -86, -72, -59, -46, -34, -22, -11, 0, 10,
    21, 31, 41, 50, 60, 69, 78, 86, 95, 87, 103, 111, 119, 127, 134, 142, 149, 156, 163, 170, 177,
    183, 190, 196, 202, 208, 214, 220, 226, 232, 237,
];

/// `I_WINDOW_LEN`, `P_WINDOW1_LEN`, `P_WINDOW2_LEN`.
const I_WINDOW: i32 = 2;
const P_WINDOW1: i32 = 5;
const P_WINDOW2: i32 = 8;

/// `max_i_delta_qp`: how much extra an intra picture may be promised, indexed by
/// the QP the previous intra picture coded at. Generous at low QP, and zero from
/// 43 upwards, where quality is already being sacrificed.
const MAX_I_DELTA_QP: [i32; 51] = [
    640, 640, 640, 640, 640, 640, 640, 640, 640, 640, 640, 640, 640, 640, 576, 576, 512, 512, 448,
    448, 384, 384, 320, 320, 320, 256, 256, 256, 192, 192, 192, 192, 192, 128, 128, 128, 128, 128,
    128, 64, 64, 64, 64, 64, 64, 0, 0, 0, 0, 0, 0,
];

/// The picture kind the allocator is told about. `InterVi` is the smart-GOP
/// virtual-intra picture, which still codes as a P slice but points at the last
/// intra picture. The intra-refresh frame type is not driven by this model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Intra,
    InterP,
    InterVi,
}

/// `RC_CBR` and `RC_VBR` of `rc_model_v2.c`. `RC_AVBR`, which weighs motion
/// statistics through `moving_ratio_calc`, is not ported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Cbr,
    Vbr,
    Avbr,
}

/// The configuration the model is initialised with, in MPP's own units: quality on
/// the 0..100 scale, bit rate in bits per second, `igop` in frames.
#[derive(Debug, Clone)]
pub struct Config {
    /// `usr_cfg.igop`, the intra GOP.
    pub igop: i32,
    pub fps_num: i32,
    pub fps_den: i32,
    pub bps_target: i32,
    pub bps_min: i32,
    pub bps_max: i32,
    /// `usr_cfg.init_ip_ratio`, clipped to 160..640. `h264e_api_v2.c` defaults it to 160.
    pub init_ip_ratio: i32,
    /// `usr_cfg.min_i_bit_prop` and `max_i_bit_prop`, percent, clipped to 10..100
    /// and to at least the minimum. Defaults 10 and 30.
    pub min_i_bit_prop: i32,
    pub max_i_bit_prop: i32,
    /// `usr_cfg.stats_time`, `set_rc_cfg`'s `rc->stats_time ? : 3` clipped to 1..60.
    pub stats_time: i32,
    /// Which controller runs.
    pub mode: Mode,
    /// `RcCfg::vgop`, nonzero only when a reference schedule makes the C choose
    /// `SMART_P`. MPP derives it from the short-term reference period.
    pub smart_p_vgop: i32,
    /// `rc:qp_vi`, the QP subtracted from a virtual-intra picture. MPP defaults
    /// it to two when it turns a caller-provided reference schedule into SMART_P.
    pub vi_quality_delta: i32,
    /// `super_cfg.super_mode`, `super_i_thd`, `super_p_thd` and `rc_priority`.
    /// Mode 1 drops an oversized picture; mode 2 asks for same-frame re-encode.
    pub super_mode: u8,
    pub super_i_thd: u32,
    pub super_p_thd: u32,
    pub rc_priority: u8,
    pub drop_mode: u8,
    pub drop_threshold: u32,
    pub drop_gap: u32,
    /// `rc:hier_qp_en` and its four layer controls. The reference walks P
    /// pictures through layers 4, 3, 2 and 1 according to
    /// `hier_frame_num`, then resets the cycle. `hier_qp_delta[layer - 1]` is
    /// subtracted after the controller picks this picture's QP.
    pub hier_qp_enabled: bool,
    pub hier_qp_delta: [i32; 4],
    pub hier_frame_num: [i32; 4],
    /// The quantiser bounds, on MPP's 0..100 quality scale. `h264e_api_v2.c`'s
    /// defaults: medium (26) as the initial target and 8..48 as the bounds, with an
    /// I-picture delta of two - the `:2` the `uuid_rc_cfg` SEI ends with.
    ///
    /// `rc_model_v2_start` treats `init_quality <= 0` as "the caller never set an
    /// initial quality" (`(usr_cfg->init_quality <= 0) ? -1 : usr_cfg->init_quality`),
    /// and `rc_model_v2_hal_start` then derives the first picture's QP from the
    /// bit budget - [`cal_first_i_start_qp`], or 26 when there is no budget to
    /// derive one from. Any value at or below zero arrives at the same place.
    pub quality_init: i32,
    pub quality_min: i32,
    pub quality_max: i32,
    pub i_quality_delta: i32,
    /// `mb_w * mb_h` of the coded picture - `MPP_ALIGN(width, 16) / 16 *
    /// (MPP_ALIGN(height, 16) / 16)`, in 16x16 blocks. Only read when
    /// `quality_init <= 0`, as the denominator of the first-QP derivation.
    pub total_mb: i32,
}

impl Config {
    /// What `h264e_api_v2.c` plus `set_rc_cfg` leave a caller with who sets only a
    /// bit rate and a GOP - which is what `mpi_enc_test -rc 1 -bps ...` produces.
    pub fn cbr(bps_target: i32, bps_min: i32, bps_max: i32, igop: i32) -> Config {
        Config {
            igop,
            fps_num: 30,
            fps_den: 1,
            bps_target,
            bps_min,
            bps_max,
            init_ip_ratio: 160,
            min_i_bit_prop: 10,
            max_i_bit_prop: 30,
            stats_time: 3,
            quality_init: 26,
            quality_min: 8,
            quality_max: 48,
            i_quality_delta: 2,
            // 320x240, the size of every capture this model is verified against.
            total_mb: 20 * 15,
            mode: Mode::Cbr,
            smart_p_vgop: 0,
            vi_quality_delta: 0,
            super_mode: 0,
            super_i_thd: 0,
            super_p_thd: 0,
            rc_priority: 0,
            drop_mode: 0,
            drop_threshold: 20,
            drop_gap: 1,
            hier_qp_enabled: false,
            hier_qp_delta: [0; 4],
            hier_frame_num: [0; 4],
        }
    }
}

/// `cal_first_i_start_qp`: the first intra picture's QP when the caller never set
/// an initial quality.
///
/// The size buckets round the picture down to the table row they fall in, and the
/// bits-per-macroblock the budget implies is read through `qscale2qp`; a budget
/// too thin to code the picture even at QP 45 lands on 45, and one too generous -
/// or a picture with no macroblocks at all, where the C's unsigned subtraction
/// below zero wraps into the same clipped answer - lands on 15.
///
/// The arithmetic is reproduced as unsigned because that is what the C is:
/// `total_mb` and `tab_bit` are `RK_U32`, so `total_mb * tab_bit[cnt] - 350`
/// wraps modulo 2^32 before the division, and whether the quotient returning
/// through the signed `index` reads negative depends on the divisor - see
/// `first_qp_clips_at_both_ends_and_wraps_unsigned_below_zero`, where an empty
/// picture returns 15 at one bit and 45 at a thousand.
pub fn cal_first_i_start_qp(target_bit: i32, total_mb: u32) -> i32 {
    // `mb_num`: the lower edge of each bucket. `cnt` ends at the last row whose
    // edge the picture has reached.
    const MB_NUM: [u32; 9] = [0, 200, 700, 1200, 2000, 4000, 8000, 16000, 20000];
    // `tab_bit`: the bits-per-macroblock scale the bucket divides by. Note the row
    // for 4000-block pictures carries 3780, the same figure as the smallest row:
    // the C table is not monotone, and sorting it would be a bug.
    const TAB_BIT: [u32; 9] = [3780, 3570, 3150, 2940, 2730, 3780, 2100, 1680, 2100];
    // `qscale2qp`: the QP a qscale maps to; the index is clipped into it.
    const QSCALE2QP: [u32; 96] = [
        15, 15, 15, 15, 15, 16, 18, 20, 21, 22, 23, 24, 25, 25, 26, 27, 28, 28, 29, 29, 30, 30, 30,
        31, 31, 32, 32, 33, 33, 33, 34, 34, 34, 34, 35, 35, 35, 36, 36, 36, 36, 36, 37, 37, 37, 37,
        38, 38, 38, 38, 38, 39, 39, 39, 39, 39, 39, 40, 40, 40, 40, 41, 41, 41, 41, 41, 41, 41, 42,
        42, 42, 42, 42, 42, 42, 42, 43, 43, 43, 43, 43, 43, 43, 43, 44, 44, 44, 44, 44, 44, 44, 44,
        45, 45, 45, 45,
    ];

    let mut cnt = 0usize;
    for edge in MB_NUM.iter().take(8) {
        if *edge > total_mb {
            break;
        }
        cnt += 1;
    }

    // The division is against the signed budget converted to unsigned by the
    // usual arithmetic conversions - the C's `target_bit` is positive here
    // because `rc_model_v2_hal_start` only calls this when `bit_target` is.
    let qscale = total_mb.wrapping_mul(TAB_BIT[cnt]).wrapping_sub(350);
    let index = qscale.wrapping_div(target_bit as u32) as i32;
    QSCALE2QP[clip(index, 4, 95) as usize] as i32
}

fn clip(value: i32, low: i32, high: i32) -> i32 {
    value.max(low).min(high)
}

/// The first intra picture's target quality, exactly as
/// `rc_model_v2_hal_start` decides it: a caller-set quality stands; anything the
/// API folded onto "unset" (`rc_model_v2_start`'s `<= 0` becoming -1) is derived
/// from the picture's bit budget, and the one case with no budget at all is the
/// log line's fallback QP 26.
fn first_quality(cfg: &Config, bit_target: i32) -> i32 {
    if cfg.quality_init <= 0 {
        if bit_target != 0 {
            cal_first_i_start_qp(bit_target, cfg.total_mb.max(0) as u32)
        } else {
            // "init qp not set on fix qp mode, use default qp"
            26
        }
    } else {
        cfg.quality_init
    }
}

/// The allocation state: the sliding windows plus the scales derived from them.
pub struct Model {
    cfg: Config,
    /// `gop_total_bits`: the GOP's whole budget.
    gop_total_bits: i64,
    bit_per_frame: i32,
    /// `i_scale` and `p_scale`, in sixteenths of a P picture.
    i_scale: i32,
    p_scale: i32,
    i_sumbits: i32,
    p_sumbits: i32,
    vi_sumbits: i32,
    vi_scale: i32,
    /// `i_bit`, `p_bit`, and the water-level statistics.
    i_bit: DataV2,
    p_bit: DataV2,
    vi_bit: DataV2,
    stat_bits: DataV2,
    stat_rate: DataV2,
    /// `ins_bps`, `stat_watl` and its two bounds.
    ins_bps: i32,
    stat_watl: i32,
    watl_thrd: i32,
    watl_base: i32,
    /// `target_bps` and the previous picture's decision and result.
    target_bps: i32,
    pre_target_bits: i32,
    pre_real_bits: i32,
    last_frame_type: FrameType,
    /// `first_frm_flg`, cleared by `rc_model_v2_end`.
    first: bool,
    /// `next_i_ratio`, the accumulated head start owed to the next intra picture.
    next_i_ratio: i32,
    /// `cur_scale_qp`: the QP in 64ths, the quantiser controller's real state - the
    /// picture's QP is only this shifted back down.
    cur_scale_qp: i32,
    /// `next_ratio`, the sum of the CBR controller's four terms.
    next_ratio: i32,
    /// `last_inst_bps`: `ins_bps` as it stood before the last picture's update.
    last_inst_bps: i32,
    /// AVBR's motion history: the last intra pictures' sizes and mean QPs, the
    /// last inter sizes, and the measured block differences.
    pre_i_bit: DataV2,
    pre_i_mean_qp: DataV2,
    pre_p_bit: DataV2,
    madi: DataV2,
    madp: DataV2,
    /// AVBR's per-GOP accounting.
    gop_bits: DataV2,
    moving_ratio: i32,
    real_gbits: i64,
    avg_gbits: i64,
    min_still_percent: i32,
    max_still_qp: i32,
    motion_sensitivity: i32,
    pre_mean_qp: i32,
    scale_qp: i32,
    /// `hier_frm_cnt` and `qp_layer_id` from `rc_hier_calc_dealt_qp`.
    hier_frame_cnt: [i32; 4],
    qp_layer_id: i32,
    /// `rc_model_v2_check_reenc` state. The current phase intentionally wires
    /// the CBR re-encode branch; the same-frame hardware loop is independent of
    /// the controller and later modes can reuse it.
    reenc_cnt: u32,
    drop_cnt: u32,
}

/// Everything the hardware measured about one picture, as the reference's
/// `EncRcTaskInfo` carries it: coded size, the mean QP reached, and the
/// block-difference statistics AVBR weighs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Measured {
    pub bits: i32,
    pub quality: i32,
    pub madi: i32,
    pub madp: i32,
}

impl Model {
    /// `bits_model_init` + `bits_model_param_init` + `bits_frm_init`.
    pub fn new(cfg: Config) -> Model {
        // `bits_model_init`'s normalisation. The all-intra and unbounded GOP cases
        // rewrite `igop` to 500 in the reference - an explicit `igop == 1` also pins
        // `init_ip_ratio` to 16 - so the allocation window is the same 500-picture
        // estimate either way.
        let mut cfg = cfg;
        cfg.min_i_bit_prop = clip(cfg.min_i_bit_prop, 10, 100);
        cfg.max_i_bit_prop = clip(cfg.max_i_bit_prop, cfg.min_i_bit_prop, 100);
        cfg.init_ip_ratio = clip(cfg.init_ip_ratio, 160, 640);
        match cfg.igop {
            i if i <= 0 => cfg.igop = 500,
            1 => {
                cfg.init_ip_ratio = 16;
                cfg.igop = 500;
            }
            _ => {}
        }

        // The GOP budget comes from `bps_target` on CBR and from `bps_max` on VBR;
        // `target_bps` - what the controllers steer towards - is `bps_target` either
        // way, which is the distinction `bits_model_init` makes between its local
        // `target_bps` and `ctx->target_bps`.
        let budget_bps = match cfg.mode {
            // Only CBR budgets from the target; VBR and AVBR budget from the max.
            Mode::Cbr => cfg.bps_target,
            Mode::Vbr | Mode::Avbr => cfg.bps_max,
        };
        let target_bps = cfg.bps_target;
        let fps = (cfg.fps_num / cfg.fps_den).max(1);
        let gop_total_bits =
            (cfg.igop as i64 * budget_bps as i64 * cfg.fps_den as i64) / cfg.fps_num as i64;
        let bit_per_frame = budget_bps * cfg.fps_den / cfg.fps_num;
        let watl_thrd = 3 * budget_bps;
        let stat_watl = watl_thrd >> 3;

        let mut out = Model {
            gop_total_bits,
            bit_per_frame,
            i_scale: cfg.init_ip_ratio,
            p_scale: 16,
            i_sumbits: 0,
            p_sumbits: 0,
            vi_sumbits: 0,
            vi_scale: 32,
            i_bit: DataV2::new(I_WINDOW, 0).unwrap(),
            p_bit: DataV2::new(P_WINDOW1, 0).unwrap(),
            vi_bit: DataV2::new(I_WINDOW, 0).unwrap(),
            stat_bits: DataV2::new(fps * cfg.stats_time, bit_per_frame).unwrap(),
            stat_rate: DataV2::new(cfg.fps_num, 0).unwrap(),
            ins_bps: 0,
            stat_watl,
            watl_thrd,
            watl_base: stat_watl,
            target_bps,
            pre_target_bits: 0,
            pre_real_bits: 0,
            last_frame_type: FrameType::Intra,
            first: true,
            next_i_ratio: 0,
            cur_scale_qp: 0,
            next_ratio: 0,
            last_inst_bps: 0,
            pre_i_bit: DataV2::new(I_WINDOW, 0).unwrap(),
            pre_i_mean_qp: DataV2::new(I_WINDOW, -1).unwrap(),
            pre_p_bit: DataV2::new(P_WINDOW2, 0).unwrap(),
            madi: DataV2::new(P_WINDOW2, 0).unwrap(),
            madp: DataV2::new(P_WINDOW2, 0).unwrap(),
            gop_bits: DataV2::new(cfg.igop, 0).unwrap(),
            moving_ratio: 0,
            real_gbits: 0,
            avg_gbits: 0,
            min_still_percent: if cfg.bps_min > 0 && cfg.bps_max > 0 {
                (i64::from(cfg.bps_min) * 100 / i64::from(cfg.bps_max)) as i32
            } else {
                25
            },
            max_still_qp: 35,
            motion_sensitivity: 90,
            pre_mean_qp: 0,
            scale_qp: 0,
            hier_frame_cnt: [0; 4],
            qp_layer_id: 0,
            reenc_cnt: 0,
            drop_cnt: 0,
            cfg,
        };
        out.bits_frm_init();
        out
    }

    /// `bits_frm_init`'s `NORMAL_P` branch: the starting allocation for a GOP that
    /// has not produced any statistics yet.
    fn bits_frm_init(&mut self) {
        let gop_len = self.cfg.igop;
        let total = self.gop_total_bits;
        if self.cfg.smart_p_vgop > 1 {
            let vi_num = {
                let count = gop_len / self.cfg.smart_p_vgop;
                if count > 0 {
                    count - 1
                } else {
                    0
                }
            };
            self.i_scale = 320;
            self.p_scale = 16;
            self.vi_scale = 32;
            let p_bit = (total * 16
                / (i64::from(self.i_scale)
                    + i64::from(self.vi_scale) * i64::from(vi_num)
                    + i64::from(self.p_scale) * i64::from(gop_len - vi_num)))
                as i32;
            self.p_bit.reset(p_bit);
            self.p_sumbits = 5 * p_bit;
            let i_bit = p_bit * self.i_scale / 16;
            self.i_bit.reset(i_bit);
            self.i_sumbits = 2 * p_bit * self.i_scale / 16;
            let vi_bit = p_bit * self.vi_scale / 16;
            self.vi_bit.reset(vi_bit);
            self.vi_sumbits = 2 * p_bit * self.vi_scale / 16;
            return;
        }

        let p_bit = if gop_len <= 1 {
            (total * 16) as i32
        } else {
            (total * 16
                / (i64::from(self.i_scale) + i64::from(self.p_scale) * (gop_len - 1) as i64))
                as i32
        };
        self.p_bit.reset(p_bit);
        self.p_sumbits = 5 * p_bit;
        let i_bit = p_bit * self.i_scale / 16;
        self.i_bit.reset(i_bit);
        self.i_sumbits = 2 * p_bit * self.i_scale / 16;
    }

    /// `bits_model_alloc`, the function that turns the GOP budget and the current
    /// I/P scales into this picture's target.
    /// `bits_model_alloc` against a caller-supplied budget - AVBR needs this
    /// because it scales the GOP budget by how still the scene looks first.
    fn alloc_with(&mut self, frame: FrameType, total_bits: i64) -> i32 {
        let max_i_prop = self.cfg.max_i_bit_prop * 16;
        let gop_len = self.cfg.igop;
        self.i_scale = 80 * self.i_sumbits / (2 * self.p_sumbits);
        let smart = self.cfg.smart_p_vgop > 1;
        let vi_num = if smart {
            let count = gop_len / self.cfg.smart_p_vgop;
            if count > 0 {
                count - 1
            } else {
                0
            }
        } else {
            0
        };
        let (i_scale, scaled) = match frame {
            FrameType::Intra => (
                clip(self.i_scale, 16, 16000),
                total_bits * self.i_scale as i64,
            ),
            FrameType::InterP => (clip(self.i_scale, 16, max_i_prop), total_bits * 16),
            FrameType::InterVi => (
                clip(self.i_scale, 16, max_i_prop),
                total_bits * self.vi_scale as i64,
            ),
        };
        let denominator = if smart {
            i64::from(i_scale)
                + 16 * i64::from(gop_len - vi_num)
                + i64::from(vi_num) * i64::from(self.vi_scale)
        } else {
            i64::from(i_scale) + 16 * (gop_len - 1) as i64
        };
        let mut alloc_bits = (scaled / denominator) as i32;
        let super_threshold = self.super_threshold(frame);
        if alloc_bits > super_threshold && self.cfg.rc_priority == 1 {
            alloc_bits = super_threshold - (super_threshold >> 4);
        }
        alloc_bits
    }

    pub fn alloc(&mut self, frame: FrameType) -> i32 {
        let max_i_prop = self.cfg.max_i_bit_prop * 16;
        let gop_len = self.cfg.igop;
        self.i_scale = 80 * self.i_sumbits / (2 * self.p_sumbits);
        let total_bits = self.gop_total_bits;
        let smart = self.cfg.smart_p_vgop > 1;
        let vi_num = if smart {
            let count = gop_len / self.cfg.smart_p_vgop;
            if count > 0 {
                count - 1
            } else {
                0
            }
        } else {
            0
        };

        let (i_scale, scaled) = match frame {
            // An intra picture may take everything it can get, up to 16000; a P
            // picture is capped by the caller's `max_i_prop` so that it cannot
            // starve the rest of the GOP.
            FrameType::Intra => (
                clip(self.i_scale, 16, 16000),
                total_bits * self.i_scale as i64,
            ),
            FrameType::InterP => (clip(self.i_scale, 16, max_i_prop), total_bits * 16),
            FrameType::InterVi => (
                clip(self.i_scale, 16, max_i_prop),
                total_bits * self.vi_scale as i64,
            ),
        };
        // The reference also clamps this against a super-frame threshold, which
        // only exists when the caller asks for one (`super_cfg.super_mode`).
        let denominator = if smart {
            i64::from(i_scale)
                + 16 * i64::from(gop_len - vi_num)
                + i64::from(vi_num) * i64::from(self.vi_scale)
        } else {
            i64::from(i_scale) + 16 * (gop_len - 1) as i64
        };
        let mut alloc_bits = (scaled / denominator) as i32;
        let super_threshold = self.super_threshold(frame);
        if alloc_bits > super_threshold && self.cfg.rc_priority == 1 {
            alloc_bits = super_threshold - (super_threshold >> 4);
        }
        alloc_bits
    }

    fn super_threshold(&self, frame: FrameType) -> i32 {
        if self.cfg.super_mode == 0 {
            i32::MAX
        } else if frame == FrameType::Intra {
            self.cfg.super_i_thd as i32
        } else {
            self.cfg.super_p_thd as i32
        }
    }

    /// `calc_next_i_ratio`: after an intra picture, how much of a head start the
    /// next one gets. It accumulates while the previous picture keeps overshooting
    /// its share, is capped by a table indexed by the QP that picture coded at, and
    /// can never go negative - underspending is not taken back from the next intra.
    pub fn calc_next_i_ratio(&mut self, pre_i_qp: usize) {
        let max_i_prop = self.cfg.max_i_bit_prop * 16;
        let gop_len = self.cfg.igop;
        let bits_alloc = if gop_len > 1 {
            self.gop_total_bits * max_i_prop as i64 / (max_i_prop + 16 * (gop_len - 1)) as i64
        } else {
            self.gop_total_bits
        } as i32;

        if self.pre_real_bits > bits_alloc || self.next_i_ratio != 0 {
            let ratio = clip(
                ((self.pre_real_bits - bits_alloc) << 8) / bits_alloc,
                -256,
                256,
            );
            let ratio = self.next_i_ratio + ratio;
            self.next_i_ratio = if ratio >= 0 {
                ratio.min(MAX_I_DELTA_QP[pre_i_qp.min(50)])
            } else {
                0
            };
        }
    }

    /// `next_i_ratio`, in 256ths of the intra allocation.
    pub fn next_i_ratio(&self) -> i32 {
        self.next_i_ratio
    }

    /// `first_frm_flg`: the reference takes the first picture's target straight from
    /// the GOP budget, before any ratio has been computed.
    pub fn is_first(&self) -> bool {
        self.first
    }

    /// `bits_model_preset`: record what was promised, before the picture is coded.
    pub fn preset(&mut self, frame: FrameType, bit_target: i32) {
        self.stat_rate.preset(i32::from(bit_target != 0));
        self.stat_bits.preset(bit_target);
        self.ins_bps = self.stat_bits.sum() / self.cfg.stats_time;
        self.stat_watl = if bit_target + self.stat_watl > self.watl_thrd {
            self.watl_thrd - self.bit_per_frame
        } else {
            bit_target + self.stat_watl - self.bit_per_frame
        }
        .max(0);

        match frame {
            FrameType::Intra => {
                self.i_bit.preset(bit_target);
                self.i_sumbits = self.i_bit.sum();
                self.i_scale = 80 * self.i_sumbits / (2 * self.p_sumbits);
            }
            FrameType::InterP => {
                self.p_bit.preset(bit_target);
                self.p_sumbits = self.p_bit.sum().max(1);
                self.p_scale = 16;
            }
            FrameType::InterVi => {
                self.vi_bit.preset(bit_target);
                self.vi_sumbits = self.vi_bit.sum();
                self.vi_scale = clip(80 * self.vi_sumbits / (2 * self.p_sumbits), 16, 320);
            }
        }
    }

    /// `bits_model_update`: replace the promise with the measured size.
    pub fn update(&mut self, frame: FrameType, real_bit: i32) {
        self.update_measured(
            frame,
            Measured {
                bits: real_bit,
                ..Measured::default()
            },
        );
    }

    fn update_measured(&mut self, frame: FrameType, measured: Measured) {
        let real_bit = measured.bits;
        self.stat_rate.update(i32::from(real_bit != 0));
        self.stat_bits.update(real_bit);
        self.ins_bps = self.stat_bits.sum() / self.cfg.stats_time;
        self.stat_watl = if real_bit + self.stat_watl > self.watl_thrd {
            self.watl_thrd - self.bit_per_frame
        } else {
            real_bit + self.stat_watl - self.bit_per_frame
        }
        .max(0);

        match frame {
            FrameType::Intra => {
                self.i_bit.update(real_bit);
                self.i_sumbits = self.i_bit.sum();
                self.i_scale = 80 * self.i_sumbits / (2 * self.p_sumbits);
            }
            FrameType::InterP => {
                self.p_bit.update(real_bit);
                self.p_sumbits = self.p_bit.sum().max(1);
                self.p_scale = 16;
                self.madi.update(measured.madi);
            }
            FrameType::InterVi => {
                self.vi_bit.update(real_bit);
                self.vi_sumbits = self.vi_bit.sum();
                self.vi_scale = clip(80 * self.vi_sumbits / (2 * self.p_sumbits), 16, 320);
            }
        }
    }

    /// `rc_model_v2_end`'s bookkeeping, minus the AVBR-only updates.
    /// `rc_model_v2_end`'s bookkeeping.
    pub fn end(&mut self, frame: FrameType, bit_target: i32, real_bit: i32) {
        self.end_with(
            frame,
            bit_target,
            Measured {
                bits: real_bit,
                ..Measured::default()
            },
        );
    }

    /// The same, with the measurements AVBR needs.
    pub fn end_with(&mut self, frame: FrameType, bit_target: i32, measured: Measured) {
        // `last_inst_bps` is taken before the update, so it is the rate as this
        // picture was *promised*, not as it turned out.
        self.last_inst_bps = self.ins_bps;
        self.pre_target_bits = bit_target;
        self.pre_real_bits = measured.bits;
        self.last_frame_type = frame;
        self.first = false;
        self.scale_qp = self.cur_scale_qp;
        self.pre_mean_qp = measured.quality;
        self.reenc_cnt = 0;
        if self.cfg.mode == Mode::Avbr {
            self.moving_judge_update(frame, measured.bits, measured.quality, measured.madp);
            self.update_measured(frame, measured);
            self.bit_statics_update(measured.bits);
        } else {
            self.update_measured(frame, measured);
        }
    }

    /// `calc_cbr_ratio`: the four terms that move the quantiser for CBR, added into
    /// `next_ratio`. The bit target is not one of them - `bits_model_alloc` has
    /// already fixed it by the time this runs.
    ///
    /// The second term is the odd one: it is a difference of table entries, then
    /// multiplied by six or four when the instantaneous rate is closing in on the
    /// target from either side - and *zeroed* otherwise, so the table usually
    /// contributes nothing at all.
    pub fn calc_cbr_ratio(&mut self, frame: FrameType) -> i32 {
        // The reference's `calc_ratio` allocates before weighing; the weighing does not
        // feed back into the target, but the order fixes which `i_scale` is current.
        let bit_target = self.alloc(frame);
        let fluc_l = 3;
        let target = self.target_bps;

        let mut bit_diff_ratio = if self.pre_target_bits > self.pre_real_bits {
            52 * (self.pre_real_bits - self.pre_target_bits) / self.pre_target_bits
        } else {
            64 * (self.pre_real_bits - self.pre_target_bits) / self.pre_target_bits
        };

        let idx1 = clip((self.ins_bps << 5) / target, 0, 63) as usize;
        let idx2 = clip((self.last_inst_bps << 5) / target, 0, 63) as usize;
        let mut ins_ratio = TAB_LNX[idx1] - TAB_LNX[idx2];
        if self.ins_bps > self.last_inst_bps && target - self.last_inst_bps < (target >> 4) {
            ins_ratio *= 6;
        } else if self.ins_bps < self.last_inst_bps && self.last_inst_bps - target < (target >> 4) {
            ins_ratio *= 4;
        } else {
            ins_ratio = 0;
        }

        bit_diff_ratio = clip(bit_diff_ratio, -128, 256);
        ins_ratio = clip(ins_ratio, -128, 256);

        let bps_ratio = clip((self.ins_bps - target) * fluc_l / (target >> 4), -32, 32);
        let wl_ratio = clip(
            4 * (self.stat_watl - self.watl_base) * fluc_l / self.watl_base,
            -16,
            32,
        );

        self.next_ratio = bit_diff_ratio + ins_ratio + bps_ratio + wl_ratio;
        bit_target
    }

    /// `calc_vbr_ratio`: the VBR controller's three terms.
    ///
    /// It shares the allocation and the QP bookkeeping with CBR and differs in what
    /// it measures against: the bit-error term weights undershoot at 32 rather than
    /// 52, the logarithm table is indexed by `bps_max` rather than `bps_target`, the
    /// instantaneous-rate term only counts while the rate is above the target or
    /// while it is falling with the target still ahead, and the buffer water level
    /// is not a term at all - VBR is allowed to spend what it has saved.
    /// `moving_judge_update`: the AVBR-only history update at the end of a picture.
    fn moving_judge_update(
        &mut self,
        frame: FrameType,
        bit_real: i32,
        quality_real: i32,
        madp: i32,
    ) {
        match frame {
            FrameType::Intra => {
                self.pre_i_bit.update(bit_real);
                self.pre_i_mean_qp.update(quality_real);
            }
            FrameType::InterP => {
                self.pre_p_bit.update(bit_real);
                self.madp.update(madp);
            }
            FrameType::InterVi => {}
        }
    }

    /// `bit_statics_update`: what the last GOP cost against what a GOP should cost.
    fn bit_statics_update(&mut self, real_bit: i32) {
        self.gop_bits.update(real_bit);
        let mean_pbits = self.pre_p_bit.mean();
        let mean_ibits = self.pre_i_bit.mean();
        self.real_gbits = i64::from(self.gop_bits.sum());
        self.avg_gbits =
            i64::from(self.cfg.igop - 1) * i64::from(mean_pbits) + i64::from(mean_ibits);
    }

    /// `moving_ratio_calc`: how still the scene is, as a 0..255 ratio.
    ///
    /// It weighs the last intra pictures' size by how far their mean QP sat from the
    /// current one, compares that against the inter pictures' cost, then blends in
    /// the measured block differences and the motion sensitivity. The `255` branch
    /// is the "nothing measured yet" answer rather than a percentage, and two of the
    /// divisions truncate - this feeds a QP, so both matter.
    fn moving_ratio_calc(&self) -> i32 {
        let mut total_bit = 0i64;
        for i in 0..2 {
            let pre_i_bit = self.pre_i_bit.pre_val(i);
            let pre_mean_qp = self.pre_i_mean_qp.pre_val(i);
            let scale = if pre_mean_qp == -1 {
                32
            } else {
                let index = pre_mean_qp + 8 - self.pre_mean_qp;
                if index >= 0 {
                    MEAN_QP2SCALE[clip(index, 0, 15) as usize]
                } else {
                    14
                }
            };
            total_bit += i64::from(scale * pre_i_bit >> 5);
        }

        let pbit_sum = self.pre_p_bit.sum();
        let madi_sum = self.madi.sum();
        let madp_sum = self.madp.sum();
        let mut percent = if pbit_sum == 0 || total_bit == 0 {
            255
        } else {
            let index = clip(
                (((total_bit << 6) / i64::from(pbit_sum)) >> 4) as i32,
                1,
                99,
            );
            (BIT2PERCENT[index as usize] << 8) / 100
        };

        let percent_a = (self.min_still_percent - 30) << 8;
        let percent_b = 100 - self.min_still_percent;
        percent = (percent_a + percent * percent_b) / 70;

        // `prev_md_prop` is written only by the hierarchical-QP path, which is not
        // driven here, so the motion-vector term reduces to its constant.
        let mv_percnt = 0;
        let mv_ratio = (percent_a + 7680 + mv_percnt * percent_b) / 100;

        let mut mad_ratio = 256;
        if madi_sum != 0 {
            mad_ratio = clip(20 * madp_sum / madi_sum, 5, 100);
            mad_ratio = (mad_ratio << 8) / 100;
        }
        mad_ratio = (percent_a + 7680 + percent_b * mad_ratio) / 100;

        (percent
            + 1
            + (mv_ratio * self.motion_sensitivity + (100 - self.motion_sensitivity) * mad_ratio)
                / 100)
            >> 1
    }

    /// `calc_avbr_ratio`: VBR's terms on a GOP budget scaled by how still the scene
    /// is, plus two GOP-difference terms, plus the "already still, do not spend
    /// more" correction that subtracts six from the QP ratio.
    pub fn calc_avbr_ratio(&mut self, frame: FrameType) -> i32 {
        let mut moving_ratio = self.moving_ratio_calc();
        if self.moving_ratio - 2 >= moving_ratio {
            moving_ratio = self.moving_ratio - 2;
        }
        if self.moving_ratio > moving_ratio && (self.max_still_qp << 6) <= self.scale_qp {
            self.moving_ratio = clip(self.moving_ratio + 1, 1, 255);
        } else {
            self.moving_ratio = moving_ratio;
        }
        let mut moving_ratio = moving_ratio.max(1);

        let gop_bits = i64::from(moving_ratio) * self.gop_total_bits >> 8;
        let mut gop_kbits = (gop_bits >> 10) as i32;
        if gop_kbits < 1 {
            gop_kbits = 1;
        }

        let bit_target = self.alloc_with(frame, gop_bits);
        let bps_change = i64::from(moving_ratio) * i64::from(self.target_bps) >> 8;
        if moving_ratio < 0 {
            moving_ratio += 255;
        }
        let moving_percent = 100 * moving_ratio >> 8;

        let mut bit_diff_ratio = if self.pre_target_bits > self.pre_real_bits {
            32 * (self.pre_real_bits - self.pre_target_bits) / self.pre_target_bits
        } else {
            64 * (self.pre_real_bits - self.pre_target_bits) / self.pre_target_bits * moving_percent
        };

        // `pre_i_scale` is read here but never written in this vintage of the model.
        let i_ratio = clip(0 >> 4, 10, 200);
        let max_bps_target = self.cfg.bps_max;
        let idx1 = clip(self.ins_bps / (max_bps_target >> 5), 0, 63) as usize;
        let idx2 = clip(self.last_inst_bps / (max_bps_target >> 5), 0, 63) as usize;
        let mut ins_ratio = TAB_LNX[idx2] - TAB_LNX[idx1];
        let max_bps = (bps_change as i32).max(self.last_inst_bps);
        if self.ins_bps > max_bps {
            ins_ratio *= 6;
        } else if self.ins_bps < self.last_inst_bps && bps_change as i32 > self.last_inst_bps {
            ins_ratio *= 3;
        } else {
            ins_ratio = 0;
        }

        ins_ratio = clip(ins_ratio >> 2, -128, 256);
        bit_diff_ratio = clip(10 * bit_diff_ratio / i_ratio, -128, 256);
        let rgop_dratio = clip(
            24 * ((self.real_gbits - gop_bits) >> 10) as i32 / gop_kbits,
            -1,
            1,
        );
        let agop_dratio = clip(
            48 * ((self.avg_gbits - gop_bits) >> 10) as i32 / gop_kbits,
            -1,
            1,
        );

        let mut qratio = 0;
        let mut final_qratio = ins_ratio + bit_diff_ratio + agop_dratio + rgop_dratio;
        if max_bps_target >= self.ins_bps
            && final_qratio > 0
            && self.scale_qp >= (self.max_still_qp << 6)
        {
            final_qratio = ins_ratio + agop_dratio + rgop_dratio;
            qratio = -6;
        }
        self.next_ratio = qratio + final_qratio;
        bit_target
    }

    pub fn calc_vbr_ratio(&mut self, frame: FrameType) -> i32 {
        let bit_target = self.alloc(frame);
        let bps_change = self.target_bps;
        let max_bps = self.cfg.bps_max;

        let mut bit_diff_ratio = if self.pre_target_bits > self.pre_real_bits {
            32 * (self.pre_real_bits - self.pre_target_bits) / self.pre_target_bits
        } else {
            64 * (self.pre_real_bits - self.pre_target_bits) / self.pre_target_bits
        };

        let idx1 = clip(self.ins_bps / (max_bps >> 5), 0, 63) as usize;
        let idx2 = clip(self.last_inst_bps / (max_bps >> 5), 0, 63) as usize;
        let mut ins_ratio = TAB_LNX[idx1] - TAB_LNX[idx2];
        if self.ins_bps > bps_change && self.ins_bps > self.last_inst_bps {
            ins_ratio *= 6;
        } else if self.ins_bps <= self.last_inst_bps && bps_change > self.last_inst_bps {
            ins_ratio *= 3;
        } else {
            ins_ratio = 0;
        }

        bit_diff_ratio = clip(bit_diff_ratio, -128, 256);
        ins_ratio = clip(ins_ratio, -128, 256);
        let bps_ratio = clip(3 * (self.ins_bps - bps_change) / (max_bps >> 4), -16, 32);

        // Past an I/P ratio of 640 the intra share is large enough that both ratio
        // terms are damped rather than trusted.
        if self.i_scale > 640 {
            bit_diff_ratio = clip(bit_diff_ratio, -16, 32);
            ins_ratio = clip(ins_ratio, -16, 32);
        }

        self.next_ratio = bit_diff_ratio + ins_ratio + bps_ratio;
        bit_target
    }

    /// `rc_model_v2_start` plus `rc_model_v2_hal_start` for CBR: this picture's bit
    /// target and the QP it will be coded at.
    ///
    /// The first picture is the odd one: it is allocated straight from the GOP
    /// budget with no ratio at all, and its QP becomes the configured initial
    /// quality, from which `cur_scale_qp` is seeded two QPs high - that offset is
    /// the I-picture delta, and it is why the *next* picture lands where it does.
    pub fn start(&mut self, frame: FrameType) -> (i32, i32) {
        // For every picture after the first, `calc_ratio` is what allocates - it is
        // not a separate step after one.
        let (bit_target, first) = if self.first {
            (self.alloc(frame), true)
        } else {
            let target = match self.cfg.mode {
                Mode::Cbr => self.calc_cbr_ratio(frame),
                Mode::Vbr => self.calc_vbr_ratio(frame),
                Mode::Avbr => self.calc_avbr_ratio(frame),
            };
            (target, false)
        };
        self.preset(frame, bit_target);

        let mut quality_target = if first {
            let quality_init = first_quality(&self.cfg, bit_target);
            let seed = quality_init + self.cfg.i_quality_delta;
            self.cur_scale_qp = clip(
                seed << 6,
                self.cfg.quality_min << 6,
                self.cfg.quality_max << 6,
            );
            quality_init
        } else {
            self.cur_scale_qp = clip(
                self.cur_scale_qp + self.next_ratio,
                self.cfg.quality_min << 6,
                self.cfg.quality_max << 6,
            );
            self.cur_scale_qp >> 6
        };

        if frame == FrameType::InterVi {
            quality_target -= self.cfg.vi_quality_delta;
            quality_target = clip(quality_target, self.cfg.quality_min, self.cfg.quality_max);
        }

        if self.cfg.hier_qp_enabled {
            self.hier_advance(frame);
            if self.qp_layer_id != 0 {
                quality_target -= self.cfg.hier_qp_delta[self.qp_layer_id as usize - 1];
            }
            quality_target = clip(quality_target, self.cfg.quality_min, self.cfg.quality_max);
        }

        (bit_target, quality_target)
    }

    /// `rc_hier_calc_dealt_qp`: an intra picture starts a new hierarchy, while
    /// inter pictures walk the four counters from the deepest layer upward.
    fn hier_advance(&mut self, frame: FrameType) {
        if matches!(frame, FrameType::Intra | FrameType::InterVi) {
            self.hier_frame_cnt = [0; 4];
            self.qp_layer_id = 0;
            return;
        }

        for layer in (0..4).rev() {
            if self.hier_frame_cnt[layer] < self.cfg.hier_frame_num[layer] {
                self.hier_frame_cnt[layer] += 1;
                for deeper in layer + 1..4 {
                    self.hier_frame_cnt[deeper] = 0;
                }
                self.qp_layer_id = layer as i32 + 1;
                return;
            }
        }

        self.hier_frame_cnt = [0; 4];
        self.qp_layer_id = 0;
    }

    /// The layer selected for the most recent picture, zero meaning the
    /// hierarchy reset or is disabled.
    pub fn qp_layer_id(&self) -> i32 {
        self.qp_layer_id
    }

    /// `rc_model_v2_check_reenc` plus `rc_model_v2_hal_start`. Returns the QP to
    /// use for the same picture when another hardware attempt is required. The C
    /// first compares the picture's measured bits with its allocation and
    /// instantaneous rate, then the mode's re-encode ratio moves the quantiser
    /// before the current frame is submitted again.
    pub fn reencode_quality(
        &mut self,
        frame: FrameType,
        bit_target: i32,
        quality_target: i32,
        measured: Measured,
        max_times: u32,
    ) -> Option<i32> {
        if self.reenc_cnt >= max_times {
            return None;
        }

        let stat_time = self.cfg.stats_time;
        let last_ins_bps = self.stat_bits.sum() / stat_time;
        let ins_bps =
            (last_ins_bps * stat_time - self.stat_bits.pre_val(-1) + measured.bits) / stat_time;
        let default_bit_thr = match frame {
            FrameType::Intra => 3 * bit_target / 2,
            _ => 3 * bit_target,
        };
        let super_thr = self.super_threshold(frame);
        let super_frame = self.cfg.super_mode != 0 && measured.bits >= super_thr;
        if !super_frame && measured.bits <= default_bit_thr {
            return None;
        }

        let target_bps = self.target_bps;
        let rate_reencode = if self.cfg.mode == Mode::Cbr {
            target_bps / 20 < ins_bps - last_ins_bps
                && (target_bps + target_bps / 10 < ins_bps
                    || target_bps - target_bps / 10 > ins_bps)
        } else {
            let max_bps = self.cfg.bps_max;
            max_bps - (max_bps >> 3) < ins_bps && target_bps / 20 < ins_bps - last_ins_bps
        };
        let must_reencode = super_frame || rate_reencode;
        if !must_reencode {
            return None;
        }

        self.next_ratio = match self.cfg.mode {
            Mode::Cbr => {
                self.reencode_cbr_ratio(frame, bit_target, measured, ins_bps, last_ins_bps)
            }
            Mode::Vbr | Mode::Avbr => {
                self.reencode_vbr_ratio(bit_target, measured, ins_bps, last_ins_bps)
            }
        };
        if self.next_ratio == 0 || quality_target >= self.cfg.quality_max {
            return None;
        }
        self.reenc_cnt += 1;

        let i_delta = if frame == FrameType::Intra {
            self.cfg.i_quality_delta
        } else {
            0
        };
        let quality = if frame == FrameType::Intra {
            self.cur_scale_qp += self.next_ratio;
            let quality = (self.cur_scale_qp >> 6) - i_delta;
            self.cur_scale_qp = clip(
                self.cur_scale_qp,
                self.cfg.quality_min << 6,
                self.cfg.quality_max << 6,
            );
            quality
        } else {
            let qp_scale = clip(
                self.cur_scale_qp + self.next_ratio,
                self.cfg.quality_min << 6,
                self.cfg.quality_max << 6,
            );
            self.cur_scale_qp = qp_scale;
            let mut quality = qp_scale >> 6;
            if frame == FrameType::InterVi {
                quality -= self.cfg.vi_quality_delta;
            }
            quality
        };
        Some(clip(quality, self.cfg.quality_min, self.cfg.quality_max))
    }

    /// `check_super_frame`'s drop branch. MPP never drops an intra picture;
    /// otherwise a picture at or above the configured threshold is rolled back
    /// to the previous reference state and produces no payload.
    pub fn drops_super_frame(&mut self, frame: FrameType, measured: Measured) -> bool {
        let drop = self.cfg.super_mode == 1
            && frame != FrameType::Intra
            && measured.bits >= self.super_threshold(frame);
        if drop {
            self.drop_cnt += 1;
        }
        drop
    }

    /// The bitrate-overflow drop/pskip decision in `check_re_enc`.
    pub fn requests_pskip(&mut self, frame: FrameType, measured: Measured) -> bool {
        if self.cfg.drop_mode != 2
            || frame != FrameType::InterP
            || self.reenc_cnt != 0
            || (self.cfg.drop_gap != 0 && self.drop_cnt >= self.cfg.drop_gap)
        {
            return false;
        }
        let last = self.stat_bits.sum() / self.cfg.stats_time;
        let ins = (last * self.cfg.stats_time - self.stat_bits.pre_val(-1) + measured.bits)
            / self.cfg.stats_time;
        let threshold =
            i64::from(self.cfg.bps_max) * (100 + i64::from(self.cfg.drop_threshold)) / 100;
        let pskip = i64::from(ins) > threshold;
        if pskip {
            self.drop_cnt += 1;
        }
        pskip
    }

    /// The NORMAL-drop form of the same bitrate-overflow decision.
    pub fn requests_drop(&mut self, frame: FrameType, measured: Measured) -> bool {
        if self.cfg.drop_mode != 1
            || frame != FrameType::InterP
            || self.reenc_cnt != 0
            || (self.cfg.drop_gap != 0 && self.drop_cnt >= self.cfg.drop_gap)
        {
            return false;
        }
        let last = self.stat_bits.sum() / self.cfg.stats_time;
        let ins = (last * self.cfg.stats_time - self.stat_bits.pre_val(-1) + measured.bits)
            / self.cfg.stats_time;
        let threshold =
            i64::from(self.cfg.bps_max) * (100 + i64::from(self.cfg.drop_threshold)) / 100;
        let drop = i64::from(ins) > threshold;
        if drop {
            self.drop_cnt += 1;
        }
        drop
    }

    /// A normally accepted picture closes the continuous-drop gap.
    pub fn accept_frame(&mut self) {
        self.drop_cnt = 0;
    }

    /// `reenc_calc_cbr_ratio`. The super-frame branch is absent because this
    /// driver has not enabled a super-frame policy.
    fn reencode_cbr_ratio(
        &self,
        frame: FrameType,
        bit_target: i32,
        measured: Measured,
        ins_bps: i32,
        pre_ins_bps: i32,
    ) -> i32 {
        let real_bit = measured.bits;
        let target_bps = self.target_bps;
        let mut water_level = self.stat_watl + real_bit - self.bit_per_frame;
        if water_level > self.watl_thrd {
            water_level = self.watl_thrd - self.bit_per_frame;
        }
        water_level = water_level.max(0);

        let mut bit_diff_ratio = if bit_target > real_bit {
            32 * (real_bit - bit_target) / real_bit
        } else {
            32 * (real_bit - bit_target) / bit_target
        };
        let idx1 = clip(ins_bps / (target_bps >> 5), 0, 63) as usize;
        let idx2 = clip(pre_ins_bps / (target_bps >> 5), 0, 63) as usize;
        let mut ins_ratio = TAB_LNX[idx1] - TAB_LNX[idx2];
        let mut bps_ratio = 96 * (ins_bps - target_bps) / target_bps;
        let mut wl_ratio = 32 * (water_level - self.watl_base) / self.watl_base;

        if pre_ins_bps < ins_bps && target_bps != pre_ins_bps {
            ins_ratio = clip(6 * ins_ratio, -192, 256);
        } else if frame == FrameType::Intra {
            ins_ratio = clip(3 * ins_ratio, -192, 256);
        } else {
            ins_ratio = 0;
        }
        bit_diff_ratio = clip(bit_diff_ratio, -128, 256);
        bps_ratio = clip(bps_ratio, -32, 32);
        wl_ratio = clip(wl_ratio, -32, 32);

        let mut next_ratio = bit_diff_ratio + ins_ratio + bps_ratio + wl_ratio;
        if frame == FrameType::Intra && measured.madi > 0 {
            let tar_bpp = bit_target / self.cfg.total_mb;
            let lnb_t = (tar_bpp as f32).ln();
            let c = 6.7204f32;
            let a = -0.1435f32;
            let b = 0.0438f32;
            let start_qp = self.cur_scale_qp >> 6;
            let qp_c = ((lnb_t - measured.madi as f32 * b - c) / a + 14.0) as i32;
            if qp_c > start_qp {
                next_ratio = (qp_c << 6) - self.cur_scale_qp;
            }
        }
        next_ratio
    }

    /// `reenc_calc_vbr_ratio`; AVBR selects this same function in
    /// `bits_model_init`, despite using its own normal allocator.
    fn reencode_vbr_ratio(
        &self,
        bit_target: i32,
        measured: Measured,
        ins_bps: i32,
        pre_ins_bps: i32,
    ) -> i32 {
        let real_bit = measured.bits;
        let target_bps = self.target_bps;
        let max_bps_target = self.cfg.bps_max;
        let mut bit_diff_ratio = if bit_target <= real_bit {
            32 * (real_bit - bit_target) / bit_target
        } else {
            32 * (real_bit - bit_target) / real_bit
        };

        let idx1 = clip(ins_bps / (max_bps_target >> 5), 0, 63) as usize;
        let idx2 = clip(pre_ins_bps / (max_bps_target >> 5), 0, 63) as usize;
        let mut ins_ratio = if pre_ins_bps < ins_bps && target_bps < ins_bps {
            clip(6 * (TAB_LNX[idx1] - TAB_LNX[idx2]), -192, 256)
        } else {
            0
        };
        let mut bps_ratio = 96 * (ins_bps - target_bps) / target_bps;
        bit_diff_ratio = clip(bit_diff_ratio, -128, 256);
        bps_ratio = clip(bps_ratio, -32, 32);
        ins_ratio = clip(ins_ratio, -128, 256);
        bit_diff_ratio + ins_ratio + bps_ratio
    }

    /// `last_inst_bps`, the previous picture's instantaneous rate.
    pub fn last_inst_bps(&self) -> i32 {
        self.last_inst_bps
    }

    /// `moving_ratio`, AVBR's still/moving level.
    pub fn moving_ratio(&self) -> i32 {
        self.moving_ratio
    }

    /// `next_ratio` as the last `calc_cbr_ratio` left it.
    pub fn next_ratio(&self) -> i32 {
        self.next_ratio
    }

    /// `stat_watl`, the encoder buffer's water level.
    pub fn stat_watl(&self) -> i32 {
        self.stat_watl
    }

    /// `cur_scale_qp`, the QP in 64ths.
    pub fn cur_scale_qp(&self) -> i32 {
        self.cur_scale_qp
    }

    /// The GOP's whole budget, `gop_total_bits`.
    pub fn gop_total_bits(&self) -> i64 {
        self.gop_total_bits
    }

    /// One frame's plain share of the rate, `bit_per_frame`.
    pub fn bit_per_frame(&self) -> i32 {
        self.bit_per_frame
    }

    /// The scales the allocator is currently working from.
    pub fn scales(&self) -> (i32, i32) {
        (self.i_scale, self.p_sumbits)
    }

    /// `pre_real_bits`, what the last picture actually cost.
    pub fn pre_real_bits(&self) -> i32 {
        self.pre_real_bits
    }
}

impl Model {
    /// `ins_bps`, the instantaneous rate over the statistics window - one of the
    /// four terms the QP controller reads.
    pub fn ins_bps(&self) -> i32 {
        self.ins_bps
    }

    /// `watl_base`, the water level the controller treats as neutral.
    pub fn watl_base(&self) -> i32 {
        self.watl_base
    }

    /// `target_bps`, the rate the model was told to hold.
    pub fn target_bps(&self) -> i32 {
        self.target_bps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capture `board_h264_rc_capture.sh` recorded: six frames of 320x240 at
    /// `mpi_enc_test -rc 1 -bps 300000:1500000:2500000`, GOP 60, everything else at
    /// `h264e_api_v2.c`'s defaults.
    fn cbr() -> Model {
        Model::new(Config::cbr(300_000, 1_500_000, 2_500_000, 60))
    }

    /// `ctu_ebit` of one captured job, read out of the register words rather than
    /// typed in, so the two fixtures cannot drift apart.
    fn traced_ctu_ebit(job: usize) -> i32 {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/vepu580_rc_c_move6_cbr_regs.txt"
        ))
        .unwrap();
        let field = &crate::hal::vepu580_regs::BASE_RC_TGT_CTU_EBIT;
        let mut current: Option<usize> = None;
        for line in text.lines() {
            let mut words = line.trim().split_whitespace();
            match words.next() {
                Some("job") => current = Some(words.next().unwrap().parse().unwrap()),
                Some("BASE") if current == Some(job) => {
                    let base: Vec<u32> =
                        words.map(|w| u32::from_str_radix(w, 16).unwrap()).collect();
                    let word = base[field.word - 160];
                    return ((word >> field.shift) & ((1 << field.width) - 1)) as i32;
                }
                _ => {}
            }
        }
        panic!("job {job} in the capture");
    }

    /// The first picture's target is decided from the configuration alone, and the
    /// reference wrote the resulting register for us: 86956 bits per GOP share,
    /// scaled into sixteenths over 300 macroblocks, is `ctu_ebit` 4637.
    #[test]
    fn the_first_intra_target_matches_the_captured_job() {
        let mut model = cbr();
        assert!(model.is_first());
        assert_eq!(model.gop_total_bits(), 600_000, "60 frames at 10000 bits");
        assert_eq!(model.bit_per_frame(), 10_000);

        let target = model.alloc(FrameType::Intra);
        assert_eq!(target, 86_956, "600000 * 160 / (160 + 16 * 59)");
        assert_eq!(
            (target << 4) / 300,
            traced_ctu_ebit(0),
            "the same arithmetic the encoder's register write is made of"
        );
    }

    /// Before any picture has been coded, a P picture's target is exactly the
    /// starting share `bits_frm_init` put in the window - the allocation and the
    /// initialisation have to agree or the first GOP is aimed at two different
    /// numbers.
    #[test]
    fn the_gop_starts_from_the_configuration_alone() {
        let mut model = cbr();
        assert_eq!(model.scales(), (160, 43_475), "init_ip_ratio and 5 * p_bit");
        assert_eq!(model.alloc(FrameType::InterP), 8_695);
        assert_eq!(model.target_bps(), 300_000);
        assert_eq!(model.watl_base(), 3 * 300_000 / 8);
    }

    /// `preset` then `update` on the same slot: the promise is replaced by the
    /// measurement, not added to it. This is the pair `MppDataV2` exists for, and
    /// the allocator's sums depend on it.
    #[test]
    fn a_promise_is_replaced_by_the_measurement_it_predicted() {
        let mut model = cbr();
        let target = model.alloc(FrameType::Intra);
        model.preset(FrameType::Intra, target);
        let promised = model.scales();

        // 80 * 173906 / (2 * 43475) is 160 again: promising the intra picture its
        // designed share does not move the scale.
        assert_eq!(promised.0, 160);
        model.update(FrameType::Intra, 35_024);
        // The coded size is 35024 bits where 86956 were promised, so the intra
        // scale falls to 112 and the next P picture inherits a bigger share.
        assert_eq!(model.scales().0, 112);
        assert_eq!(model.alloc(FrameType::InterP), 9_090);
    }

    /// Overspending the intra share buys the next intra picture a head start, the
    /// table caps it by the QP that picture coded at, and underspending never
    /// takes anything back.
    #[test]
    fn the_head_start_accumulates_and_is_capped_by_qp() {
        let mut model = cbr();
        // `bits_alloc` for 30 % max I proportion is 202247 of the 600000 GOP bits.
        model.end(FrameType::Intra, 86_956, 210_000);
        model.calc_next_i_ratio(10);
        assert_eq!(model.next_i_ratio(), 9);
        model.calc_next_i_ratio(10);
        // Called again with the same overshoot it adds the same 9 again: the debt
        // grows by the *measured* amount each time, not by compounding itself.
        assert_eq!(model.next_i_ratio(), 18);

        // The same debt at a QP the table has run out of slack for is not granted.
        let mut capped = cbr();
        capped.end(FrameType::Intra, 86_956, 210_000);
        capped.calc_next_i_ratio(45);
        assert_eq!(capped.next_i_ratio(), 0);

        // And an underspent intra picture neither pays down the debt nor creates one.
        let mut under = cbr();
        under.calc_next_i_ratio(10);
        assert_eq!(under.next_i_ratio(), 0);
    }

    /// The model's state against the reference model's *own* printout.
    ///
    /// `rc_debug=0x40` makes `rc_model_v2.c` print `i_sumbits`, `p_sumbits` and
    /// `i_scale` at every `bits_model_update` and `bits_model_alloc`; those lines
    /// for two CBR runs are in `tests/data/rc_model_v2_c_state.txt`, together with
    /// the coded sizes each run fed back. Replaying the same sizes here reproduces
    /// the reference's `i_scale` at every picture of both runs, which is a
    /// stronger statement than matching the `ctu_ebit` register it eventually
    /// produces: the scale is the state the whole allocation is computed from.
    ///
    /// It also settles the swapped pair. Run B is the `mpi_enc_test` capture whose
    /// traced registers read 476 then 484 for the first two P pictures; the
    /// reference's own model says 112 then 130 for those two pictures, which is
    /// 484 then 476 - what this port computes. The transposition is in how the
    /// traced register windows line up with the jobs, not in the model.
    #[test]
    fn the_model_matches_the_reference_model_s_own_state() {
        let runs = [
            ("A", [30_224, 1_728, 2_096, 2_216, 2_616, 2_416]),
            ("B", [35_024, 2_632, 2_288, 2_560, 2_576, 3_248]),
        ];
        let want = [
            [160, 107, 128, 156, 200, 270],
            [160, 112, 130, 157, 196, 260],
        ];
        for ((name, coded), expect) in runs.into_iter().zip(want) {
            let mut model = cbr();
            let seen: Vec<i32> = coded
                .iter()
                .enumerate()
                .map(|(i, bits)| {
                    let frame = if i == 0 {
                        FrameType::Intra
                    } else {
                        FrameType::InterP
                    };
                    // `alloc` recomputes `i_scale` on entry, which is the value
                    // the reference prints there - so read it after the call.
                    let target = model.alloc(frame);
                    let scale = model.scales().0;
                    model.preset(frame, target);
                    model.end(frame, target, *bits);
                    scale
                })
                .collect();
            assert_eq!(seen, expect.to_vec(), "run {name}");
        }
    }
    /// AVBR's still/moving level, step by step, against the reference's own printout
    /// (`tests/data/rc_model_v2_c_avbr_state.txt`).
    ///
    /// The value worth the fixture is `moving_ratio` itself: it is a chain of five
    /// integer divisions and one shift, every one of which rounds, and it scales the
    /// GOP budget before any allocation happens. Get the blend wrong and the target
    /// drifts from picture one without anything looking obviously broken.
    #[test]
    fn avbr_scales_the_gop_by_a_still_moving_level() {
        let mut cfg = Config::cbr(300_000, 250_000, 350_000, 60);
        cfg.mode = Mode::Avbr;
        let mut model = Model::new(cfg);

        // Picture 0: the GOP budget is `bps_max`-sized, like VBR, and the QP is the
        // configured initial quality.
        let (target, quality) = model.start(FrameType::Intra);
        assert_eq!(target, 101_449);
        assert_eq!(quality, 26);
        model.end_with(
            FrameType::Intra,
            target,
            Measured {
                bits: 30_224,
                quality: 24,
                madi: 0,
                madp: 0,
            },
        );

        // The level is computed inside the controller, so it exists from the next
        // picture's `start` - the reference updates it in `calc_avbr_ratio` too.
        let (target, quality) = model.start(FrameType::InterP);
        assert_eq!(model.moving_ratio(), 222, "the still/moving level itself");
        assert_eq!(target, 9_276, "222/256 of the GOP, then the usual scale");
        assert_eq!(quality, 27);
    }

    #[test]
    fn hierarchical_qp_walks_the_reference_layer_cycle() {
        let mut cfg = Config::cbr(300_000, 250_000, 350_000, 60);
        cfg.hier_qp_enabled = true;
        cfg.hier_qp_delta = [1, 2, 3, 4];
        cfg.hier_frame_num = [1, 1, 1, 1];
        let mut model = Model::new(cfg);

        let (_, quality) = model.start(FrameType::Intra);
        assert_eq!((model.qp_layer_id(), quality), (0, 26));
        model.end(FrameType::Intra, 86_956, 30_224);

        let mut seen = Vec::new();
        for _ in 0..8 {
            let (target, quality) = model.start(FrameType::InterP);
            seen.push(model.qp_layer_id());
            assert!((8..=48).contains(&quality));
            model.end(FrameType::InterP, target, 2_000);
        }
        assert_eq!(seen, [4, 3, 4, 2, 4, 3, 4, 1]);

        let (_, quality) = model.start(FrameType::InterP);
        assert_eq!(model.qp_layer_id(), 4);
        assert!((8..=48).contains(&quality));
        model.end(FrameType::InterP, 8_000, 2_000);

        let (_, quality) = model.start(FrameType::Intra);
        assert_eq!(model.qp_layer_id(), 0);
        assert!((8..=48).contains(&quality));
    }

    #[test]
    fn smart_p_initialises_the_vi_window_and_allocation() {
        let mut cfg = Config::cbr(300_000, 250_000, 350_000, 60);
        cfg.smart_p_vgop = 31;
        cfg.vi_quality_delta = 2;
        let mut model = Model::new(cfg);

        assert_eq!(model.alloc(FrameType::InterP), 7_500);
        assert_eq!(model.alloc(FrameType::InterVi), 15_000);

        let (target, quality) = model.start(FrameType::Intra);
        assert_eq!((target, quality), (150_000, 26));
        model.end(FrameType::Intra, target, 30_224);

        let (target, quality) = model.start(FrameType::InterP);
        assert_eq!((target, quality), (8_333, 27));
    }

    #[test]
    fn cbr_reencodes_the_captured_low_bitrate_idr_at_qp_48() {
        let mut cfg = Config::cbr(10_000, 8_333, 11_666, 60);
        cfg.total_mb = 300;
        let mut model = Model::new(cfg);

        let (target, quality) = model.start(FrameType::Intra);
        assert_eq!((target, quality), (2_898, 26));
        assert_eq!(
            model.reencode_quality(
                FrameType::Intra,
                target,
                quality,
                Measured {
                    bits: 25_824,
                    quality: 27,
                    madi: 16,
                    madp: 0,
                },
                1,
            ),
            Some(48)
        );
    }

    #[test]
    fn cbr_super_frame_reencodes_across_the_first_two_pictures() {
        let mut cfg = Config::cbr(300_000, 250_000, 350_000, 60);
        cfg.total_mb = 300;
        cfg.super_mode = 2;
        cfg.super_i_thd = 10_000;
        cfg.super_p_thd = 1_000;
        let mut model = Model::new(cfg);

        let (target0, quality0) = model.start(FrameType::Intra);
        assert_eq!((target0, quality0), (86_956, 26));
        let retry0 = model.reencode_quality(
            FrameType::Intra,
            target0,
            quality0,
            Measured {
                bits: 44_816,
                quality: 23,
                madi: 9,
                madp: 0,
            },
            1,
        );
        assert_eq!(retry0, Some(25));
        model.end_with(
            FrameType::Intra,
            target0,
            Measured {
                bits: 46_088,
                quality: 22,
                madi: 9,
                madp: 0,
            },
        );

        let (target1, quality1) = model.start(FrameType::InterP);
        assert_eq!((target1, quality1), (9_005, 27));
        let retry = model.reencode_quality(
            FrameType::InterP,
            target1,
            quality1,
            Measured {
                bits: 13_488,
                quality: 27,
                madi: 9,
                madp: 35,
            },
            1,
        );
        assert_eq!(
            retry,
            Some(28),
            "next_ratio {}, cur_scale {}",
            model.next_ratio(),
            model.cur_scale_qp()
        );
    }

    #[test]
    fn super_frame_drop_never_drops_an_intra_picture() {
        let mut cfg = Config::cbr(300_000, 250_000, 350_000, 60);
        cfg.super_mode = 1;
        cfg.super_i_thd = 10_000;
        cfg.super_p_thd = 1_000;
        let mut model = Model::new(cfg);
        let oversized = Measured {
            bits: 20_000,
            ..Measured::default()
        };

        assert!(!model.drops_super_frame(FrameType::Intra, oversized));
        assert!(model.drops_super_frame(FrameType::InterP, oversized));
        assert!(!model.drops_super_frame(
            FrameType::InterP,
            Measured {
                bits: 999,
                ..Measured::default()
            }
        ));
    }

    #[test]
    fn super_frame_drop_rolls_the_cbr_qp_sequence_like_the_reference() {
        let mut cfg = Config::cbr(300_000, 250_000, 350_000, 60);
        cfg.total_mb = 300;
        cfg.super_mode = 1;
        cfg.super_i_thd = 10_000;
        cfg.super_p_thd = 1_000;
        let mut model = Model::new(cfg);

        let (target, quality) = model.start(FrameType::Intra);
        let retry = model
            .reencode_quality(
                FrameType::Intra,
                target,
                quality,
                Measured {
                    bits: 44_816,
                    quality: 23,
                    madi: 9,
                    madp: 0,
                },
                1,
            )
            .unwrap();
        assert_eq!(retry, 25);
        model.end_with(
            FrameType::Intra,
            target,
            Measured {
                bits: 46_088,
                quality: 22,
                madi: 9,
                madp: 0,
            },
        );

        let attempts = [
            (13_488, 27, 35),
            (17_912, 27, 77),
            (17_664, 27, 93),
            (25_240, 25, 117),
            (28_520, 23, 132),
        ];
        let expected_qp = [27, 26, 26, 24, 23];
        let mut seen = Vec::new();
        for ((bits, measured_qp, madp), expected) in attempts.into_iter().zip(expected_qp) {
            let (target, quality) = model.start(FrameType::InterP);
            seen.push(quality);
            let measured = Measured {
                bits,
                quality: measured_qp,
                madi: 9,
                madp,
            };
            assert!(model.drops_super_frame(FrameType::InterP, measured));
            model.end_with(
                FrameType::InterP,
                target,
                Measured {
                    bits: 0,
                    quality,
                    madi: 9,
                    madp,
                },
            );
            model.end_with(
                FrameType::InterP,
                target,
                Measured {
                    bits: 0,
                    quality,
                    madi: 9,
                    madp,
                },
            );
            assert_eq!(quality, expected, "seen {seen:?}");
        }
        assert_eq!(seen, expected_qp);
    }

    /// VBR budgets from `bps_max` and steers towards `bps_target`. Both halves are
    /// visible in the reference's own printout (`mode vbr bps [250000:300000:350000]`,
    /// `total_bits 112000000`): a port that budgets from the target produces a first
    /// picture aimed at 86,956 instead of 101,449 and is wrong from picture zero.
    #[test]
    fn vbr_budgets_from_the_max_and_steers_to_the_target() {
        let mut cfg = Config::cbr(300_000, 250_000, 350_000, 60);
        cfg.mode = Mode::Vbr;
        let mut model = Model::new(cfg);

        assert_eq!(
            model.gop_total_bits(),
            700_000,
            "60 frames of 350 kbps / 30"
        );
        assert_eq!(model.bit_per_frame(), 11_666, "bps_max / fps");
        let (target, quality) = model.start(FrameType::Intra);
        assert_eq!(target, 101_449);
        assert_eq!(quality, 26);
        model.end(FrameType::Intra, target, 30_224);

        // 32 rather than 52 weights the undershoot, and the table is read against
        // `bps_max`, so the next target follows the C's 10,697 rather than the CBR
        // controller's 9,134 for the same coded size.
        let (target, quality) = model.start(FrameType::InterP);
        assert_eq!(target, 10_697);
        assert_eq!(quality, 27);
        assert_eq!(model.next_ratio(), -15);
    }

    /// The quantiser controller, picture by picture, against the reference's own
    /// printout (`tests/data/rc_model_v2_c_cbr_qp.txt`, captured with
    /// `rc_debug=0xf0`). Four numbers per picture are compared, and they are the
    /// controller's whole visible life: the bit target, the QP it asks for, the
    /// instantaneous rate and the buffer water level the decision was made from.
    ///
    /// This is the part of rate control that had been described here as "the next
    /// unknown". It is now pinned instead: `calc_cbr_ratio`'s four terms and the
    /// `cur_scale_qp` bookkeeping reproduce the reference exactly on all six
    /// pictures, including the two places where a plausible transcription would
    /// drift - `ins_ratio` being *zeroed* unless the rate is closing on the target
    /// from either side, and the I-picture delta entering as a two-QP offset on the
    /// 64ths scale at the first picture only.
    #[test]
    fn the_cbr_quantiser_matches_the_reference_printout() {
        let coded = [30_224, 1_728, 2_096, 2_216, 2_616, 2_416];
        let want_target = [86_956, 9_134, 8_955, 8_727, 8_391, 7_907];
        let want_quality = [26, 27, 27, 26, 25, 25];
        let want_next = [0, -22, -33, -31, -71, -29];
        let want_ins = [0, 306_741, 303_984, 301_349, 298_754, 296_293];
        let want_pre_ins = [0, 325_652, 306_452, 303_635, 300_925, 298_218];
        let want_watl = [0, 204, 195, 187, 178, 169];

        let mut model = cbr();
        for (i, bits) in coded.iter().enumerate() {
            let frame = if i == 0 {
                FrameType::Intra
            } else {
                FrameType::InterP
            };
            // What the controller is about to see: the state the previous picture
            // left behind, before this one promises anything.
            let (ins, pre_ins, watl) = (model.ins_bps(), model.last_inst_bps(), model.stat_watl());
            let (target, quality) = model.start(frame);

            assert_eq!(target, want_target[i], "bit target, picture {i}");
            assert_eq!(quality, want_quality[i], "quality target, picture {i}");
            assert_eq!(model.next_ratio(), want_next[i], "next_ratio, picture {i}");
            if i > 0 {
                assert_eq!(ins, want_ins[i], "ins_bps, picture {i}");
                assert_eq!(pre_ins, want_pre_ins[i], "last_inst_bps, picture {i}");
                assert_eq!(watl >> 10, want_watl[i], "stat_watl, picture {i}");
            }
            model.end(frame, target, *bits);
        }
    }

    /// The bucket edges of `cal_first_i_start_qp`, one either side of each of the
    /// eight comparisons against `mb_num`. The table values are read off
    /// `rc_model_v2.c`, not recomputed: 320x240 (300 blocks) sits in the 200-block
    /// row, whose `tab_bit` is 3150, so `(300 * 3150 - 350) / 86956` is qscale 10
    /// and `qscale2qp[10]` is QP 23 - the number the captured first picture would
    /// have got had the harness not set one.
    #[test]
    fn first_qp_uses_the_bucket_the_picture_falls_in() {
        // Each row states its own arithmetic so the numbers can be re-derived:
        // qscale = (blocks * tab_bit - 350) / budget, clipped to 4..=95, then
        // read through qscale2qp. Rows come in straddling pairs around each
        // `mb_num` edge so a one-row bucket error cannot hide.
        let cases: [(u32, i32, i32); 16] = [
            // The zero edge of `mb_num` is at or below every picture, so the
            // first picture is already on the 3570 row: qscale (3570-350)/100.
            (1, 100, 34),
            // 199 vs 200, the 3570 row trading for the 3150 one: qscale 71 -> 42
            // against 62 -> 41, so the pair discriminates the table, not just
            // the bucket.
            (199, 10_000, 42),
            (200, 10_000, 41),
            (699, 50_000, 37),      // 3150 row: 2201500/50000 = 44
            (700, 50_000, 36),      // 2940 row: 2057650/50000 = 41
            (1199, 60_000, 40),     // 2940 row: 3524610/60000 = 58
            (1200, 60_000, 39),     // 2730 row: 3275650/60000 = 54
            (1999, 60_000, 44),     // 2730 row: 5456920/60000 = 90
            (2000, 60_000, 45),     // 3780 row: 125 -> clipped
            (4000, 300_000, 33),    // 2100 row: 8399650/300000 = 27
            (7999, 300_000, 39),    // 2100 row: 16797550/300000 = 55
            (8000, 300_000, 37),    // 1680 row: 13439650/300000 = 44
            (15999, 1_000_000, 32), // 1680 row: 26877970/1e6 = 26
            (16000, 1_000_000, 34), // 2100 row: 33599650/1e6 = 33
            (19999, 1_000_000, 36), // 2100 row: 41997550/1e6 = 41
            // 320x240 against the captured first-picture target: qscale 10 -> 23.
            (300, 86_956, 23),
        ];
        for (total_mb, target_bit, want) in cases {
            assert_eq!(
                cal_first_i_start_qp(target_bit, total_mb),
                want,
                "{total_mb} blocks at {target_bit} bits"
            );
        }
    }

    /// The two clip ends of `qscale2qp`, and the unsigned wrap that decides which
    /// end a picture with no macroblocks lands on: `0 * 3570 - 350` is not a small
    /// negative number to the C, it is `0xFFFFFFEA`, which divided by anything
    /// comes back through the signed `index` below 4 and clamps to QP 15 - where
    /// an i64 transcription would reach the same 15 only by accident, and a
    /// non-wrapping one would reach it at the wrong end of the table.
    /// `bits_model_init`: an unset or all-intra GOP is measured as 500 pictures,
    /// and the all-intra form also pins `init_ip_ratio` to 16.
    #[test]
    fn unbounded_and_all_intra_gops_normalise_to_500() {
        let cfg = Config::cbr(300000, 250000, 350000, 0);
        let model = Model::new(cfg);
        assert_eq!(model.cfg.igop, 500);

        let mut cfg = Config::cbr(300000, 250000, 350000, 1);
        let model = Model::new(cfg.clone());
        assert_eq!(model.cfg.igop, 500);
        assert_eq!(model.cfg.init_ip_ratio, 16);

        cfg.igop = 60;
        assert_eq!(Model::new(cfg).cfg.igop, 60);
    }

    #[test]
    fn first_qp_clips_at_both_ends_and_wraps_unsigned_below_zero() {
        // Too little budget to code even at the coarsest row: index 95.
        assert_eq!(cal_first_i_start_qp(1000, 300), 45);
        // So much budget that qscale truncates to zero: index clamped up to 4.
        assert_eq!(cal_first_i_start_qp(10_000_000, 300), 15);
        assert_eq!(cal_first_i_start_qp(100_000_000, 300), 15);
        // The wrapped picture, at two different budgets - and the two budgets
        // do *not* agree: 0xFFFFFEA2 divided by one is the same wrapped word the
        // signed `index` reads as -350, clamping to the bottom QP 15, while
        // divided by a thousand it is 4294966, a plain positive quotient that
        // clips to the top. Only unsigned arithmetic through the division
        // reproduces both answers; the C returns 15 and 45.
    }

    /// A caller that never set an initial quality gets the derived QP on the
    /// first picture - 23 rather than the library default's 26 for the captured
    /// 320x240 CBR job - and the scale seeded two QPs above the *derived* value,
    /// because `rc_model_v2_hal_start` runs the derivation before the I-delta.
    /// Every later picture is unaffected: the derivation happens once, behind
    /// `first_frm_flg`.
    #[test]
    fn an_unset_initial_quality_is_derived_before_the_i_delta_seeds() {
        let mut cfg = Config::cbr(300_000, 250_000, 350_000, 60);
        cfg.quality_init = -1;
        assert_eq!(
            cal_first_i_start_qp(86_956, 300),
            23,
            "the target the model will use"
        );

        let mut model = Model::new(cfg);
        let (target, quality) = model.start(FrameType::Intra);
        assert_eq!(target, 86_956, "the budget itself does not change");
        assert_eq!(quality, 23, "the derived first QP, not the 26 default");
        assert_eq!(model.cur_scale_qp(), (23 + 2) << 6);

        // The next picture steers from the seeded scale like any other; the
        // derivation does not run twice.
        model.end(FrameType::Intra, target, 30_224);
        let (_, quality) = model.start(FrameType::InterP);
        assert_eq!(quality, model.cur_scale_qp() >> 6);
    }

    /// Fixed QP is not the derivation's path: `rc_model_v2_start` refuses it
    /// before the model ever reaches the allocation ("invalid fix qp found set
    /// default qp 26"). The driver maps an unset quality onto 26 there, so the
    /// model must never see it and invent a picture QP of its own.
    #[test]
    fn an_unset_quality_under_fixed_qp_stays_the_drivers_question() {
        let mut cfg = Config::cbr(300_000, 250_000, 350_000, 60);
        cfg.quality_init = -1;
        assert_eq!(first_quality(&cfg, 86_956), 23, "the derivation");
        cfg.quality_init = 0;
        assert_eq!(
            first_quality(&cfg, 86_956),
            23,
            "zero says unset too - rc_model_v2_start folds both onto -1"
        );
        cfg.quality_init = -1;
        assert_eq!(
            first_quality(&cfg, 0),
            26,
            "no budget to derive from: the log line's fallback QP"
        );
        cfg.quality_init = 31;
        assert_eq!(
            first_quality(&cfg, 0),
            31,
            "a quality the caller did set is never touched by either"
        );
    }
}
