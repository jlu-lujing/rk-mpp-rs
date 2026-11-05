//! AV1 inter-inter wedge masks.
//!
//! The normative masks are subwindows of six 64x64 master masks. The masters
//! are generated from the three one-dimensional prototypes in libaom's
//! `reconinter.c`, then indexed by the block-size codebook and sign bit.

use std::sync::OnceLock;

use crate::reference::av1::partition::BlockSize;

const MASTER_SIZE: usize = 64;
const MASTER_SQUARE: usize = MASTER_SIZE * MASTER_SIZE;
const MAX_ALPHA: u8 = 64;

const HORIZONTAL: usize = 0;
const VERTICAL: usize = 1;
const OBLIQUE27: usize = 2;
const OBLIQUE63: usize = 3;
const OBLIQUE117: usize = 4;
const OBLIQUE153: usize = 5;

const OBLIQUE_ODD: [u8; MASTER_SIZE] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 6,
    18, 37, 53, 60, 63, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64,
];
const OBLIQUE_EVEN: [u8; MASTER_SIZE] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 4, 11,
    27, 46, 58, 62, 63, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64,
];
const VERTICAL_PROTOTYPE: [u8; MASTER_SIZE] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 7,
    21, 43, 57, 62, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64,
];

const HGTW: [(usize, u8, u8); 16] = [
    (OBLIQUE27, 4, 4),
    (OBLIQUE63, 4, 4),
    (OBLIQUE117, 4, 4),
    (OBLIQUE153, 4, 4),
    (HORIZONTAL, 4, 2),
    (HORIZONTAL, 4, 4),
    (HORIZONTAL, 4, 6),
    (VERTICAL, 4, 4),
    (OBLIQUE27, 4, 2),
    (OBLIQUE27, 4, 6),
    (OBLIQUE153, 4, 2),
    (OBLIQUE153, 4, 6),
    (OBLIQUE63, 2, 4),
    (OBLIQUE63, 6, 4),
    (OBLIQUE117, 2, 4),
    (OBLIQUE117, 6, 4),
];
const HLTW: [(usize, u8, u8); 16] = [
    (OBLIQUE27, 4, 4),
    (OBLIQUE63, 4, 4),
    (OBLIQUE117, 4, 4),
    (OBLIQUE153, 4, 4),
    (VERTICAL, 2, 4),
    (VERTICAL, 4, 4),
    (VERTICAL, 6, 4),
    (HORIZONTAL, 4, 4),
    (OBLIQUE27, 4, 2),
    (OBLIQUE27, 4, 6),
    (OBLIQUE153, 4, 2),
    (OBLIQUE153, 4, 6),
    (OBLIQUE63, 2, 4),
    (OBLIQUE63, 6, 4),
    (OBLIQUE117, 2, 4),
    (OBLIQUE117, 6, 4),
];
const HEQW: [(usize, u8, u8); 16] = [
    (OBLIQUE27, 4, 4),
    (OBLIQUE63, 4, 4),
    (OBLIQUE117, 4, 4),
    (OBLIQUE153, 4, 4),
    (HORIZONTAL, 4, 2),
    (HORIZONTAL, 4, 6),
    (VERTICAL, 2, 4),
    (VERTICAL, 6, 4),
    (OBLIQUE27, 4, 2),
    (OBLIQUE27, 4, 6),
    (OBLIQUE153, 4, 2),
    (OBLIQUE153, 4, 6),
    (OBLIQUE63, 2, 4),
    (OBLIQUE63, 6, 4),
    (OBLIQUE117, 2, 4),
    (OBLIQUE117, 6, 4),
];

const SIGNFLIP_SQUARE: [bool; 16] = [
    true, true, true, true, true, true, true, true, true, true, false, true, true, true, false,
    true,
];
const SIGNFLIP_HGTW: [bool; 16] = [
    true, true, true, true, false, true, true, true, true, true, false, true, true, true, false,
    true,
];
const SIGNFLIP_8X32: [bool; 16] = [
    true, true, true, true, false, true, true, true, false, true, false, true, true, true, false,
    true,
];
const SIGNFLIP_32X8: [bool; 16] = [
    true, true, true, true, false, true, true, true, true, true, false, true, false, true, false,
    true,
];

fn shift_copy(source: &[u8], target: &mut [u8], shift: isize) {
    if shift >= 0 {
        let shift = shift as usize;
        target[shift..].copy_from_slice(&source[..MASTER_SIZE - shift]);
        target[..shift].fill(source[0]);
    } else {
        let shift = (-shift) as usize;
        target[..MASTER_SIZE - shift].copy_from_slice(&source[shift..]);
        target[MASTER_SIZE - shift..].fill(source[MASTER_SIZE - 1]);
    }
}

fn masters() -> &'static [[Vec<u8>; 6]; 2] {
    static MASTERS: OnceLock<[[Vec<u8>; 6]; 2]> = OnceLock::new();
    MASTERS.get_or_init(|| {
        let mut masks: [[Vec<u8>; 6]; 2] =
            std::array::from_fn(|_| std::array::from_fn(|_| vec![0; MASTER_SQUARE]));
        let mut shift = (MASTER_SIZE / 4) as isize;
        for row in (0..MASTER_SIZE).step_by(2) {
            shift_copy(
                &OBLIQUE_EVEN,
                &mut masks[0][OBLIQUE63][row * MASTER_SIZE..(row + 1) * MASTER_SIZE],
                shift,
            );
            shift -= 1;
            shift_copy(
                &OBLIQUE_ODD,
                &mut masks[0][OBLIQUE63][(row + 1) * MASTER_SIZE..(row + 2) * MASTER_SIZE],
                shift,
            );
            masks[0][VERTICAL][row * MASTER_SIZE..(row + 1) * MASTER_SIZE]
                .copy_from_slice(&VERTICAL_PROTOTYPE);
            masks[0][VERTICAL][(row + 1) * MASTER_SIZE..(row + 2) * MASTER_SIZE]
                .copy_from_slice(&VERTICAL_PROTOTYPE);
        }

        for row in 0..MASTER_SIZE {
            for col in 0..MASTER_SIZE {
                let value = masks[0][OBLIQUE63][row * MASTER_SIZE + col];
                masks[0][OBLIQUE27][col * MASTER_SIZE + row] = value;
                masks[0][OBLIQUE117][row * MASTER_SIZE + MASTER_SIZE - 1 - col] = MAX_ALPHA - value;
                masks[0][OBLIQUE153][(MASTER_SIZE - 1 - col) * MASTER_SIZE + row] =
                    MAX_ALPHA - value;
                masks[1][OBLIQUE63][row * MASTER_SIZE + col] = MAX_ALPHA - value;
                masks[1][OBLIQUE27][col * MASTER_SIZE + row] = MAX_ALPHA - value;
                masks[1][OBLIQUE117][row * MASTER_SIZE + MASTER_SIZE - 1 - col] = value;
                masks[1][OBLIQUE153][(MASTER_SIZE - 1 - col) * MASTER_SIZE + row] = value;

                let vertical = masks[0][VERTICAL][row * MASTER_SIZE + col];
                masks[0][HORIZONTAL][col * MASTER_SIZE + row] = vertical;
                masks[1][VERTICAL][row * MASTER_SIZE + col] = MAX_ALPHA - vertical;
                masks[1][HORIZONTAL][col * MASTER_SIZE + row] = MAX_ALPHA - vertical;
            }
        }
        masks
    })
}

fn codebook(size: BlockSize) -> Option<(&'static [(usize, u8, u8); 16], &'static [bool; 16])> {
    match (size.width, size.height) {
        (8, 8) | (16, 16) | (32, 32) => Some((&HEQW, &SIGNFLIP_SQUARE)),
        (8, 16) | (16, 32) => Some((&HGTW, &SIGNFLIP_HGTW)),
        (8, 32) => Some((&HGTW, &SIGNFLIP_8X32)),
        (16, 8) | (32, 16) => Some((&HLTW, &SIGNFLIP_HGTW)),
        (32, 8) => Some((&HLTW, &SIGNFLIP_32X8)),
        _ => None,
    }
}

/// Return the luma wedge mask for one `BLOCK_SIZES` entry.
pub fn wedge_mask(size: BlockSize, index: u8, sign: u8) -> Result<Vec<u8>, &'static str> {
    let (codebook, signflip) =
        codebook(size).ok_or("wedge is not available for this block size")?;
    let index = usize::from(index);
    let (direction, x_offset, y_offset) = *codebook
        .get(index)
        .ok_or("wedge index is outside the block-size codebook")?;
    let negative = usize::from(sign != 0) ^ usize::from(signflip[index]);
    let width = size.width as usize;
    let height = size.height as usize;
    let x_offset = (usize::from(x_offset) * width) >> 3;
    let y_offset = (usize::from(y_offset) * height) >> 3;
    let source = &masters()[negative][direction];
    let start_row = MASTER_SIZE / 2 - y_offset;
    let start_col = MASTER_SIZE / 2 - x_offset;
    let mut mask = vec![0; width * height];
    for row in 0..height {
        let source_start = (start_row + row) * MASTER_SIZE + start_col;
        mask[row * width..(row + 1) * width]
            .copy_from_slice(&source[source_start..source_start + width]);
    }
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::partition::BLOCK_SIZES;
    use std::collections::HashSet;

    #[test]
    fn every_supported_sign_pair_complements_to_64() {
        for size in BLOCK_SIZES {
            let Some((_, _)) = codebook(size) else {
                continue;
            };
            for index in 0..16 {
                let normal = wedge_mask(size, index, 0).unwrap();
                let flipped = wedge_mask(size, index, 1).unwrap();
                assert_eq!(normal.len(), size.width as usize * size.height as usize);
                assert!(normal
                    .iter()
                    .zip(&flipped)
                    .all(|(left, right)| u16::from(*left) + u16::from(*right) == 64));
                assert!(normal.iter().all(|value| *value <= 64));
            }
        }
    }

    #[test]
    fn codebooks_cover_only_the_specification_sizes() {
        let supported = BLOCK_SIZES
            .iter()
            .copied()
            .filter(|size| codebook(*size).is_some())
            .map(|size| size.name)
            .collect::<HashSet<_>>();
        assert_eq!(
            supported,
            HashSet::from([
                "BLOCK_8X8",
                "BLOCK_8X16",
                "BLOCK_16X8",
                "BLOCK_16X16",
                "BLOCK_16X32",
                "BLOCK_32X16",
                "BLOCK_32X32",
                "BLOCK_8X32",
                "BLOCK_32X8",
            ])
        );
    }
}
