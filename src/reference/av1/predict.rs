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

#[cfg(test)]
mod tests {
    use super::*;

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
