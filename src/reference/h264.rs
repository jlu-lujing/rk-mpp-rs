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

/// H.264 8.5.10 inverse 8x8 residual transform.
///
/// The coefficient block is raster order. As in the specification and FFmpeg's
/// integer implementation, the DC coefficient gets the rounding offset before
/// the vertical pass; the horizontal pass performs the final arithmetic shift
/// by six.
pub fn inverse_8x8(coeffs: [i32; 64]) -> [i32; 64] {
    let mut block: [i64; 64] = coeffs.map(i64::from);
    block[0] += 32;

    for column in 0..8 {
        let input = std::array::from_fn(|row| block[row * 8 + column]);
        let output = butterfly8(input);
        for row in 0..8 {
            block[row * 8 + column] = output[row];
        }
    }

    let mut residual = [0i32; 64];
    for row in 0..8 {
        let input = std::array::from_fn(|column| block[row * 8 + column]);
        let output = butterfly8(input);
        for column in 0..8 {
            residual[row * 8 + column] = (output[column] >> 6) as i32;
        }
    }
    residual
}

fn butterfly8(input: [i64; 8]) -> [i64; 8] {
    let a0 = input[0] + input[4];
    let a2 = input[0] - input[4];
    let a4 = (input[2] >> 1) - input[6];
    let a6 = (input[6] >> 1) + input[2];

    let b0 = a0 + a6;
    let b2 = a2 + a4;
    let b4 = a2 - a4;
    let b6 = a0 - a6;

    let a1 = -input[3] + input[5] - input[7] - (input[7] >> 1);
    let a3 = input[1] + input[7] - input[3] - (input[3] >> 1);
    let a5 = -input[1] + input[7] + input[5] + (input[5] >> 1);
    let a7 = input[3] + input[5] + input[1] + (input[1] >> 1);

    let b1 = (a7 >> 2) + a1;
    let b3 = a3 + (a5 >> 2);
    let b5 = (a3 >> 2) - a5;
    let b7 = a7 - (a1 >> 2);

    [
        b0 + b7,
        b2 + b5,
        b4 + b3,
        b6 + b1,
        b6 - b1,
        b4 - b3,
        b2 - b5,
        b0 - b7,
    ]
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

/// Add a transformed residual to an 8-bit 8x8 block and clip to 0..=255.
pub fn add_residual_8x8(
    plane: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    x: usize,
    y: usize,
    residual: &[i32; 64],
    bit_depth: u8,
) -> Result<(), &'static str> {
    if bit_depth != 8 {
        return Err("use the u16 residual add for high-bit-depth output");
    }
    if stride < width || plane.len() < stride * height {
        return Err("pixel plane is smaller than its stride x height");
    }
    if x + 8 > width || y + 8 > height {
        return Err("8x8 residual block exceeds the visible plane");
    }
    let max = ((1u32 << bit_depth) - 1) as i32;
    for row in 0..8 {
        for column in 0..8 {
            let at = (y + row) * stride + x + column;
            let value = i32::from(plane[at]) + residual[row * 8 + column];
            plane[at] = value.clamp(0, max) as u8;
        }
    }
    Ok(())
}

/// Add an 8x8 residual to a 16-bit storage plane for 8/10-bit output.
pub fn add_residual_8x8_u16(
    plane: &mut [u16],
    width: usize,
    height: usize,
    stride: usize,
    x: usize,
    y: usize,
    residual: &[i32; 64],
    bit_depth: u8,
) -> Result<(), &'static str> {
    if !(8..=10).contains(&bit_depth) {
        return Err("H.264 reference add supports 8..=10 bit output");
    }
    if stride < width || plane.len() < stride * height {
        return Err("pixel plane is smaller than its stride x height");
    }
    if x + 8 > width || y + 8 > height {
        return Err("8x8 residual block exceeds the visible plane");
    }
    let max = ((1u32 << bit_depth) - 1) as i32;
    for row in 0..8 {
        for column in 0..8 {
            let at = (y + row) * stride + x + column;
            let value = i32::from(plane[at]) + residual[row * 8 + column];
            plane[at] = value.clamp(0, max) as u16;
        }
    }
    Ok(())
}

/// Scan order to raster position for a residual 4x4 block (`scan4x4`).
const ZIGZAG_4X4: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
const FIELD_4X4: [usize; 16] = [0, 4, 1, 8, 12, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];

/// Frame/field scans used by ordinary transformed and transform-bypass blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CavlcScan4x4 {
    Frame,
    Field,
    FrameQ0,
    FieldQ0,
}

const fn transpose_scan4(scan: [usize; 16]) -> [usize; 16] {
    let mut output = [0usize; 16];
    let mut position = 0;
    while position < 16 {
        let value = scan[position];
        output[position] = (value >> 2) | ((value << 2) & 0xf);
        position += 1;
    }
    output
}

const ZIGZAG_4X4_Q0: [usize; 16] = transpose_scan4(ZIGZAG_4X4);
const FIELD_4X4_Q0: [usize; 16] = transpose_scan4(FIELD_4X4);

/// Place quantised 4x4 levels from scan order into the raster coefficient block.
pub fn inverse_scan_4x4(scan: [i32; 16]) -> [i32; 16] {
    inverse_scan_4x4_layout(scan, CavlcScan4x4::Frame)
}

/// Place 4x4 levels from the selected frame/field and transformed/q0 scan.
pub fn inverse_scan_4x4_layout(scan: [i32; 16], layout: CavlcScan4x4) -> [i32; 16] {
    let table = match layout {
        CavlcScan4x4::Frame => &ZIGZAG_4X4,
        CavlcScan4x4::Field => &FIELD_4X4,
        CavlcScan4x4::FrameQ0 => &ZIGZAG_4X4_Q0,
        CavlcScan4x4::FieldQ0 => &FIELD_4X4_Q0,
    };
    let mut raster = [0i32; 16];
    for (position, value) in scan.into_iter().enumerate() {
        raster[table[position]] = value;
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
    dequant_4x4_layout(scan, qp, bit_depth, scaling, CavlcScan4x4::Frame)
}

/// H.264 4x4 dequantisation with an explicit frame/field scan.
pub fn dequant_4x4_layout(
    scan: [i32; 16],
    qp: u8,
    bit_depth: u8,
    scaling: Option<&[u8; 16]>,
    layout: CavlcScan4x4,
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

    let table = match layout {
        CavlcScan4x4::Frame => &ZIGZAG_4X4,
        CavlcScan4x4::Field => &FIELD_4X4,
        CavlcScan4x4::FrameQ0 | CavlcScan4x4::FieldQ0 => {
            return Err("transform-bypass q0 requires lossless reconstruction");
        }
    };
    let mut output = [0i32; 16];
    for (scan_pos, level) in scan.into_iter().enumerate() {
        let raster = table[scan_pos];
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
    FrameQ0,
    FieldQ0,
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

const fn transpose_scan8(scan: [usize; 64]) -> [usize; 64] {
    let mut output = [0usize; 64];
    let mut position = 0;
    while position < 64 {
        let value = scan[position];
        output[position] = (value >> 3) | ((value & 7) << 3);
        position += 1;
    }
    output
}

const ZIGZAG_8X8_CAVLC_Q0: [usize; 64] = transpose_scan8(ZIGZAG_8X8_CAVLC);
const FIELD_8X8_CAVLC_Q0: [usize; 64] = transpose_scan8(FIELD_8X8_CAVLC);

/// Place CAVLC 8x8 levels into a raster-order coefficient block.
///
/// `FrameQ0`/`FieldQ0` are the transform-bypass scans; they are kept distinct
/// from the qscale scans because FFmpeg selects a different internal table at
/// `qscale == 0`.
pub fn inverse_scan_8x8_cavlc(scan: [i32; 64], layout: CavlcScan8x8) -> [i32; 64] {
    let table = match layout {
        CavlcScan8x8::Frame => &ZIGZAG_8X8_CAVLC,
        CavlcScan8x8::Field => &FIELD_8X8_CAVLC,
        CavlcScan8x8::FrameQ0 => &ZIGZAG_8X8_CAVLC_Q0,
        CavlcScan8x8::FieldQ0 => &FIELD_8X8_CAVLC_Q0,
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
        CavlcScan8x8::FrameQ0 | CavlcScan8x8::FieldQ0 => {
            return Err("transform-bypass q0 requires lossless reconstruction")
        }
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

/// Map luma QP to chroma QP for the given chroma offset and bit depth.
pub fn chroma_qp(qp: i32, offset: i32, bit_depth: u8) -> Result<u8, &'static str> {
    if !(8..=10).contains(&bit_depth) {
        return Err("H.264 reference QP mapping supports 8..=10 bit");
    }
    let max_qp = 51 + 6 * i32::from(bit_depth - 8);
    if !(0..=max_qp).contains(&qp) {
        return Err("H.264 luma QP is outside the profile bit-depth limit");
    }
    let qpi = (qp + offset).clamp(0, max_qp);
    let mapped = if qpi < 30 {
        qpi
    } else if qpi <= 51 {
        const HIGH: [i32; 22] = [
            29, 30, 31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38, 39, 39, 39, 39,
        ];
        HIGH[(qpi - 30) as usize] + 6 * i32::from(bit_depth - 8)
    } else {
        39 + 6 * i32::from(bit_depth - 8)
    };
    Ok(mapped as u8)
}

fn dc_qmul_4x4(qp: u8, bit_depth: u8, scale: u8) -> Result<i32, &'static str> {
    if !(8..=10).contains(&bit_depth) {
        return Err("H.264 reference dequant supports 8..=10 bit");
    }
    let max_qp = 51 + 6 * u16::from(bit_depth - 8);
    if u16::from(qp) > max_qp {
        return Err("H.264 QP exceeds the profile bit-depth limit");
    }
    let shift = qp / 6 + 2;
    Ok((DEQUANT4[(qp % 6) as usize][0] * i32::from(scale)) << shift)
}

/// Luma Intra16x16 DC inverse scan, Hadamard, dequantisation and rounding.
pub fn dequant_luma_dc_4x4(
    scan: [i32; 16],
    qp: u8,
    bit_depth: u8,
    scaling_dc: u8,
    layout: CavlcScan4x4,
) -> Result<[i32; 16], &'static str> {
    if matches!(layout, CavlcScan4x4::FrameQ0 | CavlcScan4x4::FieldQ0) {
        return Err("transform-bypass q0 requires direct DC mapping");
    }
    let qmul = dc_qmul_4x4(qp, bit_depth, scaling_dc)?;
    let levels = inverse_scan_4x4_layout(scan, layout);
    let mut output = [0i32; 16];
    for (position, value) in inverse_luma_dc_4x4(levels).into_iter().enumerate() {
        let dequant = (i64::from(value) * i64::from(qmul) + 128) >> 8;
        output[position] =
            i32::try_from(dequant).map_err(|_| "luma DC dequantisation overflows i32")?;
    }
    Ok(output)
}

/// 4:2:0 chroma DC inverse Hadamard and dequantisation.
pub fn dequant_chroma_dc_420(
    scan: [i32; 4],
    qp: u8,
    bit_depth: u8,
    scaling_dc: u8,
) -> Result<[i32; 4], &'static str> {
    let qmul = dc_qmul_4x4(qp, bit_depth, scaling_dc)?;
    let mut output = [0i32; 4];
    for (position, value) in inverse_chroma_dc_2x2(scan).into_iter().enumerate() {
        let dequant = (i64::from(value) * i64::from(qmul)) >> 7;
        output[position] =
            i32::try_from(dequant).map_err(|_| "chroma DC dequantisation overflows i32")?;
    }
    Ok(output)
}

/// 4:2:2 chroma DC inverse transform and dequantisation.
pub fn dequant_chroma_dc_422(
    scan: [i32; 8],
    qp: u8,
    bit_depth: u8,
    scaling_dc: u8,
) -> Result<[i32; 8], &'static str> {
    let qmul = dc_qmul_4x4(qp, bit_depth, scaling_dc)?;
    const CHROMA422_DC_SCAN: [usize; 8] = [0, 2, 1, 4, 6, 3, 5, 7];
    let mut levels = [0i32; 8];
    for (position, value) in scan.into_iter().enumerate() {
        levels[CHROMA422_DC_SCAN[position]] = value;
    }
    let mut temp = [0i32; 8];
    for row in 0..4 {
        let a = levels[row * 2];
        let b = levels[row * 2 + 1];
        temp[row * 2] = a + b;
        temp[row * 2 + 1] = a - b;
    }
    let mut output = [0i32; 8];
    for column in 0..2 {
        let z0 = temp[column] + temp[4 + column];
        let z1 = temp[column] - temp[4 + column];
        let z2 = temp[2 + column] - temp[6 + column];
        let z3 = temp[2 + column] + temp[6 + column];
        for (row, value) in [z0 + z3, z1 + z2, z1 - z2, z0 - z3].into_iter().enumerate() {
            let dequant = (i64::from(value) * i64::from(qmul) + 128) >> 8;
            output[row * 2 + column] =
                i32::try_from(dequant).map_err(|_| "chroma DC dequantisation overflows i32")?;
        }
    }
    Ok(output)
}

/// Transform-bypass luma DC levels in sub-block order; no Hadamard is applied.
pub fn luma_dc_transform_bypass(
    scan: [i32; 16],
    layout: CavlcScan4x4,
) -> Result<[i32; 16], &'static str> {
    if !matches!(layout, CavlcScan4x4::FrameQ0 | CavlcScan4x4::FieldQ0) {
        return Err("H.264 bypass luma DC requires a q0 scan");
    }
    Ok(inverse_scan_4x4_layout(scan, layout))
}

/// Transform-bypass chroma DC values in block order; no Hadamard is applied.
pub fn chroma_dc_transform_bypass(
    scan: &[i32],
    layout: CavlcScan8x8,
) -> Result<Vec<i32>, &'static str> {
    match layout {
        CavlcScan8x8::FrameQ0 | CavlcScan8x8::FieldQ0 => match scan.len() {
            4 => Ok(scan.to_vec()),
            8 => {
                const CHROMA422_DC_SCAN: [usize; 8] = [0, 2, 1, 4, 6, 3, 5, 7];
                let mut levels = vec![0; 8];
                for (position, &value) in scan.iter().enumerate() {
                    levels[CHROMA422_DC_SCAN[position]] = value;
                }
                Ok(levels)
            }
            _ => Err("chroma DC transform bypass expects 4 or 8 coefficients"),
        },
        _ => Err("H.264 bypass chroma DC requires a q0 scan"),
    }
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
    fn inverse_8x8_dc_spreads_with_the_specification_rounding() {
        let mut coeffs = [0i32; 64];
        coeffs[0] = 4096;
        assert_eq!(inverse_8x8(coeffs), [64; 64]);
    }

    #[test]
    fn inverse_8x8_matches_the_ffmpeg_two_pass_integer_transform() {
        let mut coeffs = [0i32; 64];
        for (position, value) in [
            (0, 1024),
            (1, 700),
            (7, -300),
            (8, -500),
            (9, 400),
            (14, -250),
            (16, 300),
            (18, -200),
            (23, 125),
            (27, -90),
            (32, 160),
            (35, -70),
            (40, 210),
            (45, -45),
            (49, 180),
            (54, -120),
            (56, 95),
            (63, 150),
        ] {
            coeffs[position] = value;
        }
        assert_eq!(
            inverse_8x8(coeffs),
            [
                36, 50, 27, 33, 9, -9, -11, -20, 22, 24, 1, 17, -11, -5, -20, -28, 38, 38, 8, 18,
                1, -3, -10, -23, 29, 39, 7, 29, -9, 15, -9, 2, 27, 25, 21, 21, 0, 11, 3, 12, 25,
                31, 12, 29, -6, 17, -4, 7, 33, 27, 36, 31, 29, 43, 21, 34, 36, 31, 35, 39, 26, 41,
                20, 27,
            ]
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
        assert!(add_residual_8x8(&mut plane, 4, 4, 4, 0, 0, &[0; 64], 8).is_err());
    }

    #[test]
    fn eight_by_eight_residual_add_respects_stride_clipping_and_bit_depth() {
        let mut plane = vec![0u8; 12 * 10];
        plane.fill(5);
        let residual: [i32; 64] =
            std::array::from_fn(|position| if position % 3 == 0 { 255 } else { -10 });
        let before = plane.clone();
        add_residual_8x8(&mut plane, 10, 10, 12, 2, 1, &residual, 8).unwrap();
        for row in 0..8 {
            for column in 0..8 {
                let expected = if (row * 8 + column) % 3 == 0 { 255 } else { 0 };
                assert_eq!(plane[(row + 1) * 12 + column + 2], expected);
            }
        }
        for row in 0..10 {
            assert_eq!(
                &plane[row * 12..row * 12 + 2],
                &before[row * 12..row * 12 + 2]
            );
            assert_eq!(
                &plane[row * 12 + 10..row * 12 + 12],
                &before[row * 12 + 10..row * 12 + 12]
            );
        }

        let mut high = vec![1020u16; 64];
        add_residual_8x8_u16(&mut high, 8, 8, 8, 0, 0, &[8; 64], 10).unwrap();
        assert!(high.iter().all(|&sample| sample == 1023));
        assert!(add_residual_8x8_u16(&mut high, 8, 8, 8, 0, 0, &[0; 64], 11).is_err());
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
    fn transform_bypass_4x4_scans_use_the_q0_frame_and_field_tables() {
        assert_eq!(
            ZIGZAG_4X4_Q0,
            [0, 4, 1, 2, 5, 8, 12, 9, 6, 3, 7, 10, 13, 14, 11, 15]
        );
        assert_eq!(
            FIELD_4X4_Q0,
            [0, 1, 4, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
        let scan: [i32; 16] = std::array::from_fn(|position| position as i32 + 1);
        let frame = inverse_scan_4x4_layout(scan, CavlcScan4x4::FrameQ0);
        let field = inverse_scan_4x4_layout(scan, CavlcScan4x4::FieldQ0);
        assert_eq!(frame[..8], [1, 3, 4, 10, 2, 5, 9, 11]);
        assert_eq!(field[..8], [1, 2, 4, 5, 3, 6, 7, 8]);
    }

    #[test]
    fn transform_bypass_8x8_scan_is_distinct_from_the_qscale_scan() {
        let scan: [i32; 64] = std::array::from_fn(|position| position as i32 + 1);
        let qscale = inverse_scan_8x8_cavlc(scan, CavlcScan8x8::Frame);
        let q0 = inverse_scan_8x8_cavlc(scan, CavlcScan8x8::FrameQ0);
        assert_eq!(qscale[12], 5);
        assert_eq!(qscale[33], 53);
        assert_eq!(q0[12], 53);
        assert_eq!(q0[33], 5);
        assert_ne!(qscale, q0);
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
    fn chroma_qp_mapping_matches_the_specification_table() {
        assert_eq!(chroma_qp(29, 0, 8), Ok(29));
        assert_eq!(chroma_qp(30, 0, 8), Ok(29));
        assert_eq!(chroma_qp(31, 0, 8), Ok(30));
        assert_eq!(chroma_qp(36, 0, 8), Ok(34));
        assert_eq!(chroma_qp(51, 0, 8), Ok(39));
        assert_eq!(chroma_qp(63, 0, 10), Ok(51));
        assert_eq!(chroma_qp(12, -20, 8), Ok(0));
        assert!(chroma_qp(52, 0, 8).is_err());
    }

    #[test]
    fn dc_dequantisation_matches_ffmpeg_rounding() {
        let mut dc = [0i32; 16];
        dc[0] = 1;
        assert_eq!(
            dequant_luma_dc_4x4(dc, 0, 8, 16, CavlcScan4x4::Frame).unwrap(),
            [3; 16]
        );
        assert_eq!(
            dequant_chroma_dc_420([1; 4], 0, 8, 16).unwrap(),
            [20, 0, 0, 0]
        );
        assert_eq!(
            dequant_chroma_dc_422([1; 8], 0, 8, 16).unwrap(),
            [20, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn transform_bypass_dc_mapping_uses_block_order() {
        let scan: [i32; 16] = std::array::from_fn(|position| position as i32 + 1);
        assert_eq!(
            luma_dc_transform_bypass(scan, CavlcScan4x4::FrameQ0).unwrap(),
            [1, 3, 4, 10, 2, 5, 9, 11, 6, 8, 12, 15, 7, 13, 14, 16]
        );
        let scan = [1i32, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(
            chroma_dc_transform_bypass(&scan, CavlcScan8x8::FrameQ0).unwrap(),
            [1, 3, 2, 6, 4, 7, 5, 8]
        );
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
            for layout in [CavlcScan4x4::Frame, CavlcScan4x4::Field] {
                let actual = dequant_4x4_layout(levels, qp, 8, Some(&scaling), layout).unwrap();
                let expected = reference_dequant_4x4(levels, qp, &scaling, layout);
                assert_eq!(actual, expected, "QP {qp}, layout {layout:?}");
            }
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
        assert!(dequant_8x8_cavlc(levels, 0, 8, None, CavlcScan8x8::FrameQ0).is_err());
        assert!(dequant_4x4_layout([1; 16], 0, 8, None, CavlcScan4x4::FrameQ0).is_err());
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
            CavlcScan8x8::FrameQ0 | CavlcScan8x8::FieldQ0 => unreachable!("test uses qscale"),
        };
        let mut output = [0i32; 64];
        for (scan_position, level) in levels.into_iter().enumerate() {
            let raster = table[scan_position];
            output[raster] = (level * qmul[raster] + 32) >> 6;
        }
        output
    }

    fn reference_dequant_4x4(
        levels: [i32; 16],
        qp: u8,
        scaling: &[u8; 16],
        layout: CavlcScan4x4,
    ) -> [i32; 16] {
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
        let table = match layout {
            CavlcScan4x4::Frame => &ZIGZAG_4X4,
            CavlcScan4x4::Field => &FIELD_4X4,
            CavlcScan4x4::FrameQ0 | CavlcScan4x4::FieldQ0 => unreachable!("test uses qscale"),
        };
        let mut output = [0i32; 16];
        for (scan_position, level) in levels.into_iter().enumerate() {
            let raster = table[scan_position];
            output[raster] = (level * qmul[raster] + 32) >> 6;
        }
        output
    }
}
