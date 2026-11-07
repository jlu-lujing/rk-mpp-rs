//! AV1 local warped-motion parameter estimation and affine prediction.

use crate::reference::av1::inter::MotionVector;
use crate::reference::av1::partition::BlockSize;
use crate::reference::av1::warped_filter::WARPED_FILTER;

const WARPEDMODEL_PREC_BITS: u32 = 16;
const WARPEDDIFF_PREC_BITS: u32 = WARPEDMODEL_PREC_BITS - 6;
const WARP_PARAM_REDUCE_BITS: u32 = 6;
const WARPEDMODEL_TRANS_CLAMP: i64 = 128 << WARPEDMODEL_PREC_BITS;
const WARPEDMODEL_NONDIAGAFFINE_CLAMP: i64 = 1 << (WARPEDMODEL_PREC_BITS - 3);
const LS_MV_MAX: i32 = 256;
const LS_STEP: i32 = 8;
const LS_MAT_DOWN_BITS: u32 = 2;
const LEAST_SQUARES_SAMPLES_MAX: usize = 8;
const FILTER_BITS: u32 = 7;
const ROUND0_BITS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProjectionSample {
    pub source: [i32; 2],
    pub destination: [i32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarpParams {
    pub matrix: [i32; 6],
    pub alpha: i16,
    pub beta: i16,
    pub gamma: i16,
    pub delta: i16,
}

impl WarpParams {
    pub(crate) fn from_matrix(matrix: [i32; 6]) -> Option<Self> {
        let (alpha, beta, gamma, delta) = shear_params(matrix)?;
        Some(Self {
            matrix,
            alpha,
            beta,
            gamma,
            delta,
        })
    }
}

fn round_power_of_two(value: i64, shift: u32) -> i64 {
    if shift == 0 {
        value
    } else {
        (value + (1i64 << (shift - 1))) >> shift
    }
}

fn round_power_of_two_signed(value: i64, shift: u32) -> i64 {
    if value < 0 {
        -round_power_of_two(value.unsigned_abs() as i64, shift)
    } else {
        round_power_of_two(value, shift)
    }
}

fn floor_log2(value: u64) -> i32 {
    debug_assert!(value > 0);
    63 - value.leading_zeros() as i32
}

fn div_lut(index: usize) -> i16 {
    debug_assert!(index <= 256);
    let denominator = (256 + index) as i64;
    (((1i64 << 22) + denominator / 2) / denominator) as i16
}

fn resolve_divisor_64(divisor: u64) -> (i16, i32) {
    debug_assert!(divisor > 0);
    let shift = floor_log2(divisor);
    let residual = divisor - (1u64 << shift);
    let fraction = if shift > 8 {
        round_power_of_two(residual as i64, (shift - 8) as u32) as usize
    } else {
        (residual << (8 - shift)) as usize
    };
    (div_lut(fraction), shift + 14)
}

fn resolve_divisor_32(divisor: u32) -> (i16, i32) {
    debug_assert!(divisor > 0);
    let shift = floor_log2(u64::from(divisor));
    let residual = i64::from(divisor) - (1i64 << shift);
    let fraction = if shift > 8 {
        round_power_of_two(residual, (shift - 8) as u32) as usize
    } else {
        (residual << (8 - shift)) as usize
    };
    (div_lut(fraction), shift + 14)
}

fn clamp_i16(value: i64) -> i16 {
    value.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

fn shear_params(matrix: [i32; 6]) -> Option<(i16, i16, i16, i16)> {
    if matrix[2] <= 0 {
        return None;
    }
    let mut alpha = clamp_i16(i64::from(matrix[2]) - (1i64 << WARPEDMODEL_PREC_BITS));
    let mut beta = clamp_i16(i64::from(matrix[3]));
    let (divisor, shift) = resolve_divisor_32(matrix[2].unsigned_abs());
    let divisor = i64::from(divisor) * if matrix[2] < 0 { -1 } else { 1 };

    let value = i64::from(matrix[4]) * (1i64 << WARPEDMODEL_PREC_BITS) * divisor;
    let mut gamma = clamp_i16(round_power_of_two_signed(value, shift as u32));
    let value = i64::from(matrix[3]) * i64::from(matrix[4]) * divisor;
    let mut delta = clamp_i16(
        i64::from(matrix[5])
            - round_power_of_two_signed(value, shift as u32)
            - (1i64 << WARPEDMODEL_PREC_BITS),
    );

    alpha = (round_power_of_two_signed(i64::from(alpha), WARP_PARAM_REDUCE_BITS)
        * (1i64 << WARP_PARAM_REDUCE_BITS)) as i16;
    beta = (round_power_of_two_signed(i64::from(beta), WARP_PARAM_REDUCE_BITS)
        * (1i64 << WARP_PARAM_REDUCE_BITS)) as i16;
    gamma = (round_power_of_two_signed(i64::from(gamma), WARP_PARAM_REDUCE_BITS)
        * (1i64 << WARP_PARAM_REDUCE_BITS)) as i16;
    delta = (round_power_of_two_signed(i64::from(delta), WARP_PARAM_REDUCE_BITS)
        * (1i64 << WARP_PARAM_REDUCE_BITS)) as i16;

    if 4 * i64::from(alpha.abs()) + 7 * i64::from(beta.abs()) >= (1i64 << WARPEDMODEL_PREC_BITS)
        || 4 * i64::from(gamma.abs()) + 4 * i64::from(delta.abs())
            >= (1i64 << WARPEDMODEL_PREC_BITS)
    {
        return None;
    }
    Some((alpha, beta, gamma, delta))
}

fn ls_square(value: i32) -> i64 {
    let value = i64::from(value);
    (value * value * 4 + value * 4 * i64::from(LS_STEP) + i64::from(LS_STEP).pow(2) * 2)
        >> (2 + LS_MAT_DOWN_BITS)
}

fn ls_product1(left: i32, right: i32) -> i64 {
    let left = i64::from(left);
    let right = i64::from(right);
    (left * right * 4 + (left + right) * 2 * i64::from(LS_STEP) + i64::from(LS_STEP).pow(2))
        >> (2 + LS_MAT_DOWN_BITS)
}

fn ls_product2(left: i32, right: i32) -> i64 {
    let left = i64::from(left);
    let right = i64::from(right);
    (left * right * 4 + (left + right) * 2 * i64::from(LS_STEP) + i64::from(LS_STEP).pow(2) * 2)
        >> (2 + LS_MAT_DOWN_BITS)
}

fn mult_shift_non_diagonal(value: i64, inverse_det: i64, shift: i32) -> i32 {
    round_power_of_two_signed(value * inverse_det, shift.max(0) as u32).clamp(
        -WARPEDMODEL_NONDIAGAFFINE_CLAMP + 1,
        WARPEDMODEL_NONDIAGAFFINE_CLAMP - 1,
    ) as i32
}

fn mult_shift_diagonal(value: i64, inverse_det: i64, shift: i32) -> i32 {
    round_power_of_two_signed(value * inverse_det, shift.max(0) as u32).clamp(
        (1i64 << WARPEDMODEL_PREC_BITS) - WARPEDMODEL_NONDIAGAFFINE_CLAMP + 1,
        (1i64 << WARPEDMODEL_PREC_BITS) + WARPEDMODEL_NONDIAGAFFINE_CLAMP - 1,
    ) as i32
}

pub(crate) fn select_samples(
    samples: &mut Vec<ProjectionSample>,
    mv: MotionVector,
    size: BlockSize,
) {
    let threshold = i32::try_from(size.width.max(size.height))
        .unwrap_or(i32::MAX)
        .clamp(16, 112);
    let mut retained = 0usize;
    for index in 0..samples.len() {
        let sample = samples[index];
        let difference = (sample.destination[0] - sample.source[0] - mv.col).abs()
            + (sample.destination[1] - sample.source[1] - mv.row).abs();
        if difference > threshold {
            continue;
        }
        if retained != index {
            samples[retained] = sample;
        }
        retained += 1;
    }
    samples.truncate(retained.max(1));
}

pub(crate) fn find_projection(
    samples: &[ProjectionSample],
    size: BlockSize,
    mv: MotionVector,
    mi_row: usize,
    mi_col: usize,
) -> Option<WarpParams> {
    if samples.is_empty() {
        return None;
    }
    let width = i32::try_from(size.width).ok()?;
    let height = i32::try_from(size.height).ok()?;
    let rsuy = height / 2 - 1;
    let rsux = width / 2 - 1;
    let source_center_y = rsuy * 8;
    let source_center_x = rsux * 8;
    let destination_center_y = source_center_y + mv.row;
    let destination_center_x = source_center_x + mv.col;

    let mut matrix_a = [[0i64; 2]; 2];
    let mut bx = [0i64; 2];
    let mut by = [0i64; 2];
    for sample in samples.iter().take(LEAST_SQUARES_SAMPLES_MAX) {
        let dx = sample.destination[0] - destination_center_x;
        let dy = sample.destination[1] - destination_center_y;
        let sx = sample.source[0] - source_center_x;
        let sy = sample.source[1] - source_center_y;
        if (sx - dx).abs() < LS_MV_MAX && (sy - dy).abs() < LS_MV_MAX {
            matrix_a[0][0] += ls_square(sx);
            matrix_a[0][1] += ls_product1(sx, sy);
            matrix_a[1][1] += ls_square(sy);
            bx[0] += ls_product2(sx, dx);
            bx[1] += ls_product1(sy, dx);
            by[0] += ls_product1(sx, dy);
            by[1] += ls_product2(sy, dy);
        }
    }

    let determinant = matrix_a[0][0] * matrix_a[1][1] - matrix_a[0][1] * matrix_a[0][1];
    if determinant == 0 {
        return None;
    }
    let (inverse, mut shift) = resolve_divisor_64(determinant.unsigned_abs());
    let mut inverse = i64::from(inverse) * if determinant < 0 { -1 } else { 1 };
    shift -= WARPEDMODEL_PREC_BITS as i32;
    if shift < 0 {
        inverse <<= -shift;
        shift = 0;
    }

    let px = [
        matrix_a[1][1] * bx[0] - matrix_a[0][1] * bx[1],
        -matrix_a[0][1] * bx[0] + matrix_a[0][0] * bx[1],
    ];
    let py = [
        matrix_a[1][1] * by[0] - matrix_a[0][1] * by[1],
        -matrix_a[0][1] * by[0] + matrix_a[0][0] * by[1],
    ];
    let mut matrix = [0i32; 6];
    matrix[2] = mult_shift_diagonal(px[0], inverse, shift);
    matrix[3] = mult_shift_non_diagonal(px[1], inverse, shift);
    matrix[4] = mult_shift_non_diagonal(py[0], inverse, shift);
    matrix[5] = mult_shift_diagonal(py[1], inverse, shift);

    let integer_center_y = i64::try_from(mi_row).ok()? * 4 + i64::from(rsuy);
    let integer_center_x = i64::try_from(mi_col).ok()? * 4 + i64::from(rsux);
    let vx = i64::from(mv.col) * (1i64 << (WARPEDMODEL_PREC_BITS - 3))
        - (integer_center_x * (i64::from(matrix[2]) - (1i64 << WARPEDMODEL_PREC_BITS))
            + integer_center_y * i64::from(matrix[3]));
    let vy = i64::from(mv.row) * (1i64 << (WARPEDMODEL_PREC_BITS - 3))
        - (integer_center_x * i64::from(matrix[4])
            + integer_center_y * (i64::from(matrix[5]) - (1i64 << WARPEDMODEL_PREC_BITS)));
    matrix[0] = vx.clamp(-WARPEDMODEL_TRANS_CLAMP, WARPEDMODEL_TRANS_CLAMP - 1) as i32;
    matrix[1] = vy.clamp(-WARPEDMODEL_TRANS_CLAMP, WARPEDMODEL_TRANS_CLAMP - 1) as i32;
    WarpParams::from_matrix(matrix)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn warp_block_samples(
    reference_data: &[u16],
    reference_stride: usize,
    reference_width: usize,
    reference_height: usize,
    params: WarpParams,
    plane_col: usize,
    plane_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
) -> Result<Vec<u16>, &'static str> {
    let res = warp_block_res(
        reference_data,
        reference_stride,
        reference_width,
        reference_height,
        params,
        plane_col,
        plane_row,
        width,
        height,
        sub_x,
        sub_y,
        bit_depth,
        None,
    )?;
    let max = (1i64 << bit_depth) - 1;
    let rounding_offset = (1i64 << (bit_depth - 1)) + (1i64 << bit_depth);
    Ok(res
        .into_iter()
        .map(|value| (value - rounding_offset).clamp(0, max) as u16)
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn warp_block_res(
    reference_data: &[u16],
    reference_stride: usize,
    reference_width: usize,
    reference_height: usize,
    params: WarpParams,
    plane_col: usize,
    plane_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
    compound_round1: Option<u32>,
) -> Result<Vec<i64>, &'static str> {
    if width == 0 || height == 0 || reference_width == 0 || reference_height == 0 {
        return Ok(Vec::new());
    }
    let reduce_bits_horiz =
        ROUND0_BITS + (bit_depth + FILTER_BITS - ROUND0_BITS).saturating_sub(14);
    let reduce_bits_vert = compound_round1.unwrap_or(2 * FILTER_BITS - reduce_bits_horiz);
    let offset_bits_horiz = bit_depth + FILTER_BITS - 1;
    let offset_bits_vert = bit_depth + 2 * FILTER_BITS - reduce_bits_horiz;
    let matrix = params.matrix.map(i64::from);
    let alpha = i64::from(params.alpha);
    let beta = i64::from(params.beta);
    let gamma = i64::from(params.gamma);
    let delta = i64::from(params.delta);
    let mut output = vec![0i64; width * height];

    let mut block_row = plane_row;
    while block_row < plane_row + height {
        let mut block_col = plane_col;
        while block_col < plane_col + width {
            let source_x = ((block_col + 4) as i64) << sub_x;
            let source_y = ((block_row + 4) as i64) << sub_y;
            let destination_x = matrix[2] * source_x + matrix[3] * source_y + matrix[0];
            let destination_y = matrix[4] * source_x + matrix[5] * source_y + matrix[1];
            let x = destination_x >> sub_x;
            let y = destination_y >> sub_y;
            let integer_x = x >> WARPEDMODEL_PREC_BITS;
            let mut fraction_x = x & ((1i64 << WARPEDMODEL_PREC_BITS) - 1);
            let integer_y = y >> WARPEDMODEL_PREC_BITS;
            let mut fraction_y = y & ((1i64 << WARPEDMODEL_PREC_BITS) - 1);
            fraction_x += -4 * alpha - 4 * beta;
            fraction_y += -4 * gamma - 4 * delta;
            fraction_x &= !((1i64 << WARP_PARAM_REDUCE_BITS) - 1);
            fraction_y &= !((1i64 << WARP_PARAM_REDUCE_BITS) - 1);

            let mut intermediate = [[0i64; 8]; 15];
            for k in -7i64..8 {
                let source_row =
                    (integer_y + k).clamp(0, reference_height.saturating_sub(1) as i64) as usize;
                let mut source_fraction = fraction_x + beta * (k + 4);
                for l in -4i64..4 {
                    let source_column = integer_x + l - 3;
                    let phase = round_power_of_two(source_fraction, WARPEDDIFF_PREC_BITS) + 64;
                    if !(0..=192).contains(&phase) {
                        return Err("warp filter phase");
                    }
                    let coefficients = &WARPED_FILTER[phase as usize];
                    let mut sum = 1i64 << offset_bits_horiz;
                    for (tap, coefficient) in coefficients.iter().enumerate() {
                        let sample_column = (source_column + tap as i64)
                            .clamp(0, reference_width.saturating_sub(1) as i64)
                            as usize;
                        sum += i64::from(
                            reference_data[source_row * reference_stride + sample_column],
                        ) * i64::from(*coefficient);
                    }
                    intermediate[(k + 7) as usize][(l + 4) as usize] =
                        round_power_of_two_signed(sum, reduce_bits_horiz);
                    source_fraction += alpha;
                }
            }

            let vertical_end = (4i64).min((plane_row + height) as i64 - block_row as i64 - 4);
            let horizontal_end = (4i64).min((plane_col + width) as i64 - block_col as i64 - 4);
            let mut k = -4i64;
            while k < vertical_end {
                let mut source_fraction = fraction_y + delta * (k + 4);
                let mut l = -4i64;
                while l < horizontal_end {
                    let phase = round_power_of_two(source_fraction, WARPEDDIFF_PREC_BITS) + 64;
                    if !(0..=192).contains(&phase) {
                        return Err("warp filter phase");
                    }
                    let coefficients = &WARPED_FILTER[phase as usize];
                    let mut sum = 1i64 << offset_bits_vert;
                    for (tap, coefficient) in coefficients.iter().enumerate() {
                        sum += intermediate[(k + tap as i64 + 4) as usize][(l + 4) as usize]
                            * i64::from(*coefficient);
                    }
                    sum = round_power_of_two_signed(sum, reduce_bits_vert);
                    let output_row = (block_row as i64 - plane_row as i64 + k + 4) as usize;
                    let output_col = (block_col as i64 - plane_col as i64 + l + 4) as usize;
                    output[output_row * width + output_col] = sum;
                    source_fraction += gamma;
                    l += 1;
                }
                k += 1;
            }
            block_col += 8;
        }
        block_row += 8;
    }
    Ok(output)
}
