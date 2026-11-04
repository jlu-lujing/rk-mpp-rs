//! AV1 loop restoration, specification section 7.17.
//!
//! The implementation covers the Wiener and self-guided restoration filters.
//! The source sample process deliberately switches between the CDEF output in
//! the current 64-luma-line stripe and the deblocked, but un-CDEFed, current
//! frame immediately outside it.

use crate::reference::av1::cdef::PlaneView;

pub const FILTER_BITS: u32 = 7;
pub const SGRPROJ_PARAMS_BITS: u32 = 4;
pub const SGRPROJ_PRJ_BITS: u32 = 7;
pub const SGRPROJ_RST_BITS: u32 = 4;
pub const SGRPROJ_MTABLE_BITS: u32 = 20;
pub const SGRPROJ_RECIP_BITS: u32 = 12;
pub const SGRPROJ_SGR_BITS: u32 = 8;
pub const SGRPROJ_PRJ_SUBEXP_K: u32 = 4;

pub const WIENER_TAPS_MID: [i32; 3] = [3, -7, 15];
pub const WIENER_TAPS_MIN: [i32; 3] = [-5, -23, -17];
pub const WIENER_TAPS_MAX: [i32; 3] = [10, 8, 46];
pub const WIENER_TAPS_K: [u32; 3] = [1, 2, 3];
pub const SGRPROJ_XQD_MID: [i32; 2] = [-32, 31];
pub const SGRPROJ_XQD_MIN: [i32; 2] = [-96, -32];
pub const SGRPROJ_XQD_MAX: [i32; 2] = [31, 95];

/// `Sgr_Params[16][4]`: radius, eps, radius, eps for the two box passes.
pub const SGR_PARAMS: [[i32; 4]; 16] = [
    [2, 12, 1, 4],
    [2, 15, 1, 6],
    [2, 18, 1, 8],
    [2, 21, 1, 9],
    [2, 24, 1, 10],
    [2, 29, 1, 11],
    [2, 36, 1, 12],
    [2, 45, 1, 13],
    [2, 56, 1, 14],
    [2, 68, 1, 15],
    [0, 0, 1, 5],
    [0, 0, 1, 8],
    [0, 0, 1, 11],
    [0, 0, 1, 14],
    [2, 30, 0, 0],
    [2, 75, 0, 0],
];

/// `FrameRestorationType` / `restoration_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RestorationType {
    None = 0,
    Wiener = 1,
    SelfGuided = 2,
    Switchable = 3,
}

impl RestorationType {
    /// Remap the frame-header `lr_type` value to `FrameRestorationType`.
    pub fn from_header_bits(value: u8) -> Self {
        match value {
            1 => Self::Switchable,
            2 => Self::Wiener,
            3 => Self::SelfGuided,
            _ => Self::None,
        }
    }

    /// Convert a decoded tile `restoration_type` syntax value.
    pub fn from_unit_symbol(value: usize) -> Self {
        match value {
            1 => Self::Wiener,
            2 => Self::SelfGuided,
            3 => Self::Switchable,
            _ => Self::None,
        }
    }
}

/// Parameters selected for one restoration unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LrFilter {
    Wiener {
        /// Three vertical coefficients, sent first.
        vertical: [i32; 3],
        /// Three horizontal coefficients, sent second.
        horizontal: [i32; 3],
    },
    SelfGuided {
        set: usize,
        /// `LrSgrXqd[plane][unitRow][unitCol][0..1]`.
        xqd: [i32; 2],
    },
}

/// Inputs to the loop-restoration source sample process for one stripe.
#[derive(Debug, Clone, Copy)]
pub struct LrSource<'a> {
    pub cdef: PlaneView<'a>,
    pub current: PlaneView<'a>,
    pub stripe_start_y: isize,
    pub stripe_end_y: isize,
    pub plane_end_x: isize,
    pub plane_end_y: isize,
}

impl<'a> LrSource<'a> {
    /// Build a source for `stripeNum`: the 64-luma-sample stripe starts eight
    /// luma lines above its nominal position and is divided by the plane's
    /// vertical subsampling.
    pub fn for_stripe(
        cdef: PlaneView<'a>,
        current: PlaneView<'a>,
        stripe_num: u32,
        sub_y: u32,
    ) -> Self {
        assert_eq!(cdef.width, current.width);
        assert_eq!(cdef.height, current.height);
        let stripe_start_y = ((-8 + stripe_num as i32 * 64) >> sub_y) as isize;
        let stripe_end_y = stripe_start_y + ((64 >> sub_y) - 1) as isize;
        Self {
            cdef,
            current,
            stripe_start_y,
            stripe_end_y,
            plane_end_x: cdef.width as isize - 1,
            plane_end_y: cdef.height as isize - 1,
        }
    }

    /// `get_source_sample`: clip to the plane, then select the CDEF or current
    /// frame and clamp requests three lines outside a stripe to its second line.
    pub fn sample(&self, x: isize, y: isize) -> u16 {
        let x = x.clamp(0, self.plane_end_x) as usize;
        let mut y = y.clamp(0, self.plane_end_y);
        let plane = if y < self.stripe_start_y {
            y = (self.stripe_start_y - 2).max(y);
            self.current
        } else if y > self.stripe_end_y {
            y = (self.stripe_end_y + 2).min(y);
            self.current
        } else {
            self.cdef
        };
        plane.samples[y as usize * plane.stride + x]
    }
}

/// `Wiener coefficient process`: three transmitted coefficients become the
/// symmetric seven-tap filter with unit DC gain.
pub fn wiener_coefficients(coeff: [i32; 3]) -> [i32; 7] {
    [
        coeff[0],
        coeff[1],
        coeff[2],
        128 - 2 * (coeff[0] + coeff[1] + coeff[2]),
        coeff[2],
        coeff[1],
        coeff[0],
    ]
}

fn inter_rounds(bit_depth: u32) -> (u32, u32) {
    if bit_depth == 12 {
        (5, 9)
    } else {
        (3, 11)
    }
}

fn round2(value: i64, shift: u32) -> i64 {
    if shift == 0 {
        value
    } else {
        (value + (1 << (shift - 1))) >> shift
    }
}

fn clip1(value: i64, bit_depth: u32) -> u16 {
    value.clamp(0, (1i64 << bit_depth) - 1) as u16
}

/// `Wiener filter process` for one restoration block.
#[allow(clippy::too_many_arguments)]
pub fn wiener_filter(
    source: LrSource<'_>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    vertical: [i32; 3],
    horizontal: [i32; 3],
    bit_depth: u32,
) -> Vec<u16> {
    let (inter_round0, inter_round1) = inter_rounds(bit_depth);
    let offset = 1i64 << (bit_depth + FILTER_BITS - inter_round0 - 1);
    let limit = (1i64 << (bit_depth + 1 + FILTER_BITS - inter_round0)) - 1;
    let hfilter = wiener_coefficients(horizontal);
    let vfilter = wiener_coefficients(vertical);

    let mut intermediate = vec![0i64; (height + 6) * width];
    for r in 0..height + 6 {
        for c in 0..width {
            let mut sum = 0i64;
            for (t, coeff) in hfilter.iter().enumerate() {
                sum += i64::from(*coeff)
                    * i64::from(source.sample(
                        x as isize + c as isize + t as isize - 3,
                        y as isize + r as isize - 3,
                    ));
            }
            intermediate[r * width + c] = round2(sum, inter_round0).clamp(-offset, limit - offset);
        }
    }

    let mut out = vec![0u16; width * height];
    for r in 0..height {
        for c in 0..width {
            let mut sum = 0i64;
            for (t, coeff) in vfilter.iter().enumerate() {
                sum += i64::from(*coeff) * intermediate[(r + t) * width + c];
            }
            out[r * width + c] = clip1(round2(sum, inter_round1), bit_depth);
        }
    }
    out
}

/// `Box filter process`. `pass` selects the first (sparse) or second (full)
/// self-guided smoothing pass.
#[allow(clippy::too_many_arguments)]
fn box_filter(
    source: LrSource<'_>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    set: usize,
    pass: usize,
    bit_depth: u32,
) -> Vec<i64> {
    let params = SGR_PARAMS[set];
    let r = params[pass * 2];
    if r == 0 {
        return vec![0; width * height];
    }
    let eps = i64::from(params[pass * 2 + 1]);
    let n = i64::from((2 * r + 1) * (2 * r + 1));
    let n2e = n * n * eps;
    let s = ((1i64 << SGRPROJ_MTABLE_BITS) + n2e / 2) / n2e;
    let aw = width + 2;
    let ah = height + 2;
    let mut a = vec![0i64; aw * ah];
    let mut b = vec![0i64; aw * ah];
    for i in -1..=height as isize {
        for j in -1..=width as isize {
            let mut sum_a = 0i64;
            let mut sum_b = 0i64;
            for dy in -r..=r {
                for dx in -r..=r {
                    let c = i64::from(
                        source.sample(x as isize + j + dx as isize, y as isize + i + dy as isize),
                    );
                    sum_a += c * c;
                    sum_b += c;
                }
            }
            let sample_a = round2(sum_a, 2 * (bit_depth - 8));
            let sample_b = round2(sum_b, bit_depth - 8);
            let variance = 0i64.max(sample_a * n - sample_b * sample_b);
            let z = round2(variance * s, SGRPROJ_MTABLE_BITS);
            let a2 = if z >= 255 {
                256
            } else if z == 0 {
                1
            } else {
                ((z << SGRPROJ_SGR_BITS) + z / 2) / (z + 1)
            };
            let reciprocal = ((1i64 << SGRPROJ_RECIP_BITS) + n / 2) / n;
            let b2 = ((1i64 << SGRPROJ_SGR_BITS) - a2) * sum_b * reciprocal;
            let index = (i + 1) as usize * aw + (j + 1) as usize;
            a[index] = a2;
            b[index] = round2(b2, SGRPROJ_RECIP_BITS);
        }
    }

    let mut out = vec![0i64; width * height];
    for i in 0..height {
        let shift = if pass == 0 && (i & 1) != 0 { 4 } else { 5 };
        for j in 0..width {
            let mut sum_a = 0i64;
            let mut sum_b = 0i64;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let weight = if pass == 0 {
                        if ((i as isize + dy) & 1) != 0 {
                            if dx == 0 {
                                6
                            } else {
                                5
                            }
                        } else {
                            0
                        }
                    } else if dx == 0 || dy == 0 {
                        4
                    } else {
                        3
                    };
                    let index =
                        (i as isize + dy + 1) as usize * aw + (j as isize + dx + 1) as usize;
                    sum_a += weight * a[index];
                    sum_b += weight * b[index];
                }
            }
            let sample = i64::from(source.cdef.samples[(y + i) * source.cdef.stride + x + j]);
            let value = sum_a * sample + sum_b;
            out[i * width + j] = round2(value, SGRPROJ_SGR_BITS + shift - SGRPROJ_RST_BITS);
        }
    }
    out
}

/// `Self guided filter process` for one restoration block.
pub fn self_guided_filter(
    source: LrSource<'_>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    set: usize,
    xqd: [i32; 2],
    bit_depth: u32,
) -> Vec<u16> {
    let flt0 = box_filter(source, x, y, width, height, set, 0, bit_depth);
    let flt1 = box_filter(source, x, y, width, height, set, 1, bit_depth);
    let w0 = i64::from(xqd[0]);
    let w1 = i64::from(xqd[1]);
    let w2 = (1i64 << SGRPROJ_PRJ_BITS) - w0 - w1;
    let r0 = SGR_PARAMS[set][0];
    let r1 = SGR_PARAMS[set][2];
    let mut out = vec![0u16; width * height];
    for i in 0..height {
        for j in 0..width {
            let sample = i64::from(source.cdef.samples[(y + i) * source.cdef.stride + x + j]);
            let u = sample << SGRPROJ_RST_BITS;
            let mut value = w1 * u;
            value += if r0 != 0 {
                w0 * flt0[i * width + j]
            } else {
                w0 * u
            };
            value += if r1 != 0 {
                w2 * flt1[i * width + j]
            } else {
                w2 * u
            };
            out[i * width + j] = clip1(
                round2(value, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS),
                bit_depth,
            );
        }
    }
    out
}

/// Apply the selected restoration unit filter to a rectangular block whose
/// samples all belong to the same restoration unit.
pub fn loop_restore_block(
    source: LrSource<'_>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    filter: Option<LrFilter>,
    bit_depth: u32,
) -> Vec<u16> {
    match filter {
        Some(LrFilter::Wiener {
            vertical,
            horizontal,
        }) => wiener_filter(source, x, y, width, height, vertical, horizontal, bit_depth),
        Some(LrFilter::SelfGuided { set, xqd }) => {
            self_guided_filter(source, x, y, width, height, set, xqd, bit_depth)
        }
        None => {
            let mut out = vec![0u16; width * height];
            for i in 0..height {
                let row = (y + i) * source.cdef.stride + x;
                out[i * width..(i + 1) * width]
                    .copy_from_slice(&source.cdef.samples[row..row + width]);
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_views<'a>(
        cdef_samples: &'a [u16],
        current_samples: &'a [u16],
        n: usize,
    ) -> (PlaneView<'a>, PlaneView<'a>) {
        (
            PlaneView::new(cdef_samples, n, n, n),
            PlaneView::new(current_samples, n, n, n),
        )
    }

    #[test]
    fn constants_match_the_specification() {
        assert_eq!(FILTER_BITS, 7);
        assert_eq!(SGR_PARAMS[0], [2, 12, 1, 4]);
        assert_eq!(SGR_PARAMS[15], [2, 75, 0, 0]);
        assert_eq!(
            RestorationType::from_header_bits(1),
            RestorationType::Switchable
        );
        assert_eq!(
            RestorationType::from_header_bits(2),
            RestorationType::Wiener
        );
        assert_eq!(
            RestorationType::from_header_bits(3),
            RestorationType::SelfGuided
        );
        assert_eq!(
            RestorationType::from_unit_symbol(1),
            RestorationType::Wiener
        );
        assert_eq!(
            RestorationType::from_unit_symbol(2),
            RestorationType::SelfGuided
        );
    }

    #[test]
    fn wiener_coefficients_are_symmetric_with_unit_dc_gain() {
        let filter = wiener_coefficients([3, -7, 15]);
        assert_eq!(filter, [3, -7, 15, 106, 15, -7, 3]);
        assert_eq!(filter.iter().sum::<i32>(), 128);
    }

    #[test]
    fn source_selection_switches_at_the_shifted_stripe_boundary() {
        let cdef = [100u16; 4096];
        let current = [200u16; 4096];
        let source = LrSource::for_stripe(
            PlaneView::new(&cdef, 64, 64, 64),
            PlaneView::new(&current, 64, 64, 64),
            0,
            0,
        );
        assert_eq!((source.stripe_start_y, source.stripe_end_y), (-8, 55));
        // Coordinates are clipped to the plane before stripe selection.
        assert_eq!(source.sample(0, -20), 100);
        assert_eq!(source.sample(0, -8), 100);
        assert_eq!(source.sample(0, 55), 100);
        assert_eq!(source.sample(0, 56), 200);
    }

    #[test]
    fn wiener_filter_preserves_a_flat_block() {
        let cdef = [128u16; 64];
        let current = [128u16; 64];
        let (cdef, current) = flat_views(&cdef, &current, 8);
        let source = LrSource::for_stripe(cdef, current, 0, 0);
        let out = wiener_filter(source, 0, 0, 8, 4, [0, 0, 0], [0, 0, 0], 8);
        assert_eq!(out, [128; 32]);
    }

    #[test]
    fn wiener_filter_matches_the_independent_spec_vector() {
        let cdef = [
            0u16, 53, 106, 159, 212, 9, 62, 115, 37, 101, 165, 229, 37, 101, 165, 229, 74, 149,
            224, 43, 118, 193, 12, 87, 111, 197, 27, 113, 199, 29, 115, 201, 148, 245, 86, 183, 24,
            121, 218, 59, 185, 37, 145, 253, 105, 213, 65, 173, 222, 85, 204, 67, 186, 49, 168, 31,
            3, 133, 7, 137, 11, 141, 15, 145,
        ];
        let current = [200u16; 64];
        let (cdef, current) = flat_views(&cdef, &current, 8);
        let source = LrSource::for_stripe(cdef, current, 0, 0);
        let out = wiener_filter(source, 0, 0, 8, 4, [3, -7, 15], [2, 4, -9], 8);
        assert_eq!(
            out,
            [
                5, 60, 100, 163, 201, 5, 87, 133, 39, 100, 164, 210, 47, 118, 141, 199, 74, 139,
                208, 59, 120, 182, 22, 128, 102, 215, 42, 99, 192, 38, 121, 165,
            ]
        );
    }

    #[test]
    fn self_guided_filter_preserves_a_flat_block() {
        let cdef = [128u16; 64];
        let current = [128u16; 64];
        let (cdef, current) = flat_views(&cdef, &current, 8);
        let source = LrSource::for_stripe(cdef, current, 0, 0);
        let out = self_guided_filter(source, 0, 0, 8, 4, 0, [0, 96], 8);
        assert_eq!(out, [128; 32]);
    }

    #[test]
    fn self_guided_filter_matches_the_independent_spec_vector() {
        let mut cdef = [128u16; 64];
        cdef[3 * 8 + 4] = 255;
        let current = [128u16; 64];
        let (cdef, current) = flat_views(&cdef, &current, 8);
        let source = LrSource::for_stripe(cdef, current, 0, 0);
        let out = self_guided_filter(source, 0, 0, 8, 4, 15, [32, 72], 8);
        let mut expected = [128u16; 32];
        expected[3 * 8 + 4] = 252;
        assert_eq!(out, expected);
    }

    #[test]
    fn none_restoration_copies_the_cdef_block() {
        let cdef: Vec<u16> = (0..64).collect();
        let current = [0u16; 64];
        let (cdef, current) = flat_views(&cdef, &current, 8);
        let source = LrSource::for_stripe(cdef, current, 0, 0);
        let out = loop_restore_block(source, 2, 3, 4, 2, None, 8);
        assert_eq!(out, [26, 27, 28, 29, 34, 35, 36, 37]);
    }
}
