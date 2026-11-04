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
use crate::reference::av1::intra::{
    cfl_allowed, read_angle_delta_with_cdf, read_intra_frame_y_mode_with_cdf,
    read_uv_mode_with_cdf, read_y_mode_with_cdf, IntraMode, UvMode,
};
use crate::reference::av1::lr_frame::{LrFrameParams, LrPlaneGeometry};
use crate::reference::av1::lr_syntax::{LrSyntaxError, LrTileState};
use crate::reference::av1::palette::{
    read_palette_tokens, PaletteError, PaletteModeInfo, PaletteState, PaletteTokens,
};
use crate::reference::av1::residual::{intra_residual_blocks, residual_size};
use crate::reference::av1::segment::SegmentState;
use crate::reference::av1::seq_header::SequenceHeader;
use crate::reference::av1::tile::DecodedBlock;
use crate::reference::av1::tile_entropy::{TileBlockSink, TileEntropyError};
use crate::reference::av1::tx::{read_tx_size_with_cdf, TxSize};
use crate::reference::av1::tx_type::{
    is_tx_type_in_set, read_intra_tx_type_with_cdf, tx_set, uv_mode_tx_type, TxType,
};

#[derive(Debug, Clone)]
pub struct IntraTileConfig {
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub sub_x: u32,
    pub sub_y: u32,
    pub bit_depth: u32,
    pub key_frame: bool,
    pub segmentation_enabled: bool,
    pub seg_id_pre_skip: bool,
    pub last_active_seg_id: usize,
    pub segment_lossless: [bool; 8],
    pub segment_skip: [bool; 8],
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
    current_qindex: u8,
    coeff_q_ctx: usize,
    delta_lf: [i32; 4],
    read_deltas_for_sb: bool,
    current_sb: Option<(usize, usize)>,
    lr: LrTileState,
    pub blocks: Vec<IntraSyntaxBlock>,
}

impl IntraTileBlockSink {
    pub fn new(config: IntraTileConfig) -> Self {
        let mi_cols = config.mi_cols;
        let mi_rows = config.mi_rows;
        let current_qindex = config.base_q_index;
        let lr = LrTileState::new(config.lr_geometry.clone());
        Self {
            config,
            tile_context: TileContext::new(mi_cols, mi_rows),
            segments: SegmentState::new(mi_cols, mi_rows, 0),
            palettes: PaletteState::new(mi_cols, mi_rows),
            cdef: CdefIndexGrid::new(mi_cols, mi_rows),
            skip_grid: vec![false; mi_cols * mi_rows],
            y_modes: vec![None; mi_cols * mi_rows],
            uv_modes: vec![None; mi_cols * mi_rows],
            tx_grid: vec![None; mi_cols * mi_rows],
            current_qindex,
            coeff_q_ctx: coeff_q_context(current_qindex),
            delta_lf: [0; 4],
            read_deltas_for_sb: false,
            current_sb: None,
            lr,
            blocks: Vec::new(),
        }
    }

    pub fn cdef_grid(&self) -> &CdefIndexGrid {
        &self.cdef
    }

    pub fn lr_params(&self) -> &LrFrameParams {
        &self.lr.params
    }

    fn neighbor<T: Copy>(grid: &[T], mi_cols: usize, row: isize, col: isize) -> Option<T> {
        if row < 0 || col < 0 {
            return None;
        }
        grid.get(row as usize * mi_cols + col as usize).copied()
    }

    fn has_chroma(&self, block: DecodedBlock) -> bool {
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
            DeltaError::MultiDeltaLfUnsupported => TileEntropyError::Unsupported("delta_lf_multi"),
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
    pub fn from_headers(seq: &SequenceHeader, header: &FrameHeader) -> Self {
        let mut segment_skip = [false; 8];
        for (segment, enabled) in segment_skip.iter_mut().enumerate() {
            *enabled = header.feature_enabled[segment][6];
        }
        Self {
            mi_cols: header.mi_cols,
            mi_rows: header.mi_rows,
            sub_x: u32::from(seq.color.subsampling_x),
            sub_y: u32::from(seq.color.subsampling_y),
            bit_depth: u32::from(seq.color.bit_depth),
            key_frame: header.frame_is_intra,
            segmentation_enabled: header.segmentation_enabled,
            seg_id_pre_skip: header.seg_id_pre_skip,
            last_active_seg_id: header.last_active_seg_id as usize,
            segment_lossless: header.lossless_array,
            segment_skip,
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
        _cdfs: &mut CdfState,
    ) -> Result<(), TileEntropyError> {
        self.lr
            .read_lr(
                decoder,
                block.row,
                block.col,
                block.size.num_4x4_wide() as usize,
                block.size.num_4x4_high() as usize,
                self.config.allow_intrabc,
                self.config.superres_denom,
            )
            .map_err(Self::lr_as_tile)
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
        let delta = read_delta_lf(
            decoder,
            cdfs,
            self.delta_lf[0],
            self.config.delta_lf_present,
            self.config.delta_lf_multi,
            self.read_deltas_for_sb,
            self.config.delta_lf_res,
            skip_sb,
            self.config.disable_cdf_update,
        )
        .map_err(Self::delta_as_tile)?;
        self.delta_lf[0] = delta;
        self.read_deltas_for_sb = false;

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

        let above_width = Self::tx_extent(
            &self.tx_grid,
            mi_cols,
            block.row as isize - 1,
            block.col as isize,
            true,
        );
        let left_height = Self::tx_extent(
            &self.tx_grid,
            mi_cols,
            block.row as isize,
            block.col as isize - 1,
            false,
        );
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

        let mut transforms = Vec::new();
        if !skip {
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
                let q_ctx = self.coeff_q_ctx;
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
                    let dir = filter.mode.map(filter_intra_direction).unwrap_or(y_mode) as usize;
                    read_intra_tx_type_with_cdf(
                        decoder,
                        cdfs,
                        tx_block.tx,
                        self.current_qindex,
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
        }

        for r in block.row..(block.row + bh4).min(mi_rows) {
            for c in block.col..(block.col + bw4).min(mi_cols) {
                self.skip_grid[r * mi_cols + c] = skip;
                self.y_modes[r * mi_cols + c] = Some(y_mode);
                self.uv_modes[r * mi_cols + c] = uv_mode;
            }
        }
        self.blocks.push(IntraSyntaxBlock {
            block,
            segment_id,
            skip,
            lossless,
            qindex: self.current_qindex,
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
            sub_x: 1,
            sub_y: 1,
            bit_depth: 8,
            key_frame: true,
            segmentation_enabled: false,
            seg_id_pre_skip: false,
            last_active_seg_id: 0,
            segment_lossless: [false; 8],
            segment_skip: [false; 8],
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
