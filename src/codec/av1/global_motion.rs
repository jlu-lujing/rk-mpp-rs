//! AV1 global-motion parameter reconstruction.
//!
//! MPP reads the AV1 syntax-space values into a separate matrix. The hardware
//! packet then consumes the reconstructed fixed-point matrix and its quantized
//! affine shear parameters.

use super::parser::{FrameHeader, FrameType, ReferenceFrame};

const DIV_LUT_BITS: u32 = 8;
const DIV_LUT_PREC_BITS: u32 = 14;
const GM_ABS_ALPHA_BITS: u32 = 12;
const GM_ALPHA_PREC_BITS: u32 = 15;
const GM_ABS_TRANS_ONLY_BITS: u32 = 9;
const GM_TRANS_ONLY_PREC_BITS: u32 = 3;
const GM_ABS_TRANS_BITS: u32 = 12;
const GM_TRANS_PREC_BITS: u32 = 6;
const WARPEDMODEL_PREC_BITS: u32 = 16;
const WARP_PARAM_REDUCE_BITS: u32 = 6;
const PRIMARY_REF_NONE: u8 = 7;

const DIV_LUT: [i16; 257] = [
    16384, 16320, 16257, 16194, 16132, 16070, 16009, 15948, 15888, 15828, 15768, 15709, 15650,
    15592, 15534, 15477, 15420, 15364, 15308, 15252, 15197, 15142, 15087, 15033, 14980, 14926,
    14873, 14821, 14769, 14717, 14665, 14614, 14564, 14513, 14463, 14413, 14364, 14315, 14266,
    14218, 14170, 14122, 14075, 14028, 13981, 13935, 13888, 13843, 13797, 13752, 13707, 13662,
    13618, 13574, 13530, 13487, 13443, 13400, 13358, 13315, 13273, 13231, 13190, 13148, 13107,
    13066, 13026, 12985, 12945, 12906, 12866, 12827, 12788, 12749, 12710, 12672, 12633, 12596,
    12558, 12520, 12483, 12446, 12409, 12373, 12336, 12300, 12264, 12228, 12193, 12157, 12122,
    12087, 12053, 12018, 11984, 11950, 11916, 11882, 11848, 11815, 11782, 11749, 11716, 11683,
    11651, 11619, 11586, 11555, 11523, 11491, 11460, 11429, 11398, 11367, 11336, 11305, 11275,
    11245, 11215, 11185, 11155, 11125, 11096, 11067, 11038, 11009, 10980, 10951, 10923, 10894,
    10866, 10838, 10810, 10782, 10755, 10727, 10700, 10673, 10645, 10618, 10592, 10565, 10538,
    10512, 10486, 10460, 10434, 10408, 10382, 10356, 10331, 10305, 10280, 10255, 10230, 10205,
    10180, 10156, 10131, 10107, 10082, 10058, 10034, 10010, 9986, 9963, 9939, 9916, 9892, 9869,
    9846, 9823, 9800, 9777, 9754, 9732, 9709, 9687, 9664, 9642, 9620, 9598, 9576, 9554, 9533, 9511,
    9489, 9468, 9447, 9425, 9404, 9383, 9362, 9341, 9321, 9300, 9279, 9259, 9239, 9218, 9198, 9178,
    9158, 9138, 9118, 9098, 9079, 9059, 9039, 9020, 9001, 8981, 8962, 8943, 8924, 8905, 8886, 8867,
    8849, 8830, 8812, 8793, 8775, 8756, 8738, 8720, 8702, 8684, 8666, 8648, 8630, 8613, 8595, 8577,
    8560, 8542, 8525, 8508, 8490, 8473, 8456, 8439, 8422, 8405, 8389, 8372, 8355, 8339, 8322, 8306,
    8289, 8273, 8257, 8240, 8224, 8208, 8192,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarpedMotion {
    pub wmtype: u8,
    pub wmmat: [i32; 6],
    pub wmmat_val: [i32; 6],
    pub alpha: i32,
    pub beta: i32,
    pub gamma: i32,
    pub delta: i32,
}

impl WarpedMotion {
    pub const fn identity() -> Self {
        Self {
            wmtype: 0,
            wmmat: [
                0,
                0,
                1 << WARPEDMODEL_PREC_BITS,
                0,
                0,
                1 << WARPEDMODEL_PREC_BITS,
            ],
            wmmat_val: [0; 6],
            alpha: 0,
            beta: 0,
            gamma: 0,
            delta: 0,
        }
    }

    fn invalid_default() -> Self {
        Self {
            alpha: 1 << WARPEDMODEL_PREC_BITS,
            beta: 1 << WARPEDMODEL_PREC_BITS,
            gamma: 1 << WARPEDMODEL_PREC_BITS,
            delta: 1 << WARPEDMODEL_PREC_BITS,
            ..Self::identity()
        }
    }
}

impl Default for WarpedMotion {
    fn default() -> Self {
        Self::identity()
    }
}

pub fn resolve(header: &FrameHeader, references: &[ReferenceFrame; 8]) -> [WarpedMotion; 8] {
    let mut result = [WarpedMotion::identity(); 8];
    if matches!(header.frame_type, FrameType::Key | FrameType::IntraOnly) {
        return result;
    }

    let primary_reference = if header.primary_ref_frame == PRIMARY_REF_NONE {
        None
    } else {
        let slot = header.ref_frame_idx[usize::from(header.primary_ref_frame)] as usize;
        Some(references[slot].warped_motion)
    };

    for reference in 1..8 {
        let global = header.global_motion[reference];
        let model = if global.identity {
            0
        } else if global.rotation_zoom {
            2
        } else if global.translation {
            1
        } else {
            3
        };
        let mut warped = WarpedMotion {
            wmtype: model,
            ..WarpedMotion::identity()
        };

        if model >= 2 {
            set_parameter(
                &mut warped,
                primary_reference,
                reference,
                model,
                header.allow_high_precision_mv,
                2,
                global.parameters[2],
            );
            set_parameter(
                &mut warped,
                primary_reference,
                reference,
                model,
                header.allow_high_precision_mv,
                3,
                global.parameters[3],
            );
            if model == 3 {
                set_parameter(
                    &mut warped,
                    primary_reference,
                    reference,
                    model,
                    header.allow_high_precision_mv,
                    4,
                    global.parameters[4],
                );
                set_parameter(
                    &mut warped,
                    primary_reference,
                    reference,
                    model,
                    header.allow_high_precision_mv,
                    5,
                    global.parameters[5],
                );
            } else {
                warped.wmmat[4] = -warped.wmmat[3];
                warped.wmmat[5] = warped.wmmat[2];
            }
        }
        if model >= 1 {
            set_parameter(
                &mut warped,
                primary_reference,
                reference,
                model,
                header.allow_high_precision_mv,
                0,
                global.parameters[0],
            );
            set_parameter(
                &mut warped,
                primary_reference,
                reference,
                model,
                header.allow_high_precision_mv,
                1,
                global.parameters[1],
            );
        }

        let slot = header.ref_frame_idx[reference - 1] as usize;
        let reference_uses_scaling = header.width != references[slot].frame_width
            || header.height != references[slot].frame_height;
        if !shear_parameters_valid(&mut warped) || reference_uses_scaling {
            warped = WarpedMotion::invalid_default();
        }
        result[reference] = warped;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn set_parameter(
    warped: &mut WarpedMotion,
    primary_reference: Option<[WarpedMotion; 8]>,
    reference: usize,
    model: u8,
    allow_high_precision_mv: bool,
    index: usize,
    syntax_value: i32,
) {
    let previous = primary_reference.map_or_else(
        || WarpedMotion::identity().wmmat[index],
        |motion| motion[reference].wmmat[index],
    );
    let (abs_bits, precision_bits) = if index < 2 {
        if model == 1 {
            let high_precision = u32::from(allow_high_precision_mv);
            (
                GM_ABS_TRANS_ONLY_BITS + high_precision - 1,
                GM_TRANS_ONLY_PREC_BITS + high_precision - 1,
            )
        } else {
            (GM_ABS_TRANS_BITS, GM_TRANS_PREC_BITS)
        }
    } else {
        (GM_ABS_ALPHA_BITS, GM_ALPHA_PREC_BITS)
    };
    let round = if index % 3 == 2 {
        1 << WARPEDMODEL_PREC_BITS
    } else {
        0
    };
    let precision_difference = WARPEDMODEL_PREC_BITS - precision_bits;
    let sub = if index % 3 == 2 {
        1 << precision_bits
    } else {
        0
    };
    let maximum = 1i32 << abs_bits;
    let previous_rounded = (previous >> precision_difference) - sub;
    let value = decode_signed_subexp(syntax_value as u32, -maximum, maximum + 1, previous_rounded);
    warped.wmmat_val[index] = value;
    warped.wmmat[index] = (value << precision_difference) + round;
}

fn decode_signed_subexp(sub_exp: u32, low: i32, high: i32, reference: i32) -> i32 {
    let high = high - low;
    let reference = reference - low;
    let value = if high < (reference << 1) {
        high - 1 - inverse_recenter_nonnegative(high - 1 - reference, sub_exp)
    } else {
        inverse_recenter_nonnegative(reference, sub_exp)
    };
    value + low
}

fn inverse_recenter_nonnegative(reference: i32, value: u32) -> i32 {
    if value > (reference << 1) as u32 {
        value as i32
    } else if value & 1 != 0 {
        reference - ((value + 1) >> 1) as i32
    } else {
        reference + (value >> 1) as i32
    }
}

fn shear_parameters_valid(warped: &mut WarpedMotion) -> bool {
    let matrix = warped.wmmat;
    if matrix[2] < 0 {
        return false;
    }

    let clip_i16 = |value: i32| value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    let mut alpha = clip_i16(matrix[2] - (1 << WARPEDMODEL_PREC_BITS));
    let mut beta = clip_i16(matrix[3]);
    let mut shift = 0u16;
    let divisor = resolve_divisor(matrix[2].unsigned_abs(), &mut shift);
    let v = i64::from(matrix[4]) * (1 << WARPEDMODEL_PREC_BITS);
    let w = i64::from(matrix[3]) * i64::from(matrix[4]);
    let mut gamma = clip_i16(round_power_of_two_signed(v * i64::from(divisor), shift) as i32);
    let mut delta = clip_i16(
        matrix[5]
            - round_power_of_two_signed(w * i64::from(divisor), shift) as i32
            - (1 << WARPEDMODEL_PREC_BITS),
    );

    let reduce_shift = WARP_PARAM_REDUCE_BITS as u16;
    alpha = ((round_power_of_two_signed(i64::from(alpha), reduce_shift) << WARP_PARAM_REDUCE_BITS)
        as i32) as i16;
    beta = ((round_power_of_two_signed(i64::from(beta), reduce_shift) << WARP_PARAM_REDUCE_BITS)
        as i32) as i16;
    gamma = ((round_power_of_two_signed(i64::from(gamma), reduce_shift) << WARP_PARAM_REDUCE_BITS)
        as i32) as i16;
    delta = ((round_power_of_two_signed(i64::from(delta), reduce_shift) << WARP_PARAM_REDUCE_BITS)
        as i32) as i16;

    warped.alpha = i32::from(alpha);
    warped.beta = i32::from(beta);
    warped.gamma = i32::from(gamma);
    warped.delta = i32::from(delta);

    let out_of_range = 4 * i32::from(alpha.unsigned_abs()) + 7 * i32::from(beta.unsigned_abs())
        >= (1 << WARPEDMODEL_PREC_BITS)
        || 4 * i32::from(gamma.unsigned_abs()) + 4 * i32::from(delta.unsigned_abs())
            >= (1 << WARPEDMODEL_PREC_BITS);
    !out_of_range
}

fn resolve_divisor(divisor: u32, shift: &mut u16) -> i16 {
    let exponent = 31 - divisor.leading_zeros();
    let remainder = divisor - (1 << exponent);
    let fraction = if exponent > DIV_LUT_BITS {
        round_power_of_two(
            u64::from(remainder),
            u16::try_from(exponent - DIV_LUT_BITS).unwrap(),
        )
    } else {
        u64::from(remainder << (DIV_LUT_BITS - exponent))
    };
    *shift = u16::try_from(exponent + DIV_LUT_PREC_BITS).unwrap();
    DIV_LUT[fraction as usize]
}

fn round_power_of_two(value: u64, shift: u16) -> u64 {
    if shift == 0 {
        value
    } else {
        (value + (1 << (shift - 1))) >> shift
    }
}

fn round_power_of_two_signed(value: i64, shift: u16) -> i64 {
    if value < 0 {
        -(round_power_of_two(value.unsigned_abs(), shift) as i64)
    } else {
        round_power_of_two(value as u64, shift) as i64
    }
}
