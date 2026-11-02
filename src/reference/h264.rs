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

/// Scan order to raster position for a residual 4x4 block (`scan4x4`).
const ZIGZAG_4X4: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// Place quantised 4x4 levels from scan order into the raster coefficient block.
pub fn inverse_scan_4x4(scan: [i32; 16]) -> [i32; 16] {
    let mut raster = [0i32; 16];
    for (position, value) in scan.into_iter().enumerate() {
        raster[ZIGZAG_4X4[position]] = value;
    }
    raster
}

const DEQUANT4: [[i32; 3]; 6] = [
    [10, 13, 16],
    [11, 14, 18],
    [13, 16, 20],
    [14, 18, 23],
    [16, 20, 25],
    [18, 23, 29],
];

/// H.264 8.5.9 inverse quantisation for one scanned 4x4 residual block.
///
/// `scaling` is a raster-order 4x4 matrix factor. `None` is the flat matrix of
/// sixteen used by the baseline streams. The level at scan position zero is
/// treated like every other level; luma/chroma DC streams use their separate
/// DC transform path before this function.
pub fn dequant_4x4(
    scan: [i32; 16],
    qp: u8,
    bit_depth: u8,
    scaling: Option<&[u8; 16]>,
) -> Result<[i32; 16], &'static str> {
    if !(8..=10).contains(&bit_depth) {
        return Err("H.264 reference dequant supports 8..=10 bit");
    }
    let max_qp = 51 + 6 * (bit_depth as u16 - 8);
    if u16::from(qp) > max_qp {
        return Err("H.264 QP exceeds the profile bit-depth limit");
    }

    let shift = qp / 6 + 2;
    let index = qp % 6;
    let scaling = scaling.map(|matrix| matrix.map(i32::from));
    let mut qmul = [0i32; 16];
    for x in 0..16 {
        let scale = scaling.unwrap_or([16; 16])[x];
        let class = ((x & 1) + ((x >> 2) & 1)) as usize;
        let destination = (x >> 2) | ((x << 2) & 0xf);
        qmul[destination] = (DEQUANT4[index as usize][class] * scale) << shift;
    }

    let mut output = [0i32; 16];
    for (scan_pos, level) in scan.into_iter().enumerate() {
        let raster = ZIGZAG_4X4[scan_pos];
        let value = (i64::from(level) * i64::from(qmul[raster]) + 32) >> 6;
        output[raster] =
            i32::try_from(value).map_err(|_| "dequantised H.264 coefficient overflows i32")?;
    }
    Ok(output)
}

/// Inverse 4x4 luma DC Hadamard transform before QP-dependent scaling.
pub fn inverse_luma_dc_4x4(input: [i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for row in 0..4 {
        hadamard4(
            [
                input[row * 4],
                input[row * 4 + 1],
                input[row * 4 + 2],
                input[row * 4 + 3],
            ],
            &mut tmp[row * 4..row * 4 + 4],
        );
    }
    let mut output = [0i32; 16];
    for column in 0..4 {
        let mut values = [0i32; 4];
        hadamard4(
            [
                tmp[column],
                tmp[4 + column],
                tmp[8 + column],
                tmp[12 + column],
            ],
            &mut values,
        );
        for row in 0..4 {
            output[row * 4 + column] = values[row];
        }
    }
    output
}

fn hadamard4(input: [i32; 4], output: &mut [i32]) {
    output[0] = input[0] + input[1] + input[2] + input[3];
    output[1] = input[0] + input[1] - input[2] - input[3];
    output[2] = input[0] - input[1] - input[2] + input[3];
    output[3] = input[0] - input[1] + input[2] - input[3];
}

/// Inverse chroma DC 2x2 Hadamard transform before chroma QP scaling.
pub fn inverse_chroma_dc_2x2(input: [i32; 4]) -> [i32; 4] {
    [
        input[0] + input[1] + input[2] + input[3],
        input[0] - input[1] + input[2] - input[3],
        input[0] + input[1] - input[2] - input[3],
        input[0] - input[1] - input[2] + input[3],
    ]
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

    #[test]
    fn inverse_scan_places_levels_at_the_zigzag_raster_positions() {
        let scan: [i32; 16] = std::array::from_fn(|index| index as i32 + 1);
        let raster = inverse_scan_4x4(scan);
        assert_eq!(
            raster,
            [1, 2, 6, 7, 3, 5, 8, 13, 4, 9, 12, 14, 10, 11, 15, 16]
        );
    }

    #[test]
    fn inverse_dc_hadamard_matches_constant_and_impulse_vectors() {
        assert_eq!(
            inverse_luma_dc_4x4([16; 16]),
            [256, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        let mut impulse = [0i32; 16];
        impulse[0] = 1;
        assert_eq!(inverse_luma_dc_4x4(impulse), [1; 16]);

        assert_eq!(inverse_chroma_dc_2x2([4; 4]), [16, 0, 0, 0]);
        assert_eq!(inverse_chroma_dc_2x2([1, 0, 0, 0]), [1; 4]);
    }

    #[test]
    fn dequant_flat_4x4_uses_the_position_classes_for_both_qp_bands() {
        let levels = [1i32; 16];
        assert_eq!(
            dequant_4x4(levels, 0, 8, None).unwrap(),
            [10, 13, 10, 13, 13, 16, 13, 16, 10, 13, 10, 13, 13, 16, 13, 16,]
        );
        assert_eq!(
            dequant_4x4(levels, 24, 8, None).unwrap(),
            [160, 208, 160, 208, 208, 256, 208, 256, 160, 208, 160, 208, 208, 256, 208, 256,]
        );
    }

    #[test]
    fn dequant_rounds_negative_levels_like_ffmpeg_and_checks_qp_depth() {
        let mut levels = [0i32; 16];
        levels[0] = -1;
        let output = dequant_4x4(levels, 0, 8, None).unwrap();
        assert_eq!(output[0], -10);

        assert!(dequant_4x4(levels, 63, 10, None).is_ok());
        assert!(dequant_4x4(levels, 64, 10, None).is_err());
        assert!(dequant_4x4(levels, 52, 8, None).is_err());
    }

    #[test]
    fn dequant_uses_the_supplied_scaling_matrix() {
        let mut scaling = [16u8; 16];
        scaling[0] = 1;
        let mut levels = [0i32; 16];
        levels[0] = 1;
        let output = dequant_4x4(levels, 0, 8, Some(&scaling)).unwrap();
        assert_eq!(output[0], 1);
    }
}
