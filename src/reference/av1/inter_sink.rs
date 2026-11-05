//! Inter-frame tile sink: single-reference, integer-translation blocks.
//!
//! The first inter slice keeps the syntax boundary explicit: it consumes the
//! complete mode/reference/MV syntax and records the motion state that later
//! blocks use for MV prediction. Non-skip blocks, intra blocks inside an inter
//! frame, compound prediction and fractional MVs are rejected until their
//! reconstruction stages are implemented.

use crate::reference::av1::cdef::CdefIndexGrid;
use crate::reference::av1::cdf_defaults::DEFAULT_SKIP_MODE_CDF;
use crate::reference::av1::cdf_state::CdfState;
use crate::reference::av1::delta::{read_delta_lf, read_delta_qindex, DeltaError};
use crate::reference::av1::frame_header::FrameHeader;
use crate::reference::av1::inter::{
    assign_mv, find_mv_stack, global_motion_vector_from_params, read_inter_mode, read_is_inter,
    read_motion_mode, read_ref_mv_index, read_single_ref_frame, InterBlock, InterError,
    InterNeighbors, MotionGrid, MotionVector, MvCdfs, INTRA_FRAME, NONE_FRAME,
};
use crate::reference::av1::lr_frame::{LrFrameParams, LrPlaneGeometry};
use crate::reference::av1::lr_syntax::{LrSyntaxError, LrTileState};
use crate::reference::av1::partition::BlockSize;
use crate::reference::av1::seq_header::SequenceHeader;
use crate::reference::av1::tile::DecodedBlock;
use crate::reference::av1::tile_entropy::{TileBlockSink, TileEntropyError};

#[derive(Debug, Clone)]
pub struct InterTileConfig {
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub sub_x: u32,
    pub sub_y: u32,
    pub bit_depth: u32,
    pub segmentation_enabled: bool,
    pub seg_id_pre_skip: bool,
    pub segment_skip: [bool; 8],
    pub segment_ref_frame: [Option<i8>; 8],
    pub segment_globalmv: [bool; 8],
    pub disable_cdf_update: bool,
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
    pub reference_select: bool,
    pub skip_mode_present: bool,
    pub allow_high_precision_mv: bool,
    pub force_integer_mv: bool,
    pub use_ref_frame_mvs: bool,
    pub is_filter_switchable: bool,
    pub interpolation_filter: u8,
    pub is_motion_mode_switchable: bool,
    pub allow_warped_motion: bool,
    pub enable_order_hint: bool,
    pub gm_types: [u8; 7],
    pub gm_params: [[i32; 6]; 7],
}

impl InterTileConfig {
    pub fn from_headers(sequence: &SequenceHeader, header: &FrameHeader) -> Self {
        let mut segment_skip = [false; 8];
        let mut segment_ref_frame = [None; 8];
        let mut segment_globalmv = [false; 8];
        for segment in 0..8 {
            segment_skip[segment] = header.feature_enabled[segment][6];
            if header.feature_enabled[segment][5] {
                segment_ref_frame[segment] = Some(header.feature_data[segment][5] as i8);
            }
            segment_globalmv[segment] = header.feature_enabled[segment][7];
        }
        Self {
            mi_cols: header.mi_cols,
            mi_rows: header.mi_rows,
            sub_x: u32::from(sequence.color.subsampling_x),
            sub_y: u32::from(sequence.color.subsampling_y),
            bit_depth: u32::from(sequence.color.bit_depth),
            segmentation_enabled: header.segmentation_enabled,
            seg_id_pre_skip: header.seg_id_pre_skip,
            segment_skip,
            segment_ref_frame,
            segment_globalmv,
            disable_cdf_update: header.disable_cdf_update,
            delta_q_present: header.delta_q_present,
            delta_q_res: header.delta_q_res,
            delta_lf_present: header.delta_lf_present,
            delta_lf_res: header.delta_lf_res,
            delta_lf_multi: header.delta_lf_multi,
            cdef_enabled: sequence.enable_cdef,
            cdef_bits: header.cdef_bits,
            base_q_index: header.base_q_idx,
            superblock_mi: if sequence.use_128x128_superblock {
                32
            } else {
                16
            },
            allow_intrabc: header.allow_intrabc,
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
            allow_high_precision_mv: header.allow_high_precision_mv,
            force_integer_mv: header.force_integer_mv != 0,
            use_ref_frame_mvs: header.use_ref_frame_mvs,
            is_filter_switchable: header.is_filter_switchable,
            interpolation_filter: header.interpolation_filter,
            is_motion_mode_switchable: header.is_motion_mode_switchable,
            allow_warped_motion: header.allow_warped_motion,
            enable_order_hint: sequence.enable_order_hint,
            gm_types: header.gm_types,
            gm_params: header.gm_params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterSyntaxBlock {
    pub block: DecodedBlock,
    pub segment_id: usize,
    pub skip: bool,
    pub qindex: u8,
    pub delta_lf: [i32; 4],
    pub inter: InterBlock,
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
    mvs: Vec<[MotionVector; 2]>,
    mv_cdfs: MvCdfs,
    current_qindex: u8,
    delta_lf: [i32; 4],
    read_deltas_for_sb: bool,
    current_sb: Option<(usize, usize)>,
    lr: LrTileState,
    pub blocks: Vec<InterSyntaxBlock>,
}

impl InterTileBlockSink {
    pub fn new(config: InterTileConfig) -> Self {
        let mi_cols = config.mi_cols;
        let mi_rows = config.mi_rows;
        let current_qindex = config.base_q_index;
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
            mvs: vec![[MotionVector::default(); 2]; mi_cols * mi_rows],
            mv_cdfs: MvCdfs::default(),
            current_qindex,
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
            DeltaError::MultiDeltaLfUnsupported => TileEntropyError::Unsupported("delta_lf_multi"),
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

    fn neighbors(&self, block: DecodedBlock) -> InterNeighbors {
        let above = (block.row > 0
            && self.decoded[Self::at(block.row - 1, block.col, self.config.mi_cols)])
        .then(|| self.refs[Self::at(block.row - 1, block.col, self.config.mi_cols)]);
        let left = (block.col > 0
            && self.decoded[Self::at(block.row, block.col - 1, self.config.mi_cols)])
        .then(|| self.refs[Self::at(block.row, block.col - 1, self.config.mi_cols)]);
        InterNeighbors { above, left }
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
        if block.row > 0 {
            ctx += usize::from(
                self.skip_modes[Self::at(block.row - 1, block.col, self.config.mi_cols)],
            );
        }
        if block.col > 0 {
            ctx += usize::from(
                self.skip_modes[Self::at(block.row, block.col - 1, self.config.mi_cols)],
            );
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
        _decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
    ) -> Result<[u8; 2], InterError> {
        if self.config.is_filter_switchable {
            // The switchable-filter syntax needs neighbour filter contexts.
            // Refuse before consuming a block payload rather than desync.
            return Err(InterError::Unsupported("switchable interpolation filter"));
        }
        let fixed = self.config.interpolation_filter;
        Ok([fixed, fixed])
    }

    fn record_block(
        &mut self,
        block: DecodedBlock,
        skip: bool,
        skip_mode: bool,
        inter: InterBlock,
    ) {
        let bw4 = block.size.num_4x4_wide() as usize;
        let bh4 = block.size.num_4x4_high() as usize;
        for row in block.row..(block.row + bh4).min(self.config.mi_rows) {
            for col in block.col..(block.col + bw4).min(self.config.mi_cols) {
                let index = Self::at(row, col, self.config.mi_cols);
                self.decoded[index] = true;
                self.sizes[index] = Some(block.size);
                self.is_inter[index] = true;
                self.refs[index] = [inter.ref_frame, NONE_FRAME];
                self.modes[index] = Some(inter.mode);
                self.mvs[index] = [inter.mv, MotionVector::default()];
                self.skip_grid[index] = skip;
                self.skip_modes[index] = skip_mode;
            }
        }
    }
}

impl TileBlockSink for InterTileBlockSink {
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
        if self.config.segmentation_enabled {
            return Err(TileEntropyError::Unsupported("segmentation in inter frame"));
        }
        if self.config.enable_order_hint {
            return Err(TileEntropyError::Unsupported("order-hint inter frame"));
        }
        let segment_id = 0usize;
        let skip_mode = self
            .read_skip_mode(decoder, cdfs, block, segment_id)
            .map_err(Self::conversion)?;
        if skip_mode {
            return Err(TileEntropyError::Unsupported("skip mode"));
        }

        let above_skip = block.row > 0
            && self.skip_grid[Self::at(block.row - 1, block.col, self.config.mi_cols)];
        let left_skip = block.col > 0
            && self.skip_grid[Self::at(block.row, block.col - 1, self.config.mi_cols)];
        let skip = crate::reference::av1::block::read_skip_with_cdf(
            decoder,
            cdfs,
            above_skip,
            left_skip,
            self.config.disable_cdf_update,
        )
        .map_err(TileEntropyError::Cdf)?;

        self.cdef
            .read(
                decoder,
                block.row,
                block.col,
                block.size,
                skip,
                false,
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
        let delta_lf = read_delta_lf(
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
        self.delta_lf[0] = delta_lf;
        self.read_deltas_for_sb = false;

        let neighbors = self.neighbors(block);
        let above_intra = neighbors
            .above
            .map(|refs| refs[0] <= INTRA_FRAME)
            .unwrap_or(true);
        let left_intra = neighbors
            .left
            .map(|refs| refs[0] <= INTRA_FRAME)
            .unwrap_or(true);
        let is_inter = read_is_inter(
            decoder,
            cdfs,
            neighbors.above.is_some(),
            neighbors.left.is_some(),
            above_intra,
            left_intra,
            self.config.disable_cdf_update,
        )
        .map_err(Self::conversion)?;
        if !is_inter {
            return Err(TileEntropyError::Unsupported(
                "intra block inside inter frame",
            ));
        }

        let ref_frame = read_single_ref_frame(
            decoder,
            cdfs,
            neighbors,
            block.size,
            self.config.reference_select,
            self.config.disable_cdf_update,
        )
        .map_err(Self::conversion)?;
        let global_mv = global_motion_vector_from_params(
            &self.config.gm_types,
            &self.config.gm_params,
            ref_frame,
            self.config.allow_high_precision_mv,
            self.config.force_integer_mv,
        )
        .map_err(Self::conversion)?;
        let grid = MotionGrid {
            mi_cols: self.config.mi_cols,
            mi_rows: self.config.mi_rows,
            decoded: &self.decoded,
            sizes: &self.sizes,
            is_inter: &self.is_inter,
            refs: &self.refs,
            modes: &self.modes,
            mvs: &self.mvs,
        };
        let stack = find_mv_stack(
            grid,
            block.row,
            block.col,
            block.size,
            ref_frame,
            global_mv,
            self.config.allow_high_precision_mv,
            self.config.force_integer_mv,
            self.config.use_ref_frame_mvs,
        )
        .map_err(Self::conversion)?;
        let mode = read_inter_mode(decoder, cdfs, &stack, self.config.disable_cdf_update)
            .map_err(Self::conversion)?;
        let ref_mv_idx =
            read_ref_mv_index(decoder, cdfs, &stack, mode, self.config.disable_cdf_update)
                .map_err(Self::conversion)?;
        let mv = assign_mv(
            decoder,
            &mut self.mv_cdfs,
            &stack,
            ref_mv_idx,
            mode,
            global_mv,
            self.config.allow_high_precision_mv,
            self.config.force_integer_mv,
            self.config.disable_cdf_update,
        )
        .map_err(Self::conversion)?;
        if !mv.is_integer() {
            return Err(TileEntropyError::Unsupported("fractional motion vector"));
        }
        let motion_mode = read_motion_mode(
            decoder,
            cdfs,
            block.size,
            self.config.is_motion_mode_switchable,
            skip_mode,
            self.config.disable_cdf_update,
        )
        .map_err(Self::conversion)?;
        let interp_filter = self.read_filter(decoder).map_err(Self::conversion)?;
        if !skip {
            return Err(TileEntropyError::Unsupported("inter residual"));
        }

        let inter = InterBlock {
            ref_frame,
            mode,
            mv,
            motion_mode,
            interp_filter,
            ref_mv_idx,
        };
        self.record_block(block, skip, skip_mode, inter.clone());
        self.blocks.push(InterSyntaxBlock {
            block,
            segment_id,
            skip,
            qindex: self.current_qindex,
            delta_lf: self.delta_lf,
            inter,
        });
        Ok(())
    }
}
