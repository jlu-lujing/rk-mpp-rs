//! `MppDataV2` of `mpp/codec/rc/rc_base.h`, the fixed-length statistic window that
//! `rc_model_v2.c` keeps its bit and QP history in.
//!
//! It is a ring buffer plus a running sum, with three details that are not
//! interchangeable with a generic ring buffer and that the model's arithmetic
//! depends on:
//!
//!   * `preset` and `update` are different operations. `preset` writes ahead of
//!     the read cursor for a value the model has *predicted*, and `update`
//!     replaces those predictions with measured values while any are outstanding
//!     (`pos_ahead`). The pair is how a predicted bit count and the real one share
//!     one window.
//!   * the sum is a 64-bit accumulator, but `sum()` and `mean()` narrow it to 32
//!     bits first, exactly as the C casts `(RK_S32)p->sum` do. On the window
//!     lengths the model uses the narrowing never bites; on a long window with
//!     large values it does, and then it bites identically on both sides.
//!   * `sum_with_ratio` walks the array from index zero rather than from the write
//!     position, and its geometric weights are 64-bit accumulators that overflow
//!     for a long window. Both are recorded, not reproduced as "fixed" behaviour:
//!     see the 64-slot rows of the golden fixture, where the ratio sum runs away.
//!
//! The golden test replays a scripted sequence against
//! [`DataV2`]'s public surface and compares the whole transcript with
//! `tests/data/rc_data_c_probe.txt`, which is the same sequence run against
//! `librockchip_mpp.so.0` built from the source on t6 that the encoder is
//! byte-compared against (`tools/rc_data_probe.c`).

/// The sum rounding `DIV(a, b)` of `rc_base.c` performs: divide with the
/// half-width added back in the sign of the dividend, with `SIGN(0)` being one.
fn div_round(sum: i64, len: i32) -> i32 {
    let sign: i64 = if sum < 0 { -1 } else { 1 };
    // The half is itself computed by a C integer division, so an odd `len`
    // truncates here before the add that follows.
    let half = sign * i64::from(len) / 2;
    ((sum + half) / i64::from(len)) as i32
}

/// One window of `len` samples with the running sum the model reads instead of
/// re-scanning the history.
pub struct DataV2 {
    /// Window length, `size`.
    pub size: i32,
    /// `pos_r`: `size` while every slot has been accounted for, and one lower for
    /// as long as a write has landed on a slot that has not been read back. Every
    /// write starts by testing `pos_r == size`, so this is the "window is full"
    /// flag as much as a position.
    pub pos_r: i32,
    /// `pos_pw`: the slot `preset` will write next.
    pub pos_pw: i32,
    /// `pos_w`: the slot `update` will write next, and the anchor `pre_val` counts
    /// back from.
    pub pos_w: i32,
    /// `pos_ahead`: predictions waiting to be replaced by a measurement.
    pub pos_ahead: i32,
    /// `sum`: the 64-bit accumulator; the accessors narrow it the way C does.
    pub sum: i64,
    val: Vec<i32>,
}

impl DataV2 {
    /// `mpp_data_init_v2`, which refuses a non-positive length and fills the
    /// window with `value`.
    pub fn new(len: i32, value: i32) -> Option<DataV2> {
        if len <= 0 {
            return None;
        }
        let mut out = DataV2 {
            size: len,
            pos_r: 0,
            pos_pw: 0,
            pos_w: 0,
            pos_ahead: 0,
            sum: 0,
            val: vec![0; usize::try_from(len).unwrap()],
        };
        out.reset(value);
        Some(out)
    }

    /// `mpp_data_reset_v2`: every slot becomes `val`, both write positions rewind,
    /// and the sum is the C `int` product `val * size` widened afterwards - so a
    /// large window with a large value wraps before it widens, as it does there.
    pub fn reset(&mut self, val: i32) {
        self.pos_pw = 0;
        self.pos_w = 0;
        self.pos_r = self.size;
        self.sum = val.wrapping_mul(self.size) as i64;
        for slot in &mut self.val {
            *slot = val;
        }
    }

    /// `mpp_data_preset_v2`: write a predicted value at the preset position.
    pub fn preset(&mut self, val: i32) {
        if self.pos_r == self.size {
            self.pos_r -= 1;
            self.sum -= i64::from(self.val[self.pos_pw as usize]);
        }
        self.val[self.pos_pw as usize] = val;
        // Read back after the store, as the C does: the sum gains the value just
        // written, not the one it replaced a line above.
        self.sum += i64::from(self.val[self.pos_pw as usize]);
        self.pos_pw += 1;
        self.pos_r += 1;
        if self.pos_pw >= self.size {
            self.pos_pw = 0;
        }
        self.pos_ahead += 1;
    }

    /// `mpp_data_update_v2`: write a measured value, replacing a pending
    /// prediction first if there is one.
    pub fn update(&mut self, val: i32) {
        if self.pos_ahead > 0 {
            // The subtraction is 32-bit in C, so an extreme pair wraps before the
            // 64-bit accumulator sees it.
            self.sum += val.wrapping_sub(self.val[self.pos_w as usize]) as i64;
            self.val[self.pos_w as usize] = val;
            self.pos_w += 1;
            if self.pos_w >= self.size {
                self.pos_w = 0;
            }
            self.pos_ahead -= 1;
            return;
        }

        if self.pos_r == self.size {
            self.pos_r -= 1;
            self.sum -= i64::from(self.val[self.pos_w as usize]);
        }
        self.val[self.pos_w as usize] = val;
        self.sum += i64::from(val);
        self.pos_w += 1;
        self.pos_r += 1;
        if self.pos_w >= self.size {
            self.pos_w = 0;
        }
    }

    /// `mpp_data_get_pre_val_v2`: the value `idx` writes back from the newest one,
    /// so `idx = 0` is the most recently written slot and `idx = 1` the one before
    /// it. A negative `idx` is not "further back" - it wraps around the array end,
    /// which lands on the *older* half of the window: `idx = -1` is the next slot
    /// `update` will overwrite.
    pub fn pre_val(&self, idx: i32) -> i32 {
        let mut idx = idx;
        if idx < 0 {
            idx += self.size;
        }
        let mut pos = self.pos_w - 1;
        if pos - idx < 0 {
            let pos1 = idx - pos;
            pos = self.size - pos1;
        } else {
            pos -= idx;
        }
        self.val[pos as usize]
    }

    /// `mpp_data_sum_v2`, including the narrowing cast.
    pub fn sum(&self) -> i32 {
        self.sum as i32
    }

    /// `mpp_data_mean_v2`: the narrowed sum divided by the window length, which in
    /// both languages truncates toward zero.
    pub fn mean(&self) -> i32 {
        self.sum() / self.size
    }

    /// `mpp_data_sum_with_ratio_v2`: the first `len` slots, each scaled by
    /// `(num/denom)` raised to its own power, then divided round-to-nearest by
    /// `len`. The weights are accumulated in 64 bits and overflow for a long
    /// window; `num == denom` takes the shortcut branch, which is *not* the same as
    /// the general branch once the weights have wrapped.
    pub fn sum_with_ratio(&self, len: i32, num: i32, denom: i32) -> i32 {
        let len = usize::try_from(len).unwrap();
        let mut sum: i64 = 0;
        if num == denom {
            for slot in &self.val[..len] {
                sum += i64::from(*slot);
            }
        } else {
            let mut acc_num: i64 = 1;
            let mut acc_denom: i64 = 1;
            for slot in self.val.iter().take(len) {
                sum += i64::from(*slot).wrapping_mul(acc_num) / acc_denom;
                acc_num = acc_num.wrapping_mul(i64::from(num));
                acc_denom = acc_denom.wrapping_mul(i64::from(denom));
            }
        }
        div_round(sum, len as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generator `tools/rc_data_probe.c` runs on the board: a plain LCG, so the
    /// same 310-line transcript is reproducible here without reading C source.
    struct Probe {
        seed: u32,
    }

    impl Probe {
        fn next(&mut self, spread: i32) -> i32 {
            self.seed = self.seed.wrapping_mul(1103515245).wrapping_add(12345);
            ((self.seed >> 7) % spread as u32) as i32
        }
    }

    /// One line in the shape the probe prints, so the comparison is with the C
    /// library's own observable state and not with a paraphrase of it.
    fn dump(d: &DataV2, op: &str, v: i32) -> String {
        format!(
            "{} {} {} sum64={} sum={} mean={} pre1={} pre2={} swr={},{},{} \
pos_w={} pos_r={} pos_pw={} pos_ahead={}",
            d.size,
            op,
            v,
            d.sum,
            d.sum(),
            d.mean(),
            d.pre_val(-1),
            d.pre_val(-2),
            d.sum_with_ratio(d.size, 3, 2),
            d.sum_with_ratio(d.size, 2, 2),
            d.sum_with_ratio(1, 5, 4),
            d.pos_w,
            d.pos_r,
            d.pos_pw,
            d.pos_ahead,
        )
    }

    fn transcript() -> String {
        let mut out = String::new();
        // One generator for the whole file, as in the probe: the seed is a file
        // scope variable there, so window lengths share one value stream.
        let mut probe = Probe { seed: 20240425 };
        for size in [2, 5, 8, 16, 64] {
            let mut d = DataV2::new(size, 0).unwrap();
            out.push_str(&format!("# size={}\n", size));
            out.push_str(&dump(&d, "init", 0));
            out.push('\n');
            for step in 0..size * 3 {
                let v = probe.next(4000) - 200;
                match step % 3 {
                    0 => {
                        d.preset(v);
                        out.push_str(&dump(&d, "preset", v));
                    }
                    1 => {
                        d.update(v);
                        out.push_str(&dump(&d, "update", v));
                    }
                    _ => {
                        d.update(v * 2);
                        out.push_str(&dump(&d, "update2", v * 2));
                    }
                }
                out.push('\n');
            }
            d.reset(7);
            out.push_str(&dump(&d, "reset", 7));
            out.push('\n');
            d.reset(-3);
            out.push_str(&dump(&d, "reset_neg", -3));
            out.push('\n');
            let v = probe.next(1000);
            d.update(v);
            out.push_str(&dump(&d, "after_reset", 0));
            out.push('\n');
        }
        out
    }

    /// The window is the foundation of the whole rate-control port, so it is
    /// checked against the library the encoder is byte-compared against, field by
    /// field and line by line: `tests/data/rc_data_c_probe.txt` is this same
    /// sequence run against `librockchip_mpp.so.0` from the reference source.
    #[test]
    fn the_window_matches_the_reference_library_line_for_line() {
        let want = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/rc_data_c_probe.txt"
        ))
        .expect("the C probe transcript");
        let got = transcript();
        let want: Vec<&str> = want.trim_end().lines().collect();
        let got: Vec<&str> = got.trim_end().lines().collect();
        assert_eq!(want.len(), got.len(), "transcript length");
        for (i, (w, g)) in want.iter().zip(got.iter()).enumerate() {
            assert_eq!(w, g, "line {} of the transcript", i + 1);
        }
    }

    #[test]
    fn a_prediction_is_replaced_before_it_reaches_the_end_of_the_window() {
        // The reason `preset` and `update` are two calls: the model predicts a
        // picture's bit count, and the measured count has to land in the same slot.
        let mut d = DataV2::new(2, 0).unwrap();
        d.preset(500);
        assert_eq!(
            d.sum(),
            500,
            "the prediction is counted while it is pending"
        );
        assert_eq!(d.pos_ahead, 1);
        d.update(200);
        assert_eq!(
            d.sum(),
            200,
            "the measurement replaces it rather than adding"
        );
        assert_eq!(d.pos_ahead, 0);
        assert_eq!(d.pre_val(-1), 0, "the untouched slot is still the oldest");
    }

    #[test]
    fn the_sum_narrows_to_thirty_two_bits_like_the_c_cast() {
        let mut d = DataV2::new(64, 0).unwrap();
        for _ in 0..64 {
            d.update(i32::MAX);
        }
        assert!(d.sum > i64::from(i32::MAX), "the accumulator is 64-bit");
        assert_eq!(
            d.sum(),
            (d.sum as i128 as u32 as i32),
            "sum() reports the wrapped low half, as (RK_S32)p->sum does"
        );
    }

    #[test]
    fn reset_multiplies_in_thirty_two_bits_before_widening() {
        // `p->sum = val * p->size` is an `int` product in C: the window length is
        // applied first, so an extreme fill value wraps on the way in.
        let mut d = DataV2::new(8, 0).unwrap();
        let fill = i32::MAX / 3;
        d.reset(fill);
        assert_eq!(d.sum, fill.wrapping_mul(8) as i64);
        assert_ne!(
            d.sum,
            i64::from(fill) * 8,
            "the widening comes too late to help"
        );
    }

    #[test]
    fn equal_ratio_and_plain_sum_agree_only_while_the_weights_hold() {
        let mut d = DataV2::new(4, 0).unwrap();
        for v in [10, 20, 30, 40] {
            d.update(v);
        }
        // With num == denom the shortcut branch adds the raw values; with 4/4
        // through the general branch the weights are 1,1,1,1 as well.
        assert_eq!(d.sum_with_ratio(4, 2, 2), d.sum_with_ratio(4, 4, 4));
        // Half again per step, rounded once at the end by the divide.
        assert_eq!(
            d.sum_with_ratio(4, 1, 2),
            (10 + 20 / 2 + 30 / 4 + 40 / 8 + 2) / 4
        );
    }

    #[test]
    fn a_window_has_to_have_slots() {
        assert!(DataV2::new(0, 0).is_none());
        assert!(DataV2::new(-4, 1).is_none());
    }
}
