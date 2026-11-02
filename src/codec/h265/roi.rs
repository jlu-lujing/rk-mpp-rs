//! VEPU580 H.265 ROI tables.
//!
//! This follows the reference `mpp_enc_roi_utils.c` path for RK3588: regions
//! first populate the common 16x16 grid, the HEVC raster marks affected CTUs,
//! and `gen_vepu580_roi_h265` expands each CTU into the hardware's CU64, CU32,
//! CU16 and CU8 records. The two DMA surfaces are 64 bytes of base config and
//! 256 bytes of QP config per 64x64 CTU.

use std::fmt;

const MAX_REGIONS: usize = 8;
const BASE_CFG_BYTES: usize = 64;
const QP_CFG_BYTES: usize = 256;

const RASTER_TO_SCAN8: [usize; 64] = [
    0, 1, 4, 5, 16, 17, 20, 21, 2, 3, 6, 7, 18, 19, 22, 23, 8, 9, 12, 13, 24, 25, 28, 29, 10, 11,
    14, 15, 26, 27, 30, 31, 32, 33, 36, 37, 48, 49, 52, 53, 34, 35, 38, 39, 50, 51, 54, 55, 40, 41,
    44, 45, 56, 57, 60, 61, 42, 43, 46, 47, 58, 59, 62, 63,
];

const RASTER_TO_ZSCAN16: [usize; 16] = [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// One `RoiRegionCfg` entry. Geometry is the unsigned range of the C struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoiRegion {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub force_intra: bool,
    /// True selects absolute QP mode; false adjusts relative to the picture QP.
    pub qp_absolute: bool,
    pub qp_val: i16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoiTables {
    pub base: Vec<u8>,
    pub qp: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default)]
struct CommonCfg {
    force_intra: bool,
    qp_adj: i16,
    qp_absolute: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct BaseCfg {
    force_split: bool,
    force_intra: u8,
    qp_adj: bool,
}

/// Build the two surfaces consumed by `vepu580_h265_set_roi_regs`.
pub fn build_tables(
    width: usize,
    height: usize,
    regions: &[RoiRegion],
) -> Result<RoiTables, Error> {
    if width == 0 || height == 0 {
        return Err(Error("ROI needs a non-zero picture".into()));
    }
    if regions.len() > MAX_REGIONS {
        return Err(Error(format!(
            "{} ROI regions exceed the reference maximum of {MAX_REGIONS}",
            regions.len()
        )));
    }
    validate_regions(width, height, regions)?;

    let mb_w = align(width, 16) / 16;
    let mb_h = align(height, 16) / 16;
    let stride_h = align(mb_w, 4);
    let stride_v = align(mb_h, 4);
    let mut common = vec![CommonCfg::default(); stride_h * stride_v];
    let ctu_w = align(width, 64) / 64;
    let ctu_h = align(height, 64) / 64;
    let mut ctu_map = vec![false; ctu_w * ctu_h];

    for region in regions {
        let qp_adj = region.qp_val;
        let cfg = CommonCfg {
            force_intra: region.force_intra,
            qp_adj,
            qp_absolute: region.qp_absolute,
        };

        let roi_width = (i32::from(region.w) + 15) / 16;
        let roi_height = (i32::from(region.h) + 15) / 16;
        let pos_x_init = (i32::from(region.x) + 15) / 16;
        let pos_y_init = (i32::from(region.y) + 15) / 16;
        let pos_x_end = pos_x_init + roi_width;
        let pos_y_end = pos_y_init + roi_height;
        for y in pos_y_init..pos_y_end {
            for x in pos_x_init..pos_x_end {
                common[y as usize * stride_h + x as usize] = cfg;
            }
        }

        // The second pass is HEVC-specific in `gen_vepu54x_roi`. Its arithmetic
        // deliberately uses C truncating division and the odd `(w + 64) / 64`
        // expansion, not a conventional ceil-div.
        let mut roi_width = (i32::from(region.w) + 64) / 64;
        let mut roi_height = (i32::from(region.h) + 64) / 64;
        let mut pos_x_init;
        let mut pos_y_init;
        let x = i32::from(region.x);
        let y = i32::from(region.y);
        if x < 64 {
            pos_x_init = 0;
            roi_width += 2;
        } else if x % 64 != 0 {
            pos_x_init = (x - 64) / 64;
            roi_width += 2;
        } else {
            pos_x_init = x / 64;
        }
        if y < 64 {
            pos_y_init = 0;
            roi_height += 2;
        } else if y % 64 != 0 {
            pos_y_init = (y - 64) / 64;
            roi_height += 2;
        } else {
            pos_y_init = y / 64;
        }
        let pos_x_end = (pos_x_init + roi_width).min(ctu_w as i32);
        let pos_y_end = (pos_y_init + roi_height).min(ctu_h as i32);
        pos_x_init = pos_x_init.max(0);
        pos_y_init = pos_y_init.max(0);
        for y in pos_y_init..pos_y_end {
            for x in pos_x_init..pos_x_end {
                ctu_map[y as usize * ctu_w + x as usize] = true;
            }
        }
    }

    let mut base = vec![0u8; ctu_w * ctu_h * BASE_CFG_BYTES];
    let mut qp = vec![0u8; ctu_w * ctu_h * QP_CFG_BYTES];
    for ctu_y in 0..ctu_h {
        for ctu_x in 0..ctu_w {
            let ctu_index = ctu_y * ctu_w + ctu_x;
            if !ctu_map[ctu_index] {
                continue;
            }
            let base = &mut base[ctu_index * BASE_CFG_BYTES..(ctu_index + 1) * BASE_CFG_BYTES];
            let qp = &mut qp[ctu_index * QP_CFG_BYTES..(ctu_index + 1) * QP_CFG_BYTES];
            let mut adjusted = 0usize;

            for cu16 in 0..16usize {
                let cu16_x = ctu_x * 4 + (cu16 & 3);
                let cu16_y = ctu_y * 4 + cu16 / 4;
                let cfg = common[cu16_y * stride_h + cu16_x];
                let val = BaseCfg {
                    force_split: false,
                    force_intra: u8::from(cfg.force_intra),
                    qp_adj: cfg.qp_adj != 0,
                };
                if val.force_intra != 0 || val.qp_adj {
                    adjusted += 1;
                }

                for cu8 in 0..4usize {
                    let cu8_x = cu8 & 1;
                    let cu8_y = cu8 / 2;
                    let raster = (cu16 / 4 * 2 + cu8_y) * 8 + (cu16 & 3) * 2 + cu8_x;
                    set_cu8_base(base, RASTER_TO_SCAN8[raster], val);
                }
                set_cu16_base(base, RASTER_TO_ZSCAN16[cu16], val);
                set_qp(qp, 64 + RASTER_TO_ZSCAN16[cu16], cfg);

                if adjusted == 16 && cu16 == 15 {
                    set_cu64_base(base, val);
                    set_qp(qp, 84, cfg);
                    for cu32 in 0..4 {
                        set_cu32_base(base, cu32, val);
                        set_qp(qp, 80 + cu32, cfg);
                    }
                    for cu8 in 0..64 {
                        set_cu8_base(base, cu8, val);
                        set_qp(qp, cu8, cfg);
                    }
                } else if cu16 == 15 && adjusted > 0 {
                    let force = BaseCfg {
                        force_split: true,
                        ..val
                    };
                    set_cu64_base(base, force);
                    for cu32 in 0..4 {
                        set_cu32_base(base, cu32, force);
                    }
                    for cu16 in 0..16 {
                        set_cu16_base(base, cu16, force);
                    }
                }
            }
        }
    }

    Ok(RoiTables { base, qp })
}

fn validate_regions(width: usize, height: usize, regions: &[RoiRegion]) -> Result<(), Error> {
    for region in regions {
        if region.w == 0 || region.h == 0 {
            return Err(Error("ROI width and height must be non-zero".into()));
        }
        if u32::from(region.x) + u32::from(region.w) > width as u32
            || u32::from(region.y) + u32::from(region.h) > height as u32
        {
            return Err(Error(format!(
                "ROI [{}/{} {}x{}] exceeds {}x{}",
                region.x, region.y, region.w, region.h, width, height
            )));
        }
        if region.qp_absolute && !(0..=51).contains(&region.qp_val) {
            return Err(Error(format!(
                "absolute ROI QP {} is outside 0..=51",
                region.qp_val
            )));
        }
        if !region.qp_absolute && !(-51..=51).contains(&region.qp_val) {
            return Err(Error(format!(
                "relative ROI QP {} is outside -51..=51",
                region.qp_val
            )));
        }
    }
    Ok(())
}

fn set_cu8_base(buf: &mut [u8], index: usize, val: BaseCfg) {
    set_base_fields(buf, index, val);
}

fn set_cu16_base(buf: &mut [u8], index: usize, val: BaseCfg) {
    set_base_fields(buf, 64 + index, val);
}

fn set_cu32_base(buf: &mut [u8], index: usize, val: BaseCfg) {
    set_base_fields(buf, 80 + index, val);
}

fn set_cu64_base(buf: &mut [u8], val: BaseCfg) {
    set_base_fields(buf, 84, val);
}

fn set_base_fields(buf: &mut [u8], index: usize, val: BaseCfg) {
    set_bits(buf, 425 + index, u32::from(val.qp_adj), 1);
    set_bits(buf, 340 + index, u32::from(val.force_split), 1);
    set_bits(buf, 170 + index * 2, u32::from(val.force_intra), 2);
    set_bits(buf, index * 2, 0, 2);
}

fn set_qp(buf: &mut [u8], index: usize, cfg: CommonCfg) {
    let word = (((cfg.qp_adj as u16) & 0x7f) << 8) | (u16::from(cfg.qp_absolute) << 15);
    buf[index * 2..index * 2 + 2].copy_from_slice(&word.to_le_bytes());
}

fn set_bits(buf: &mut [u8], start: usize, value: u32, width: usize) {
    for bit in 0..width {
        let index = start + bit;
        let mask = 1u8 << (index % 8);
        if value & (1 << bit) != 0 {
            buf[index / 8] |= mask;
        } else {
            buf[index / 8] &= !mask;
        }
    }
}

fn align(value: usize, to: usize) -> usize {
    (value + to - 1) & !(to - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_roi_has_the_reference_surface_sizes() {
        let tables = build_tables(320, 240, &[]).unwrap();
        assert_eq!(tables.base.len(), 5 * 4 * 64);
        assert_eq!(tables.qp.len(), 5 * 4 * 256);
        assert!(tables.base.iter().all(|byte| *byte == 0));
        assert!(tables.qp.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_full_ctu_region_expands_to_cu64_and_cu32() {
        let region = RoiRegion {
            x: 0,
            y: 0,
            w: 64,
            h: 64,
            force_intra: true,
            qp_absolute: true,
            qp_val: 20,
        };
        let tables = build_tables(320, 240, &[region]).unwrap();
        let base = &tables.base[..64];
        let qp = &tables.qp[..256];

        assert_eq!(bit(base, 338), 1); // CU64 force_intra
        assert_eq!(bit(base, 509), 1); // CU64 qp_adj_en
        assert_eq!(bit(base, 424), 0); // all 16 are adjusted: no force-split
        for index in 0..64 {
            assert_eq!(bit(base, 425 + index), 1, "CU8 qp_adj {index}");
            assert_eq!(bit(base, 170 + index * 2), 1, "CU8 intra {index}");
        }
        for index in 0..85 {
            let word = u16::from_le_bytes([qp[index * 2], qp[index * 2 + 1]]);
            assert_eq!(word, (1 << 15) | (20 << 8), "QP record {index}");
        }
    }

    #[test]
    fn a_partial_region_forces_cu16_split() {
        let region = RoiRegion {
            x: 0,
            y: 0,
            w: 16,
            h: 16,
            force_intra: false,
            qp_absolute: false,
            qp_val: -3,
        };
        let tables = build_tables(320, 240, &[region]).unwrap();
        let base = &tables.base[..64];
        assert_eq!(bit(base, 424), 1);
        assert_eq!(bit(base, 420), 1);
        assert_eq!(bit(base, 404), 1);
    }

    #[test]
    fn regions_use_the_reference_bounds_checks() {
        let outside = RoiRegion {
            x: 300,
            y: 0,
            w: 32,
            h: 16,
            force_intra: false,
            qp_absolute: false,
            qp_val: 0,
        };
        assert!(build_tables(320, 240, &[outside]).is_err());

        let bad_absolute = RoiRegion {
            x: 0,
            y: 0,
            w: 16,
            h: 16,
            force_intra: false,
            qp_absolute: true,
            qp_val: 52,
        };
        assert!(build_tables(320, 240, &[bad_absolute]).is_err());
    }

    fn bit(buf: &[u8], index: usize) -> u8 {
        (buf[index / 8] >> (index % 8)) & 1
    }
}
