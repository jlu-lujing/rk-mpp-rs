//! AV1 film-grain image generation for the RK3588 `AV1D_VPU`.
//!
//! The hardware consumes three scaling lookup tables and two cropped grain
//! blocks. This module ports the exact integer sequence used by MPP's
//! `vdpu_av1d_set_fgs()` and `film_grain_noise_table.c`.

use crate::codec::av1::picparams::{Av1PicParams, FilmGrain};

use super::film_grain_table::GAUSSIAN_SEQUENCE;

pub const LUMA_BLOCK_WIDTH: usize = 82;
pub const LUMA_BLOCK_HEIGHT: usize = 73;
pub const CHROMA_BLOCK_WIDTH: usize = 44;
pub const CHROMA_BLOCK_HEIGHT: usize = 38;
pub const FILM_GRAIN_MEMORY_SIZE: usize = 3 * 256 + 2 * (64 * 64) + 2 * (32 * 64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmGrainError {
    UnsupportedBitDepth(u16),
    InvalidScalingPoints,
}

/// Build the exact 13,056-byte `AV1FilmGrainMemory` image consumed by AV1D.
pub fn build_film_grain_memory(
    params: &Av1PicParams,
) -> Result<[u8; FILM_GRAIN_MEMORY_SIZE], FilmGrainError> {
    if !matches!(params.bitdepth, 8..=12) {
        return Err(FilmGrainError::UnsupportedBitDepth(params.bitdepth));
    }
    let grain = &params.film_grain;
    if grain.apply_grain == 0 {
        return Ok([0; FILM_GRAIN_MEMORY_SIZE]);
    }

    let mut luma_lut = [0u8; 256];
    let mut cb_lut = [0u8; 256];
    let mut cr_lut = [0u8; 256];
    init_scaling_function(&grain.scaling_points_y, grain.num_y_points, &mut luma_lut)?;
    if grain.chroma_scaling_from_luma != 0 {
        cb_lut = luma_lut;
        cr_lut = luma_lut;
    } else {
        init_scaling_function(&grain.scaling_points_cb, grain.num_cb_points, &mut cb_lut)?;
        init_scaling_function(&grain.scaling_points_cr, grain.num_cr_points, &mut cr_lut)?;
    }

    let mut ar_coeffs_y = [0i32; 24];
    let mut ar_coeffs_cb = [0i32; 25];
    let mut ar_coeffs_cr = [0i32; 25];
    for index in 0..25 {
        if index < ar_coeffs_y.len() {
            ar_coeffs_y[index] = i32::from(grain.ar_coeffs_y[index]) - 128;
        }
        ar_coeffs_cb[index] = i32::from(grain.ar_coeffs_cb[index]) - 128;
        ar_coeffs_cr[index] = i32::from(grain.ar_coeffs_cr[index]) - 128;
    }

    let bit_depth = i32::from(params.bitdepth);
    let grain_center = 128 << (bit_depth - 8);
    let grain_min = -grain_center;
    let grain_max = (256 << (bit_depth - 8)) - 1 - grain_center;
    let luma = generate_luma_block(grain, bit_depth, &ar_coeffs_y, grain_min, grain_max);
    let (cb, cr) = generate_chroma_blocks(
        &luma,
        grain,
        bit_depth,
        &ar_coeffs_cb,
        &ar_coeffs_cr,
        grain_min,
        grain_max,
    );

    let mut output = [0u8; FILM_GRAIN_MEMORY_SIZE];
    output[0..256].copy_from_slice(&luma_lut);
    output[256..512].copy_from_slice(&cb_lut);
    output[512..768].copy_from_slice(&cr_lut);

    let mut offset = 768;
    for y in 0..64 {
        for x in 0..64 {
            offset += 2;
            output[offset - 2..offset].copy_from_slice(&(luma[y + 9][x + 9] as i16).to_le_bytes());
        }
    }
    for y in 0..32 {
        for x in 0..32 {
            offset += 2;
            output[offset - 2..offset].copy_from_slice(&(cb[y + 6][x + 6] as i16).to_le_bytes());
            offset += 2;
            output[offset - 2..offset].copy_from_slice(&(cr[y + 6][x + 6] as i16).to_le_bytes());
        }
    }
    debug_assert_eq!(offset, FILM_GRAIN_MEMORY_SIZE);
    Ok(output)
}

fn init_scaling_function(
    points: &[[u8; 2]],
    count: u8,
    lut: &mut [u8; 256],
) -> Result<(), FilmGrainError> {
    let count = usize::from(count);
    if count > points.len() {
        return Err(FilmGrainError::InvalidScalingPoints);
    }
    lut.fill(0);
    if count == 0 {
        return Ok(());
    }

    lut[..usize::from(points[0][0])].fill(points[0][1]);
    for point in 0..count - 1 {
        let x0 = usize::from(points[point][0]);
        let x1 = usize::from(points[point + 1][0]);
        if x1 <= x0 || x1 > lut.len() {
            return Err(FilmGrainError::InvalidScalingPoints);
        }
        let delta_y = i32::from(points[point + 1][1]) - i32::from(points[point][1]);
        let delta_x = i32::from(points[point + 1][0]) - i32::from(points[point][0]);
        let delta = if delta_x == 0 {
            0
        } else {
            i64::from(delta_y) * i64::from((65_536 + (delta_x >> 1)) / delta_x)
        };
        for x in 0..usize::from(points[point + 1][0] - points[point][0]) {
            lut[x0 + x] =
                (i32::from(points[point][1]) + ((x as i64 * delta + 32_768) >> 16) as i32) as u8;
        }
    }
    let last = usize::from(points[count - 1][0]);
    if last > lut.len() {
        return Err(FilmGrainError::InvalidScalingPoints);
    }
    lut[last..].fill(points[count - 1][1]);
    Ok(())
}

fn generate_luma_block(
    grain: &FilmGrain,
    bit_depth: i32,
    ar_coeffs: &[i32; 24],
    grain_min: i32,
    grain_max: i32,
) -> [[i32; LUMA_BLOCK_WIDTH]; LUMA_BLOCK_HEIGHT] {
    let gauss_shift = 12 - bit_depth + i32::from(grain.grain_scale_shift);
    let mut random = u16::from(grain.grain_seed);
    let mut block = [[0i32; LUMA_BLOCK_WIDTH]; LUMA_BLOCK_HEIGHT];
    for row in &mut block {
        for value in row {
            if grain.num_y_points > 0 {
                update_random_register(&mut random);
                *value = round_power_of_two(
                    i32::from(GAUSSIAN_SEQUENCE[random_number(random)]),
                    gauss_shift,
                );
            }
        }
    }

    let lag = usize::from(grain.ar_coeff_lag);
    let coeff_shift = i32::from(grain.ar_coeff_shift_minus6) + 6;
    for y in 3..LUMA_BLOCK_HEIGHT {
        for x in 3..LUMA_BLOCK_WIDTH - 3 {
            let weight = autoregressive_weight(&block, y, x, lag, ar_coeffs);
            block[y][x] = round_power_of_two(weight, coeff_shift) + block[y][x];
            block[y][x] = block[y][x].clamp(grain_min, grain_max);
        }
    }
    block
}

#[allow(clippy::too_many_arguments)]
fn generate_chroma_blocks(
    luma: &[[i32; LUMA_BLOCK_WIDTH]; LUMA_BLOCK_HEIGHT],
    grain: &FilmGrain,
    bit_depth: i32,
    ar_coeffs_cb: &[i32; 25],
    ar_coeffs_cr: &[i32; 25],
    grain_min: i32,
    grain_max: i32,
) -> (
    [[i32; CHROMA_BLOCK_WIDTH]; CHROMA_BLOCK_HEIGHT],
    [[i32; CHROMA_BLOCK_WIDTH]; CHROMA_BLOCK_HEIGHT],
) {
    let gauss_shift = 12 - bit_depth + i32::from(grain.grain_scale_shift);
    let mut cb = [[0i32; CHROMA_BLOCK_WIDTH]; CHROMA_BLOCK_HEIGHT];
    let mut cr = [[0i32; CHROMA_BLOCK_WIDTH]; CHROMA_BLOCK_HEIGHT];

    let mut random = init_random_generator(7, grain.grain_seed);
    for row in &mut cb {
        for value in row {
            if grain.num_cb_points != 0 || grain.chroma_scaling_from_luma != 0 {
                update_random_register(&mut random);
                *value = round_power_of_two(
                    i32::from(GAUSSIAN_SEQUENCE[random_number(random)]),
                    gauss_shift,
                );
            }
        }
    }

    random = init_random_generator(11, grain.grain_seed);
    for row in &mut cr {
        for value in row {
            if grain.num_cr_points != 0 || grain.chroma_scaling_from_luma != 0 {
                update_random_register(&mut random);
                *value = round_power_of_two(
                    i32::from(GAUSSIAN_SEQUENCE[random_number(random)]),
                    gauss_shift,
                );
            }
        }
    }

    let lag = usize::from(grain.ar_coeff_lag);
    let coeff_shift = i32::from(grain.ar_coeff_shift_minus6) + 6;
    for y in 3..CHROMA_BLOCK_HEIGHT {
        for x in 3..CHROMA_BLOCK_WIDTH - 3 {
            let pos = 2 * lag * (lag + 1);
            let mut cb_weight = autoregressive_weight(&cb, y, x, lag, ar_coeffs_cb);
            let mut cr_weight = autoregressive_weight(&cr, y, x, lag, ar_coeffs_cr);
            if grain.num_y_points > 0 {
                let luma_y = (y << 1) - 3;
                let luma_x = (x << 1) - 3;
                let average = round_power_of_two(
                    luma[luma_y][luma_x]
                        + luma[luma_y][luma_x + 1]
                        + luma[luma_y + 1][luma_x]
                        + luma[luma_y + 1][luma_x + 1],
                    2,
                );
                cb_weight += ar_coeffs_cb[pos] * average;
                cr_weight += ar_coeffs_cr[pos] * average;
            }
            if grain.num_cb_points != 0 || grain.chroma_scaling_from_luma != 0 {
                cb[y][x] = (cb[y][x] + round_power_of_two(cb_weight, coeff_shift))
                    .clamp(grain_min, grain_max);
            }
            if grain.num_cr_points != 0 || grain.chroma_scaling_from_luma != 0 {
                cr[y][x] = (cr[y][x] + round_power_of_two(cr_weight, coeff_shift))
                    .clamp(grain_min, grain_max);
            }
        }
    }
    (cb, cr)
}

fn autoregressive_weight<const W: usize, const H: usize>(
    block: &[[i32; W]; H],
    y: usize,
    x: usize,
    lag: usize,
    coefficients: &[i32],
) -> i32 {
    let mut weight = 0i32;
    let mut pos = 0usize;
    for delta_row in -(lag as isize)..=0 {
        for delta_col in -(lag as isize)..=lag as isize {
            if delta_row == 0 && delta_col == 0 {
                break;
            }
            weight += coefficients[pos]
                * block[(y as isize + delta_row) as usize][(x as isize + delta_col) as usize];
            pos += 1;
        }
    }
    weight
}

fn init_random_generator(luma_num: u16, seed: u16) -> u16 {
    let mut random = seed;
    random ^= ((luma_num * 37 + 178) & 255) << 8;
    random ^= (luma_num * 173 + 105) & 255;
    random
}

fn update_random_register(random: &mut u16) {
    let bit = (*random ^ (*random >> 1) ^ (*random >> 3) ^ (*random >> 12)) & 1;
    *random = (*random >> 1) | (bit << 15);
}

fn random_number(random: u16) -> usize {
    usize::from((random >> 5) & ((1 << 11) - 1))
}

fn round_power_of_two(value: i32, shift: i32) -> i32 {
    (value + (1 << (shift - 1))) >> shift
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c_probe_cases() -> [Av1PicParams; 3] {
        let mut base = Av1PicParams::default();
        base.bitdepth = 8;
        base.film_grain.apply_grain = 1;
        base.film_grain.num_y_points = 3;
        base.film_grain.num_cb_points = 2;
        base.film_grain.num_cr_points = 2;
        base.film_grain.ar_coeff_lag = 2;
        base.film_grain.ar_coeff_shift_minus6 = 1;
        base.film_grain.grain_scale_shift = 0;
        base.film_grain.grain_seed = 7391;
        base.film_grain.scaling_points_y[0] = [0, 12];
        base.film_grain.scaling_points_y[1] = [96, 80];
        base.film_grain.scaling_points_y[2] = [255, 35];
        base.film_grain.scaling_points_cb[0] = [8, 24];
        base.film_grain.scaling_points_cb[1] = [240, 67];
        base.film_grain.scaling_points_cr[0] = [16, 64];
        base.film_grain.scaling_points_cr[1] = [220, 18];
        for index in 0..24 {
            base.film_grain.ar_coeffs_y[index] = (128 + index % 9 - 4) as u8;
        }
        for index in 0..25 {
            base.film_grain.ar_coeffs_cb[index] = (128 + index % 7 - 3) as u8;
            base.film_grain.ar_coeffs_cr[index] = (128 + index % 5 - 2) as u8;
        }

        let mut ten_bit = base;
        ten_bit.bitdepth = 10;
        ten_bit.film_grain.num_cb_points = 0;
        ten_bit.film_grain.num_cr_points = 0;
        ten_bit.film_grain.chroma_scaling_from_luma = 1;
        ten_bit.film_grain.ar_coeff_lag = 3;
        ten_bit.film_grain.ar_coeff_shift_minus6 = 2;
        ten_bit.film_grain.grain_scale_shift = 2;
        ten_bit.film_grain.grain_seed = 0xa53c;

        let mut twelve_bit = base;
        twelve_bit.bitdepth = 12;
        twelve_bit.film_grain.num_y_points = 0;
        twelve_bit.film_grain.num_cb_points = 1;
        twelve_bit.film_grain.num_cr_points = 1;
        twelve_bit.film_grain.ar_coeff_lag = 1;
        twelve_bit.film_grain.ar_coeff_shift_minus6 = 0;
        twelve_bit.film_grain.grain_scale_shift = 3;
        twelve_bit.film_grain.grain_seed = 0x1234;

        [base, ten_bit, twelve_bit]
    }

    #[test]
    fn complete_memory_images_match_the_c_mpp_noise_generator() {
        let expected: [&[u8]; 3] = [
            include_bytes!("../../../tests/data/av1_film_grain/film_grain_00.bin"),
            include_bytes!("../../../tests/data/av1_film_grain/film_grain_01.bin"),
            include_bytes!("../../../tests/data/av1_film_grain/film_grain_02.bin"),
        ];

        for (index, (params, expected)) in c_probe_cases().iter().zip(expected).enumerate() {
            let actual = build_film_grain_memory(params).unwrap();
            assert_eq!(
                actual.as_slice(),
                expected,
                "film-grain image {index} differs from the C MPP generator"
            );
        }
    }

    #[test]
    fn scaling_function_matches_the_mpp_integer_interpolation() {
        let mut points = [[0u8; 2]; 14];
        points[0] = [12, 20];
        points[1] = [200, 90];
        let mut lut = [0u8; 256];

        init_scaling_function(&points, 2, &mut lut).unwrap();

        assert_eq!(&lut[..12], &[20; 12]);
        assert_eq!(lut[12], 20);
        assert_eq!(lut[105], 55);
        assert_eq!(lut[186], 85);
        assert_eq!(lut[198], 89);
        assert_eq!(lut[199], 90);
        assert_eq!(&lut[200..], &[90; 56]);
    }

    #[test]
    fn invalid_scaling_points_are_rejected_instead_of_indexing_out_of_bounds() {
        let mut points = [[0u8; 2]; 14];
        points[0] = [20, 1];
        points[1] = [20, 2];
        let mut lut = [0u8; 256];

        assert_eq!(
            init_scaling_function(&points, 2, &mut lut),
            Err(FilmGrainError::InvalidScalingPoints)
        );
    }
}
