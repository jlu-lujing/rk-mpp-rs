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

/// The picture kind the allocator is told about. `GOP_MODE_SMART_P` and the
/// intra-refresh frame type are the reference's other cases; this driver drives
/// `NORMAL_P` and does not pretend otherwise - see [`Model::start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Intra,
    InterP,
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
    /// The quantiser bounds, on MPP's 0..100 quality scale. `h264e_api_v2.c`'s
    /// defaults: medium (26) as the initial target and 8..48 as the bounds, with an
    /// I-picture delta of two - the `:2` the `uuid_rc_cfg` SEI ends with.
    pub quality_init: i32,
    pub quality_min: i32,
    pub quality_max: i32,
    pub i_quality_delta: i32,
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
            mode: Mode::Cbr,
        }
    }
}

fn clip(value: i32, low: i32, high: i32) -> i32 {
    value.max(low).min(high)
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
    /// `i_bit`, `p_bit`, and the water-level statistics.
    i_bit: DataV2,
    p_bit: DataV2,
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
        // rewrite `igop` to 500 in the reference; this driver only ever asks for a
        // real GOP, and the rewrite would silently change the allocation window.
        let mut cfg = cfg;
        cfg.min_i_bit_prop = clip(cfg.min_i_bit_prop, 10, 100);
        cfg.max_i_bit_prop = clip(cfg.max_i_bit_prop, cfg.min_i_bit_prop, 100);
        cfg.init_ip_ratio = clip(cfg.init_ip_ratio, 160, 640);
        assert!(
            cfg.igop > 1,
            "the all-intra and infinite GOP forms are not ported"
        );

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
            i_bit: DataV2::new(I_WINDOW, 0).unwrap(),
            p_bit: DataV2::new(P_WINDOW1, 0).unwrap(),
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
        let (i_scale, scaled) = match frame {
            FrameType::Intra => (
                clip(self.i_scale, 16, 16000),
                total_bits * self.i_scale as i64,
            ),
            FrameType::InterP => (clip(self.i_scale, 16, max_i_prop), total_bits * 16),
        };
        (scaled / (i_scale as i64 + 16 * (gop_len - 1) as i64)) as i32
    }

    pub fn alloc(&mut self, frame: FrameType) -> i32 {
        let max_i_prop = self.cfg.max_i_bit_prop * 16;
        let gop_len = self.cfg.igop;
        self.i_scale = 80 * self.i_sumbits / (2 * self.p_sumbits);
        let total_bits = self.gop_total_bits;

        let (i_scale, scaled) = match frame {
            // An intra picture may take everything it can get, up to 16000; a P
            // picture is capped by the caller's `max_i_prop` so that it cannot
            // starve the rest of the GOP.
            FrameType::Intra => (
                clip(self.i_scale, 16, 16000),
                total_bits * self.i_scale as i64,
            ),
            FrameType::InterP => (clip(self.i_scale, 16, max_i_prop), total_bits * 16),
        };
        // The reference also clamps this against a super-frame threshold, which
        // only exists when the caller asks for one (`super_cfg.super_mode`).
        (scaled / (i_scale as i64 + 16 * (gop_len - 1) as i64)) as i32
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
        }
    }

    /// `bits_model_update`: replace the promise with the measured size.
    pub fn update(&mut self, frame: FrameType, real_bit: i32) {
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
        if self.cfg.mode == Mode::Avbr {
            self.moving_judge_update(frame, measured.bits, measured.quality, measured.madp);
            self.update(frame, measured.bits);
            self.bit_statics_update(measured.bits);
        } else {
            self.update(frame, measured.bits);
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

        let quality_target = if first {
            let seed = self.cfg.quality_init + self.cfg.i_quality_delta;
            self.cur_scale_qp = clip(
                seed << 6,
                self.cfg.quality_min << 6,
                self.cfg.quality_max << 6,
            );
            self.cfg.quality_init
        } else {
            self.cur_scale_qp = clip(
                self.cur_scale_qp + self.next_ratio,
                self.cfg.quality_min << 6,
                self.cfg.quality_max << 6,
            );
            self.cur_scale_qp >> 6
        };

        (bit_target, quality_target)
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
}
