//! AV1 inter-prediction syntax and spatial motion-vector prediction.
//!
//! This module implements `inter_block_mode_info()` for single and average
//! compound references: reference-frame signalling, the inter-mode symbols,
//! the spatial motion-vector stack, and MV-difference decoding. The tile sink
//! owns the per-frame grids required by the spatial stack.
//!
//! Inter-intra, wedge/distance-weighted compound, local warp and global warp
//! are decoded here; temporal motion-field candidates remain an explicit error
//! rather than a silently approximated path.

use crate::reference::av1::cdf_defaults::{
    CdfTable, DEFAULT_COMPOUND_TYPE_CDF, DEFAULT_COMP_BWD_REF_CDF, DEFAULT_COMP_MODE_CDF,
    DEFAULT_COMP_REF_CDF, DEFAULT_COMP_REF_TYPE_CDF, DEFAULT_DRL_MODE_CDF, DEFAULT_INTER_INTRA_CDF,
    DEFAULT_INTER_INTRA_MODE_CDF, DEFAULT_IS_INTER_CDF, DEFAULT_MOTION_MODE_CDF,
    DEFAULT_NEW_MV_CDF, DEFAULT_REF_MV_CDF, DEFAULT_SINGLE_REF_CDF, DEFAULT_UNI_COMP_REF_CDF,
    DEFAULT_USE_OBMC_CDF, DEFAULT_WEDGE_INDEX_CDF, DEFAULT_WEDGE_INTER_INTRA_CDF,
    DEFAULT_ZERO_MV_CDF,
};
use crate::reference::av1::cdf_state::{CdfState, CdfStateError, MvCdfs};
use crate::reference::av1::frame_header::{FrameHeader, GlobalMotionType, WARPEDMODEL_PREC_BITS};
use crate::reference::av1::partition::{block_size_index, size_group, BlockSize, BLOCK_SIZES};
use crate::reference::av1::symbol::{SymbolDecoder, SymbolError};
use crate::reference::av1::temporal::TemporalMotionField;
use crate::reference::av1::warped::WarpParams;

pub const INTRA_FRAME: i8 = 0;
pub const LAST_FRAME: i8 = 1;
pub const LAST2_FRAME: i8 = 2;
pub const LAST3_FRAME: i8 = 3;
pub const GOLDEN_FRAME: i8 = 4;
pub const BWDREF_FRAME: i8 = 5;
pub const ALTREF2_FRAME: i8 = 6;
pub const ALTREF_FRAME: i8 = 7;
pub const NONE_FRAME: i8 = -1;

pub const MV_BORDER: i32 = 16 << 3;
const REF_CAT_LEVEL: u16 = 640;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MotionVector {
    /// Vertical displacement in 1/8 luma samples.
    pub row: i32,
    /// Horizontal displacement in 1/8 luma samples.
    pub col: i32,
}

impl MotionVector {
    pub const fn new(row: i32, col: i32) -> Self {
        Self { row, col }
    }

    pub fn is_integer(self) -> bool {
        self.row & 7 == 0 && self.col & 7 == 0
    }

    fn lower_precision(&mut self, allow_high_precision: bool, force_integer: bool) {
        if allow_high_precision {
            return;
        }
        for component in [&mut self.row, &mut self.col] {
            if force_integer {
                let magnitude = component.unsigned_abs();
                let integer = (magnitude + 3) >> 3;
                *component = if *component > 0 {
                    (integer << 3) as i32
                } else {
                    -((integer << 3) as i32)
                };
            } else if *component & 1 != 0 {
                *component += if *component > 0 { -1 } else { 1 };
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterMode {
    Nearest,
    Near,
    Global,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundMode {
    NearestNearest,
    NearNear,
    NearestNew,
    NewNearest,
    NearNew,
    NewNear,
    GlobalGlobal,
    NewNew,
}

impl CompoundMode {
    pub fn ref0_mode(self) -> InterMode {
        match self {
            Self::NearestNearest | Self::NearestNew => InterMode::Nearest,
            Self::NearNear | Self::NearNew => InterMode::Near,
            Self::NewNearest | Self::NewNear | Self::NewNew => InterMode::New,
            Self::GlobalGlobal => InterMode::Global,
        }
    }

    pub fn ref1_mode(self) -> InterMode {
        match self {
            Self::NearestNearest | Self::NewNearest => InterMode::Nearest,
            Self::NearNear | Self::NewNear => InterMode::Near,
            Self::NearestNew | Self::NearNew | Self::NewNew => InterMode::New,
            Self::GlobalGlobal => InterMode::Global,
        }
    }

    fn has_new_mv(self) -> bool {
        matches!(
            self,
            Self::NearestNew | Self::NewNearest | Self::NearNew | Self::NewNear | Self::NewNew
        )
    }

    fn has_near_mv(self) -> bool {
        matches!(self, Self::NearNear | Self::NearNew | Self::NewNear)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundType {
    Average,
    DistanceWeighted,
    Wedge { index: u8, sign: u8 },
    DifferenceWeighted { mask_type: u8 },
}

impl CompoundType {
    pub fn comp_group_idx(self) -> u8 {
        match self {
            Self::Average | Self::DistanceWeighted => 0,
            Self::Wedge { .. } | Self::DifferenceWeighted { .. } => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterIntraMode {
    Dc,
    V,
    H,
    Smooth,
}

impl InterIntraMode {
    fn from_symbol(symbol: usize) -> Option<Self> {
        Some(match symbol {
            0 => Self::Dc,
            1 => Self::V,
            2 => Self::H,
            3 => Self::Smooth,
            _ => return None,
        })
    }

    pub fn intra_mode(self) -> crate::reference::av1::intra::IntraMode {
        use crate::reference::av1::intra::IntraMode;
        match self {
            Self::Dc => IntraMode::Dc,
            Self::V => IntraMode::V,
            Self::H => IntraMode::H,
            Self::Smooth => IntraMode::Smooth,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterIntra {
    pub mode: InterIntraMode,
    pub wedge_index: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionMode {
    Simple,
    Obmc,
    LocalWarp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionModeAllowed {
    Simple,
    Obmc,
    Warped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterBlock {
    pub size: BlockSize,
    pub ref_frame: i8,
    pub mode: InterMode,
    pub mv: MotionVector,
    pub ref_frame1: i8,
    pub mv1: MotionVector,
    pub compound_mode: Option<CompoundMode>,
    pub compound_type: Option<CompoundType>,
    pub distance_weights: Option<[u8; 2]>,
    pub interintra: Option<InterIntra>,
    pub motion_mode: MotionMode,
    pub warp: Option<WarpParams>,
    pub warp1: Option<WarpParams>,
    pub interp_filter: [u8; 2],
    pub ref_mv_idx: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct InterNeighbors {
    pub above: Option<[i8; 2]>,
    pub left: Option<[i8; 2]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterError {
    Cdf(CdfStateError),
    Symbol(SymbolError),
    Unsupported(&'static str),
    InvalidSyntax(&'static str),
}

impl From<CdfStateError> for InterError {
    fn from(error: CdfStateError) -> Self {
        Self::Cdf(error)
    }
}

impl From<SymbolError> for InterError {
    fn from(error: SymbolError) -> Self {
        Self::Symbol(error)
    }
}

fn binary(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    table: &CdfTable,
    row: usize,
    disable_cdf_update: bool,
) -> Result<usize, InterError> {
    Ok(cdfs.read_symbol(decoder, table, row * 3, 3, disable_cdf_update)?)
}

fn ref_count_ctx(left: usize, right: usize) -> usize {
    match left.cmp(&right) {
        std::cmp::Ordering::Less => 0,
        std::cmp::Ordering::Equal => 1,
        std::cmp::Ordering::Greater => 2,
    }
}

fn count_refs(neighbors: InterNeighbors, frame: i8) -> usize {
    let mut count = 0;
    for refs in [neighbors.above, neighbors.left].into_iter().flatten() {
        count += usize::from(refs[0] == frame);
        count += usize::from(refs[1] == frame);
    }
    count
}

fn count_set(neighbors: InterNeighbors, frames: &[i8]) -> usize {
    frames
        .iter()
        .map(|frame| count_refs(neighbors, *frame))
        .sum()
}

fn check_backward(frame: i8) -> bool {
    (BWDREF_FRAME..=ALTREF_FRAME).contains(&frame)
}

fn comparison_context(left: usize, right: usize) -> usize {
    match left.cmp(&right) {
        std::cmp::Ordering::Less => 0,
        std::cmp::Ordering::Equal => 1,
        std::cmp::Ordering::Greater => 2,
    }
}

fn is_compound_pair(refs: [i8; 2]) -> bool {
    refs[0] > INTRA_FRAME && refs[1] > INTRA_FRAME
}

fn is_uni_compound_pair(refs: [i8; 2]) -> bool {
    is_compound_pair(refs) && check_backward(refs[0]) == check_backward(refs[1])
}

fn comp_reference_type_context(neighbors: InterNeighbors) -> usize {
    match (neighbors.above, neighbors.left) {
        (Some(above), Some(left)) => {
            let above_intra = above[0] <= INTRA_FRAME;
            let left_intra = left[0] <= INTRA_FRAME;
            if above_intra && left_intra {
                2
            } else if above_intra || left_intra {
                let inter = if above_intra { left } else { above };
                if is_compound_pair(inter) {
                    1 + 2 * usize::from(is_uni_compound_pair(inter))
                } else {
                    2
                }
            } else {
                let above_single = !is_compound_pair(above);
                let left_single = !is_compound_pair(left);
                if above_single && left_single {
                    1 + 2 * usize::from(check_backward(above[0]) == check_backward(left[0]))
                } else if above_single || left_single {
                    let compound = if above_single { left } else { above };
                    if is_uni_compound_pair(compound) {
                        3 + usize::from(check_backward(above[0]) == check_backward(left[0]))
                    } else {
                        1
                    }
                } else if !is_uni_compound_pair(above) && !is_uni_compound_pair(left) {
                    0
                } else if !is_uni_compound_pair(above) || !is_uni_compound_pair(left) {
                    2
                } else {
                    3 + usize::from((above[0] == BWDREF_FRAME) == (left[0] == BWDREF_FRAME))
                }
            }
        }
        (Some(edge), None) | (None, Some(edge)) => {
            if edge[0] <= INTRA_FRAME || !is_compound_pair(edge) {
                2
            } else {
                4 * usize::from(is_uni_compound_pair(edge))
            }
        }
        (None, None) => 2,
    }
}

fn reference_mode_context(neighbors: InterNeighbors) -> usize {
    let above = neighbors.above.map(|refs| {
        let intra = refs[0] <= INTRA_FRAME;
        let single = refs[1] <= INTRA_FRAME;
        (refs[0], intra, single)
    });
    let left = neighbors.left.map(|refs| {
        let intra = refs[0] <= INTRA_FRAME;
        let single = refs[1] <= INTRA_FRAME;
        (refs[0], intra, single)
    });
    match (above, left) {
        (
            Some((above_ref, above_intra, above_single)),
            Some((left_ref, left_intra, left_single)),
        ) => {
            if above_single && left_single {
                usize::from(check_backward(above_ref) != check_backward(left_ref))
            } else if above_single {
                2 + usize::from(check_backward(above_ref) || above_intra)
            } else if left_single {
                2 + usize::from(check_backward(left_ref) || left_intra)
            } else {
                4
            }
        }
        (Some((above_ref, _, above_single)), None) => {
            if above_single {
                usize::from(check_backward(above_ref))
            } else {
                3
            }
        }
        (None, Some((left_ref, _, left_single))) => {
            if left_single {
                usize::from(check_backward(left_ref))
            } else {
                3
            }
        }
        (None, None) => 1,
    }
}

/// `read_is_inter()` for a non-skip, non-segmented block.
#[allow(clippy::too_many_arguments)]
pub fn read_is_inter(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    avail_u: bool,
    avail_l: bool,
    above_intra: bool,
    left_intra: bool,
    disable_cdf_update: bool,
) -> Result<bool, InterError> {
    let ctx = is_inter_context(avail_u, avail_l, above_intra, left_intra);
    Ok(binary(
        decoder,
        cdfs,
        &DEFAULT_IS_INTER_CDF,
        ctx,
        disable_cdf_update,
    )? != 0)
}

pub fn is_inter_context(
    avail_u: bool,
    avail_l: bool,
    above_intra: bool,
    left_intra: bool,
) -> usize {
    if avail_u && avail_l {
        if left_intra && above_intra {
            3
        } else {
            usize::from(left_intra || above_intra)
        }
    } else if avail_u || avail_l {
        2 * usize::from(if avail_u { above_intra } else { left_intra })
    } else {
        0
    }
}

fn read_compound_ref_frames(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    neighbors: InterNeighbors,
    disable_cdf_update: bool,
) -> Result<[i8; 2], InterError> {
    let comp_type_ctx = comp_reference_type_context(neighbors);
    let comp_type = cdfs.read_symbol(
        decoder,
        &DEFAULT_COMP_REF_TYPE_CDF,
        comp_type_ctx * 3,
        3,
        disable_cdf_update,
    )?;
    if comp_type == 0 {
        let fwd = count_set(
            neighbors,
            &[LAST_FRAME, LAST2_FRAME, LAST3_FRAME, GOLDEN_FRAME],
        );
        let bwd = count_set(neighbors, &[BWDREF_FRAME, ALTREF2_FRAME, ALTREF_FRAME]);
        let p = binary(
            decoder,
            cdfs,
            &DEFAULT_UNI_COMP_REF_CDF,
            comparison_context(fwd, bwd) * 3,
            disable_cdf_update,
        )?;
        if p != 0 {
            return Ok([BWDREF_FRAME, ALTREF_FRAME]);
        }
        let last2 = count_refs(neighbors, LAST2_FRAME);
        let last3 = count_refs(neighbors, LAST3_FRAME);
        let golden = count_refs(neighbors, GOLDEN_FRAME);
        let p1 = binary(
            decoder,
            cdfs,
            &DEFAULT_UNI_COMP_REF_CDF,
            comparison_context(last2, last3 + golden) * 3 + 1,
            disable_cdf_update,
        )?;
        if p1 == 0 {
            return Ok([LAST_FRAME, LAST2_FRAME]);
        }
        let p2 = binary(
            decoder,
            cdfs,
            &DEFAULT_UNI_COMP_REF_CDF,
            comparison_context(last3, golden) * 3 + 2,
            disable_cdf_update,
        )?;
        return Ok(if p2 != 0 {
            [LAST_FRAME, GOLDEN_FRAME]
        } else {
            [LAST_FRAME, LAST3_FRAME]
        });
    }

    let last12 = count_refs(neighbors, LAST_FRAME) + count_refs(neighbors, LAST2_FRAME);
    let last3gold = count_refs(neighbors, LAST3_FRAME) + count_refs(neighbors, GOLDEN_FRAME);
    let p = binary(
        decoder,
        cdfs,
        &DEFAULT_COMP_REF_CDF,
        comparison_context(last12, last3gold) * 3,
        disable_cdf_update,
    )?;
    let ref0 = if p == 0 {
        let p1 = binary(
            decoder,
            cdfs,
            &DEFAULT_COMP_REF_CDF,
            comparison_context(
                count_refs(neighbors, LAST_FRAME),
                count_refs(neighbors, LAST2_FRAME),
            ) * 3
                + 1,
            disable_cdf_update,
        )?;
        if p1 != 0 {
            LAST2_FRAME
        } else {
            LAST_FRAME
        }
    } else {
        let p2 = binary(
            decoder,
            cdfs,
            &DEFAULT_COMP_REF_CDF,
            comparison_context(
                count_refs(neighbors, LAST3_FRAME),
                count_refs(neighbors, GOLDEN_FRAME),
            ) * 3
                + 2,
            disable_cdf_update,
        )?;
        if p2 != 0 {
            GOLDEN_FRAME
        } else {
            LAST3_FRAME
        }
    };
    let brfarf2 = count_refs(neighbors, BWDREF_FRAME) + count_refs(neighbors, ALTREF2_FRAME);
    let arf = count_refs(neighbors, ALTREF_FRAME);
    let pb = binary(
        decoder,
        cdfs,
        &DEFAULT_COMP_BWD_REF_CDF,
        comparison_context(brfarf2, arf) * 2,
        disable_cdf_update,
    )?;
    let ref1 = if pb == 0 {
        let p1 = binary(
            decoder,
            cdfs,
            &DEFAULT_COMP_BWD_REF_CDF,
            comparison_context(
                count_refs(neighbors, BWDREF_FRAME),
                count_refs(neighbors, ALTREF2_FRAME),
            ) * 2
                + 1,
            disable_cdf_update,
        )?;
        if p1 != 0 {
            ALTREF2_FRAME
        } else {
            BWDREF_FRAME
        }
    } else {
        ALTREF_FRAME
    };
    Ok([ref0, ref1])
}

/// `read_ref_frames()` for single and compound references.
pub fn read_ref_frames(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    neighbors: InterNeighbors,
    size: BlockSize,
    reference_select: bool,
    disable_cdf_update: bool,
) -> Result<[i8; 2], InterError> {
    if reference_select && size.num_4x4_wide().min(size.num_4x4_high()) >= 2 {
        let ctx = reference_mode_context(neighbors);
        let compound = binary(
            decoder,
            cdfs,
            &DEFAULT_COMP_MODE_CDF,
            ctx,
            disable_cdf_update,
        )?;
        if compound != 0 {
            return read_compound_ref_frames(decoder, cdfs, neighbors, disable_cdf_update);
        }
    }

    let fwd = count_set(
        neighbors,
        &[LAST_FRAME, LAST2_FRAME, LAST3_FRAME, GOLDEN_FRAME],
    );
    let bwd = count_set(neighbors, &[BWDREF_FRAME, ALTREF2_FRAME, ALTREF_FRAME]);
    let p1 = binary(
        decoder,
        cdfs,
        &DEFAULT_SINGLE_REF_CDF,
        ref_count_ctx(fwd, bwd) * 6,
        disable_cdf_update,
    )?;
    if p1 != 0 {
        let brfarf2 = count_set(neighbors, &[BWDREF_FRAME, ALTREF2_FRAME]);
        let arf = count_refs(neighbors, ALTREF_FRAME);
        let p2 = binary(
            decoder,
            cdfs,
            &DEFAULT_SINGLE_REF_CDF,
            ref_count_ctx(brfarf2, arf) * 6 + 1,
            disable_cdf_update,
        )?;
        if p2 != 0 {
            return Ok([ALTREF_FRAME, NONE_FRAME]);
        }
        let brf = count_refs(neighbors, BWDREF_FRAME);
        let arf2 = count_refs(neighbors, ALTREF2_FRAME);
        let p6 = binary(
            decoder,
            cdfs,
            &DEFAULT_SINGLE_REF_CDF,
            ref_count_ctx(brf, arf2) * 6 + 5,
            disable_cdf_update,
        )?;
        return Ok([
            if p6 != 0 { ALTREF2_FRAME } else { BWDREF_FRAME },
            NONE_FRAME,
        ]);
    }

    let last12 = count_refs(neighbors, LAST_FRAME) + count_refs(neighbors, LAST2_FRAME);
    let last3gold = count_refs(neighbors, LAST3_FRAME) + count_refs(neighbors, GOLDEN_FRAME);
    let p3 = binary(
        decoder,
        cdfs,
        &DEFAULT_SINGLE_REF_CDF,
        ref_count_ctx(last12, last3gold) * 6 + 2,
        disable_cdf_update,
    )?;
    if p3 != 0 {
        let last3 = count_refs(neighbors, LAST3_FRAME);
        let gold = count_refs(neighbors, GOLDEN_FRAME);
        let p5 = binary(
            decoder,
            cdfs,
            &DEFAULT_SINGLE_REF_CDF,
            ref_count_ctx(last3, gold) * 6 + 4,
            disable_cdf_update,
        )?;
        return Ok([if p5 != 0 { GOLDEN_FRAME } else { LAST3_FRAME }, NONE_FRAME]);
    }
    let last = count_refs(neighbors, LAST_FRAME);
    let last2 = count_refs(neighbors, LAST2_FRAME);
    let p4 = binary(
        decoder,
        cdfs,
        &DEFAULT_SINGLE_REF_CDF,
        ref_count_ctx(last, last2) * 6 + 3,
        disable_cdf_update,
    )?;
    Ok([if p4 != 0 { LAST2_FRAME } else { LAST_FRAME }, NONE_FRAME])
}

/// Compatibility wrapper for callers that only accept a single reference.
pub fn read_single_ref_frame(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    neighbors: InterNeighbors,
    size: BlockSize,
    reference_select: bool,
    disable_cdf_update: bool,
) -> Result<i8, InterError> {
    let refs = read_ref_frames(
        decoder,
        cdfs,
        neighbors,
        size,
        reference_select,
        disable_cdf_update,
    )?;
    if refs[1] > INTRA_FRAME {
        Err(InterError::Unsupported("compound reference"))
    } else {
        Ok(refs[0])
    }
}

/// The spatial `find_mv_stack()` result used by the mode syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionVectorStack {
    pub candidates: [[MotionVector; 2]; 8],
    pub weights: [u16; 8],
    pub count: usize,
    mode_context: u8,
    compound_mode_context: u8,
}

impl MotionVectorStack {
    pub fn mode_context(&self) -> u8 {
        self.mode_context
    }

    pub fn compound_mode_context(&self) -> u8 {
        self.compound_mode_context
    }

    fn drl_context(&self, index: usize) -> usize {
        if index + 1 >= self.count {
            return 0;
        }
        match (
            self.weights[index] >= REF_CAT_LEVEL,
            self.weights[index + 1] >= REF_CAT_LEVEL,
        ) {
            (true, false) => 1,
            (false, false) => 2,
            _ => 0,
        }
    }
}

/// Per-frame motion state visible to the stack builder.
pub struct MotionGrid<'a> {
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub decoded: &'a [bool],
    pub sizes: &'a [Option<BlockSize>],
    pub is_inter: &'a [bool],
    pub refs: &'a [[i8; 2]],
    pub modes: &'a [Option<InterMode>],
    pub compound_modes: &'a [Option<CompoundMode>],
    pub mvs: &'a [[MotionVector; 2]],
    pub temporal: Option<&'a TemporalMotionField>,
}

impl MotionGrid<'_> {
    fn inside(&self, row: isize, col: isize) -> bool {
        row >= 0 && col >= 0 && (row as usize) < self.mi_rows && (col as usize) < self.mi_cols
    }

    fn index(&self, row: usize, col: usize) -> usize {
        row * self.mi_cols + col
    }

    fn size(&self, row: usize, col: usize) -> Option<BlockSize> {
        self.sizes[self.index(row, col)]
    }
}

/// Resolve a global motion vector at the centre of a block.
pub fn global_motion_vector(
    header: &FrameHeader,
    reference: i8,
    size: BlockSize,
    mi_row: usize,
    mi_col: usize,
    allow_high_precision_mv: bool,
    force_integer_mv: bool,
) -> Result<MotionVector, InterError> {
    global_motion_vector_from_params(
        &header.gm_types,
        &header.gm_params,
        reference,
        size,
        mi_row,
        mi_col,
        allow_high_precision_mv,
        force_integer_mv,
    )
}

fn round_motion_signed(value: i64, shift: u32) -> i64 {
    if value < 0 {
        -((value.unsigned_abs() + (1u64 << (shift - 1))) as i64 >> shift)
    } else {
        (value + (1i64 << (shift - 1))) >> shift
    }
}

pub fn global_motion_vector_from_params(
    gm_types: &[u8; 7],
    gm_params: &[[i32; 6]; 7],
    reference: i8,
    size: BlockSize,
    mi_row: usize,
    mi_col: usize,
    allow_high_precision_mv: bool,
    force_integer_mv: bool,
) -> Result<MotionVector, InterError> {
    if !(LAST_FRAME..=ALTREF_FRAME).contains(&reference) {
        return Err(InterError::InvalidSyntax("global motion needs a reference"));
    }
    let index = (reference - LAST_FRAME) as usize;
    let matrix = gm_params[index];
    let mut mv = match GlobalMotionType::from_bits(gm_types[index]) {
        GlobalMotionType::Identity => MotionVector::default(),
        GlobalMotionType::Translation => MotionVector::new(
            matrix[0] >> (WARPEDMODEL_PREC_BITS - 3),
            matrix[1] >> (WARPEDMODEL_PREC_BITS - 3),
        ),
        GlobalMotionType::RotZoom | GlobalMotionType::Affine => {
            let x = mi_col as i64 * 4 + i64::from(size.width / 2) - 1;
            let y = mi_row as i64 * 4 + i64::from(size.height / 2) - 1;
            let tx = (i64::from(matrix[2]) - (1i64 << WARPEDMODEL_PREC_BITS)) * x
                + i64::from(matrix[3]) * y
                + i64::from(matrix[0]);
            let ty = i64::from(matrix[4]) * x
                + (i64::from(matrix[5]) - (1i64 << WARPEDMODEL_PREC_BITS)) * y
                + i64::from(matrix[1]);
            let convert = |value: i64| {
                if allow_high_precision_mv {
                    round_motion_signed(value, WARPEDMODEL_PREC_BITS - 3)
                } else {
                    round_motion_signed(value, WARPEDMODEL_PREC_BITS - 2) * 2
                }
            };
            MotionVector::new(convert(ty) as i32, convert(tx) as i32)
        }
    };
    if force_integer_mv {
        mv.lower_precision(allow_high_precision_mv, true);
    }
    Ok(mv)
}

struct StackBuilder<'a> {
    stack: MotionVectorStack,
    grid: MotionGrid<'a>,
    ref_frames: [i8; 2],
    global_mvs: [MotionVector; 2],
    is_compound: bool,
    allow_high_precision_mv: bool,
    force_integer_mv: bool,
    found: bool,
    new_mv_count: usize,
}

impl StackBuilder<'_> {
    fn add_to_stack(&mut self, mvs: [MotionVector; 2], weight: u16) {
        for index in 0..self.stack.count {
            if self.stack.candidates[index] == mvs {
                self.stack.weights[index] += weight;
                return;
            }
        }
        if self.stack.count < self.stack.candidates.len() {
            self.stack.candidates[self.stack.count] = mvs;
            self.stack.weights[self.stack.count] = weight;
            self.stack.count += 1;
        }
    }

    fn add_ref_candidate(&mut self, row: usize, col: usize, weight: u16) {
        let index = self.grid.index(row, col);
        if !self.grid.is_inter[index] {
            return;
        }
        let refs = self.grid.refs[index];
        if self.is_compound {
            if refs != self.ref_frames {
                return;
            }
            let mut mvs = self.grid.mvs[index];
            for list in 0..2 {
                mvs[list].lower_precision(self.allow_high_precision_mv, self.force_integer_mv);
            }
            if self.grid.compound_modes[index]
                .unwrap_or(CompoundMode::NearestNearest)
                .has_new_mv()
            {
                self.new_mv_count += 1;
            }
            self.found = true;
            self.add_to_stack(mvs, weight);
        } else {
            let Some(list) = (0..2).find(|list| refs[*list] == self.ref_frames[0]) else {
                return;
            };
            let mode = self.grid.modes[index].unwrap_or(InterMode::Nearest);
            let size = self.grid.size(row, col).unwrap_or(BLOCK_SIZES[0]);
            let mut mv = if mode == InterMode::Global
                && size.width.min(size.height) >= 8
                && self.global_mvs[0] != MotionVector::default()
            {
                self.global_mvs[0]
            } else {
                self.grid.mvs[index][list]
            };
            mv.lower_precision(self.allow_high_precision_mv, self.force_integer_mv);
            if matches!(mode, InterMode::New) {
                self.new_mv_count += 1;
            }
            self.found = true;
            self.add_to_stack([mv, MotionVector::default()], weight);
        }
    }

    fn add_temporal_candidate(
        &mut self,
        block_row: usize,
        block_col: usize,
        offset_row: isize,
        offset_col: isize,
    ) -> Option<[MotionVector; 2]> {
        let temporal = self.grid.temporal?;
        let position_row = block_row as isize + offset_row + if block_row & 1 == 0 { 1 } else { 0 };
        let position_col = block_col as isize + offset_col + if block_col & 1 == 0 { 1 } else { 0 };
        if position_row < 0
            || position_col < 0
            || position_row >= temporal.mi_rows as isize
            || position_col >= temporal.mi_cols as isize
        {
            return None;
        }
        let candidate =
            temporal.candidate((position_row as usize) >> 1, (position_col as usize) >> 1)?;
        let mut mvs = [MotionVector::default(); 2];
        for list in 0..=usize::from(self.is_compound) {
            let reference = self.ref_frames[list];
            let reference_hint = temporal.ref_order_hints[(reference - 1) as usize];
            let current_offset = crate::reference::av1::frame_header::get_relative_dist(
                i32::from(temporal.current_order_hint),
                i32::from(reference_hint),
                temporal.enable_order_hint,
                temporal.order_hint_bits,
            );
            mvs[list] = crate::reference::av1::temporal::project_mv(
                candidate.mv,
                current_offset,
                candidate.ref_frame_offset,
            );
            mvs[list].lower_precision(self.allow_high_precision_mv, self.force_integer_mv);
        }
        self.add_to_stack(mvs, 2);
        Some(mvs)
    }

    fn scan_row(&mut self, delta_row: isize, block_row: usize, block_col: usize, size: BlockSize) {
        let bw4 = size.num_4x4_wide() as isize;
        let end4 = bw4
            .min((self.grid.mi_cols as isize - block_col as isize).max(0))
            .min(16);
        let mut delta_col = 0isize;
        let mut row = block_row as isize + delta_row;
        if delta_row.unsigned_abs() > 1 {
            row += (block_row & 1) as isize;
            delta_col = 1 - (block_col & 1) as isize;
            if !self.grid.inside(row, block_col as isize + delta_col) {
                return;
            }
        }
        let use_step16 = bw4 >= 16;
        let mut i = 0isize;
        while i < end4 {
            let col = block_col as isize + delta_col + i;
            if !self.grid.inside(row, col)
                || !self.grid.decoded[self.grid.index(row as usize, col as usize)]
            {
                break;
            }
            let candidate = self
                .grid
                .size(row as usize, col as usize)
                .unwrap_or(BLOCK_SIZES[0]);
            let mut len = bw4.min(candidate.num_4x4_wide() as isize);
            if delta_row.unsigned_abs() > 1 {
                len = len.max(2);
            }
            if use_step16 {
                len = len.max(4);
            }
            self.add_ref_candidate(row as usize, col as usize, (len * 2) as u16);
            i += len;
        }
    }

    fn scan_col(&mut self, delta_col: isize, block_row: usize, block_col: usize, size: BlockSize) {
        let bh4 = size.num_4x4_high() as isize;
        let end4 = bh4
            .min((self.grid.mi_rows as isize - block_row as isize).max(0))
            .min(16);
        let mut delta_row = 0isize;
        let mut col = block_col as isize + delta_col;
        if delta_col.unsigned_abs() > 1 {
            col += (block_col & 1) as isize;
            delta_row = 1 - (block_row & 1) as isize;
            if !self.grid.inside(block_row as isize + delta_row, col) {
                return;
            }
        }
        let use_step16 = bh4 >= 16;
        let mut i = 0isize;
        while i < end4 {
            let row = block_row as isize + delta_row + i;
            if !self.grid.inside(row, col)
                || !self.grid.decoded[self.grid.index(row as usize, col as usize)]
            {
                break;
            }
            let candidate = self
                .grid
                .size(row as usize, col as usize)
                .unwrap_or(BLOCK_SIZES[0]);
            let mut len = bh4.min(candidate.num_4x4_high() as isize);
            if delta_col.unsigned_abs() > 1 {
                len = len.max(2);
            }
            if use_step16 {
                len = len.max(4);
            }
            self.add_ref_candidate(row as usize, col as usize, (len * 2) as u16);
            i += len;
        }
    }

    fn scan_point(&mut self, row: isize, col: isize) -> bool {
        if !self.grid.inside(row, col)
            || !self.grid.decoded[self.grid.index(row as usize, col as usize)]
        {
            return false;
        }
        self.found = false;
        self.add_ref_candidate(row as usize, col as usize, 4);
        self.found
    }

    fn sort(&mut self, start: usize, end: usize) {
        let mut end = end;
        while end > start {
            let mut new_end = start;
            for index in (start + 1)..end {
                if self.stack.weights[index - 1] < self.stack.weights[index] {
                    self.stack.weights.swap(index - 1, index);
                    self.stack.candidates.swap(index - 1, index);
                    new_end = index;
                }
            }
            end = new_end;
        }
    }

    fn extra_search(&mut self, block_row: usize, block_col: usize, size: BlockSize) {
        let w4 = (size.num_4x4_wide() as usize)
            .min(16)
            .min(self.grid.mi_cols.saturating_sub(block_col));
        let h4 = (size.num_4x4_high() as usize)
            .min(16)
            .min(self.grid.mi_rows.saturating_sub(block_row));
        let num4x4 = w4.min(h4);
        let mut id_mvs = [[MotionVector::default(); 2]; 2];
        let mut id_counts = [0usize; 2];
        let mut diff_mvs = [[MotionVector::default(); 2]; 2];
        let mut diff_counts = [0usize; 2];
        for pass in 0..2 {
            let mut index = 0usize;
            while index < num4x4 && self.stack.count < 2 {
                let (row, col) = if pass == 0 {
                    (block_row as isize - 1, block_col as isize + index as isize)
                } else {
                    (block_row as isize + index as isize, block_col as isize - 1)
                };
                if !self.grid.inside(row, col) {
                    break;
                }
                if self.is_compound {
                    self.collect_extra_candidate(
                        row as usize,
                        col as usize,
                        &mut id_mvs,
                        &mut id_counts,
                        &mut diff_mvs,
                        &mut diff_counts,
                    );
                } else {
                    self.add_extra_candidate(row as usize, col as usize);
                }
                let candidate = self
                    .grid
                    .size(row as usize, col as usize)
                    .unwrap_or(BLOCK_SIZES[0]);
                index += if pass == 0 {
                    candidate.num_4x4_wide() as usize
                } else {
                    candidate.num_4x4_high() as usize
                };
            }
        }
        if self.is_compound {
            let mut combined = [[MotionVector::default(); 2]; 2];
            for list in 0..2 {
                let mut count = 0usize;
                for index in 0..id_counts[list] {
                    combined[count][list] = id_mvs[list][index];
                    count += 1;
                }
                for index in 0..diff_counts[list].min(2 - count) {
                    combined[count][list] = diff_mvs[list][index];
                    count += 1;
                }
                while count < 2 {
                    combined[count][list] = self.global_mvs[list];
                    count += 1;
                }
            }
            if self.stack.count == 1 {
                if combined[0] == self.stack.candidates[0] {
                    self.stack.candidates[1] = combined[1];
                } else {
                    self.stack.candidates[1] = combined[0];
                }
                self.stack.weights[1] = 2;
                self.stack.count += 1;
            } else {
                for index in 0..2 {
                    self.stack.candidates[self.stack.count] = combined[index];
                    self.stack.weights[self.stack.count] = 2;
                    self.stack.count += 1;
                }
            }
            return;
        }
        // The global-MV fallback fills RefStackMv[0..1] but does not increase
        // NumMvFound; DRL syntax uses the number of real matches.
        let mut fill = self.stack.count;
        while fill < 2 {
            self.stack.candidates[fill] = [self.global_mvs[0], MotionVector::default()];
            fill += 1;
        }
    }

    fn add_extra_candidate(&mut self, row: usize, col: usize) {
        let index = self.grid.index(row, col);
        if !self.grid.is_inter[index] {
            return;
        }
        for list in 0..2 {
            let candidate_ref = self.grid.refs[index][list];
            if candidate_ref <= INTRA_FRAME {
                continue;
            }
            let mut mv = self.grid.mvs[index][list];
            // The supported path requires order hints to be disabled, so all
            // RefFrameSignBias values are zero and no sign inversion is needed.
            mv.lower_precision(self.allow_high_precision_mv, self.force_integer_mv);
            let candidate = [mv, MotionVector::default()];
            let found = (0..self.stack.count).any(|i| self.stack.candidates[i] == candidate);
            if !found && self.stack.count < self.stack.candidates.len() {
                self.stack.candidates[self.stack.count] = candidate;
                self.stack.weights[self.stack.count] = 2;
                self.stack.count += 1;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_extra_candidate(
        &self,
        row: usize,
        col: usize,
        id_mvs: &mut [[MotionVector; 2]; 2],
        id_counts: &mut [usize; 2],
        diff_mvs: &mut [[MotionVector; 2]; 2],
        diff_counts: &mut [usize; 2],
    ) {
        let index = self.grid.index(row, col);
        if !self.grid.is_inter[index] {
            return;
        }
        for candidate_list in 0..2 {
            let candidate_ref = self.grid.refs[index][candidate_list];
            if candidate_ref <= INTRA_FRAME {
                continue;
            }
            let mut candidate_mv = self.grid.mvs[index][candidate_list];
            candidate_mv.lower_precision(self.allow_high_precision_mv, self.force_integer_mv);
            for list in 0..2 {
                if candidate_ref == self.ref_frames[list] && id_counts[list] < 2 {
                    id_mvs[list][id_counts[list]] = candidate_mv;
                    id_counts[list] += 1;
                } else if diff_counts[list] < 2 {
                    // The order-hint path is rejected by the caller, so every
                    // RefFrameSignBias is zero and the vector is not inverted.
                    diff_mvs[list][diff_counts[list]] = candidate_mv;
                    diff_counts[list] += 1;
                }
            }
        }
    }

    fn clamp(&mut self, block_row: usize, block_col: usize, size: BlockSize) {
        let bw4 = size.num_4x4_wide() as i32;
        let bh4 = size.num_4x4_high() as i32;
        let top = -((block_row as i32 * 4) * 8);
        let bottom = ((self.grid.mi_rows as i32 - bh4 - block_row as i32) * 4) * 8;
        let left = -((block_col as i32 * 4) * 8);
        let right = ((self.grid.mi_cols as i32 - bw4 - block_col as i32) * 4) * 8;
        let border_row = MV_BORDER + size.height as i32 * 8;
        let border_col = MV_BORDER + size.width as i32 * 8;
        let lists = if self.is_compound { 2 } else { 1 };
        for candidate in &mut self.stack.candidates {
            for mv in &mut candidate[..lists] {
                mv.row = mv.row.clamp(top - border_row, bottom + border_row);
                mv.col = mv.col.clamp(left - border_col, right + border_col);
            }
        }
    }
}

/// Build the spatial two-entry stack for one or two reference frames.
#[allow(clippy::too_many_arguments)]
pub fn find_mv_stack(
    grid: MotionGrid<'_>,
    block_row: usize,
    block_col: usize,
    size: BlockSize,
    ref_frames: [i8; 2],
    global_mvs: [MotionVector; 2],
    allow_high_precision_mv: bool,
    force_integer_mv: bool,
    use_ref_frame_mvs: bool,
) -> Result<MotionVectorStack, InterError> {
    let mut builder = StackBuilder {
        stack: MotionVectorStack {
            candidates: [[MotionVector::default(); 2]; 8],
            weights: [0; 8],
            count: 0,
            mode_context: 0,
            compound_mode_context: 0,
        },
        grid,
        ref_frames,
        global_mvs,
        is_compound: ref_frames[1] > INTRA_FRAME,
        allow_high_precision_mv,
        force_integer_mv,
        found: false,
        new_mv_count: 0,
    };

    builder.scan_row(-1, block_row, block_col, size);
    let mut found_above = builder.found;
    builder.found = false;
    builder.scan_col(-1, block_row, block_col, size);
    let found_left = builder.found;
    if size.num_4x4_wide().max(size.num_4x4_high()) <= 16 {
        builder.found = false;
        let point_found = builder.scan_point(
            block_row as isize - 1,
            block_col as isize + size.num_4x4_wide() as isize,
        );
        found_above |= point_found;
    }
    let close_matches = usize::from(found_above) + usize::from(found_left);
    let num_nearest = builder.stack.count;
    let num_new = builder.new_mv_count;
    for weight in &mut builder.stack.weights[..num_nearest] {
        *weight += REF_CAT_LEVEL;
    }

    let mut temporal_global_bit = false;
    if use_ref_frame_mvs {
        if builder.grid.temporal.is_some() {
            let height_mi = size.num_4x4_high() as isize;
            let width_mi = size.num_4x4_wide() as isize;
            let vertical_offset = height_mi.max(4);
            let horizontal_offset = width_mi.max(4);
            let end_row = height_mi.min(16);
            let end_col = width_mi.min(16);
            let step_row = if height_mi >= 16 { 4 } else { 2 };
            let step_col = if width_mi >= 16 { 4 } else { 2 };
            let mut base_available = false;
            let mut row_offset = 0;
            while row_offset < end_row {
                let mut col_offset = 0;
                while col_offset < end_col {
                    let candidate = builder
                        .add_temporal_candidate(block_row, block_col, row_offset, col_offset);
                    if row_offset == 0 && col_offset == 0 {
                        base_available = candidate.is_some();
                        if let Some(candidate) = candidate {
                            let far = if builder.is_compound {
                                (candidate[0].row - global_mvs[0].row).abs() >= 16
                                    || (candidate[0].col - global_mvs[0].col).abs() >= 16
                                    || (candidate[1].row - global_mvs[1].row).abs() >= 16
                                    || (candidate[1].col - global_mvs[1].col).abs() >= 16
                            } else {
                                (candidate[0].row - global_mvs[0].row).abs() >= 16
                                    || (candidate[0].col - global_mvs[0].col).abs() >= 16
                            };
                            temporal_global_bit |= far;
                        }
                    }
                    col_offset += step_col;
                }
                row_offset += step_row;
            }
            if !base_available {
                temporal_global_bit = true;
            } else {
                let allow_extension =
                    height_mi >= 2 && height_mi < 16 && width_mi >= 2 && width_mi < 16;
                if allow_extension {
                    let sb_row = (block_row & 15) as isize;
                    let sb_col = (block_col & 15) as isize;
                    for (offset_row, offset_col) in [
                        (vertical_offset, -2),
                        (vertical_offset, horizontal_offset),
                        (vertical_offset - 2, horizontal_offset),
                    ] {
                        let row = sb_row + offset_row;
                        let col = sb_col + offset_col;
                        if !(0..16).contains(&row) || !(0..16).contains(&col) {
                            continue;
                        }
                        let _ = builder
                            .add_temporal_candidate(block_row, block_col, offset_row, offset_col);
                    }
                }
            }
        } else {
            temporal_global_bit = true;
        }
    }

    builder.found = false;
    let point_found = builder.scan_point(block_row as isize - 1, block_col as isize - 1);
    if point_found {
        found_above = true;
    }
    let mut total_above = found_above;
    let mut total_left = found_left;
    builder.found = false;
    builder.scan_row(-3, block_row, block_col, size);
    total_above |= builder.found;
    builder.found = false;
    builder.scan_col(-3, block_row, block_col, size);
    total_left |= builder.found;
    builder.found = false;
    if size.num_4x4_high() > 1 {
        builder.scan_row(-5, block_row, block_col, size);
        total_above |= builder.found;
    }
    builder.found = false;
    if size.num_4x4_wide() > 1 {
        builder.scan_col(-5, block_row, block_col, size);
        total_left |= builder.found;
    }
    let total_matches = usize::from(total_above) + usize::from(total_left);

    builder.sort(0, num_nearest);
    builder.sort(num_nearest, builder.stack.count);
    if builder.stack.count < 2 {
        builder.extra_search(block_row, block_col, size);
    }
    builder.clamp(block_row, block_col, size);

    let (new_mv_ctx, ref_mv_ctx) = match close_matches {
        0 => (usize::from(total_matches.min(1)), total_matches),
        1 => (3 - usize::from(num_new.min(1)), 2 + total_matches),
        _ => (5 - usize::from(num_new.min(1)), 5),
    };
    builder.stack.mode_context =
        (new_mv_ctx | (ref_mv_ctx << 4) | (usize::from(temporal_global_bit) << 3)) as u8;
    const COMPOUND_MODE_CTX_MAP: [[u8; 5]; 3] = [[0, 1, 1, 1, 1], [1, 2, 3, 4, 4], [4, 4, 5, 6, 7]];
    builder.stack.compound_mode_context =
        COMPOUND_MODE_CTX_MAP[(ref_mv_ctx >> 1).min(2)][new_mv_ctx.min(4)];
    Ok(builder.stack)
}

/// Decode the single-reference `inter_mode` syntax.
pub fn read_inter_mode(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    stack: &MotionVectorStack,
    disable_cdf_update: bool,
) -> Result<InterMode, InterError> {
    let mode_ctx = stack.mode_context as usize;
    let new_mv = binary(
        decoder,
        cdfs,
        &DEFAULT_NEW_MV_CDF,
        mode_ctx & 7,
        disable_cdf_update,
    )?;
    if new_mv == 0 {
        return Ok(InterMode::New);
    }
    let zero_mv = binary(
        decoder,
        cdfs,
        &DEFAULT_ZERO_MV_CDF,
        (mode_ctx >> 3) & 1,
        disable_cdf_update,
    )?;
    if zero_mv == 0 {
        return Ok(InterMode::Global);
    }
    let ref_mv = binary(
        decoder,
        cdfs,
        &DEFAULT_REF_MV_CDF,
        (mode_ctx >> 4) & 7,
        disable_cdf_update,
    )?;
    Ok(if ref_mv == 0 {
        InterMode::Nearest
    } else {
        InterMode::Near
    })
}

/// `drl_mode` for NEWMV/NEARMV candidate selection.
pub fn read_ref_mv_index(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    stack: &MotionVectorStack,
    mode: InterMode,
    disable_cdf_update: bool,
) -> Result<usize, InterError> {
    let mut result = 0;
    match mode {
        InterMode::New => {
            for index in 0..2 {
                if stack.count > index + 1 {
                    let bit = binary(
                        decoder,
                        cdfs,
                        &DEFAULT_DRL_MODE_CDF,
                        stack.drl_context(index),
                        disable_cdf_update,
                    )?;
                    result = index + bit;
                    if bit == 0 {
                        return Ok(result);
                    }
                }
            }
        }
        InterMode::Near => {
            // NEARMV offsets the first implicit candidate, so the transmitted
            // loop starts at one and subtracts that offset from ref_mv_idx.
            for index in 1..3 {
                if stack.count > index + 1 {
                    let bit = binary(
                        decoder,
                        cdfs,
                        &DEFAULT_DRL_MODE_CDF,
                        stack.drl_context(index),
                        disable_cdf_update,
                    )?;
                    result = index + bit - 1;
                    if bit == 0 {
                        return Ok(result);
                    }
                }
            }
        }
        _ => {}
    }
    Ok(result)
}

/// Read `compound_mode` and its optional DRL index.
pub fn read_compound_mode(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    stack: &MotionVectorStack,
    disable_cdf_update: bool,
) -> Result<CompoundMode, InterError> {
    let symbol = cdfs.read_symbol(
        decoder,
        &crate::reference::av1::cdf_defaults::DEFAULT_COMPOUND_MODE_CDF,
        usize::from(stack.compound_mode_context()) * 9,
        9,
        disable_cdf_update,
    )?;
    Ok(match symbol {
        0 => CompoundMode::NearestNearest,
        1 => CompoundMode::NearNear,
        2 => CompoundMode::NearestNew,
        3 => CompoundMode::NewNearest,
        4 => CompoundMode::NearNew,
        5 => CompoundMode::NewNear,
        6 => CompoundMode::GlobalGlobal,
        7 => CompoundMode::NewNew,
        _ => return Err(InterError::InvalidSyntax("compound mode")),
    })
}

pub(crate) fn wedge_compound_allowed(size: BlockSize) -> bool {
    matches!(
        (size.width, size.height),
        (8, 8) | (8, 16) | (16, 8) | (16, 16) | (16, 32) | (32, 16) | (32, 32) | (8, 32) | (32, 8)
    )
}

pub fn interintra_allowed(size: BlockSize, ref_frames: [i8; 2]) -> bool {
    size.width >= 8
        && size.height >= 8
        && size.width <= 32
        && size.height <= 32
        && ref_frames[0] > INTRA_FRAME
        && ref_frames[1] <= INTRA_FRAME
}

/// Read the single-reference inter-intra decision after `assign_mv()`.
pub fn read_interintra(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    size: BlockSize,
    disable_cdf_update: bool,
) -> Result<Option<InterIntra>, InterError> {
    let group = usize::from(size_group(size));
    debug_assert!(group > 0);
    let interintra = cdfs.read_symbol(
        decoder,
        &DEFAULT_INTER_INTRA_CDF,
        (group - 1) * 3,
        3,
        disable_cdf_update,
    )?;
    if interintra == 0 {
        return Ok(None);
    }
    let mode = cdfs.read_symbol(
        decoder,
        &DEFAULT_INTER_INTRA_MODE_CDF,
        (group - 1) * 5,
        5,
        disable_cdf_update,
    )?;
    let mode =
        InterIntraMode::from_symbol(mode).ok_or(InterError::InvalidSyntax("inter-intra mode"))?;
    let wedge_index = if wedge_compound_allowed(size) {
        let use_wedge = cdfs.read_symbol(
            decoder,
            &DEFAULT_WEDGE_INTER_INTRA_CDF,
            block_size_index(size) * 3,
            3,
            disable_cdf_update,
        )?;
        if use_wedge != 0 {
            Some(cdfs.read_symbol(
                decoder,
                &DEFAULT_WEDGE_INDEX_CDF,
                block_size_index(size) * 17,
                17,
                disable_cdf_update,
            )? as u8)
        } else {
            None
        }
    } else {
        None
    };
    Ok(Some(InterIntra { mode, wedge_index }))
}

/// Read the masked/average compound type selected after `comp_group_idx`.
pub fn read_compound_type(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    size: BlockSize,
    comp_group_idx: u8,
    disable_cdf_update: bool,
) -> Result<CompoundType, InterError> {
    if comp_group_idx == 0 {
        return Ok(CompoundType::Average);
    }
    if comp_group_idx != 1 {
        return Err(InterError::InvalidSyntax("compound group index"));
    }

    let use_wedge = if wedge_compound_allowed(size) {
        cdfs.read_symbol(
            decoder,
            &DEFAULT_COMPOUND_TYPE_CDF,
            block_size_index(size) * 3,
            3,
            disable_cdf_update,
        )? == 0
    } else {
        false
    };
    if use_wedge {
        let index = cdfs.read_symbol(
            decoder,
            &DEFAULT_WEDGE_INDEX_CDF,
            block_size_index(size) * 17,
            17,
            disable_cdf_update,
        )? as u8;
        let sign = decoder.read_literal(1)? as u8;
        Ok(CompoundType::Wedge { index, sign })
    } else {
        let mask_type = decoder.read_literal(1)? as u8;
        Ok(CompoundType::DifferenceWeighted { mask_type })
    }
}

/// `drl_mode` for compound modes with a NEWMV/NearMV component.
pub fn read_compound_ref_mv_index(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    stack: &MotionVectorStack,
    mode: CompoundMode,
    disable_cdf_update: bool,
) -> Result<usize, InterError> {
    let mut result = 0;
    if mode == CompoundMode::NewNew {
        for index in 0..2 {
            if stack.count > index + 1 {
                let bit = binary(
                    decoder,
                    cdfs,
                    &DEFAULT_DRL_MODE_CDF,
                    stack.drl_context(index),
                    disable_cdf_update,
                )?;
                result = index + bit;
                if bit == 0 {
                    return Ok(result);
                }
            }
        }
    }
    if mode.has_near_mv() {
        for index in 1..3 {
            if stack.count > index + 1 {
                let bit = binary(
                    decoder,
                    cdfs,
                    &DEFAULT_DRL_MODE_CDF,
                    stack.drl_context(index),
                    disable_cdf_update,
                )?;
                result = index + bit - 1;
                if bit == 0 {
                    return Ok(result);
                }
            }
        }
    }
    Ok(result)
}

fn read_component(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut MvCdfs,
    component: usize,
    use_subpel: bool,
    use_high_precision: bool,
    disable_cdf_update: bool,
) -> Result<i32, InterError> {
    let sign = decoder.read_symbol_with(&mut cdfs.sign[component], disable_cdf_update)?;
    let mv_class = decoder.read_symbol_with(&mut cdfs.class[component], disable_cdf_update)? as i32;
    let (base, class0_bit) = if mv_class == 0 {
        let bit = decoder.read_symbol_with(&mut cdfs.class0_bit[component], disable_cdf_update)?;
        (0i32, bit)
    } else {
        let count = mv_class as usize;
        let mut bits = 0i32;
        for index in 0..count {
            bits |= (decoder
                .read_symbol_with(&mut cdfs.bits[component][index], disable_cdf_update)?
                as i32)
                << index;
        }
        ((2i32 << (mv_class + 2)) + (bits << 3), 0)
    };
    let (fraction, high_precision) = if use_subpel {
        let fraction_cdf = if mv_class == 0 {
            &mut cdfs.class0_fr[component][class0_bit]
        } else {
            &mut cdfs.fr[component]
        };
        let fraction = decoder.read_symbol_with(fraction_cdf, disable_cdf_update)? as i32;
        let hp = if use_high_precision {
            let hp_cdf = if mv_class == 0 {
                &mut cdfs.class0_hp[component]
            } else {
                &mut cdfs.hp[component]
            };
            decoder.read_symbol_with(hp_cdf, disable_cdf_update)? as i32
        } else {
            1
        };
        (fraction, hp)
    } else {
        (3, 1)
    };
    let magnitude = base + (((class0_bit as i32) << 3) | (fraction << 1) | high_precision) + 1;
    Ok(if sign != 0 { -magnitude } else { magnitude })
}

/// `read_mv()` for one prediction list.
pub fn read_mv(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut MvCdfs,
    reference: MotionVector,
    allow_high_precision_mv: bool,
    force_integer_mv: bool,
    disable_cdf_update: bool,
) -> Result<MotionVector, InterError> {
    let joint = decoder.read_symbol_with(&mut cdfs.joint, disable_cdf_update)?;
    let mut diff = MotionVector::default();
    let use_subpel = !force_integer_mv;
    let use_high_precision = allow_high_precision_mv;
    if matches!(joint, 2 | 3) {
        diff.row = read_component(
            decoder,
            cdfs,
            0,
            use_subpel,
            use_high_precision,
            disable_cdf_update,
        )?;
    }
    if matches!(joint, 1 | 3) {
        diff.col = read_component(
            decoder,
            cdfs,
            1,
            use_subpel,
            use_high_precision,
            disable_cdf_update,
        )?;
    }
    let result = MotionVector::new(reference.row + diff.row, reference.col + diff.col);
    Ok(result)
}

/// Assign the MV for a single-reference mode after the mode syntax.
pub fn assign_mv(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut MvCdfs,
    stack: &MotionVectorStack,
    ref_mv_idx: usize,
    mode: InterMode,
    global_mv: MotionVector,
    allow_high_precision_mv: bool,
    force_integer_mv: bool,
    disable_cdf_update: bool,
) -> Result<MotionVector, InterError> {
    Ok(match mode {
        InterMode::Nearest => stack.candidates[0][0],
        InterMode::Near => stack.candidates[(1 + ref_mv_idx).min(stack.candidates.len() - 1)][0],
        InterMode::Global => global_mv,
        InterMode::New => {
            let reference = if stack.count > 1 {
                stack.candidates[ref_mv_idx][0]
            } else {
                stack.candidates[0][0]
            };
            read_mv(
                decoder,
                cdfs,
                reference,
                allow_high_precision_mv,
                force_integer_mv,
                disable_cdf_update,
            )?
        }
    })
}

/// Assign both list vectors for a compound mode after the mode syntax.
#[allow(clippy::too_many_arguments)]
pub fn assign_compound_mv(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut MvCdfs,
    stack: &MotionVectorStack,
    ref_mv_idx: usize,
    mode: CompoundMode,
    global_mvs: [MotionVector; 2],
    allow_high_precision_mv: bool,
    force_integer_mv: bool,
    disable_cdf_update: bool,
) -> Result<[MotionVector; 2], InterError> {
    let modes = [mode.ref0_mode(), mode.ref1_mode()];
    let mut result = [MotionVector::default(); 2];
    for list in 0..2 {
        result[list] = match modes[list] {
            InterMode::Nearest => stack.candidates[0][list],
            InterMode::Near => {
                stack.candidates[(1 + ref_mv_idx).min(stack.candidates.len() - 1)][list]
            }
            InterMode::Global => global_mvs[list],
            InterMode::New => {
                let position = if stack.count <= 1 { 0 } else { ref_mv_idx };
                read_mv(
                    decoder,
                    cdfs,
                    stack.candidates[position][list],
                    allow_high_precision_mv,
                    force_integer_mv,
                    disable_cdf_update,
                )?
            }
        };
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub fn motion_mode_allowed(
    size: BlockSize,
    mode: InterMode,
    ref_frame1: i8,
    overlappable_neighbors: usize,
    force_integer_mv: bool,
    global_motion_type: u8,
    projected_refs: usize,
    allow_warped_motion: bool,
    reference_is_scaled: bool,
) -> MotionModeAllowed {
    if overlappable_neighbors == 0 {
        return MotionModeAllowed::Simple;
    }
    let motion_variation_size = size.width.min(size.height) >= 8;
    if !force_integer_mv
        && mode == InterMode::Global
        && global_motion_type > 0
        && motion_variation_size
    {
        return MotionModeAllowed::Simple;
    }
    if !motion_variation_size || ref_frame1 > INTRA_FRAME {
        return MotionModeAllowed::Simple;
    }
    if projected_refs >= 1 && allow_warped_motion && !force_integer_mv && !reference_is_scaled {
        MotionModeAllowed::Warped
    } else {
        MotionModeAllowed::Obmc
    }
}

/// Read `motion_mode` for a single-reference block.
pub fn read_motion_mode(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    size: BlockSize,
    last_allowed: MotionModeAllowed,
    disable_cdf_update: bool,
) -> Result<MotionMode, InterError> {
    let index = BLOCK_SIZES.iter().position(|entry| *entry == size).unwrap();
    let symbol = match last_allowed {
        MotionModeAllowed::Simple => return Ok(MotionMode::Simple),
        MotionModeAllowed::Obmc => cdfs.read_symbol(
            decoder,
            &DEFAULT_USE_OBMC_CDF,
            index * 3,
            3,
            disable_cdf_update,
        )?,
        MotionModeAllowed::Warped => cdfs.read_symbol(
            decoder,
            &DEFAULT_MOTION_MODE_CDF,
            index * 4,
            4,
            disable_cdf_update,
        )?,
    };
    match symbol {
        0 => Ok(MotionMode::Simple),
        1 => Ok(MotionMode::Obmc),
        2 if last_allowed == MotionModeAllowed::Warped => Ok(MotionMode::LocalWarp),
        _ => Err(InterError::InvalidSyntax("motion mode")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_mode_allowed_matches_overlap_global_and_warp_preconditions() {
        let size = BLOCK_SIZES
            .iter()
            .find(|size| size.name == "BLOCK_16X16")
            .copied()
            .unwrap();

        assert_eq!(
            motion_mode_allowed(
                size,
                InterMode::Nearest,
                NONE_FRAME,
                0,
                false,
                0,
                1,
                true,
                false,
            ),
            MotionModeAllowed::Simple
        );
        assert_eq!(
            motion_mode_allowed(
                size,
                InterMode::Nearest,
                NONE_FRAME,
                1,
                false,
                0,
                0,
                true,
                false,
            ),
            MotionModeAllowed::Obmc
        );
        assert_eq!(
            motion_mode_allowed(
                size,
                InterMode::Global,
                NONE_FRAME,
                1,
                false,
                1,
                1,
                true,
                false,
            ),
            MotionModeAllowed::Simple
        );
        assert_eq!(
            motion_mode_allowed(
                size,
                InterMode::Nearest,
                NONE_FRAME,
                1,
                false,
                0,
                1,
                true,
                false,
            ),
            MotionModeAllowed::Warped
        );
        assert_eq!(
            motion_mode_allowed(
                size,
                InterMode::Nearest,
                NONE_FRAME,
                1,
                true,
                0,
                1,
                true,
                false,
            ),
            MotionModeAllowed::Obmc
        );
        assert_eq!(
            motion_mode_allowed(
                size,
                InterMode::Nearest,
                LAST_FRAME,
                1,
                false,
                0,
                1,
                true,
                false,
            ),
            MotionModeAllowed::Simple
        );
    }

    #[test]
    fn motion_vector_lowering_matches_integer_and_half_pel_rules() {
        let mut mv = MotionVector::new(13, -13);
        mv.lower_precision(false, true);
        assert_eq!(mv, MotionVector::new(16, -16));
        let mut mv = MotionVector::new(13, -13);
        mv.lower_precision(false, false);
        assert_eq!(mv, MotionVector::new(12, -12));
        let mut mv = MotionVector::new(13, -13);
        mv.lower_precision(true, false);
        assert_eq!(mv, MotionVector::new(13, -13));
    }

    #[test]
    fn reference_count_context_orders_counts() {
        assert_eq!(ref_count_ctx(1, 2), 0);
        assert_eq!(ref_count_ctx(2, 2), 1);
        assert_eq!(ref_count_ctx(3, 2), 2);
    }

    #[test]
    fn zero_bitstream_decodes_a_zero_motion_difference() {
        let mut decoder = SymbolDecoder::init(&[0u8; 16]).unwrap();
        let mut cdfs = MvCdfs::default();
        let mv = read_mv(
            &mut decoder,
            &mut cdfs,
            MotionVector::default(),
            true,
            false,
            false,
        )
        .unwrap();
        assert_eq!(mv, MotionVector::default());
    }
}
