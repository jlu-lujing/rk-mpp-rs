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

/// Predict one 4x4 block using modes 0..8.
pub fn predict_4x4(
    mode: u8,
    bit_depth: u8,
    neighbors: &Intra4x4Neighbors,
) -> Result<[u16; 16], PredictionError> {
    validate_depth(bit_depth)?;
    let top = *neighbors.top.as_ref().ok_or(PredictionError::MissingTop)?;
    let left = *neighbors
        .left
        .as_ref()
        .ok_or(PredictionError::MissingLeft)?;
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
        2 => {
            out.fill(dc_value(&top, &left, bit_depth));
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
            out[4] = out[0];
            out[1] = (top[0] + top[1] + 1) >> 1;
            out[5] = out[1];
            out[2] = (top[1] + top[2] + 1) >> 1;
            out[6] = out[2];
            out[3] = (top[2] + top[3] + 1) >> 1;
            out[8] = avg3(left[0], top_left, top[0]);
            out[12] = out[8];
            out[9] = avg3(top_left, top[0], top[1]);
            out[13] = out[9];
            out[10] = avg3(top[0], top[1], top[2]);
            out[14] = out[10];
            out[7] = avg3(top[1], top[2], top[3]);
            out[11] = avg3(top_left, left[0], left[1]);
            out[15] = avg3(left[0], left[1], left[2]);
        }
        6 => {
            out[0] = (top_left + left[0] + 1) >> 1;
            out[8] = out[0];
            out[1] = avg3(left[0], top_left, top[0]);
            out[9] = out[1];
            out[2] = avg3(top_left, top[0], top[1]);
            out[3] = avg3(top[0], top[1], top[2]);
            out[4] = (left[0] + left[1] + 1) >> 1;
            out[12] = out[4];
            out[5] = avg3(top_left, left[0], left[1]);
            out[13] = out[5];
            out[6] = (left[1] + left[2] + 1) >> 1;
            out[10] = out[6];
            out[7] = avg3(left[0], left[1], left[2]);
            out[14] = out[7];
            out[11] = (left[2] + left[3] + 1) >> 1;
            out[15] = avg3(left[1], left[2], left[3]);
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
            out[4] = (t[1] + t[2] + 1) >> 1;
            out[2] = out[4];
            out[8] = (t[2] + t[3] + 1) >> 1;
            out[6] = out[8];
            out[12] = (t[3] + t[4] + 1) >> 1;
            out[10] = out[12];
            out[3] = out[12];
            out[14] = (t[4] + t[5] + 1) >> 1;
            out[1] = avg3(t[0], t[1], t[2]);
            out[5] = avg3(t[1], t[2], t[3]);
            out[9] = avg3(t[2], t[3], t[4]);
            out[7] = out[9];
            out[13] = avg3(t[3], t[4], t[5]);
            out[11] = out[13];
            out[15] = avg3(t[4], t[5], t[6]);
        }
        8 => {
            out[0] = (left[0] + left[1] + 1) >> 1;
            out[4] = avg3(left[0], left[1], left[2]);
            out[8] = (left[1] + left[2] + 1) >> 1;
            out[1] = out[8];
            out[12] = avg3(left[1], left[2], left[3]);
            out[5] = out[12];
            out[9] = (left[2] + left[3] + 1) >> 1;
            out[2] = out[9];
            out[13] = avg3(left[2], left[3], left[3]);
            out[6] = out[13];
            out[3] = left[3];
            out[7] = left[3];
            out[10] = left[3];
            out[11] = left[3];
            out[14] = left[3];
            out[15] = left[3];
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
            let a = 16 * (i32::from(top[15]) + i32::from(left[15]) + 1) - 7 * (v + h);
            for row in 0..16 {
                for column in 0..16 {
                    let value = (a + (column as i32 - 7) * b + (row as i32 - 7) * c + 16) >> 5;
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
            let left = left.ok_or(PredictionError::MissingLeft)?;
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
            let (anchor_v, anchor_h, v_coeff) = if height == 8 { (4, 4, 3) } else { (8, 4, 7) };
            let a =
                16 * (i32::from(left[height - 1]) + i32::from(top[7]) + 1) - v_coeff * v - 3 * h;
            let middle = i32::try_from(anchor_v + anchor_h).unwrap() * 0;
            for row in 0..height {
                for column in 0..8 {
                    let value = (a
                        + (column as i32 - anchor_h as i32 + middle) * b
                        + (row as i32 - anchor_v as i32) * c
                        + 16)
                        >> 5;
                    out[row * 8 + column] = clip(value, bit_depth);
                }
            }
        }
        _ => return Err(PredictionError::InvalidMode(mode)),
    }
    Ok(out)
}

fn dc_value(top: &[u16; 4], left: &[u16; 4], _bit_depth: u8) -> u16 {
    ((top.iter().chain(left).map(|&v| u32::from(v)).sum::<u32>() + 4) >> 3) as u16
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
    fn four_by_four_diagonal_modes_are_deterministic() {
        let diagonal_left = predict_4x4(3, 8, &neighbors()).unwrap();
        assert_eq!(diagonal_left[..4], [20, 30, 50, 80]);
        let diagonal_right = predict_4x4(4, 8, &neighbors()).unwrap();
        assert_eq!(diagonal_right[..4], [18, 11, 20, 30]);
        assert_ne!(diagonal_left, diagonal_right);
        assert_eq!(
            predict_4x4(5, 8, &neighbors()).unwrap()[..4],
            [8, 15, 25, 35]
        );
        assert_eq!(
            predict_4x4(6, 8, &neighbors()).unwrap()[..4],
            [28, 18, 11, 20]
        );
        assert_eq!(
            predict_4x4(7, 8, &neighbors()).unwrap()[..4],
            [15, 20, 25, 65]
        );
        assert_eq!(
            predict_4x4(8, 8, &neighbors()).unwrap()[..4],
            [55, 65, 75, 80]
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
            [49; 256]
        );
        assert_eq!(predict_16x16(2, 8, None, None, None).unwrap(), [128; 256]);
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
            [65; 128]
        );
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
