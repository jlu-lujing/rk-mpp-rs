//! Intra key-frame tile sink: consume block syntax and transform residuals.
//!
//! This first sink records syntax and coefficients but does not write pixels.
//! It is the correctness gate for tile entropy order before the already-tested
//! prediction/reconstruction block is attached.

use crate::reference::av1::cdef::CdefIndexGrid;
use crate::reference::av1::cdf_state::CdfState;
use crate::reference::av1::cfl::{read_cfl_alphas, CflAlphas};
use crate::reference::av1::coeff::{coeff_q_context, read_all_zero_with_cdf, seg_eob};
use crate::reference::av1::coeff_block::decode_coefficient_payload;
use crate::reference::av1::context::TileContext;
use crate::reference::av1::delta::{read_delta_lf, read_delta_qindex, DeltaError};
use crate::reference::av1::filter_intra::{
    filter_intra_direction, read_filter_intra, FilterIntraMode,
};
use crate::reference::av1::frame_header::FrameHeader;
use crate::reference::av1::inter::{
    find_mv_stack, read_mv, CompoundMode, InterError, InterMode, MotionGrid, MotionVector,
    INTRA_FRAME, NONE_FRAME,
};
use crate::reference::av1::intra::{
    cfl_allowed, read_angle_delta_with_cdf, read_intra_frame_y_mode_with_cdf,
    read_uv_mode_with_cdf, read_y_mode_with_cdf, IntraMode, UvMode,
};
use crate::reference::av1::lr_frame::{LrFrameParams, LrPlaneGeometry};
use crate::reference::av1::lr_syntax::{LrSyntaxError, LrTileState};
use crate::reference::av1::palette::{
    read_palette_tokens, PaletteError, PaletteModeInfo, PaletteState, PaletteTokens,
};
use crate::reference::av1::partition::BlockSize;
use crate::reference::av1::residual::{intra_residual_blocks, residual_size, ResidualTxBlock};
use crate::reference::av1::segment::{SegmentCarry, SegmentMap, SegmentState};
use crate::reference::av1::seq_header::SequenceHeader;
use crate::reference::av1::tile::DecodedBlock;
use crate::reference::av1::tile_entropy::{TileBlockSink, TileEntropyError};
use crate::reference::av1::tx::{max_tx_size_rect, read_tx_size_with_cdf, split_tx_size, TxSize};
use crate::reference::av1::tx_type::{
    is_tx_type_in_set, read_inter_tx_type_with_cdf, read_intra_tx_type_with_cdf, tx_set,
    uv_mode_tx_type, TxType,
};

#[derive(Debug, Clone)]
pub struct IntraTileConfig {
    pub mi_cols: usize,
    pub mi_rows: usize,
    /// mi origin of the tile being decoded; segment-id spatial prediction and
    /// the seg-id-predicted context clip here, not at the picture edge.
    pub tile_row_start: usize,
    pub tile_col_start: usize,
    pub tile_row_end: usize,
    pub tile_col_end: usize,
    pub sub_x: u32,
    pub sub_y: u32,
    pub bit_depth: u32,
    pub key_frame: bool,
    pub segmentation_enabled: bool,
    pub segmentation_update_map: bool,
    pub seg_id_pre_skip: bool,
    pub last_active_seg_id: usize,
    pub segment_alt_q: [i32; 8],
    pub segment_lossless: [bool; 8],
    pub segment_skip: [bool; 8],
    pub previous_segment_map: Option<SegmentMap>,
    pub allow_screen_content_tools: bool,
    pub enable_filter_intra: bool,
    pub enable_intra_edge_filter: bool,
    pub disable_cdf_update: bool,
    pub tx_mode_select: bool,
    pub reduced_tx_set: bool,
    pub delta_q_present: bool,
    pub delta_q_res: u8,
    pub delta_lf_present: bool,
    pub delta_lf_res: u8,
    pub delta_lf_multi: bool,
    pub has_chroma: bool,
    pub cdef_enabled: bool,
    pub cdef_bits: u8,
    pub base_q_index: u8,
    pub superblock_mi: usize,
    pub allow_intrabc: bool,
    pub lr_geometry: Vec<LrPlaneGeometry>,
    pub superres_denom: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntraSyntaxBlock {
    pub block: DecodedBlock,
    pub segment_id: usize,
    pub skip: bool,
    pub lossless: bool,
    pub qindex: u8,
    pub delta_lf: [i32; 4],
    pub y_mode: IntraMode,
    pub uv_mode: Option<UvMode>,
    pub cfl_alphas: CflAlphas,
    pub angle_y: i32,
    pub angle_uv: i32,
    pub filter_intra: Option<FilterIntraMode>,
    pub palette: PaletteModeInfo,
    pub palette_tokens: PaletteTokens,
    pub tx: TxSize,
    pub intrabc_dv: Option<MotionVector>,
    pub transforms: Vec<IntraSyntaxTx>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntraSyntaxTx {
    pub plane: usize,
    pub x: usize,
    pub y: usize,
    pub tx: TxSize,
    pub tx_type: TxType,
    pub all_zero: bool,
    pub eob: u32,
    pub quant: Vec<i32>,
}

pub struct IntraTileBlockSink {
    config: IntraTileConfig,
    tile_context: TileContext,
    segments: SegmentState,
    palettes: PaletteState,
    cdef: CdefIndexGrid,
    skip_grid: Vec<bool>,
    y_modes: Vec<Option<IntraMode>>,
    uv_modes: Vec<Option<UvMode>>,
    tx_grid: Vec<Option<TxSize>>,
    decoded: Vec<bool>,
    sizes: Vec<Option<BlockSize>>,
    is_inter: Vec<bool>,
    refs: Vec<[i8; 2]>,
    inter_modes: Vec<Option<InterMode>>,
    compound_modes: Vec<Option<CompoundMode>>,
    mvs: Vec<[MotionVector; 2]>,
    inter_tx_types: Vec<Option<TxType>>,
    current_qindex: u8,
    delta_lf: [i32; 4],
    read_deltas_for_sb: bool,
    current_sb: Option<(usize, usize)>,
    lr: LrTileState,
    lr_cdfs_loaded: bool,
    pub blocks: Vec<IntraSyntaxBlock>,
}

impl IntraTileBlockSink {
    pub fn new(config: IntraTileConfig) -> Self {
        let mi_cols = config.mi_cols;
        let mi_rows = config.mi_rows;
        let (tile_row_start, tile_col_start) = (config.tile_row_start, config.tile_col_start);
        let current_qindex = config.base_q_index;
        let previous_segment_map = config.previous_segment_map.clone();
        let last_active_seg_id = config.last_active_seg_id;
        let lr = LrTileState::new(config.lr_geometry.clone());
        Self {
            config,
            tile_context: TileContext::new(mi_cols, mi_rows),
            segments: {
                let mut segments = SegmentState::with_previous(
                    mi_cols,
                    mi_rows,
                    last_active_seg_id,
                    previous_segment_map,
                );
                segments.set_tile_origin(tile_row_start, tile_col_start);
                segments
            },
            palettes: PaletteState::new(mi_cols, mi_rows),
            cdef: CdefIndexGrid::new(mi_cols, mi_rows),
            skip_grid: vec![false; mi_cols * mi_rows],
            y_modes: vec![None; mi_cols * mi_rows],
            uv_modes: vec![None; mi_cols * mi_rows],
            tx_grid: vec![None; mi_cols * mi_rows],
            decoded: vec![false; mi_cols * mi_rows],
            sizes: vec![None; mi_cols * mi_rows],
            is_inter: vec![false; mi_cols * mi_rows],
            refs: vec![[INTRA_FRAME, NONE_FRAME]; mi_cols * mi_rows],
            inter_modes: vec![None; mi_cols * mi_rows],
            compound_modes: vec![None; mi_cols * mi_rows],
            mvs: vec![[MotionVector::default(); 2]; mi_cols * mi_rows],
            inter_tx_types: vec![None; mi_cols * mi_rows],
            current_qindex,
            delta_lf: [0; 4],
            read_deltas_for_sb: false,
            current_sb: None,
            lr,
            lr_cdfs_loaded: false,
            blocks: Vec::new(),
        }
    }

    /// The segment state libaom keeps in the frame: a tile must see the ids an
    /// earlier tile wrote.
    pub fn segment_carry(&self) -> SegmentCarry {
        self.segments.carry()
    }

    /// Continue a fresh sink from the segment ids of the previous tile.
    #[must_use]
    pub fn with_segment_carry(mut self, carry: SegmentCarry) -> Self {
        self.segments.restore_carry(&carry);
        self
    }

    pub fn cdef_grid(&self) -> &CdefIndexGrid {
        &self.cdef
    }

    pub fn lr_params(&self) -> &LrFrameParams {
        &self.lr.params
    }

    pub fn segment_map(&self) -> SegmentMap {
        self.segments.snapshot()
    }

    fn neighbor<T: Copy>(grid: &[T], mi_cols: usize, row: isize, col: isize) -> Option<T> {
        if row < 0 || col < 0 {
            return None;
        }
        grid.get(row as usize * mi_cols + col as usize).copied()
    }

    fn has_chroma(&self, block: DecodedBlock) -> bool {
        if !self.config.has_chroma {
            return false;
        }
        let bw4 = block.size.num_4x4_wide() as usize;
        let bh4 = block.size.num_4x4_high() as usize;
        if bh4 == 1 && self.config.sub_y != 0 && block.row & 1 == 0 {
            false
        } else if bw4 == 1 && self.config.sub_x != 0 && block.col & 1 == 0 {
            false
        } else {
            true
        }
    }

    fn delta_as_tile(error: DeltaError) -> TileEntropyError {
        match error {
            DeltaError::Cdf(error) => TileEntropyError::Cdf(error),
            DeltaError::Symbol(error) => TileEntropyError::Symbol(error),
        }
    }

    fn palette_as_tile(error: PaletteError) -> TileEntropyError {
        match error {
            PaletteError::Cdf(error) => TileEntropyError::Cdf(error),
            PaletteError::Symbol(error) => TileEntropyError::Symbol(error),
            PaletteError::BadPaletteSize(_) => TileEntropyError::Unsupported("palette size"),
            PaletteError::MissingTokens => TileEntropyError::Unsupported("palette tokens"),
            PaletteError::InvalidIndex(_) => TileEntropyError::Unsupported("palette color index"),
        }
    }

    fn lr_as_tile(error: LrSyntaxError) -> TileEntropyError {
        match error {
            LrSyntaxError::Symbol(error) => TileEntropyError::Symbol(error),
            _ => TileEntropyError::Unsupported("loop restoration syntax"),
        }
    }

    fn inter_as_tile(error: InterError) -> TileEntropyError {
        match error {
            InterError::Cdf(error) => TileEntropyError::Cdf(error),
            InterError::Symbol(error) => TileEntropyError::Symbol(error),
            InterError::Unsupported(reason) | InterError::InvalidSyntax(reason) => {
                TileEntropyError::Unsupported(reason)
            }
        }
    }

    fn intrabc_dv_reference(&self, block: DecodedBlock) -> Result<MotionVector, TileEntropyError> {
        let grid = MotionGrid {
            mi_cols: self.config.mi_cols,
            mi_rows: self.config.mi_rows,
            tile_row_start: self.config.tile_row_start,
            tile_row_end: self.config.tile_row_end,
            tile_col_start: self.config.tile_col_start,
            tile_col_end: self.config.tile_col_end,
            decoded: &self.decoded,
            sizes: &self.sizes,
            is_inter: &self.is_inter,
            refs: &self.refs,
            modes: &self.inter_modes,
            compound_modes: &self.compound_modes,
            mvs: &self.mvs,
            temporal: None,
        };
        let stack = find_mv_stack(
            grid,
            block.row,
            block.col,
            block.size,
            [INTRA_FRAME, NONE_FRAME],
            [MotionVector::default(); 2],
            false,
            false,
            false,
        )
        .map_err(Self::inter_as_tile)?;
        let nearest = stack.candidates[0][0];
        let near = stack.candidates[1][0];
        let mut reference = if nearest != MotionVector::default() {
            nearest
        } else {
            near
        };
        if reference == MotionVector::default() {
            let mib_size = self.config.superblock_mi as i32;
            reference = if block.row < self.config.tile_row_start + self.config.superblock_mi {
                MotionVector::new(0, -(4 * mib_size + 256) * 8)
            } else {
                MotionVector::new(-4 * mib_size * 8, 0)
            };
        }
        Ok(reference)
    }

    fn intrabc_dv_valid(&self, block: DecodedBlock, dv: MotionVector) -> bool {
        if !dv.is_integer() {
            return false;
        }
        let scale = 8i64;
        let mi_size = 4i64;
        let src_top = block.row as i64 * mi_size * scale + i64::from(dv.row);
        let src_left = block.col as i64 * mi_size * scale + i64::from(dv.col);
        let src_bottom =
            (block.row as i64 * mi_size + i64::from(block.size.height)) * scale + i64::from(dv.row);
        let src_right =
            (block.col as i64 * mi_size + i64::from(block.size.width)) * scale + i64::from(dv.col);
        src_top >= self.config.tile_row_start as i64 * mi_size * scale
            && src_left >= self.config.tile_col_start as i64 * mi_size * scale
            && src_bottom <= self.config.tile_row_end as i64 * mi_size * scale
            && src_right <= self.config.tile_col_end as i64 * mi_size * scale
    }

    fn record_block(
        &mut self,
        block: DecodedBlock,
        skip: bool,
        y_mode: IntraMode,
        uv_mode: Option<UvMode>,
        tx: TxSize,
        intrabc_dv: Option<MotionVector>,
    ) {
        let bw4 = block.size.num_4x4_wide() as usize;
        let bh4 = block.size.num_4x4_high() as usize;
        for row in block.row..(block.row + bh4).min(self.config.mi_rows) {
            for col in block.col..(block.col + bw4).min(self.config.mi_cols) {
                let index = row * self.config.mi_cols + col;
                self.skip_grid[index] = skip;
                self.y_modes[index] = Some(y_mode);
                self.uv_modes[index] = uv_mode;
                self.tx_grid[index] = Some(tx);
                self.decoded[index] = true;
                self.sizes[index] = Some(block.size);
                self.is_inter[index] = intrabc_dv.is_some();
                self.refs[index] = [INTRA_FRAME, NONE_FRAME];
                self.inter_modes[index] = intrabc_dv.map(|_| InterMode::Nearest);
                self.compound_modes[index] = None;
                self.mvs[index] = [intrabc_dv.unwrap_or_default(), MotionVector::default()];
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_transforms(
        &mut self,
        decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        block: DecodedBlock,
        tx: TxSize,
        block_qindex: u8,
        y_mode: IntraMode,
        uv_mode: Option<UvMode>,
        filter_mode: Option<FilterIntraMode>,
        has_chroma: bool,
        lossless: bool,
    ) -> Result<Vec<IntraSyntaxTx>, TileEntropyError> {
        let mi_cols = self.config.mi_cols;
        let mi_rows = self.config.mi_rows;
        let residual_blocks = intra_residual_blocks(
            block.row,
            block.col,
            block.size,
            tx,
            has_chroma,
            lossless,
            self.config.sub_x,
            self.config.sub_y,
            mi_cols,
            mi_rows,
        );
        let mut transforms = Vec::new();
        for tx_block in residual_blocks {
            let (sx, sy) = if tx_block.plane == 0 {
                (0, 0)
            } else {
                (self.config.sub_x, self.config.sub_y)
            };
            let residual_size = residual_size(block.size, sx, sy)
                .ok_or(TileEntropyError::Unsupported("invalid residual size"))?;
            let x4 = tx_block.x >> 2;
            let y4 = tx_block.y >> 2;
            let max_x4 = mi_cols >> sx;
            let max_y4 = mi_rows >> sy;
            let txb_ctx = self.tile_context.txb_skip_context(
                tx_block.plane,
                x4,
                y4,
                tx_block.tx,
                residual_size,
                max_x4,
                max_y4,
            );
            let q_ctx = coeff_q_context(self.config.base_q_index);
            let all_zero = read_all_zero_with_cdf(
                decoder,
                cdfs,
                tx_block.tx,
                q_ctx,
                txb_ctx,
                self.config.disable_cdf_update,
            )
            .map_err(TileEntropyError::Cdf)?;
            let tx_type = if all_zero {
                self.tile_context.update(
                    tx_block.plane,
                    x4,
                    y4,
                    (tx_block.tx.width() / 4) as usize,
                    (tx_block.tx.height() / 4) as usize,
                    0,
                    0,
                );
                TxType::DctDct
            } else if tx_block.plane == 0 {
                let dir = filter_mode.map(filter_intra_direction).unwrap_or(y_mode) as usize;
                read_intra_tx_type_with_cdf(
                    decoder,
                    cdfs,
                    tx_block.tx,
                    block_qindex,
                    dir,
                    self.config.reduced_tx_set,
                    self.config.disable_cdf_update,
                )
                .map_err(TileEntropyError::Cdf)?
            } else {
                let typ = uv_mode_tx_type(uv_mode.unwrap_or(UvMode::Dc));
                let set = tx_set(tx_block.tx, false, self.config.reduced_tx_set);
                if is_tx_type_in_set(set, typ, false) {
                    typ
                } else {
                    TxType::DctDct
                }
            };
            let decoded = if all_zero {
                crate::reference::av1::coeff_block::DecodedCoeffs {
                    all_zero: true,
                    eob: 0,
                    quant: vec![0; seg_eob(tx_block.tx) as usize],
                    tx_type,
                    dc_category: 0,
                    cul_level: 0,
                }
            } else {
                decode_coefficient_payload(
                    decoder,
                    cdfs,
                    &mut self.tile_context,
                    tx_block.plane,
                    x4,
                    y4,
                    max_x4,
                    max_y4,
                    tx_block.tx,
                    tx_type,
                    q_ctx,
                    self.config.disable_cdf_update,
                )
                .map_err(TileEntropyError::Cdf)?
            };
            transforms.push(IntraSyntaxTx {
                plane: tx_block.plane,
                x: tx_block.x,
                y: tx_block.y,
                tx: tx_block.tx,
                tx_type: decoded.tx_type,
                all_zero: decoded.all_zero,
                eob: decoded.eob,
                quant: decoded.quant,
            });
        }
        Ok(transforms)
    }

    fn decode_intrabc_transforms(
        &mut self,
        decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        block: DecodedBlock,
        tx: TxSize,
        has_chroma: bool,
        lossless: bool,
        mut luma_tx_blocks: Vec<ResidualTxBlock>,
    ) -> Result<Vec<IntraSyntaxTx>, TileEntropyError> {
        let mut plane_blocks = intra_residual_blocks(
            block.row,
            block.col,
            block.size,
            tx,
            has_chroma,
            lossless,
            self.config.sub_x,
            self.config.sub_y,
            self.config.mi_cols,
            self.config.mi_rows,
        );
        let residual_blocks = if luma_tx_blocks.is_empty() {
            plane_blocks
        } else {
            luma_tx_blocks.extend(plane_blocks.drain(..).filter(|entry| entry.plane > 0));
            luma_tx_blocks
        };
        let q_ctx = coeff_q_context(self.config.base_q_index);
        let mut transforms = Vec::new();
        for tx_block in residual_blocks {
            let (sx, sy) = if tx_block.plane == 0 {
                (0, 0)
            } else {
                (self.config.sub_x, self.config.sub_y)
            };
            let residual_size = residual_size(block.size, sx, sy)
                .ok_or(TileEntropyError::Unsupported("invalid residual size"))?;
            let x4 = tx_block.x >> 2;
            let y4 = tx_block.y >> 2;
            let max_x4 = self.config.mi_cols >> sx;
            let max_y4 = self.config.mi_rows >> sy;
            let txb_ctx = self.tile_context.txb_skip_context(
                tx_block.plane,
                x4,
                y4,
                tx_block.tx,
                residual_size,
                max_x4,
                max_y4,
            );
            let all_zero = read_all_zero_with_cdf(
                decoder,
                cdfs,
                tx_block.tx,
                q_ctx,
                txb_ctx,
                self.config.disable_cdf_update,
            )
            .map_err(TileEntropyError::Cdf)?;
            let tx_type = if all_zero {
                self.tile_context.update(
                    tx_block.plane,
                    x4,
                    y4,
                    (tx_block.tx.width() / 4) as usize,
                    (tx_block.tx.height() / 4) as usize,
                    0,
                    0,
                );
                if tx_block.plane == 0 {
                    let width = (tx_block.tx.width() / 4) as usize;
                    let height = (tx_block.tx.height() / 4) as usize;
                    for row in y4..(y4 + height).min(self.config.mi_rows) {
                        for col in x4..(x4 + width).min(self.config.mi_cols) {
                            self.inter_tx_types[row * self.config.mi_cols + col] =
                                Some(TxType::DctDct);
                        }
                    }
                }
                TxType::DctDct
            } else if tx_block.plane == 0 {
                let tx_type = read_inter_tx_type_with_cdf(
                    decoder,
                    cdfs,
                    tx_block.tx,
                    self.current_qindex,
                    self.config.reduced_tx_set,
                    self.config.disable_cdf_update,
                )
                .map_err(TileEntropyError::Cdf)?;
                let width = (tx_block.tx.width() / 4) as usize;
                let height = (tx_block.tx.height() / 4) as usize;
                for row in y4..(y4 + height).min(self.config.mi_rows) {
                    for col in x4..(x4 + width).min(self.config.mi_cols) {
                        self.inter_tx_types[row * self.config.mi_cols + col] = Some(tx_type);
                    }
                }
                tx_type
            } else {
                let luma_x = (x4 << sx).max(block.col);
                let luma_y = (y4 << sy).max(block.row);
                let candidate = self.inter_tx_types[luma_y * self.config.mi_cols + luma_x]
                    .unwrap_or(TxType::DctDct);
                let set = tx_set(tx_block.tx, true, self.config.reduced_tx_set);
                if is_tx_type_in_set(set, candidate, true) {
                    candidate
                } else {
                    TxType::DctDct
                }
            };
            let decoded = if all_zero {
                crate::reference::av1::coeff_block::DecodedCoeffs {
                    all_zero: true,
                    eob: 0,
                    quant: vec![0; seg_eob(tx_block.tx) as usize],
                    tx_type,
                    dc_category: 0,
                    cul_level: 0,
                }
            } else {
                decode_coefficient_payload(
                    decoder,
                    cdfs,
                    &mut self.tile_context,
                    tx_block.plane,
                    x4,
                    y4,
                    max_x4,
                    max_y4,
                    tx_block.tx,
                    tx_type,
                    q_ctx,
                    self.config.disable_cdf_update,
                )
                .map_err(TileEntropyError::Cdf)?
            };
            transforms.push(IntraSyntaxTx {
                plane: tx_block.plane,
                x: tx_block.x,
                y: tx_block.y,
                tx: tx_block.tx,
                tx_type: decoded.tx_type,
                all_zero: decoded.all_zero,
                eob: decoded.eob,
                quant: decoded.quant,
            });
        }
        Ok(transforms)
    }

    fn clear_skip_context(
        &mut self,
        block: DecodedBlock,
        has_chroma: bool,
    ) -> Result<(), TileEntropyError> {
        let planes = if has_chroma { 3 } else { 1 };
        for plane in 0..planes {
            let (sub_x, sub_y) = if plane == 0 {
                (0, 0)
            } else {
                (self.config.sub_x, self.config.sub_y)
            };
            let residual_size = residual_size(block.size, sub_x, sub_y)
                .ok_or(TileEntropyError::Unsupported("invalid residual size"))?;
            self.tile_context.clear_block(
                plane,
                block.col >> sub_x,
                block.row >> sub_y,
                residual_size.num_4x4_wide() as usize,
                residual_size.num_4x4_high() as usize,
            );
        }
        Ok(())
    }

    fn tx_depth_extents(&self, block: DecodedBlock) -> (u32, u32) {
        let mi_cols = self.config.mi_cols;
        let above = if block.row > self.config.tile_row_start {
            let index = (block.row - 1) * mi_cols + block.col;
            if self.is_inter[index] {
                self.sizes[index].map_or(0, |size| size.width)
            } else {
                Self::tx_extent(
                    &self.tx_grid,
                    mi_cols,
                    block.row as isize - 1,
                    block.col as isize,
                    true,
                )
            }
        } else {
            0
        };
        let left = if block.col > self.config.tile_col_start {
            let index = block.row * mi_cols + block.col - 1;
            if self.is_inter[index] {
                self.sizes[index].map_or(0, |size| size.height)
            } else {
                Self::tx_extent(
                    &self.tx_grid,
                    mi_cols,
                    block.row as isize,
                    block.col as isize - 1,
                    false,
                )
            }
        } else {
            0
        };
        (above, left)
    }

    fn above_context_index(&self, row: usize, col: usize) -> Option<usize> {
        (row > self.config.tile_row_start).then(|| (row - 1) * self.config.mi_cols + col)
    }

    fn left_context_index(&self, row: usize, col: usize) -> Option<usize> {
        (col > self.config.tile_col_start).then(|| row * self.config.mi_cols + col - 1)
    }

    fn above_tx_width(&self, block: DecodedBlock, row: usize, col: usize) -> u32 {
        if row == block.row && row == 0 {
            return 64;
        }
        if row == block.row {
            let Some(index) = self.above_context_index(row, col) else {
                return 64;
            };
            if self.skip_grid[index] && self.is_inter[index] {
                return self.sizes[index].map_or(64, |size| size.width);
            }
        }
        let Some(index) = self.above_context_index(row, col) else {
            return 64;
        };
        self.tx_grid[index].map_or(0, TxSize::width)
    }

    fn left_tx_height(&self, block: DecodedBlock, row: usize, col: usize) -> u32 {
        if col == block.col && col == 0 {
            return 64;
        }
        if col == block.col {
            let Some(index) = self.left_context_index(row, col) else {
                return 64;
            };
            if self.skip_grid[index] && self.is_inter[index] {
                return self.sizes[index].map_or(64, |size| size.height);
            }
        }
        let Some(index) = self.left_context_index(row, col) else {
            return 64;
        };
        self.tx_grid[index].map_or(0, TxSize::height)
    }

    fn txfm_partition_context(
        &self,
        block: DecodedBlock,
        row: usize,
        col: usize,
        tx: TxSize,
    ) -> usize {
        let above = usize::from(self.above_tx_width(block, row, col) < tx.width());
        let left = usize::from(self.left_tx_height(block, row, col) < tx.height());
        let size = block
            .size
            .width
            .max(block.size.height)
            .min(64)
            .trailing_zeros()
            .saturating_sub(2) as usize;
        let square_up = crate::reference::av1::coeff::tx_sqr_up_index(tx);
        (usize::from(square_up != size) * 3) + (4 - size) * 6 + above + left
    }

    fn read_var_tx_size(
        &mut self,
        decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        block: DecodedBlock,
        row: usize,
        col: usize,
        tx: TxSize,
        depth: u8,
        luma_blocks: &mut Vec<ResidualTxBlock>,
    ) -> Result<(), TileEntropyError> {
        if row >= self.config.mi_rows || col >= self.config.mi_cols {
            return Ok(());
        }
        let split = if tx == TxSize::Tx4x4 || depth == 2 {
            false
        } else {
            let context = self.txfm_partition_context(block, row, col, tx);
            cdfs.read_symbol(
                decoder,
                &crate::reference::av1::cdf_defaults::DEFAULT_TXFM_SPLIT_CDF,
                context * 3,
                3,
                self.config.disable_cdf_update,
            )
            .map_err(TileEntropyError::Cdf)?
                != 0
        };
        let width4 = (tx.width() / 4) as usize;
        let height4 = (tx.height() / 4) as usize;
        if split {
            let sub_tx = split_tx_size(tx);
            let step_width = (sub_tx.width() / 4) as usize;
            let step_height = (sub_tx.height() / 4) as usize;
            for child_row in (0..height4).step_by(step_height) {
                for child_col in (0..width4).step_by(step_width) {
                    self.read_var_tx_size(
                        decoder,
                        cdfs,
                        block,
                        row + child_row,
                        col + child_col,
                        sub_tx,
                        depth + 1,
                        luma_blocks,
                    )?;
                }
            }
        } else {
            for r in row..(row + height4).min(self.config.mi_rows) {
                for c in col..(col + width4).min(self.config.mi_cols) {
                    self.tx_grid[r * self.config.mi_cols + c] = Some(tx);
                }
            }
            luma_blocks.push(ResidualTxBlock {
                plane: 0,
                x: col * 4,
                y: row * 4,
                tx,
            });
        }
        Ok(())
    }

    fn read_inter_tx_sizes(
        &mut self,
        decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        block: DecodedBlock,
    ) -> Result<Vec<ResidualTxBlock>, TileEntropyError> {
        let max_tx = max_tx_size_rect(block.size);
        let step_width = (max_tx.width() / 4) as usize;
        let step_height = (max_tx.height() / 4) as usize;
        let width = block.size.num_4x4_wide() as usize;
        let height = block.size.num_4x4_high() as usize;
        let mut blocks = Vec::new();
        for row in (0..height).step_by(step_height) {
            for col in (0..width).step_by(step_width) {
                self.read_var_tx_size(
                    decoder,
                    cdfs,
                    block,
                    block.row + row,
                    block.col + col,
                    max_tx,
                    0,
                    &mut blocks,
                )?;
            }
        }
        Ok(blocks)
    }

    fn tx_extent(
        grid: &[Option<TxSize>],
        mi_cols: usize,
        row: isize,
        col: isize,
        horizontal: bool,
    ) -> u32 {
        match Self::neighbor(grid, mi_cols, row, col).flatten() {
            Some(tx) if horizontal => tx.width(),
            Some(tx) if !horizontal => tx.height(),
            _ => 1,
        }
    }
}

impl IntraTileConfig {
    /// Point the tile-boundary clipping at one tile.
    #[must_use]
    pub fn with_tile_bounds(
        mut self,
        tile_row_start: usize,
        tile_row_end: usize,
        tile_col_start: usize,
        tile_col_end: usize,
    ) -> Self {
        self.tile_row_start = tile_row_start;
        self.tile_row_end = tile_row_end;
        self.tile_col_start = tile_col_start;
        self.tile_col_end = tile_col_end;
        self
    }

    pub fn from_headers(seq: &SequenceHeader, header: &FrameHeader) -> Self {
        let mut segment_skip = [false; 8];
        let mut segment_alt_q = [0; 8];
        for (segment, enabled) in segment_skip.iter_mut().enumerate() {
            *enabled = header.feature_enabled[segment][6];
            if header.feature_enabled[segment][0] {
                segment_alt_q[segment] = header.feature_data[segment][0];
            }
        }
        Self {
            mi_cols: header.mi_cols,
            mi_rows: header.mi_rows,
            tile_row_start: 0,
            tile_col_start: 0,
            tile_row_end: header.mi_rows,
            tile_col_end: header.mi_cols,
            sub_x: u32::from(seq.color.subsampling_x),
            sub_y: u32::from(seq.color.subsampling_y),
            bit_depth: u32::from(seq.color.bit_depth),
            key_frame: header.frame_is_intra,
            segmentation_enabled: header.segmentation_enabled,
            segmentation_update_map: header.segmentation_update_map,
            seg_id_pre_skip: header.seg_id_pre_skip,
            last_active_seg_id: header.last_active_seg_id as usize,
            segment_alt_q,
            segment_lossless: header.lossless_array,
            segment_skip,
            previous_segment_map: None,
            allow_screen_content_tools: header.allow_screen_content_tools != 0,
            enable_filter_intra: seq.enable_filter_intra,
            enable_intra_edge_filter: seq.enable_intra_edge_filter,
            disable_cdf_update: header.disable_cdf_update,
            tx_mode_select: header.tx_mode == 2,
            reduced_tx_set: header.reduced_tx_set,
            delta_q_present: header.delta_q_present,
            delta_q_res: header.delta_q_res,
            delta_lf_present: header.delta_lf_present,
            delta_lf_res: header.delta_lf_res,
            delta_lf_multi: header.delta_lf_multi,
            has_chroma: !seq.color.mono_chrome,
            cdef_enabled: seq.enable_cdef,
            cdef_bits: header.cdef_bits,
            base_q_index: header.base_q_idx,
            superblock_mi: if seq.use_128x128_superblock { 32 } else { 16 },
            allow_intrabc: header.allow_intrabc,
            lr_geometry: (0..seq.color.num_planes)
                .map(|plane| {
                    if header.uses_lr {
                        header.lr_geometry(plane, seq)
                    } else {
                        let (sub_x, sub_y) = if plane == 0 {
                            (0, 0)
                        } else {
                            (
                                u32::from(seq.color.subsampling_x),
                                u32::from(seq.color.subsampling_y),
                            )
                        };
                        LrPlaneGeometry::new(
                            header.frame_height as usize,
                            header.upscaled_width as usize,
                            64,
                            sub_x,
                            sub_y,
                            crate::reference::av1::restoration::RestorationType::None,
                        )
                    }
                })
                .collect(),
            superres_denom: header.use_superres.then_some(header.superres_denom),
        }
    }
}

impl TileBlockSink for IntraTileBlockSink {
    fn decode_superblock(
        &mut self,
        block: DecodedBlock,
        decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
        cdfs: &mut CdfState,
    ) -> Result<(), TileEntropyError> {
        if !self.lr_cdfs_loaded {
            self.lr.load_frame_cdfs(cdfs)?;
            self.lr_cdfs_loaded = true;
        }
        self.lr
            .read_lr(
                decoder,
                block.row,
                block.col,
                block.size.num_4x4_wide() as usize,
                block.size.num_4x4_high() as usize,
                self.config.allow_intrabc,
                self.config.superres_denom,
                self.config.disable_cdf_update,
            )
            .map_err(Self::lr_as_tile)?;
        self.lr.store_frame_cdfs(cdfs)?;
        Ok(())
    }

    fn decode_block(
        &mut self,
        block: DecodedBlock,
        decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
        cdfs: &mut CdfState,
    ) -> Result<(), TileEntropyError> {
        let mi_cols = self.config.mi_cols;
        let mi_rows = self.config.mi_rows;
        let avail_u = block.row > 0;
        let avail_l = block.col > 0;
        let above_skip = avail_u && self.skip_grid[(block.row - 1) * mi_cols + block.col];
        let left_skip = avail_l && self.skip_grid[block.row * mi_cols + block.col - 1];
        let above_mode = if avail_u {
            self.y_modes[(block.row - 1) * mi_cols + block.col]
        } else {
            None
        };
        let left_mode = if avail_l {
            self.y_modes[block.row * mi_cols + block.col - 1]
        } else {
            None
        };
        let has_chroma = self.has_chroma(block);
        let sb = (
            block.row / self.config.superblock_mi,
            block.col / self.config.superblock_mi,
        );
        if self.current_sb != Some(sb) {
            self.current_sb = Some(sb);
            self.read_deltas_for_sb = true;
        }

        let bw4 = block.size.num_4x4_wide() as usize;
        let bh4 = block.size.num_4x4_high() as usize;
        let segment_before = if self.config.seg_id_pre_skip {
            Some(
                self.segments
                    .read_intra_segment_id(
                        decoder,
                        cdfs,
                        block.row,
                        block.col,
                        bw4,
                        bh4,
                        false,
                        self.config.segmentation_enabled,
                        self.config.disable_cdf_update,
                    )
                    .map_err(TileEntropyError::Cdf)?,
            )
        } else {
            None
        };
        let forced_skip = segment_before.is_some_and(|id| self.config.segment_skip[id]);
        let skip = if forced_skip {
            true
        } else {
            crate::reference::av1::block::read_skip_with_cdf(
                decoder,
                cdfs,
                above_skip,
                left_skip,
                self.config.disable_cdf_update,
            )
            .map_err(TileEntropyError::Cdf)?
        };
        let segment_id = match segment_before {
            Some(id) => id,
            None => self
                .segments
                .read_intra_segment_id(
                    decoder,
                    cdfs,
                    block.row,
                    block.col,
                    bw4,
                    bh4,
                    skip,
                    self.config.segmentation_enabled,
                    self.config.disable_cdf_update,
                )
                .map_err(TileEntropyError::Cdf)?,
        };
        let lossless = self.config.segment_lossless[segment_id];

        self.cdef
            .read(
                decoder,
                block.row,
                block.col,
                block.size,
                skip,
                lossless,
                self.config.cdef_enabled,
                false,
                self.config.cdef_bits,
            )
            .map_err(TileEntropyError::Symbol)?;

        let skip_sb = skip && block.size == crate::reference::av1::partition::BLOCK_SIZES[12];
        self.current_qindex = read_delta_qindex(
            decoder,
            cdfs,
            self.current_qindex,
            self.config.delta_q_present,
            self.read_deltas_for_sb,
            self.config.delta_q_res,
            skip_sb,
            self.config.disable_cdf_update,
        )
        .map_err(Self::delta_as_tile)?;
        self.delta_lf = read_delta_lf(
            decoder,
            cdfs,
            self.delta_lf,
            self.config.delta_lf_present,
            self.config.delta_lf_multi,
            self.read_deltas_for_sb,
            self.config.delta_lf_res,
            skip_sb,
            self.config.disable_cdf_update,
            self.config.has_chroma,
        )
        .map_err(Self::delta_as_tile)?;
        self.read_deltas_for_sb = false;
        let block_qindex = (i32::from(self.current_qindex)
            + if self.config.segmentation_enabled {
                self.config.segment_alt_q[segment_id]
            } else {
                0
            })
        .clamp(0, 255) as u8;

        let use_intrabc = if self.config.allow_intrabc {
            cdfs.read_symbol(
                decoder,
                &crate::reference::av1::cdf_defaults::DEFAULT_INTRABC_CDF,
                0,
                3,
                self.config.disable_cdf_update,
            )? != 0
        } else {
            false
        };
        if use_intrabc {
            let reference = self.intrabc_dv_reference(block)?;
            if !reference.is_integer() {
                return Err(TileEntropyError::Unsupported(
                    "non-integer IntraBC reference vector",
                ));
            }
            let dv = read_mv(
                decoder,
                &mut cdfs.dv,
                reference,
                false,
                true,
                self.config.disable_cdf_update,
            )
            .map_err(Self::inter_as_tile)?;
            if !self.intrabc_dv_valid(block, dv) {
                return Err(TileEntropyError::Unsupported("invalid IntraBC vector"));
            }
            let (above_width, left_height) = self.tx_depth_extents(block);
            let variable_tx =
                !lossless && !skip && self.config.tx_mode_select && !block.size.is_4x4();
            let (tx, luma_tx_blocks) = if variable_tx {
                (
                    max_tx_size_rect(block.size),
                    self.read_inter_tx_sizes(decoder, cdfs, block)?,
                )
            } else {
                (
                    read_tx_size_with_cdf(
                        decoder,
                        cdfs,
                        block.size,
                        false,
                        lossless,
                        self.config.tx_mode_select,
                        above_width,
                        left_height,
                        self.config.disable_cdf_update,
                    )?,
                    Vec::new(),
                )
            };
            if skip {
                self.clear_skip_context(block, has_chroma)?;
            }
            let transforms = if skip {
                Vec::new()
            } else {
                self.decode_intrabc_transforms(
                    decoder,
                    cdfs,
                    block,
                    tx,
                    has_chroma,
                    lossless,
                    luma_tx_blocks,
                )?
            };
            self.record_block(block, skip, IntraMode::Dc, Some(UvMode::Dc), tx, Some(dv));
            self.blocks.push(IntraSyntaxBlock {
                block,
                segment_id,
                skip,
                lossless,
                qindex: block_qindex,
                delta_lf: self.delta_lf,
                y_mode: IntraMode::Dc,
                uv_mode: Some(UvMode::Dc),
                cfl_alphas: CflAlphas::default(),
                angle_y: 0,
                angle_uv: 0,
                filter_intra: None,
                palette: PaletteModeInfo::default(),
                palette_tokens: PaletteTokens { y: None, uv: None },
                tx,
                intrabc_dv: Some(dv),
                transforms,
            });
            return Ok(());
        }

        let y_mode = if self.config.key_frame {
            read_intra_frame_y_mode_with_cdf(
                decoder,
                cdfs,
                above_mode,
                left_mode,
                self.config.disable_cdf_update,
            )
            .map_err(TileEntropyError::Cdf)?
        } else {
            read_y_mode_with_cdf(decoder, cdfs, block.size, self.config.disable_cdf_update)
                .map_err(TileEntropyError::Cdf)?
        };
        let angle_y = read_angle_delta_with_cdf(
            decoder,
            cdfs,
            y_mode,
            block.size,
            self.config.disable_cdf_update,
        )
        .map_err(TileEntropyError::Cdf)?;
        let uv_mode = if has_chroma {
            Some(
                read_uv_mode_with_cdf(
                    decoder,
                    cdfs,
                    y_mode,
                    cfl_allowed(block.size, lossless),
                    self.config.disable_cdf_update,
                )
                .map_err(TileEntropyError::Cdf)?,
            )
        } else {
            None
        };
        let cfl_alphas = if uv_mode == Some(UvMode::Cfl) {
            read_cfl_alphas(decoder, cdfs, self.config.disable_cdf_update)
                .map_err(TileEntropyError::Cdf)?
        } else {
            CflAlphas::default()
        };
        let angle_uv = match uv_mode {
            Some(UvMode::Cfl) => 0,
            Some(UvMode::Dc) => read_angle_delta_with_cdf(
                decoder,
                cdfs,
                IntraMode::Dc,
                block.size,
                self.config.disable_cdf_update,
            )
            .map_err(TileEntropyError::Cdf)?,
            Some(mode) => {
                let mapped = match mode {
                    UvMode::V => IntraMode::V,
                    UvMode::H => IntraMode::H,
                    UvMode::D45 => IntraMode::D45,
                    UvMode::D135 => IntraMode::D135,
                    UvMode::D113 => IntraMode::D113,
                    UvMode::D157 => IntraMode::D157,
                    UvMode::D203 => IntraMode::D203,
                    UvMode::D67 => IntraMode::D67,
                    UvMode::Smooth => IntraMode::Smooth,
                    UvMode::SmoothV => IntraMode::SmoothV,
                    UvMode::SmoothH => IntraMode::SmoothH,
                    UvMode::Paeth => IntraMode::Paeth,
                    UvMode::Dc | UvMode::Cfl => unreachable!(),
                };
                read_angle_delta_with_cdf(
                    decoder,
                    cdfs,
                    mapped,
                    block.size,
                    self.config.disable_cdf_update,
                )
                .map_err(TileEntropyError::Cdf)?
            }
            None => 0,
        };

        let palette = self
            .palettes
            .read_mode_info(
                decoder,
                cdfs,
                block.row,
                block.col,
                block.size,
                y_mode,
                uv_mode,
                has_chroma,
                self.config.allow_screen_content_tools,
                self.config.disable_cdf_update,
                self.config.bit_depth,
            )
            .map_err(Self::palette_as_tile)?;
        let filter = read_filter_intra(
            decoder,
            cdfs,
            block.size,
            y_mode,
            palette.y_size,
            self.config.enable_filter_intra,
            self.config.disable_cdf_update,
        )
        .map_err(TileEntropyError::Cdf)?;
        let palette_tokens = read_palette_tokens(
            decoder,
            cdfs,
            palette,
            block.row,
            block.col,
            mi_rows,
            mi_cols,
            block.size,
            self.config.sub_x,
            self.config.sub_y,
            self.config.disable_cdf_update,
        )
        .map_err(Self::palette_as_tile)?;
        let (above_width, left_height) = self.tx_depth_extents(block);
        let tx = read_tx_size_with_cdf(
            decoder,
            cdfs,
            block.size,
            true,
            lossless,
            self.config.tx_mode_select,
            above_width,
            left_height,
            self.config.disable_cdf_update,
        )
        .map_err(TileEntropyError::Cdf)?;
        for r in block.row..(block.row + bh4).min(mi_rows) {
            for c in block.col..(block.col + bw4).min(mi_cols) {
                self.tx_grid[r * mi_cols + c] = Some(tx);
            }
        }
        if skip {
            self.clear_skip_context(block, has_chroma)?;
        }

        let transforms = if skip {
            Vec::new()
        } else {
            self.decode_transforms(
                decoder,
                cdfs,
                block,
                tx,
                block_qindex,
                y_mode,
                uv_mode,
                filter.mode,
                has_chroma,
                lossless,
            )?
        };
        self.record_block(block, skip, y_mode, uv_mode, tx, None);
        self.blocks.push(IntraSyntaxBlock {
            block,
            segment_id,
            skip,
            lossless,
            qindex: block_qindex,
            delta_lf: self.delta_lf,
            y_mode,
            uv_mode,
            cfl_alphas,
            angle_y,
            angle_uv,
            filter_intra: filter.mode,
            palette,
            palette_tokens,
            tx,
            intrabc_dv: None,
            transforms,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::partition::BLOCK_SIZES;
    use crate::reference::av1::symbol::SymbolDecoder;

    fn config() -> IntraTileConfig {
        IntraTileConfig {
            mi_cols: 32,
            mi_rows: 32,
            tile_row_start: 0,
            tile_col_start: 0,
            tile_row_end: 32,
            tile_col_end: 32,
            sub_x: 1,
            sub_y: 1,
            bit_depth: 8,
            key_frame: true,
            segmentation_enabled: false,
            segmentation_update_map: false,
            seg_id_pre_skip: false,
            last_active_seg_id: 0,
            segment_alt_q: [0; 8],
            segment_lossless: [false; 8],
            segment_skip: [false; 8],
            previous_segment_map: None,
            allow_screen_content_tools: false,
            enable_filter_intra: false,
            enable_intra_edge_filter: false,
            disable_cdf_update: false,
            tx_mode_select: false,
            reduced_tx_set: false,
            delta_q_present: false,
            delta_q_res: 0,
            delta_lf_present: false,
            delta_lf_res: 0,
            delta_lf_multi: false,
            has_chroma: true,
            cdef_enabled: false,
            cdef_bits: 0,
            base_q_index: 64,
            superblock_mi: 16,
            allow_intrabc: false,
            lr_geometry: vec![LrPlaneGeometry::new(
                128,
                128,
                64,
                0,
                0,
                crate::reference::av1::restoration::RestorationType::None,
            )],
            superres_denom: None,
        }
    }

    #[test]
    fn sink_consumes_one_intra_block_and_records_transforms() {
        let mut sink = IntraTileBlockSink::new(config());
        let mut cdfs = CdfState::new();
        let mut decoder = SymbolDecoder::init(&[0u8; 4096]).unwrap();
        sink.decode_block(
            DecodedBlock {
                row: 0,
                col: 0,
                size: BLOCK_SIZES[3],
            },
            &mut decoder,
            &mut cdfs,
        )
        .unwrap();
        assert_eq!(sink.blocks.len(), 1);
        assert_eq!(sink.blocks[0].segment_id, 0);
        assert!(!sink.blocks[0].transforms.is_empty());
    }

    #[test]
    fn real_96_keyframe_tile_reaches_entropy_exit() {
        use crate::reference::av1::obu::parse_ivf;
        use crate::reference::av1::stream::{ReferenceStream, StreamEvent};

        let data = include_bytes!("../../../tests/data/av1_320x240_10.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let packets = stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        let frame = packets
            .iter()
            .flatten()
            .find_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .unwrap();
        let config = IntraTileConfig::from_headers(sequence, &frame.header);
        let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
        let tile = &frame.layout.tiles[0];
        let result = crate::reference::av1::tile_entropy::decode_tile_partitions(
            &frame.payload[tile.bytes.clone()],
            &frame.header,
            tile.tile,
            sb_size,
            false,
            true,
            IntraTileBlockSink::new(config),
        )
        .unwrap();
        assert!(!result.sink.blocks.is_empty());
    }
}
