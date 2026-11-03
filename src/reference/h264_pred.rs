//! H.264 intra prediction primitives.
//!
//! Neighbour samples are supplied in prediction order and use `u16` storage so
//! the same formulas cover 8-bit and 10-bit output. Reference-availability
//! substitution and frame-edge loading are handled by the caller.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intra4x4Neighbors {
    pub top: Option<[u16; 4]>,
    pub left: Option<[u16; 4]>,
    pub top_right: Option<[u16; 4]>,
    pub top_left: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intra8x8Neighbors {
    pub top: Option<[u16; 8]>,
    pub left: Option<[u16; 8]>,
    pub top_right: Option<[u16; 8]>,
    pub top_left: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionError {
    InvalidMode(u8),
    InvalidBitDepth(u8),
    MissingTop,
    MissingLeft,
    MissingTopLeft,
    Unsupported(&'static str),
}

impl fmt::Display for PredictionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PredictionError::InvalidMode(mode) => write!(f, "invalid H.264 intra mode {mode}"),
            PredictionError::InvalidBitDepth(depth) => {
                write!(f, "H.264 prediction supports 8..=10 bit, got {depth}")
            }
            PredictionError::MissingTop => write!(f, "H.264 intra prediction needs top samples"),
            PredictionError::MissingLeft => write!(f, "H.264 intra prediction needs left samples"),
            PredictionError::MissingTopLeft => {
                write!(f, "H.264 intra prediction needs the top-left sample")
            }
            PredictionError::Unsupported(what) => {
                write!(f, "unsupported H.264 prediction: {what}")
            }
        }
    }
}

impl std::error::Error for PredictionError {}

macro_rules! set {
    ($output:ident, $index:expr, $value:expr) => {{
        let value = $value;
        $output[$index] = value;
    }};
}

/// Predict one 4x4 block using modes 0..8.
pub fn predict_4x4(
    mode: u8,
    bit_depth: u8,
    neighbors: &Intra4x4Neighbors,
) -> Result<[u16; 16], PredictionError> {
    validate_depth(bit_depth)?;
    if mode == 2 {
        let dc = match (neighbors.top, neighbors.left) {
            (Some(top), Some(left)) => {
                (top.iter()
                    .chain(left.iter())
                    .map(|&v| u32::from(v))
                    .sum::<u32>()
                    + 4)
                    >> 3
            }
            (Some(top), None) => (top.iter().map(|&v| u32::from(v)).sum::<u32>() + 2) >> 2,
            (None, Some(left)) => (left.iter().map(|&v| u32::from(v)).sum::<u32>() + 2) >> 2,
            (None, None) => 1 << (bit_depth - 1),
        };
        return Ok([dc as u16; 16]);
    }
    if matches!(mode, 0 | 3 | 7) && neighbors.top.is_none() {
        return Err(PredictionError::MissingTop);
    }
    if matches!(mode, 1 | 8) && neighbors.left.is_none() {
        return Err(PredictionError::MissingLeft);
    }
    if matches!(mode, 4 | 5 | 6) {
        neighbors.top.ok_or(PredictionError::MissingTop)?;
        neighbors.left.ok_or(PredictionError::MissingLeft)?;
    }
    let top = neighbors.top.unwrap_or([0; 4]);
    let left = neighbors.left.unwrap_or([0; 4]);
    let top_right = neighbors.top_right.unwrap_or([top[3]; 4]);
    let top_left = neighbors.top_left.ok_or(PredictionError::MissingTopLeft)?;
    let mut out = [0u16; 16];
    match mode {
        0 => {
            for row in 0..4 {
                out[row * 4..row * 4 + 4].copy_from_slice(&top);
            }
        }
        1 => {
            for (row, &sample) in left.iter().enumerate() {
                out[row * 4..row * 4 + 4].fill(sample);
            }
        }
        3 => {
            let t = [
                top[0],
                top[1],
                top[2],
                top[3],
                top_right[0],
                top_right[1],
                top_right[2],
                top_right[3],
            ];
            out[0] = avg3(t[0], t[1], t[2]);
            out[1] = avg3(t[1], t[2], t[3]);
            out[4] = out[1];
            out[2] = avg3(t[2], t[3], t[4]);
            out[5] = out[2];
            out[8] = out[2];
            out[3] = avg3(t[3], t[4], t[5]);
            out[6] = out[3];
            out[9] = out[3];
            out[12] = out[3];
            out[7] = avg3(t[4], t[5], t[6]);
            out[10] = out[7];
            out[13] = out[7];
            out[11] = avg3(t[5], t[6], t[7]);
            out[14] = out[11];
            out[15] = (t[6] + 3 * t[7] + 2) >> 2;
        }
        4 => {
            out[12] = avg3(left[3], left[2], left[1]);
            out[8] = avg3(left[2], left[1], left[0]);
            out[13] = out[8];
            out[4] = avg3(left[1], left[0], top_left);
            out[9] = out[4];
            out[14] = out[4];
            out[0] = avg3(left[0], top_left, top[0]);
            out[5] = out[0];
            out[10] = out[0];
            out[15] = out[0];
            out[1] = avg3(top_left, top[0], top[1]);
            out[6] = out[1];
            out[11] = out[1];
            out[2] = avg3(top[0], top[1], top[2]);
            out[7] = out[2];
            out[3] = avg3(top[1], top[2], top[3]);
        }
        5 => {
            out[0] = (top_left + top[0] + 1) >> 1;
            out[1] = (top[0] + top[1] + 1) >> 1;
            out[2] = (top[1] + top[2] + 1) >> 1;
            out[3] = (top[2] + top[3] + 1) >> 1;
            out[4] = avg3(left[0], top_left, top[0]);
            out[5] = avg3(top_left, top[0], top[1]);
            out[6] = avg3(top[0], top[1], top[2]);
            out[7] = avg3(top[1], top[2], top[3]);
            out[8] = avg3(top_left, left[0], left[1]);
            out[9] = out[0];
            out[10] = out[1];
            out[11] = out[2];
            out[12] = avg3(left[0], left[1], left[2]);
            out[13] = out[4];
            out[14] = out[5];
            out[15] = out[6];
        }
        6 => {
            out[0] = (top_left + left[0] + 1) >> 1;
            out[6] = out[0];
            out[1] = avg3(left[0], top_left, top[0]);
            out[7] = out[1];
            out[2] = avg3(top_left, top[0], top[1]);
            out[3] = avg3(top[0], top[1], top[2]);
            out[4] = (left[0] + left[1] + 1) >> 1;
            out[10] = out[4];
            out[5] = avg3(top_left, left[0], left[1]);
            out[11] = out[5];
            out[8] = (left[1] + left[2] + 1) >> 1;
            out[9] = avg3(left[0], left[1], left[2]);
            out[12] = (left[2] + left[3] + 1) >> 1;
            out[13] = avg3(left[1], left[2], left[3]);
            out[14] = out[8];
            out[15] = out[9];
        }
        7 => {
            let t = [
                top[0],
                top[1],
                top[2],
                top[3],
                top_right[0],
                top_right[1],
                top_right[2],
                top_right[3],
            ];
            out[0] = (t[0] + t[1] + 1) >> 1;
            out[1] = (t[1] + t[2] + 1) >> 1;
            out[8] = out[1];
            out[2] = (t[2] + t[3] + 1) >> 1;
            out[9] = out[2];
            out[3] = (t[3] + t[4] + 1) >> 1;
            out[10] = out[3];
            out[11] = (t[4] + t[5] + 1) >> 1;
            out[4] = avg3(t[0], t[1], t[2]);
            out[5] = avg3(t[1], t[2], t[3]);
            out[12] = out[5];
            out[6] = avg3(t[2], t[3], t[4]);
            out[13] = out[6];
            out[7] = avg3(t[3], t[4], t[5]);
            out[14] = out[7];
            out[15] = avg3(t[4], t[5], t[6]);
        }
        8 => {
            out[0] = (left[0] + left[1] + 1) >> 1;
            out[1] = avg3(left[0], left[1], left[2]);
            out[2] = (left[1] + left[2] + 1) >> 1;
            out[4] = out[2];
            out[3] = avg3(left[1], left[2], left[3]);
            out[5] = out[3];
            out[6] = (left[2] + left[3] + 1) >> 1;
            out[8] = out[6];
            out[7] = avg3(left[2], left[3], left[3]);
            out[9] = out[7];
            out[10] = left[3];
            out[11] = left[3];
            out[12] = left[3];
            out[13] = left[3];
            out[14] = left[3];
            out[15] = left[3];
        }
        _ => return Err(PredictionError::InvalidMode(mode)),
    }
    Ok(out)
}

/// Predict one filtered luma 8x8 block using modes 0..8.
pub fn predict_8x8(
    mode: u8,
    bit_depth: u8,
    neighbors: &Intra8x8Neighbors,
) -> Result<[u16; 64], PredictionError> {
    validate_depth(bit_depth)?;
    let top = neighbors
        .top
        .map(|top| filtered_top(top, neighbors.top_right, neighbors.top_left));
    let left = neighbors
        .left
        .map(|left| filtered_left(left, neighbors.top_left));
    let top_right = neighbors
        .top
        .map(|top| filtered_top_right(top, neighbors.top_right));
    let mut out = [0u16; 64];
    match mode {
        0 => {
            let t = top.ok_or(PredictionError::MissingTop)?;
            for row in 0..8 {
                out[row * 8..row * 8 + 8].copy_from_slice(&t[..8]);
            }
        }
        1 => {
            let l = left.ok_or(PredictionError::MissingLeft)?;
            for (row, &sample) in l.iter().enumerate() {
                out[row * 8..row * 8 + 8].fill(sample);
            }
        }
        2 => {
            let dc = match (top, left) {
                (Some(top), Some(left)) => {
                    (top[..8]
                        .iter()
                        .chain(left.iter())
                        .map(|&v| u32::from(v))
                        .sum::<u32>()
                        + 8)
                        >> 4
                }
                (Some(top), None) => (top[..8].iter().map(|&v| u32::from(v)).sum::<u32>() + 4) >> 3,
                (None, Some(left)) => (left.iter().map(|&v| u32::from(v)).sum::<u32>() + 4) >> 3,
                (None, None) => 1 << (bit_depth - 1),
            };
            out.fill(dc as u16);
        }
        3 => {
            let top = top.ok_or(PredictionError::MissingTop)?;
            let top_right = top_right.ok_or(PredictionError::MissingTop)?;
            let t: [u16; 16] = std::array::from_fn(|index| {
                if index < 8 {
                    top[index]
                } else {
                    top_right[index - 8]
                }
            });
            for y in 0..8 {
                for x in 0..8 {
                    let d = x + y;
                    out[y * 8 + x] = if d < 14 {
                        avg3(t[d], t[d + 1], t[d + 2])
                    } else {
                        (t[14] + 3 * t[15] + 2) >> 2
                    };
                }
            }
        }
        4 => {
            let top = top.ok_or(PredictionError::MissingTop)?;
            let left = left.ok_or(PredictionError::MissingLeft)?;
            let lt = filtered_lt(
                neighbors.left.unwrap(),
                neighbors.top.unwrap(),
                neighbors.top_left,
            );
            for y in 0..8 {
                for x in 0..8 {
                    let c = x as i32 - y as i32;
                    out[y * 8 + x] = if c < 0 {
                        let j = (-c) as usize;
                        avg3(left[j], left[j - 1], if j >= 2 { left[j - 2] } else { lt })
                    } else if c == 0 {
                        avg3(left[0], lt, top[0])
                    } else {
                        let c = c as usize;
                        avg3(if c == 1 { lt } else { top[c - 2] }, top[c - 1], top[c])
                    };
                }
            }
        }
        _ => {
            return predict_8x8_directional(mode, neighbors, top, left, top_right, bit_depth);
        }
    }
    Ok(out)
}

fn predict_8x8_directional(
    mode: u8,
    neighbors: &Intra8x8Neighbors,
    top: Option<[u16; 8]>,
    left: Option<[u16; 8]>,
    top_right: Option<[u16; 8]>,
    _bit_depth: u8,
) -> Result<[u16; 64], PredictionError> {
    let t = top.ok_or(PredictionError::MissingTop)?;
    let lt = neighbors
        .left
        .map(|left| filtered_lt(left, neighbors.top.unwrap(), neighbors.top_left));
    let mut out = [0u16; 64];
    match mode {
        5 => {
            let l = left.ok_or(PredictionError::MissingLeft)?;
            let lt = lt.ok_or(PredictionError::MissingTopLeft)?;
            out[6] = avg3(l[5], l[4], l[3]);
            out[7] = avg3(l[6], l[5], l[4]);
            set!(out, 4, avg3(l[3], l[2], l[1]));
            set!(out, 14, out[4]);
            set!(out, 5, avg3(l[4], l[3], l[2]));
            set!(out, 15, out[5]);
            set!(out, 2, avg3(l[1], l[0], lt));
            set!(out, 12, out[2]);
            set!(out, 22, out[2]);
            set!(out, 3, avg3(l[2], l[1], l[0]));
            set!(out, 13, out[3]);
            set!(out, 23, out[3]);
            set!(out, 1, avg3(l[0], lt, t[0]));
            set!(out, 11, out[1]);
            set!(out, 21, out[1]);
            set!(out, 31, out[1]);
            set!(out, 0, (lt + t[0] + 1) >> 1);
            set!(out, 10, out[0]);
            set!(out, 20, out[0]);
            set!(out, 30, out[0]);
            set!(out, 9, avg3(lt, t[0], t[1]));
            set!(out, 19, out[9]);
            set!(out, 29, out[9]);
            set!(out, 39, out[9]);
            set!(out, 8, (t[0] + t[1] + 1) >> 1);
            set!(out, 18, out[8]);
            set!(out, 28, out[8]);
            set!(out, 38, out[8]);
            set!(out, 17, avg3(t[0], t[1], t[2]));
            set!(out, 27, out[17]);
            set!(out, 37, out[17]);
            set!(out, 47, out[17]);
            set!(out, 16, (t[1] + t[2] + 1) >> 1);
            set!(out, 26, out[16]);
            set!(out, 36, out[16]);
            set!(out, 46, out[16]);
            set!(out, 25, avg3(t[1], t[2], t[3]));
            set!(out, 35, out[25]);
            set!(out, 45, out[25]);
            set!(out, 55, out[25]);
            set!(out, 24, (t[2] + t[3] + 1) >> 1);
            set!(out, 34, out[24]);
            set!(out, 44, out[24]);
            set!(out, 54, out[24]);
            set!(out, 33, avg3(t[2], t[3], t[4]));
            set!(out, 43, out[33]);
            set!(out, 53, out[33]);
            set!(out, 63, out[33]);
            set!(out, 32, (t[3] + t[4] + 1) >> 1);
            set!(out, 42, out[32]);
            set!(out, 52, out[32]);
            set!(out, 62, out[32]);
            set!(out, 41, avg3(t[3], t[4], t[5]));
            set!(out, 51, out[41]);
            set!(out, 61, out[41]);
            set!(out, 40, (t[4] + t[5] + 1) >> 1);
            set!(out, 50, out[40]);
            set!(out, 60, out[40]);
            set!(out, 49, avg3(t[4], t[5], t[6]));
            set!(out, 59, out[49]);
            set!(out, 48, (t[5] + t[6] + 1) >> 1);
            set!(out, 58, out[48]);
            out[57] = avg3(t[5], t[6], t[7]);
            out[56] = (t[6] + t[7] + 1) >> 1;
        }
        6 => {
            let l = left.ok_or(PredictionError::MissingLeft)?;
            let lt = lt.ok_or(PredictionError::MissingTopLeft)?;
            out[7] = (l[6] + l[7] + 1) >> 1;
            out[15] = avg3(l[5], l[6], l[7]);
            out[6] = (l[5] + l[6] + 1) >> 1;
            out[23] = out[6];
            out[14] = avg3(l[4], l[5], l[6]);
            out[31] = out[14];
            out[5] = (l[4] + l[5] + 1) >> 1;
            out[22] = out[5];
            out[39] = out[5];
            out[13] = avg3(l[3], l[4], l[5]);
            out[30] = out[13];
            out[47] = out[13];
            out[4] = (l[3] + l[4] + 1) >> 1;
            out[21] = out[4];
            out[38] = out[4];
            out[55] = out[4];
            out[12] = avg3(l[2], l[3], l[4]);
            out[29] = out[12];
            out[46] = out[12];
            out[63] = out[12];
            out[3] = (l[2] + l[3] + 1) >> 1;
            out[20] = out[3];
            out[37] = out[3];
            out[54] = out[3];
            out[11] = avg3(l[1], l[2], l[3]);
            out[28] = out[11];
            out[45] = out[11];
            out[62] = out[11];
            out[2] = (l[1] + l[2] + 1) >> 1;
            out[19] = out[2];
            out[36] = out[2];
            out[53] = out[2];
            out[10] = avg3(l[0], l[1], l[2]);
            out[27] = out[10];
            out[44] = out[10];
            out[61] = out[10];
            out[1] = (l[0] + l[1] + 1) >> 1;
            out[18] = out[1];
            out[35] = out[1];
            out[52] = out[1];
            out[9] = avg3(lt, l[0], l[1]);
            out[26] = out[9];
            out[43] = out[9];
            out[60] = out[9];
            out[0] = (lt + l[0] + 1) >> 1;
            out[17] = out[0];
            out[34] = out[0];
            out[51] = out[0];
            out[8] = avg3(l[0], lt, t[0]);
            out[25] = out[8];
            out[42] = out[8];
            out[59] = out[8];
            out[16] = avg3(t[1], t[0], lt);
            out[33] = out[16];
            out[50] = out[16];
            out[24] = avg3(t[2], t[1], t[0]);
            out[41] = out[24];
            out[58] = out[24];
            out[32] = avg3(t[3], t[2], t[1]);
            out[49] = out[32];
            out[40] = avg3(t[4], t[3], t[2]);
            out[57] = out[40];
            out[48] = avg3(t[5], t[4], t[3]);
            out[56] = avg3(t[6], t[5], t[4]);
        }
        7 => {
            let tr = top_right.ok_or(PredictionError::MissingTop)?;
            let t: [u16; 16] =
                std::array::from_fn(|index| if index < 8 { t[index] } else { tr[index - 8] });
            out[0] = (t[0] + t[1] + 1) >> 1;
            out[8] = avg3(t[0], t[1], t[2]);
            let value = (t[1] + t[2] + 1) >> 1;
            out[16] = value;
            out[1] = value;
            let value = avg3(t[1], t[2], t[3]);
            out[24] = value;
            out[9] = value;
            let value = (t[2] + t[3] + 1) >> 1;
            out[32] = value;
            out[17] = value;
            out[2] = value;
            let value = avg3(t[2], t[3], t[4]);
            out[40] = value;
            out[25] = value;
            out[10] = value;
            let value = (t[3] + t[4] + 1) >> 1;
            out[48] = value;
            out[33] = value;
            out[18] = value;
            out[3] = value;
            let value = avg3(t[3], t[4], t[5]);
            out[56] = value;
            out[41] = value;
            out[26] = value;
            out[11] = value;
            let value = (t[4] + t[5] + 1) >> 1;
            out[49] = value;
            out[34] = value;
            out[19] = value;
            out[4] = value;
            let value = avg3(t[4], t[5], t[6]);
            out[57] = value;
            out[42] = value;
            out[27] = value;
            out[12] = value;
            let value = (t[5] + t[6] + 1) >> 1;
            out[50] = value;
            out[35] = value;
            out[20] = value;
            out[5] = value;
            let value = avg3(t[5], t[6], t[7]);
            out[58] = value;
            out[43] = value;
            out[28] = value;
            out[13] = value;
            let value = (t[6] + t[7] + 1) >> 1;
            out[51] = value;
            out[36] = value;
            out[21] = value;
            out[6] = value;
            let value = avg3(t[6], t[7], t[8]);
            out[59] = value;
            out[44] = value;
            out[29] = value;
            out[14] = value;
            let value = (t[7] + t[8] + 1) >> 1;
            out[52] = value;
            out[37] = value;
            out[22] = value;
            out[7] = value;
            let value = avg3(t[7], t[8], t[9]);
            out[60] = value;
            out[45] = value;
            out[30] = value;
            out[15] = value;
            let value = (t[8] + t[9] + 1) >> 1;
            out[53] = value;
            out[38] = value;
            out[23] = value;
            let value = avg3(t[8], t[9], t[10]);
            out[61] = value;
            out[46] = value;
            out[31] = value;
            let value = (t[9] + t[10] + 1) >> 1;
            out[54] = value;
            out[39] = value;
            let value = avg3(t[9], t[10], t[11]);
            out[62] = value;
            out[47] = value;
            out[55] = (t[10] + t[11] + 1) >> 1;
            out[63] = avg3(t[10], t[11], t[12]);
        }
        8 => {
            let l = left.ok_or(PredictionError::MissingLeft)?;
            out[0] = (l[0] + l[1] + 1) >> 1;
            out[8] = avg3(l[0], l[1], l[2]);
            set!(out, 1, (l[1] + l[2] + 1) >> 1);
            set!(out, 16, out[1]);
            set!(out, 9, avg3(l[1], l[2], l[3]));
            set!(out, 24, out[9]);
            set!(out, 2, (l[2] + l[3] + 1) >> 1);
            set!(out, 17, out[2]);
            set!(out, 32, out[2]);
            set!(out, 10, avg3(l[2], l[3], l[4]));
            set!(out, 25, out[10]);
            set!(out, 40, out[10]);
            set!(out, 3, (l[3] + l[4] + 1) >> 1);
            set!(out, 18, out[3]);
            set!(out, 33, out[3]);
            set!(out, 48, out[3]);
            set!(out, 11, avg3(l[3], l[4], l[5]));
            set!(out, 26, out[11]);
            set!(out, 41, out[11]);
            set!(out, 56, out[11]);
            set!(out, 4, (l[4] + l[5] + 1) >> 1);
            set!(out, 19, out[4]);
            set!(out, 34, out[4]);
            set!(out, 49, out[4]);
            set!(out, 12, avg3(l[4], l[5], l[6]));
            set!(out, 27, out[12]);
            set!(out, 42, out[12]);
            set!(out, 57, out[12]);
            set!(out, 5, (l[5] + l[6] + 1) >> 1);
            set!(out, 20, out[5]);
            set!(out, 35, out[5]);
            set!(out, 50, out[5]);
            set!(out, 13, avg3(l[5], l[6], l[7]));
            set!(out, 28, out[13]);
            set!(out, 43, out[13]);
            set!(out, 58, out[13]);
            set!(out, 6, (l[6] + l[7] + 1) >> 1);
            set!(out, 21, out[6]);
            set!(out, 36, out[6]);
            set!(out, 51, out[6]);
            set!(out, 14, avg3(l[6], l[7], l[7]));
            set!(out, 29, out[14]);
            set!(out, 44, out[14]);
            set!(out, 59, out[14]);
            for index in [
                7, 15, 22, 23, 30, 31, 37, 38, 39, 45, 46, 47, 52, 53, 54, 55, 60, 61, 62, 63,
            ] {
                out[index] = l[7];
            }
        }
        _ => return Err(PredictionError::InvalidMode(mode)),
    }
    Ok(out)
}

/// Predict a 16x16 luma block. Mode 0 is vertical, 1 horizontal, 2 DC and 3 plane.
pub fn predict_16x16(
    mode: u8,
    bit_depth: u8,
    top: Option<&[u16; 16]>,
    left: Option<&[u16; 16]>,
    top_left: Option<u16>,
) -> Result<[u16; 256], PredictionError> {
    validate_depth(bit_depth)?;
    let mut out = [0u16; 256];
    match mode {
        0 => {
            let top = top.ok_or(PredictionError::MissingTop)?;
            for row in 0..16 {
                out[row * 16..row * 16 + 16].copy_from_slice(top);
            }
        }
        1 => {
            let left = left.ok_or(PredictionError::MissingLeft)?;
            for (row, &sample) in left.iter().enumerate() {
                out[row * 16..row * 16 + 16].fill(sample);
            }
        }
        2 => {
            let dc = match (top, left) {
                (Some(top), Some(left)) => {
                    (top.iter().chain(left).map(|&v| i32::from(v)).sum::<i32>() + 16) >> 5
                }
                (Some(top), None) => (top.iter().map(|&v| i32::from(v)).sum::<i32>() + 8) >> 4,
                (None, Some(left)) => (left.iter().map(|&v| i32::from(v)).sum::<i32>() + 8) >> 4,
                (None, None) => 1 << (bit_depth - 1),
            };
            out.fill(dc as u16);
        }
        3 => {
            let top = top.ok_or(PredictionError::MissingTop)?;
            let left = left.ok_or(PredictionError::MissingLeft)?;
            let top_left = top_left.ok_or(PredictionError::MissingTopLeft)?;
            let h = plane_h16(top, top_left);
            let v = plane_v16(left, top_left);
            let b = (5 * h + 32) >> 6;
            let c = (5 * v + 32) >> 6;
            let a = 16 * (i32::from(top[15]) + i32::from(left[15]) + 1) - 7 * (c + b);
            for row in 0..16 {
                for column in 0..16 {
                    let value = (a + column as i32 * b + row as i32 * c) >> 5;
                    out[row * 16 + column] = clip(value, bit_depth);
                }
            }
        }
        _ => return Err(PredictionError::InvalidMode(mode)),
    }
    Ok(out)
}

/// Predict an 8x8 or 8x16 chroma block. Modes match luma 16x16.
pub fn predict_chroma(
    mode: u8,
    bit_depth: u8,
    top: Option<&[u16]>,
    left: Option<&[u16]>,
    height: usize,
    top_left: Option<u16>,
) -> Result<Vec<u16>, PredictionError> {
    validate_depth(bit_depth)?;
    if !matches!(height, 8 | 16) {
        return Err(PredictionError::Unsupported("chroma prediction height"));
    }
    let mut out = vec![0u16; 8 * height];
    match mode {
        0 => {
            let top = top.ok_or(PredictionError::MissingTop)?;
            if top.len() < 8 {
                return Err(PredictionError::MissingTop);
            }
            for row in 0..height {
                out[row * 8..row * 8 + 8].copy_from_slice(&top[..8]);
            }
        }
        1 => {
            let Some(left) = left else {
                return predict_chroma(2, bit_depth, top, None, height, top_left);
            };
            if left.len() < height {
                return Err(PredictionError::MissingLeft);
            }
            for (row, &sample) in left[..height].iter().enumerate() {
                out[row * 8..row * 8 + 8].fill(sample);
            }
        }
        2 => {
            for section in 0..height / 8 {
                let left_section = left.map(|left| &left[section * 8..section * 8 + 8]);
                let dc = chroma_dc_8x8(top, left_section, bit_depth)?;
                for y in 0..8 {
                    for x in 0..8 {
                        out[(section * 8 + y) * 8 + x] = dc[(y / 4) * 2 + x / 4];
                    }
                }
            }
        }
        3 => {
            let top = top.ok_or(PredictionError::MissingTop)?;
            let left = left.ok_or(PredictionError::MissingLeft)?;
            let top_left = top_left.ok_or(PredictionError::MissingTopLeft)?;
            if top.len() < 8 || left.len() < height {
                return Err(PredictionError::MissingTop);
            }
            let h = plane_h8(top, top_left);
            let v = if height == 8 {
                plane_v8(&left[..8], top_left)
            } else {
                plane_v16(&left[..16], top_left)
            };
            let b = (17 * h + 16) >> 5;
            let c = if height == 8 {
                (17 * v + 16) >> 5
            } else {
                (5 * v + 32) >> 6
            };
            let v_coeff = if height == 8 { 3 } else { 7 };
            let a =
                16 * (i32::from(left[height - 1]) + i32::from(top[7]) + 1) - v_coeff * c - 3 * b;
            for row in 0..height {
                for column in 0..8 {
                    let value = (a + column as i32 * b + row as i32 * c) >> 5;
                    out[row * 8 + column] = clip(value, bit_depth);
                }
            }
        }
        _ => return Err(PredictionError::InvalidMode(mode)),
    }
    Ok(out)
}

fn chroma_dc_8x8(
    top: Option<&[u16]>,
    left: Option<&[u16]>,
    bit_depth: u8,
) -> Result<[u16; 4], PredictionError> {
    let sum = |samples: &[u16]| samples.iter().map(|&v| u32::from(v)).sum::<u32>();
    let values = match (top, left) {
        (Some(top), Some(left)) => {
            if top.len() < 8 || left.len() < 8 {
                return Err(PredictionError::MissingTop);
            }
            [
                ((sum(&top[..4]) + sum(&left[..4]) + 4) >> 3) as u16,
                ((sum(&top[4..8]) + 2) >> 2) as u16,
                ((sum(&left[4..8]) + 2) >> 2) as u16,
                ((sum(&top[4..8]) + sum(&left[4..8]) + 4) >> 3) as u16,
            ]
        }
        (Some(top), None) => {
            if top.len() < 8 {
                return Err(PredictionError::MissingTop);
            }
            let a = ((sum(&top[..4]) + 2) >> 2) as u16;
            let b = ((sum(&top[4..8]) + 2) >> 2) as u16;
            [a, b, a, b]
        }
        (None, Some(left)) => {
            if left.len() < 8 {
                return Err(PredictionError::MissingLeft);
            }
            let a = ((sum(&left[..4]) + 2) >> 2) as u16;
            let b = ((sum(&left[4..8]) + 2) >> 2) as u16;
            [a, a, b, b]
        }
        (None, None) => {
            let value = 1u16 << (bit_depth - 1);
            [value; 4]
        }
    };
    Ok(values)
}

fn plane_h16(top: &[u16; 16], top_left: u16) -> i32 {
    let mut h = i32::from(top[8]) - i32::from(top[6]);
    for k in 2..=8 {
        h += k as i32 * (sample_at(top, 7 + k, top_left) - sample_at(top, 7 - k, top_left));
    }
    h
}

fn plane_v16(left: &[u16], top_left: u16) -> i32 {
    let mut v = i32::from(left[8]) - i32::from(left[6]);
    for k in 2..=8 {
        v += k as i32 * (sample_at(left, 7 + k, top_left) - sample_at(left, 7 - k, top_left));
    }
    v
}

fn plane_h8(top: &[u16], top_left: u16) -> i32 {
    let mut h = i32::from(top[4]) - i32::from(top[2]);
    for k in 2..=4 {
        h += k as i32 * (sample_at(top, 3 + k, top_left) - sample_at(top, 3 - k, top_left));
    }
    h
}

fn plane_v8(left: &[u16], top_left: u16) -> i32 {
    let mut v = i32::from(left[4]) - i32::from(left[2]);
    for k in 2..=4 {
        v += k as i32 * (sample_at(left, 3 + k, top_left) - sample_at(left, 3 - k, top_left));
    }
    v
}

fn sample_at(samples: &[u16], index: i32, top_left: u16) -> i32 {
    if index < 0 {
        i32::from(top_left)
    } else {
        i32::from(samples[index as usize])
    }
}

fn avg3(a: u16, b: u16, c: u16) -> u16 {
    ((u32::from(a) + 2 * u32::from(b) + u32::from(c) + 2) >> 2) as u16
}

fn filtered_top(top: [u16; 8], top_right: Option<[u16; 8]>, top_left: Option<u16>) -> [u16; 8] {
    let left = top_left.unwrap_or(top[0]);
    let right = top_right.unwrap_or([top[7]; 8]);
    [
        avg3(left, top[0], top[1]),
        avg3(top[0], top[1], top[2]),
        avg3(top[1], top[2], top[3]),
        avg3(top[2], top[3], top[4]),
        avg3(top[3], top[4], top[5]),
        avg3(top[4], top[5], top[6]),
        avg3(top[5], top[6], top[7]),
        avg3(top[6], top[7], right[0]),
    ]
}

fn filtered_left(left: [u16; 8], top_left: Option<u16>) -> [u16; 8] {
    let above = top_left.unwrap_or(left[0]);
    [
        avg3(above, left[0], left[1]),
        avg3(left[0], left[1], left[2]),
        avg3(left[1], left[2], left[3]),
        avg3(left[2], left[3], left[4]),
        avg3(left[3], left[4], left[5]),
        avg3(left[4], left[5], left[6]),
        avg3(left[5], left[6], left[7]),
        (left[6] + 3 * left[7] + 2) >> 2,
    ]
}

fn filtered_top_right(top: [u16; 8], top_right: Option<[u16; 8]>) -> [u16; 8] {
    let Some(right) = top_right else {
        return [top[7]; 8];
    };
    [
        avg3(top[7], right[0], right[1]),
        avg3(right[0], right[1], right[2]),
        avg3(right[1], right[2], right[3]),
        avg3(right[2], right[3], right[4]),
        avg3(right[3], right[4], right[5]),
        avg3(right[4], right[5], right[6]),
        avg3(right[5], right[6], right[7]),
        (right[6] + 3 * right[7] + 2) >> 2,
    ]
}

fn filtered_lt(left: [u16; 8], top: [u16; 8], top_left: Option<u16>) -> u16 {
    avg3(left[0], top_left.unwrap_or(left[0]), top[0])
}

fn validate_depth(bit_depth: u8) -> Result<(), PredictionError> {
    if (8..=10).contains(&bit_depth) {
        Ok(())
    } else {
        Err(PredictionError::InvalidBitDepth(bit_depth))
    }
}

fn clip(value: i32, bit_depth: u8) -> u16 {
    value.clamp(0, (1 << bit_depth) - 1) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neighbors() -> Intra4x4Neighbors {
        Intra4x4Neighbors {
            top: Some([10, 20, 30, 40]),
            left: Some([50, 60, 70, 80]),
            top_right: Some([90, 100, 110, 120]),
            top_left: Some(5),
        }
    }

    #[test]
    fn four_by_four_cardinal_and_dc_modes_match_the_spec() {
        assert_eq!(
            predict_4x4(0, 8, &neighbors()).unwrap(),
            [10, 20, 30, 40, 10, 20, 30, 40, 10, 20, 30, 40, 10, 20, 30, 40]
        );
        assert_eq!(
            predict_4x4(1, 8, &neighbors()).unwrap(),
            [50, 50, 50, 50, 60, 60, 60, 60, 70, 70, 70, 70, 80, 80, 80, 80]
        );
        assert_eq!(predict_4x4(2, 8, &neighbors()).unwrap(), [45; 16]);
    }

    #[test]
    fn four_by_four_dc_uses_only_the_available_edge() {
        let top_only = Intra4x4Neighbors {
            top: Some([10, 20, 30, 40]),
            left: None,
            top_right: None,
            top_left: Some(5),
        };
        assert_eq!(predict_4x4(2, 8, &top_only).unwrap(), [25; 16]);

        let left_only = Intra4x4Neighbors {
            top: None,
            left: Some([50, 60, 70, 80]),
            top_right: None,
            top_left: Some(5),
        };
        assert_eq!(predict_4x4(2, 8, &left_only).unwrap(), [65; 16]);
    }

    #[test]
    fn four_by_four_diagonal_modes_are_deterministic() {
        let diagonal_left = predict_4x4(3, 8, &neighbors()).unwrap();
        assert_eq!(diagonal_left[..4], [20, 30, 50, 80]);
        let diagonal_right = predict_4x4(4, 8, &neighbors()).unwrap();
        assert_eq!(diagonal_right[..4], [18, 11, 20, 30]);
        assert_ne!(diagonal_left, diagonal_right);
        assert_eq!(
            predict_4x4(5, 8, &neighbors()).unwrap(),
            [8, 15, 25, 35, 18, 11, 20, 30, 41, 8, 15, 25, 60, 18, 11, 20,]
        );
        assert_eq!(
            predict_4x4(6, 8, &neighbors()).unwrap(),
            [28, 18, 11, 20, 55, 41, 28, 18, 65, 60, 55, 41, 75, 70, 65, 60,]
        );
        assert_eq!(
            predict_4x4(7, 8, &neighbors()).unwrap(),
            [15, 25, 35, 65, 20, 30, 50, 80, 25, 35, 65, 95, 30, 50, 80, 100,]
        );
        assert_eq!(
            predict_4x4(8, 8, &neighbors()).unwrap(),
            [55, 60, 65, 70, 65, 70, 75, 78, 75, 78, 80, 80, 80, 80, 80, 80,]
        );
    }

    #[test]
    fn sixteen_by_sixteen_modes_handle_edges_and_plane() {
        let top = [48u16; 16];
        let left = [48u16; 16];
        assert_eq!(
            predict_16x16(0, 8, Some(&top), Some(&left), Some(48)).unwrap(),
            [48; 256]
        );
        assert_eq!(
            predict_16x16(1, 8, Some(&top), Some(&left), Some(48)).unwrap(),
            [48; 256]
        );
        assert_eq!(
            predict_16x16(2, 8, Some(&top), Some(&left), Some(48)).unwrap(),
            [48; 256]
        );
        assert_eq!(
            predict_16x16(3, 8, Some(&top), Some(&left), Some(48)).unwrap(),
            [48; 256]
        );
        assert_eq!(predict_16x16(2, 8, None, None, None).unwrap(), [128; 256]);
    }

    #[test]
    fn filtered_eight_by_eight_modes_match_reference_edges() {
        let neighbors = Intra8x8Neighbors {
            top: Some([10, 20, 30, 40, 50, 60, 70, 80]),
            left: Some([90, 100, 110, 120, 130, 140, 150, 160]),
            top_right: Some([90, 100, 110, 120, 130, 140, 150, 160]),
            top_left: Some(5),
        };
        assert_eq!(
            predict_8x8(0, 8, &neighbors).unwrap()[..8],
            [11, 20, 30, 40, 50, 60, 70, 80]
        );
        assert_eq!(predict_8x8(1, 8, &neighbors).unwrap()[0], 71);
        assert_eq!(predict_8x8(2, 8, &neighbors).unwrap(), [84; 64]);
        assert_eq!(
            predict_8x8(3, 8, &neighbors).unwrap()[..8],
            [20, 30, 40, 50, 60, 70, 80, 90]
        );
        assert_eq!(
            predict_8x8(4, 8, &neighbors).unwrap()[..8],
            [35, 18, 20, 30, 40, 50, 60, 70]
        );
        assert_eq!(
            predict_8x8(5, 8, &neighbors).unwrap()[..8],
            [20, 35, 68, 95, 110, 120, 130, 140]
        );
        assert_eq!(
            predict_8x8(6, 8, &neighbors).unwrap()[..8],
            [50, 86, 105, 115, 125, 135, 145, 154]
        );
        assert_eq!(
            predict_8x8(7, 8, &neighbors).unwrap()[..8],
            [16, 25, 35, 45, 55, 65, 75, 85]
        );
        assert_eq!(
            predict_8x8(8, 8, &neighbors).unwrap()[..8],
            [86, 105, 115, 125, 135, 145, 154, 158]
        );
    }

    #[test]
    fn chroma_dc_uses_four_quadrants_and_ten_bit_midpoint() {
        let top = [10, 10, 10, 10, 30, 30, 30, 30];
        let left = [20, 20, 20, 20, 40, 40, 40, 40];
        let out = predict_chroma(2, 8, Some(&top), Some(&left), 8, Some(0)).unwrap();
        assert_eq!(out[0], 15);
        assert_eq!(out[4], 30);
        assert_eq!(out[32], 40);
        assert_eq!(out[36], 35);
        let top16 = [64u16; 8];
        let left16 = [64u16; 16];
        assert_eq!(
            predict_chroma(3, 10, Some(&top16), Some(&left16), 16, Some(64)).unwrap(),
            [64; 128]
        );
    }

    #[test]
    fn chroma_vertical_without_left_edge_uses_the_top_edge() {
        let top = [10u16, 20, 30, 40, 50, 60, 70, 80];
        let prediction = predict_chroma(0, 8, Some(&top), None, 8, Some(5)).unwrap();
        assert_eq!(&prediction[..8], top);
        assert_eq!(&prediction[56..], top);
    }

    #[test]
    fn invalid_modes_depth_and_edges_are_named() {
        assert_eq!(
            predict_4x4(9, 8, &neighbors()),
            Err(PredictionError::InvalidMode(9))
        );
        assert_eq!(
            predict_16x16(0, 8, None, None, None),
            Err(PredictionError::MissingTop)
        );
        assert_eq!(
            predict_chroma(0, 11, Some(&[0; 8]), Some(&[0; 8]), 8, Some(0)),
            Err(PredictionError::InvalidBitDepth(11))
        );
    }
}
