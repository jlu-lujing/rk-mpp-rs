//! AV1 loop restoration tile syntax, specification section 7.17.
//!
//! `lr_frame.rs` walks the restoration-unit grid and filters pixels; this module
//! is the bitstream half that fills that grid in. `decode_tile()` resets both the
//! tile CDF arrays (copies of the specification's `Default_*_Cdf`) and the
//! `RefLrWiener` / `RefSgrXqd` reference values at the start of every tile, so
//! one [`LrTileState`] represents one tile.
//!
//! `read_lr` and `loop_restore_block` index the *same* `LrType` array. The
//! syntax side visits units in superblock order (`unitRowStart` uses a ceiling
//! division with no stripe shift), while the filter side looks units up with
//! `(row * MI_SIZE + 8) >> subY`. The eight-line shift only changes when a unit
//! is decoded relative to when it is consumed, not which grid entry it writes.

use crate::reference::av1::cdf_defaults::{
    DEFAULT_RESTORATION_TYPE_CDF, DEFAULT_USE_SGRPROJ_CDF, DEFAULT_USE_WIENER_CDF,
};
use crate::reference::av1::lr_frame::{LrFrameParams, LrPlaneGeometry, MI_SIZE};
use crate::reference::av1::restoration::{
    LrFilter, RestorationType, SGRPROJ_PARAMS_BITS, SGRPROJ_PRJ_BITS, SGRPROJ_PRJ_SUBEXP_K,
    SGRPROJ_XQD_MAX, SGRPROJ_XQD_MID, SGRPROJ_XQD_MIN, SGR_PARAMS, WIENER_TAPS_K, WIENER_TAPS_MAX,
    WIENER_TAPS_MID, WIENER_TAPS_MIN,
};
use crate::reference::av1::subexp::decode_signed_subexp_with_ref_bool;
use crate::reference::av1::symbol::{SymbolDecoder, SymbolError};

/// `SUPERRES_NUM`: numerator for the upscaling ratio.
pub const SUPERRES_NUM: u32 = 8;

/// Errors from the loop restoration syntax layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LrSyntaxError {
    Symbol(SymbolError),
    /// A `lr_sgr_set` outside `Sgr_Params`.
    SgrSetOutOfRange(u32),
    /// `TileRestorationTypeCdf` has three symbols; `RESTORE_SWITCHABLE` is not a
    /// legal per-unit type.
    BadRestorationType(usize),
    /// The syntax tried to write a unit outside the plane's grid.
    BadUnit {
        plane: usize,
        unit_row: usize,
        unit_col: usize,
    },
}

impl From<SymbolError> for LrSyntaxError {
    fn from(error: SymbolError) -> Self {
        Self::Symbol(error)
    }
}

/// Tile CDF arrays, copied from the specification defaults at `decode_tile()`.
/// Each array holds `N` cumulative probabilities followed by the adaptation
/// counter, exactly what [`SymbolDecoder::read_symbol`] consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LrTileCdf {
    /// `TileUseWienerCdf` = `Default_Use_Wiener_Cdf`.
    pub use_wiener: [u16; 3],
    /// `TileUseSgrprojCdf` = `Default_Use_Sgrproj_Cdf`.
    pub use_sgrproj: [u16; 3],
    /// `TileRestorationTypeCdf` = `Default_Restoration_Type_Cdf`.
    pub restoration_type: [u16; 4],
}

impl LrTileCdf {
    pub fn new() -> Self {
        let mut use_wiener = [0u16; 3];
        use_wiener.copy_from_slice(DEFAULT_USE_WIENER_CDF.values);
        let mut use_sgrproj = [0u16; 3];
        use_sgrproj.copy_from_slice(DEFAULT_USE_SGRPROJ_CDF.values);
        let mut restoration_type = [0u16; 4];
        restoration_type.copy_from_slice(DEFAULT_RESTORATION_TYPE_CDF.values);
        Self {
            use_wiener,
            use_sgrproj,
            restoration_type,
        }
    }

    /// Re-copy the defaults when a new tile starts.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

impl Default for LrTileCdf {
    fn default() -> Self {
        Self::new()
    }
}

/// `RefLrWiener[plane][pass][tap]` and `RefSgrXqd[plane][index]`, initialised
/// at `decode_tile()` to `Wiener_Taps_Mid` and `Sgrproj_Xqd_Mid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LrReferences {
    pub wiener: [[[i32; 3]; 2]; 3],
    pub sgr_xqd: [[i32; 2]; 3],
}

impl LrReferences {
    pub fn new() -> Self {
        Self {
            wiener: [[WIENER_TAPS_MID; 2]; 3],
            sgr_xqd: [SGRPROJ_XQD_MID; 3],
        }
    }
}

impl Default for LrReferences {
    fn default() -> Self {
        Self::new()
    }
}

/// `Clip3(low, high, value)`.
pub const fn clip3(low: i32, high: i32, value: i32) -> i32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

fn ceil_div(value: usize, divisor: usize) -> usize {
    debug_assert!(divisor > 0);
    (value + divisor - 1) / divisor
}

/// The half-open unit range visited by one syntax element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitRange {
    pub start: usize,
    pub end: usize,
}

/// `unitRowStart` / `unitRowEnd` from `read_lr( r, c, bSize )`.
pub fn unit_row_range(
    r4: usize,
    h4: usize,
    unit_rows: usize,
    unit_size: usize,
    sub_y: u32,
) -> UnitRange {
    let step = MI_SIZE >> sub_y;
    UnitRange {
        start: ceil_div(r4 * step, unit_size),
        end: ceil_div((r4 + h4) * step, unit_size).min(unit_rows),
    }
}

/// `unitColStart` / `unitColEnd`, including the `use_superres` numerator and
/// denominator. `superres_denom` is `Some(SuperresDenom)` when superresolution
/// is active.
pub fn unit_col_range(
    c4: usize,
    w4: usize,
    unit_cols: usize,
    unit_size: usize,
    sub_x: u32,
    superres_denom: Option<u32>,
) -> UnitRange {
    let (numerator, denominator) = match superres_denom {
        Some(denom) => (
            (MI_SIZE >> sub_x) as u32 * denom,
            unit_size as u32 * SUPERRES_NUM,
        ),
        None => ((MI_SIZE >> sub_x) as u32, unit_size as u32),
    };
    let numerator = numerator as usize;
    let denominator = denominator as usize;
    UnitRange {
        start: ceil_div(c4 * numerator, denominator),
        end: ceil_div((c4 + w4) * numerator, denominator).min(unit_cols),
    }
}

/// One pass (0 = vertical, 1 = horizontal) of `read_lr_unit`'s Wiener loop.
/// Chroma planes skip tap zero and set it to zero without updating the
/// reference value, as the specification requires.
pub fn decode_wiener_pass(
    decoder: &mut SymbolDecoder<'_>,
    plane: usize,
    pass: usize,
    refs: &mut LrReferences,
) -> Result<[i32; 3], LrSyntaxError> {
    let first_coeff = if plane != 0 { 1 } else { 0 };
    let mut taps = [0i32; 3];
    for j in first_coeff..3 {
        let value = decode_signed_subexp_with_ref_bool(
            decoder,
            WIENER_TAPS_MIN[j],
            WIENER_TAPS_MAX[j] + 1,
            WIENER_TAPS_K[j],
            refs.wiener[plane][pass][j],
        )?;
        taps[j] = value;
        refs.wiener[plane][pass][j] = value;
    }
    Ok(taps)
}

/// The SGRPROJ half of `read_lr_unit`: `lr_sgr_set` selects the radii, and a
/// zero radius short-circuits its xqd (the second one uses the complement of
/// the first reference, clipped to the transmitted range).
pub fn decode_sgrproj_xqd(
    decoder: &mut SymbolDecoder<'_>,
    plane: usize,
    set: usize,
    refs: &mut LrReferences,
) -> Result<[i32; 2], LrSyntaxError> {
    if set >= SGR_PARAMS.len() {
        return Err(LrSyntaxError::SgrSetOutOfRange(set as u32));
    }
    let mut xqd = [0i32; 2];
    for i in 0..2 {
        let radius = SGR_PARAMS[set][i * 2];
        let (min, max) = (SGRPROJ_XQD_MIN[i], SGRPROJ_XQD_MAX[i]);
        let value = if radius != 0 {
            decode_signed_subexp_with_ref_bool(
                decoder,
                min,
                max + 1,
                SGRPROJ_PRJ_SUBEXP_K,
                refs.sgr_xqd[plane][i],
            )?
        } else if i == 1 {
            clip3(min, max, (1 << SGRPROJ_PRJ_BITS) - refs.sgr_xqd[plane][0])
        } else {
            0
        };
        xqd[i] = value;
        refs.sgr_xqd[plane][i] = value;
    }
    Ok(xqd)
}

/// The symbol-selection half of `read_lr_unit`: `use_wiener`, `use_sgrproj` or
/// `restoration_type`, depending on `FrameRestorationType[plane]`.
pub fn read_restoration_type(
    decoder: &mut SymbolDecoder<'_>,
    cdf: &mut LrTileCdf,
    frame_type: RestorationType,
) -> Result<RestorationType, LrSyntaxError> {
    match frame_type {
        RestorationType::None => Ok(RestorationType::None),
        RestorationType::Wiener => {
            let symbol = decoder.read_symbol(&mut cdf.use_wiener)?;
            Ok(if symbol != 0 {
                RestorationType::Wiener
            } else {
                RestorationType::None
            })
        }
        RestorationType::SelfGuided => {
            let symbol = decoder.read_symbol(&mut cdf.use_sgrproj)?;
            Ok(if symbol != 0 {
                RestorationType::SelfGuided
            } else {
                RestorationType::None
            })
        }
        RestorationType::Switchable => {
            let symbol = decoder.read_symbol(&mut cdf.restoration_type)?;
            if symbol > 2 {
                return Err(LrSyntaxError::BadRestorationType(symbol));
            }
            Ok(RestorationType::from_unit_symbol(symbol))
        }
    }
}

/// The parameter half of `read_lr_unit`, separated so the subexp-coded taps
/// and xqd can be verified without first consuming a CDF symbol.
pub fn read_lr_params(
    decoder: &mut SymbolDecoder<'_>,
    plane: usize,
    restoration_type: RestorationType,
    refs: &mut LrReferences,
) -> Result<Option<LrFilter>, LrSyntaxError> {
    match restoration_type {
        RestorationType::Wiener => {
            let vertical = decode_wiener_pass(decoder, plane, 0, refs)?;
            let horizontal = decode_wiener_pass(decoder, plane, 1, refs)?;
            Ok(Some(LrFilter::Wiener {
                vertical,
                horizontal,
            }))
        }
        RestorationType::SelfGuided => {
            let set = decoder.read_literal(SGRPROJ_PARAMS_BITS)? as usize;
            let xqd = decode_sgrproj_xqd(decoder, plane, set, refs)?;
            Ok(Some(LrFilter::SelfGuided { set, xqd }))
        }
        RestorationType::None => Ok(None),
        RestorationType::Switchable => Err(LrSyntaxError::BadRestorationType(3)),
    }
}

/// All state `read_lr` needs for one tile. `geometry` carries the per-plane
/// `FrameRestorationType`, unit sizes and subsampling.
#[derive(Debug, Clone)]
pub struct LrTileState {
    pub cdf: LrTileCdf,
    pub refs: LrReferences,
    pub params: LrFrameParams,
    pub geometry: Vec<LrPlaneGeometry>,
}

impl LrTileState {
    pub fn new(geometry: Vec<LrPlaneGeometry>) -> Self {
        let params = LrFrameParams::for_geometry(&geometry);
        Self {
            cdf: LrTileCdf::new(),
            refs: LrReferences::new(),
            params,
            geometry,
        }
    }

    /// `read_lr( r, c, bSize )`: `r4`/`c4` are the superblock origin in 4x4
    /// luma units and `w4`/`h4` are `Num_4x4_Blocks_Wide/High[bSize]`.
    pub fn read_lr(
        &mut self,
        decoder: &mut SymbolDecoder<'_>,
        r4: usize,
        c4: usize,
        w4: usize,
        h4: usize,
        allow_intrabc: bool,
        superres_denom: Option<u32>,
    ) -> Result<(), LrSyntaxError> {
        if allow_intrabc {
            return Ok(());
        }
        for plane in 0..self.geometry.len() {
            let geometry = self.geometry[plane];
            if geometry.frame_type == RestorationType::None {
                continue;
            }
            let rows = unit_row_range(
                r4,
                h4,
                geometry.unit_rows,
                geometry.unit_size,
                geometry.sub_y,
            );
            let cols = unit_col_range(
                c4,
                w4,
                geometry.unit_cols,
                geometry.unit_size,
                geometry.sub_x,
                superres_denom,
            );
            for unit_row in rows.start..rows.end {
                for unit_col in cols.start..cols.end {
                    self.read_lr_unit(decoder, plane, unit_row, unit_col)?;
                }
            }
        }
        Ok(())
    }

    /// `read_lr_unit(plane, unitRow, unitCol)`.
    pub fn read_lr_unit(
        &mut self,
        decoder: &mut SymbolDecoder<'_>,
        plane: usize,
        unit_row: usize,
        unit_col: usize,
    ) -> Result<(), LrSyntaxError> {
        let restoration_type =
            read_restoration_type(decoder, &mut self.cdf, self.geometry[plane].frame_type)?;
        let filter = read_lr_params(decoder, plane, restoration_type, &mut self.refs)?;
        let geometry = self.geometry[plane];
        if !self
            .params
            .set(plane, unit_row, unit_col, filter, &geometry)
        {
            return Err(LrSyntaxError::BadUnit {
                plane,
                unit_row,
                unit_col,
            });
        }
        Ok(())
    }

    /// Hand the decoded grid to `apply_loop_restoration`.
    pub fn into_params(self) -> LrFrameParams {
        self.params
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::cdef::PlaneView;
    use crate::reference::av1::lr_frame::apply_loop_restoration;

    const ZEROS: [u8; 256] = [0u8; 256];

    fn luma(
        width: usize,
        height: usize,
        unit_size: usize,
        frame_type: RestorationType,
    ) -> Vec<LrPlaneGeometry> {
        vec![LrPlaneGeometry::new(
            height, width, unit_size, 0, 0, frame_type,
        )]
    }

    /// Force `read_symbol` to return `symbol` (with a high `SymbolValue`, i.e.
    /// an all-zero stream): zero CDF entries before it are skipped, and a
    /// 32768 entry at `symbol` makes `cur` small enough to stop there. This is
    /// an extreme, non-conforming CDF, so only use it when no further symbols
    /// are read from the same decoder.
    fn force_symbol(cdf: &mut [u16], symbol: usize) {
        let n = cdf.len() - 1;
        assert!(symbol < n);
        for (i, entry) in cdf.iter_mut().enumerate() {
            *entry = match i {
                i if i < symbol => 0,
                i if i == n => 0,
                _ => 32768,
            };
        }
    }

    #[test]
    fn tile_cdfs_are_copies_of_the_specification_defaults() {
        let cdf = LrTileCdf::new();
        assert_eq!(cdf.use_wiener.as_slice(), DEFAULT_USE_WIENER_CDF.values);
        assert_eq!(cdf.use_sgrproj.as_slice(), DEFAULT_USE_SGRPROJ_CDF.values);
        assert_eq!(
            cdf.restoration_type.as_slice(),
            DEFAULT_RESTORATION_TYPE_CDF.values
        );
        // Two symbols plus the adaptation counter; three symbols plus counter.
        assert_eq!(cdf.use_wiener.len() - 1, 2);
        assert_eq!(cdf.restoration_type.len() - 1, 3);
        assert_eq!(cdf.use_wiener[2], 0);
        assert_eq!(cdf.restoration_type[3], 0);

        let mut changed = cdf.clone();
        changed.use_wiener[0] = 1;
        changed.reset();
        assert_eq!(changed, cdf);
    }

    #[test]
    fn references_start_at_the_published_mid_values() {
        let refs = LrReferences::new();
        for plane in 0..3 {
            for pass in 0..2 {
                assert_eq!(refs.wiener[plane][pass], WIENER_TAPS_MID);
            }
            assert_eq!(refs.sgr_xqd[plane], SGRPROJ_XQD_MID);
        }
    }

    #[test]
    fn clip3_clamps_both_ends() {
        assert_eq!(clip3(-32, 95, 160), 95);
        assert_eq!(clip3(-32, 95, -100), -32);
        assert_eq!(clip3(-32, 95, 7), 7);
    }

    #[test]
    fn unit_row_range_uses_the_ceiling_division() {
        assert_eq!(
            unit_row_range(0, 32, 4, 64, 0),
            UnitRange { start: 0, end: 2 }
        );
        assert_eq!(
            unit_row_range(16, 16, 4, 64, 0),
            UnitRange { start: 1, end: 2 }
        );
        // Chroma halves the luma step: 32 MI rows cover 64 chroma samples.
        assert_eq!(
            unit_row_range(0, 32, 4, 64, 1),
            UnitRange { start: 0, end: 1 }
        );
        // The end is clamped to unitRows.
        assert_eq!(
            unit_row_range(0, 32, 1, 64, 0),
            UnitRange { start: 0, end: 1 }
        );
    }

    #[test]
    fn unit_col_range_handles_superres() {
        assert_eq!(
            unit_col_range(0, 32, 8, 64, 0, None),
            UnitRange { start: 0, end: 2 }
        );
        // SuperresDenom = 9: numerator = 4 * 9 = 36, denominator = 64 * 8 =
        // 512, so 32 MI columns reach ceil(32 * 36 / 512) = 3 units.
        assert_eq!(
            unit_col_range(0, 32, 8, 64, 0, Some(9)),
            UnitRange { start: 0, end: 3 }
        );
        // Without superres the same denominator would be 4 units.
        assert_eq!(
            unit_col_range(0, 32, 8, 32, 0, None),
            UnitRange { start: 0, end: 4 }
        );
    }

    #[test]
    fn zero_stream_selects_restore_none_for_a_wiener_frame() {
        let mut state = LrTileState::new(luma(64, 64, 64, RestorationType::Wiener));
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        state.read_lr_unit(&mut decoder, 0, 0, 0).unwrap();
        assert_eq!(state.params.planes[0][0], None);
        // The CDF adaptation counter records the one symbol read.
        assert_eq!(state.cdf.use_wiener[2], 1);
    }

    #[test]
    fn forced_use_wiener_symbol_and_mid_taps() {
        let mut cdf = LrTileCdf::new();
        force_symbol(&mut cdf.use_wiener, 1);
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        assert_eq!(
            read_restoration_type(&mut decoder, &mut cdf, RestorationType::Wiener).unwrap(),
            RestorationType::Wiener
        );

        // Parameter decoding uses a fresh decoder: an all-zero stream leaves
        // every subexp tap at its reference value, Wiener_Taps_Mid.
        let mut refs = LrReferences::new();
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        assert_eq!(
            read_lr_params(&mut decoder, 0, RestorationType::Wiener, &mut refs).unwrap(),
            Some(LrFilter::Wiener {
                vertical: WIENER_TAPS_MID,
                horizontal: WIENER_TAPS_MID,
            })
        );
        assert_eq!(refs.wiener[0][0], WIENER_TAPS_MID);
    }

    #[test]
    fn chroma_wiener_zeroes_the_first_tap_without_updating_its_reference() {
        let mut refs = LrReferences::new();
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        assert_eq!(
            read_lr_params(&mut decoder, 1, RestorationType::Wiener, &mut refs).unwrap(),
            Some(LrFilter::Wiener {
                vertical: [0, WIENER_TAPS_MID[1], WIENER_TAPS_MID[2]],
                horizontal: [0, WIENER_TAPS_MID[1], WIENER_TAPS_MID[2]],
            })
        );
        // Tap zero's reference is untouched: only j = 1, 2 are transmitted.
        assert_eq!(refs.wiener[1][0][0], WIENER_TAPS_MID[0]);
    }

    #[test]
    fn switchable_symbol_selects_none_wiener_or_self_guided() {
        for (symbol, expected) in [
            (0, RestorationType::None),
            (1, RestorationType::Wiener),
            (2, RestorationType::SelfGuided),
        ] {
            let mut cdf = LrTileCdf::new();
            force_symbol(&mut cdf.restoration_type, symbol);
            let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
            assert_eq!(
                read_restoration_type(&mut decoder, &mut cdf, RestorationType::Switchable).unwrap(),
                expected,
                "symbol {symbol}"
            );
        }

        // An all-zero stream pins both parameter sets on its own.
        let mut refs = LrReferences::new();
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        assert!(matches!(
            read_lr_params(&mut decoder, 0, RestorationType::Wiener, &mut refs).unwrap(),
            Some(LrFilter::Wiener { .. })
        ));
        let mut refs = LrReferences::new();
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        assert_eq!(
            read_lr_params(&mut decoder, 0, RestorationType::SelfGuided, &mut refs).unwrap(),
            Some(LrFilter::SelfGuided {
                set: 0,
                xqd: SGRPROJ_XQD_MID,
            })
        );
        assert_eq!(refs.sgr_xqd[0], SGRPROJ_XQD_MID);
    }

    #[test]
    fn zero_radii_use_the_complement_and_skip_the_subexp() {
        // Sgr_Params[14] = { 2, 30, 0, 0 }: the second radius is zero, so
        // xqd[1] = Clip3(min, max, (1 << 7) - RefSgrXqd[plane][0]) = 95.
        let mut refs = LrReferences::new();
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        let xqd = decode_sgrproj_xqd(&mut decoder, 0, 14, &mut refs).unwrap();
        assert_eq!(xqd[0], SGRPROJ_XQD_MID[0]);
        assert_eq!(xqd[1], SGRPROJ_XQD_MAX[1]);
        assert_eq!(refs.sgr_xqd[0], xqd);
        assert_eq!(
            decode_sgrproj_xqd(&mut decoder, 0, 16, &mut refs),
            Err(LrSyntaxError::SgrSetOutOfRange(16))
        );

        // Sgr_Params[10] = { 0, 0, 1, 5 }: the first radius is zero, so the
        // first xqd is exactly zero and only the second one is transmitted.
        let mut refs = LrReferences::new();
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        let xqd = decode_sgrproj_xqd(&mut decoder, 0, 10, &mut refs).unwrap();
        assert_eq!(xqd[0], 0);
        assert_eq!(xqd[1], SGRPROJ_XQD_MID[1]);
    }

    #[test]
    fn read_lr_visits_every_unit_of_a_superblock() {
        // 128x128 luma with 64-sample units -> 2 x 2 units.
        let mut state = LrTileState::new(luma(128, 128, 64, RestorationType::Wiener));
        assert_eq!(state.geometry[0].unit_rows, 2);
        assert_eq!(state.geometry[0].unit_cols, 2);
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        state
            .read_lr(&mut decoder, 0, 0, 32, 32, false, None)
            .unwrap();
        // One use_wiener symbol per unit, all four units visited.
        assert_eq!(state.cdf.use_wiener[2], 4);
        assert_eq!(state.params.planes[0].len(), 4);
    }

    #[test]
    fn read_lr_honours_intrabc_and_per_plane_types() {
        let mut geometry = luma(64, 64, 64, RestorationType::Wiener);
        geometry.push(LrPlaneGeometry::new(
            64,
            64,
            64,
            0,
            0,
            RestorationType::None,
        ));
        let mut state = LrTileState::new(geometry);
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        state
            .read_lr(&mut decoder, 0, 0, 16, 16, true, None)
            .unwrap();
        assert_eq!(
            state.cdf.use_wiener[2], 0,
            "allow_intrabc returns immediately"
        );

        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        state
            .read_lr(&mut decoder, 0, 0, 16, 16, false, None)
            .unwrap();
        assert_eq!(state.cdf.use_wiener[2], 1, "only the Y plane is read");
        assert_eq!(state.params.planes[1][0], None);
    }

    #[test]
    fn read_lr_superres_visits_more_columns() {
        let mut state = LrTileState::new(luma(256, 128, 64, RestorationType::Wiener));
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        state
            .read_lr(&mut decoder, 0, 0, 32, 32, false, None)
            .unwrap();
        let plain = state.cdf.use_wiener[2];

        let mut state = LrTileState::new(luma(256, 128, 64, RestorationType::Wiener));
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        state
            .read_lr(&mut decoder, 0, 0, 32, 32, false, Some(9))
            .unwrap();
        let superres = state.cdf.use_wiener[2];
        assert_eq!(plain, 2 * 2);
        assert_eq!(superres, 2 * 3);
    }

    #[test]
    fn read_lr_rejects_units_outside_the_grid() {
        let mut state = LrTileState::new(luma(64, 64, 64, RestorationType::Wiener));
        let mut decoder = SymbolDecoder::init(&ZEROS).unwrap();
        assert_eq!(
            state.read_lr_unit(&mut decoder, 0, 9, 0),
            Err(LrSyntaxError::BadUnit {
                plane: 0,
                unit_row: 9,
                unit_col: 0,
            })
        );
    }

    #[test]
    fn decoded_grid_drives_the_frame_filter() {
        let geometry = luma(8, 8, 64, RestorationType::Wiener);
        let mut state = LrTileState::new(geometry.clone());
        // An all-one stream selects use_wiener = 1 with the unmodified default
        // CDF, so the arithmetic decoder stays in a conforming state.
        let ones = [0xffu8; 256];
        let mut decoder = SymbolDecoder::init(&ones).unwrap();
        state
            .read_lr(&mut decoder, 0, 0, 2, 2, false, None)
            .unwrap();
        let params = state.into_params();
        assert!(params.planes[0][0].is_some());

        let cdef: Vec<u16> = (0..64).map(|v| (v * 7 % 251) as u16).collect();
        let current = vec![200u16; 64];
        let view = PlaneView::new(&cdef, 8, 8, 8);
        let current_view = PlaneView::new(&current, 8, 8, 8);
        let out = apply_loop_restoration(&[view], &[current_view], &geometry, &params, 8, 8, 8);
        assert_ne!(out[0], cdef, "the decoded Wiener filter must change pixels");
    }
}
