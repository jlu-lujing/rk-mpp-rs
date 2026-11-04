//! AV1 intra prediction: the DC, vertical and horizontal processes.
//!
//! DC prediction (specification 7.11.2.5) averages whichever edges are
//! available: both, left only, above only, or a `1 << (BitDepth - 1)` fill when
//! neither exists. Vertical and horizontal copy the above row and left column
//! respectively; the directional, smooth, Paeth and CFL processes follow.

/// Clip a value to the sample range.
fn clip1(value: i32, bit_depth: u32) -> u16 {
    value.clamp(0, (1 << bit_depth) - 1) as u16
}

/// `DC_PRED`: one average value for the whole `w` x `h` block.
pub fn dc_predict(
    above: &[u16],
    left: &[u16],
    have_above: bool,
    have_left: bool,
    log2_w: u32,
    log2_h: u32,
    bit_depth: u32,
) -> Vec<u16> {
    let w = 1usize << log2_w;
    let h = 1usize << log2_h;
    let value = if have_above && have_left {
        let mut sum = 0u32;
        for k in 0..h {
            sum += u32::from(left[k]);
        }
        for k in 0..w {
            sum += u32::from(above[k]);
        }
        sum += ((w + h) >> 1) as u32;
        clip1((sum / (w + h) as u32) as i32, bit_depth)
    } else if have_left {
        let sum: u32 = left[..h].iter().map(|v| u32::from(*v)).sum();
        clip1(((sum + (h as u32 >> 1)) >> log2_h) as i32, bit_depth)
    } else if have_above {
        let sum: u32 = above[..w].iter().map(|v| u32::from(*v)).sum();
        clip1(((sum + (w as u32 >> 1)) >> log2_w) as i32, bit_depth)
    } else {
        (1 << (bit_depth - 1)) as u16
    };
    vec![value; w * h]
}

/// `V_PRED`: each row repeats the above row.
pub fn v_predict(above: &[u16], log2_w: u32, log2_h: u32) -> Vec<u16> {
    let w = 1usize << log2_w;
    let h = 1usize << log2_h;
    let mut out = Vec::with_capacity(w * h);
    for _ in 0..h {
        out.extend_from_slice(&above[..w]);
    }
    out
}

/// `H_PRED`: each column repeats the left column.
pub fn h_predict(left: &[u16], log2_w: u32, log2_h: u32) -> Vec<u16> {
    let w = 1usize << log2_w;
    let h = 1usize << log2_h;
    let mut out = vec![0u16; w * h];
    for row in 0..h {
        for col in 0..w {
            out[row * w + col] = left[row];
        }
    }
    out
}

/// `Sm_Weights_Tx_*` for a transform dimension.
pub fn smooth_weights(log2: u32) -> &'static [u8] {
    use crate::reference::av1::sm_weights::{
        SM_WEIGHTS_TX_16X16, SM_WEIGHTS_TX_32X32, SM_WEIGHTS_TX_4X4, SM_WEIGHTS_TX_64X64,
        SM_WEIGHTS_TX_8X8,
    };
    match log2 {
        2 => &SM_WEIGHTS_TX_4X4,
        3 => &SM_WEIGHTS_TX_8X8,
        4 => &SM_WEIGHTS_TX_16X16,
        5 => &SM_WEIGHTS_TX_32X32,
        _ => &SM_WEIGHTS_TX_64X64,
    }
}

/// `Round2(value, n)`.
fn round2(value: i32, n: u32) -> u16 {
    ((value + (1 << (n - 1))) >> n) as u16
}

/// The three smooth modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmoothMode {
    Smooth,
    SmoothV,
    SmoothH,
}

/// `SMOOTH_PRED`, `SMOOTH_V_PRED` and `SMOOTH_H_PRED`.
pub fn smooth_predict(
    mode: SmoothMode,
    above: &[u16],
    left: &[u16],
    log2_w: u32,
    log2_h: u32,
) -> Vec<u16> {
    let w = 1usize << log2_w;
    let h = 1usize << log2_h;
    let weights_x = smooth_weights(log2_w);
    let weights_y = smooth_weights(log2_h);
    let mut out = vec![0u16; w * h];
    for i in 0..h {
        for j in 0..w {
            let value = match mode {
                SmoothMode::Smooth => {
                    let wy = i32::from(weights_y[i]);
                    let wx = i32::from(weights_x[j]);
                    wy * i32::from(above[j])
                        + (256 - wy) * i32::from(left[h - 1])
                        + wx * i32::from(left[i])
                        + (256 - wx) * i32::from(above[w - 1])
                }
                SmoothMode::SmoothV => {
                    let wy = i32::from(weights_y[i]);
                    wy * i32::from(above[j]) + (256 - wy) * i32::from(left[h - 1])
                }
                SmoothMode::SmoothH => {
                    let wx = i32::from(weights_x[j]);
                    wx * i32::from(left[i]) + (256 - wx) * i32::from(above[w - 1])
                }
            };
            out[i * w + j] = round2(value, if mode == SmoothMode::Smooth { 9 } else { 8 });
        }
    }
    out
}

/// `PAETH_PRED` (basic intra prediction process).
pub fn paeth_predict(
    above: &[u16],
    left: &[u16],
    top_left: u16,
    log2_w: u32,
    log2_h: u32,
) -> Vec<u16> {
    let w = 1usize << log2_w;
    let h = 1usize << log2_h;
    let mut out = vec![0u16; w * h];
    for i in 0..h {
        for j in 0..w {
            let base = i32::from(above[j]) + i32::from(left[i]) - i32::from(top_left);
            let p_left = (base - i32::from(left[i])).abs();
            let p_top = (base - i32::from(above[j])).abs();
            let p_top_left = (base - i32::from(top_left)).abs();
            out[i * w + j] = if p_left <= p_top && p_left <= p_top_left {
                left[i]
            } else if p_top <= p_top_left {
                above[j]
            } else {
                top_left
            };
        }
    }
    out
}

/// `Dr_Intra_Derivative[ 90 ]`.
pub static DR_INTRA_DERIVATIVE: [u16; 90] = [
    0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0, 0,
    132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0, 51, 0, 0,
    45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0, 0, 0, 0, 11, 0,
    0, 7, 0, 0, 3, 0, 0,
];

/// The derivative for an angle in `3..=87` (multiples of three are unused and
/// zero, as the specification's table stores them).
pub fn dr_intra_derivative(angle: u32) -> u16 {
    DR_INTRA_DERIVATIVE[angle as usize]
}

/// Directional prediction for `pAngle < 90` (the unfiltered, non-upsampled
/// path of the specification's directional process). `above` must supply at
/// least `w + h` samples.
pub fn directional_lt90(above: &[u16], dx: u16, log2_w: u32, log2_h: u32) -> Vec<u16> {
    let w = 1usize << log2_w;
    let h = 1usize << log2_h;
    let max_base = w + h - 1;
    let mut out = vec![0u16; w * h];
    for i in 0..h {
        for j in 0..w {
            let idx = (i + 1) as u32 * u32::from(dx);
            let base = (idx >> 6) as usize + j;
            let shift = (idx >> 1) & 0x1F;
            out[i * w + j] = if base < max_base {
                let value =
                    u32::from(above[base]) * (32 - shift) + u32::from(above[base + 1]) * shift;
                ((value + 16) >> 5) as u16
            } else {
                above[max_base]
            };
        }
    }
    out
}

/// Directional prediction for `90 < pAngle < 180` (the unfiltered,
/// non-upsampled path). `above` and `left` include the top-left sample at
/// index 0, so `above[base + 1]` is the specification's `AboveRow[base]`.
pub fn directional_gte90_lt180(
    above: &[u16],
    left: &[u16],
    dx: u16,
    dy: u16,
    log2_w: u32,
    log2_h: u32,
) -> Vec<u16> {
    let w = 1usize << log2_w;
    let h = 1usize << log2_h;
    let mut out = vec![0u16; w * h];
    for i in 0..h {
        for j in 0..w {
            let idx = (j as i32) * 64 - (i as i32 + 1) * i32::from(dx);
            let base = idx >> 6;
            if base >= -1 {
                let shift = ((idx as u32) << 1 & 0x3F) >> 1 & 0x1F;
                let at = (base + 1) as usize;
                let value = u32::from(above[at]) * (32 - shift) + u32::from(above[at + 1]) * shift;
                out[i * w + j] = ((value + 16) >> 5) as u16;
            } else {
                let idx = (i as i32) * 64 - (j as i32 + 1) * i32::from(dy);
                let base = idx >> 6;
                let shift = ((idx as u32) >> 1) & 0x1F;
                // Index 0 of `left` is the top-left sample, i.e. LeftCol[-1].
                let at = (base + 1) as usize;
                let value = u32::from(left[at]) * (32 - shift) + u32::from(left[at + 1]) * shift;
                out[i * w + j] = ((value + 16) >> 5) as u16;
            }
        }
    }
    out
}

/// Directional prediction for `pAngle > 180` (unfiltered, non-upsampled).
pub fn directional_gt180(left: &[u16], dy: u16, log2_w: u32, log2_h: u32) -> Vec<u16> {
    let w = 1usize << log2_w;
    let h = 1usize << log2_h;
    let max_base = w + h - 1;
    let mut out = vec![0u16; w * h];
    for i in 0..h {
        for j in 0..w {
            let idx = (j + 1) as u32 * u32::from(dy);
            let base = (idx >> 6) as usize + i;
            let shift = (idx >> 1) & 0x1F;
            out[i * w + j] = if base < max_base {
                let value =
                    u32::from(left[base]) * (32 - shift) + u32::from(left[base + 1]) * shift;
                ((value + 16) >> 5) as u16
            } else {
                left[max_base]
            };
        }
    }
    out
}

/// Intra edge filter strength (specification 7.11.2.8).
pub fn intra_edge_filter_strength(w: u32, h: u32, filter_type: u32, delta: i32) -> u8 {
    let d = delta.unsigned_abs();
    let blk = w + h;
    let mut strength = 0;
    if filter_type == 0 {
        if blk <= 8 {
            if d >= 56 {
                strength = 1;
            }
        } else if blk <= 16 {
            if d >= 40 {
                strength = 1;
            }
        } else if blk <= 24 {
            if d >= 8 {
                strength = 1;
            }
            if d >= 16 {
                strength = 2;
            }
            if d >= 32 {
                strength = 3;
            }
        } else if blk <= 32 {
            strength = 1;
            if d >= 4 {
                strength = 2;
            }
            if d >= 32 {
                strength = 3;
            }
        } else {
            strength = 3;
        }
    } else if blk <= 8 {
        if d >= 40 {
            strength = 1;
        }
        if d >= 64 {
            strength = 2;
        }
    } else if blk <= 16 {
        if d >= 20 {
            strength = 1;
        }
        if d >= 48 {
            strength = 2;
        }
    } else if blk <= 24 {
        if d >= 4 {
            strength = 3;
        }
    } else {
        strength = 3;
    }
    strength
}

/// Intra edge upsample selection (specification 7.11.2.9).
pub fn intra_edge_upsample_selection(w: u32, h: u32, filter_type: u32, delta: i32) -> bool {
    let d = delta.unsigned_abs();
    let blk = w + h;
    if d == 0 || d >= 40 {
        false
    } else if filter_type == 0 {
        blk <= 16
    } else {
        blk <= 8
    }
}

/// Intra edge upsample process (specification 7.11.2.10).
///
/// `buf` holds entries `-1..num_px-1` with index 0 representing `-1`; the
/// output holds `-2..2*num_px-2` with index 0 representing `-2`.
pub fn upsample_edge(buf: &[u16], num_px: usize, bit_depth: u32) -> Vec<u16> {
    // dup = [buf[-1], buf[-1], buf[0], .., buf[numPx-1], buf[numPx-1]].
    let mut dup = Vec::with_capacity(num_px + 3);
    dup.push(buf[0]); // buf[-1]
    dup.push(buf[0]); // the loop's i = -1 entry
    dup.extend_from_slice(&buf[1..=num_px]); // buf[0] .. buf[numPx-1]
    dup.push(buf[num_px]); // buf[numPx-1]
    let mut out = vec![0u16; 2 * num_px + 1];
    out[0] = dup[0];
    for i in 0..num_px {
        let s = -i32::from(dup[i]) + 9 * i32::from(dup[i + 1]) + 9 * i32::from(dup[i + 2])
            - i32::from(dup[i + 3]);
        let s = clip1((s + 8) >> 4, bit_depth);
        out[2 * i + 1] = s;
        out[2 * i + 2] = dup[i + 2];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directional_135_mixes_above_and_left() {
        // Dr[45] = 64 for both axes at 135 degrees; index 0 of each edge slice
        // is the top-left sample.
        let above = [50u16, 60, 70, 80, 90];
        let left = [50u16, 61, 62, 63, 64];
        let dx = dr_intra_derivative(45);
        let dy = dr_intra_derivative(45);
        assert_eq!((dx, dy), (64, 64));
        let pred = directional_gte90_lt180(&above, &left, dx, dy, 1, 1);
        assert_eq!(pred, [50, 60, 61, 50]);
    }

    #[test]
    fn directional_203_stays_on_the_left_edge() {
        // Dr[67] = 27 for both samples of a 2-wide block.
        let left = [50u16, 60, 70, 80, 90];
        let dy = dr_intra_derivative(67);
        assert_eq!(dy, 27);
        let pred = directional_gt180(&left, dy, 1, 1);
        let blend = |a: u16, b: u16, shift: u32| {
            ((u32::from(a) * (32 - shift) + u32::from(b) * shift + 16) >> 5) as u16
        };
        assert_eq!(pred[0], blend(50, 60, 13));
        assert_eq!(pred[1], blend(50, 60, 27));
        assert_eq!(pred[2], blend(60, 70, 13));
        assert_eq!(pred[3], blend(60, 70, 27));
    }

    #[test]
    fn edge_strength_follows_the_dimension_and_delta_branches() {
        assert_eq!(intra_edge_filter_strength(4, 4, 0, 56), 1);
        assert_eq!(intra_edge_filter_strength(16, 16, 0, 3), 1);
        assert_eq!(intra_edge_filter_strength(16, 16, 0, 4), 2);
        assert_eq!(intra_edge_filter_strength(16, 16, 0, 32), 3);
        assert_eq!(intra_edge_filter_strength(20, 20, 0, 0), 3);
        assert_eq!(intra_edge_filter_strength(4, 4, 1, 64), 2);
        assert_eq!(intra_edge_filter_strength(8, 8, 1, 48), 2);
        assert_eq!(intra_edge_filter_strength(12, 12, 1, 4), 3);
    }

    #[test]
    fn upsample_selection_uses_delta_and_size() {
        assert!(!intra_edge_upsample_selection(4, 4, 0, 0));
        assert!(intra_edge_upsample_selection(8, 8, 0, 39));
        assert!(!intra_edge_upsample_selection(12, 12, 0, 39));
        assert!(intra_edge_upsample_selection(4, 4, 1, 20));
        assert!(!intra_edge_upsample_selection(8, 8, 1, 20));
        assert!(!intra_edge_upsample_selection(4, 4, 0, 40));
    }

    #[test]
    fn upsample_doubles_the_edge_with_the_4_tap_kernel() {
        // Entries -1, 0, 1.
        let buf = [10u16, 20, 30];
        assert_eq!(upsample_edge(&buf, 2, 8), [10, 14, 20, 26, 30]);
    }

    #[test]
    fn directional_45_follows_the_diagonal() {
        // Dr_Intra_Derivative[45] = 64: idx = (i+1)*64, so base = i+j+1 and
        // the shift is zero.
        let above = [10u16, 20, 30, 40, 50, 60];
        assert_eq!(dr_intra_derivative(45), 64);
        let pred = directional_lt90(&above, 64, 1, 1);
        assert_eq!(pred, [20, 30, 30, 40]);
    }

    #[test]
    fn directional_67_uses_its_derivative() {
        assert_eq!(dr_intra_derivative(67), 27);
        let above = [0u16, 64, 128, 192, 256, 320];
        let pred = directional_lt90(&above, 27, 1, 1);
        // Row 1 uses idx 54: base 0+j and shift 27 -> blend of above[0..2].
        let shift = 27u32;
        assert_eq!(pred[2], ((0 * (32 - shift) + 64 * shift + 16) >> 5) as u16);
        assert_eq!(
            pred[3],
            ((64 * (32 - shift) + 128 * shift + 16) >> 5) as u16
        );
    }

    #[test]
    fn smooth_modes_blend_the_edge_midpoints() {
        let above = [128u16; 4];
        let left = [128u16; 4];
        for mode in [SmoothMode::Smooth, SmoothMode::SmoothV, SmoothMode::SmoothH] {
            assert_eq!(smooth_predict(mode, &above, &left, 2, 2), [128; 16]);
        }
        // SMOOTH_V with a 200/100 split: the first row keeps the above value,
        // the last row's weight 64 rounds to 125.
        let above = [200u16; 4];
        let left = [100u16; 4];
        let pred = smooth_predict(SmoothMode::SmoothV, &above, &left, 2, 2);
        assert_eq!(pred[0], 200);
        assert_eq!(pred[12], 125);
    }

    #[test]
    fn paeth_picks_the_closest_edge() {
        // The prediction is `above + left - top_left`; each candidate wins when
        // its distance to that base is the smallest.
        assert_eq!(paeth_predict(&[100], &[50], 50, 0, 0), [100]);
        // above == top_left makes the left sample the closest.
        assert_eq!(paeth_predict(&[10], &[100], 10, 0, 0), [100]);
        assert_eq!(paeth_predict(&[100], &[90], 95, 0, 0), [95]);
    }

    #[test]
    fn dc_uses_both_edges_when_available() {
        // sum = 10 + 20 + 30 + 40 = 100, plus (w + h) >> 1 = 2, over 4.
        let pred = dc_predict(&[10, 20], &[30, 40], true, true, 1, 1, 8);
        assert_eq!(pred, [25; 4]);
    }

    #[test]
    fn dc_uses_the_available_edge_only() {
        assert_eq!(
            dc_predict(&[], &[10, 20, 30, 40], false, true, 2, 2, 8),
            [25; 16]
        );
        // above only: (10 + 20 + 1) >> 1 = 15.
        assert_eq!(dc_predict(&[10, 20], &[], true, false, 1, 1, 8), [15; 4]);
    }

    #[test]
    fn dc_falls_back_to_mid_gray() {
        assert_eq!(dc_predict(&[], &[], false, false, 1, 1, 8), [128; 4]);
        assert_eq!(dc_predict(&[], &[], false, false, 1, 1, 10), [512; 4]);
    }

    #[test]
    fn vertical_and_horizontal_copy_their_edges() {
        assert_eq!(v_predict(&[1, 2, 3, 4], 2, 1), [1, 2, 3, 4, 1, 2, 3, 4]);
        assert_eq!(h_predict(&[7, 8], 2, 1), [7, 7, 7, 7, 8, 8, 8, 8]);
    }
}
