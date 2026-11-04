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

#[cfg(test)]
mod tests {
    use super::*;

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
