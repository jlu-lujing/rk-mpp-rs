//! AV1 intra prediction: the DC, vertical and horizontal processes.
//!
//! DC prediction (specification 7.11.2.5) averages whichever edges are
//! available: both, left only, above only, or a `1 << (BitDepth - 1)` fill when
//! neither exists. Vertical and horizontal copy the above row and left column
//! respectively; the directional, smooth, Paeth and CFL processes follow.

use crate::reference::av1::intra::IntraMode;

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

/// `Intra_Edge_Kernel[ INTRA_EDGE_KERNELS ][ INTRA_EDGE_TAPS ]`.
pub static INTRA_EDGE_KERNEL: [[i32; 5]; 3] = [[0, 4, 8, 4, 0], [0, 5, 6, 5, 0], [2, 4, 4, 4, 2]];

/// Intra edge filter process (specification 7.11.2.11).
///
/// `edge` holds `buf[-1] .. buf[sz-2]`; the return value holds the filtered
/// `buf[0] .. buf[sz-2]` (the `buf[-1]` sample is never rewritten).
pub fn intra_edge_filter(edge: &[u16], sz: usize, strength: u8, bit_depth: u32) -> Vec<u16> {
    let mut out = vec![0u16; sz - 1];
    if strength == 0 {
        out.copy_from_slice(&edge[1..sz]);
        return out;
    }
    let kernel = &INTRA_EDGE_KERNEL[(strength - 1) as usize];
    for i in 1..sz {
        let mut s = 0i32;
        for (j, weight) in kernel.iter().enumerate() {
            let k = (i as i32 - 2 + j as i32).clamp(0, sz as i32 - 1) as usize;
            s += weight * i32::from(edge[k]);
        }
        out[i - 1] = clip1((s + 8) >> 4, bit_depth);
    }
    out
}

/// `Round2Signed(value, n)`.
fn round2_signed(value: i32, n: u32) -> i32 {
    if value < 0 {
        -(((-value) + (1 << (n - 1))) >> n)
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// Chroma-from-luma prediction (specification 7.11.5).
///
/// `luma` is the reconstructed luma plane with `luma_stride`; `dc` is the
/// already DC-predicted chroma block in row-major order. `sub_x`/`sub_y` are
/// the chroma subsampling shifts (both 1 for 4:2:0).
#[allow(clippy::too_many_arguments)]
pub fn cfl_predict(
    luma: &[u16],
    luma_stride: usize,
    start_x: usize,
    start_y: usize,
    w: usize,
    h: usize,
    sub_x: u32,
    sub_y: u32,
    max_luma_w: usize,
    max_luma_h: usize,
    alpha: i32,
    dc: &[u16],
    bit_depth: u32,
) -> Vec<u16> {
    let log2_w = (w as u32).trailing_zeros();
    let log2_h = (h as u32).trailing_zeros();
    let mut samples = vec![0u32; w * h];
    let mut luma_avg = 0u32;
    for i in 0..h {
        for j in 0..w {
            let luma_y = (((start_y + i) << sub_y).min(max_luma_h - (1 << sub_y))) as usize;
            let luma_x = (((start_x + j) << sub_x).min(max_luma_w - (1 << sub_x))) as usize;
            let mut t = 0u32;
            for dy in 0..=sub_y as usize {
                for dx in 0..=sub_x as usize {
                    t += u32::from(luma[(luma_y + dy) * luma_stride + luma_x + dx]);
                }
            }
            let v = t << (3 - sub_x - sub_y);
            samples[i * w + j] = v;
            luma_avg += v;
        }
    }
    luma_avg = (luma_avg + (1 << (log2_w + log2_h - 1))) >> (log2_w + log2_h);
    let mut out = vec![0u16; w * h];
    for i in 0..w * h {
        let scaled = round2_signed(alpha * (samples[i] as i32 - luma_avg as i32), 6);
        out[i] = clip1(i32::from(dc[i]) + scaled, bit_depth);
    }
    out
}

/// The edge samples needed by one intra prediction invocation. `above` and
/// `left` start at index 0; the shared top-left sample is stored separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntraEdges {
    pub above: Vec<u16>,
    pub left: Vec<u16>,
    pub top_left: u16,
    pub have_above: bool,
    pub have_left: bool,
    /// Number of samples available before the right/bottom frame boundary.
    pub above_available: usize,
    pub left_available: usize,
}

impl IntraEdges {
    /// Build `AboveRow` / `LeftCol` for the intra prediction process, including
    /// the unavailable-edge substitutions from §7.11.2.1.
    #[allow(clippy::too_many_arguments)]
    pub fn from_plane(
        frame: &[u16],
        stride: usize,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        have_above: bool,
        have_left: bool,
        have_above_right: bool,
        have_below_left: bool,
        max_x: usize,
        max_y: usize,
        bit_depth: u32,
    ) -> Self {
        assert!(stride >= max_x + 1);
        assert!(frame.len() >= stride * (max_y + 1));
        let mid = 1u16 << (bit_depth - 1);
        let mut above = vec![0u16; w + h];
        let mut left = vec![0u16; w + h];
        if !have_above && have_left {
            above.fill(frame[y * stride + x - 1]);
        } else if !have_above {
            above.fill(mid - 1);
        } else {
            let above_limit = max_x.min(x + if have_above_right { 2 * w } else { w } - 1);
            for (i, sample) in above.iter_mut().enumerate() {
                *sample = frame[(y - 1) * stride + (x + i).min(above_limit)];
            }
        }
        if !have_left && have_above {
            left.fill(frame[(y - 1) * stride + x]);
        } else if !have_left {
            left.fill(mid + 1);
        } else {
            let left_limit = max_y.min(y + if have_below_left { 2 * h } else { h } - 1);
            for (i, sample) in left.iter_mut().enumerate() {
                *sample = frame[(y + i).min(left_limit) * stride + x - 1];
            }
        }
        let top_left = match (have_above, have_left) {
            (true, true) => frame[(y - 1) * stride + x - 1],
            (true, false) => frame[(y - 1) * stride + x],
            (false, true) => frame[y * stride + x - 1],
            (false, false) => mid,
        };
        Self {
            above,
            left,
            top_left,
            have_above,
            have_left,
            above_available: w.min(max_x.saturating_sub(x) + 1),
            left_available: h.min(max_y.saturating_sub(y) + 1),
        }
    }
}

/// Select the intra edge filter type from the above/left neighbour modes.
pub fn intra_filter_type(above: Option<IntraMode>, left: Option<IntraMode>) -> u8 {
    let smooth = |mode: Option<IntraMode>| {
        matches!(
            mode,
            Some(IntraMode::Smooth | IntraMode::SmoothV | IntraMode::SmoothH)
        )
    };
    u8::from(smooth(above) || smooth(left))
}

fn mode_angle(mode: IntraMode) -> Option<i32> {
    Some(match mode {
        IntraMode::D45 => 45,
        IntraMode::D67 => 67,
        IntraMode::V => 90,
        IntraMode::D113 => 113,
        IntraMode::D135 => 135,
        IntraMode::D157 => 157,
        IntraMode::H => 180,
        IntraMode::D203 => 203,
        _ => return None,
    })
}

#[derive(Debug, Clone)]
struct DirectionalEdge {
    samples: Vec<u16>,
    /// Logical index of `samples[0]`.
    origin: isize,
}

impl DirectionalEdge {
    fn at(&self, index: isize) -> u16 {
        let slot = index - self.origin;
        debug_assert!(slot >= 0 && slot < self.samples.len() as isize);
        self.samples[slot as usize]
    }
}

/// Run intra prediction for one transform block. `angle_delta` is the syntax
/// element in `-3..=3`; it is multiplied by `ANGLE_STEP` (3 degrees).
#[allow(clippy::too_many_arguments)]
pub fn predict_intra(
    mode: IntraMode,
    edges: &IntraEdges,
    log2_w: u32,
    log2_h: u32,
    angle_delta: i32,
    enable_intra_edge_filter: bool,
    filter_type: u8,
    bit_depth: u32,
) -> Vec<u16> {
    let w = 1usize << log2_w;
    let h = 1usize << log2_h;
    assert!(filter_type <= 1);
    assert!(edges.above.len() >= w + h);
    assert!(edges.left.len() >= w + h);
    match mode {
        IntraMode::Dc => dc_predict(
            &edges.above,
            &edges.left,
            edges.have_above,
            edges.have_left,
            log2_w,
            log2_h,
            bit_depth,
        ),
        IntraMode::V if angle_delta == 0 => v_predict(&edges.above, log2_w, log2_h),
        IntraMode::H if angle_delta == 0 => h_predict(&edges.left, log2_w, log2_h),
        IntraMode::Smooth => smooth_predict(
            SmoothMode::Smooth,
            &edges.above,
            &edges.left,
            log2_w,
            log2_h,
        ),
        IntraMode::SmoothV => smooth_predict(
            SmoothMode::SmoothV,
            &edges.above,
            &edges.left,
            log2_w,
            log2_h,
        ),
        IntraMode::SmoothH => smooth_predict(
            SmoothMode::SmoothH,
            &edges.above,
            &edges.left,
            log2_w,
            log2_h,
        ),
        IntraMode::Paeth => {
            paeth_predict(&edges.above, &edges.left, edges.top_left, log2_w, log2_h)
        }
        _ => directional_predict(
            mode,
            edges,
            w,
            h,
            angle_delta,
            enable_intra_edge_filter,
            filter_type,
            bit_depth,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn directional_predict(
    mode: IntraMode,
    edges: &IntraEdges,
    w: usize,
    h: usize,
    angle_delta: i32,
    enable_intra_edge_filter: bool,
    filter_type: u8,
    bit_depth: u32,
) -> Vec<u16> {
    let p_angle = mode_angle(mode).expect("directional mode") + angle_delta * 3;
    let mut above = Vec::with_capacity(edges.above.len() + 1);
    above.push(edges.top_left);
    above.extend_from_slice(&edges.above);
    let mut left = Vec::with_capacity(edges.left.len() + 1);
    left.push(edges.top_left);
    left.extend_from_slice(&edges.left);

    let mut upsample_above = false;
    let mut upsample_left = false;
    if enable_intra_edge_filter && p_angle != 90 && p_angle != 180 {
        if (90..180).contains(&p_angle) && w + h >= 24 {
            let corner = round2(
                i32::from(left[1]) * 5 + i32::from(above[0]) * 6 + i32::from(above[1]) * 5,
                4,
            );
            above[0] = corner;
            left[0] = corner;
        }
        if edges.have_above {
            let strength = intra_edge_filter_strength(
                w as u32,
                h as u32,
                u32::from(filter_type),
                p_angle - 90,
            );
            let num_px = edges.above_available + usize::from(p_angle < 90) * h + 1;
            let filtered = intra_edge_filter(&above[..num_px], num_px, strength, bit_depth);
            above[1..num_px].copy_from_slice(&filtered);
        }
        if edges.have_left {
            let strength = intra_edge_filter_strength(
                w as u32,
                h as u32,
                u32::from(filter_type),
                p_angle - 180,
            );
            let num_px = edges.left_available + usize::from(p_angle > 180) * w + 1;
            let filtered = intra_edge_filter(&left[..num_px], num_px, strength, bit_depth);
            left[1..num_px].copy_from_slice(&filtered);
        }
        upsample_above =
            intra_edge_upsample_selection(w as u32, h as u32, u32::from(filter_type), p_angle - 90);
        if upsample_above {
            let num_px = w + usize::from(p_angle < 90) * h;
            above = upsample_edge(&above[..=num_px], num_px, bit_depth);
        }
        upsample_left = intra_edge_upsample_selection(
            w as u32,
            h as u32,
            u32::from(filter_type),
            p_angle - 180,
        );
        if upsample_left {
            let num_px = h + usize::from(p_angle > 180) * w;
            left = upsample_edge(&left[..=num_px], num_px, bit_depth);
        }
    }

    let (above, left) = if upsample_above || upsample_left {
        let above = if upsample_above {
            DirectionalEdge {
                samples: above,
                origin: -2,
            }
        } else {
            DirectionalEdge {
                samples: above,
                origin: -1,
            }
        };
        let left = if upsample_left {
            DirectionalEdge {
                samples: left,
                origin: -2,
            }
        } else {
            DirectionalEdge {
                samples: left,
                origin: -1,
            }
        };
        (above, left)
    } else {
        (
            DirectionalEdge {
                samples: above,
                origin: -1,
            },
            DirectionalEdge {
                samples: left,
                origin: -1,
            },
        )
    };
    let up_above = usize::from(upsample_above);
    let up_left = usize::from(upsample_left);
    let mut out = vec![0u16; w * h];
    if p_angle < 90 {
        let dx = u32::from(dr_intra_derivative(p_angle as u32));
        let max_base = (w + h - 1) << up_above;
        for i in 0..h {
            for j in 0..w {
                let idx = (i + 1) as u32 * dx;
                let base = (idx >> (6 - up_above)) as usize + (j << up_above);
                let shift = ((idx << up_above) >> 1) & 0x1F;
                out[i * w + j] = if base < max_base {
                    let value = u32::from(above.at(base as isize)) * (32 - shift)
                        + u32::from(above.at(base as isize + 1)) * shift;
                    ((value + 16) >> 5) as u16
                } else {
                    above.at(max_base as isize)
                };
            }
        }
    } else if (90..180).contains(&p_angle) {
        let dx = u32::from(dr_intra_derivative((180 - p_angle) as u32));
        let dy = u32::from(dr_intra_derivative((p_angle - 90) as u32));
        for i in 0..h {
            for j in 0..w {
                let idx = j as i32 * 64 - (i as i32 + 1) * dx as i32;
                let base = idx >> (6 - up_above);
                if base >= -(1 << up_above) {
                    let shift = ((idx << up_above) as u32 >> 1) & 0x1F;
                    let value = u32::from(above.at(base as isize)) * (32 - shift)
                        + u32::from(above.at(base as isize + 1)) * shift;
                    out[i * w + j] = ((value + 16) >> 5) as u16;
                } else {
                    let idx = i as i32 * 64 - (j as i32 + 1) * dy as i32;
                    let base = idx >> (6 - up_left);
                    let shift = ((idx << up_left) as u32 >> 1) & 0x1F;
                    let value = u32::from(left.at(base as isize)) * (32 - shift)
                        + u32::from(left.at(base as isize + 1)) * shift;
                    out[i * w + j] = ((value + 16) >> 5) as u16;
                }
            }
        }
    } else if p_angle > 180 {
        let dy = u32::from(dr_intra_derivative((270 - p_angle) as u32));
        for i in 0..h {
            for j in 0..w {
                let idx = (j + 1) as u32 * dy;
                let base = (idx >> (6 - up_left)) as usize + (i << up_left);
                let shift = ((idx << up_left) >> 1) & 0x1F;
                let value = u32::from(left.at(base as isize)) * (32 - shift)
                    + u32::from(left.at(base as isize + 1)) * shift;
                out[i * w + j] = ((value + 16) >> 5) as u16;
            }
        }
    } else if p_angle == 90 {
        for i in 0..h {
            out[i * w..(i + 1) * w].copy_from_slice(&edges.above[..w]);
        }
    } else {
        for i in 0..h {
            out[i * w..(i + 1) * w].fill(edges.left[i]);
        }
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
    fn cfl_constant_luma_leaves_the_dc_prediction() {
        let luma = vec![100u16; 64];
        let dc = vec![128u16; 16];
        let pred = cfl_predict(&luma, 8, 0, 0, 4, 4, 1, 1, 8, 8, 1, &dc, 8);
        assert_eq!(pred, dc);
    }

    #[test]
    fn cfl_adds_the_scaled_high_frequency() {
        let mut luma = vec![0u16; 64];
        luma[0] = 255;
        let dc = vec![128u16; 16];
        let pred = cfl_predict(&luma, 8, 0, 0, 4, 4, 1, 1, 8, 8, 1, &dc, 8);
        assert_eq!(pred[0], 135);
        assert!(pred[1..].iter().all(|v| *v == 127), "{pred:?}");
        // A negative alpha mirrors the correction around the DC value.
        let pred = cfl_predict(&luma, 8, 0, 0, 4, 4, 1, 1, 8, 8, -1, &dc, 8);
        assert_eq!(pred[0], 121);
    }

    #[test]
    fn edge_filter_applies_the_strength_kernel() {
        let edge = [10u16, 20, 30, 40];
        // Strength 1 is (0,4,8,4,0); edge[0] is buf[-1] and is not rewritten.
        assert_eq!(intra_edge_filter(&edge, 4, 1, 8), [20, 30, 38]);
        // Strength 0 copies buf[0..] unchanged.
        assert_eq!(intra_edge_filter(&edge, 4, 0, 8), [20, 30, 40]);
        // Strength 3 is (2,4,4,4,2).
        assert_eq!(intra_edge_filter(&edge, 4, 3, 8), [21, 29, 35]);
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
    fn intra_edges_substitute_unavailable_neighbours() {
        let mut frame = vec![0u16; 64];
        for i in 0..8 {
            for j in 0..8 {
                frame[i * 8 + j] = (i * 10 + j) as u16;
            }
        }
        let edges =
            IntraEdges::from_plane(&frame, 8, 2, 2, 2, 2, false, true, false, false, 7, 7, 8);
        assert_eq!(edges.above, [21, 21, 21, 21]);
        assert_eq!(edges.left, [21, 31, 31, 31]);
        assert_eq!(edges.top_left, 21);
        assert_eq!(edges.above_available, 2);

        let edges =
            IntraEdges::from_plane(&frame, 8, 2, 2, 2, 2, false, false, false, false, 7, 7, 8);
        assert!(edges.above.iter().all(|v| *v == 127));
        assert!(edges.left.iter().all(|v| *v == 129));
        assert_eq!(edges.top_left, 128);
    }

    #[test]
    fn intra_prediction_dispatches_cardinal_modes() {
        let edges = IntraEdges {
            above: vec![10, 20, 30, 40],
            left: vec![50, 60, 70, 80],
            top_left: 5,
            have_above: true,
            have_left: true,
            above_available: 2,
            left_available: 2,
        };
        assert_eq!(
            predict_intra(IntraMode::V, &edges, 1, 1, 0, false, 0, 8),
            [10, 20, 10, 20]
        );
        assert_eq!(
            predict_intra(IntraMode::H, &edges, 1, 1, 0, false, 0, 8),
            [50, 50, 60, 60]
        );
        assert_eq!(
            predict_intra(IntraMode::D45, &edges, 1, 1, 0, false, 0, 8),
            [20, 30, 30, 40]
        );
    }

    #[test]
    fn horizontal_angle_delta_uses_the_directional_process() {
        let edges = IntraEdges {
            above: vec![110, 110, 110, 110, 110, 110, 96, 65, 67, 98, 113, 52],
            left: vec![108, 105, 107, 30, 33, 34, 34, 55, 54, 54, 54, 54],
            top_left: 107,
            have_above: true,
            have_left: true,
            above_available: 12,
            left_available: 12,
        };
        let pred = predict_intra(IntraMode::H, &edges, 3, 2, -1, true, 0, 8);
        assert_eq!(
            pred,
            [
                108, 108, 108, 108, 108, 108, 108, 108, 105, 105, 105, 105, 105, 106, 106, 106,
                107, 108, 108, 109, 109, 109, 110, 110, 34, 37, 41, 45, 48, 52, 56, 59,
            ]
        );
    }

    #[test]
    fn upsampled_directional_edge_uses_the_upsampled_interpolation_phase() {
        let edges = IntraEdges {
            above: vec![20, 21, 17, 20, 16, 16, 21, 26],
            left: vec![20, 17, 17, 20, 146, 146, 146, 146],
            top_left: 18,
            have_above: true,
            have_left: true,
            above_available: 4,
            left_available: 4,
        };
        let pred = predict_intra(IntraMode::V, &edges, 2, 2, -2, true, 0, 8);
        assert_eq!(
            pred,
            [20, 21, 17, 20, 20, 20, 18, 19, 21, 20, 18, 19, 21, 19, 19, 18]
        );
    }

    #[test]
    fn left_upsampled_directional_edge_uses_the_upsampled_interpolation_phase() {
        let edges = IntraEdges {
            above: vec![105, 126, 164, 90, 90, 90, 90, 90],
            left: vec![112, 113, 115, 116, 116, 116, 116, 116],
            top_left: 114,
            have_above: true,
            have_left: true,
            above_available: 3,
            left_available: 4,
        };
        let pred = predict_intra(IntraMode::D203, &edges, 2, 2, 0, true, 0, 8);
        assert_eq!(
            pred,
            [112, 113, 114, 114, 114, 115, 116, 116, 116, 116, 116, 116, 116, 116, 116, 116]
        );
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
