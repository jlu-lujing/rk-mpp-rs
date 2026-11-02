//! H.264 software-reference primitives.
//!
//! This starts the decoder ground-truth path with the specification's inverse
//! 4x4 residual transform, CAVLC 8x8 scan/dequantisation, constant DC
//! prediction and clipped residual add. The functions operate on quantised or
//! already dequantised coefficients and integer pixel planes; syntax decoding
//! and reference-picture scheduling build on them.

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

#[cfg(test)]
fn inverse_dequant_position_4x4(position: usize) -> usize {
    (position >> 2) | ((position << 2) & 0xf)
}

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
    for position in 0..16 {
        let scale = scaling.unwrap_or([16; 16])[position];
        let class = (position & 1) + ((position >> 2) & 1);
        qmul[position] = (DEQUANT4[index as usize][class] * scale) << shift;
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

/// The frame and field 8x8 CAVLC scan orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CavlcScan8x8 {
    Frame,
    Field,
}

/// CAVLC 8x8 scan order to raster position. This is deliberately not an
/// ordinary 8x8 zigzag: each group of four positions follows one 4x4
/// sub-block, as specified by H.264 Table 8-13.
const ZIGZAG_8X8_CAVLC: [usize; 64] = [
    0, 9, 17, 18, 12, 40, 27, 7, 35, 57, 29, 30, 58, 38, 53, 47, 1, 2, 24, 11, 19, 48, 20, 14, 42,
    50, 22, 37, 59, 31, 60, 55, 8, 3, 32, 4, 26, 41, 13, 21, 49, 43, 15, 44, 52, 39, 61, 62, 16,
    10, 25, 5, 33, 34, 6, 28, 56, 36, 23, 51, 45, 46, 54, 63,
];

const FIELD_8X8_CAVLC: [usize; 64] = [
    0, 9, 2, 56, 18, 26, 34, 27, 35, 28, 36, 29, 45, 7, 54, 39, 8, 24, 25, 33, 41, 11, 42, 12, 43,
    13, 44, 14, 53, 15, 62, 47, 16, 32, 40, 10, 49, 4, 50, 5, 51, 6, 52, 22, 61, 38, 23, 55, 1, 17,
    48, 3, 57, 19, 58, 20, 59, 21, 60, 37, 30, 46, 31, 63,
];

/// Place CAVLC 8x8 levels into a raster-order coefficient block.
///
/// This is the normal CAVLC path. Transform-bypass streams use a separate q0
/// scan and are intentionally not conflated with this table.
pub fn inverse_scan_8x8_cavlc(scan: [i32; 64], layout: CavlcScan8x8) -> [i32; 64] {
    let table = match layout {
        CavlcScan8x8::Frame => &ZIGZAG_8X8_CAVLC,
        CavlcScan8x8::Field => &FIELD_8X8_CAVLC,
    };
    let mut raster = [0i32; 64];
    for (position, value) in scan.into_iter().enumerate() {
        raster[table[position]] = value;
    }
    raster
}

const DEQUANT8: [[i32; 6]; 6] = [
    [20, 18, 32, 19, 25, 24],
    [22, 19, 35, 21, 28, 26],
    [26, 23, 42, 24, 33, 31],
    [28, 25, 45, 26, 35, 33],
    [32, 28, 51, 30, 40, 38],
    [36, 32, 58, 34, 46, 43],
];

const DEQUANT8_SCAN: [usize; 16] = [0, 3, 4, 3, 3, 1, 5, 1, 4, 5, 2, 5, 3, 1, 5, 1];

#[cfg(test)]
fn inverse_dequant_position_8x8(position: usize) -> usize {
    (position >> 3) | ((position & 7) << 3)
}

/// H.264 8.5.9 inverse quantisation for one CAVLC-scanned 8x8 residual block.
///
/// `scaling` is a raster-order 8x8 matrix factor. `None` is the flat matrix of
/// sixty-four used by High-profile streams without a custom 8x8 matrix.
pub fn dequant_8x8_cavlc(
    scan: [i32; 64],
    qp: u8,
    bit_depth: u8,
    scaling: Option<&[u8; 64]>,
    layout: CavlcScan8x8,
) -> Result<[i32; 64], &'static str> {
    if !(8..=10).contains(&bit_depth) {
        return Err("H.264 reference dequant supports 8..=10 bit");
    }
    let max_qp = 51 + 6 * (bit_depth as u16 - 8);
    if u16::from(qp) > max_qp {
        return Err("H.264 QP exceeds the profile bit-depth limit");
    }

    let shift = qp / 6;
    let index = qp % 6;
    let scaling = scaling.map(|matrix| matrix.map(i32::from));
    let mut qmul = [0i32; 64];
    for position in 0..64 {
        let scale = scaling.unwrap_or([16; 64])[position];
        let class_position = ((position >> 1) & 12) | (position & 3);
        let class = DEQUANT8_SCAN[class_position];
        qmul[position] = (DEQUANT8[index as usize][class] * scale) << shift;
    }

    let table = match layout {
        CavlcScan8x8::Frame => &ZIGZAG_8X8_CAVLC,
        CavlcScan8x8::Field => &FIELD_8X8_CAVLC,
    };
    let mut output = [0i32; 64];
    for (scan_pos, level) in scan.into_iter().enumerate() {
        let raster = table[scan_pos];
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

    #[test]
    fn dequant_4x4_keeps_custom_scaling_in_raster_order() {
        let levels: [i32; 16] = std::array::from_fn(|position| position as i32 - 8);
        let scaling: [u8; 16] = std::array::from_fn(|position| position as u8 + 1);
        for qp in [0, 5, 6, 7, 51] {
            let actual = dequant_4x4(levels, qp, 8, Some(&scaling)).unwrap();
            let expected = reference_dequant_4x4(levels, qp, &scaling);
            assert_eq!(actual, expected, "QP {qp}");
        }
    }

    #[test]
    fn cavlc_8x8_frame_scan_is_not_the_regular_zigzag() {
        let scan: [i32; 64] = std::array::from_fn(|position| position as i32 + 1);
        let raster = inverse_scan_8x8_cavlc(scan, CavlcScan8x8::Frame);
        assert_eq!(
            raster[..20],
            [1, 17, 18, 34, 36, 52, 55, 8, 33, 2, 50, 20, 5, 39, 24, 43, 49, 3, 4, 21]
        );
        let mut seen = [false; 64];
        for &position in &ZIGZAG_8X8_CAVLC {
            assert!(!seen[position]);
            seen[position] = true;
        }
        assert!(seen.into_iter().all(|present| present));
    }

    #[test]
    fn cavlc_8x8_field_scan_covers_the_other_macroblock_layout() {
        let scan: [i32; 64] = std::array::from_fn(|position| position as i32 + 1);
        let raster = inverse_scan_8x8_cavlc(scan, CavlcScan8x8::Field);
        assert_eq!(
            raster[..16],
            [1, 49, 3, 52, 38, 40, 42, 14, 17, 2, 36, 22, 24, 26, 28, 30]
        );
        let mut seen = [false; 64];
        for &position in &FIELD_8X8_CAVLC {
            assert!(!seen[position]);
            seen[position] = true;
        }
        assert!(seen.into_iter().all(|present| present));
    }

    #[test]
    fn cavlc_8x8_dequant_matches_the_ffmpeg_internal_transposed_layout() {
        let levels: [i32; 64] = std::array::from_fn(|position| position as i32 - 32);
        let scaling: [u8; 64] = std::array::from_fn(|position| position as u8 + 1);
        for qp in [0, 5, 6, 7, 51] {
            for layout in [CavlcScan8x8::Frame, CavlcScan8x8::Field] {
                let actual = dequant_8x8_cavlc(levels, qp, 8, Some(&scaling), layout).unwrap();
                let expected = reference_dequant_8x8(levels, qp, &scaling, layout);
                assert_eq!(actual, expected, "QP {qp}, layout {layout:?}");
            }
        }
    }

    #[test]
    fn cavlc_8x8_dequant_keeps_custom_scaling_in_raster_order() {
        let mut scaling = [16u8; 64];
        scaling[37] = 3;
        let mut levels = [0i32; 64];
        levels[27] = 64; // frame scan index 27 maps to raster position 37.
        let output = dequant_8x8_cavlc(levels, 7, 8, Some(&scaling), CavlcScan8x8::Frame).unwrap();
        assert_eq!(output[37], 126);
        assert!(output
            .iter()
            .enumerate()
            .all(|(position, &value)| position == 37 || value == 0));
    }

    #[test]
    fn cavlc_8x8_dequant_checks_qp_depth_and_overflows() {
        let levels = [1i32; 64];
        assert!(dequant_8x8_cavlc(levels, 51, 8, None, CavlcScan8x8::Frame).is_ok());
        assert!(dequant_8x8_cavlc(levels, 52, 8, None, CavlcScan8x8::Frame).is_err());
        assert!(dequant_8x8_cavlc(levels, 63, 10, None, CavlcScan8x8::Frame).is_ok());
        assert!(dequant_8x8_cavlc(levels, 64, 10, None, CavlcScan8x8::Frame).is_err());
    }

    fn reference_dequant_8x8(
        levels: [i32; 64],
        qp: u8,
        scaling: &[u8; 64],
        layout: CavlcScan8x8,
    ) -> [i32; 64] {
        let shift = qp / 6;
        let index = qp % 6;
        let mut internal_qmul = [0i32; 64];
        for position in 0..64 {
            let class_position = ((position >> 1) & 12) | (position & 3);
            let class = DEQUANT8_SCAN[class_position];
            internal_qmul[inverse_dequant_position_8x8(position)] =
                (DEQUANT8[index as usize][class] * i32::from(scaling[position])) << shift;
        }
        let mut qmul = [0i32; 64];
        for position in 0..64 {
            qmul[position] = internal_qmul[inverse_dequant_position_8x8(position)];
        }
        let table = match layout {
            CavlcScan8x8::Frame => &ZIGZAG_8X8_CAVLC,
            CavlcScan8x8::Field => &FIELD_8X8_CAVLC,
        };
        let mut output = [0i32; 64];
        for (scan_position, level) in levels.into_iter().enumerate() {
            let raster = table[scan_position];
            output[raster] = (level * qmul[raster] + 32) >> 6;
        }
        output
    }

    fn reference_dequant_4x4(levels: [i32; 16], qp: u8, scaling: &[u8; 16]) -> [i32; 16] {
        let shift = qp / 6 + 2;
        let index = qp % 6;
        let mut internal_qmul = [0i32; 16];
        for position in 0..16 {
            let class = (position & 1) + ((position >> 2) & 1);
            internal_qmul[inverse_dequant_position_4x4(position)] =
                (DEQUANT4[index as usize][class] * i32::from(scaling[position])) << shift;
        }
        let mut qmul = [0i32; 16];
        for position in 0..16 {
            qmul[position] = internal_qmul[inverse_dequant_position_4x4(position)];
        }
        let mut output = [0i32; 16];
        for (scan_position, level) in levels.into_iter().enumerate() {
            let raster = ZIGZAG_4X4[scan_position];
            output[raster] = (level * qmul[raster] + 32) >> 6;
        }
        output
    }
}
