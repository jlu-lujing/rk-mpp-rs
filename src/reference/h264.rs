//! H.264 software-reference primitives.
//!
//! This starts the decoder ground-truth path with the specification's inverse
//! 4x4 residual transform, constant DC prediction and clipped residual add. The
//! functions operate on already dequantised coefficients and integer pixel
//! planes; syntax decoding and reference-picture scheduling build on them.

/// H.264 8.5.12 inverse 4x4 residual transform.
///
/// The two 1-D passes use the integer butterfly from the specification. The
/// final column pass adds the rounding offset and shifts by six:
/// `(e + 32) >> 6`.
pub fn inverse_4x4(coeffs: [i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for row in 0..4 {
        let at = row * 4;
        butterfly(
            [coeffs[at], coeffs[at + 1], coeffs[at + 2], coeffs[at + 3]],
            &mut tmp[at..at + 4],
        );
    }

    let mut out = [0i32; 16];
    for column in 0..4 {
        let mut values = [0i32; 4];
        butterfly(
            [
                tmp[column],
                tmp[4 + column],
                tmp[8 + column],
                tmp[12 + column],
            ],
            &mut values,
        );
        for row in 0..4 {
            out[row * 4 + column] = (values[row] + 32) >> 6;
        }
    }
    out
}

fn butterfly(input: [i32; 4], output: &mut [i32]) {
    let i0 = input[0] + input[2];
    let i1 = input[0] - input[2];
    let i2 = (input[1] >> 1) - input[3];
    let i3 = input[1] + (input[3] >> 1);
    output[0] = i0 + i3;
    output[1] = i1 + i2;
    output[2] = i1 - i2;
    output[3] = i0 - i3;
}

/// H.264 constant DC prediction for an `size x size` block.
///
/// Neighbours must be exactly `size` samples when present. With both edges the
/// prediction is `(sum(top) + sum(left) + size) >> log2(2*size)`; one edge
/// averages that edge with the same rounding; no edge yields mid-level.
pub fn dc_predict(size: usize, top: Option<&[u8]>, left: Option<&[u8]>) -> Vec<u8> {
    assert!(matches!(size, 4 | 8 | 16));
    assert!(top.is_none_or(|edge| edge.len() == size));
    assert!(left.is_none_or(|edge| edge.len() == size));

    let value = match (top, left) {
        (Some(top), Some(left)) => {
            let sum = top
                .iter()
                .chain(left)
                .map(|sample| u32::from(*sample))
                .sum::<u32>();
            ((sum + size as u32) >> (size.trailing_zeros() + 1)) as u8
        }
        (Some(top), None) => {
            let sum = top.iter().map(|sample| u32::from(*sample)).sum::<u32>();
            ((sum + size as u32 / 2) >> size.trailing_zeros()) as u8
        }
        (None, Some(left)) => {
            let sum = left.iter().map(|sample| u32::from(*sample)).sum::<u32>();
            ((sum + size as u32 / 2) >> size.trailing_zeros()) as u8
        }
        (None, None) => 128,
    };
    vec![value; size * size]
}

/// Add a transformed residual to an 8-bit plane and clip to 0..=255.
pub fn add_residual_4x4(
    plane: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    x: usize,
    y: usize,
    residual: &[i32; 16],
    bit_depth: u8,
) -> Result<(), &'static str> {
    if bit_depth != 8 {
        return Err("use the u16 residual add for high-bit-depth output");
    }
    if stride < width || plane.len() < stride * height {
        return Err("pixel plane is smaller than its stride x height");
    }
    if x + 4 > width || y + 4 > height {
        return Err("4x4 residual block exceeds the visible plane");
    }
    let max = ((1u32 << bit_depth) - 1) as i32;
    for row in 0..4 {
        for column in 0..4 {
            let at = (y + row) * stride + x + column;
            let value = i32::from(plane[at]) + residual[row * 4 + column];
            plane[at] = value.clamp(0, max) as u8;
        }
    }
    Ok(())
}

/// Add a transformed residual to a 16-bit storage plane for 8/10-bit output.
pub fn add_residual_4x4_u16(
    plane: &mut [u16],
    width: usize,
    height: usize,
    stride: usize,
    x: usize,
    y: usize,
    residual: &[i32; 16],
    bit_depth: u8,
) -> Result<(), &'static str> {
    if !(8..=10).contains(&bit_depth) {
        return Err("H.264 reference add supports 8..=10 bit output");
    }
    if stride < width || plane.len() < stride * height {
        return Err("pixel plane is smaller than its stride x height");
    }
    if x + 4 > width || y + 4 > height {
        return Err("4x4 residual block exceeds the visible plane");
    }
    let max = ((1u32 << bit_depth) - 1) as i32;
    for row in 0..4 {
        for column in 0..4 {
            let at = (y + row) * stride + x + column;
            let value = i32::from(plane[at]) + residual[row * 4 + column];
            plane[at] = value.clamp(0, max) as u16;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_coefficient_spreads_to_every_sample() {
        let mut coeffs = [0i32; 16];
        coeffs[0] = 4096;
        let residual = inverse_4x4(coeffs);
        assert_eq!(residual, [64; 16]);
    }

    #[test]
    fn one_ac_coefficient_matches_the_integer_butterfly() {
        let mut coeffs = [0i32; 16];
        coeffs[1] = 4096;
        assert_eq!(
            inverse_4x4(coeffs),
            [64, 32, -32, -64, 64, 32, -32, -64, 64, 32, -32, -64, 64, 32, -32, -64,]
        );
    }

    #[test]
    fn dc_prediction_uses_the_available_edges() {
        assert_eq!(
            dc_predict(4, Some(&[10, 11, 12, 13]), Some(&[20, 21, 22, 23])),
            vec![17; 16]
        );
        assert_eq!(dc_predict(4, Some(&[10, 11, 12, 13]), None), vec![12; 16]);
        assert_eq!(dc_predict(4, None, Some(&[20, 21, 22, 23])), vec![22; 16]);
        assert_eq!(dc_predict(4, None, None), vec![128; 16]);
        assert_eq!(
            dc_predict(16, Some(&[16; 16]), Some(&[32; 16])),
            vec![24; 256]
        );
    }

    #[test]
    fn residual_add_clips_and_respects_the_plane_stride() {
        let mut plane = vec![0u8; 8 * 6];
        for (index, sample) in plane.iter_mut().enumerate() {
            *sample = (index % 17) as u8;
        }
        let before = plane.clone();
        let residual = [255; 16];
        add_residual_4x4(&mut plane, 6, 6, 8, 1, 1, &residual, 8).unwrap();
        for row in 0..4 {
            for column in 0..4 {
                assert_eq!(plane[(row + 1) * 8 + column + 1], 255);
            }
        }
        for row in 0..6 {
            assert_eq!(
                plane[row * 8 + 6..row * 8 + 8],
                before[row * 8 + 6..row * 8 + 8]
            );
        }

        let mut high = vec![0u16; 16];
        high[0] = 1022;
        add_residual_4x4_u16(&mut high, 4, 4, 4, 0, 0, &[4; 16], 10).unwrap();
        assert_eq!(high[0], 1023);
        assert_eq!(&high[1..], &[4; 15]);
    }

    #[test]
    fn residual_add_rejects_out_of_bounds_blocks() {
        let mut plane = vec![0u8; 16];
        assert!(add_residual_4x4(&mut plane, 4, 4, 4, 1, 0, &[0; 16], 8).is_err());
        assert!(add_residual_4x4(&mut plane, 4, 4, 3, 0, 0, &[0; 16], 8).is_err());
    }
}
