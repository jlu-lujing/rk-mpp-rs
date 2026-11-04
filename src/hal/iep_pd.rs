//! `iep2/iep2_pd.c` and `iep2/iep2_ff.c`: the inverse-telecine (pulldown) and
//! field-order state machines.
//!
//! IEP2 runs these only when the caller selects `IEP2_DIL_MODE_DECT` (or `PD`):
//! at the start of a frame the *previous* job's `struct iep2_output` statistics
//! drive both machines, a confirmed pulldown switches the frame to
//! `IEP2_DIL_MODE_PD` with a per-frame `pd_mode`, and a `PD` frame is followed
//! by a second `DECT` job. `iep2_test` hard-codes I5O2, so this path needs
//! `tools/iep2_dect_probe.c` as its oracle.
//!
//! The tables, thresholds and early returns are copied from the reference so a
//! state machine can be compared frame by frame rather than only through the
//! final pixels.

use crate::hal::iep::{
    Iep2FieldOrder, OUTPUT_WORDS, OUT_DECT_FF_BLE_BCNT, OUT_DECT_FF_BLE_TCNT, OUT_DECT_FF_COMB_F,
    OUT_DECT_FF_CUR_BCNT, OUT_DECT_FF_CUR_TCNT, OUT_DECT_FF_NXT_BCNT, OUT_DECT_FF_NXT_TCNT,
    OUT_DECT_FF_NZ, OUT_DECT_PD_BCNT, OUT_DECT_PD_TCNT, OUT_FF_GRADT_BCNT, OUT_FF_GRADT_TCNT,
};

/// `FIELD_DIFF_SUM_THR` from `iep2_pd.c`.
pub const FIELD_DIFF_SUM_THR: i32 = 32;
/// `PD_FRAME_PERIOD`.
pub const PD_FRAME_PERIOD: i32 = 5;
/// `FIELD_ORDER_RATIO_SIZE`.
pub const FIELD_ORDER_RATIO_SIZE: usize = 10;

/// `PD_BS`: top field changed, bottom unchanged.
const PD_BS: i32 = 2;
/// `PD_TS`: top unchanged, bottom changed.
const PD_TS: i32 = 1;
/// `PD_DF`: both fields changed.
const PD_DF: i32 = 0;

/// `PD_TYPES_*` in the order of `pd_table`.
pub const PD_TYPES_3_2_3_2: i32 = 0;
pub const PD_TYPES_2_3_2_3: i32 = 1;
pub const PD_TYPES_2_3_3_2: i32 = 2;
pub const PD_TYPES_3_2_2_3: i32 = 3;
pub const PD_TYPES_UNKNOWN: i32 = 4;

/// `PD_COMP_FLAG_*`: the per-frame compose mode written to `pd_mode`.
pub const PD_COMP_FLAG_CC: i32 = 0;
pub const PD_COMP_FLAG_CN: i32 = 1;
pub const PD_COMP_FLAG_NC: i32 = 2;
pub const PD_COMP_FLAG_NON: i32 = 3;

const PD_TABLE: [[i32; 5]; 5] = [
    [PD_TS, PD_DF, PD_BS, PD_DF, PD_DF], // 3:2:3:2
    [PD_DF, PD_BS, PD_DF, PD_TS, PD_DF], // 2:3:2:3
    [PD_TS, PD_BS, PD_DF, PD_DF, PD_DF], // 2:3:3:2
    [PD_BS, PD_DF, PD_DF, PD_TS, PD_DF], // 3:2:2:3
    [PD_DF, PD_DF, PD_DF, PD_DF, PD_DF], // unknown
];

const SP_TABLE: [[i32; 5]; 5] = [
    [0, 1, 1, 0, 0],
    [0, 0, 1, 1, 0],
    [0, 1, 0, 0, 0],
    [0, 1, 1, 1, 0],
    [1, 1, 1, 1, 1],
];

const FP_TABLE: [[i32; 5]; 5] = [
    [1, 1, 1, 0, 0],
    [0, 1, 1, 1, 0],
    [0, 1, 1, 0, 0],
    [1, 1, 1, 1, 0],
    [1, 1, 1, 1, 1],
];

/// The `struct iep2_output` counters both machines read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Iep2DectStats {
    pub pd_tcnt: u32,
    pub pd_bcnt: u32,
    pub ff_cur_tcnt: u32,
    pub ff_cur_bcnt: u32,
    pub ff_nxt_tcnt: u32,
    pub ff_nxt_bcnt: u32,
    pub ff_ble_tcnt: u32,
    pub ff_ble_bcnt: u32,
    pub ff_nz: u32,
    pub ff_comb_f: u32,
    pub gradt_tcnt: u32,
    pub gradt_bcnt: u32,
}

impl Iep2DectStats {
    /// Read the counters out of a completed `MPP_CMD_SET_REG_READ` block.
    pub fn from_words(words: &[u32; OUTPUT_WORDS]) -> Self {
        Self {
            pd_tcnt: words[OUT_DECT_PD_TCNT],
            pd_bcnt: words[OUT_DECT_PD_BCNT],
            ff_cur_tcnt: words[OUT_DECT_FF_CUR_TCNT],
            ff_cur_bcnt: words[OUT_DECT_FF_CUR_BCNT],
            ff_nxt_tcnt: words[OUT_DECT_FF_NXT_TCNT],
            ff_nxt_bcnt: words[OUT_DECT_FF_NXT_BCNT],
            ff_ble_tcnt: words[OUT_DECT_FF_BLE_TCNT],
            ff_ble_bcnt: words[OUT_DECT_FF_BLE_BCNT],
            ff_nz: words[OUT_DECT_FF_NZ],
            ff_comb_f: words[OUT_DECT_FF_COMB_F],
            gradt_tcnt: words[OUT_FF_GRADT_TCNT],
            gradt_bcnt: words[OUT_FF_GRADT_BCNT],
        }
    }
}

/// `struct iep2_ff_info`, zero-initialised by `iep2_init`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldOrderState {
    pub tff_offset: i32,
    pub bff_offset: i32,
    pub tff_score: i32,
    pub bff_score: i32,
    pub frm_offset: i32,
    pub fie_offset: i32,
    pub frm_score: i32,
    pub fie_score: i32,
    pub fo_detected: i32,
    pub is_frm: i32,
    pub fo_ratio: [u32; FIELD_ORDER_RATIO_SIZE],
    pub fo_ratio_idx: u32,
    pub fo_ratio_cnt: u32,
    pub fo_ratio_sum: u32,
    pub fo_ratio_avg: u32,
}

/// `struct iep2_pd_info`; `pdtype` starts at `PD_TYPES_UNKNOWN` and `step` at -1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PulldownState {
    pub temporal: [i32; 5],
    pub spatial: [i32; 5],
    pub fcoeff: [i32; 5],
    pub i: i32,
    pub pdtype: i32,
    pub step: i32,
}

impl Default for PulldownState {
    fn default() -> Self {
        Self {
            temporal: [0; 5],
            spatial: [0; 5],
            fcoeff: [0; 5],
            i: 0,
            pdtype: PD_TYPES_UNKNOWN,
            step: -1,
        }
    }
}

fn clip(value: i32, low: i32, high: i32) -> i32 {
    value.clamp(low, high)
}

/// `iep2_check_ffo`: frame-vs-field preference and field order from the previous
/// job's counters. Mirrors the two early returns and the score clipping.
pub fn check_field_order(
    stats: &Iep2DectStats,
    ff: &mut FieldOrderState,
) -> Option<Iep2FieldOrder> {
    let tcnt = stats.pd_tcnt as i32;
    let bcnt = stats.pd_bcnt as i32;
    let tdiff = stats.gradt_tcnt + 1;
    let bdiff = stats.gradt_bcnt + 1;
    let ff00t = (stats.ff_cur_tcnt << 5) / tdiff;
    let ff00b = (stats.ff_cur_bcnt << 5) / bdiff;
    let ff11t = (stats.ff_nxt_tcnt << 5) / tdiff;
    let ff11b = (stats.ff_nxt_bcnt << 5) / bdiff;
    let ff0t1b = (stats.ff_ble_tcnt << 5) / bdiff;
    let ff0b1t = (stats.ff_ble_bcnt << 5) / bdiff;

    let ff00 = ff00t.min(ff00b);
    let ff11 = ff11t.min(ff11b);
    let ffx = ff0t1b.min(ff0b1t);
    let ffi = ff00.max(ff11);
    let mut thr = ffx / 10;

    if ff00t > 120 || ff00b > 120 {
        return None;
    }
    if ffi <= 3 && ffx <= 3 {
        return None;
    }

    let coef = 2;
    if ffi * coef <= ffx && (ffx - ffi * coef) > 2 * ffx / 10 {
        ff.frm_score = clip(ff.frm_score + 1, 0, 20);
        ff.fie_score = clip(ff.fie_score - 1, 0, 20);
    } else {
        ff.frm_score = clip(ff.frm_score - 1, 0, 20);
        ff.fie_score = clip(ff.fie_score + 1, 0, 20);
    }

    let frm_score = ff.frm_score + ff.frm_offset;
    let fie_score = ff.fie_score + ff.fie_offset;
    if (frm_score - fie_score).abs() > 10 {
        ff.is_frm = i32::from(frm_score > fie_score);
    }

    if tcnt <= 3 && bcnt <= 3 {
        return None;
    }

    thr = if thr == 0 { 1 } else { thr };
    let ratio = (ff0t1b as i64 - ff0b1t as i64).unsigned_abs() as u32 / thr * 10;

    let idx = ff.fo_ratio_idx as usize % FIELD_ORDER_RATIO_SIZE;
    ff.fo_ratio_sum = ff.fo_ratio_sum + ratio - ff.fo_ratio[idx];
    ff.fo_ratio[idx] = ratio;
    ff.fo_ratio_idx = ((idx + 1) % FIELD_ORDER_RATIO_SIZE) as u32;
    ff.fo_ratio_cnt = (ff.fo_ratio_cnt + 1).min(FIELD_ORDER_RATIO_SIZE as u32);
    ff.fo_ratio_avg = ff.fo_ratio_sum / ff.fo_ratio_cnt.max(1);

    if ratio > 10 {
        if ff0t1b > ff0b1t {
            ff.tff_score = clip(ff.tff_score + 1, 0, 10);
            ff.bff_score = clip(ff.bff_score - 1, 0, 10);
        } else {
            ff.tff_score = clip(ff.tff_score - 1, 0, 10);
            ff.bff_score = clip(ff.bff_score + 1, 0, 10);
        }
    }

    let tff = ff.tff_score + ff.tff_offset;
    let bff = ff.bff_score + ff.bff_offset;
    ff.fo_detected = 1;
    if (tff - bff).abs() > 5 {
        if tff > bff {
            return Some(Iep2FieldOrder::Tff);
        }
        return Some(Iep2FieldOrder::Bff);
    }
    None
}

/// `iep2_check_pd`: fold the counters into the five-frame window, drop a
/// confirmed type that no longer matches, and look for a new one.
pub fn check_pulldown(stats: &Iep2DectStats, pd: &mut PulldownState) {
    let tcnt = stats.pd_tcnt as i32;
    let bcnt = stats.pd_bcnt as i32;
    let tdiff = stats.gradt_tcnt + 1;
    let bdiff = stats.gradt_bcnt + 1;
    let idx = (pd.i % PD_FRAME_PERIOD) as usize;
    let ff00t = (stats.ff_cur_tcnt << 5) / tdiff;
    let ff00b = (stats.ff_cur_bcnt << 5) / bdiff;
    let nz = stats.ff_nz + 1;
    let f = stats.ff_comb_f;

    pd.spatial[idx] = ff00t.min(ff00b) as i32;
    pd.temporal[idx] =
        i32::from(tcnt < FIELD_DIFF_SUM_THR) | (i32::from(bcnt < FIELD_DIFF_SUM_THR) << 1);
    pd.fcoeff[idx] = (f as i32) * 100 / nz as i32;

    if pd.pdtype != PD_TYPES_UNKNOWN && pd.step != -1 {
        let row = pd.pdtype as usize;
        let expected = PD_TABLE[row][((pd.step + 1) % PD_FRAME_PERIOD) as usize];
        if (expected == PD_TS && !(tcnt < FIELD_DIFF_SUM_THR))
            || (expected == PD_BS && !(bcnt < FIELD_DIFF_SUM_THR))
        {
            pd.pdtype = PD_TYPES_UNKNOWN;
            pd.step = -1;
        }
    }

    pd.step = if pd.step != -1 {
        (pd.step + 1) % PD_FRAME_PERIOD
    } else {
        -1
    };

    if pd.pdtype != PD_TYPES_UNKNOWN {
        pd.i += 1;
        return;
    }

    for row in 0..PD_TABLE.len() {
        let matches = (0..PD_FRAME_PERIOD as usize)
            .all(|k| pd.temporal[(idx + k) % PD_FRAME_PERIOD as usize] == PD_TABLE[row][k]);
        if !matches {
            continue;
        }
        if row as i32 != PD_TYPES_UNKNOWN {
            let mut vmax = i32::MAX;
            let mut fmax = i32::MAX;
            let mut vmin = 0;
            let mut fmin = 0;
            for j in 0..PD_FRAME_PERIOD as usize {
                if SP_TABLE[row][j] == 1 {
                    vmax = vmax.min(pd.spatial[j]);
                } else {
                    vmin = vmin.max(pd.spatial[j]);
                }
                if FP_TABLE[row][j] == 1 {
                    fmax = fmax.min(pd.fcoeff[(idx + j) % PD_FRAME_PERIOD as usize]);
                } else {
                    fmin = fmin.max(pd.fcoeff[(idx + j) % PD_FRAME_PERIOD as usize]);
                }
            }
            if vmax > vmin || fmax > fmin {
                pd.pdtype = if row as i32 == PD_TYPES_3_2_2_3
                    && pd.spatial[1] > pd.spatial[0].max(pd.spatial[4])
                {
                    PD_TYPES_3_2_3_2
                } else {
                    row as i32
                };
                pd.step = 0;
            }
        }
        break;
    }

    pd.i += 1;
}

/// `iep2_pd_get_output`: the flag `iep2.c` writes to `pd_mode` for this frame.
/// `None` is the reference's `-1` (unsupported telecine type).
pub fn pulldown_mode(pd: &PulldownState) -> Option<i32> {
    let step = (pd.step + 1) % PD_FRAME_PERIOD;
    match pd.pdtype {
        PD_TYPES_3_2_3_2 => match step {
            1 => Some(PD_COMP_FLAG_NC),
            2 => Some(PD_COMP_FLAG_NON),
            _ => Some(PD_COMP_FLAG_CC),
        },
        PD_TYPES_2_3_2_3 => match step {
            2 => Some(PD_COMP_FLAG_CN),
            3 => Some(PD_COMP_FLAG_NON),
            _ => Some(PD_COMP_FLAG_CC),
        },
        PD_TYPES_2_3_3_2 => match step {
            2 => Some(PD_COMP_FLAG_NON),
            _ => Some(PD_COMP_FLAG_CC),
        },
        PD_TYPES_3_2_2_3 => match step {
            1 | 2 => Some(PD_COMP_FLAG_CN),
            3 => Some(PD_COMP_FLAG_NON),
            _ => Some(PD_COMP_FLAG_CC),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `temporal` is `(tcnt < 32) | (bcnt < 32) << 1`.
    fn frame(temporal: i32, spatial: i32, fcoeff: i32) -> Iep2DectStats {
        Iep2DectStats {
            pd_tcnt: if temporal & 1 != 0 { 0 } else { 5000 },
            pd_bcnt: if temporal & 2 != 0 { 0 } else { 5000 },
            ff_cur_tcnt: spatial as u32,
            ff_cur_bcnt: spatial as u32,
            ff_comb_f: fcoeff.max(0) as u32,
            ff_nz: 99,
            gradt_tcnt: 31,
            gradt_bcnt: 31,
            ..Iep2DectStats::default()
        }
    }

    #[test]
    fn three_two_pulldown_is_confirmed_after_one_period() {
        // The order the board produced for ffmpeg's `telecine` at 720x480:
        // temporal [1, 0, 2, 0, 0] repeating, one high-spatial frame. The window
        // only lines up once it wraps back to index 0, so confirmation lands on
        // the sixth check - the trace's frame 5.
        let seen = [(1, 12), (0, 63), (2, 114), (0, 12), (0, 12), (1, 11)];
        let mut pd = PulldownState::default();
        for (temporal, spatial) in seen {
            check_pulldown(&frame(temporal, spatial, 50), &mut pd);
        }
        assert_eq!(pd.pdtype, PD_TYPES_3_2_3_2);
        assert_eq!(pd.step, 0);
        assert_eq!(pd.temporal, [1, 0, 2, 0, 0]);
        assert_eq!(pd.spatial, [11, 63, 114, 12, 12]);
        assert_eq!(pulldown_mode(&pd), Some(PD_COMP_FLAG_NC));
    }

    #[test]
    fn confirmed_pulldown_cycles_through_the_compose_flags() {
        let seen = [(1, 12), (0, 63), (2, 114), (0, 12), (0, 12), (1, 11)];
        let mut pd = PulldownState::default();
        for (temporal, spatial) in seen {
            check_pulldown(&frame(temporal, spatial, 50), &mut pd);
        }

        let mut flags = Vec::new();
        for (temporal, spatial) in [(0, 69), (2, 113), (0, 13), (0, 12), (1, 12)] {
            check_pulldown(&frame(temporal, spatial, 50), &mut pd);
            flags.push(pulldown_mode(&pd).expect("type stays confirmed"));
        }
        // The board's trace: NON, CC, CC, CC, then NC again.
        assert_eq!(
            flags,
            [
                PD_COMP_FLAG_NON,
                PD_COMP_FLAG_CC,
                PD_COMP_FLAG_CC,
                PD_COMP_FLAG_CC,
                PD_COMP_FLAG_NC
            ]
        );
    }

    #[test]
    fn a_broken_expectation_resets_the_type() {
        let seen = [(1, 12), (0, 63), (2, 114), (0, 12), (0, 12), (1, 11)];
        let mut pd = PulldownState::default();
        for (temporal, spatial) in seen {
            check_pulldown(&frame(temporal, spatial, 50), &mut pd);
        }
        assert_eq!(pd.pdtype, PD_TYPES_3_2_3_2);
        // One more frame advances step 0 -> 1; `3:2:3:2` then expects PD_BS,
        // i.e. a changed bottom field. A frame with `bcnt >= 32` violates it.
        check_pulldown(&frame(0, 69, 50), &mut pd);
        assert_eq!(pd.step, 1);
        assert_eq!(pd.pdtype, PD_TYPES_3_2_3_2);
        check_pulldown(&frame(0, 12, 50), &mut pd);
        assert_eq!(pd.pdtype, PD_TYPES_UNKNOWN);
        assert_eq!(pd.step, -1);
    }

    #[test]
    fn field_order_tracks_the_dominant_field() {
        let mut ff = FieldOrderState::default();
        let mut order = None;
        for _ in 0..8 {
            let stats = Iep2DectStats {
                pd_tcnt: 5000,
                pd_bcnt: 5000,
                ff_cur_tcnt: 20,
                ff_cur_bcnt: 20,
                ff_ble_tcnt: 120,
                ff_ble_bcnt: 40,
                ff_nz: 10,
                ff_comb_f: 1,
                gradt_tcnt: 31,
                gradt_bcnt: 31,
                ..Iep2DectStats::default()
            };
            order = check_field_order(&stats, &mut ff).or(order);
        }
        assert_eq!(order, Some(Iep2FieldOrder::Tff));
        assert!(ff.tff_score > ff.bff_score);

        let mut ff = FieldOrderState::default();
        let mut order = None;
        for _ in 0..8 {
            let stats = Iep2DectStats {
                pd_tcnt: 5000,
                pd_bcnt: 5000,
                ff_cur_tcnt: 20,
                ff_cur_bcnt: 20,
                ff_ble_tcnt: 40,
                ff_ble_bcnt: 120,
                ff_nz: 10,
                ff_comb_f: 1,
                gradt_tcnt: 31,
                gradt_bcnt: 31,
                ..Iep2DectStats::default()
            };
            order = check_field_order(&stats, &mut ff).or(order);
        }
        assert_eq!(order, Some(Iep2FieldOrder::Bff));
    }

    #[test]
    fn stats_are_read_from_the_documented_words() {
        let mut words = [0u32; OUTPUT_WORDS];
        words[OUT_DECT_PD_TCNT] = 7;
        words[OUT_DECT_FF_CUR_TCNT] = 9;
        words[OUT_FF_GRADT_BCNT] = 11;
        let stats = Iep2DectStats::from_words(&words);
        assert_eq!(stats.pd_tcnt, 7);
        assert_eq!(stats.ff_cur_tcnt, 9);
        assert_eq!(stats.gradt_bcnt, 11);
    }
}
