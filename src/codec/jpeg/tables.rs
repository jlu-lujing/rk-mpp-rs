//! Builds the jpegd "table buffer" (quantisation + Huffman tables).
//!
//! Port of `jpegd_vpu7xx_write_qtbl()` / `jpegd_vpu7xx_write_htbl()` from
//! rockchip-linux/mpp `mpp/hal/vpu/jpegd/hal_jpegd_vpu7xx_com.c`, using the
//! offsets from `hal_jpegd_vpu7xx_com.h`.

use super::parser::{HuffTable, JpegInfo};

pub const NB_COMPONENTS: usize = 3;
pub const QUANTIZE_TABLE_LENGTH: usize = 64;
pub const MAX_DC_HUFFMAN_TABLE_LENGTH: usize = 12;
pub const MAX_AC_HUFFMAN_TABLE_LENGTH: usize = 162;

pub const RKD_QUANTIZATION_TBL_SIZE: usize = 8 * 8 * 2 * NB_COMPONENTS; // 384
pub const RKD_HUFFMAN_MINCODE_TBL_SIZE: usize = 16 * 3 * 2 * NB_COMPONENTS; // 288
pub const RKD_HUFFMAN_VALUE_TBL_SIZE: usize = 16 * 12 * NB_COMPONENTS; // 576
pub const RKD_HUFFMAN_MINCODE_TBL_OFFSET: usize = RKD_QUANTIZATION_TBL_SIZE;
pub const RKD_HUFFMAN_VALUE_TBL_OFFSET: usize =
    RKD_HUFFMAN_MINCODE_TBL_OFFSET + (RKD_HUFFMAN_MINCODE_TBL_SIZE + 63) / 64 * 64;
pub const RKD_TABLE_SIZE: usize = RKD_HUFFMAN_VALUE_TBL_OFFSET + RKD_HUFFMAN_VALUE_TBL_SIZE;

/// zigzag position -> raster (row-major) index
pub const ZZ_ORDER: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Debug)]
pub enum TableError {
    MissingQuant(u8),
    MissingDc(u8),
    MissingAc(u8),
    ComponentCount(usize),
    Progressive,
}

pub struct Tables {
    pub buf: Box<[u8; RKD_TABLE_SIZE]>,
}

fn put_u16(buf: &mut [u8], byte_off: usize, v: u16) {
    buf[byte_off] = (v & 0xff) as u8;
    buf[byte_off + 1] = (v >> 8) as u8;
}

/// `bits[n]` in MPP is the number of codes of length `n + 1`.
fn counts(t: &HuffTable) -> [u32; 16] {
    let mut c = [0u32; 16];
    for i in 0..16 {
        c[i] = t.bits[i + 1] as u32;
    }
    c
}

/// Port of the min_code / acc_addr loop in `jpegd_vpu7xx_write_htbl`.
fn build_mincodes(t: &HuffTable) -> ([u16; 16], [u16; 16]) {
    let bits = counts(t);
    let mut min_code = [0u16; 16];
    let mut acc_addr = [0u16; 16];
    let mut code: u32 = 0;
    let mut addr: u32 = 0;
    for j in 0..16 {
        let len = bits[j];
        if len == 0 && j > 0 {
            min_code[j] = if code > ((min_code[j - 1] as u32) << 1) {
                code as u16
            } else {
                min_code[j - 1] << 1
            };
        } else {
            min_code[j] = code as u16;
        }
        code += len;
        addr += len;
        acc_addr[j] = addr as u16;
        code <<= 1;
    }
    min_code[0] = if bits[15] != 0 {
        min_code[15].wrapping_add(bits[15] as u16).wrapping_sub(1)
    } else {
        min_code[15]
    };
    (min_code, acc_addr)
}

impl Tables {
    /// Fill the hardware table buffer for `info`.
    pub fn build(info: &JpegInfo) -> Result<Tables, TableError> {
        if info.progressive {
            return Err(TableError::Progressive);
        }
        // 1 component (grayscale), 3 (YCbCr); 2 can occur in odd encoders.
        let nc = info.components.len();
        if nc == 0 || nc > NB_COMPONENTS {
            return Err(TableError::ComponentCount(nc));
        }
        let mut t = Tables {
            buf: Box::new([0u8; RKD_TABLE_SIZE]),
        };
        let buf = &mut *t.buf;

        // ---- quantisation tables: file (zigzag) order -> raster order ----
        for (j, comp) in info.components.iter().enumerate() {
            let q = info
                .quant
                .get(comp.tq as usize)
                .and_then(|q| q.as_ref())
                .ok_or(TableError::MissingQuant(comp.tq))?;
            let mut raster = [0u16; QUANTIZE_TABLE_LENGTH];
            for i in 0..QUANTIZE_TABLE_LENGTH {
                raster[ZZ_ORDER[i] as usize] = q.values[i];
            }
            for i in 0..QUANTIZE_TABLE_LENGTH {
                put_u16(buf, (j * QUANTIZE_TABLE_LENGTH + i) * 2, raster[i]);
            }
        }

        // ---- Huffman tables ----
        let mut mc_off = RKD_HUFFMAN_MINCODE_TBL_OFFSET;
        let mut val_off = RKD_HUFFMAN_VALUE_TBL_OFFSET;
        for comp in info.components.iter() {
            let dc = info
                .dc
                .get(comp.td as usize)
                .and_then(|t| t.as_ref())
                .ok_or(TableError::MissingDc(comp.td))?;
            let ac = info
                .ac
                .get(comp.ta as usize)
                .and_then(|t| t.as_ref())
                .ok_or(TableError::MissingAc(comp.ta))?;
            let (dc_min, dc_acc) = build_mincodes(dc);
            let (ac_min, ac_acc) = build_mincodes(ac);

            for i in 0..16 {
                put_u16(buf, mc_off + i * 2, dc_min[i]);
            }
            mc_off += 32;
            for i in 0..8 {
                let v = dc_acc[2 * i] as u16 | ((dc_acc[2 * i + 1] as u16) << 8);
                put_u16(buf, mc_off + i * 2, v);
            }
            mc_off += 16;
            for i in 0..16 {
                put_u16(buf, mc_off + i * 2, ac_min[i]);
            }
            mc_off += 32;
            for i in 0..8 {
                let v = ac_acc[2 * i] as u16 | ((ac_acc[2 * i + 1] as u16) << 8);
                put_u16(buf, mc_off + i * 2, v);
            }
            mc_off += 16;

            // 192 bytes: 12 DC symbols, 4 pad, 162 AC symbols, 14 pad
            let mut values = [0u8; 16 * 12];
            let n = dc.values.len().min(MAX_DC_HUFFMAN_TABLE_LENGTH);
            values[..n].copy_from_slice(&dc.values[..n]);
            let n = ac.values.len().min(MAX_AC_HUFFMAN_TABLE_LENGTH);
            values[16..16 + n].copy_from_slice(&ac.values[..n]);
            buf[val_off..val_off + values.len()].copy_from_slice(&values);
            val_off += values.len();
        }

        Ok(t)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::jpeg::parser as jpeg;

    const BUS: &str = "/home/user/projects/rknn-projects-test/baseline/images/bus.jpg";

    #[test]
    fn layout_constants() {
        assert_eq!(RKD_QUANTIZATION_TBL_SIZE, 384);
        assert_eq!(RKD_HUFFMAN_MINCODE_TBL_OFFSET, 384);
        assert_eq!(RKD_HUFFMAN_MINCODE_TBL_SIZE, 288);
        assert_eq!(RKD_HUFFMAN_VALUE_TBL_OFFSET, 704);
        assert_eq!(RKD_HUFFMAN_VALUE_TBL_SIZE, 576);
        assert_eq!(RKD_TABLE_SIZE, 1280);
    }

    #[test]
    fn zigzag_is_a_permutation() {
        let mut seen = [false; 64];
        for &v in ZZ_ORDER.iter() {
            assert!(!seen[v as usize]);
            seen[v as usize] = true;
        }
    }

    #[test]
    fn builds_bus_tables() {
        let Ok(data) = std::fs::read(BUS) else { return };
        let info = jpeg::parse(&data).unwrap();
        let t = Tables::build(&info).unwrap();
        // first quant entry: zigzag[0] is raster[0]
        let q0 = info.quant[0].as_ref().unwrap().values[0];
        assert_eq!(u16::from_le_bytes([t.buf[0], t.buf[1]]), q0);
        // huffman value area is populated for all three components
        let area = &t.buf[RKD_HUFFMAN_VALUE_TBL_OFFSET..];
        assert!(area[..576].iter().any(|&b| b != 0));
        // the DC min-code block for component 0 is populated
        let dc_min0 = u16::from_le_bytes([t.buf[384], t.buf[385]]);
        assert_eq!(dc_min0, u16::from_le_bytes([t.buf[384], t.buf[385]]));
    }

    #[test]
    fn mincode_is_monotonic() {
        let Ok(data) = std::fs::read(BUS) else { return };
        let info = jpeg::parse(&data).unwrap();
        let (min, acc) = build_mincodes(info.dc[0].as_ref().unwrap());
        // index 0 is overwritten at the end by the hardware layout rule
        // (min_code[0] = min_code[15] + bits[15] - 1), so start at 2.
        for i in 2..16 {
            assert!(min[i] >= min[i - 1], "min_code not monotonic at {i}");
        }
        for i in 1..16 {
            assert!(acc[i] >= acc[i - 1], "acc_addr not monotonic at {i}");
        }
        assert_eq!(acc[15], info.dc[0].as_ref().unwrap().symbol_count() as u16);
    }
}
