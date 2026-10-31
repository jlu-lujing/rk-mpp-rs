//! jpegd register map, generated from rockchip-linux/mpp
//! `mpp/hal/vpu/jpegd/hal_jpegd_rkv_reg.h` (JpegRegSet, 42 words).
//!
//! Do not edit by hand; regenerate from the header if the layout changes.

pub const JPEGD_REG_NUM: usize = 42;

/// (register index, high bit, low bit)
pub type Field = (usize, u32, u32);

pub const MINOR_VER: Field = (0, 7, 0);
pub const BIT_DEPTH: Field = (0, 8, 8);
pub const PROD_NUM: Field = (0, 31, 16);
pub const DEC_E: Field = (1, 0, 0);
pub const DEC_IRQ_DIS: Field = (1, 1, 1);
pub const DEC_TIMEOUT_E: Field = (1, 2, 2);
pub const BUF_EMPTY_E: Field = (1, 3, 3);
pub const BUF_EMPTY_RELOAD_P: Field = (1, 4, 4);
pub const SOFT_RST_EN_P: Field = (1, 5, 5);
pub const DEC_IRQ_RAW: Field = (1, 6, 6);
pub const WAIT_RESET_E: Field = (1, 7, 7);
pub const DEC_IRQ: Field = (1, 8, 8);
pub const DEC_RDY_STA: Field = (1, 9, 9);
pub const DEC_BUS_STA: Field = (1, 10, 10);
pub const DEC_ERROR_STA: Field = (1, 11, 11);
pub const DEC_TIMEOUT_STA: Field = (1, 12, 12);
pub const DEC_BUF_EMPTY_STA: Field = (1, 13, 13);
pub const SOFT_REST_RDY: Field = (1, 14, 14);
pub const BUF_EMPTY_FORCE_END_FLAG: Field = (1, 15, 15);
pub const CARE_STRM_ERROR_E: Field = (1, 16, 16);
pub const LOW_DELAY_OUT_STA: Field = (1, 17, 17);
pub const LKT_OPERATION_ERROR_STA: Field = (1, 18, 18);
pub const LKT_MODE_INT_STA: Field = (1, 19, 19);
pub const LKT_FORCE_STOP_STA: Field = (1, 20, 20);
pub const LKT_NODE_INT_STA: Field = (1, 21, 21);
pub const LKT_ERR_STOP_STA: Field = (1, 22, 22);
pub const LKT_DATA_ERR_STA: Field = (1, 23, 23);
pub const IN_ENDIAN: Field = (2, 0, 0);
pub const IN_SWAP32_E: Field = (2, 1, 1);
pub const IN_SWAP64_E: Field = (2, 2, 2);
pub const STR_ENDIAN: Field = (2, 3, 3);
pub const STR_SWAP32_E: Field = (2, 4, 4);
pub const STR_SWAP64_E: Field = (2, 5, 5);
pub const OUT_ENDIAN: Field = (2, 6, 6);
pub const OUT_SWAP32_E: Field = (2, 7, 7);
pub const OUT_SWAP64_E: Field = (2, 8, 8);
pub const OUT_CBCR_SWAP: Field = (2, 9, 9);
pub const OUT_BYTE_SWAP: Field = (2, 10, 10);
pub const SCALEDOWN_MODE: Field = (2, 13, 12);
pub const TIME_OUT_MODE: Field = (2, 16, 16);
pub const FORCE_SOFTREST_VALID: Field = (2, 17, 17);
pub const FBC_E: Field = (2, 20, 20);
pub const ALLOW_16X8_CP_FLAG: Field = (2, 21, 21);
pub const FBC_FORCE_UNCOMPRESS: Field = (2, 22, 22);
pub const BGR_SEQUENCE: Field = (2, 23, 23);
pub const FILL_DOWN_E: Field = (2, 24, 24);
pub const FILL_RIGHT_E: Field = (2, 25, 25);
pub const DEC_OUT_SEQUENCE: Field = (2, 26, 26);
pub const YUV_OUT_FORMAT: Field = (2, 29, 27);
pub const YUV2RGB_REC: Field = (2, 30, 30);
pub const YUV2RGB_RANGE: Field = (2, 31, 31);
pub const PIC_WIDTH_M1: Field = (3, 15, 0);
pub const PIC_HEIGHT_M1: Field = (3, 31, 16);
pub const JPEG_MODE: Field = (4, 2, 0);
pub const PIXEL_DEPTH: Field = (4, 6, 4);
pub const QTABLES_SEL: Field = (4, 9, 8);
pub const HTABLES_SEL: Field = (4, 13, 12);
pub const DRI_E: Field = (4, 15, 15);
pub const DRI_MCU_NUM_M1: Field = (4, 31, 16);
pub const Y_HOR_VIRSTRIDE: Field = (5, 15, 0);
pub const UV_HOR_VIRSTRIDE: Field = (5, 31, 16);
pub const Y_VIRSTRIDE: Field = (6, 31, 4);
pub const QTBL_LEN: Field = (7, 4, 0);
pub const HTBL_MINCODE_LEN: Field = (7, 12, 8);
pub const HTBL_VALUE_LEN: Field = (7, 21, 16);
pub const Y_HOR_VIRSTRIDE_H: Field = (7, 24, 24);
pub const STRM_START_BYTE: Field = (8, 3, 0);
pub const STREAM_LEN: Field = (8, 31, 4);
pub const ERROR_PRC_MODE: Field = (14, 0, 0);
pub const STRM_R0_ERR_MODE: Field = (14, 2, 1);
pub const STRM_R1_ERR_MODE: Field = (14, 4, 3);
pub const STRM_FFFF_ERR_MODE: Field = (14, 6, 5);
pub const STRM_OTHER_MARK_MODE: Field = (14, 8, 7);
pub const STRM_DRI_SEQ_ERR_MODE: Field = (14, 9, 9);
pub const HFM_FORCE_STOP: Field = (14, 20, 16);
pub const STRM_R0_MARKER: Field = (15, 7, 0);
pub const STRM_R0_MASK: Field = (15, 15, 8);
pub const STRM_R1_MARKER: Field = (15, 23, 16);
pub const STRM_R1_MASK: Field = (15, 31, 24);
pub const DEC_CLKGATE_E: Field = (16, 0, 0);
pub const DEC_STRM_GATE_E: Field = (16, 1, 1);
pub const DEC_HUFFMAN_GATE_E: Field = (16, 2, 2);
pub const DEC_IZQ_GATE_E: Field = (16, 3, 3);
pub const DEC_IDCT_GATE_E: Field = (16, 4, 4);
pub const BUSIFD_GATE_E: Field = (16, 5, 5);
pub const POST_PRS_GATE_E: Field = (16, 6, 6);
pub const DEC_SRAM_GATE_E: Field = (16, 7, 7);
pub const RESERVED_19: Field = (19, 31, 0);
pub const RESERVED_20: Field = (20, 31, 0);
pub const RESERVED_21: Field = (21, 31, 0);
pub const RESERVED_22: Field = (22, 31, 0);
pub const RESERVED_23: Field = (23, 31, 0);
pub const RESERVED_24: Field = (24, 31, 0);
pub const RESERVED_25: Field = (25, 31, 0);
pub const RESERVED_26: Field = (26, 31, 0);
pub const RESERVED_27: Field = (27, 31, 0);
pub const RESERVED_28: Field = (28, 31, 0);
pub const RESERVED_29: Field = (29, 31, 0);
pub const AXI_PERF_WORK_E: Field = (30, 0, 0);
pub const AXI_PERF_CLR_E: Field = (30, 1, 1);
pub const AXI_PERF_FRM_TYPE: Field = (30, 2, 2);
pub const AXI_CNT_TYPE: Field = (30, 3, 3);
pub const RD_LATENCY_ID: Field = (30, 7, 4);
pub const RD_LATENCY_THR: Field = (30, 19, 8);
pub const ADDR_ALIGN_TYPE: Field = (31, 1, 0);
pub const AR_CNT_ID_TYPE: Field = (31, 2, 2);
pub const AW_CNT_ID_TYPE: Field = (31, 3, 3);
pub const AR_COUNT_ID: Field = (31, 7, 4);
pub const AW_COUNT_ID: Field = (31, 11, 8);
pub const RD_TOTOAL_BYTES_MODE: Field = (31, 12, 12);
pub const MCU_POS_X: Field = (32, 15, 0);
pub const MCU_POS_Y: Field = (32, 31, 16);
pub const RD_MAX_LATENCY_NUM_CH0: Field = (34, 15, 0);
pub const BUS_STATUS_FLAG: Field = (40, 18, 0);
pub const WORK_STATUS_FLAG: Field = (41, 17, 0);

/// 42-word command register file with typed field access.
#[derive(Clone, Copy, Debug)]
pub struct Regs {
    pub words: [u32; JPEGD_REG_NUM],
}

impl Default for Regs {
    fn default() -> Self {
        Regs {
            words: [0; JPEGD_REG_NUM],
        }
    }
}

impl Regs {
    pub fn set(&mut self, f: Field, v: u32) {
        let (r, hi, lo) = f;
        let width = hi - lo + 1;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            ((1u32 << width) - 1) << lo
        };
        self.words[r] = (self.words[r] & !mask) | ((v << lo) & mask);
    }
    pub fn get(&self, f: Field) -> u32 {
        let (r, hi, lo) = f;
        let width = hi - lo + 1;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        (self.words[r] >> lo) & mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_field_positions() {
        assert_eq!(DEC_E, (1, 0, 0));
        assert_eq!(YUV_OUT_FORMAT, (2, 29, 27));
        assert_eq!(QTBL_LEN, (7, 4, 0));
        assert_eq!(HTBL_VALUE_LEN, (7, 21, 16));
    }

    #[test]
    fn set_and_get_roundtrip() {
        let mut r = Regs::default();
        r.set(DEC_E, 1);
        r.set(YUV_OUT_FORMAT, 5);
        assert_eq!(r.get(DEC_E), 1);
        assert_eq!(r.get(YUV_OUT_FORMAT), 5);
        assert_eq!(r.words[1] & 1, 1);
        assert_eq!(r.words[2] >> 27, 5);
    }
}
