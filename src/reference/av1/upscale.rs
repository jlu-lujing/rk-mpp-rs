//! AV1 normative horizontal super-resolution upscaling, specification §7.16.

use crate::reference::av1::lr_frame::round2;

pub const SUPERRES_FILTER_BITS: u32 = 6;
pub const SUPERRES_FILTER_TAPS: usize = 8;
pub const SUPERRES_FILTER_OFFSET: usize = 3;
pub const SUPERRES_SCALE_BITS: u32 = 14;
pub const SUPERRES_SCALE_MASK: i64 = (1 << SUPERRES_SCALE_BITS) - 1;
pub const SUPERRES_EXTRA_BITS: u32 = 8;
pub const FILTER_BITS: u32 = 7;

pub static UPSCALE_FILTER: [[i32; SUPERRES_FILTER_TAPS]; 64] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, -1, 128, 2, -1, 0, 0],
    [0, 1, -3, 127, 4, -2, 1, 0],
    [0, 1, -4, 127, 6, -3, 1, 0],
    [0, 2, -6, 126, 8, -3, 1, 0],
    [0, 2, -7, 125, 11, -4, 1, 0],
    [-1, 2, -8, 125, 13, -5, 2, 0],
    [-1, 3, -9, 124, 15, -6, 2, 0],
    [-1, 3, -10, 123, 18, -6, 2, -1],
    [-1, 3, -11, 122, 20, -7, 3, -1],
    [-1, 4, -12, 121, 22, -8, 3, -1],
    [-1, 4, -13, 120, 25, -9, 3, -1],
    [-1, 4, -14, 118, 28, -9, 3, -1],
    [-1, 4, -15, 117, 30, -10, 4, -1],
    [-1, 5, -16, 116, 32, -11, 4, -1],
    [-1, 5, -16, 114, 35, -12, 4, -1],
    [-1, 5, -17, 112, 38, -12, 4, -1],
    [-1, 5, -18, 111, 40, -13, 5, -1],
    [-1, 5, -18, 109, 43, -14, 5, -1],
    [-1, 6, -19, 107, 45, -14, 5, -1],
    [-1, 6, -19, 105, 48, -15, 5, -1],
    [-1, 6, -19, 103, 51, -16, 5, -1],
    [-1, 6, -20, 101, 53, -16, 6, -1],
    [-1, 6, -20, 99, 56, -17, 6, -1],
    [-1, 6, -20, 97, 58, -17, 6, -1],
    [-1, 6, -20, 95, 61, -18, 6, -1],
    [-2, 7, -20, 93, 64, -18, 6, -2],
    [-2, 7, -20, 91, 66, -19, 6, -1],
    [-2, 7, -20, 88, 69, -19, 6, -1],
    [-2, 7, -20, 86, 71, -19, 6, -1],
    [-2, 7, -20, 84, 74, -20, 7, -2],
    [-2, 7, -20, 81, 76, -20, 7, -1],
    [-2, 7, -20, 79, 79, -20, 7, -2],
    [-1, 7, -20, 76, 81, -20, 7, -2],
    [-2, 7, -20, 74, 84, -20, 7, -2],
    [-1, 6, -19, 71, 86, -20, 7, -2],
    [-1, 6, -19, 69, 88, -20, 7, -2],
    [-1, 6, -19, 66, 91, -20, 7, -2],
    [-2, 6, -18, 64, 93, -20, 7, -2],
    [-1, 6, -18, 61, 95, -20, 6, -1],
    [-1, 6, -17, 58, 97, -20, 6, -1],
    [-1, 6, -17, 56, 99, -20, 6, -1],
    [-1, 6, -16, 53, 101, -20, 6, -1],
    [-1, 5, -16, 51, 103, -19, 6, -1],
    [-1, 5, -15, 48, 105, -19, 6, -1],
    [-1, 5, -14, 45, 107, -19, 6, -1],
    [-1, 5, -14, 43, 109, -18, 5, -1],
    [-1, 5, -13, 40, 111, -18, 5, -1],
    [-1, 4, -12, 38, 112, -17, 5, -1],
    [-1, 4, -12, 35, 114, -16, 5, -1],
    [-1, 4, -11, 32, 116, -16, 5, -1],
    [-1, 4, -10, 30, 117, -15, 4, -1],
    [-1, 3, -9, 28, 118, -14, 4, -1],
    [-1, 3, -9, 25, 120, -13, 4, -1],
    [-1, 3, -8, 22, 121, -12, 4, -1],
    [-1, 3, -7, 20, 122, -11, 3, -1],
    [-1, 2, -6, 18, 123, -10, 3, -1],
    [0, 2, -6, 15, 124, -9, 3, -1],
    [0, 2, -5, 13, 125, -8, 2, -1],
    [0, 1, -4, 11, 125, -7, 2, 0],
    [0, 1, -3, 8, 126, -6, 2, 0],
    [0, 1, -3, 6, 127, -4, 1, 0],
    [0, 1, -2, 4, 127, -3, 1, 0],
    [0, 0, -1, 2, 128, -1, 0, 0],
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpscaledPlane {
    pub data: Vec<u16>,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

/// §7.16 upscaling for one plane.
#[allow(clippy::too_many_arguments)]
pub fn upscale_plane(
    input: &[u16],
    input_stride: usize,
    input_plane_width: usize,
    frame_width: usize,
    frame_height: usize,
    upscaled_width: usize,
    sub_x: u32,
    sub_y: u32,
    bit_depth: u32,
) -> UpscaledPlane {
    let downscaled_plane_width = round2(frame_width, sub_x);
    let upscaled_plane_width = round2(upscaled_width, sub_x);
    let plane_height = round2(frame_height, sub_y);
    assert!(upscaled_plane_width > downscaled_plane_width);
    assert!(input_stride >= input_plane_width);
    assert!(input.len() >= input_stride * plane_height);

    let step_x = (((downscaled_plane_width as i64) << SUPERRES_SCALE_BITS)
        + (upscaled_plane_width as i64 / 2))
        / upscaled_plane_width as i64;
    let err = upscaled_plane_width as i64 * step_x
        - ((downscaled_plane_width as i64) << SUPERRES_SCALE_BITS);
    let initial_subpel_x = (-(((upscaled_plane_width - downscaled_plane_width) as i64)
        << (SUPERRES_SCALE_BITS - 1))
        + upscaled_plane_width as i64 / 2)
        / upscaled_plane_width as i64
        + (1i64 << (SUPERRES_EXTRA_BITS - 1))
        - err / 2;
    let initial_subpel_x = initial_subpel_x & SUPERRES_SCALE_MASK;
    let max_x = input_plane_width as i64 - 1;
    let stride = upscaled_plane_width.div_ceil(64) * 64;
    let mut data = vec![0u16; stride * plane_height];
    let max_sample = (1i64 << bit_depth) - 1;
    for y in 0..plane_height {
        for x in 0..upscaled_plane_width {
            let src_x = -(1i64 << SUPERRES_SCALE_BITS) + initial_subpel_x + x as i64 * step_x;
            let src_x_px = src_x >> SUPERRES_SCALE_BITS;
            let phase = ((src_x & SUPERRES_SCALE_MASK) >> SUPERRES_EXTRA_BITS) as usize;
            let mut sum = 0i64;
            for (k, tap) in UPSCALE_FILTER[phase].iter().enumerate() {
                let sample_x =
                    (src_x_px + k as i64 - SUPERRES_FILTER_OFFSET as i64).clamp(0, max_x) as usize;
                sum += i64::from(input[y * input_stride + sample_x]) * i64::from(*tap);
            }
            let rounded = (sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS;
            data[y * stride + x] = rounded.clamp(0, max_sample) as u16;
        }
    }
    UpscaledPlane {
        data,
        width: upscaled_plane_width,
        height: plane_height,
        stride,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normative_filter_phases_have_unit_dc_gain() {
        for filter in UPSCALE_FILTER {
            assert_eq!(filter.iter().sum::<i32>(), 128);
        }
    }

    #[test]
    fn constant_plane_stays_constant() {
        let input = [42u16; 8];
        let out = upscale_plane(&input, 8, 8, 8, 1, 16, 0, 0, 8);
        assert!(out.data[..16].iter().all(|sample| *sample == 42));
    }
}
