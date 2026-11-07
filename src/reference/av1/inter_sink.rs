//! Inter-frame tile sink for single references and average compound blocks.
//!
//! The sink consumes the complete mode/reference/MV syntax and records the
//! motion state that later blocks use for MV prediction. Average and
//! difference/distance/wedge compound, inter-intra, fractional motion,
//! residuals, mixed intra/inter blocks, temporal segmentation and inter
//! post-filters are wired through the reconstruction path. Local/global warp
//! and OBMC are decoded.

use crate::reference::av1::cdef::CdefIndexGrid;
use crate::reference::av1::cdf_defaults::{
    DEFAULT_COMPOUND_IDX_CDF, DEFAULT_COMP_GROUP_IDX_CDF, DEFAULT_INTERP_FILTER_CDF,
    DEFAULT_SKIP_MODE_CDF, DEFAULT_TXFM_SPLIT_CDF,
};
use crate::reference::av1::cdf_state::CdfState;
use crate::reference::av1::cfl::{read_cfl_alphas, CflAlphas};
use crate::reference::av1::coeff::{coeff_q_context, read_all_zero_with_cdf, seg_eob};
use crate::reference::av1::coeff_block::decode_coefficient_payload;
use crate::reference::av1::context::TileContext;
use crate::reference::av1::delta::{read_delta_lf, read_delta_qindex, DeltaError};
use crate::reference::av1::filter_intra::{filter_intra_direction, read_filter_intra};
use crate::reference::av1::frame_header::{
    get_relative_dist, FrameHeader, SEG_LVL_ALT_Q, SEG_LVL_GLOBALMV, SEG_LVL_REF_FRAME,
    SEG_LVL_SKIP,
};
use crate::reference::av1::inter::{
    assign_compound_mv, assign_mv, find_mv_stack, global_motion_vector_from_params,
    interintra_allowed, motion_mode_allowed, read_compound_mode, read_compound_ref_mv_index,
    read_compound_type, read_inter_mode, read_interintra, read_is_inter, read_motion_mode,
    read_ref_frames, read_ref_mv_index, CompoundMode, CompoundType, InterBlock, InterError,
    InterNeighbors, MotionGrid, MotionModeAllowed, MotionVector, ALTREF_FRAME, INTRA_FRAME,
    LAST_FRAME, NONE_FRAME,
};
use crate::reference::av1::intra::{
    cfl_allowed, read_angle_delta_with_cdf, read_uv_mode_with_cdf, read_y_mode_with_cdf, IntraMode,
    UvMode,
};
use crate::reference::av1::intra_sink::{IntraSyntaxBlock, IntraSyntaxTx};
use crate::reference::av1::lr_frame::{LrFrameParams, LrPlaneGeometry};
use crate::reference::av1::lr_syntax::{LrSyntaxError, LrTileState};
use crate::reference::av1::palette::{read_palette_tokens, PaletteError, PaletteState};
use crate::reference::av1::partition::BlockSize;
use crate::reference::av1::residual::{intra_residual_blocks, residual_size, ResidualTxBlock};
use crate::reference::av1::segment::{SegmentCarry, SegmentMap, SegmentState};
use crate::reference::av1::seq_header::SequenceHeader;
use crate::reference::av1::temporal::TemporalMotionField;
use crate::reference::av1::tile::DecodedBlock;
use crate::reference::av1::tile_entropy::{TileBlockSink, TileEntropyError};

const MAX_FRAME_DISTANCE: i32 = 31;
const QUANT_DIST_WEIGHT: [[i32; 2]; 4] = [[2, 3], [2, 5], [2, 7], [1, MAX_FRAME_DISTANCE]];
const QUANT_DIST_LOOKUP: [[u8; 2]; 4] = [[9, 7], [11, 5], [12, 4], [13, 3]];
use crate::reference::av1::tx::{max_tx_size_rect, read_tx_size_with_cdf, split_tx_size, TxSize};
use crate::reference::av1::tx_type::{
    is_tx_type_in_set, read_inter_tx_type_with_cdf, read_intra_tx_type_with_cdf, tx_set, TxType,
};
use crate::reference::av1::warped::{
    find_projection, select_samples, ProjectionSample, WarpParams,
};

#[derive(Debug, Clone)]
pub struct InterTileConfig {
    pub mi_cols: usize,
    /// mi origin of the tile currently being decoded. `set_mi_row_col` clips
    /// every neighbour-context read at the tile boundary rather than the
    /// picture boundary (`up_available = mi_row > tile->mi_row_start`), while
    /// the MV stack scans keep reading the frame-wide mi state.
    pub tile_row_start: usize,
    pub tile_row_end: usize,
    pub tile_col_start: usize,
    pub tile_col_end: usize,
    pub mi_rows: usize,
    pub sub_x: u32,
    pub sub_y: u32,
    pub bit_depth: u32,
    pub segmentation_enabled: bool,
    pub segmentation_update_map: bool,
    pub segmentation_temporal_update: bool,
    pub seg_id_pre_skip: bool,
    pub last_active_seg_id: usize,
    pub segment_alt_q: [i32; 8],
    pub segment_lossless: [bool; 8],
    pub segment_skip: [bool; 8],
    pub segment_ref_frame: [Option<i8>; 8],
    pub segment_globalmv: [bool; 8],
    pub previous_segment_map: Option<SegmentMap>,
    pub disable_cdf_update: bool,
    pub delta_q_present: bool,
    pub delta_q_res: u8,
    pub delta_lf_present: bool,
    pub delta_lf_res: u8,
    pub delta_lf_multi: bool,
    pub tx_mode_select: bool,
    pub reduced_tx_set: bool,
    pub has_chroma: bool,
    pub cdef_enabled: bool,
    pub cdef_bits: u8,
    pub base_q_index: u8,
    pub superblock_mi: usize,
    pub allow_intrabc: bool,
    pub allow_screen_content_tools: bool,
    pub enable_filter_intra: bool,
    pub enable_intra_edge_filter: bool,
    pub lr_geometry: Vec<LrPlaneGeometry>,
    pub superres_denom: Option<u32>,
    pub reference_select: bool,
    pub skip_mode_present: bool,
    pub skip_mode_frame: [u8; 2],
    pub allow_high_precision_mv: bool,
    pub force_integer_mv: bool,
    pub use_ref_frame_mvs: bool,
    pub temporal_motion: Option<TemporalMotionField>,
    pub is_filter_switchable: bool,
    pub interpolation_filter: u8,
    pub enable_dual_filter: bool,
    pub is_motion_mode_switchable: bool,
    pub allow_warped_motion: bool,
    pub enable_interintra_compound: bool,
    pub enable_masked_compound: bool,
    pub enable_order_hint: bool,
    pub enable_dist_wtd_comp: bool,
    pub order_hint_bits: u8,
    pub current_order_hint: u8,
    pub reference_order_hints: [u8; 8],
    pub ref_frame_idx: [u8; 7],
    pub gm_types: [u8; 7],
    pub gm_params: [[i32; 6]; 7],
}

impl InterTileConfig {
    pub fn from_headers(sequence: &SequenceHeader, header: &FrameHeader) -> Self {
        let mut segment_skip = [false; 8];
        let mut segment_ref_frame = [None; 8];
        let mut segment_globalmv = [false; 8];
        let mut segment_alt_q = [0; 8];
        for segment in 0..8 {
            if header.feature_enabled[segment][SEG_LVL_ALT_Q] {
                segment_alt_q[segment] = header.feature_data[segment][SEG_LVL_ALT_Q];
            }
            segment_skip[segment] = header.feature_enabled[segment][SEG_LVL_SKIP];
            if header.feature_enabled[segment][SEG_LVL_REF_FRAME] {
                segment_ref_frame[segment] =
                    Some(header.feature_data[segment][SEG_LVL_REF_FRAME] as i8);
            }
            segment_globalmv[segment] = header.feature_enabled[segment][SEG_LVL_GLOBALMV];
        }
        Self {
            mi_cols: header.mi_cols,
            mi_rows: header.mi_rows,
            sub_x: u32::from(sequence.color.subsampling_x),
            sub_y: u32::from(sequence.color.subsampling_y),
            bit_depth: u32::from(sequence.color.bit_depth),
            segmentation_enabled: header.segmentation_enabled,
            segmentation_update_map: header.segmentation_update_map,
            segmentation_temporal_update: header.segmentation_temporal_update,
            seg_id_pre_skip: header.seg_id_pre_skip,
            last_active_seg_id: usize::from(header.last_active_seg_id),
            tile_row_start: 0,
            tile_row_end: header.mi_rows,
            tile_col_start: 0,
            tile_col_end: header.mi_cols,
            segment_alt_q,
            segment_lossless: header.lossless_array,
            segment_skip,
            segment_ref_frame,
            segment_globalmv,
            previous_segment_map: None,
            disable_cdf_update: header.disable_cdf_update,
            delta_q_present: header.delta_q_present,
            delta_q_res: header.delta_q_res,
            delta_lf_present: header.delta_lf_present,
            delta_lf_res: header.delta_lf_res,
            delta_lf_multi: header.delta_lf_multi,
            tx_mode_select: header.tx_mode == 2,
            reduced_tx_set: header.reduced_tx_set,
            has_chroma: !sequence.color.mono_chrome,
            cdef_enabled: sequence.enable_cdef,
            cdef_bits: header.cdef_bits,
            base_q_index: header.base_q_idx,
            superblock_mi: if sequence.use_128x128_superblock {
                32
            } else {
                16
            },
            allow_intrabc: header.allow_intrabc,
            allow_screen_content_tools: header.allow_screen_content_tools != 0,
            enable_filter_intra: sequence.enable_filter_intra,
            enable_intra_edge_filter: sequence.enable_intra_edge_filter,
            lr_geometry: (0..sequence.color.num_planes)
                .map(|plane| {
                    if header.uses_lr {
                        header.lr_geometry(plane, sequence)
                    } else {
                        let (sub_x, sub_y) = if plane == 0 {
                            (0, 0)
                        } else {
                            (
                                u32::from(sequence.color.subsampling_x),
                                u32::from(sequence.color.subsampling_y),
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
            reference_select: header.reference_select,
            skip_mode_present: header.skip_mode_present,
            skip_mode_frame: header.skip_mode_frame,
            allow_high_precision_mv: header.allow_high_precision_mv,
            force_integer_mv: header.force_integer_mv != 0,
            use_ref_frame_mvs: header.use_ref_frame_mvs,
            temporal_motion: None,
            is_filter_switchable: header.is_filter_switchable,
            interpolation_filter: header.interpolation_filter,
            enable_dual_filter: sequence.enable_dual_filter,
            is_motion_mode_switchable: header.is_motion_mode_switchable,
            allow_warped_motion: header.allow_warped_motion,
            enable_interintra_compound: sequence.enable_interintra_compound,
            enable_masked_compound: sequence.enable_masked_compound,
            enable_order_hint: sequence.enable_order_hint,
            enable_dist_wtd_comp: sequence.enable_jnt_comp,
            order_hint_bits: sequence.order_hint_bits,
            current_order_hint: header.order_hint,
            reference_order_hints: [0; 8],
            ref_frame_idx: header.ref_frame_idx,
            gm_types: header.gm_types,
            gm_params: header.gm_params,
        }
    }

    pub fn with_reference_order_hints(mut self, hints: [u8; 8]) -> Self {
        self.reference_order_hints = hints;
        self
    }

    pub fn with_temporal_motion(mut self, motion: TemporalMotionField) -> Self {
        self.temporal_motion = Some(motion);
        self
    }

    pub fn with_previous_segment_map(mut self, map: Option<SegmentMap>) -> Self {
        self.previous_segment_map = map;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterSyntaxBlock {
    pub block: DecodedBlock,
    pub segment_id: usize,
    pub skip: bool,
    pub qindex: u8,
    pub delta_lf: [i32; 4],
    pub inter: Option<InterBlock>,
    pub intra: Option<IntraSyntaxBlock>,
    pub lossless: bool,
    pub tx: TxSize,
    pub transforms: Vec<crate::reference::av1::intra_sink::IntraSyntaxTx>,
}

pub struct InterTileBlockSink {
    config: InterTileConfig,
    cdef: CdefIndexGrid,
    skip_grid: Vec<bool>,
    skip_modes: Vec<bool>,
    decoded: Vec<bool>,
    sizes: Vec<Option<BlockSize>>,
    is_inter: Vec<bool>,
    refs: Vec<[i8; 2]>,
    modes: Vec<Option<crate::reference::av1::inter::InterMode>>,
    compound_modes: Vec<Option<CompoundMode>>,
    compound_groups: Vec<u8>,
    compound_indices: Vec<u8>,
    mvs: Vec<[MotionVector; 2]>,
    y_modes: Vec<Option<IntraMode>>,
    uv_modes: Vec<Option<UvMode>>,
    interp_filters: Vec<[u8; 2]>,
    tile_context: TileContext,
    palettes: PaletteState,
    segments: SegmentState,
    tx_grid: Vec<Option<TxSize>>,
    tx_type_grid: Vec<Option<TxType>>,
    current_qindex: u8,
    delta_lf: [i32; 4],
    read_deltas_for_sb: bool,
    current_sb: Option<(usize, usize)>,
    lr: LrTileState,
    lr_cdfs_loaded: bool,
    pub blocks: Vec<InterSyntaxBlock>,
}

/// The neighbour state libaom keeps in the frame-wide mi grid.
///
/// A tile sees the blocks an earlier tile in decode order wrote and cannot see
/// the blocks a later tile has not reached, which is libaom's `mv_proj` poc
/// check reading a freshly zeroed mi grid. Everything that is genuinely
/// tile-local in libaom stays out of this struct: the coefficient and partition
/// above/left contexts (`av1_zero(td->cb_buffer_base.dqcoeff)` per tile), the
/// palette neighbourhood, the CDEF transmission flags, and the per-tile
/// `current_base_qindex` reset.
#[derive(Debug, Clone)]
pub struct InterNeighbourState {
    decoded: Vec<bool>,
    sizes: Vec<Option<BlockSize>>,
    is_inter: Vec<bool>,
    refs: Vec<[i8; 2]>,
    modes: Vec<Option<crate::reference::av1::inter::InterMode>>,
    compound_modes: Vec<Option<CompoundMode>>,
    compound_groups: Vec<u8>,
    compound_indices: Vec<u8>,
    mvs: Vec<[MotionVector; 2]>,
    y_modes: Vec<Option<IntraMode>>,
    uv_modes: Vec<Option<UvMode>>,
    interp_filters: Vec<[u8; 2]>,
    skip_grid: Vec<bool>,
    skip_modes: Vec<bool>,
    tx_grid: Vec<Option<TxSize>>,
    tx_type_grid: Vec<Option<TxType>>,
    segments: SegmentCarry,
}

impl InterTileBlockSink {
    /// Take the frame-level neighbour state at a tile boundary.
    pub fn neighbour_state(&self) -> InterNeighbourState {
        InterNeighbourState {
            decoded: self.decoded.clone(),
            sizes: self.sizes.clone(),
            is_inter: self.is_inter.clone(),
            refs: self.refs.clone(),
            modes: self.modes.clone(),
            compound_modes: self.compound_modes.clone(),
            compound_groups: self.compound_groups.clone(),
            compound_indices: self.compound_indices.clone(),
            mvs: self.mvs.clone(),
            y_modes: self.y_modes.clone(),
            uv_modes: self.uv_modes.clone(),
            interp_filters: self.interp_filters.clone(),
            skip_grid: self.skip_grid.clone(),
            skip_modes: self.skip_modes.clone(),
            tx_grid: self.tx_grid.clone(),
            tx_type_grid: self.tx_type_grid.clone(),
            segments: self.segments.carry(),
        }
    }

    /// Continue a fresh sink from the neighbour state of the previous tile.
    #[must_use]
    pub fn with_neighbour_state(mut self, state: InterNeighbourState) -> Self {
        if state.decoded.len() == self.decoded.len() {
            self.decoded = state.decoded;
            self.sizes = state.sizes;
            self.is_inter = state.is_inter;
            self.refs = state.refs;
            self.modes = state.modes;
            self.compound_modes = state.compound_modes;
            self.compound_groups = state.compound_groups;
            self.compound_indices = state.compound_indices;
            self.mvs = state.mvs;
            self.y_modes = state.y_modes;
            self.uv_modes = state.uv_modes;
            self.interp_filters = state.interp_filters;
            self.skip_grid = state.skip_grid;
            self.skip_modes = state.skip_modes;
            self.tx_grid = state.tx_grid;
            self.tx_type_grid = state.tx_type_grid;
            self.segments.restore_carry(&state.segments);
        }
        self
    }
}

impl InterTileConfig {
    /// Point the neighbour-context clipping at one tile.
    #[must_use]
    pub fn with_tile_origin(
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
}

impl InterTileBlockSink {
    pub fn new(config: InterTileConfig) -> Self {
        let mi_cols = config.mi_cols;
        let mi_rows = config.mi_rows;
        let (tile_row_start, tile_col_start) = (config.tile_row_start, config.tile_col_start);
        let current_qindex = config.base_q_index;
        let previous_segment_map = config.previous_segment_map.clone();
        let last_active_seg_id = config.last_active_seg_id;
        let lr = LrTileState::new(config.lr_geometry.clone());
        Self {
            config,
            cdef: CdefIndexGrid::new(mi_cols, mi_rows),
            skip_grid: vec![false; mi_cols * mi_rows],
            skip_modes: vec![false; mi_cols * mi_rows],
            decoded: vec![false; mi_cols * mi_rows],
            sizes: vec![None; mi_cols * mi_rows],
            is_inter: vec![false; mi_cols * mi_rows],
            refs: vec![[INTRA_FRAME, NONE_FRAME]; mi_cols * mi_rows],
            modes: vec![None; mi_cols * mi_rows],
            compound_modes: vec![None; mi_cols * mi_rows],
            compound_groups: vec![0; mi_cols * mi_rows],
            compound_indices: vec![1; mi_cols * mi_rows],
            mvs: vec![[MotionVector::default(); 2]; mi_cols * mi_rows],
            y_modes: vec![None; mi_cols * mi_rows],
            uv_modes: vec![None; mi_cols * mi_rows],
            interp_filters: vec![[0, 0]; mi_cols * mi_rows],
            tile_context: TileContext::new(mi_cols, mi_rows),
            palettes: PaletteState::new(mi_cols, mi_rows),
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
            tx_grid: vec![None; mi_cols * mi_rows],
            tx_type_grid: vec![None; mi_cols * mi_rows],
            current_qindex,
            delta_lf: [0; 4],
            read_deltas_for_sb: false,
            current_sb: None,
            lr,
            lr_cdfs_loaded: false,
            blocks: Vec::new(),
        }
    }

    /// The above-neighbour cell a *context* may read, or `None` at the tile
    /// boundary. `set_mi_row_col` nulls `xd->above_mbmi` when
    /// `mi_row == tile->mi_row_start`, so the tile edge behaves exactly like the
    /// picture edge for every context, including the carried state of a tile
    /// decoded earlier.
    fn above_context_index(&self, row: usize, col: usize) -> Option<usize> {
        (row > self.config.tile_row_start).then(|| Self::at(row - 1, col, self.config.mi_cols))
    }

    /// `left_available` counterpart of [`Self::above_context_index`].
    fn left_context_index(&self, row: usize, col: usize) -> Option<usize> {
        (col > self.config.tile_col_start).then(|| Self::at(row, col - 1, self.config.mi_cols))
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

    fn selected_tx_above_width(&self, block: DecodedBlock) -> u32 {
        let Some(index) = self.above_context_index(block.row, block.col) else {
            return 0;
        };
        if self.is_inter[index] {
            self.sizes[index].map_or(0, |size| size.width)
        } else {
            self.tx_grid[index].map_or(0, TxSize::width)
        }
    }

    fn selected_tx_left_height(&self, block: DecodedBlock) -> u32 {
        let Some(index) = self.left_context_index(block.row, block.col) else {
            return 0;
        };
        if self.is_inter[index] {
            self.sizes[index].map_or(0, |size| size.height)
        } else {
            self.tx_grid[index].map_or(0, TxSize::height)
        }
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
                &DEFAULT_TXFM_SPLIT_CDF,
                context * 3,
                3,
                self.config.disable_cdf_update,
            )
            .map_err(TileEntropyError::Cdf)?
                != 0
        };

        let w4 = (tx.width() / 4) as usize;
        let h4 = (tx.height() / 4) as usize;
        if split {
            let sub_tx = split_tx_size(tx);
            let step_w = (sub_tx.width() / 4) as usize;
            let step_h = (sub_tx.height() / 4) as usize;
            for child_row in (0..h4).step_by(step_h) {
                for child_col in (0..w4).step_by(step_w) {
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
            for r in row..(row + h4).min(self.config.mi_rows) {
                for c in col..(col + w4).min(self.config.mi_cols) {
                    self.tx_grid[Self::at(r, c, self.config.mi_cols)] = Some(tx);
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
        let step_w = (max_tx.width() / 4) as usize;
        let step_h = (max_tx.height() / 4) as usize;
        let width = block.size.num_4x4_wide() as usize;
        let height = block.size.num_4x4_high() as usize;
        let mut blocks = Vec::new();
        for row in (0..height).step_by(step_h) {
            for col in (0..width).step_by(step_w) {
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

    pub fn cdef_grid(&self) -> &CdefIndexGrid {
        &self.cdef
    }

    pub fn lr_params(&self) -> &LrFrameParams {
        &self.lr.params
    }

    pub fn segment_map(&self) -> SegmentMap {
        self.segments.snapshot()
    }

    fn conversion(error: InterError) -> TileEntropyError {
        match error {
            InterError::Cdf(error) => TileEntropyError::Cdf(error),
            InterError::Symbol(error) => TileEntropyError::Symbol(error),
            InterError::Unsupported(reason) => TileEntropyError::Unsupported(reason),
            InterError::InvalidSyntax(reason) => TileEntropyError::Unsupported(reason),
        }
    }

    fn delta_as_tile(error: DeltaError) -> TileEntropyError {
        match error {
            DeltaError::Cdf(error) => TileEntropyError::Cdf(error),
            DeltaError::Symbol(error) => TileEntropyError::Symbol(error),
        }
    }

    fn lr_as_tile(error: LrSyntaxError) -> TileEntropyError {
        match error {
            LrSyntaxError::Symbol(error) => TileEntropyError::Symbol(error),
            _ => TileEntropyError::Unsupported("loop restoration syntax"),
        }
    }

    fn at(row: usize, col: usize, cols: usize) -> usize {
        row * cols + col
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

    fn palette_as_tile(error: PaletteError) -> TileEntropyError {
        match error {
            PaletteError::Cdf(error) => TileEntropyError::Cdf(error),
            PaletteError::Symbol(error) => TileEntropyError::Symbol(error),
            PaletteError::BadPaletteSize(_) => TileEntropyError::Unsupported("palette size"),
            PaletteError::MissingTokens => TileEntropyError::Unsupported("palette tokens"),
            PaletteError::InvalidIndex(_) => TileEntropyError::Unsupported("palette color index"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_intra_syntax(
        &mut self,
        block: DecodedBlock,
        decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        segment_id: usize,
        skip: bool,
        qindex: u8,
        lossless: bool,
        delta_lf: [i32; 4],
    ) -> Result<IntraSyntaxBlock, TileEntropyError> {
        let mi_cols = self.config.mi_cols;
        let mi_rows = self.config.mi_rows;
        let has_chroma = self.has_chroma(block);
        let y_mode =
            read_y_mode_with_cdf(decoder, cdfs, block.size, self.config.disable_cdf_update)
                .map_err(TileEntropyError::Cdf)?;
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
                let mode = match mode {
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
                    mode,
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
        let above_width = self.selected_tx_above_width(block);
        let left_height = self.selected_tx_left_height(block);
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
        let bw4 = block.size.num_4x4_wide() as usize;
        let bh4 = block.size.num_4x4_high() as usize;
        for row in block.row..(block.row + bh4).min(mi_rows) {
            for col in block.col..(block.col + bw4).min(mi_cols) {
                self.tx_grid[Self::at(row, col, mi_cols)] = Some(tx);
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
                let all_zero = read_all_zero_with_cdf(
                    decoder,
                    cdfs,
                    tx_block.tx,
                    coeff_q_context(self.config.base_q_index),
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
                        qindex,
                        dir,
                        self.config.reduced_tx_set,
                        self.config.disable_cdf_update,
                    )
                    .map_err(TileEntropyError::Cdf)?
                } else {
                    let typ = crate::reference::av1::tx_type::uv_mode_tx_type(
                        uv_mode.unwrap_or(UvMode::Dc),
                    );
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
                        coeff_q_context(self.config.base_q_index),
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
        Ok(IntraSyntaxBlock {
            block,
            segment_id,
            skip,
            lossless,
            qindex,
            delta_lf,
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
        })
    }

    fn neighbors(&self, block: DecodedBlock) -> InterNeighbors {
        // `av1_get_reference_mode_context` reads `xd->above_mbmi` and
        // `xd->left_mbmi` under `up_available`/`left_available`, which
        // `set_mi_row_col` gates on the tile, not the picture.
        let above = self
            .above_context_index(block.row, block.col)
            .filter(|&index| self.decoded[index])
            .map(|index| self.refs[index]);
        let left = self
            .left_context_index(block.row, block.col)
            .filter(|&index| self.decoded[index])
            .map(|index| self.refs[index]);
        InterNeighbors { above, left }
    }

    fn overlappable_above(&self, block: DecodedBlock) -> usize {
        if block.row == self.config.tile_row_start {
            return 0;
        }
        let end_col =
            (block.col + block.size.num_4x4_wide() as usize).min(self.config.tile_col_end);
        let mut col = block.col;
        let mut count = 0;
        while col < end_col {
            let mut index = Self::at(block.row - 1, col, self.config.mi_cols);
            let mut step = self.sizes[index]
                .map_or(1, |size| size.num_4x4_wide() as usize)
                .min(16);
            if step == 1 {
                col &= !1;
                if col + 1 >= self.config.mi_cols {
                    break;
                }
                index = Self::at(block.row - 1, col + 1, self.config.mi_cols);
                step = 2;
            }
            if self.decoded[index] && self.is_inter[index] {
                count += 1;
            }
            col += step;
        }
        count
    }

    fn overlappable_left(&self, block: DecodedBlock) -> usize {
        if block.col == self.config.tile_col_start {
            return 0;
        }
        let end_row =
            (block.row + block.size.num_4x4_high() as usize).min(self.config.tile_row_end);
        let mut row = block.row;
        let mut count = 0;
        while row < end_row {
            let mut index = Self::at(row, block.col - 1, self.config.mi_cols);
            let mut step = self.sizes[index]
                .map_or(1, |size| size.num_4x4_high() as usize)
                .min(16);
            if step == 1 {
                row &= !1;
                if row + 1 >= self.config.mi_rows {
                    break;
                }
                index = Self::at(row + 1, block.col - 1, self.config.mi_cols);
                step = 2;
            }
            if self.decoded[index] && self.is_inter[index] {
                count += 1;
            }
            row += step;
        }
        count
    }

    fn projected_ref(&self, index: usize, ref_frame: i8) -> bool {
        self.decoded[index] && self.is_inter[index] && self.refs[index] == [ref_frame, NONE_FRAME]
    }

    fn projection_sample(
        &self,
        index: usize,
        ref_frame: i8,
        row_offset: i32,
        row_sign: i32,
        col_offset: i32,
        col_sign: i32,
    ) -> Option<ProjectionSample> {
        if !self.projected_ref(index, ref_frame) {
            return None;
        }
        let size = self.sizes[index]?;
        let x = col_offset * 4 + col_sign * (size.width as i32 / 2) - 1;
        let y = row_offset * 4 + row_sign * (size.height as i32 / 2) - 1;
        let source = [x * 8, y * 8];
        let mv = self.mvs[index][0];
        Some(ProjectionSample {
            source,
            destination: [source[0] + mv.col, source[1] + mv.row],
        })
    }

    fn projection_samples(&self, block: DecodedBlock, ref_frame: i8) -> Vec<ProjectionSample> {
        let width = block.size.num_4x4_wide() as usize;
        let height = block.size.num_4x4_high() as usize;
        let mut top_left =
            block.row > self.config.tile_row_start && block.col > self.config.tile_col_start;
        let mut top_right =
            block.row > self.config.tile_row_start && block.col + width < self.config.tile_col_end;
        let mut samples = Vec::with_capacity(8);
        let push = |samples: &mut Vec<ProjectionSample>, sample: Option<ProjectionSample>| {
            if samples.len() < 8 {
                if let Some(sample) = sample {
                    samples.push(sample);
                }
            }
        };

        if block.row > self.config.tile_row_start {
            let first = Self::at(block.row - 1, block.col, self.config.mi_cols);
            let above_width = self.sizes[first].map_or(1, |size| size.num_4x4_wide() as usize);
            if width <= above_width {
                let col_offset = -((block.col as i32) % above_width as i32);
                if col_offset < 0 {
                    top_left = false;
                }
                if col_offset + above_width as i32 > width as i32 {
                    top_right = false;
                }
                push(
                    &mut samples,
                    self.projection_sample(first, ref_frame, 0, -1, col_offset, 1),
                );
            } else {
                let mut offset = 0;
                while offset < width.min(self.config.tile_col_end - block.col) {
                    let index = Self::at(block.row - 1, block.col + offset, self.config.mi_cols);
                    push(
                        &mut samples,
                        self.projection_sample(index, ref_frame, 0, -1, offset as i32, 1),
                    );
                    let above_width = self.sizes[index]
                        .map_or(1, |size| size.num_4x4_wide() as usize)
                        .max(1);
                    offset += above_width;
                }
            }
        }

        if block.col > self.config.tile_col_start {
            let first = Self::at(block.row, block.col - 1, self.config.mi_cols);
            let left_height = self.sizes[first].map_or(1, |size| size.num_4x4_high() as usize);
            if height <= left_height {
                let row_offset = -((block.row as i32) % left_height as i32);
                if row_offset < 0 {
                    top_left = false;
                }
                push(
                    &mut samples,
                    self.projection_sample(first, ref_frame, row_offset, 1, 0, -1),
                );
            } else {
                let mut offset = 0;
                while offset < height.min(self.config.tile_row_end - block.row) {
                    let index = Self::at(block.row + offset, block.col - 1, self.config.mi_cols);
                    push(
                        &mut samples,
                        self.projection_sample(index, ref_frame, offset as i32, 1, 0, -1),
                    );
                    let left_height = self.sizes[index]
                        .map_or(1, |size| size.num_4x4_high() as usize)
                        .max(1);
                    offset += left_height;
                }
            }
        }

        if top_left {
            push(
                &mut samples,
                self.projection_sample(
                    Self::at(block.row - 1, block.col - 1, self.config.mi_cols),
                    ref_frame,
                    0,
                    -1,
                    0,
                    -1,
                ),
            );
        }
        if top_right {
            push(
                &mut samples,
                self.projection_sample(
                    Self::at(block.row - 1, block.col + width, self.config.mi_cols),
                    ref_frame,
                    0,
                    -1,
                    width as i32,
                    1,
                ),
            );
        }
        samples
    }

    fn motion_mode_allowed(
        &self,
        block: DecodedBlock,
        mode: crate::reference::av1::inter::InterMode,
        ref_frames: [i8; 2],
    ) -> MotionModeAllowed {
        let above = self.overlappable_above(block);
        let neighbors = if above != 0 {
            above
        } else {
            self.overlappable_left(block)
        };
        motion_mode_allowed(
            block.size,
            mode,
            ref_frames[1],
            neighbors,
            self.config.force_integer_mv,
            self.config.gm_types[(ref_frames[0] - 1) as usize],
            self.projection_samples(block, ref_frames[0]).len(),
            self.config.allow_warped_motion,
            self.config.superres_denom.is_some(),
        )
    }

    fn read_skip_mode(
        &self,
        decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        block: DecodedBlock,
        segment_id: usize,
    ) -> Result<bool, InterError> {
        let segment_disables = self.config.segment_skip[segment_id]
            || self.config.segment_ref_frame[segment_id].is_some()
            || self.config.segment_globalmv[segment_id];
        if segment_disables
            || !self.config.skip_mode_present
            || block.size.width < 8
            || block.size.height < 8
        {
            return Ok(false);
        }
        let mut ctx = 0usize;
        if let Some(index) = self.above_context_index(block.row, block.col) {
            ctx += usize::from(self.skip_modes[index]);
        }
        if let Some(index) = self.left_context_index(block.row, block.col) {
            ctx += usize::from(self.skip_modes[index]);
        }
        Ok(cdfs.read_symbol(
            decoder,
            &DEFAULT_SKIP_MODE_CDF,
            ctx * 3,
            3,
            self.config.disable_cdf_update,
        )? != 0)
    }

    fn read_filter(
        &self,
        decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
        cdfs: &mut CdfState,
        block: DecodedBlock,
        ref_frame: i8,
        ref_frame1: i8,
        mode: crate::reference::av1::inter::InterMode,
        compound_mode: Option<CompoundMode>,
        motion_mode: crate::reference::av1::inter::MotionMode,
        skip_mode: bool,
        neighbors: &InterNeighbors,
    ) -> Result<[u8; 2], InterError> {
        if !self.config.is_filter_switchable {
            let fixed = self.config.interpolation_filter;
            return Ok([fixed, fixed]);
        }

        let large = block.size.width.min(block.size.height) >= 8;
        let needs_filter =
            !skip_mode
                && motion_mode != crate::reference::av1::inter::MotionMode::LocalWarp
                && match compound_mode {
                    Some(CompoundMode::GlobalGlobal) if large => self.config.gm_types
                        [(ref_frame - 1) as usize]
                        == crate::reference::av1::frame_header::GlobalMotionType::Translation as u8
                        || self.config.gm_types[(ref_frame1 - 1) as usize]
                            == crate::reference::av1::frame_header::GlobalMotionType::Translation
                                as u8,
                    Some(_) => true,
                    None => !(large && mode == crate::reference::av1::inter::InterMode::Global)
                        || self.config.gm_types[(ref_frame - 1) as usize]
                            == crate::reference::av1::frame_header::GlobalMotionType::Translation
                                as u8,
                };
        let mut filters = [0u8; 2];
        let directions = if self.config.enable_dual_filter { 2 } else { 1 };
        for dir in 0..directions {
            if needs_filter {
                let mut left = 3usize;
                let mut above = 3usize;
                if let Some(refs) = neighbors.left {
                    if refs[0] == ref_frame || refs[1] == ref_frame {
                        left = self.interp_filters
                            [Self::at(block.row, block.col - 1, self.config.mi_cols)][dir]
                            as usize;
                    }
                }
                if let Some(refs) = neighbors.above {
                    if refs[0] == ref_frame || refs[1] == ref_frame {
                        above = self.interp_filters
                            [Self::at(block.row - 1, block.col, self.config.mi_cols)][dir]
                            as usize;
                    }
                }
                let neighbor_context = if left == above {
                    left
                } else if left == 3 {
                    above
                } else if above == 3 {
                    left
                } else {
                    3
                };
                let context =
                    (dir * 2 + usize::from(compound_mode.is_some())) * 4 + neighbor_context;
                filters[dir] = cdfs.read_symbol(
                    decoder,
                    &DEFAULT_INTERP_FILTER_CDF,
                    context * 4,
                    4,
                    self.config.disable_cdf_update,
                )? as u8;
            }
        }
        if !self.config.enable_dual_filter {
            filters[1] = filters[0];
        }
        Ok(filters)
    }

    fn compound_group_context(&self, block: DecodedBlock, neighbors: &InterNeighbors) -> usize {
        let context = |refs: Option<[i8; 2]>, row: usize, col: usize| {
            let Some(refs) = refs else {
                return 0;
            };
            if refs[1] > INTRA_FRAME {
                self.compound_groups[Self::at(row, col, self.config.mi_cols)] as usize
            } else if refs[0] == ALTREF_FRAME {
                3
            } else {
                0
            }
        };
        let above = if block.row > 0 {
            context(neighbors.above, block.row.saturating_sub(1), block.col)
        } else {
            0
        };
        let left = if block.col > 0 {
            context(neighbors.left, block.row, block.col.saturating_sub(1))
        } else {
            0
        };
        (above + left).min(5)
    }

    fn compound_index_context(
        &self,
        block: DecodedBlock,
        ref_frames: [i8; 2],
        neighbors: &InterNeighbors,
    ) -> usize {
        let bck_hint = self.config.reference_order_hints
            [self.config.ref_frame_idx[(ref_frames[0] - 1) as usize] as usize];
        let fwd_hint = self.config.reference_order_hints
            [self.config.ref_frame_idx[(ref_frames[1] - 1) as usize] as usize];
        let current = i32::from(self.config.current_order_hint);
        let fwd = get_relative_dist(
            i32::from(fwd_hint),
            current,
            self.config.enable_order_hint,
            self.config.order_hint_bits,
        )
        .abs();
        let bck = get_relative_dist(
            current,
            i32::from(bck_hint),
            self.config.enable_order_hint,
            self.config.order_hint_bits,
        )
        .abs();
        let context = |refs: Option<[i8; 2]>, row: usize, col: usize| match refs {
            Some(refs) => {
                if refs[1] > INTRA_FRAME {
                    usize::from(self.compound_indices[Self::at(row, col, self.config.mi_cols)])
                } else if refs[0] == ALTREF_FRAME {
                    1
                } else {
                    0
                }
            }
            None => 0,
        };
        let above = if block.row > 0 {
            context(neighbors.above, block.row.saturating_sub(1), block.col)
        } else {
            0
        };
        let left = if block.col > 0 {
            context(neighbors.left, block.row, block.col.saturating_sub(1))
        } else {
            0
        };
        above + left + 3 * usize::from(fwd == bck)
    }

    fn distance_weights(&self, ref_frames: [i8; 2]) -> [u8; 2] {
        let bck_hint = self.config.reference_order_hints
            [self.config.ref_frame_idx[(ref_frames[0] - 1) as usize] as usize];
        let fwd_hint = self.config.reference_order_hints
            [self.config.ref_frame_idx[(ref_frames[1] - 1) as usize] as usize];
        let current = i32::from(self.config.current_order_hint);
        let d0 = get_relative_dist(
            i32::from(fwd_hint),
            current,
            self.config.enable_order_hint,
            self.config.order_hint_bits,
        )
        .abs()
        .clamp(0, MAX_FRAME_DISTANCE);
        let d1 = get_relative_dist(
            current,
            i32::from(bck_hint),
            self.config.enable_order_hint,
            self.config.order_hint_bits,
        )
        .abs()
        .clamp(0, MAX_FRAME_DISTANCE);
        let order = usize::from(d0 <= d1);
        let index = if d0 == 0 || d1 == 0 {
            3
        } else {
            (0..3)
                .find(|index| {
                    let c0 = QUANT_DIST_WEIGHT[*index][order];
                    let c1 = QUANT_DIST_WEIGHT[*index][1 - order];
                    let d0_c0 = d0 * c0;
                    let d1_c1 = d1 * c1;
                    (d0 > d1 && d0_c0 < d1_c1) || (d0 <= d1 && d0_c0 > d1_c1)
                })
                .unwrap_or(3)
        };
        [
            QUANT_DIST_LOOKUP[index][order],
            QUANT_DIST_LOOKUP[index][1 - order],
        ]
    }

    fn record_block(
        &mut self,
        block: DecodedBlock,
        skip: bool,
        skip_mode: bool,
        inter: Option<InterBlock>,
        y_mode: Option<IntraMode>,
        uv_mode: Option<UvMode>,
    ) {
        let refs = inter
            .map(|inter| [inter.ref_frame, inter.ref_frame1])
            .unwrap_or([INTRA_FRAME, NONE_FRAME]);
        let is_inter = inter.is_some();
        let mode = inter.map(|inter| inter.mode);
        let compound_mode = inter.and_then(|inter| inter.compound_mode);
        let compound_group = inter
            .and_then(|inter| inter.compound_type)
            .map(CompoundType::comp_group_idx)
            .unwrap_or(0);
        let compound_index = usize::from(!matches!(
            inter.and_then(|inter| inter.compound_type),
            Some(CompoundType::DistanceWeighted)
        )) as u8;
        let interp_filter = inter.map(|inter| inter.interp_filter).unwrap_or([0, 0]);
        let mvs = inter
            .map(|inter| [inter.mv, inter.mv1])
            .unwrap_or([MotionVector::default(); 2]);
        let bw4 = block.size.num_4x4_wide() as usize;
        let bh4 = block.size.num_4x4_high() as usize;
        for row in block.row..(block.row + bh4).min(self.config.mi_rows) {
            for col in block.col..(block.col + bw4).min(self.config.mi_cols) {
                let index = Self::at(row, col, self.config.mi_cols);
                self.decoded[index] = true;
                self.sizes[index] = Some(block.size);
                self.is_inter[index] = is_inter;
                self.refs[index] = refs;
                self.modes[index] = mode;
                self.compound_modes[index] = compound_mode;
                self.compound_groups[index] = compound_group;
                self.compound_indices[index] = compound_index;
                self.mvs[index] = mvs;
                self.interp_filters[index] = interp_filter;
                self.skip_grid[index] = skip;
                self.skip_modes[index] = skip_mode;
                self.y_modes[index] = y_mode;
                self.uv_modes[index] = uv_mode;
            }
        }
    }
}

impl TileBlockSink for InterTileBlockSink {
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
        let bw4 = block.size.num_4x4_wide() as usize;
        let bh4 = block.size.num_4x4_high() as usize;
        let preskip_segment = if self.config.segmentation_enabled && self.config.seg_id_pre_skip {
            Some(
                self.segments
                    .read_inter_segment_id(
                        decoder,
                        cdfs,
                        block.row,
                        block.col,
                        bw4,
                        bh4,
                        true,
                        false,
                        self.config.segmentation_enabled,
                        self.config.segmentation_update_map,
                        self.config.segmentation_temporal_update,
                        self.config.seg_id_pre_skip,
                        self.config.disable_cdf_update,
                    )
                    .map_err(TileEntropyError::Cdf)?,
            )
        } else {
            None
        };
        let initial_segment_id = preskip_segment.unwrap_or(0);
        let skip_mode = self
            .read_skip_mode(decoder, cdfs, block, initial_segment_id)
            .map_err(Self::conversion)?;
        // `set_mi_row_col` gates `up_available`/`left_available` on the tile,
        // so the tile edge behaves like the picture edge for the skip
        // context even when the carried frame grid holds decoded blocks from
        // an earlier tile.
        let above_skip = self
            .above_context_index(block.row, block.col)
            .is_some_and(|index| self.skip_grid[index]);
        let left_skip = self
            .left_context_index(block.row, block.col)
            .is_some_and(|index| self.skip_grid[index]);
        let skip = if skip_mode {
            true
        } else if self.config.segmentation_enabled && self.config.segment_skip[initial_segment_id] {
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
        let segment_id = match preskip_segment {
            Some(segment_id) => segment_id,
            None => self
                .segments
                .read_inter_segment_id(
                    decoder,
                    cdfs,
                    block.row,
                    block.col,
                    bw4,
                    bh4,
                    false,
                    skip,
                    self.config.segmentation_enabled,
                    self.config.segmentation_update_map,
                    self.config.segmentation_temporal_update,
                    self.config.seg_id_pre_skip,
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

        let sb = (
            block.row / self.config.superblock_mi,
            block.col / self.config.superblock_mi,
        );
        if self.current_sb != Some(sb) {
            self.current_sb = Some(sb);
            self.read_deltas_for_sb = true;
        }
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

        let neighbors = self.neighbors(block);
        let above_intra = neighbors
            .above
            .map(|refs| refs[0] <= INTRA_FRAME)
            .unwrap_or(true);
        let left_intra = neighbors
            .left
            .map(|refs| refs[0] <= INTRA_FRAME)
            .unwrap_or(true);
        let is_inter = if skip_mode {
            true
        } else if let Some(ref_frame) = self.config.segment_ref_frame[segment_id] {
            ref_frame > INTRA_FRAME
        } else if self.config.segment_globalmv[segment_id] {
            true
        } else {
            read_is_inter(
                decoder,
                cdfs,
                neighbors.above.is_some(),
                neighbors.left.is_some(),
                above_intra,
                left_intra,
                self.config.disable_cdf_update,
            )
            .map_err(Self::conversion)?
        };
        if !is_inter {
            let intra = self.decode_intra_syntax(
                block,
                decoder,
                cdfs,
                segment_id,
                skip,
                block_qindex,
                lossless,
                self.delta_lf,
            )?;
            self.record_block(
                block,
                skip,
                skip_mode,
                None,
                Some(intra.y_mode),
                intra.uv_mode,
            );
            self.blocks.push(InterSyntaxBlock {
                block,
                segment_id,
                skip,
                qindex: block_qindex,
                delta_lf: self.delta_lf,
                inter: None,
                intra: Some(intra.clone()),
                lossless: intra.lossless,
                tx: intra.tx,
                transforms: intra.transforms.clone(),
            });
            return Ok(());
        }

        let ref_frames = if skip_mode {
            [
                LAST_FRAME + self.config.skip_mode_frame[0] as i8,
                LAST_FRAME + self.config.skip_mode_frame[1] as i8,
            ]
        } else if let Some(ref_frame) = self.config.segment_ref_frame[segment_id] {
            [ref_frame, NONE_FRAME]
        } else if self.config.segment_skip[segment_id] || self.config.segment_globalmv[segment_id] {
            [LAST_FRAME, NONE_FRAME]
        } else {
            read_ref_frames(
                decoder,
                cdfs,
                neighbors,
                block.size,
                self.config.reference_select,
                self.config.disable_cdf_update,
            )
            .map_err(Self::conversion)?
        };
        let is_compound = ref_frames[1] > INTRA_FRAME;
        let global_mv0 = global_motion_vector_from_params(
            &self.config.gm_types,
            &self.config.gm_params,
            ref_frames[0],
            block.size,
            block.row,
            block.col,
            self.config.allow_high_precision_mv,
            self.config.force_integer_mv,
        )
        .map_err(Self::conversion)?;
        let global_mv1 = if is_compound {
            global_motion_vector_from_params(
                &self.config.gm_types,
                &self.config.gm_params,
                ref_frames[1],
                block.size,
                block.row,
                block.col,
                self.config.allow_high_precision_mv,
                self.config.force_integer_mv,
            )
            .map_err(Self::conversion)?
        } else {
            MotionVector::default()
        };
        let grid = MotionGrid {
            mi_cols: self.config.mi_cols,
            mi_rows: self.config.mi_rows,
            decoded: &self.decoded,
            sizes: &self.sizes,
            is_inter: &self.is_inter,
            refs: &self.refs,
            modes: &self.modes,
            compound_modes: &self.compound_modes,
            mvs: &self.mvs,
            temporal: self.config.temporal_motion.as_ref(),
            tile_row_start: self.config.tile_row_start,
            tile_row_end: self.config.tile_row_end,
            tile_col_start: self.config.tile_col_start,
            tile_col_end: self.config.tile_col_end,
        };
        let stack = find_mv_stack(
            grid,
            block.row,
            block.col,
            block.size,
            ref_frames,
            [global_mv0, global_mv1],
            self.config.allow_high_precision_mv,
            self.config.force_integer_mv,
            self.config.use_ref_frame_mvs,
        )
        .map_err(Self::conversion)?;
        let segment_forces_global =
            self.config.segment_skip[segment_id] || self.config.segment_globalmv[segment_id];
        let (mode, compound_mode, ref_mv_idx, mv0, mv1, motion_mode) = if skip_mode {
            let compound_mode = CompoundMode::NearestNearest;
            let mvs = assign_compound_mv(
                decoder,
                &mut cdfs.mv,
                &stack,
                0,
                compound_mode,
                [global_mv0, global_mv1],
                self.config.allow_high_precision_mv,
                self.config.force_integer_mv,
                self.config.disable_cdf_update,
            )
            .map_err(Self::conversion)?;
            (
                compound_mode.ref0_mode(),
                Some(compound_mode),
                0,
                mvs[0],
                mvs[1],
                crate::reference::av1::inter::MotionMode::Simple,
            )
        } else if segment_forces_global {
            (
                crate::reference::av1::inter::InterMode::Global,
                None,
                0,
                global_mv0,
                MotionVector::default(),
                crate::reference::av1::inter::MotionMode::Simple,
            )
        } else if is_compound {
            let compound_mode =
                read_compound_mode(decoder, cdfs, &stack, self.config.disable_cdf_update)
                    .map_err(Self::conversion)?;
            let ref_mv_idx = read_compound_ref_mv_index(
                decoder,
                cdfs,
                &stack,
                compound_mode,
                self.config.disable_cdf_update,
            )
            .map_err(Self::conversion)?;
            let mvs = assign_compound_mv(
                decoder,
                &mut cdfs.mv,
                &stack,
                ref_mv_idx,
                compound_mode,
                [global_mv0, global_mv1],
                self.config.allow_high_precision_mv,
                self.config.force_integer_mv,
                self.config.disable_cdf_update,
            )
            .map_err(Self::conversion)?;
            (
                compound_mode.ref0_mode(),
                Some(compound_mode),
                ref_mv_idx,
                mvs[0],
                mvs[1],
                crate::reference::av1::inter::MotionMode::Simple,
            )
        } else {
            let mode = read_inter_mode(decoder, cdfs, &stack, self.config.disable_cdf_update)
                .map_err(Self::conversion)?;
            let ref_mv_idx =
                read_ref_mv_index(decoder, cdfs, &stack, mode, self.config.disable_cdf_update)
                    .map_err(Self::conversion)?;
            let mv = assign_mv(
                decoder,
                &mut cdfs.mv,
                &stack,
                ref_mv_idx,
                mode,
                global_mv0,
                self.config.allow_high_precision_mv,
                self.config.force_integer_mv,
                self.config.disable_cdf_update,
            )
            .map_err(Self::conversion)?;
            (
                mode,
                None,
                ref_mv_idx,
                mv,
                MotionVector::default(),
                crate::reference::av1::inter::MotionMode::Simple,
            )
        };
        let interintra = if !skip_mode
            && !is_compound
            && self.config.enable_interintra_compound
            && interintra_allowed(block.size, ref_frames)
        {
            read_interintra(decoder, cdfs, block.size, self.config.disable_cdf_update)
                .map_err(Self::conversion)?
        } else {
            None
        };
        let (motion_mode, mut warp) = if !is_compound
            && interintra.is_none()
            && !skip_mode
            && self.config.is_motion_mode_switchable
        {
            let last_allowed = self.motion_mode_allowed(block, mode, ref_frames);
            let selected = read_motion_mode(
                decoder,
                cdfs,
                block.size,
                last_allowed,
                self.config.disable_cdf_update,
            )
            .map_err(Self::conversion)?;
            let warp = if selected == crate::reference::av1::inter::MotionMode::LocalWarp {
                let mut samples = self.projection_samples(block, ref_frames[0]);
                if samples.len() > 1 {
                    select_samples(&mut samples, mv0, block.size);
                }
                find_projection(&samples, block.size, mv0, block.row, block.col)
            } else {
                None
            };
            (selected, warp)
        } else {
            (motion_mode, None)
        };
        let mut warp1 = None;
        if mode == crate::reference::av1::inter::InterMode::Global
            && self.config.gm_types[(ref_frames[0] - 1) as usize]
                >= crate::reference::av1::frame_header::GlobalMotionType::RotZoom as u8
        {
            warp = WarpParams::from_matrix(self.config.gm_params[(ref_frames[0] - 1) as usize]);
        }
        if is_compound
            && compound_mode == Some(CompoundMode::GlobalGlobal)
            && self.config.gm_types[(ref_frames[1] - 1) as usize]
                >= crate::reference::av1::frame_header::GlobalMotionType::RotZoom as u8
        {
            warp1 = WarpParams::from_matrix(self.config.gm_params[(ref_frames[1] - 1) as usize]);
        }
        let ref_frame1 = if interintra.is_some() {
            INTRA_FRAME
        } else {
            ref_frames[1]
        };
        let compound_type = if skip_mode {
            Some(CompoundType::Average)
        } else if is_compound {
            let masked_compound_used = self.config.enable_masked_compound
                && !skip_mode
                && block.size.width.min(block.size.height) >= 8;
            let comp_group_idx = if masked_compound_used {
                let context = self.compound_group_context(block, &neighbors);
                cdfs.read_symbol(
                    decoder,
                    &DEFAULT_COMP_GROUP_IDX_CDF,
                    context * 3,
                    3,
                    self.config.disable_cdf_update,
                )? as u8
            } else {
                0
            };
            Some(if comp_group_idx == 0 {
                if self.config.enable_dist_wtd_comp && !skip_mode {
                    let context = self.compound_index_context(block, ref_frames, &neighbors);
                    let compound_idx = cdfs.read_symbol(
                        decoder,
                        &DEFAULT_COMPOUND_IDX_CDF,
                        context * 3,
                        3,
                        self.config.disable_cdf_update,
                    )?;
                    if compound_idx != 0 {
                        CompoundType::Average
                    } else {
                        CompoundType::DistanceWeighted
                    }
                } else {
                    CompoundType::Average
                }
            } else {
                read_compound_type(
                    decoder,
                    cdfs,
                    block.size,
                    comp_group_idx,
                    self.config.disable_cdf_update,
                )
                .map_err(Self::conversion)?
            })
        } else {
            None
        };
        let distance_weights = if compound_type == Some(CompoundType::DistanceWeighted) {
            Some(self.distance_weights(ref_frames))
        } else {
            None
        };
        let interp_filter = self
            .read_filter(
                decoder,
                cdfs,
                block,
                ref_frames[0],
                ref_frame1,
                mode,
                compound_mode,
                motion_mode,
                skip_mode,
                &neighbors,
            )
            .map_err(Self::conversion)?;
        let bw4 = block.size.num_4x4_wide() as usize;
        let bh4 = block.size.num_4x4_high() as usize;
        let above_width = self.selected_tx_above_width(block);
        let left_height = self.selected_tx_left_height(block);
        let variable_tx = !lossless && !skip && self.config.tx_mode_select && !block.size.is_4x4();
        let (tx, luma_tx_blocks) = if lossless {
            (TxSize::Tx4x4, Vec::new())
        } else if skip {
            (
                crate::reference::av1::tx::max_tx_size_rect(block.size),
                Vec::new(),
            )
        } else if variable_tx {
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
                    true,
                    lossless,
                    self.config.tx_mode_select,
                    above_width,
                    left_height,
                    self.config.disable_cdf_update,
                )
                .map_err(TileEntropyError::Cdf)?,
                Vec::new(),
            )
        };
        if !variable_tx {
            for row in block.row..(block.row + bh4).min(self.config.mi_rows) {
                for col in block.col..(block.col + bw4).min(self.config.mi_cols) {
                    self.tx_grid[Self::at(row, col, self.config.mi_cols)] = Some(tx);
                }
            }
        }
        if skip {
            let planes = if self.has_chroma(block) { 3 } else { 1 };
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
        }

        let mut transforms = Vec::new();
        if !skip {
            let has_chroma = self.has_chroma(block);
            let mut planes = intra_residual_blocks(
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
            let residual_blocks = if variable_tx {
                let mut residual_blocks = luma_tx_blocks;
                residual_blocks.extend(planes.drain(..).filter(|block| block.plane > 0));
                residual_blocks
            } else {
                planes
            };
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
                    coeff_q_context(self.config.base_q_index),
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
                        let w = (tx_block.tx.width() / 4) as usize;
                        let h = (tx_block.tx.height() / 4) as usize;
                        for row in y4..(y4 + h).min(self.config.mi_rows) {
                            for col in x4..(x4 + w).min(self.config.mi_cols) {
                                self.tx_type_grid[Self::at(row, col, self.config.mi_cols)] =
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
                        block_qindex,
                        self.config.reduced_tx_set,
                        self.config.disable_cdf_update,
                    )
                    .map_err(TileEntropyError::Cdf)?;
                    let w = (tx_block.tx.width() / 4) as usize;
                    let h = (tx_block.tx.height() / 4) as usize;
                    for row in y4..(y4 + h).min(self.config.mi_rows) {
                        for col in x4..(x4 + w).min(self.config.mi_cols) {
                            self.tx_type_grid[Self::at(row, col, self.config.mi_cols)] =
                                Some(tx_type);
                        }
                    }
                    tx_type
                } else {
                    let luma_x = (x4 << sx).max(block.col);
                    let luma_y = (y4 << sy).max(block.row);
                    let candidate = self.tx_type_grid
                        [Self::at(luma_y, luma_x, self.config.mi_cols)]
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
                        coeff_q_context(self.config.base_q_index),
                        self.config.disable_cdf_update,
                    )
                    .map_err(TileEntropyError::Cdf)?
                };
                transforms.push(crate::reference::av1::intra_sink::IntraSyntaxTx {
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

        let inter = InterBlock {
            size: block.size,
            ref_frame: ref_frames[0],
            mode,
            mv: mv0,
            ref_frame1,
            mv1,
            compound_mode,
            compound_type,
            distance_weights,
            interintra,
            motion_mode,
            warp,
            warp1,
            interp_filter,
            ref_mv_idx,
        };
        self.record_block(block, skip, skip_mode, Some(inter), None, None);
        self.blocks.push(InterSyntaxBlock {
            block,
            segment_id,
            skip,
            qindex: block_qindex,
            delta_lf: self.delta_lf,
            inter: Some(inter),
            intra: None,
            lossless,
            tx,
            transforms,
        });
        Ok(())
    }
}
