//! AV1 palette mode info and color-index tokens.

use crate::reference::av1::cdf_defaults::{
    CdfTable, DEFAULT_PALETTE_SIZE_2_UV_COLOR_CDF, DEFAULT_PALETTE_SIZE_2_Y_COLOR_CDF,
    DEFAULT_PALETTE_SIZE_3_UV_COLOR_CDF, DEFAULT_PALETTE_SIZE_3_Y_COLOR_CDF,
    DEFAULT_PALETTE_SIZE_4_UV_COLOR_CDF, DEFAULT_PALETTE_SIZE_4_Y_COLOR_CDF,
    DEFAULT_PALETTE_SIZE_5_UV_COLOR_CDF, DEFAULT_PALETTE_SIZE_5_Y_COLOR_CDF,
    DEFAULT_PALETTE_SIZE_6_UV_COLOR_CDF, DEFAULT_PALETTE_SIZE_6_Y_COLOR_CDF,
    DEFAULT_PALETTE_SIZE_7_UV_COLOR_CDF, DEFAULT_PALETTE_SIZE_7_Y_COLOR_CDF,
    DEFAULT_PALETTE_SIZE_8_UV_COLOR_CDF, DEFAULT_PALETTE_SIZE_8_Y_COLOR_CDF,
    DEFAULT_PALETTE_UV_MODE_CDF, DEFAULT_PALETTE_UV_SIZE_CDF, DEFAULT_PALETTE_Y_MODE_CDF,
    DEFAULT_PALETTE_Y_SIZE_CDF,
};
use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::intra::{IntraMode, UvMode};
use crate::reference::av1::partition::{block_size_index, BlockSize, BLOCK_SIZES};
use crate::reference::av1::subexp::read_ns;
use crate::reference::av1::symbol::{SymbolDecoder, SymbolError};

const COLOR_CONTEXT: [i8; 9] = [-1, -1, 0, -1, -1, 4, 3, 2, 1];
const COLOR_HASH_MULTIPLIERS: [u8; 3] = [1, 2, 2];

const Y_COLOR_CDFS: [&CdfTable; 7] = [
    &DEFAULT_PALETTE_SIZE_2_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_3_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_4_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_5_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_6_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_7_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_8_Y_COLOR_CDF,
];

const UV_COLOR_CDFS: [&CdfTable; 7] = [
    &DEFAULT_PALETTE_SIZE_2_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_3_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_4_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_5_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_6_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_7_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_8_UV_COLOR_CDF,
];

fn palette_allowed_size(size: BlockSize) -> bool {
    block_size_index(size) >= block_size_index(BLOCK_SIZES[3])
        && size.width <= 64
        && size.height <= 64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteError {
    Cdf(CdfStateError),
    Symbol(SymbolError),
    BadPaletteSize(usize),
    MissingTokens,
    InvalidIndex(u8),
}

impl From<CdfStateError> for PaletteError {
    fn from(error: CdfStateError) -> Self {
        Self::Cdf(error)
    }
}

impl From<SymbolError> for PaletteError {
    fn from(error: SymbolError) -> Self {
        Self::Symbol(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteModeInfo {
    pub y_size: usize,
    pub y_colors: [u16; 8],
    pub uv_size: usize,
    pub u_colors: [u16; 8],
    pub v_colors: [u16; 8],
}

impl Default for PaletteModeInfo {
    fn default() -> Self {
        Self {
            y_size: 0,
            y_colors: [0; 8],
            uv_size: 0,
            u_colors: [0; 8],
            v_colors: [0; 8],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteMap {
    pub width: usize,
    pub height: usize,
    pub indices: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteTokens {
    pub y: Option<PaletteMap>,
    pub uv: Option<PaletteMap>,
}

/// Expand one plane region of a palette block. `block_x`/`block_y` and
/// `tx_x`/`tx_y` are plane-sample coordinates; out-of-onscreen map cells use
/// the specification's last-column/last-row replication.
#[allow(clippy::too_many_arguments)]
pub fn predict_palette_plane(
    info: &PaletteModeInfo,
    tokens: &PaletteTokens,
    plane: usize,
    block_x: usize,
    block_y: usize,
    tx_x: usize,
    tx_y: usize,
    w: usize,
    h: usize,
) -> Result<Option<Vec<u16>>, PaletteError> {
    let (size, colors, map) = if plane == 0 {
        (info.y_size, &info.y_colors, tokens.y.as_ref())
    } else {
        (
            info.uv_size,
            if plane == 1 {
                &info.u_colors
            } else {
                &info.v_colors
            },
            tokens.uv.as_ref(),
        )
    };
    if size == 0 {
        return Ok(None);
    }
    let map = map.ok_or(PaletteError::MissingTokens)?;
    if map.width == 0 || map.height == 0 {
        return Err(PaletteError::MissingTokens);
    }
    let map_x = (block_x / map.width) * map.width;
    let map_y = (block_y / map.height) * map.height;
    let mut out = vec![0u16; w * h];
    for row in 0..h {
        let source_y = (tx_y + row).clamp(map_y, map_y + map.height - 1) - map_y;
        for col in 0..w {
            let source_x = (tx_x + col).clamp(map_x, map_x + map.width - 1) - map_x;
            let index = map.indices[source_y * map.width + source_x];
            if usize::from(index) >= size {
                return Err(PaletteError::InvalidIndex(index));
            }
            out[row * w + col] = colors[usize::from(index)];
        }
    }
    Ok(Some(out))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteState {
    pub mi_cols: usize,
    pub mi_rows: usize,
    y_sizes: Vec<u8>,
    uv_sizes: Vec<u8>,
    y_colors: Vec<[u16; 8]>,
    u_colors: Vec<[u16; 8]>,
    v_colors: Vec<[u16; 8]>,
}

impl PaletteState {
    pub fn new(mi_cols: usize, mi_rows: usize) -> Self {
        Self {
            mi_cols,
            mi_rows,
            y_sizes: vec![0; mi_cols * mi_rows],
            uv_sizes: vec![0; mi_cols * mi_rows],
            y_colors: vec![[0; 8]; mi_cols * mi_rows],
            u_colors: vec![[0; 8]; mi_cols * mi_rows],
            v_colors: vec![[0; 8]; mi_cols * mi_rows],
        }
    }

    pub fn y_size(&self, row: usize, col: usize) -> u8 {
        self.y_sizes
            .get(row * self.mi_cols + col)
            .copied()
            .unwrap_or(0)
    }

    pub fn uv_size(&self, row: usize, col: usize) -> u8 {
        self.uv_sizes
            .get(row * self.mi_cols + col)
            .copied()
            .unwrap_or(0)
    }

    fn colors(&self, colors: &[[u16; 8]], row: usize, col: usize) -> Option<[u16; 8]> {
        colors.get(row * self.mi_cols + col).copied()
    }

    fn cache(&self, plane: usize, row: usize, col: usize) -> Vec<u16> {
        let mut values = Vec::new();
        if row > 0 && (row * 4) % 64 != 0 {
            let n = if plane == 0 {
                self.y_size(row - 1, col)
            } else {
                self.uv_size(row - 1, col)
            } as usize;
            if let Some(colors) = if plane == 0 {
                self.colors(&self.y_colors, row - 1, col)
            } else {
                self.colors(&self.u_colors, row - 1, col)
            } {
                values.extend_from_slice(&colors[..n]);
            }
        }
        if col > 0 {
            let n = if plane == 0 {
                self.y_size(row, col - 1)
            } else {
                self.uv_size(row, col - 1)
            } as usize;
            if let Some(colors) = if plane == 0 {
                self.colors(&self.y_colors, row, col - 1)
            } else {
                self.colors(&self.u_colors, row, col - 1)
            } {
                values.extend_from_slice(&colors[..n]);
            }
        }
        values.sort_unstable();
        values.dedup();
        values
    }

    #[allow(clippy::too_many_arguments)]
    fn read_colors(
        &self,
        decoder: &mut SymbolDecoder<'_>,
        plane: usize,
        size: usize,
        row: usize,
        col: usize,
        bit_depth: u32,
    ) -> Result<[u16; 8], PaletteError> {
        let mut colors = [0u16; 8];
        let mut idx = 0;
        let cache = self.cache(plane, row, col);
        for color in cache {
            if idx == size {
                break;
            }
            if decoder.read_literal(1)? != 0 {
                colors[idx] = color;
                idx += 1;
            }
        }
        if idx < size {
            colors[idx] = decoder.read_literal(bit_depth)? as u16;
            idx += 1;
        }
        if idx < size {
            let mut bits = bit_depth - 3 + decoder.read_literal(2)?;
            while idx < size {
                // Luma deltas are transmitted as value-minus-one; UV deltas are
                // literal, matching palette_colors_y/uv in the specification.
                let delta = decoder.read_literal(bits)? + u32::from(plane == 0);
                let value =
                    (u32::from(colors[idx - 1]) + delta).min((1u32 << bit_depth) - 1) as u16;
                colors[idx] = value;
                bits = bits.min(ceil_log2(palette_delta_range(bit_depth, value, plane)));
                idx += 1;
            }
        }
        colors[..size].sort_unstable();
        Ok(colors)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn read_mode_info(
        &mut self,
        decoder: &mut SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        row: usize,
        col: usize,
        size: BlockSize,
        y_mode: IntraMode,
        uv_mode: Option<UvMode>,
        has_chroma: bool,
        allow_screen_content_tools: bool,
        disable_cdf_update: bool,
        bit_depth: u32,
    ) -> Result<PaletteModeInfo, PaletteError> {
        if !allow_screen_content_tools || !palette_allowed_size(size) {
            return Ok(PaletteModeInfo::default());
        }
        let bsize_ctx = (size.mi_width_log2() + size.mi_height_log2() - 2) as usize;
        let mut info = PaletteModeInfo::default();
        if y_mode == IntraMode::Dc {
            let above = row > 0 && self.y_size(row - 1, col) > 0;
            let left = col > 0 && self.y_size(row, col - 1) > 0;
            let ctx = usize::from(above) + usize::from(left);
            if cdfs.read_symbol(
                decoder,
                &DEFAULT_PALETTE_Y_MODE_CDF,
                (bsize_ctx * 3 + ctx) * 3,
                3,
                disable_cdf_update,
            )? != 0
            {
                let n = cdfs.read_symbol(
                    decoder,
                    &DEFAULT_PALETTE_Y_SIZE_CDF,
                    bsize_ctx * 8,
                    8,
                    disable_cdf_update,
                )? + 2;
                let colors = self.read_colors(decoder, 0, n, row, col, bit_depth)?;
                info.y_size = n;
                info.y_colors = colors;
                self.fill(0, row, col, size, n as u8, colors);
            }
        }
        if has_chroma && uv_mode == Some(UvMode::Dc) {
            let ctx = usize::from(info.y_size > 0);
            if cdfs.read_symbol(
                decoder,
                &DEFAULT_PALETTE_UV_MODE_CDF,
                ctx * 3,
                3,
                disable_cdf_update,
            )? != 0
            {
                let n = cdfs.read_symbol(
                    decoder,
                    &DEFAULT_PALETTE_UV_SIZE_CDF,
                    bsize_ctx * 8,
                    8,
                    disable_cdf_update,
                )? + 2;
                let u = self.read_colors(decoder, 1, n, row, col, bit_depth)?;
                let mut v = [0u16; 8];
                if decoder.read_literal(1)? != 0 {
                    let bits = bit_depth - 4 + decoder.read_literal(2)?;
                    v[0] = decoder.read_literal(bit_depth)? as u16;
                    for idx in 1..n {
                        let mut delta = decoder.read_literal(bits)? as i32;
                        if delta != 0 && decoder.read_literal(1)? != 0 {
                            delta = -delta;
                        }
                        let max = 1i32 << bit_depth;
                        let mut value = i32::from(v[idx - 1]) + delta;
                        if value < 0 {
                            value += max;
                        }
                        if value >= max {
                            value -= max;
                        }
                        v[idx] = value.clamp(0, max - 1) as u16;
                    }
                } else {
                    for color in v.iter_mut().take(n) {
                        *color = decoder.read_literal(bit_depth)? as u16;
                    }
                }
                info.uv_size = n;
                info.u_colors = u;
                info.v_colors = v;
                self.fill(1, row, col, size, n as u8, u);
                self.fill_v(row, col, size, v);
            }
        }
        Ok(info)
    }

    fn fill(
        &mut self,
        plane: usize,
        row: usize,
        col: usize,
        size: BlockSize,
        n: u8,
        colors: [u16; 8],
    ) {
        let w = size.num_4x4_wide() as usize;
        let h = size.num_4x4_high() as usize;
        for r in row..(row + h).min(self.mi_rows) {
            for c in col..(col + w).min(self.mi_cols) {
                if plane == 0 {
                    self.y_sizes[r * self.mi_cols + c] = n;
                    self.y_colors[r * self.mi_cols + c] = colors;
                } else {
                    self.uv_sizes[r * self.mi_cols + c] = n;
                    self.u_colors[r * self.mi_cols + c] = colors;
                }
            }
        }
    }

    fn fill_v(&mut self, row: usize, col: usize, size: BlockSize, colors: [u16; 8]) {
        let w = size.num_4x4_wide() as usize;
        let h = size.num_4x4_high() as usize;
        for r in row..(row + h).min(self.mi_rows) {
            for c in col..(col + w).min(self.mi_cols) {
                self.v_colors[r * self.mi_cols + c] = colors;
            }
        }
    }
}

fn ceil_log2(value: u32) -> u32 {
    32 - (value - 1).leading_zeros()
}

fn palette_delta_range(bit_depth: u32, previous: u16, plane: usize) -> u32 {
    let range = (1u32 << bit_depth) - u32::from(previous) - u32::from(plane == 0);
    range.max(1)
}

fn color_context(map: &[u8], width: usize, r: usize, c: usize, n: usize) -> ([u8; 8], usize) {
    let mut scores = [0u8; 8];
    let mut order = [0u8; 8];
    for (i, slot) in order.iter_mut().enumerate() {
        *slot = i as u8;
    }
    if c > 0 {
        scores[map[r * width + c - 1] as usize] += 2;
    }
    if r > 0 {
        scores[map[(r - 1) * width + c] as usize] += 2;
        if c > 0 {
            scores[map[(r - 1) * width + c - 1] as usize] += 1;
        }
    }
    for i in 0..3 {
        let mut max_idx = i;
        for j in i + 1..n {
            if scores[j] > scores[max_idx] {
                max_idx = j;
            }
        }
        if max_idx != i {
            let score = scores[max_idx];
            let color = order[max_idx];
            for k in (i + 1..=max_idx).rev() {
                scores[k] = scores[k - 1];
                order[k] = order[k - 1];
            }
            scores[i] = score;
            order[i] = color;
        }
    }
    let hash: usize = (0..3)
        .map(|i| usize::from(scores[i]) * usize::from(COLOR_HASH_MULTIPLIERS[i]))
        .sum();
    (order, COLOR_CONTEXT[hash] as usize)
}

fn decode_map(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    width: usize,
    height: usize,
    plane: usize,
    palette_size: usize,
    disable_cdf_update: bool,
) -> Result<PaletteMap, PaletteError> {
    if !(2..=8).contains(&palette_size) {
        return Err(PaletteError::BadPaletteSize(palette_size));
    }
    let mut indices = vec![0u8; width * height];
    let first = read_ns(decoder, palette_size as u32)? as u8;
    indices[0] = first;
    for i in 1..width + height - 1 {
        let hi = i.min(width - 1);
        let lo = i.saturating_sub(height - 1);
        for j in (lo..=hi).rev() {
            let r = i - j;
            let (order, ctx) = color_context(&indices, width, r, j, palette_size);
            let table = if plane == 0 {
                Y_COLOR_CDFS[palette_size - 2]
            } else {
                UV_COLOR_CDFS[palette_size - 2]
            };
            let width_cdf = palette_size + 1;
            let symbol = cdfs.read_symbol(
                decoder,
                table,
                ctx * width_cdf,
                width_cdf,
                disable_cdf_update,
            )?;
            indices[r * width + j] = order[symbol];
        }
    }
    Ok(PaletteMap {
        width,
        height,
        indices,
    })
}

/// `palette_tokens()` for one block.
#[allow(clippy::too_many_arguments)]
pub fn read_palette_tokens(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    info: PaletteModeInfo,
    mi_row: usize,
    mi_col: usize,
    mi_rows: usize,
    mi_cols: usize,
    block_size: BlockSize,
    sub_x: u32,
    sub_y: u32,
    disable_cdf_update: bool,
) -> Result<PaletteTokens, PaletteError> {
    let mut y = None;
    if info.y_size > 0 {
        let width = block_size.width as usize;
        let height = block_size.height as usize;
        let onscreen_width = width.min((mi_cols - mi_col) * 4);
        let onscreen_height = height.min((mi_rows - mi_row) * 4);
        let map = decode_map(
            decoder,
            cdfs,
            onscreen_width,
            onscreen_height,
            usize::from(false),
            info.y_size,
            disable_cdf_update,
        )?;
        y = Some(map);
    }
    let mut uv = None;
    if info.uv_size > 0 {
        let block_width = (block_size.width as usize) >> sub_x;
        let block_height = (block_size.height as usize) >> sub_y;
        let mut onscreen_width = block_width.min(((mi_cols - mi_col) * 4) >> sub_x);
        let mut onscreen_height = block_height.min(((mi_rows - mi_row) * 4) >> sub_y);
        if block_width < 4 {
            onscreen_width += 2;
        }
        if block_height < 4 {
            onscreen_height += 2;
        }
        uv = Some(decode_map(
            decoder,
            cdfs,
            onscreen_width,
            onscreen_height,
            1,
            info.uv_size,
            disable_cdf_update,
        )?);
    }
    Ok(PaletteTokens { y, uv })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::partition::BLOCK_SIZES;

    #[test]
    fn zero_stream_selects_no_palette() {
        let mut state = PaletteState::new(8, 8);
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        let mut cdfs = CdfState::new();
        let info = state
            .read_mode_info(
                &mut decoder,
                &mut cdfs,
                0,
                0,
                BLOCK_SIZES[6],
                IntraMode::Dc,
                Some(UvMode::Dc),
                true,
                true,
                false,
                8,
            )
            .unwrap();
        assert_eq!(info.y_size, 0);
        assert_eq!(state.y_size(0, 0), 0);
    }

    #[test]
    fn palette_size_gate_uses_the_block_size_enumeration() {
        assert!(!palette_allowed_size(BLOCK_SIZES[0])); // 4x4
        assert!(!palette_allowed_size(BLOCK_SIZES[1])); // 4x8
        assert!(!palette_allowed_size(BLOCK_SIZES[2])); // 8x4
        assert!(palette_allowed_size(BLOCK_SIZES[3])); // 8x8
        assert!(palette_allowed_size(
            *BLOCK_SIZES
                .iter()
                .find(|size| size.name == "BLOCK_16X4")
                .unwrap()
        ));
        assert!(palette_allowed_size(
            *BLOCK_SIZES
                .iter()
                .find(|size| size.name == "BLOCK_4X16")
                .unwrap()
        ));
    }

    #[test]
    fn palette_index_map_uses_valid_symbols() {
        let mut decoder = SymbolDecoder::init(&[0u8; 256]).unwrap();
        let mut cdfs = CdfState::new();
        let tokens = read_palette_tokens(
            &mut decoder,
            &mut cdfs,
            PaletteModeInfo {
                y_size: 2,
                ..PaletteModeInfo::default()
            },
            0,
            0,
            8,
            8,
            BLOCK_SIZES[3],
            1,
            1,
            false,
        )
        .unwrap();
        let map = tokens.y.unwrap();
        assert_eq!((map.width, map.height), (8, 8));
        assert!(map.indices.iter().all(|index| *index < 2));
    }

    #[test]
    fn palette_delta_range_subtracts_one_only_for_luma() {
        assert_eq!(palette_delta_range(8, 254, 0), 1);
        assert_eq!(palette_delta_range(8, 254, 1), 2);
        assert_eq!(palette_delta_range(10, 1022, 0), 1);
        assert_eq!(palette_delta_range(10, 1022, 1), 2);
    }

    #[test]
    fn palette_colors_keep_ten_bit_samples() {
        let state = PaletteState::new(4, 4);
        let mut decoder = SymbolDecoder::init(&[0xffu8; 32]).unwrap();
        let colors = state.read_colors(&mut decoder, 0, 1, 0, 0, 10).unwrap();
        assert_eq!(colors[0], 1023);
    }

    #[test]
    fn palette_maps_expand_with_edge_replication() {
        let info = PaletteModeInfo {
            y_size: 3,
            y_colors: [10, 20, 30, 0, 0, 0, 0, 0],
            ..PaletteModeInfo::default()
        };
        let tokens = PaletteTokens {
            y: Some(PaletteMap {
                width: 2,
                height: 2,
                indices: vec![0, 1, 2, 1],
            }),
            uv: None,
        };
        let out = predict_palette_plane(&info, &tokens, 0, 0, 0, 0, 0, 4, 4)
            .unwrap()
            .unwrap();
        assert_eq!(
            out,
            [10, 20, 20, 20, 30, 20, 20, 20, 30, 20, 20, 20, 30, 20, 20, 20]
        );
    }

    #[test]
    fn palette_prediction_uses_transform_coordinates_without_adding_the_block_origin_twice() {
        let info = PaletteModeInfo {
            y_size: 4,
            y_colors: [10, 20, 30, 40, 0, 0, 0, 0],
            ..PaletteModeInfo::default()
        };
        let tokens = PaletteTokens {
            y: Some(PaletteMap {
                width: 4,
                height: 1,
                indices: vec![0, 1, 2, 3],
            }),
            uv: None,
        };
        assert_eq!(
            predict_palette_plane(&info, &tokens, 0, 8, 0, 8, 0, 2, 1)
                .unwrap()
                .unwrap(),
            [10, 20]
        );
    }

    #[test]
    fn palette_prediction_keeps_nonzero_transform_offsets() {
        let info = PaletteModeInfo {
            y_size: 7,
            y_colors: [32, 48, 67, 81, 113, 130, 145, 0],
            ..PaletteModeInfo::default()
        };
        let tokens = PaletteTokens {
            y: Some(PaletteMap {
                width: 16,
                height: 8,
                indices: [3, 3, 3, 3, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6].repeat(8),
            }),
            uv: None,
        };
        assert_eq!(
            predict_palette_plane(&info, &tokens, 0, 32, 0, 32, 0, 8, 8)
                .unwrap()
                .unwrap()[..8],
            [81, 81, 81, 81, 145, 145, 145, 145]
        );
    }

    #[test]
    fn palette_uv_prediction_selects_u_or_v_colors() {
        let info = PaletteModeInfo {
            uv_size: 2,
            u_colors: [40, 50, 0, 0, 0, 0, 0, 0],
            v_colors: [60, 70, 0, 0, 0, 0, 0, 0],
            ..PaletteModeInfo::default()
        };
        let tokens = PaletteTokens {
            y: None,
            uv: Some(PaletteMap {
                width: 2,
                height: 2,
                indices: vec![0, 1, 1, 0],
            }),
        };
        assert_eq!(
            predict_palette_plane(&info, &tokens, 1, 0, 0, 0, 0, 2, 2)
                .unwrap()
                .unwrap(),
            [40, 50, 50, 40]
        );
        assert_eq!(
            predict_palette_plane(&info, &tokens, 2, 0, 0, 0, 0, 2, 2)
                .unwrap()
                .unwrap(),
            [60, 70, 70, 60]
        );
    }

    #[test]
    fn only_luma_palette_deltas_are_biased_by_one() {
        let state = PaletteState::new(4, 4);
        let mut y_decoder = SymbolDecoder::init(&[0u8; 32]).unwrap();
        let y = state.read_colors(&mut y_decoder, 0, 2, 0, 0, 8).unwrap();
        let mut uv_decoder = SymbolDecoder::init(&[0u8; 32]).unwrap();
        let uv = state.read_colors(&mut uv_decoder, 1, 2, 0, 0, 8).unwrap();
        assert_eq!(&y[..2], &[0, 1]);
        assert_eq!(&uv[..2], &[0, 0]);
    }
}
