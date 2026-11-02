//! H.264 deblocking primitives.
//!
//! Edge traversal and alpha/beta/tc table selection are separate from the
//! arithmetic filters. Lines use `p3,p2,p1,p0,q0,q1,q2,q3` order and `u16`
//! samples, so the same kernels support 8- and 10-bit planes.

/// I-slice boundary strength before motion-vector rules are needed.
pub fn i_boundary_strength(
    macroblock_edge: bool,
    p_has_coefficients: bool,
    q_has_coefficients: bool,
) -> u8 {
    if macroblock_edge {
        4
    } else if p_has_coefficients || q_has_coefficients {
        2
    } else {
        0
    }
}

/// Ordinary luma edge filter, including the optional p1/q1 updates.
pub fn filter_luma(line: &mut [u16; 8], alpha: i32, beta: i32, tc0: i32, bit_depth: u8) -> bool {
    let p2 = i32::from(line[1]);
    let p1 = i32::from(line[2]);
    let p0 = i32::from(line[3]);
    let q0 = i32::from(line[4]);
    let q1 = i32::from(line[5]);
    let q2 = i32::from(line[6]);
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return false;
    }

    let mut tc = tc0;
    if (p2 - p0).abs() < beta {
        if tc0 != 0 {
            let value = ((p2 + ((p0 + q0 + 1) >> 1)) >> 1) - p1;
            line[2] = clip(p1 + value.clamp(-tc0, tc0), bit_depth);
        }
        tc += 1;
    }
    if (q2 - q0).abs() < beta {
        if tc0 != 0 {
            let value = ((q2 + ((p0 + q0 + 1) >> 1)) >> 1) - q1;
            line[5] = clip(q1 + value.clamp(-tc0, tc0), bit_depth);
        }
        tc += 1;
    }
    let delta = ((4 * (q0 - p0) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
    line[3] = clip(p0 + delta, bit_depth);
    line[4] = clip(q0 - delta, bit_depth);
    true
}

/// Strong intra luma edge filter.
pub fn filter_luma_intra(line: &mut [u16; 8], alpha: i32, beta: i32, bit_depth: u8) -> bool {
    let p3 = i32::from(line[0]);
    let p2 = i32::from(line[1]);
    let p1 = i32::from(line[2]);
    let p0 = i32::from(line[3]);
    let q0 = i32::from(line[4]);
    let q1 = i32::from(line[5]);
    let q2 = i32::from(line[6]);
    let q3 = i32::from(line[7]);
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return false;
    }

    if (p0 - q0).abs() < (alpha >> 2) + 2 {
        if (p2 - p0).abs() < beta {
            line[3] = clip((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3, bit_depth);
            line[2] = clip((p2 + p1 + p0 + q0 + 2) >> 2, bit_depth);
            line[1] = clip((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3, bit_depth);
        } else {
            line[3] = clip((2 * p1 + p0 + q1 + 2) >> 2, bit_depth);
        }
        if (q2 - q0).abs() < beta {
            line[4] = clip((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3, bit_depth);
            line[5] = clip((p0 + q0 + q1 + q2 + 2) >> 2, bit_depth);
            line[6] = clip((2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3, bit_depth);
        } else {
            line[4] = clip((2 * q1 + q0 + p1 + 2) >> 2, bit_depth);
        }
    } else {
        line[3] = clip((2 * p1 + p0 + q1 + 2) >> 2, bit_depth);
        line[4] = clip((2 * q1 + q0 + p1 + 2) >> 2, bit_depth);
    }
    true
}

/// Ordinary chroma edge filter. The line is `p1,p0,q0,q1`.
pub fn filter_chroma(line: &mut [u16; 4], alpha: i32, beta: i32, tc: i32, bit_depth: u8) -> bool {
    let p1 = i32::from(line[0]);
    let p0 = i32::from(line[1]);
    let q0 = i32::from(line[2]);
    let q1 = i32::from(line[3]);
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return false;
    }
    let delta = ((4 * (q0 - p0) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
    line[1] = clip(p0 + delta, bit_depth);
    line[2] = clip(q0 - delta, bit_depth);
    true
}

/// Strong intra chroma edge filter.
pub fn filter_chroma_intra(line: &mut [u16; 4], alpha: i32, beta: i32, bit_depth: u8) -> bool {
    let p1 = i32::from(line[0]);
    let p0 = i32::from(line[1]);
    let q0 = i32::from(line[2]);
    let q1 = i32::from(line[3]);
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return false;
    }
    line[1] = clip((2 * p1 + p0 + q1 + 2) >> 2, bit_depth);
    line[2] = clip((2 * q1 + q0 + p1 + 2) >> 2, bit_depth);
    true
}

fn clip(value: i32, bit_depth: u8) -> u16 {
    value.clamp(0, (1 << bit_depth) - 1) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_luma_filter_updates_p0_and_q0_with_the_rounded_delta() {
        let mut line = [100, 110, 120, 130, 140, 150, 160, 170];
        assert!(filter_luma(&mut line, 255, 18, 4, 8));
        assert_eq!(line, [100, 110, 120, 131, 139, 150, 160, 170]);
    }

    #[test]
    fn strong_luma_filter_handles_p1_and_q1() {
        let mut line = [80, 110, 120, 130, 145, 160, 175, 190];
        assert!(filter_luma_intra(&mut line, 255, 40, 8));
        assert_eq!(line[1..7], [111, 126, 133, 146, 153, 168]);
    }

    #[test]
    fn chroma_filters_use_the_same_rounding_pattern() {
        let mut line = [120, 130, 140, 150];
        assert!(filter_chroma(&mut line, 255, 18, 2, 8));
        assert_eq!(line, [120, 131, 139, 150]);

        let mut line = [120, 130, 140, 150];
        assert!(filter_chroma_intra(&mut line, 255, 22, 8));
        assert_eq!(line, [120, 130, 140, 150]);
    }

    #[test]
    fn boundary_strength_distinguishes_macroblock_and_internal_edges() {
        assert_eq!(i_boundary_strength(true, false, false), 4);
        assert_eq!(i_boundary_strength(false, true, false), 2);
        assert_eq!(i_boundary_strength(false, false, true), 2);
        assert_eq!(i_boundary_strength(false, false, false), 0);
    }

    #[test]
    fn high_bit_depth_samples_are_clipped_to_the_configured_depth() {
        let mut line = [1000, 1010, 1020, 1022, 1023, 1023, 1023, 1023];
        filter_luma(&mut line, 255, 18, 8, 10);
        assert!(line.iter().all(|&sample| sample <= 1023));
    }
}
