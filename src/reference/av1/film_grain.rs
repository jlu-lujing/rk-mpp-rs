//! AV1 film grain synthesis (specification section 7.18.3).
//!
//! The hardware path builds its own film-grain memory image through
//! `crate::hal::av1d::build_film_grain_memory`. This module is the independent software
//! decoder's final display transform: references retain the post-filter,
//! pre-grain picture and grain is applied only when a frame is output.

use crate::hal::av1d::GAUSSIAN_SEQUENCE;
use crate::reference::av1::frame_header::{FilmGrainParams, FrameHeader};
use crate::reference::av1::intra_frame::{IntraFrame, PlaneBuffer};
use crate::reference::av1::seq_header::{SequenceHeader, MC_IDENTITY};

const LUMA_SUBBLOCK_SIZE: usize = 32;
const AR_PADDING: usize = 3;
const EDGE_PAD: usize = 3;
const GAUSS_BITS: usize = 11;
const MIN_LUMA_LEGAL_RANGE: i32 = 16;
const MAX_LUMA_LEGAL_RANGE: i32 = 235;
const MIN_CHROMA_LEGAL_RANGE: i32 = 16;
const MAX_CHROMA_LEGAL_RANGE: i32 = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmGrainError {
    UnsupportedBitDepth(u32),
    PlaneBitDepthMismatch { frame: u32, sequence: u8 },
    InvalidAutoregressiveLag(u8),
    InvalidScalingPoints,
    MissingChromaPlane,
}

impl core::fmt::Display for FilmGrainError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Apply the frame's normative film grain to a post-filter display picture.
///
/// `frame` deliberately retains its original visible dimensions and padded
/// stride. The synthesis runs on an even-aligned temporary copy, matching
/// libaom's edge extension for odd display sizes, then copies only visible
/// samples back.
pub fn apply_film_grain(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    frame: &mut IntraFrame,
) -> Result<(), FilmGrainError> {
    apply_film_grain_params(sequence, &header.film_grain, frame)
}

/// Apply an explicitly selected parameter set, used for `show_existing_frame`
/// where the grain parameters belong to the referenced frame rather than the
/// short display header.
pub fn apply_film_grain_params(
    sequence: &SequenceHeader,
    params: &FilmGrainParams,
    frame: &mut IntraFrame,
) -> Result<(), FilmGrainError> {
    if !params.apply_grain {
        return Ok(());
    }
    if !matches!(frame.bit_depth, 8..=12) {
        return Err(FilmGrainError::UnsupportedBitDepth(frame.bit_depth));
    }
    if frame.bit_depth != u32::from(sequence.color.bit_depth) {
        return Err(FilmGrainError::PlaneBitDepthMismatch {
            frame: frame.bit_depth,
            sequence: sequence.color.bit_depth,
        });
    }
    if usize::from(params.ar_coeff_lag) > 3 {
        return Err(FilmGrainError::InvalidAutoregressiveLag(
            params.ar_coeff_lag,
        ));
    }
    if sequence.color.mono_chrome {
        if frame.u.is_some() || frame.v.is_some() {
            return Err(FilmGrainError::MissingChromaPlane);
        }
    } else if frame.u.is_none() || frame.v.is_none() {
        return Err(FilmGrainError::MissingChromaPlane);
    }

    let width = frame.viewport_width + (frame.viewport_width & 1);
    let height = frame.viewport_height + (frame.viewport_height & 1);
    let sub_x = usize::from(sequence.color.subsampling_x);
    let sub_y = usize::from(sequence.color.subsampling_y);
    let chroma_width = width >> sub_x;
    let chroma_height = height >> sub_y;

    let mut working = WorkingImage {
        y: copy_and_extend(&frame.y, width, height),
        cb: frame
            .u
            .as_ref()
            .map(|plane| copy_and_extend(plane, chroma_width, chroma_height))
            .unwrap_or_else(|| vec![0; chroma_width * chroma_height]),
        cr: frame
            .v
            .as_ref()
            .map(|plane| copy_and_extend(plane, chroma_width, chroma_height))
            .unwrap_or_else(|| vec![0; chroma_width * chroma_height]),
        width,
        height,
        chroma_width,
    };

    add_film_grain_run(
        params,
        &mut working,
        frame.bit_depth as u8,
        sub_x,
        sub_y,
        sequence.color.matrix_coefficients == MC_IDENTITY,
    )?;

    copy_visible_back(&working.y, width, &mut frame.y);
    if let Some(plane) = frame.u.as_mut() {
        copy_visible_back(&working.cb, chroma_width, plane);
    }
    if let Some(plane) = frame.v.as_mut() {
        copy_visible_back(&working.cr, chroma_width, plane);
    }
    Ok(())
}

struct WorkingImage {
    y: Vec<u16>,
    cb: Vec<u16>,
    cr: Vec<u16>,
    width: usize,
    height: usize,
    chroma_width: usize,
}

fn copy_and_extend(plane: &PlaneBuffer, width: usize, height: usize) -> Vec<u16> {
    let mut output = vec![0; width * height];
    let copy_width = plane.width.min(width);
    let copy_height = plane.height.min(height);
    for row in 0..copy_height {
        let source = row * plane.stride;
        let target = row * width;
        output[target..target + copy_width]
            .copy_from_slice(&plane.data[source..source + copy_width]);
        if copy_width < width {
            output[target + copy_width..target + width].fill(plane.data[source + copy_width - 1]);
        }
    }
    if copy_height < height {
        let last = (copy_height - 1) * width;
        for row in copy_height..height {
            let target = row * width;
            output.copy_within(last..last + width, target);
        }
    }
    output
}

fn copy_visible_back(source_data: &[u16], source_stride: usize, plane: &mut PlaneBuffer) {
    for row in 0..plane.height {
        let source = row * source_stride;
        let target = row * plane.stride;
        plane.data[target..target + plane.width]
            .copy_from_slice(&source_data[source..source + plane.width]);
    }
}

#[derive(Clone, Copy)]
struct GrainPlane {
    stride: usize,
    height: usize,
    width: usize,
}

impl GrainPlane {
    fn new(height: usize, width: usize) -> Self {
        Self {
            stride: width,
            height,
            width,
        }
    }

    fn offset(self, row: usize, col: usize) -> usize {
        row * self.stride + col
    }
}

#[derive(Clone, Copy)]
struct ScalingLuts {
    y: [i32; 256],
    cb: [i32; 256],
    cr: [i32; 256],
}

impl Default for ScalingLuts {
    fn default() -> Self {
        Self {
            y: [0; 256],
            cb: [0; 256],
            cr: [0; 256],
        }
    }
}

struct Rng(u16);

impl Rng {
    fn init(luma_line: usize, seed: u16) -> Self {
        let luma_num = luma_line >> 5;
        let mut random = seed;
        random ^= (((luma_num * 37 + 178) & 255) as u16) << 8;
        random ^= ((luma_num * 173 + 105) & 255) as u16;
        Self(random)
    }

    fn number(&mut self, bits: usize) -> i32 {
        let bit = (self.0 ^ (self.0 >> 1) ^ (self.0 >> 3) ^ (self.0 >> 12)) & 1;
        self.0 = (self.0 >> 1) | (bit << 15);
        i32::from((self.0 >> (16 - bits)) & ((1 << bits) - 1))
    }
}

fn add_film_grain_run(
    params: &FilmGrainParams,
    image: &mut WorkingImage,
    bit_depth: u8,
    sub_x: usize,
    sub_y: usize,
    matrix_coefficients_identity: bool,
) -> Result<(), FilmGrainError> {
    let bit_depth_i32 = i32::from(bit_depth);
    let chroma_subblock_y = LUMA_SUBBLOCK_SIZE >> sub_y;
    let chroma_subblock_x = LUMA_SUBBLOCK_SIZE >> sub_x;
    let luma_height = EDGE_PAD + 2 * AR_PADDING + LUMA_SUBBLOCK_SIZE * 2;
    let luma_width = EDGE_PAD + 2 * AR_PADDING + LUMA_SUBBLOCK_SIZE * 2 + 2 * AR_PADDING + EDGE_PAD;
    let chroma_height = EDGE_PAD + (2 >> sub_y) * AR_PADDING + chroma_subblock_y * 2;
    let chroma_width = EDGE_PAD
        + (2 >> sub_x) * AR_PADDING
        + chroma_subblock_x * 2
        + (2 >> sub_x) * AR_PADDING
        + EDGE_PAD;

    let luma_shape = GrainPlane::new(luma_height, luma_width);
    let chroma_shape = GrainPlane::new(chroma_height, chroma_width);
    let (pred_luma, pred_chroma) = prediction_positions(params);
    let mut luma_grain = vec![0; luma_shape.height * luma_shape.stride];
    let mut cb_grain = vec![0; chroma_shape.height * chroma_shape.stride];
    let mut cr_grain = vec![0; chroma_shape.height * chroma_shape.stride];

    let mut rng = Rng(params.grain_seed);
    generate_luma_grain(
        params,
        &mut rng,
        &pred_luma,
        &mut luma_grain,
        luma_shape,
        bit_depth_i32,
    );
    generate_chroma_grain(
        params,
        &mut rng,
        &pred_chroma,
        &luma_grain,
        luma_shape,
        &mut cb_grain,
        &mut cr_grain,
        chroma_shape,
        sub_x,
        sub_y,
        bit_depth_i32,
    )?;

    let luts = scaling_luts(params)?;
    let grain_min = -(1i32 << (bit_depth_i32 - 1));
    let grain_max = (1i32 << (bit_depth_i32 - 1)) - 1;

    let mut y_line_buf = vec![0; image.width * 2];
    let mut cb_line_buf = vec![0; image.chroma_width * (2 >> sub_y)];
    let mut cr_line_buf = vec![0; image.chroma_width * (2 >> sub_y)];
    let mut y_col_buf = vec![0; (LUMA_SUBBLOCK_SIZE + 2) * 2];
    let mut cb_col_buf = vec![0; (chroma_subblock_y + (2 >> sub_y)) * (2 >> sub_x)];
    let mut cr_col_buf = vec![0; (chroma_subblock_y + (2 >> sub_y)) * (2 >> sub_x)];

    for y in (0..image.height / 2).step_by(LUMA_SUBBLOCK_SIZE / 2) {
        rng = Rng::init(y * 2, params.grain_seed);
        for x in (0..image.width / 2).step_by(LUMA_SUBBLOCK_SIZE / 2) {
            let random = rng.number(8);
            let offset_x = ((random >> 4) & 15) as usize;
            let offset_y = (random & 15) as usize;
            let luma_offset_y = EDGE_PAD + 2 * AR_PADDING + (offset_y << 1);
            let luma_offset_x = EDGE_PAD + 2 * AR_PADDING + (offset_x << 1);
            let chroma_offset_y = EDGE_PAD + (2 >> sub_y) * AR_PADDING + offset_y * (2 >> sub_y);
            let chroma_offset_x = EDGE_PAD + (2 >> sub_x) * AR_PADDING + offset_x * (2 >> sub_x);

            if params.overlap_flag && x != 0 {
                vertical_overlap(
                    &mut y_col_buf,
                    2,
                    &luma_grain,
                    luma_shape.stride,
                    luma_shape.offset(luma_offset_y, luma_offset_x),
                    2,
                    (LUMA_SUBBLOCK_SIZE + 2).min(image.height - (y << 1)),
                    grain_min,
                    grain_max,
                );
                vertical_overlap(
                    &mut cb_col_buf,
                    2 >> sub_x,
                    &cb_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(chroma_offset_y, chroma_offset_x),
                    2 >> sub_x,
                    (chroma_subblock_y + (2 >> sub_y)).min((image.height - (y << 1)) >> sub_y),
                    grain_min,
                    grain_max,
                );
                vertical_overlap(
                    &mut cr_col_buf,
                    2 >> sub_x,
                    &cr_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(chroma_offset_y, chroma_offset_x),
                    2 >> sub_x,
                    (chroma_subblock_y + (2 >> sub_y)).min((image.height - (y << 1)) >> sub_y),
                    grain_min,
                    grain_max,
                );

                let i = usize::from(y != 0);
                add_noise_to_block(
                    params,
                    &luts,
                    image,
                    bit_depth,
                    (x << 1, (y + i) << 1),
                    (&y_col_buf, 2, i * 4),
                    (x << (1 - sub_x), (y + i) << (1 - sub_y)),
                    (&cb_col_buf, 2 >> sub_x, 0),
                    (&cr_col_buf, 2 >> sub_x, 0),
                    (LUMA_SUBBLOCK_SIZE / 2).min(image.height / 2 - y) - i,
                    1,
                    sub_x,
                    sub_y,
                    matrix_coefficients_identity,
                );
            }

            if params.overlap_flag && y != 0 {
                if x != 0 {
                    horizontal_overlap(
                        &mut y_line_buf,
                        image.width,
                        x << 1,
                        &y_col_buf,
                        2,
                        0,
                        2,
                        2,
                        grain_min,
                        grain_max,
                    );
                    horizontal_overlap(
                        &mut cb_line_buf,
                        image.chroma_width,
                        x << (1 - sub_x),
                        &cb_col_buf,
                        2 >> sub_x,
                        0,
                        2 >> sub_x,
                        2 >> sub_y,
                        grain_min,
                        grain_max,
                    );
                    horizontal_overlap(
                        &mut cr_line_buf,
                        image.chroma_width,
                        x << (1 - sub_x),
                        &cr_col_buf,
                        2 >> sub_x,
                        0,
                        2 >> sub_x,
                        2 >> sub_y,
                        grain_min,
                        grain_max,
                    );
                }

                let target_x = (if x != 0 { x + 1 } else { 0 }) << 1;
                horizontal_overlap(
                    &mut y_line_buf,
                    image.width,
                    target_x,
                    &luma_grain,
                    luma_shape.stride,
                    luma_shape.offset(luma_offset_y, luma_offset_x + if x != 0 { 2 } else { 0 }),
                    (LUMA_SUBBLOCK_SIZE - if x != 0 { 2 } else { 0 }).min(image.width - target_x),
                    2,
                    grain_min,
                    grain_max,
                );
                let chroma_target_x = (if x != 0 { x + 1 } else { 0 }) << (1 - sub_x);
                horizontal_overlap(
                    &mut cb_line_buf,
                    image.chroma_width,
                    chroma_target_x,
                    &cb_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(
                        chroma_offset_y,
                        chroma_offset_x + if x != 0 { 2 >> sub_x } else { 0 },
                    ),
                    (chroma_subblock_x - if x != 0 { 2 >> sub_x } else { 0 })
                        .min((image.width - target_x) >> sub_x),
                    2 >> sub_y,
                    grain_min,
                    grain_max,
                );
                horizontal_overlap(
                    &mut cr_line_buf,
                    image.chroma_width,
                    chroma_target_x,
                    &cr_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(
                        chroma_offset_y,
                        chroma_offset_x + if x != 0 { 2 >> sub_x } else { 0 },
                    ),
                    (chroma_subblock_x - if x != 0 { 2 >> sub_x } else { 0 })
                        .min((image.width - target_x) >> sub_x),
                    2 >> sub_y,
                    grain_min,
                    grain_max,
                );

                add_noise_to_block(
                    params,
                    &luts,
                    image,
                    bit_depth,
                    (x << 1, y << 1),
                    (&y_line_buf, image.width, x << 1),
                    (x << (1 - sub_x), y << (1 - sub_y)),
                    (&cb_line_buf, image.chroma_width, x << (1 - sub_x)),
                    (&cr_line_buf, image.chroma_width, x << (1 - sub_x)),
                    1,
                    (LUMA_SUBBLOCK_SIZE / 2).min(image.width / 2 - x),
                    sub_x,
                    sub_y,
                    matrix_coefficients_identity,
                );
            }

            let i = usize::from(params.overlap_flag && y != 0);
            let j = usize::from(params.overlap_flag && x != 0);
            add_noise_to_block(
                params,
                &luts,
                image,
                bit_depth,
                ((x + j) << 1, (y + i) << 1),
                (
                    &luma_grain,
                    luma_shape.stride,
                    luma_shape.offset(luma_offset_y + (i << 1), luma_offset_x + (j << 1)),
                ),
                ((x + j) << (1 - sub_x), (y + i) << (1 - sub_y)),
                (
                    &cb_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(
                        chroma_offset_y + (i << (1 - sub_y)),
                        chroma_offset_x + (j << (1 - sub_x)),
                    ),
                ),
                (
                    &cr_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(
                        chroma_offset_y + (i << (1 - sub_y)),
                        chroma_offset_x + (j << (1 - sub_x)),
                    ),
                ),
                (LUMA_SUBBLOCK_SIZE / 2).min(image.height / 2 - y) - i,
                (LUMA_SUBBLOCK_SIZE / 2).min(image.width / 2 - x) - j,
                sub_x,
                sub_y,
                matrix_coefficients_identity,
            );

            if params.overlap_flag {
                if x != 0 {
                    copy_area(
                        &y_col_buf,
                        2,
                        LUMA_SUBBLOCK_SIZE << 1,
                        &mut y_line_buf,
                        image.width,
                        x << 1,
                        2,
                        2,
                    );
                    copy_area(
                        &cb_col_buf,
                        2 >> sub_x,
                        chroma_subblock_y << (1 - sub_x),
                        &mut cb_line_buf,
                        image.chroma_width,
                        x << (1 - sub_x),
                        2 >> sub_x,
                        2 >> sub_y,
                    );
                    copy_area(
                        &cr_col_buf,
                        2 >> sub_x,
                        chroma_subblock_y << (1 - sub_x),
                        &mut cr_line_buf,
                        image.chroma_width,
                        x << (1 - sub_x),
                        2 >> sub_x,
                        2 >> sub_y,
                    );
                }

                copy_area(
                    &luma_grain,
                    luma_shape.stride,
                    luma_shape.offset(
                        luma_offset_y + LUMA_SUBBLOCK_SIZE,
                        luma_offset_x + if x != 0 { 2 } else { 0 },
                    ),
                    &mut y_line_buf,
                    image.width,
                    (if x != 0 { x + 1 } else { 0 }) << 1,
                    LUMA_SUBBLOCK_SIZE.min(image.width - (x << 1)) - if x != 0 { 2 } else { 0 },
                    2,
                );
                copy_area(
                    &cb_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(
                        chroma_offset_y + chroma_subblock_y,
                        chroma_offset_x + if x != 0 { 2 >> sub_x } else { 0 },
                    ),
                    &mut cb_line_buf,
                    image.chroma_width,
                    (if x != 0 { x + 1 } else { 0 }) << (1 - sub_x),
                    chroma_subblock_x.min((image.width - (x << 1)) >> sub_x)
                        - if x != 0 { 2 >> sub_x } else { 0 },
                    2 >> sub_y,
                );
                copy_area(
                    &cr_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(
                        chroma_offset_y + chroma_subblock_y,
                        chroma_offset_x + if x != 0 { 2 >> sub_x } else { 0 },
                    ),
                    &mut cr_line_buf,
                    image.chroma_width,
                    (if x != 0 { x + 1 } else { 0 }) << (1 - sub_x),
                    chroma_subblock_x.min((image.width - (x << 1)) >> sub_x)
                        - if x != 0 { 2 >> sub_x } else { 0 },
                    2 >> sub_y,
                );

                copy_area(
                    &luma_grain,
                    luma_shape.stride,
                    luma_shape.offset(luma_offset_y, luma_offset_x + LUMA_SUBBLOCK_SIZE),
                    &mut y_col_buf,
                    2,
                    0,
                    2,
                    (LUMA_SUBBLOCK_SIZE + 2).min(image.height - (y << 1)),
                );
                copy_area(
                    &cb_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(chroma_offset_y, chroma_offset_x + chroma_subblock_x),
                    &mut cb_col_buf,
                    2 >> sub_x,
                    0,
                    2 >> sub_x,
                    (chroma_subblock_y + (2 >> sub_y)).min((image.height - (y << 1)) >> sub_y),
                );
                copy_area(
                    &cr_grain,
                    chroma_shape.stride,
                    chroma_shape.offset(chroma_offset_y, chroma_offset_x + chroma_subblock_x),
                    &mut cr_col_buf,
                    2 >> sub_x,
                    0,
                    2 >> sub_x,
                    (chroma_subblock_y + (2 >> sub_y)).min((image.height - (y << 1)) >> sub_y),
                );
            }
        }
    }
    Ok(())
}

fn prediction_positions(
    params: &FilmGrainParams,
) -> (Vec<(isize, isize)>, Vec<(isize, isize, u8)>) {
    let lag = isize::from(params.ar_coeff_lag);
    let mut luma = Vec::new();
    for row in -lag..0 {
        for col in -lag..=lag {
            luma.push((row, col));
        }
    }
    for col in -lag..0 {
        luma.push((0, col));
    }

    let mut chroma = luma
        .iter()
        .copied()
        .map(|(row, col)| (row, col, 0))
        .collect::<Vec<_>>();
    if params.num_y_points > 0 {
        chroma.push((0, 0, 1));
    }
    (luma, chroma)
}

fn generate_luma_grain(
    params: &FilmGrainParams,
    rng: &mut Rng,
    positions: &[(isize, isize)],
    block: &mut [i32],
    shape: GrainPlane,
    bit_depth: i32,
) {
    if params.num_y_points == 0 {
        return;
    }
    let gauss_shift = 12 - bit_depth + i32::from(params.grain_scale_shift);
    let rounding = 1i32 << (params.ar_coeff_shift_minus_6 + 5);
    let shift = i32::from(params.ar_coeff_shift_minus_6) + 6;
    let grain_min = -(1i32 << (bit_depth - 1));
    let grain_max = (1i32 << (bit_depth - 1)) - 1;

    for row in 0..shape.height {
        for col in 0..shape.width {
            block[shape.offset(row, col)] = round_shift(
                i32::from(GAUSSIAN_SEQUENCE[rng.number(GAUSS_BITS) as usize]),
                gauss_shift,
            );
        }
    }
    for row in EDGE_PAD..shape.height {
        for col in EDGE_PAD..shape.width - EDGE_PAD {
            let mut weight = 0;
            for (index, &(delta_row, delta_col)) in positions.iter().enumerate() {
                let coefficient = i32::from(params.ar_coeffs_y[index]) - 128;
                let source_row = (row as isize + delta_row) as usize;
                let source_col = (col as isize + delta_col) as usize;
                weight += coefficient * block[shape.offset(source_row, source_col)];
            }
            let offset = shape.offset(row, col);
            block[offset] =
                (block[offset] + ((weight + rounding) >> shift)).clamp(grain_min, grain_max);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_chroma_grain(
    params: &FilmGrainParams,
    rng: &mut Rng,
    positions: &[(isize, isize, u8)],
    luma: &[i32],
    luma_shape: GrainPlane,
    cb: &mut [i32],
    cr: &mut [i32],
    shape: GrainPlane,
    sub_x: usize,
    sub_y: usize,
    bit_depth: i32,
) -> Result<(), FilmGrainError> {
    let gauss_shift = 12 - bit_depth + i32::from(params.grain_scale_shift);
    let rounding = 1i32 << (params.ar_coeff_shift_minus_6 + 5);
    let shift = i32::from(params.ar_coeff_shift_minus_6) + 6;
    let grain_min = -(1i32 << (bit_depth - 1));
    let grain_max = (1i32 << (bit_depth - 1)) - 1;
    let apply_cb = params.num_cb_points != 0 || params.chroma_scaling_from_luma;
    let apply_cr = params.num_cr_points != 0 || params.chroma_scaling_from_luma;

    if apply_cb {
        *rng = Rng::init(7 << 5, params.grain_seed);
        for row in 0..shape.height {
            for col in 0..shape.width {
                cb[shape.offset(row, col)] = round_shift(
                    i32::from(GAUSSIAN_SEQUENCE[rng.number(GAUSS_BITS) as usize]),
                    gauss_shift,
                );
            }
        }
    }
    if apply_cr {
        *rng = Rng::init(11 << 5, params.grain_seed);
        for row in 0..shape.height {
            for col in 0..shape.width {
                cr[shape.offset(row, col)] = round_shift(
                    i32::from(GAUSSIAN_SEQUENCE[rng.number(GAUSS_BITS) as usize]),
                    gauss_shift,
                );
            }
        }
    }

    for row in EDGE_PAD..shape.height {
        for col in EDGE_PAD..shape.width - EDGE_PAD {
            let mut weight_cb = 0;
            let mut weight_cr = 0;
            for (index, &(delta_row, delta_col, source)) in positions.iter().enumerate() {
                if source == 0 {
                    let source_row = (row as isize + delta_row) as usize;
                    let source_col = (col as isize + delta_col) as usize;
                    weight_cb += (i32::from(params.ar_coeffs_cb[index]) - 128)
                        * cb[shape.offset(source_row, source_col)];
                    weight_cr += (i32::from(params.ar_coeffs_cr[index]) - 128)
                        * cr[shape.offset(source_row, source_col)];
                } else if source == 1 {
                    let luma_row = ((row - EDGE_PAD) << sub_y) + EDGE_PAD;
                    let luma_col = ((col - EDGE_PAD) << sub_x) + EDGE_PAD;
                    let mut average = 0;
                    for source_y in luma_row..luma_row + sub_y + 1 {
                        for source_x in luma_col..luma_col + sub_x + 1 {
                            average += luma[luma_shape.offset(source_y, source_x)];
                        }
                    }
                    average = (average + ((1i32 << (sub_x + sub_y)) >> 1)) >> (sub_x + sub_y);
                    weight_cb += (i32::from(params.ar_coeffs_cb[index]) - 128) * average;
                    weight_cr += (i32::from(params.ar_coeffs_cr[index]) - 128) * average;
                } else {
                    return Err(FilmGrainError::InvalidScalingPoints);
                }
            }
            if apply_cb {
                let offset = shape.offset(row, col);
                cb[offset] =
                    (cb[offset] + ((weight_cb + rounding) >> shift)).clamp(grain_min, grain_max);
            }
            if apply_cr {
                let offset = shape.offset(row, col);
                cr[offset] =
                    (cr[offset] + ((weight_cr + rounding) >> shift)).clamp(grain_min, grain_max);
            }
        }
    }
    Ok(())
}

fn scaling_luts(params: &FilmGrainParams) -> Result<ScalingLuts, FilmGrainError> {
    let mut luts = ScalingLuts::default();
    init_scaling_function(
        &params.point_y_value,
        &params.point_y_scaling,
        params.num_y_points,
        &mut luts.y,
    )?;
    if params.chroma_scaling_from_luma {
        luts.cb = luts.y;
        luts.cr = luts.y;
    } else {
        init_scaling_function(
            &params.point_cb_value,
            &params.point_cb_scaling,
            params.num_cb_points,
            &mut luts.cb,
        )?;
        init_scaling_function(
            &params.point_cr_value,
            &params.point_cr_scaling,
            params.num_cr_points,
            &mut luts.cr,
        )?;
    }
    Ok(luts)
}

fn init_scaling_function(
    values: &[u8; 16],
    scaling: &[u8; 16],
    count: usize,
    lut: &mut [i32; 256],
) -> Result<(), FilmGrainError> {
    if count == 0 {
        return Ok(());
    }
    lut[..usize::from(values[0])].fill(i32::from(scaling[0]));
    for point in 0..count - 1 {
        let x0 = usize::from(values[point]);
        let x1 = usize::from(values[point + 1]);
        let delta_x = x1
            .checked_sub(x0)
            .ok_or(FilmGrainError::InvalidScalingPoints)?;
        if delta_x == 0 || x1 > lut.len() {
            return Err(FilmGrainError::InvalidScalingPoints);
        }
        let delta_y = i32::from(scaling[point + 1]) - i32::from(scaling[point]);
        let delta = i64::from(delta_y) * ((65_536 + (delta_x >> 1)) / delta_x) as i64;
        for x in 0..delta_x {
            lut[x0 + x] = i32::from(scaling[point]) + ((x as i64 * delta + 32_768) >> 16) as i32;
        }
    }
    let last = usize::from(values[count - 1]);
    if last > lut.len() {
        return Err(FilmGrainError::InvalidScalingPoints);
    }
    lut[last..].fill(i32::from(scaling[count - 1]));
    Ok(())
}

fn scale_lut(lut: &[i32; 256], index: u16, bit_depth: u8) -> i32 {
    let shift = u32::from(bit_depth - 8);
    let x = usize::from(index >> shift);
    if shift == 0 || x == 255 {
        lut[x]
    } else {
        lut[x]
            + ((i32::from(lut[x + 1] - lut[x]) * i32::from(index & ((1 << shift) - 1))
                + (1 << (bit_depth - 9)))
                >> shift)
    }
}

#[allow(clippy::too_many_arguments)]
fn add_noise_to_block(
    params: &FilmGrainParams,
    luts: &ScalingLuts,
    image: &mut WorkingImage,
    bit_depth: u8,
    luma_origin: (usize, usize),
    luma_grain: (&[i32], usize, usize),
    chroma_origin: (usize, usize),
    cb_grain: (&[i32], usize, usize),
    cr_grain: (&[i32], usize, usize),
    half_luma_height: usize,
    half_luma_width: usize,
    sub_x: usize,
    sub_y: usize,
    matrix_coefficients_identity: bool,
) {
    let mut cb_mult = i32::from(params.cb_mult) - 128;
    let mut cb_luma_mult = i32::from(params.cb_luma_mult) - 128;
    let mut cb_offset = if bit_depth == 8 {
        i32::from(params.cb_offset) - 256
    } else {
        (i32::from(params.cb_offset) << (bit_depth - 8)) - (1 << bit_depth)
    };
    let mut cr_mult = i32::from(params.cr_mult) - 128;
    let mut cr_luma_mult = i32::from(params.cr_luma_mult) - 128;
    let mut cr_offset = if bit_depth == 8 {
        i32::from(params.cr_offset) - 256
    } else {
        (i32::from(params.cr_offset) << (bit_depth - 8)) - (1 << bit_depth)
    };
    if params.chroma_scaling_from_luma {
        cb_mult = 0;
        cb_luma_mult = 64;
        cb_offset = 0;
        cr_mult = 0;
        cr_luma_mult = 64;
        cr_offset = 0;
    }
    let apply_y = params.num_y_points != 0;
    let apply_cb = params.num_cb_points != 0 || params.chroma_scaling_from_luma;
    let apply_cr = params.num_cr_points != 0 || params.chroma_scaling_from_luma;
    let rounding = 1i32 << (params.grain_scaling_minus_8 + 7);
    let shift = u32::from(params.grain_scaling_minus_8) + 8;
    let (min_luma, max_luma, min_chroma, max_chroma) = if params.clip_to_restricted_range {
        let scale = 1i32 << (bit_depth - 8);
        let chroma_max = if matrix_coefficients_identity {
            MAX_LUMA_LEGAL_RANGE
        } else {
            MAX_CHROMA_LEGAL_RANGE
        };
        (
            MIN_LUMA_LEGAL_RANGE << (bit_depth - 8),
            MAX_LUMA_LEGAL_RANGE << (bit_depth - 8),
            MIN_CHROMA_LEGAL_RANGE << (bit_depth - 8),
            chroma_max * scale,
        )
    } else {
        let maximum = (256i32 << (bit_depth - 8)) - 1;
        (0, maximum, 0, maximum)
    };

    for row in 0..(half_luma_height << (1 - sub_y)) {
        for col in 0..(half_luma_width << (1 - sub_x)) {
            let luma_row = row << sub_y;
            let luma_col = col << sub_x;
            let average_luma = if sub_x != 0 {
                let left =
                    image.y[(luma_origin.1 + luma_row) * image.width + luma_origin.0 + luma_col];
                let right = image.y
                    [(luma_origin.1 + luma_row) * image.width + luma_origin.0 + luma_col + 1];
                (i32::from(left) + i32::from(right) + 1) >> 1
            } else {
                i32::from(
                    image.y[(luma_origin.1 + luma_row) * image.width + luma_origin.0 + luma_col],
                )
            };

            if apply_cb {
                let index = (chroma_origin.1 + row) * image.chroma_width + chroma_origin.0 + col;
                let value = i32::from(image.cb[index]);
                let scaled_index =
                    ((average_luma * cb_luma_mult + cb_mult * value) >> 6) + cb_offset;
                let noise = scale_lut(
                    &luts.cb,
                    scaled_index.clamp(0, (256i32 << (bit_depth - 8)) - 1) as u16,
                    bit_depth,
                ) * cb_grain.0[cb_grain.2 + row * cb_grain.1 + col];
                image.cb[index] =
                    (value + ((noise + rounding) >> shift)).clamp(min_chroma, max_chroma) as u16;
            }
            if apply_cr {
                let index = (chroma_origin.1 + row) * image.chroma_width + chroma_origin.0 + col;
                let value = i32::from(image.cr[index]);
                let scaled_index =
                    ((average_luma * cr_luma_mult + cr_mult * value) >> 6) + cr_offset;
                let noise = scale_lut(
                    &luts.cr,
                    scaled_index.clamp(0, (256i32 << (bit_depth - 8)) - 1) as u16,
                    bit_depth,
                ) * cr_grain.0[cr_grain.2 + row * cr_grain.1 + col];
                image.cr[index] =
                    (value + ((noise + rounding) >> shift)).clamp(min_chroma, max_chroma) as u16;
            }
        }
    }

    if apply_y {
        for row in 0..(half_luma_height << 1) {
            for col in 0..(half_luma_width << 1) {
                let index = (luma_origin.1 + row) * image.width + luma_origin.0 + col;
                let value = image.y[index];
                let noise = scale_lut(&luts.y, value, bit_depth)
                    * luma_grain.0[luma_grain.2 + row * luma_grain.1 + col];
                image.y[index] = (i32::from(value) + ((noise + rounding) >> shift))
                    .clamp(min_luma, max_luma) as u16;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn vertical_overlap(
    destination: &mut [i32],
    stride: usize,
    right: &[i32],
    right_stride: usize,
    right_offset: usize,
    width: usize,
    height: usize,
    grain_min: i32,
    grain_max: i32,
) {
    for row in 0..height {
        let left_base = row * stride;
        let right_base = right_offset + row * right_stride;
        let destination_base = row * stride;
        if width == 1 {
            let value = (destination[left_base] * 23 + right[right_base] * 22 + 16) >> 5;
            destination[destination_base] = value.clamp(grain_min, grain_max);
        } else {
            let first = (destination[left_base] * 27 + right[right_base] * 17 + 16) >> 5;
            let second = (destination[left_base + 1] * 17 + right[right_base + 1] * 27 + 16) >> 5;
            destination[destination_base] = first.clamp(grain_min, grain_max);
            destination[destination_base + 1] = second.clamp(grain_min, grain_max);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn horizontal_overlap(
    destination: &mut [i32],
    stride: usize,
    destination_offset: usize,
    bottom: &[i32],
    bottom_stride: usize,
    bottom_offset: usize,
    width: usize,
    height: usize,
    grain_min: i32,
    grain_max: i32,
) {
    for col in 0..width {
        let top_base = destination_offset + col;
        let bottom_base = bottom_offset + col;
        let destination_base = destination_offset + col;
        if height == 1 {
            let value = (destination[top_base] * 23 + bottom[bottom_base] * 22 + 16) >> 5;
            destination[destination_base] = value.clamp(grain_min, grain_max);
        } else {
            let first = (destination[top_base] * 27 + bottom[bottom_base] * 17 + 16) >> 5;
            let second = (destination[top_base + stride] * 17
                + bottom[bottom_base + bottom_stride] * 27
                + 16)
                >> 5;
            destination[destination_base] = first.clamp(grain_min, grain_max);
            destination[destination_base + stride] = second.clamp(grain_min, grain_max);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_area(
    source: &[i32],
    source_stride: usize,
    source_offset: usize,
    destination: &mut [i32],
    destination_stride: usize,
    destination_offset: usize,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let source_start = source_offset + row * source_stride;
        let destination_start = destination_offset + row * destination_stride;
        destination[destination_start..destination_start + width]
            .copy_from_slice(&source[source_start..source_start + width]);
    }
}

fn round_shift(value: i32, shift: i32) -> i32 {
    (value + ((1 << shift) >> 1)) >> shift
}
