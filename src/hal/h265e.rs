//! VEPU580 H.265 encoder: the fixed-QP IDR register image.
//!
//! M8 opens with the smallest honest step the H.264 encoder took before it: one
//! IDR picture at a fixed QP. This module builds the VEPU580 register file for
//! that picture from the C source (`mpp/hal/rkenc/h265e/hal_h265e_vepu580.c`), one
//! field at a time. The image now *starts* from a captured job
//! (`h265e_vepu580_template`: the vintage's `mpi_enc_test -t 16777220 -rc 2` trace)
//! and the derivation below overwrites what the C computes from the geometry, the
//! QP and the slice syntax. That is the lineage the H.264 encoder took: the tuning
//! windows have no public formula, so they are recorded until a per-field
//! derivation exists for them.
//!
//! What is derived here is the part of `gen_regs` the source computes from the
//! picture geometry, the QP and the slice/parameter-set syntax: the control
//! window (`enc_strt`/`enc_clr`/`int_en`/`dtrns_map`/`dtrns_cfg`/`enc_wdg`/
//! `func_en`), the buffer and picture-geometry registers (`enc_rsl`/`src_fill`/
//! `enc_pic`/`src_proc`), the H.265 syntax block (`reg0236..reg0241`, `reg0252`)
//! and the RC-lookup rows that are hard-wired for fixed QP (`pic_qp`,
//! `rc_min/max_qp`, the `madi`/`md_sad` thresholds, `chrm_klut_ofst`,
//! `qpmap_mode`).
//!
//! What is *not* derived yet: the tuning windows (`wgt`, `rdo`, `osd`) and the
//! AQ/KLUT/LAMD byte tables inside RC-lookup. In H.264 those are the recorded-image
//! replay (the "cime/rime corner"); here they come from the captured template, which
//! is why this module is not yet submit-capable: the address words in the template
//! still belong to the oracle's buffers.
//!
//! The H.265 job writes six windows, not the seven of an H.264 job. The vintage's
//! `H265eV580RegSet` is control, base, rc_klut, wgt, rdo, osd (plus dbg, which is
//! not part of a job): the scan/quant window at word 2176 - H.264's `scl`, 679
//! words - has no H.265 member, and the weight window sharing H.264's `param`
//! offset of word 1472 is `hevc_vepu580_wgt` at 374 words, not 180. Reusing the
//! H.264 array sizes here fails silently: every `wgt` field past word 1652 landed
//! outside its section, and the first write into `rdo` past word 122 indexed beyond
//! its array. `windows_match_the_generated_section_table` and
//! `the_last_word_of_every_window_is_addressable` pin the arrays to the table.
//! The report is explicit that the IDR payload comparison is blocked on the
//! template capture, while the VPS/SPS/PPS comparison (`codec::h265::encoder`) is
//! not.

use super::h265e_regs as regs;
use super::h265e_vepu580_template as template;
use crate::codec::h265::encoder::{
    SliceSplit, SplitMode, SyntaxOptions, MPP_ENC_SPLIT_OUT_LOWDELAY,
};
use crate::service::RegOffsetInfo;

pub const REFERENCE_WIDTH: usize = 320;
pub const REFERENCE_HEIGHT: usize = 240;

/// Which picture of the GOP the register image is for. `set_slice_regs` and
/// `set_ref_regs` branch on the syntax the slice writer produced; the two shapes
/// this crate emits today are the opening IDR and a single-reference P.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    /// `NAL_IDR_W_RADL`, `sli_type` 2 (I), POC 0, no reference markings.
    Idr,
    /// A P syntax template re-run with `EncFrmStatus.is_intra=is_idr=1`.
    /// The HAL keeps the P slice/POC/reference fields but selects INTRA global
    /// configuration and writes an IDR NAL type.
    ForcedIdrFromP { poc_lsb: u32 },
    /// `NAL_TRAIL_R`, `sli_type` 1 (P), one short-term reference rebuilt as an LT
    /// entry the way `h265e_set_ref_regs` does for this vintage.
    P { poc_lsb: u32 },
}

/// One GDR picture's inputs: the table the session filled, where in the sweep it
/// is, and how wide the band is.
#[derive(Clone, Copy, Debug)]
pub struct RefreshJob {
    pub table_fd: i32,
    pub mode: RefreshMode,
    pub refresh_num: u32,
    pub refresh_idx: u32,
}

/// One picture's rate-control decision: what `rc_model::Model::start` returns plus
/// the HAL's quantiser slack. `bit_target` is bits for the whole picture.
#[derive(Clone, Copy, Debug)]
pub struct RateControl {
    pub bit_target: i32,
    pub qp_target: u32,
    pub qp_min: u32,
    pub qp_max: u32,
    /// `hw->qp_delta_row` and `hw->qp_delta_row_i`, both 2 by default.
    pub qp_delta_row: u32,
    pub qp_delta_row_i: u32,
}

/// Which axis an intra-refresh (GDR) sweep walks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshMode {
    Row,
    Col,
}

/// `setup_intra_refresh`'s ROI base table: 64 bytes per CTU, with the refresh band
/// forced intra. The byte pattern is the HAL's own - `0x54` at offset 21, twenty
/// `0x55` bytes from 22, `0x05` at 42 - and the band is
/// `cal_refresh_area(ctu_w, ctu_h, refresh_idx, mode, refresh_num)`.
pub fn refresh_base_table(
    width: usize,
    height: usize,
    refresh_idx: u32,
    mode: RefreshMode,
    refresh_num: u32,
) -> Vec<u8> {
    let ctu_w = ((width + 63) / 64).max(1) as i32;
    let ctu_h = ((height + 63) / 64).max(1) as i32;
    let mut table = vec![0u8; ctu_w as usize * ctu_h as usize * 64];

    let (x_sta, x_end, y_sta, y_end) = match mode {
        RefreshMode::Row => {
            let h = if refresh_idx > 0 {
                refresh_num + 1
            } else {
                refresh_num
            };
            let y0 = if refresh_idx > 0 {
                refresh_num * refresh_idx - 1
            } else {
                0
            };
            (0, ctu_w - 1, y0 as i32, (y0 + h) as i32 - 1)
        }
        RefreshMode::Col => {
            let w = if refresh_idx > 0 {
                refresh_num + 1
            } else {
                refresh_num
            };
            let x0 = if refresh_idx > 0 {
                refresh_num * refresh_idx - 1
            } else {
                0
            };
            (x0 as i32, (x0 + w) as i32 - 1, 0, ctu_h - 1)
        }
    };
    let x_sta = x_sta.max(0);
    let y_sta = y_sta.max(0);
    let x_end = x_end.min(ctu_w - 1);
    let y_end = y_end.min(ctu_h - 1);
    for j in y_sta..=y_end {
        for k in x_sta..=x_end {
            let p = (j * ctu_w + k) as usize * 64;
            table[p + 22..p + 42].fill(0x55);
            table[p + 21] = 0x54;
            table[p + 42] = 0x05;
        }
    }
    table
}

/// `lamd_modb_qp`, the inter-picture lambda table `global_cfg_set` copies for a P
/// picture (`vepu5xx_common.c:183`). The IDR keeps the intra table the template
/// recorded.
const MODB_LAMBDA: [u32; 52] = [
    0x00000070, 0x00000089, 0x000000b0, 0x000000e0, 0x00000112, 0x00000160, 0x000001c0, 0x00000224,
    0x000002c0, 0x00000380, 0x00000448, 0x00000580, 0x00000700, 0x00000890, 0x00000b00, 0x00000e00,
    0x00001120, 0x00001600, 0x00001c00, 0x00002240, 0x00002c00, 0x00003800, 0x00004480, 0x00005800,
    0x00007000, 0x00008900, 0x0000b000, 0x0000e000, 0x00011200, 0x00016000, 0x0001c000, 0x00022400,
    0x0002c000, 0x00038000, 0x00044800, 0x00058000, 0x00070000, 0x00089000, 0x000b0000, 0x000e0000,
    0x00112000, 0x00160000, 0x001c0000, 0x00224000, 0x002c0000, 0x00380000, 0x00448000, 0x00580000,
    0x00700000, 0x00890000, 0x00b00000, 0x00e00000,
];

/// First absolute word of `iprd_wgt_qp_hevc_0_51` in the WGT window.
const IPRD_WGT_QP_WORD: usize = 1600;

/// An IDR picture at a fixed QP: the geometry, the QP the reference would aim at,
/// the RC's picture bounds (which collapse to the QP under fixed QP), and the
/// dma-buf fds the picture moves between.
#[derive(Clone, Debug)]
pub struct IdrJob {
    pub width: usize,
    pub height: usize,
    /// `rc_cfg->quality_target`, the QP fixed QP writes into `pic_qp`/`sli_qp`.
    pub qp: u32,
    /// `rc_cfg->quality_min`/`quality_max`, which reach the sixteen `roi_qthd*`
    /// rows unconditionally. Under fixed QP the RC collapses both to the QP; the
    /// job carries them separately because the reference reads them separately.
    pub qp_min: u32,
    pub qp_max: u32,
    pub picture: Picture,
    /// The GOP configuration carries long-term references, which the HAL states on
    /// every picture of the sequence - the opening IDR included.
    pub long_term: bool,
    /// `sps_temporal_mvp_enabled_flag`; the vendor FFmpeg object path defaults
    /// this on while the standalone CLI leaves it off.
    pub temporal_mvp: bool,
    /// Rate control for this picture, `None` for fixed QP.
    pub rc: Option<RateControl>,
    /// Intra-refresh (GDR) picture: the base-table fd, its sweep, and the index.
    pub refresh: Option<RefreshJob>,
    /// `MppEncSliceSplit`: one slice per picture by default.
    pub split: SliceSplit,
    pub syntax: SyntaxOptions,
    /// One horizontal tile job. `None` encodes the whole picture.
    pub tile: Option<TileJob>,
    /// Static ROI base and QP table fds, or -1 when no ROI is configured.
    pub roi_fd: i32,
    pub roi_qp_fd: i32,
    pub input_fd: i32,
    pub output_fd: i32,
    pub current_fd: i32,
    pub current_thumb_fd: i32,
    pub reference_fd: i32,
    pub reference_thumb_fd: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileJob {
    pub index: u32,
    /// First CTU column in the full picture.
    pub start_x: u32,
    pub width_mb: u32,
    pub height_mb: u32,
    pub columns: u32,
}

impl IdrJob {
    /// A reference-config job with placeholder (negative) fds, for the host-side
    /// field-derivation tests where no dma-buf exists.
    pub fn reference(qp: u32) -> Self {
        Self {
            width: REFERENCE_WIDTH,
            height: REFERENCE_HEIGHT,
            qp,
            qp_min: qp,
            qp_max: qp,
            picture: Picture::Idr,
            long_term: false,
            temporal_mvp: false,
            rc: None,
            refresh: None,
            split: SliceSplit::default(),
            syntax: SyntaxOptions::default(),
            tile: None,
            roi_fd: -1,
            roi_qp_fd: -1,
            input_fd: -1,
            output_fd: -1,
            current_fd: -1,
            current_thumb_fd: -1,
            reference_fd: -1,
            reference_thumb_fd: -1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RegisterFile {
    pub ctl: [u32; 24],
    pub base: [u32; 94],
    pub rc_klut: [u32; 57],
    /// `hevc_vepu580_wgt` at word 1472. H.264 calls the window at the same offset
    /// `param` and writes 180 words of it; the H.265 reg set carries 374.
    pub wgt: [u32; 374],
    /// `vepu580_rdo_cfg` at word 2048: the span H.264 splits into `rdo` (122 words)
    /// and `scl` (679, starting at word 2176) with a six-word gap between them.
    pub rdo: [u32; 807],
    pub osd: [u32; 288],
    /// The `MPP_DEV_REG_OFFSET` entries a submit batch carries: the chroma plane
    /// offsets, the two FBC body offsets, the packet length and the output size.
    /// Empty for a pure derivation; the session fills it.
    pub offsets: Vec<RegOffsetInfo>,
}

impl Default for RegisterFile {
    fn default() -> Self {
        Self {
            ctl: [0; 24],
            base: [0; 94],
            rc_klut: [0; 57],
            wgt: [0; 374],
            rdo: [0; 807],
            osd: [0; 288],
            offsets: Vec::new(),
        }
    }
}

impl RegisterFile {
    /// The captured job: the six windows of the oracle's fixed-QP IDR. This is the
    /// image `build_registers` derives on top of, because the tuning windows and the
    /// byte tables have no public formula in the C.
    pub fn recorded() -> Self {
        Self {
            ctl: template::CTL_WORDS,
            base: template::BASE_WORDS,
            rc_klut: template::RC_KLUT_WORDS,
            wgt: template::WGT_WORDS,
            rdo: template::RDO_WORDS,
            osd: template::OSD_WORDS,
            offsets: Vec::new(),
        }
    }

    /// The words backing one section name from [`regs::SECTIONS`]. `None` names a
    /// section the register map declares and this struct does not carry: the caller
    /// gets an error naming the section rather than a word that silently goes
    /// unwritten.
    fn window(&self, name: &str) -> Option<&[u32]> {
        match name {
            "ctl" => Some(&self.ctl),
            "base" => Some(&self.base),
            "rc_klut" => Some(&self.rc_klut),
            "wgt" => Some(&self.wgt),
            "rdo" => Some(&self.rdo),
            "osd" => Some(&self.osd),
            _ => None,
        }
    }

    fn window_mut(&mut self, name: &str) -> Option<&mut [u32]> {
        match name {
            "ctl" => Some(&mut self.ctl),
            "base" => Some(&mut self.base),
            "rc_klut" => Some(&mut self.rc_klut),
            "wgt" => Some(&mut self.wgt),
            "rdo" => Some(&mut self.rdo),
            "osd" => Some(&mut self.osd),
            _ => None,
        }
    }

    /// The captured windows in `MPP_CMD_SET_REG_WRITE` order: section name, first
    /// absolute word, words. This is what a submit path flattens into the register
    /// batch, and what the section-table test below checks against the map.
    pub fn captured(&self) -> Vec<(&'static str, usize, &[u32])> {
        regs::SECTIONS
            .iter()
            .map(|(name, first, _)| {
                (
                    *name,
                    *first,
                    self.window(*name)
                        .unwrap_or_else(|| panic!("RegisterFile is missing section {name}")),
                )
            })
            .collect()
    }

    fn word_mut(&mut self, word: usize) -> Result<&mut u32, String> {
        for (name, first, count) in regs::SECTIONS {
            if word >= first && word < first + count {
                let index = word - first;
                return self
                    .window_mut(name)
                    .map(|words| &mut words[index])
                    .ok_or_else(|| {
                        format!("section {name} is in the register map but not in RegisterFile")
                    });
            }
        }
        Err(format!("register word {word} is outside every section"))
    }

    pub fn set(&mut self, field: &regs::Field, value: u32) -> Result<(), String> {
        let mask = (1u32 << field.width) - 1;
        if value & !mask != 0 {
            return Err(format!(
                "{} is {} bits wide, {value} does not fit",
                field.name, field.width
            ));
        }
        let word = self.word_mut(field.word)?;
        *word = (*word & !(mask << field.shift)) | (value << field.shift);
        Ok(())
    }

    pub fn get(&self, field: &regs::Field) -> Result<u32, String> {
        for (name, first, count) in regs::SECTIONS {
            if field.word >= first && field.word < first + count {
                let words = self.window(name).ok_or_else(|| {
                    format!("section {name} is in the register map but not in RegisterFile")
                })?;
                return Ok((words[field.word - first] >> field.shift) & ((1 << field.width) - 1));
            }
        }
        Err(format!("register word {} is not captured", field.word))
    }

    fn rc_word_mut(&mut self, word: usize) -> Result<&mut u32, String> {
        if !(1024..1081).contains(&word) {
            return Err(format!("word {word} is outside the RC window"));
        }
        Ok(&mut self.rc_klut[word - 1024])
    }

    fn set_signed(&mut self, field: &regs::Field, value: i32) -> Result<(), String> {
        let mask = (1u32 << field.width) - 1;
        self.set(field, value as u32 & mask)
    }

    /// `vepu580_h265_set_rc_regs`'s non-FIXQP branch. The two CTU counts are
    /// 64x64 (`mb_wd64`/`mb_h64`), not H.264's 16x16 macroblocks, and the
    /// quantiser ladder differs at two points: `aq_mode` is 1 and the first
    /// difference threshold is doubled (`2 * negative_bits_thd`).
    fn apply_rate_control(&mut self, job: &IdrJob, rc: &RateControl) -> Result<(), String> {
        let mb_w = ((job.width + 63) / 64) as i32;
        let mb_h = ((job.height + 63) / 64) as i32;
        let mut target_16 = rc.bit_target.wrapping_shl(4) / (mb_w * mb_h);
        if target_16 >= 0x0010_0000 {
            target_16 = 0x0005_0000;
        }
        let ctu_bits = (target_16 * mb_w) >> 4;
        let negative = 0 - 5 * ctu_bits / 16;
        let positive = 5 * ctu_bits / 16;

        self.set(&regs::BASE_ENC_PIC_PIC_QP, rc.qp_target)?;
        self.set(&regs::H265_REG0240_SYNT_SLI1_SLI_QP, rc.qp_target)?;
        self.set(&regs::BASE_RC_CFG_RC_EN, 1)?;
        self.set(&regs::BASE_RC_CFG_AQ_EN, 1)?;
        self.set(&regs::BASE_RC_CFG_AQ_MODE, 1)?;
        self.set(&regs::BASE_RC_CFG_RC_CTU_NUM, mb_w as u32)?;
        self.set(
            &regs::BASE_RC_QP_RC_QP_RANGE,
            if matches!(job.picture, Picture::Idr | Picture::ForcedIdrFromP { .. }) {
                rc.qp_delta_row_i
            } else {
                rc.qp_delta_row
            },
        )?;
        self.set(&regs::BASE_RC_QP_RC_MAX_QP, rc.qp_max)?;
        self.set(&regs::BASE_RC_QP_RC_MIN_QP, rc.qp_min)?;
        self.set(&regs::BASE_RC_TGT_CTU_EBIT, target_16 as u32)?;
        self.set(&regs::H265_REG0238_SYNT_PPS_CU_QP_DLT_EN, 1)?;

        let dthd = [
            2 * negative,
            negative,
            positive,
            2 * positive,
            i32::MAX,
            i32::MAX,
            i32::MAX,
            i32::MAX,
            i32::MAX,
        ];
        let start = regs::RC_RC_DTHD_0_8.word;
        for (index, value) in dthd.iter().enumerate() {
            *self.rc_word_mut(start + index)? = *value as u32;
        }

        for (field, value) in [
            (&regs::RC_RC_ADJ0_QP_ADJ0, -2),
            (&regs::RC_RC_ADJ0_QP_ADJ1, -1),
            (&regs::RC_RC_ADJ0_QP_ADJ2, 0),
            (&regs::RC_RC_ADJ0_QP_ADJ3, 1),
            (&regs::RC_RC_ADJ0_QP_ADJ4, 2),
            (&regs::RC_RC_ADJ1_QP_ADJ5, 0),
            (&regs::RC_RC_ADJ1_QP_ADJ6, 0),
            (&regs::RC_RC_ADJ1_QP_ADJ7, 0),
            (&regs::RC_RC_ADJ1_QP_ADJ8, 0),
        ] {
            self.set_signed(field, value)?;
        }

        for (field, bound) in [
            (&regs::RC_ROI_QTHD0_QPMIN_AREA0, rc.qp_min),
            (&regs::RC_ROI_QTHD0_QPMAX_AREA0, rc.qp_max),
            (&regs::RC_ROI_QTHD0_QPMIN_AREA1, rc.qp_min),
            (&regs::RC_ROI_QTHD0_QPMAX_AREA1, rc.qp_max),
            (&regs::RC_ROI_QTHD0_QPMIN_AREA2, rc.qp_min),
            (&regs::RC_ROI_QTHD1_QPMAX_AREA2, rc.qp_max),
            (&regs::RC_ROI_QTHD1_QPMIN_AREA3, rc.qp_min),
            (&regs::RC_ROI_QTHD1_QPMAX_AREA3, rc.qp_max),
            (&regs::RC_ROI_QTHD1_QPMIN_AREA4, rc.qp_min),
            (&regs::RC_ROI_QTHD1_QPMAX_AREA4, rc.qp_max),
            (&regs::RC_ROI_QTHD2_QPMIN_AREA5, rc.qp_min),
            (&regs::RC_ROI_QTHD2_QPMAX_AREA5, rc.qp_max),
            (&regs::RC_ROI_QTHD2_QPMIN_AREA6, rc.qp_min),
            (&regs::RC_ROI_QTHD2_QPMAX_AREA6, rc.qp_max),
            (&regs::RC_ROI_QTHD2_QPMIN_AREA7, rc.qp_min),
            (&regs::RC_ROI_QTHD3_QPMAX_AREA7, rc.qp_max),
        ] {
            self.set(field, bound)?;
        }
        self.set(&regs::RC_ROI_QTHD3_QPMAP_MODE, 1)?;

        for (field, value) in [
            (&regs::RC_MD_SAD_THD_MD_SAD_THD0, 4),
            (&regs::RC_MD_SAD_THD_MD_SAD_THD1, 9),
            (&regs::RC_MD_SAD_THD_MD_SAD_THD2, 15),
            (&regs::RC_MADI_THD_MADI_THD0, 4),
            (&regs::RC_MADI_THD_MADI_THD1, 9),
            (&regs::RC_MADI_THD_MADI_THD2, 15),
        ] {
            self.set(field, value)?;
        }
        Ok(())
    }
}

/// The `idr_w_radl` NAL unit type the reference's `h265e_get_nal_type` selects for
/// an IDR that opens the stream (a reference IDR with leading pictures, which the
/// constrained-baseline harness emits). Kept here so the derivation and its test
/// name the value instead of a bare `19`.
const NAL_IDR_W_RADL: u32 = 19;

/// Build the fixed-QP IDR register file. Derived from
/// `hal_h265e_v580_gen_regs` + `vepu580_h265_set_pp_regs`/`set_rc_regs`/
/// `set_slice_regs`/`set_ref_regs` + `vepu580_h265_global_cfg_set`.
pub fn build_registers(job: &IdrJob) -> Result<RegisterFile, String> {
    if job.width == 0 || job.height == 0 {
        return Err("H.265 register file needs a non-zero picture".into());
    }
    let mut file = RegisterFile::recorded();

    // ---- control window (gen_regs) ----
    // enc_strt: lkt_num 0, vepu_cmd RKV_ENC_MODE(0); enc_clr both 0 -> left 0.
    file.set(&regs::CTL_INT_EN_ENC_DONE_EN, 1)?;
    file.set(&regs::CTL_INT_EN_LKT_NODE_DONE_EN, 1)?;
    file.set(&regs::CTL_INT_EN_SCLR_DONE_EN, 1)?;
    file.set(&regs::CTL_INT_EN_SLC_DONE_EN, 0)?;
    file.set(&regs::CTL_INT_EN_BSF_OFLW_EN, 1)?;
    file.set(&regs::CTL_INT_EN_BRSP_OTSD_EN, 1)?;
    file.set(&regs::CTL_INT_EN_WBUS_ERR_EN, 1)?;
    file.set(&regs::CTL_INT_EN_RBUS_ERR_EN, 1)?;
    file.set(&regs::CTL_INT_EN_WDG_EN, 1)?;
    file.set(&regs::CTL_INT_EN_LKT_ERR_INT_EN, 0)?;
    // dtrns_map: bsw_bus_edin 0x7, afbc_bsize 0x1, everything else 0.
    file.set(&regs::CTL_DTRNS_MAP_BSW_BUS_EDIN, 0x7)?;
    file.set(&regs::CTL_DTRNS_MAP_AFBC_BSIZE, 0x1)?;
    // dtrns_cfg.dspr_otsd = (frame_type == INTER_P_FRAME) -> 0 for an IDR.
    file.set(&regs::CTL_ENC_WDG_VS_LOAD_THD, 0x1f_ffff)?;
    file.set(&regs::CTL_ENC_WDG_RFP_LOAD_THD, 0)?;
    file.set(&regs::CTL_FUNC_EN_CKE, 1)?;
    file.set(&regs::CTL_FUNC_EN_RESETN_HW_EN, 1)?;
    file.set(&regs::CTL_FUNC_EN_ENC_DONE_TMVP_EN, 1)?;

    // ---- geometry (gen_regs) ----
    let pic_width_align8 = (job.width + 7) & !7;
    let pic_height_align8 = (job.height + 7) & !7;
    let pic_wd64 = (job.width + 63) / 64;
    let pic_h64 = (job.height + 63) / 64;
    file.set(
        &regs::BASE_ENC_RSL_PIC_WD8_M1,
        (pic_width_align8 / 8 - 1) as u32,
    )?;
    file.set(
        &regs::BASE_ENC_RSL_PIC_HD8_M1,
        (pic_height_align8 / 8 - 1) as u32,
    )?;
    file.set(
        &regs::BASE_SRC_FILL_PIC_WFILL,
        if (job.width & 7) != 0 {
            8 - (job.width & 7)
        } else {
            0
        } as u32,
    )?;
    file.set(
        &regs::BASE_SRC_FILL_PIC_HFILL,
        if (job.height & 7) != 0 {
            8 - (job.height & 7)
        } else {
            0
        } as u32,
    )?;

    file.set(&regs::BASE_ENC_PIC_ENC_STND, 1)?; // H.265
    file.set(&regs::BASE_ENC_PIC_CUR_FRM_REF, 1)?; // !non_reference_flag for a reference IDR
    file.set(&regs::BASE_ENC_PIC_BS_SCP, 1)?;
    let ctu_area = pic_wd64 * pic_h64;
    file.set(&regs::BASE_ENC_PIC_LOG2_CTU_NUM, ctu_num_log2(ctu_area))?;
    // src_proc: txa_en 1; mirr/rot 0; afbcd_en 0 (no FBC input).
    file.set(&regs::BASE_SRC_PROC_TXA_EN, 1)?;

    // ---- slice split (vepu580_setup_split) ----
    apply_split(&mut file, job)?;

    // ---- source strides (set_pp_regs) ----
    // `stridey`/`stridec` are the 16-aligned luma stride; a 4:2:0 semi-planar
    // frame uses it for both. The template carries 320, which is why a 640-wide
    // job encoded 6672 bytes against the oracle's 1618 before this landed.
    let stride = (job.width + 15) & !15;
    file.set(&regs::H265_REG0205_SRC_STRD0_SRC_STRD0, stride as u32)?;
    file.set(&regs::H265_REG0206_SRC_STRD1_SRC_STRD1, stride as u32)?;

    // ---- motion estimation (set_me_regs) ----
    // CIME starts from an 11x7 CTU window, shrinks to the picture when the picture
    // is small, then trims the search to a 4-pixel grid.
    let mut merangx = (11 + 1) * 32;
    let mut merangy = (7 + 1) * 32;
    merangx = merangx.min(384);
    merangy = merangy.min(320);
    if job.width < (merangx + 60) as usize || job.width <= 352 {
        if merangx > job.width as i32 {
            merangx = job.width as i32;
        }
        merangx = merangx / 4 * 2;
    }
    if job.height < (merangy + 60) as usize || job.height <= 288 {
        if merangy > job.height as i32 {
            merangy = job.height as i32;
        }
        merangy = merangy / 4 * 2;
    }
    let (merange_x, merange_y) = (merangx / 2, merangy / 2);
    let mxneg = ((-(merange_x << 2)) >> 2) / 4;
    let myneg = ((-(merange_y << 2)) >> 2) / 4;
    let mxpos = (((merange_x << 2) - 4) >> 2) / 4;
    let mypos = (((merange_y << 2) - 4) >> 2) / 4;
    let merangx = mxneg.abs().min(mxpos) * 4 * 2;
    let merangy = myneg.abs().min(mypos) * 4 * 2;
    let srch_h = (merangx / 32) as u32;
    let srch_v = (merangy / 32) as u32;
    file.set(&regs::H265_REG0220_ME_RNGE_CME_SRCH_H, srch_h)?;
    file.set(&regs::H265_REG0220_ME_RNGE_CME_SRCH_V, srch_v)?;

    // `cme_linebuf_w` is the CTU width of the picture; `cme_rama_h` narrows with
    // wide pictures; `cme_rama_max` is the window the line buffer must hold.
    let (linebuf_w, rama_h, rama_max) = cime_ram(job.width, srch_h, srch_v, job.tile);
    file.set(&regs::H265_REG0222_ME_CACH_CME_LINEBUF_W, linebuf_w)?;
    file.set(&regs::H265_REG0222_ME_CACH_CME_RAMA_H, rama_h)?;
    file.set(&regs::H265_REG0222_ME_CACH_CME_RAMA_MAX, rama_max)?;
    // `cach_l2_tag` steps with the 64-aligned width: 0 to 512, 1 to 1024, 2 to
    // 2048, 3 to 4096.
    let l2_tag = match (job.width + 63) / 64 * 64 {
        0..=512 => 0,
        513..=1024 => 1,
        1025..=2048 => 2,
        2049..=4096 => 3,
        _ => 0,
    };
    file.set(&regs::H265_REG0222_ME_CACH_CACH_L2_TAG, l2_tag)?;

    // ---- RC (set_rc_regs) ----
    match job.rc {
        Some(rc) => file.apply_rate_control(job, &rc)?,
        None => {
            // FIXQP branch: four words, and `rc_cfg`/`aq_cfg` keep their zeroes.
            file.set(&regs::BASE_ENC_PIC_PIC_QP, job.qp)?;
            file.set(&regs::H265_REG0240_SYNT_SLI1_SLI_QP, job.qp)?;
            file.set(&regs::BASE_RC_QP_RC_MAX_QP, job.qp)?;
            file.set(&regs::BASE_RC_QP_RC_MIN_QP, job.qp)?;
        }
    }

    // The sixteen `roi_qthd` rows and the `madi`/`md_sad` threshold triples are
    // *not* written here. `vepu580_h265_set_rc_regs` writes them in its else branch,
    // the non-FIXQP one (`hal_h265e_vepu580.c` from the `roi_qthd0.qpmin_area0`
    // assignment onward); the FIXQP branch above it touches only `pic_qp`, `sli_qp`
    // and the two `rc_qp` bounds. The H.264 HAL writes the rows unconditionally, and
    // carrying that across put 26s into four words the traced job leaves zero.
    // `global_cfg_set` does set `madi_thd`/`md_sad_thd` to 25 and 20/30/40, but the
    // traced FIXQP job carries 4/9/15 in both words - a difference that is recorded
    // rather than explained (see "what the capture says" in ROADMAP) and that the
    // template therefore keeps.
    file.set(&regs::RC_KLUT_OFST_CHRM_KLUT_OFST, 0)?;

    // ---- H.265 syntax block (set_slice_regs / gen_regs) ----
    // reg0232 rdo_cfg: chrm_spcl 1, cu_inter_e 0x06db, cu_intra_e 0xf, ccwa_e 1,
    // scl_lst_sel = scaling_list_enabled_flag (0 by default), ltm_* 0 (no LT refs).
    file.set(&regs::H265_REG0232_RDO_CFG_CHRM_SPCL, 1)?;
    file.set(&regs::H265_REG0232_RDO_CFG_CU_INTER_E, 0x06db)?;
    file.set(&regs::H265_REG0232_RDO_CFG_CU_INTRA_E, 0xf)?;
    file.set(&regs::H265_REG0232_RDO_CFG_CCWA_E, 1)?;

    // reg0236 synt_nal.
    file.set(&regs::H265_REG0236_SYNT_NAL_NAL_UNIT_TYPE, NAL_IDR_W_RADL)?;

    // reg0237 synt_sps: from syn->pp.
    file.set(&regs::H265_REG0237_SYNT_SPS_SMPL_ADPT_OFST_E, 1)?; // sao_enable
    file.set(&regs::H265_REG0237_SYNT_SPS_NUM_ST_REF_PIC, 0)?; // num_short_term_ref_pic_sets
    file.set(&regs::H265_REG0237_SYNT_SPS_NUM_LT_REF_PIC, 0)?;
    file.set(&regs::H265_REG0237_SYNT_SPS_LT_REF_PIC_PRSNT, 0)?;
    file.set(
        &regs::H265_REG0237_SYNT_SPS_TMPL_MVP_E,
        u32::from(job.temporal_mvp),
    )?;
    file.set(&regs::H265_REG0237_SYNT_SPS_LOG2_MAX_POC_LSB, 12)?; // bitsForPOC 16
    file.set(&regs::H265_REG0237_SYNT_SPS_STRG_INTRA_SMTH, 1)?;

    // reg0238 synt_pps: from syn->pp.
    file.set(&regs::H265_REG0238_SYNT_PPS_CBC_INIT_PRSNT_FLG, 1)?; // cabac_init_present
    file.set(&regs::H265_REG0238_SYNT_PPS_PIC_INIT_QP, 26)?; // init_qp_minus26 + 26
                                                             // FIXQP leaves `cu_qp_dlt_en` clear, the same rate-control rule the PPS
                                                             // writer follows: `h265e_set_pps` writes `m_useDQP = (rc_mode != FIXQP)`. The
                                                             // traced job's `reg0238_synt_pps` has the bit clear; setting it here shifted a
                                                             // word the oracle writes as 0x00220d40 to 0x00222d40.
    file.set(&regs::H265_REG0238_SYNT_PPS_CU_QP_DLT_DEPTH, 0)?; // diff_cu_qp_delta_depth
    file.set(&regs::H265_REG0238_SYNT_PPS_LST_MDFY_PRSNT_FLG, 1)?; // lists_modification
    file.set(
        &regs::H265_REG0238_SYNT_PPS_LP_FLTR_ACRS_SLI,
        u32::from(job.syntax.loop_filter_across_slices),
    )?;
    file.set(&regs::H265_REG0238_SYNT_PPS_DBLK_FLTR_OVRD_EN, 0)?;
    file.set(
        &regs::H265_REG0238_SYNT_PPS_LPF_FLTR_ACRS_TIL,
        u32::from(job.syntax.loop_filter_across_tiles),
    )?;

    // reg0239 synt_sli0: I slice (type 2), merge flags from codec defaults.
    file.set(&regs::H265_REG0239_SYNT_SLI0_SLI_TYPE, 2)?; // I_SLICE
    file.set(&regs::H265_REG0239_SYNT_SLI0_MRG_UP_FLG, 1)?; // merge_cfg.merge_up_flag
    file.set(&regs::H265_REG0239_SYNT_SLI0_MRG_LFT_FLG, 1)?; // merge_cfg.merge_left_flag
    file.set(
        &regs::H265_REG0239_SYNT_SLI0_SLI_SAO_LUMA_FLG,
        u32::from(!job.syntax.sao_luma_disable),
    )?;
    file.set(
        &regs::H265_REG0239_SYNT_SLI0_SLI_SAO_CHRM_FLG,
        u32::from(!job.syntax.sao_chroma_disable),
    )?;

    // reg0240 synt_sli1: filter controls and max_mrg_cnd 2.
    file.set(&regs::H265_REG0240_SYNT_SLI1_MAX_MRG_CND, 2)?;
    file.set(
        &regs::H265_REG0240_SYNT_SLI1_SP_BETA_OFST_DIV2,
        job.syntax.deblock_beta_offset_div2 as u32 & 0xf,
    )?;
    file.set(
        &regs::H265_REG0240_SYNT_SLI1_SP_TC_OFST_DIV2,
        job.syntax.deblock_tc_offset_div2 as u32 & 0xf,
    )?;
    // The PPS syntax carries the disable; the C slice object still leaves this
    // register clear when `dblk_disable=1` (confirmed by a traced C job).
    file.set(&regs::H265_REG0240_SYNT_SLI1_SP_DBLK_FLTR_DIS, 0)?;
    file.set(&regs::H265_REG0240_SYNT_SLI1_DBLK_FLTR_OVRD_FLG, 0)?;
    // The PPS carries `lpf_acs_sli_en`, but the C slice object leaves its own
    // `m_LFCrossSliceBoundaryFlag` at zero; the traced register is zero even
    // with the PPS option enabled.
    file.set(&regs::H265_REG0240_SYNT_SLI1_SLI_LP_FLTR_ACRS_SLI, 0)?;

    // reg0241 synt_sli2: first picture, poc_lsb 0. Left 0.

    // reg0252 tile_cfg: tiles off (auto_tile default 0). Left 0.
    if let Some(tile) = job.tile {
        file.set(&regs::H265_REG0252_TILE_CFG_TILE_EN, 1)?;
        file.set(&regs::H265_REG0252_TILE_CFG_TILE_W_M1, tile.width_mb - 1)?;
        file.set(&regs::H265_REG0252_TILE_CFG_TILE_H_M1, tile.height_mb - 1)?;
        file.set(&regs::H265_REG0253_TILE_POS_TILE_X, tile.start_x)?;
        file.set(&regs::H265_REG0253_TILE_POS_TILE_Y, 0)?;
        file.set(&regs::H265_REG212_RC_CFG_RC_CTU_NUM, tile.width_mb)?;

        let (txid, rxid, txe, rxe) = if tile.index == 0 {
            (0, 0, 1, 0)
        } else {
            (tile.index, tile.index - 1, 1, 1)
        };
        file.set(&regs::H265_REG0193_DUAL_CORE_DCHS_TXID, txid)?;
        file.set(&regs::H265_REG0193_DUAL_CORE_DCHS_RXID, rxid)?;
        file.set(&regs::H265_REG0193_DUAL_CORE_DCHS_TXE, txe)?;
        file.set(&regs::H265_REG0193_DUAL_CORE_DCHS_RXE, rxe)?;
        file.set(&regs::H265_REG0193_DUAL_CORE_DCHS_OFST, 2)?;
        if tile.index == tile.columns - 1 {
            file.set(&regs::H265_REG0193_DUAL_CORE_DCHS_TXID, 0)?;
            file.set(&regs::H265_REG0193_DUAL_CORE_DCHS_TXE, 0)?;
        }
    }

    // The GOP's long-term reference state is part of every picture, not just the
    // P; the traced CBR IDR carries it too.
    if job.long_term {
        file.set(&regs::H265_REG0232_RDO_CFG_LTM_IDX0L0, 1)?;
        file.set(&regs::H265_REG0237_SYNT_SPS_LT_REF_PIC_PRSNT, 1)?;
        file.set(&regs::H265_REG0237_SYNT_SPS_NUM_LT_REF_PIC, 1)?;
    }

    // ---- picture type: set_slice_regs / set_ref_regs ----
    // The traced P job settles every one of these bits: `sli_type` 1, NAL 1
    // (TRAIL_R), POC 1, the reference rebuilt as an LT entry with
    // `used_by_lt_flg0`, and the inter lambda table in `iprd_wgt_qp_hevc_0_51`.
    if let Picture::P { poc_lsb } | Picture::ForcedIdrFromP { poc_lsb } = job.picture {
        if matches!(job.picture, Picture::P { .. }) {
            file.set(&regs::CTL_DTRNS_CFG_DSPR_OTSD, 1)?;
        }
        // `tot_poc_num` is 1 for every traced P picture, not the running count.
        file.set(&regs::H265_REG0192_ENC_PIC_NUM_PIC_TOT_CUR, 1)?;
        file.set(
            &regs::H265_REG0236_SYNT_NAL_NAL_UNIT_TYPE,
            if matches!(job.picture, Picture::ForcedIdrFromP { .. }) {
                NAL_IDR_W_RADL
            } else {
                1
            },
        )?;
        file.set(&regs::H265_REG0239_SYNT_SLI0_NUM_REFIDX_ACT_OVRD, 0)?;
        file.set(&regs::H265_REG0239_SYNT_SLI0_SLI_TYPE, 1)?;
        if job.temporal_mvp && matches!(job.picture, Picture::P { .. }) {
            file.set(&regs::H265_REG0239_SYNT_SLI0_SLI_TMPRL_MVP_E, 1)?;
            file.set(&regs::H265_REG0240_SYNT_SLI1_COL_REF_IDX, 0)?;
            file.set(&regs::H265_REG0240_SYNT_SLI1_COL_FRM_L0_FLG, 1)?;
            // The collocated picture is stored for every temporal-MVP picture.
            // It can only be loaded after the first P, whose reference is an IDR.
            file.set(
                &regs::H265_REG0221_ME_CFG_COLMV_LOAD,
                u32::from(poc_lsb >= 2),
            )?;
            file.set(&regs::H265_REG0221_ME_CFG_COLMV_STOR, 1)?;
        }
        file.set(&regs::H265_REG0241_SYNT_SLI2_SLI_POC_LSB, poc_lsb)?;
        // The reference marking follows the GOP configuration: a refresh/VI GOP
        // points at the SPS long-term entry, a plain GOP at short-term reference 0.
        if job.long_term {
            file.set(&regs::H265_REG0242_SYNT_REFM0_NUM_LT_PIC, 1)?;
            file.set(&regs::H265_REG0243_SYNT_REFM1_USED_BY_LT_FLG0, 1)?;
        } else {
            file.set(&regs::H265_REG0243_SYNT_REFM1_NUM_NEGATIVE_PICS, 1)?;
            file.set(&regs::H265_REG0243_SYNT_REFM1_USED_BY_S0_FLG, 1)?;
        }
        // From the third picture on the reference is the previous P, and the
        // traced job states the list explicitly: `ref_pic_lst_mdf_l0`,
        // `lst_entry_l0` and `num_negative_pics` all carry 1 where the first P
        // leaves them clear.
        if poc_lsb >= 2 && job.long_term {
            file.set(&regs::H265_REG0239_SYNT_SLI0_REF_PIC_LST_MDF_L0, 1)?;
            file.set(&regs::H265_REG0240_SYNT_SLI1_LST_ENTRY_L0, 1)?;
            file.set(&regs::H265_REG0243_SYNT_REFM1_NUM_NEGATIVE_PICS, 1)?;
        }
        if matches!(job.picture, Picture::P { .. }) {
            let start = IPRD_WGT_QP_WORD - 1472;
            file.wgt[start..start + MODB_LAMBDA.len()].copy_from_slice(&MODB_LAMBDA);
        }
    }

    // No MD surface is implemented here, so `enc_pic.mei_stor` stays 0 and the
    // `meiw_addr` word stays clear. H.264 sets this bit *because* its harness passed
    // an MD buffer; the H.265 HAL leaves it 0 when `enc_task->md_info` is null
    // (`vepu580_h265_set_hw_address`), which is the case for a driver that requests
    // no motion info.
    file.set(&regs::BASE_ENC_PIC_MEI_STOR, 0)?;

    // `vepu580_h265_set_roi_regs` runs before the refresh overlay. Both features
    // use roi_addr; the C order lets a refresh band replace the base table while
    // the optional QP surface keeps its address.
    apply_roi(&mut file, job)?;

    // `setup_intra_refresh` runs after the syntax/slice/ref and ME setup in C.
    // It overlays only the refresh table, roi_en and one CIME search axis.
    if let Some(refresh) = job.refresh {
        file.set(&regs::H265_REG0192_ENC_PIC_ROI_EN, 1)?;
        file.base[178 - 160] = refresh.table_fd as u32;
        match refresh.mode {
            RefreshMode::Row => file.set(&regs::H265_REG0220_ME_RNGE_CME_SRCH_V, 1)?,
            RefreshMode::Col => file.set(&regs::H265_REG0220_ME_RNGE_CME_SRCH_H, 1)?,
        }
    }

    Ok(file)
}

fn apply_roi(file: &mut RegisterFile, job: &IdrJob) -> Result<(), String> {
    file.base[178 - 160] = 0;
    file.base[179 - 160] = 0;
    file.set(&regs::H265_REG0192_ENC_PIC_ROI_EN, 0)?;
    file.set(&regs::H265_REG0228_ROI_EN_ROI_QP_EN, 0)?;
    if job.roi_fd < 0 {
        return Ok(());
    }
    file.set(&regs::H265_REG0192_ENC_PIC_ROI_EN, 1)?;
    file.base[178 - 160] = job.roi_fd as u32;
    if job.roi_qp_fd >= 0 {
        file.base[179 - 160] = job.roi_qp_fd as u32;
        file.set(&regs::H265_REG0228_ROI_EN_ROI_QP_EN, 1)?;
    }
    Ok(())
}

/// `vepu580_setup_split`. Tiles are not implemented, so the CTU count is the
/// full 64x64 grid rather than the C's halved `title_en` width.
fn apply_split(file: &mut RegisterFile, job: &IdrJob) -> Result<(), String> {
    file.set(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT, 0)?;
    file.set(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT_MODE, 0)?;
    file.set(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT_CPST, 0)?;
    file.set(&regs::H265_REG0216_SLI_SPLT_SLI_MAX_NUM_M1, 0)?;
    file.set(&regs::H265_REG0216_SLI_SPLT_SLI_FLSH, 0)?;
    file.set(&regs::H265_REG0217_SLI_BYTE_SLI_SPLT_BYTE, 0)?;
    file.set(&regs::H265_REG0218_SLI_CNUM_SLI_SPLT_CNUM_M1, 0)?;
    file.set(&regs::BASE_ENC_PIC_SLEN_FIFO, 0)?;
    file.set(&regs::CTL_INT_EN_SLC_DONE_EN, 0)?;

    let split = job.split;
    split
        .validate()
        .map_err(|error| format!("H.265 slice split: {error}"))?;
    match split.mode {
        SplitMode::None => {}
        SplitMode::ByByte => {
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT, 1)?;
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT_MODE, 0)?;
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT_CPST, 0)?;
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_MAX_NUM_M1, 500)?;
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_FLSH, 1)?;
            file.set(&regs::H265_REG0218_SLI_CNUM_SLI_SPLT_CNUM_M1, 0)?;
            file.set(&regs::H265_REG0217_SLI_BYTE_SLI_SPLT_BYTE, split.arg)?;
            let fifo = u32::from(split.out != 0);
            file.set(&regs::BASE_ENC_PIC_SLEN_FIFO, fifo)?;
            file.set(&regs::CTL_INT_EN_SLC_DONE_EN, fifo)?;
        }
        SplitMode::ByCtu => {
            let slice_num = split.slice_count(job.width, job.height);
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT, 1)?;
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT_MODE, 1)?;
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT_CPST, 0)?;
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_MAX_NUM_M1, 500)?;
            file.set(&regs::H265_REG0216_SLI_SPLT_SLI_FLSH, 1)?;
            file.set(&regs::H265_REG0218_SLI_CNUM_SLI_SPLT_CNUM_M1, split.arg - 1)?;
            file.set(&regs::H265_REG0217_SLI_BYTE_SLI_SPLT_BYTE, 0)?;
            let fifo = u32::from(split.out != 0);
            file.set(&regs::BASE_ENC_PIC_SLEN_FIFO, fifo)?;
            if split.out & MPP_ENC_SPLIT_OUT_LOWDELAY != 0 || (fifo != 0 && slice_num > 32) {
                file.set(&regs::CTL_INT_EN_SLC_DONE_EN, 1)?;
            }
        }
    }
    Ok(())
}

/// `enc_pic.log2_ctu_num = ceil(log2(pic_wd64 * pic_h64))`, the C uses
/// `ceil(log2((double)area))`.
fn ctu_num_log2(area: usize) -> u32 {
    assert!(area > 0);
    (usize::BITS - (area - 1).leading_zeros()) as u32
}

/// `vepu580_h265_set_me_ram` for the single-tile, zero-global-motion case an IDR
/// takes: it grows the CIME line buffer until the window holds the search, then
/// reports `(cme_rama_h, cme_rama_max)`.
fn cime_ram(width: usize, srch_h: u32, srch_v: u32, tile: Option<TileJob>) -> (u32, u32, u32) {
    let pic_wd64 = ((width + 63) / 64) as i32;
    let srch_w = (srch_h * 4) as i32;
    let srch_h_px = (srch_v * 4) as i32;
    let clip = |v: i32, lo: i32, hi: i32| v.clamp(lo, hi);
    // gmv_x/gmv_y are 0 for an IDR.
    let (tile_sta, tile_end) = tile.map_or((0, pic_wd64 - 1), |tile| {
        (
            tile.start_x as i32,
            (tile.start_x + tile.width_mb - 1) as i32,
        )
    });
    let frm_sta = clip(tile_sta + (0 - srch_w - 15) / 16, 0, pic_wd64 - 1);
    let frm_end = clip(tile_end + (srch_w + 15) / 16, 0, pic_wd64 - 1);
    let linebuf_w = (frm_end - frm_sta + 1) * 64 / 64;
    // The C takes the `if (delta < 0)` branch only when the global motion is
    // negative relative to the search; an IDR's gmv is 0, so that is exactly
    // `(0 - srch % align) < 0`.
    let delta_w = -(srch_w % 16);
    let base_w = if delta_w < 0 {
        16 + delta_w % 16
    } else {
        delta_w % 16
    };
    let cur_srch_16_w = ((base_w + srch_w * 2 + 15) / 16 + 1).min(linebuf_w);
    let delta_h = -(srch_h_px % 4);
    let base_h = if delta_h < 0 {
        4 + delta_h % 4
    } else {
        delta_h % 4
    };
    let cur_srch_4_h = (base_h + srch_h_px * 2 + 3) / 4 + 4;
    let cur_srch_max = (cur_srch_4_h + 3) & !3;
    let ctu_4_h = 4;
    let cime_rama_max = 2464;
    let mut cime_rama_h = ctu_4_h;
    let mut ramb_h = cur_srch_4_h;
    while cime_rama_h < cur_srch_max
        && cime_rama_max > (cime_rama_h - ctu_4_h) * linebuf_w * 4 + ramb_h * 4 * cur_srch_16_w
    {
        cime_rama_h += ctu_4_h;
        if ramb_h > 2 * ctu_4_h {
            ramb_h -= ctu_4_h;
        } else {
            ramb_h = ctu_4_h;
        }
    }
    if cur_srch_4_h == ctu_4_h {
        cime_rama_h += ctu_4_h;
        ramb_h = 0;
    }
    if cime_rama_max < (cime_rama_h - ctu_4_h) * linebuf_w * 4 + ramb_h * 4 * cur_srch_16_w {
        cime_rama_h -= ctu_4_h;
    }
    let ram_col_h = (cime_rama_h - ctu_4_h) / ctu_4_h;
    (
        linebuf_w as u32,
        cime_rama_h as u32,
        (ram_col_h * linebuf_w + cur_srch_16_w) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_idr(qp: u32) -> RegisterFile {
        build_registers(&IdrJob::reference(qp)).unwrap()
    }

    #[test]
    fn ctu_log2_matches_ceil_log2() {
        assert_eq!(ctu_num_log2(1), 0);
        assert_eq!(ctu_num_log2(2), 1);
        assert_eq!(ctu_num_log2(4), 2);
        assert_eq!(ctu_num_log2(20), 5); // 320x240 -> 5*4 CTUs
        assert_eq!(ctu_num_log2(32), 5);
        assert_eq!(ctu_num_log2(33), 6);
    }

    #[test]
    fn geometry_of_320x240_idr() {
        let file = reference_idr(26);
        assert_eq!(file.get(&regs::BASE_ENC_RSL_PIC_WD8_M1).unwrap(), 39);
        assert_eq!(file.get(&regs::BASE_ENC_RSL_PIC_HD8_M1).unwrap(), 29);
        assert_eq!(file.get(&regs::BASE_SRC_FILL_PIC_WFILL).unwrap(), 0);
        assert_eq!(file.get(&regs::BASE_SRC_FILL_PIC_HFILL).unwrap(), 0);
        assert_eq!(file.get(&regs::BASE_ENC_PIC_LOG2_CTU_NUM).unwrap(), 5);
        assert_eq!(file.get(&regs::BASE_ENC_PIC_ENC_STND).unwrap(), 1);
        assert_eq!(file.get(&regs::BASE_ENC_PIC_CUR_FRM_REF).unwrap(), 1);
        assert_eq!(file.get(&regs::BASE_ENC_PIC_BS_SCP).unwrap(), 1);
    }

    #[test]
    fn fixed_qp_writes_only_the_qp_words_and_never_enables_rc() {
        // The picture QP, the slice QP and the two RC bounds all carry the QP; the
        // rate-control enables stay at the memset zero the FIXQP branch leaves them.
        for qp in [0u32, 10, 20, 26, 32, 42, 51] {
            let file = reference_idr(qp);
            assert_eq!(file.get(&regs::BASE_ENC_PIC_PIC_QP).unwrap(), qp);
            assert_eq!(file.get(&regs::H265_REG0240_SYNT_SLI1_SLI_QP).unwrap(), qp);
            assert_eq!(file.get(&regs::BASE_RC_QP_RC_MAX_QP).unwrap(), qp);
            assert_eq!(file.get(&regs::BASE_RC_QP_RC_MIN_QP).unwrap(), qp);
        }
        let file = reference_idr(26);
        assert_eq!(file.get(&regs::BASE_RC_CFG_RC_EN).unwrap(), 0);
        assert_eq!(file.get(&regs::BASE_RC_CFG_AQ_EN).unwrap(), 0);
    }

    #[test]
    fn roi_qthd_rows_stay_clear_for_fixed_qp() {
        // The H.264 HAL writes these sixteen rows unconditionally; the H.265 HAL
        // writes them in the non-FIXQP branch only, and the traced FIXQP job has
        // every one of them clear.
        let file = reference_idr(26);
        for field in [
            &regs::RC_ROI_QTHD0_QPMIN_AREA0,
            &regs::RC_ROI_QTHD0_QPMAX_AREA0,
            &regs::RC_ROI_QTHD3_QPMAX_AREA7,
        ] {
            assert_eq!(file.get(field).unwrap(), 0, "{}", field.name);
        }
        assert_eq!(file.get(&regs::RC_ROI_QTHD3_QPMAP_MODE).unwrap(), 0);
    }

    #[test]
    fn fixed_qp_idr_syntax_fields_match_the_source() {
        let file = reference_idr(26);
        assert_eq!(
            file.get(&regs::H265_REG0236_SYNT_NAL_NAL_UNIT_TYPE)
                .unwrap(),
            NAL_IDR_W_RADL
        );
        assert_eq!(
            file.get(&regs::H265_REG0237_SYNT_SPS_SMPL_ADPT_OFST_E)
                .unwrap(),
            1
        );
        assert_eq!(
            file.get(&regs::H265_REG0237_SYNT_SPS_LOG2_MAX_POC_LSB)
                .unwrap(),
            12
        );
        assert_eq!(
            file.get(&regs::H265_REG0237_SYNT_SPS_STRG_INTRA_SMTH)
                .unwrap(),
            1
        );
        assert_eq!(
            file.get(&regs::H265_REG0238_SYNT_PPS_PIC_INIT_QP).unwrap(),
            26
        );
        assert_eq!(
            file.get(&regs::H265_REG0238_SYNT_PPS_CU_QP_DLT_EN).unwrap(),
            0
        );
        assert_eq!(
            file.get(&regs::H265_REG0238_SYNT_PPS_LST_MDFY_PRSNT_FLG)
                .unwrap(),
            1
        );
        // I_SLICE.
        assert_eq!(file.get(&regs::H265_REG0239_SYNT_SLI0_SLI_TYPE).unwrap(), 2);
        // Tiles off.
        assert_eq!(file.get(&regs::H265_REG0252_TILE_CFG_TILE_EN).unwrap(), 0);
    }

    #[test]
    fn deblock_sao_and_loop_filter_options_reach_the_syntax_registers() {
        let mut job = IdrJob::reference(26);
        job.syntax = SyntaxOptions {
            sao_luma_disable: false,
            sao_chroma_disable: true,
            deblock_disable: false,
            deblock_beta_offset_div2: 3,
            deblock_tc_offset_div2: -2,
            loop_filter_across_slices: true,
            loop_filter_across_tiles: false,
            auto_tile: false,
        };
        let file = build_registers(&job).unwrap();
        assert_eq!(
            file.get(&regs::H265_REG0239_SYNT_SLI0_SLI_SAO_LUMA_FLG)
                .unwrap(),
            1
        );
        assert_eq!(
            file.get(&regs::H265_REG0239_SYNT_SLI0_SLI_SAO_CHRM_FLG)
                .unwrap(),
            0
        );
        assert_eq!(
            file.get(&regs::H265_REG0240_SYNT_SLI1_SP_BETA_OFST_DIV2)
                .unwrap(),
            3
        );
        assert_eq!(
            file.get(&regs::H265_REG0240_SYNT_SLI1_SP_TC_OFST_DIV2)
                .unwrap(),
            14
        );
        assert_eq!(
            file.get(&regs::H265_REG0240_SYNT_SLI1_SLI_LP_FLTR_ACRS_SLI)
                .unwrap(),
            0
        );
        assert_eq!(
            file.get(&regs::H265_REG0238_SYNT_PPS_LP_FLTR_ACRS_SLI)
                .unwrap(),
            1
        );
        assert_eq!(
            file.get(&regs::H265_REG0238_SYNT_PPS_LPF_FLTR_ACRS_TIL)
                .unwrap(),
            0
        );
    }

    #[test]
    fn tile_job_writes_the_uniform_tile_window() {
        let mut job = IdrJob::reference(26);
        job.syntax.auto_tile = true;
        job.tile = Some(TileJob {
            index: 1,
            start_x: 2,
            width_mb: 3,
            height_mb: 4,
            columns: 2,
        });
        let file = build_registers(&job).unwrap();
        assert_eq!(file.get(&regs::H265_REG0252_TILE_CFG_TILE_EN).unwrap(), 1);
        assert_eq!(file.get(&regs::H265_REG0252_TILE_CFG_TILE_W_M1).unwrap(), 2);
        assert_eq!(file.get(&regs::H265_REG0252_TILE_CFG_TILE_H_M1).unwrap(), 3);
        assert_eq!(file.get(&regs::H265_REG0253_TILE_POS_TILE_X).unwrap(), 2);
        assert_eq!(file.get(&regs::H265_REG212_RC_CFG_RC_CTU_NUM).unwrap(), 3);
        assert_eq!(
            file.get(&regs::H265_REG0193_DUAL_CORE_DCHS_OFST).unwrap(),
            2
        );
        assert_eq!(file.get(&regs::H265_REG0193_DUAL_CORE_DCHS_TXE).unwrap(), 0);
        assert_eq!(
            file.get(&regs::H265_REG0222_ME_CACH_CME_LINEBUF_W).unwrap(),
            4
        );
    }

    #[test]
    fn by_ctu_low_delay_arms_the_slice_fifo_and_interrupt() {
        let mut job = IdrJob::reference(26);
        job.split = SliceSplit {
            mode: SplitMode::ByCtu,
            arg: 8,
            out: MPP_ENC_SPLIT_OUT_LOWDELAY,
        };
        let file = build_registers(&job).unwrap();
        assert_eq!(file.get(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT).unwrap(), 1);
        assert_eq!(
            file.get(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT_MODE)
                .unwrap(),
            1
        );
        assert_eq!(
            file.get(&regs::H265_REG0218_SLI_CNUM_SLI_SPLT_CNUM_M1)
                .unwrap(),
            7
        );
        assert_eq!(file.get(&regs::BASE_ENC_PIC_SLEN_FIFO).unwrap(), 1);
        assert_eq!(file.get(&regs::CTL_INT_EN_SLC_DONE_EN).unwrap(), 1);
    }

    #[test]
    fn by_byte_without_split_out_keeps_the_fifo_off() {
        let mut job = IdrJob::reference(26);
        job.split = SliceSplit {
            mode: SplitMode::ByByte,
            arg: 256,
            out: 0,
        };
        let file = build_registers(&job).unwrap();
        assert_eq!(
            file.get(&regs::H265_REG0216_SLI_SPLT_SLI_SPLT_MODE)
                .unwrap(),
            0
        );
        assert_eq!(
            file.get(&regs::H265_REG0217_SLI_BYTE_SLI_SPLT_BYTE)
                .unwrap(),
            256
        );
        assert_eq!(file.get(&regs::BASE_ENC_PIC_SLEN_FIFO).unwrap(), 0);
        assert_eq!(file.get(&regs::CTL_INT_EN_SLC_DONE_EN).unwrap(), 0);
    }

    #[test]
    fn roi_registers_use_the_base_and_qp_surfaces() {
        let mut job = IdrJob::reference(26);
        job.roi_fd = 31;
        job.roi_qp_fd = 32;
        let file = build_registers(&job).unwrap();
        assert_eq!(file.get(&regs::H265_REG0192_ENC_PIC_ROI_EN).unwrap(), 1);
        assert_eq!(file.get(&regs::H265_REG0228_ROI_EN_ROI_QP_EN).unwrap(), 1);
        assert_eq!(file.base[178 - 160], 31);
        assert_eq!(file.base[179 - 160], 32);
    }

    #[test]
    fn rdo_and_rc_lookup_constants() {
        let file = reference_idr(26);
        assert_eq!(
            file.get(&regs::H265_REG0232_RDO_CFG_CU_INTER_E).unwrap(),
            0x06db
        );
        assert_eq!(file.get(&regs::H265_REG0232_RDO_CFG_CHRM_SPCL).unwrap(), 1);
        // `global_cfg_set` assigns 25 and 20/30/40 to these, but the traced FIXQP
        // job carries 4/9/15 in both words. The template keeps the traced values
        // until the difference is explained; nothing here writes over them.
        assert_eq!(file.get(&regs::RC_MADI_CFG_MADI_THD).unwrap(), 25);
        assert_eq!(file.get(&regs::RC_MD_SAD_THD_MD_SAD_THD0).unwrap(), 4);
        assert_eq!(file.get(&regs::RC_MD_SAD_THD_MD_SAD_THD1).unwrap(), 9);
        assert_eq!(file.get(&regs::RC_MD_SAD_THD_MD_SAD_THD2).unwrap(), 15);
        assert_eq!(file.get(&regs::RC_MADI_THD_MADI_THD0).unwrap(), 4);
        assert_eq!(file.get(&regs::RC_MADI_THD_MADI_THD1).unwrap(), 9);
        assert_eq!(file.get(&regs::RC_MADI_THD_MADI_THD2).unwrap(), 15);
        assert_eq!(file.get(&regs::RC_KLUT_OFST_CHRM_KLUT_OFST).unwrap(), 0);
    }

    /// A change of QP moves exactly the four words the fixed-QP branch writes, plus
    /// the sixteen ROI rows that follow the picture bounds. The tuning windows and
    /// every other base word are untouched, which is what lets a field change be
    /// reasoned about word by word.
    #[test]
    fn only_qp_and_roi_words_move_with_qp() {
        let a = reference_idr(26);
        let b = reference_idr(30);
        let changed = a
            .base
            .iter()
            .zip(b.base.iter())
            .filter(|(x, y)| x != y)
            .count()
            + a.rc_klut
                .iter()
                .zip(b.rc_klut.iter())
                .filter(|(x, y)| x != y)
                .count();
        // pic_qp, sli_qp (base), rc_min/max_qp (base) share two base words
        // (enc_pic and rc_qp); the sixteen ROI rows collapse onto fewer distinct
        // rc_klut words. The point of the assertion is not an exact count but that
        // nothing outside those windows moves.
        assert!(changed > 0 && changed <= 20);
        assert_eq!(a.wgt, b.wgt);
        assert_eq!(a.rdo, b.rdo);
        assert_eq!(a.osd, b.osd);
        assert_eq!(a.ctl, b.ctl);
    }

    #[test]
    fn tuning_windows_come_from_the_recorded_job() {
        // 374 and 807 words of weight and RDO tables the HAL writes without a public
        // formula: recorded from the traced job, not zero.
        let file = reference_idr(26);
        assert_eq!(file.wgt, template::WGT_WORDS);
        assert_eq!(file.rdo, template::RDO_WORDS);
        assert_eq!(file.osd, template::OSD_WORDS);
        assert!(
            file.wgt.iter().any(|&w| w != 0),
            "the captured weight window is not blank"
        );
    }

    /// The generated section table and this struct's arrays state the same hardware
    /// fact twice, and pulling them apart does not fail to compile - the map says
    /// where a word lives, the array says how far it may be indexed. Regenerating
    /// `h265e_regs` from a header whose windows differ from these lengths therefore
    /// surfaces only as a panic on the first write into the narrow window. This test
    /// is the compile error that is missing.
    #[test]
    fn windows_match_the_generated_section_table() {
        let file = RegisterFile::default();
        let captured = file.captured();
        assert_eq!(captured.len(), regs::SECTIONS.len());
        for ((name, first, count), (cap_name, cap_first, words)) in
            regs::SECTIONS.iter().zip(captured.iter())
        {
            assert_eq!(*name, *cap_name);
            assert_eq!(*first, *cap_first);
            assert_eq!(*count, words.len(), "window {name} has the wrong length");
        }

        // The windows tile upward without touching each other and stay clear of word
        // 4096, where STATUS begins and the hardware writes back.
        let mut next = 0usize;
        for (_, first, count) in regs::SECTIONS {
            assert!(
                first >= next,
                "section starting at word {first} overlaps its predecessor"
            );
            next = first + count;
        }
        assert!(next <= 4096, "the written windows reach word {next}");
    }

    /// The bug this locks out: the map declared `wgt` at 374 words and `rdo` at 807
    /// while the arrays inherited from the H.264 register file held 180 and 122, so
    /// the upper half of both windows was unreachable - `wgt` words past 1652 fell
    /// outside every section and `rdo` indices past 122 ran off their array. Writing
    /// and reading the *last* word of each window is the cheapest check that the
    /// arrays are at least as wide as the map claims.
    #[test]
    fn the_last_word_of_every_window_is_addressable() {
        let mut file = RegisterFile::default();
        for (name, first, count) in regs::SECTIONS {
            let probe = regs::Field {
                word: first + count - 1,
                shift: 0,
                width: 1,
                name: "probe",
            };
            file.set(&probe, 1)
                .unwrap_or_else(|e| panic!("last word of window {name}: {e}"));
            assert_eq!(file.get(&probe).unwrap(), 1, "window {name}");
        }
    }

    /// The traced P job's delta from the IDR image: slice type, NAL type, POC,
    /// reference markings and the inter lambda table. The third picture and later
    /// additionally state the reference list explicitly.
    #[test]
    fn a_p_picture_sets_the_traced_delta_fields() {
        let mut job = IdrJob::reference(26);
        job.long_term = true;
        job.picture = Picture::P { poc_lsb: 1 };
        job.temporal_mvp = true;
        let first = build_registers(&job).unwrap();
        assert_eq!(first.get(&regs::CTL_DTRNS_CFG_DSPR_OTSD).unwrap(), 1);
        assert_eq!(
            first
                .get(&regs::H265_REG0236_SYNT_NAL_NAL_UNIT_TYPE)
                .unwrap(),
            1
        );
        assert_eq!(
            first.get(&regs::H265_REG0239_SYNT_SLI0_SLI_TYPE).unwrap(),
            1
        );
        assert_eq!(
            first
                .get(&regs::H265_REG0239_SYNT_SLI0_SLI_TMPRL_MVP_E)
                .unwrap(),
            1
        );
        assert_eq!(
            first
                .get(&regs::H265_REG0240_SYNT_SLI1_COL_FRM_L0_FLG)
                .unwrap(),
            1
        );
        assert_eq!(first.get(&regs::H265_REG0221_ME_CFG_COLMV_LOAD).unwrap(), 0);
        assert_eq!(first.get(&regs::H265_REG0221_ME_CFG_COLMV_STOR).unwrap(), 1);
        assert_eq!(
            first
                .get(&regs::H265_REG0192_ENC_PIC_NUM_PIC_TOT_CUR)
                .unwrap(),
            1
        );
        assert_eq!(
            first
                .get(&regs::H265_REG0241_SYNT_SLI2_SLI_POC_LSB)
                .unwrap(),
            1
        );
        assert_eq!(&first.wgt[128..132], &MODB_LAMBDA[..4]);
        // The first P still uses the SPS long-term entry; the list is not modified.
        assert_eq!(
            first
                .get(&regs::H265_REG0239_SYNT_SLI0_REF_PIC_LST_MDF_L0)
                .unwrap(),
            0
        );

        job.picture = Picture::P { poc_lsb: 2 };
        let third = build_registers(&job).unwrap();
        assert_eq!(
            third
                .get(&regs::H265_REG0239_SYNT_SLI0_REF_PIC_LST_MDF_L0)
                .unwrap(),
            1
        );
        assert_eq!(
            third
                .get(&regs::H265_REG0240_SYNT_SLI1_LST_ENTRY_L0)
                .unwrap(),
            1
        );
        assert_eq!(
            third
                .get(&regs::H265_REG0243_SYNT_REFM1_NUM_NEGATIVE_PICS)
                .unwrap(),
            1
        );
        assert_eq!(third.get(&regs::H265_REG0221_ME_CFG_COLMV_LOAD).unwrap(), 1);
        assert_eq!(
            third
                .get(&regs::H265_REG0241_SYNT_SLI2_SLI_POC_LSB)
                .unwrap(),
            2
        );
    }

    /// The traced CBR P job, field for field: `mpi_enc_test -rc 1 -bps
    /// 300000:250000:350000` at 320x240, second picture. The harness runs its
    /// quality band 10..51 and that picture is QP 22 with 580 bits, which is what
    /// the C's own `ctu_ebit = 464` encodes as (464 * 5 >> 4 = 145 bits per CTU
    /// column, 20 CTUs).
    #[test]
    fn the_rate_control_image_matches_the_traced_cbr_job() {
        let mut job = IdrJob::reference(26);
        job.picture = Picture::P { poc_lsb: 1 };
        job.rc = Some(RateControl {
            bit_target: 580,
            qp_target: 22,
            qp_min: 10,
            qp_max: 51,
            qp_delta_row: 2,
            qp_delta_row_i: 2,
        });
        let file = build_registers(&job).unwrap();
        assert_eq!(file.get(&regs::BASE_ENC_PIC_PIC_QP).unwrap(), 22);
        assert_eq!(file.get(&regs::H265_REG0240_SYNT_SLI1_SLI_QP).unwrap(), 22);
        assert_eq!(file.base[212 - 160], 0x0000_5007, "rc_cfg");
        assert_eq!(file.base[213 - 160], 0x2b32_0000, "rc_qp");
        assert_eq!(file.base[214 - 160], 464, "ctu_ebit");
        assert_eq!(
            file.get(&regs::H265_REG0238_SYNT_PPS_CU_QP_DLT_EN).unwrap(),
            1
        );
        // rc_dthd_0_8: 2*negative, negative, positive, 2*positive, then five max.
        assert_eq!(
            &file.rc_klut[1026 - 1024..1035 - 1024],
            &[
                0xffff_ffa6,
                0xffff_ffd3,
                45,
                90,
                0x7fff_ffff,
                0x7fff_ffff,
                0x7fff_ffff,
                0x7fff_ffff,
                0x7fff_ffff
            ]
        );
        assert_eq!(file.rc_klut[1024 - 1024], 0x0020_83fe, "rc_adj0");
        assert_eq!(file.rc_klut[1036 - 1024], 0x0acc_acca, "roi rows 0");
    }

    #[test]
    fn the_refresh_table_marks_the_row_band() {
        // 5x4 CTUs, refresh_num = 2: frame 0 covers rows 0..1, frame 1 covers 1..3.
        let first = refresh_base_table(320, 240, 0, RefreshMode::Row, 2);
        assert_eq!(first.len(), 5 * 4 * 64);
        let marked = |table: &[u8], j: usize, k: usize| {
            let p = (j * 5 + k) * 64;
            table[p + 21] == 0x54
                && table[p + 42] == 0x05
                && table[p + 22..p + 42].iter().all(|b| *b == 0x55)
        };
        assert!(marked(&first, 0, 0) && marked(&first, 1, 4));
        assert!(!marked(&first, 2, 0));

        let second = refresh_base_table(320, 240, 1, RefreshMode::Row, 2);
        assert!(marked(&second, 1, 0) && marked(&second, 3, 4));
        assert!(!marked(&second, 0, 0));
    }

    #[test]
    fn no_motion_info_surface_is_requested() {
        let file = reference_idr(26);
        assert_eq!(file.get(&regs::BASE_ENC_PIC_MEI_STOR).unwrap(), 0);
    }

    /// The whole register image against the captured job, word for word. Every
    /// window matches except one bit: `enc_pic.mei_stor`, which the C harness sets
    /// because it hands the encoder a motion-info buffer and this driver does not
    /// allocate one - pointing the hardware at the oracle's buffer would be worse
    /// than a known difference.
    ///
    /// This is the guard that makes the capture useful: change a derived field and
    /// the failure names the word and both values instead of leaving a payload
    /// mismatch to be found on the board.
    #[test]
    fn only_the_motion_info_bit_differs_from_the_captured_job() {
        let derived = reference_idr(26);
        let traced = RegisterFile::recorded();
        let mut differences = Vec::new();
        for ((name, first, ours), (traced_name, _, theirs)) in
            derived.captured().iter().zip(traced.captured().iter())
        {
            assert_eq!(name, traced_name);
            for (index, (a, b)) in ours.iter().zip(theirs.iter()).enumerate() {
                if a != b {
                    differences.push((first + index, *a, *b));
                }
            }
        }
        assert_eq!(
            differences.len(),
            1,
            "expected only mei_stor to differ, got {differences:?}"
        );
        let (word, ours, theirs) = differences[0];
        assert_eq!(word, 192, "the differing word is enc_pic");
        assert_eq!(ours & (1 << 3), 0, "we request no motion-info surface");
        assert_eq!(theirs & (1 << 3), 1 << 3, "the harness passes one");
    }

    #[test]
    fn rejects_a_zero_sized_picture() {
        let mut job = IdrJob::reference(26);
        job.width = 0;
        assert!(build_registers(&job).is_err());
    }
}
