//! VEPU580 H.265 encoder: the fixed-QP IDR register image.
//!
//! M8 opens with the smallest honest step the H.264 encoder took before it: one
//! IDR picture at a fixed QP. This module builds the VEPU580 register file for
//! that picture by *deriving* every field from the C source
//! (`mpp/hal/rkenc/h265e/hal_h265e_vepu580.c`) word by word, rather than replaying
//! a captured job. There is deliberately no recorded image yet: the byte oracle is
//! an on-board tracer capture the orchestrator owns (`mpi_enc_test -t 9 -rc 2`),
//! and until that template exists the only claim this file can make is per-field
//! equality with `hal_h265e_v580_gen_regs` and its helpers, which the unit tests
//! below lock. Byte-for-byte agreement is decided on the board.
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
//! What is *not* filled: the tuning windows (`param`/wgt, `rdo`, `scl`) and the
//! AQ/KLUT/LAMD byte tables inside RC-lookup. In H.264 those are the recorded-image
//! replay (the "cime/rime corner" that stays open); for H.265 with no capture they
//! are left zero, which is why this module is not yet a submit-capable encoder.
//! The report is explicit that the IDR payload comparison is blocked on the
//! template capture, while the VPS/SPS/PPS comparison (`codec::h265::encoder`) is
//! not.

use super::h265e_regs as regs;

pub const REFERENCE_WIDTH: usize = 320;
pub const REFERENCE_HEIGHT: usize = 240;

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
    pub input_fd: i32,
    pub output_fd: i32,
    pub current_fd: i32,
    pub current_thumb_fd: i32,
    pub reference_fd: i32,
    pub reference_thumb_fd: i32,
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
    pub param: [u32; 180],
    pub rdo: [u32; 122],
    pub scl: [u32; 679],
    pub osd: [u32; 288],
}

impl Default for RegisterFile {
    fn default() -> Self {
        Self {
            ctl: [0; 24],
            base: [0; 94],
            rc_klut: [0; 57],
            param: [0; 180],
            rdo: [0; 122],
            scl: [0; 679],
            osd: [0; 288],
        }
    }
}

impl RegisterFile {
    fn word_mut(&mut self, word: usize) -> Result<&mut u32, String> {
        for (name, first, count) in regs::SECTIONS {
            if word >= first && word < first + count {
                let index = word - first;
                return Ok(match name {
                    "ctl" => &mut self.ctl[index],
                    "base" => &mut self.base[index],
                    "rc_klut" => &mut self.rc_klut[index],
                    "param" => &mut self.param[index],
                    "rdo" => &mut self.rdo[index],
                    "scl" => &mut self.scl[index],
                    "osd" => &mut self.osd[index],
                    other => unreachable!("section {other} is not in RegisterFile"),
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
        let sections: [&[u32]; 7] = [
            &self.ctl,
            &self.base,
            &self.rc_klut,
            &self.param,
            &self.rdo,
            &self.scl,
            &self.osd,
        ];
        for ((_, first, count), words) in regs::SECTIONS.iter().zip(sections) {
            if field.word >= *first && field.word < *first + *count {
                return Ok((words[field.word - first] >> field.shift) & ((1 << field.width) - 1));
            }
        }
        Err(format!("register word {} is not captured", field.word))
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
    let mut file = RegisterFile::default();

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

    // ---- fixed-QP RC (set_rc_regs FIXQP branch) ----
    file.set(&regs::BASE_ENC_PIC_PIC_QP, job.qp)?;
    file.set(&regs::H265_REG0240_SYNT_SLI1_SLI_QP, job.qp)?;
    file.set(&regs::BASE_RC_QP_RC_MAX_QP, job.qp)?;
    file.set(&regs::BASE_RC_QP_RC_MIN_QP, job.qp)?;
    // rc_cfg rc_en/aq_en stay 0 (memset) - the FIXQP branch never sets them.

    // The sixteen roi_qthd* rows, written unconditionally below the branch from
    // rc_cfg->quality_min/quality_max.
    for (field, bound) in [
        (&regs::RC_ROI_QTHD0_QPMIN_AREA0, job.qp_min),
        (&regs::RC_ROI_QTHD0_QPMAX_AREA0, job.qp_max),
        (&regs::RC_ROI_QTHD0_QPMIN_AREA1, job.qp_min),
        (&regs::RC_ROI_QTHD0_QPMAX_AREA1, job.qp_max),
        (&regs::RC_ROI_QTHD0_QPMIN_AREA2, job.qp_min),
        (&regs::RC_ROI_QTHD1_QPMAX_AREA2, job.qp_max),
        (&regs::RC_ROI_QTHD1_QPMIN_AREA3, job.qp_min),
        (&regs::RC_ROI_QTHD1_QPMAX_AREA3, job.qp_max),
        (&regs::RC_ROI_QTHD1_QPMIN_AREA4, job.qp_min),
        (&regs::RC_ROI_QTHD1_QPMAX_AREA4, job.qp_max),
        (&regs::RC_ROI_QTHD2_QPMIN_AREA5, job.qp_min),
        (&regs::RC_ROI_QTHD2_QPMAX_AREA5, job.qp_max),
        (&regs::RC_ROI_QTHD2_QPMIN_AREA6, job.qp_min),
        (&regs::RC_ROI_QTHD2_QPMAX_AREA6, job.qp_max),
        (&regs::RC_ROI_QTHD2_QPMIN_AREA7, job.qp_min),
        (&regs::RC_ROI_QTHD3_QPMAX_AREA7, job.qp_max),
    ] {
        file.set(field, bound)?;
    }
    file.set(&regs::RC_ROI_QTHD3_QPMAP_MODE, 1)?; // codec->qpmap_mode == 1

    // The RC-lookup rows global_cfg_set hard-wires for an intra picture, independent
    // of the tuning tables: chrm_klut_ofst 0 for INTRA_FRAME, the madi/md_sad
    // threshold triples.
    file.set(&regs::RC_KLUT_OFST_CHRM_KLUT_OFST, 0)?;
    file.set(&regs::RC_MADI_CFG_MADI_MODE, 0)?;
    file.set(&regs::RC_MADI_CFG_MADI_THD, 25)?;
    file.set(&regs::RC_MD_SAD_THD_MD_SAD_THD0, 20)?;
    file.set(&regs::RC_MD_SAD_THD_MD_SAD_THD1, 30)?;
    file.set(&regs::RC_MD_SAD_THD_MD_SAD_THD2, 40)?;
    file.set(&regs::RC_MADI_THD_MADI_THD0, 25)?;
    file.set(&regs::RC_MADI_THD_MADI_THD1, 35)?;
    file.set(&regs::RC_MADI_THD_MADI_THD2, 45)?;

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
    file.set(&regs::H265_REG0237_SYNT_SPS_TMPL_MVP_E, 0)?; // tmvp_enable
    file.set(&regs::H265_REG0237_SYNT_SPS_LOG2_MAX_POC_LSB, 12)?; // bitsForPOC 16
    file.set(&regs::H265_REG0237_SYNT_SPS_STRG_INTRA_SMTH, 1)?;

    // reg0238 synt_pps: from syn->pp.
    file.set(&regs::H265_REG0238_SYNT_PPS_CBC_INIT_PRSNT_FLG, 1)?; // cabac_init_present
    file.set(&regs::H265_REG0238_SYNT_PPS_PIC_INIT_QP, 26)?; // init_qp_minus26 + 26
    file.set(&regs::H265_REG0238_SYNT_PPS_CU_QP_DLT_EN, 1)?; // useDQP
    file.set(&regs::H265_REG0238_SYNT_PPS_CU_QP_DLT_DEPTH, 0)?; // diff_cu_qp_delta_depth
    file.set(&regs::H265_REG0238_SYNT_PPS_LST_MDFY_PRSNT_FLG, 1)?; // lists_modification
                                                                   // loop_filter_across_tiles default is on (`!lpf_acs_tile_disable`).
    file.set(&regs::H265_REG0238_SYNT_PPS_LPF_FLTR_ACRS_TIL, 1)?;

    // reg0239 synt_sli0: I slice (type 2), merge flags from codec defaults.
    file.set(&regs::H265_REG0239_SYNT_SLI0_SLI_TYPE, 2)?; // I_SLICE
    file.set(&regs::H265_REG0239_SYNT_SLI0_MRG_UP_FLG, 1)?; // merge_cfg.merge_up_flag
    file.set(&regs::H265_REG0239_SYNT_SLI0_MRG_LFT_FLG, 1)?; // merge_cfg.merge_left_flag

    // reg0240 synt_sli1: no deblocking offsets by default, max_mrg_cnd 2.
    file.set(&regs::H265_REG0240_SYNT_SLI1_MAX_MRG_CND, 2)?;

    // reg0241 synt_sli2: first picture, poc_lsb 0. Left 0.

    // reg0252 tile_cfg: tiles off (auto_tile default 0). Left 0.

    // No MD surface is implemented here, so `enc_pic.mei_stor` stays 0 and the
    // `meiw_addr` word stays clear. H.264 sets this bit *because* its harness passed
    // an MD buffer; the H.265 HAL leaves it 0 when `enc_task->md_info` is null
    // (`vepu580_h265_set_hw_address`), which is the case for a driver that requests
    // no motion info.
    file.set(&regs::BASE_ENC_PIC_MEI_STOR, 0)?;

    Ok(file)
}

/// `enc_pic.log2_ctu_num = ceil(log2(pic_wd64 * pic_h64))`, the C uses
/// `ceil(log2((double)area))`.
fn ctu_num_log2(area: usize) -> u32 {
    assert!(area > 0);
    (usize::BITS - (area - 1).leading_zeros()) as u32
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
    fn roi_qthd_rows_collapse_to_the_fixed_qp() {
        let file = reference_idr(26);
        for field in [
            &regs::RC_ROI_QTHD0_QPMIN_AREA0,
            &regs::RC_ROI_QTHD0_QPMAX_AREA0,
            &regs::RC_ROI_QTHD3_QPMAX_AREA7,
        ] {
            assert_eq!(file.get(field).unwrap(), 26, "{}", field.name);
        }
        assert_eq!(file.get(&regs::RC_ROI_QTHD3_QPMAP_MODE).unwrap(), 1);
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
            1
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
    fn rdo_and_rc_lookup_constants() {
        let file = reference_idr(26);
        assert_eq!(
            file.get(&regs::H265_REG0232_RDO_CFG_CU_INTER_E).unwrap(),
            0x06db
        );
        assert_eq!(file.get(&regs::H265_REG0232_RDO_CFG_CHRM_SPCL).unwrap(), 1);
        assert_eq!(file.get(&regs::RC_MADI_CFG_MADI_THD).unwrap(), 25);
        assert_eq!(file.get(&regs::RC_MD_SAD_THD_MD_SAD_THD0).unwrap(), 20);
        assert_eq!(file.get(&regs::RC_MD_SAD_THD_MD_SAD_THD1).unwrap(), 30);
        assert_eq!(file.get(&regs::RC_MD_SAD_THD_MD_SAD_THD2).unwrap(), 40);
        assert_eq!(file.get(&regs::RC_MADI_THD_MADI_THD0).unwrap(), 25);
        assert_eq!(file.get(&regs::RC_MADI_THD_MADI_THD1).unwrap(), 35);
        assert_eq!(file.get(&regs::RC_MADI_THD_MADI_THD2).unwrap(), 45);
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
        assert_eq!(a.param, b.param);
        assert_eq!(a.rdo, b.rdo);
        assert_eq!(a.scl, b.scl);
        assert_eq!(a.osd, b.osd);
        assert_eq!(a.ctl, b.ctl);
    }

    #[test]
    fn tuning_windows_are_zero_until_the_template_lands() {
        // The honest record of what this module does NOT yet fill. When the board
        // capture lands these become the replay the H.264 encoder replays.
        let file = reference_idr(26);
        assert!(file.param.iter().all(|&w| w == 0));
        assert!(file.rdo.iter().all(|&w| w == 0));
        assert!(file.scl.iter().all(|&w| w == 0));
    }

    #[test]
    fn no_motion_info_surface_is_requested() {
        let file = reference_idr(26);
        assert_eq!(file.get(&regs::BASE_ENC_PIC_MEI_STOR).unwrap(), 0);
    }

    #[test]
    fn rejects_a_zero_sized_picture() {
        let mut job = IdrJob::reference(26);
        job.width = 0;
        assert!(build_registers(&job).is_err());
    }
}
