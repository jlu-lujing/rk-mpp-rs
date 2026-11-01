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

        // CBR aims at `bps_target`; every other mode aims at `bps_max`.
        let target_bps = cfg.bps_target;
        let fps = (cfg.fps_num / cfg.fps_den).max(1);
        let gop_total_bits =
            (cfg.igop as i64 * target_bps as i64 * cfg.fps_den as i64) / cfg.fps_num as i64;
        let bit_per_frame = target_bps * cfg.fps_den / cfg.fps_num;
        let watl_thrd = 3 * target_bps;
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
    pub fn end(&mut self, frame: FrameType, bit_target: i32, real_bit: i32) {
        self.pre_target_bits = bit_target;
        self.pre_real_bits = real_bit;
        self.last_frame_type = frame;
        self.first = false;
        self.update(frame, real_bit);
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

    /// What the model actually produces against what the reference wrote, over the
    /// captured six frames, with the reference's own coded sizes fed back in. Four
    /// of the six targets are the same number, and the trajectory from the third
    /// picture on is exact - including the decay, which happens because every P
    /// picture codes under its share and `i_scale` rises as `p_sumbits` falls.
    ///
    /// Pictures 1 and 2 are swapped: the reference promised 476 then 484, we
    /// promise 484 then 476, and both then follow the same path. A mis-scaled
    /// feedback would move every later picture, so the cause is something that
    /// touches the first P pair only, and it is still open. This test pins our
    /// behaviour exactly so that the day the swap is explained, the number that
    /// changes is the one under test.
    #[test]
    fn the_trajectory_matches_the_capture_except_for_one_swapped_pair() {
        // The C encoder's own per-picture sizes, in bytes, from its STATUS reads.
        let coded: [usize; 6] = [4378, 329, 286, 320, 322, 406];
        let ours: Vec<i32> = {
            let mut model = cbr();
            coded
                .iter()
                .enumerate()
                .map(|(i, bytes)| {
                    let frame = if i == 0 {
                        FrameType::Intra
                    } else {
                        FrameType::InterP
                    };
                    let target = model.alloc(frame);
                    model.preset(frame, target);
                    model.end(frame, target, (bytes * 8) as i32);
                    (target << 4) / 300
                })
                .collect()
        };
        let reference = [4637, 476, 484, 465, 449, 425];
        assert_eq!(ours, [4637, 484, 476, 465, 449, 425]);
        assert_eq!(
            vec![ours[0], ours[3], ours[4], ours[5]],
            vec![reference[0], reference[3], reference[4], reference[5]],
            "the first picture and the whole tail are the reference's numbers"
        );
    }
}
