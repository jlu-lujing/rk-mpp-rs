//! VEPU580 H.264 encoder.
//!
//! The first supported job is one IDR picture from the board reference
//! configuration: Constrained Baseline, CAVLC, GOP 1, level 3.0. Its register and
//! DMA layout starts from a captured VEPU580 job from the installed t6 MPP
//! library, and the fields that a second configuration changes are regenerated
//! over that image - so far the addresses, the picture geometry and the QP words
//! of `setup_vepu580_rc_base`. Keeping the rest behind
//! `validate_reference_config` gives the project a byte-exact integration oracle
//! while the parameterized generator grows to cover the rest.
//!
//! `tools/gen_vepu580_regs.py` derives every field position from MPP's own
//! `vepu580_common.h`, and the test over the recorded image checks those
//! positions against reality: the captured job decodes to QP 26 in
//! `enc_pic.pic_qp`, `rc_qp.rc_min_qp` and `rc_qp.rc_max_qp`, which is exactly
//! what the reference writes there for fixed QP.

use crate::codec::h264::encoder::{self, EncoderConfig, RcMode};
use crate::codec::h264::sei;
use crate::codec::rc::model as rc_model;
use crate::service::dma_heap::DmaBuf;
use crate::service::{Batch, Client, MppService, RegOffsetInfo};

use super::h264e_vepu580_template as template;
use super::vepu580_regs;

pub const REFERENCE_WIDTH: usize = 320;
pub const REFERENCE_HEIGHT: usize = 240;
pub const REFERENCE_OUTPUT_MARGIN: usize = 4096;

const CTL_OFFSET: u32 = 0;
const BASE_OFFSET: u32 = 640;
const RCKLUT_OFFSET: u32 = 4096;
const PARAM_OFFSET: u32 = 5888;
const RDO_OFFSET: u32 = 8192;
const SCL_OFFSET: u32 = 8704;
const OSD_OFFSET: u32 = 12288;
const STATUS_OFFSET: u32 = 16384;
const HW_STATUS_OFFSET: u32 = 44;

const BASE_ADR_SRC0: usize = 0;
const BASE_ADR_SRC1: usize = 1;
const BASE_ADR_SRC2: usize = 2;
const BASE_RFPW_H: usize = 3;
const BASE_RFPW_B: usize = 4;
const BASE_RFPR_H: usize = 5;
const BASE_RFPR_B: usize = 6;
const BASE_DSPW: usize = 9;
const BASE_DSPR: usize = 10;
const BASE_BSBT: usize = 12;
const BASE_BSBB: usize = 13;
const BASE_BSBR: usize = 14;
const BASE_ADR_BSBS: usize = 15;
/// `sw171`, the motion derivation (MEI) surface. The reference job was captured
/// with MD info requested, so the recorded word holds that run's fd; this driver
/// implements no MD output and clears it. Submitting the recorded value would
/// hand the hardware a dma-buf handle from somebody else's process.
const BASE_ADR_MEI: usize = 11;
const BASE_ENC_RSL: usize = 36;
const BASE_SRC_FILL: usize = 37;
const BASE_SRC_STRD0: usize = 45;
const BASE_SRC_STRD1: usize = 46;
const BASE_RC_CFG: usize = 52;
const BASE_ME_RNGE: usize = 60;
const BASE_ME_CACH: usize = 62;

/// `meiw_addr`, the surface the encoder writes per-macroblock motion derivation
/// info to when `enc_pic.mei_stor` is set.
const BASE_MEIW: usize = 11;

const REG_SRC_1_OFFSET: u32 = 161;
const REG_SRC_2_OFFSET: u32 = 162;
const REG_BSBT_END: u32 = 172;
const REG_BSBS_START: u32 = 175;
const REG_RFPW_B: u32 = 164;
const REG_RFPR_B: u32 = 166;

const FBC_HEADER_SIZE: usize = 8192;
const STATUS_WORDS: usize = 180;
const HW_ERROR_MASK: u32 = 0x3f0;

/// One encoded picture. `headers` is non-empty only when the picture is an IDR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub annex_b: Vec<u8>,
    pub payload: Vec<u8>,
    pub headers: Vec<u8>,
    pub is_idr: bool,
    pub frame_num: u32,
    pub poc: i32,
    /// What the hardware reported about this picture.
    pub statistics: Statistics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedIdr {
    /// Complete Annex-B access unit: SPS, PPS and the hardware IDR.
    pub annex_b: Vec<u8>,
    /// Hardware-produced NAL, including its start code.
    pub payload: Vec<u8>,
    pub headers: Vec<u8>,
}

/// One reconstruction slot. VEPU580 writes the FBC-compressed reconstruction and
/// its thumb CMO surface here, and reads the *previous* slot's surfaces back as
/// the reference of a P picture - `setup_vepu580_recn_refr` takes both from
/// `ctx->hw_recn`, indexed by `frms->curr_idx` and `frms->refr_idx`.
pub struct Recon {
    pub pixel: DmaBuf,
    pub thumb: DmaBuf,
}

impl Recon {
    pub fn alloc(pixel_size: usize, thumb_size: usize) -> Result<Self, String> {
        Ok(Self {
            pixel: DmaBuf::alloc("system", pixel_size, false)
                .map_err(|e| format!("allocate H.264 reconstruction: {e}"))?,
            thumb: DmaBuf::alloc("system", thumb_size, false)
                .map_err(|e| format!("allocate H.264 thumb: {e}"))?,
        })
    }
}

/// Where the encoder is inside the GOP, and what the slice header therefore says.
///
/// `poc` follows PocType 20 with no reordering, which is the only case this
/// encoder reaches while B frames are unsupported: the standard's recurrence
/// `poc = prev_poc + 2 * (frame_num - prev_frame_num)` reduced to +2 per picture,
/// reset at an IDR. `h264e_dpb` does the same (`frame_num = 0` on IDR, otherwise
/// the running count) and the C encoder's second picture in a GOP carries
/// `frame_num = 1, poc_lsb = 2`, which is what these numbers have to reproduce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameState {
    /// Pictures encoded by this session so far.
    pub index: usize,
    pub is_idr: bool,
    pub frame_num: u32,
    pub idr_pic_id: u32,
    pub poc: i32,
}

impl Default for FrameState {
    fn default() -> Self {
        Self {
            index: 0,
            is_idr: true,
            frame_num: 0,
            idr_pic_id: 0,
            poc: 0,
        }
    }
}

impl FrameState {
    /// The state of the next picture. `gop` is the IDR interval: picture 0 and
    /// every `gop`-th picture after it opens a new GOP with an IDR.
    pub fn next(&self, gop: u32) -> FrameState {
        let gop = gop.max(1) as usize;
        let index = self.index + 1;
        let is_idr = index % gop == 0;
        if is_idr {
            return FrameState {
                index,
                is_idr: true,
                frame_num: 0,
                // `h264e_slice_init` alternates idr_pic_id across IDR pictures, and
                // a P picture carries the id of the IDR that opened its GOP.
                idr_pic_id: (self.idr_pic_id + 1) & 1,
                poc: 0,
            };
        }
        let frame_num = if is_idr { 0 } else { self.frame_num + 1 };
        FrameState {
            index,
            is_idr: false,
            frame_num,
            idr_pic_id: self.idr_pic_id,
            poc: self.poc + 2 * (frame_num - self.frame_num) as i32,
        }
    }
}

/// One encode job: the picture, the buffers it moves between, and where it sits
/// in the GOP.
pub struct Job<'a> {
    pub config: &'a EncoderConfig,
    pub input_fd: i32,
    pub output_fd: i32,
    /// The reconstruction surface this picture is written to, and its thumb.
    pub current_fd: i32,
    pub current_thumb_fd: i32,
    /// The reconstruction surface to read the reference from - the previous
    /// slot, or this one for an IDR, where there is nothing to read.
    pub reference_fd: i32,
    pub reference_thumb_fd: i32,
    /// The motion derivation info surface, `meiw_addr`.
    pub md_fd: i32,
    pub state: FrameState,
    pub output_capacity: usize,
    /// What rate control decided for this picture, or `None` under fixed QP.
    pub rc: Option<RateControl>,
}

/// What rate control decided about one picture.
///
/// `setup_vepu580_rc_base` reads exactly these four things out of `EncRcTaskInfo`
/// plus the two hardware QP-row defaults, so this is the whole surface the model
/// in `rc_model_v2.c` has on the register file. `bit_target` is the picture's bit
/// budget, not the configured bit rate: the model divides the rate by the frame
/// rate and then moves bits between intra and inter pictures, which is the part
/// still to be ported.
#[derive(Debug, Clone, Copy)]
pub struct RateControl {
    /// `rc_info->bit_target`, in bits for this picture.
    pub bit_target: i32,
    /// `rc_info->quality_target`, the QP the model aims at.
    pub qp_target: u32,
    pub qp_min: u32,
    pub qp_max: u32,
    /// `hw->qp_delta_row` and `hw->qp_delta_row_i`, the per-row QP slack. The
    /// VEPU580 HAL's default block sets both to 2.
    pub qp_delta_row: u32,
    pub qp_delta_row_i: u32,
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
    pub offsets: Vec<RegOffsetInfo>,
}

impl RegisterFile {
    /// The section array and index a register word lives in.
    fn word_mut(&mut self, word: usize) -> Result<&mut u32, String> {
        for (name, first, count) in vepu580_regs::SECTIONS {
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
        Err(format!(
            "register word {word} is outside every section this crate captures"
        ))
    }

    /// Write one named field, clearing the bits the previous value left behind.
    pub fn set(&mut self, field: &vepu580_regs::Field, value: u32) -> Result<(), String> {
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

    /// Read one named field back, which is how the tests check what was written.
    pub fn get(&self, field: &vepu580_regs::Field) -> Result<u32, String> {
        let sections: [&[u32]; 7] = [
            &self.ctl,
            &self.base,
            &self.rc_klut,
            &self.param,
            &self.rdo,
            &self.scl,
            &self.osd,
        ];
        for ((name, first, count), words) in vepu580_regs::SECTIONS.iter().zip(sections) {
            let _ = name;
            if field.word >= *first && field.word < *first + *count {
                return Ok((words[field.word - first] >> field.shift) & ((1 << field.width) - 1));
            }
        }
        Err(format!("register word {} is not captured", field.word))
    }

    /// The QP fields of `setup_vepu580_rc_base` in fixed-QP mode: the picture QP
    /// and the two RC bounds, which the reference sets to the same value and then
    /// returns on. Everything else the function owns - `rc_en`, `aq_en`,
    /// `rc_ctu_num`, `rc_qp_range`, the `rc_adj` ladder, the `rc_dthd` thresholds,
    /// the ROI area thresholds and `qpmap_mode` - belongs to the rate-control path
    /// and stays at the zero the reference image carries. Those are exactly the
    /// words a CBR or VBR encode will have to fill, which is the remaining M7 work.
    /// The profile- and entropy-dependent fields of `setup_vepu580_codec`.
    ///
    /// `vlc_lmt` and `rect_size` are CAVLC and Baseline-at-or-below-3.0 knobs,
    /// `etpy_mode` and `trns_8x8` mirror the PPS flags, and `cbc_init_idc` is the
    /// slice field - `h264e_slice_setup` sets `cabac_init_idc = -1` when CABAC is
    /// off, which lands as 3 in the two-bit register field. A recorded Baseline
    /// job therefore has 3 there, and a Main job 0 from the default
    /// `cabac_init_idc`; taking either value from the image would encode every
    /// profile the way the captured one was encoded.
    fn apply_profile(&mut self, config: &EncoderConfig) -> Result<(), String> {
        let cabac = u32::from(config.entropy_coding);
        self.set(&vepu580_regs::H264_SYNT_PPS_ETPY_MODE, cabac)?;
        self.set(
            &vepu580_regs::H264_SYNT_PPS_TRNS_8X8,
            u32::from(config.transform_8x8),
        )?;
        // `h264e_sps_init` sets `direct8x8_inference = transform8x8_mode`, which is
        // also what `build_sps` writes as `direct_8x8_inference_flag`. The High
        // reference differs from Baseline in exactly this bit of `synt_sps`.
        self.set(
            &vepu580_regs::H264_SYNT_SPS_DRCT_8X8,
            u32::from(config.transform_8x8),
        )?;
        self.set(
            &vepu580_regs::H264_SYNT_SLI0_CBC_INIT_IDC,
            if config.entropy_coding { 0 } else { 3 },
        )?;
        self.set(
            &vepu580_regs::H264_RDO_CFG_RECT_SIZE,
            u32::from(config.profile == encoder::PROFILE_BASELINE && config.level_idc <= 30),
        )?;
        self.set(
            &vepu580_regs::H264_RDO_CFG_VLC_LMT,
            u32::from(config.profile < encoder::PROFILE_MAIN && !config.entropy_coding),
        )?;
        Ok(())
    }

    fn apply_fixed_qp(&mut self, qp: u32) -> Result<(), String> {
        self.set(&vepu580_regs::BASE_ENC_PIC_PIC_QP, qp)?;
        self.set(&vepu580_regs::BASE_RC_QP_RC_MIN_QP, qp)?;
        self.set(&vepu580_regs::BASE_RC_QP_RC_MAX_QP, qp)?;
        Ok(())
    }

    /// The rate-control half of `setup_vepu580_rc_base` - the branch that only
    /// runs under CBR, VBR or AVBR. The fixed-QP branch above is the other one,
    /// and it is the branch that writes three words and returns.
    ///
    /// Almost all of it is constants: the `qp_adj` ladder, the ROI threshold rows,
    /// the `qpmap_mode`, the motion and `madi` threshold triples. What is computed
    /// is the per-macroblock bit target and the three thresholds derived from it,
    /// which is why the arithmetic is written in the same order the C does it -
    /// `mb_target_bits` comes from the *truncated* scaled target, and the
    /// thresholds truncate again.
    pub fn apply_rate_control(
        &mut self,
        config: &EncoderConfig,
        is_idr: bool,
        rc: &RateControl,
    ) -> Result<(), String> {
        let mb_w = (config.coded_width() / 16) as i32;
        let mb_h = (config.coded_height() / 16) as i32;

        // `mb_target_bits_mul_16 = (bit_target << 4) / (mb_w * mb_h)`, the target
        // in sixteenths of a bit per macroblock. A picture aimed at more than
        // 0x100000 is reset to 0x50000 rather than clamped - the reference does not
        // trust the hardware above that figure.
        let mut target_16 = rc.bit_target.wrapping_shl(4) / (mb_w * mb_h);
        if target_16 >= 0x0010_0000 {
            target_16 = 0x0005_0000;
        }
        let mb_target_bits = (target_16 * mb_w) >> 4;
        let negative_bits_thd = 0 - 5 * mb_target_bits / 16;
        let positive_bits_thd = 5 * mb_target_bits / 16;

        self.set(&vepu580_regs::BASE_ENC_PIC_PIC_QP, rc.qp_target)?;
        self.set(&vepu580_regs::BASE_RC_CFG_RC_EN, 1)?;
        self.set(&vepu580_regs::BASE_RC_CFG_AQ_EN, 1)?;
        self.set(&vepu580_regs::BASE_RC_CFG_AQ_MODE, 0)?;
        self.set(&vepu580_regs::BASE_RC_CFG_RC_CTU_NUM, mb_w as u32)?;
        self.set(
            &vepu580_regs::BASE_RC_QP_RC_QP_RANGE,
            if is_idr {
                rc.qp_delta_row_i
            } else {
                rc.qp_delta_row
            },
        )?;
        self.set(&vepu580_regs::BASE_RC_QP_RC_MAX_QP, rc.qp_max)?;
        self.set(&vepu580_regs::BASE_RC_QP_RC_MIN_QP, rc.qp_min)?;
        self.set(&vepu580_regs::BASE_RC_TGT_CTU_EBIT, target_16 as u32)?;

        // The quantiser-adjustment ladder, indexed by how far a block came in
        // under its target. The negative entries land as two's complement in the
        // five-bit fields, which is why a capture shows 30 and 31 where the source
        // says -2 and -1.
        for (field, value) in [
            (&vepu580_regs::RCKLUT_RC_ADJ0_QP_ADJ0, -2),
            (&vepu580_regs::RCKLUT_RC_ADJ0_QP_ADJ1, -1),
            (&vepu580_regs::RCKLUT_RC_ADJ0_QP_ADJ2, 0),
            (&vepu580_regs::RCKLUT_RC_ADJ0_QP_ADJ3, 1),
            (&vepu580_regs::RCKLUT_RC_ADJ0_QP_ADJ4, 2),
            (&vepu580_regs::RCKLUT_RC_ADJ1_QP_ADJ5, 0),
            (&vepu580_regs::RCKLUT_RC_ADJ1_QP_ADJ6, 0),
            (&vepu580_regs::RCKLUT_RC_ADJ1_QP_ADJ7, 0),
            (&vepu580_regs::RCKLUT_RC_ADJ1_QP_ADJ8, 1),
        ] {
            self.set_signed(field, value)?;
        }

        let dthd = [
            4 * negative_bits_thd,
            negative_bits_thd,
            positive_bits_thd,
            4 * positive_bits_thd,
            0x7fff_ffff,
            0x7fff_ffff,
            0x7fff_ffff,
            0x7fff_ffff,
            0x7fff_ffff,
        ];
        for (index, value) in dthd.iter().enumerate() {
            let word = self.rc_klut_word_mut(vepu580_regs::RCKLUT_RC_DTHD_0_8.word + index)?;
            *word = *value as u32;
        }

        // Every ROI area gets the picture's own bounds: with no ROI configured the
        // rows are uniform, and the hardware still reads them.
        for (field, bound) in [
            (&vepu580_regs::RCKLUT_ROI_QTHD0_QPMIN_AREA0, rc.qp_min),
            (&vepu580_regs::RCKLUT_ROI_QTHD0_QPMAX_AREA0, rc.qp_max),
            (&vepu580_regs::RCKLUT_ROI_QTHD0_QPMIN_AREA1, rc.qp_min),
            (&vepu580_regs::RCKLUT_ROI_QTHD0_QPMAX_AREA1, rc.qp_max),
            (&vepu580_regs::RCKLUT_ROI_QTHD0_QPMIN_AREA2, rc.qp_min),
            (&vepu580_regs::RCKLUT_ROI_QTHD1_QPMAX_AREA2, rc.qp_max),
            (&vepu580_regs::RCKLUT_ROI_QTHD1_QPMIN_AREA3, rc.qp_min),
            (&vepu580_regs::RCKLUT_ROI_QTHD1_QPMAX_AREA3, rc.qp_max),
            (&vepu580_regs::RCKLUT_ROI_QTHD1_QPMIN_AREA4, rc.qp_min),
            (&vepu580_regs::RCKLUT_ROI_QTHD1_QPMAX_AREA4, rc.qp_max),
            (&vepu580_regs::RCKLUT_ROI_QTHD2_QPMIN_AREA5, rc.qp_min),
            (&vepu580_regs::RCKLUT_ROI_QTHD2_QPMAX_AREA5, rc.qp_max),
            (&vepu580_regs::RCKLUT_ROI_QTHD2_QPMIN_AREA6, rc.qp_min),
            (&vepu580_regs::RCKLUT_ROI_QTHD2_QPMAX_AREA6, rc.qp_max),
            (&vepu580_regs::RCKLUT_ROI_QTHD2_QPMIN_AREA7, rc.qp_min),
            (&vepu580_regs::RCKLUT_ROI_QTHD3_QPMAX_AREA7, rc.qp_max),
        ] {
            self.set(field, bound)?;
        }
        self.set(&vepu580_regs::RCKLUT_ROI_QTHD3_QPMAP_MODE, 1)?;

        for (field, value) in [
            (&vepu580_regs::RCKLUT_MD_SAD_THD_MD_SAD_THD0, 4),
            (&vepu580_regs::RCKLUT_MD_SAD_THD_MD_SAD_THD1, 9),
            (&vepu580_regs::RCKLUT_MD_SAD_THD_MD_SAD_THD2, 15),
            (&vepu580_regs::RCKLUT_MADI_THD_MADI_THD0, 4),
            (&vepu580_regs::RCKLUT_MADI_THD_MADI_THD1, 9),
            (&vepu580_regs::RCKLUT_MADI_THD_MADI_THD2, 15),
        ] {
            self.set(field, value)?;
        }
        Ok(())
    }

    /// Write a signed value into a field the reference declares as a signed
    /// bitfield. The C stores the two's complement bits, so -2 in a five-bit field
    /// is 30, and a plain `set` would refuse it.
    fn set_signed(&mut self, field: &vepu580_regs::Field, value: i32) -> Result<(), String> {
        let mask = (1u32 << field.width) - 1;
        self.set(field, value as u32 & mask)
    }
}

pub struct Vepu580H264 {
    svc: MppService,
    config: EncoderConfig,
    input: DmaBuf,
    output: DmaBuf,
    started: bool,
    recons: [Recon; 2],
    md_info: DmaBuf,
    state: FrameState,
    slot: usize,
    /// The rate-control model, present only in CBR mode.
    rc: Option<rc_model::Model>,
}

impl Vepu580H264 {
    pub fn open(config: EncoderConfig) -> Result<Self, String> {
        validate_reference_config(&config)?;
        verified_on_board(&config)?;
        let input_size = input_size(&config)?;
        let output_size = input_size
            .checked_add(REFERENCE_OUTPUT_MARGIN)
            .ok_or_else(|| "H.264 output size overflow".to_string())?;
        let (pixel_size, thumb_size) = reconstruction_sizes(&config)?;

        Ok(Self {
            svc: MppService::open(Client::RkvEnc as u32)
                .map_err(|e| format!("open rkv encoder: {e}"))?,
            started: false,
            input: DmaBuf::alloc("system", input_size, false)
                .map_err(|e| format!("allocate H.264 input: {e}"))?,
            output: DmaBuf::alloc("system", output_size, false)
                .map_err(|e| format!("allocate H.264 output: {e}"))?,
            recons: [
                Recon::alloc(pixel_size, thumb_size)?,
                Recon::alloc(pixel_size, thumb_size)?,
            ],
            md_info: DmaBuf::alloc("system", mdinfo_size(&config)?, false)
                .map_err(|e| format!("allocate H.264 motion info: {e}"))?,
            state: FrameState::default(),
            slot: 0,
            rc: match config.rc_mode {
                RcMode::FixedQp => None,
                RcMode::Cbr | RcMode::Vbr | RcMode::Avbr => {
                    Some(rc_model::Model::new(rc_model_config(&config)?))
                }
            },
            config,
        })
    }

    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// The prefix an IDR carries: SPS, PPS, then the two user-data SEI NALUs.
    ///
    /// Step 17 of `mpp_enc_impl.cpp` puts `uuid_version` and then `uuid_rc_cfg`
    /// ahead of every IDR when the SEI mode is `MPP_ENC_SEI_MODE_ONE_SEQ`, which
    /// is what `mpp_enc_v2.cpp` initialises it to. Under fixed QP neither string
    /// affects one coded bit, but both are in the access unit MPP produces, so
    /// leaving them out would make this driver's output a different stream from
    /// the one being compared against.
    pub fn headers(&self) -> Result<Vec<u8>, String> {
        let mut out = self.parameter_sets()?;
        out.extend(sei::user_data(
            &sei::UUID_VERSION,
            version_info(&self.config).as_bytes(),
        ));
        let line = match self.config.rc_mode {
            RcMode::FixedQp => sei::RcCfgLine::fixqp(
                i32::from(self.config.qp_init),
                self.config.bps_target,
                self.config.bps_min,
                self.config.bps_max,
                i64::from(self.config.gop),
            ),
            // VBR renders the same line with a different mode name: the bit rate,
            // bounds and quality band are the library's own defaults in both.
            RcMode::Cbr | RcMode::Vbr | RcMode::Avbr => sei::RcCfgLine::cbr(
                self.config.bps_target,
                self.config.bps_min,
                self.config.bps_max,
                i64::from(self.config.gop),
            )
            .with_mode(match self.config.rc_mode {
                RcMode::Vbr => "vbr",
                RcMode::Avbr => "avbr",
                _ => "cbr",
            }),
        };
        out.extend(sei::user_data(
            &sei::UUID_RC_CFG,
            sei::rc_cfg_line(&line).as_bytes(),
        ));
        Ok(out)
    }

    /// The SPS and PPS for the current configuration.
    fn parameter_sets(&self) -> Result<Vec<u8>, String> {
        encoder::build_headers(&self.config).map_err(|e| e.to_string())
    }

    /// Encode one picture, continuing the GOP: the first call of a session is an
    /// IDR and every `gop`-th picture after it opens a new one.
    pub fn encode_frame(&mut self, nv12: &[u8]) -> Result<EncodedFrame, String> {
        if self.state.index != 0 || self.started {
            self.state = self.state.next(self.config.gop);
            self.slot = self.state.index % 2;
        } else {
            self.started = true;
        }
        let state = self.state;
        let expected = input_size(&self.config)?;
        if nv12.len() < expected {
            return Err(format!(
                "H.264 input is {} bytes, expected at least {expected}",
                nv12.len()
            ));
        }

        self.input
            .sync_write_begin()
            .map_err(|e| format!("H.264 input sync begin: {e}"))?;
        self.input.as_mut_slice()[..expected].copy_from_slice(&nv12[..expected]);
        self.input
            .sync_write_end()
            .map_err(|e| format!("H.264 input flush: {e}"))?;

        let reference_slot = if state.is_idr {
            self.slot
        } else {
            1 - self.slot
        };
        // The model decides the picture's bits and QP before the register file is
        // built; under fixed QP there is no decision to make.
        let rc = match self.rc.as_mut() {
            Some(model) => {
                let frame = if state.is_idr {
                    rc_model::FrameType::Intra
                } else {
                    rc_model::FrameType::InterP
                };
                let (bit_target, quality_target) = model.start(frame);
                Some(RateControl {
                    bit_target,
                    qp_target: quality_target as u32,
                    qp_min: 8,
                    qp_max: 48,
                    qp_delta_row: 2,
                    qp_delta_row_i: 2,
                })
            }
            None => None,
        };
        let job = Job {
            config: &self.config,
            input_fd: self.input.fd(),
            output_fd: self.output.fd(),
            current_fd: self.recons[self.slot].pixel.fd(),
            current_thumb_fd: self.recons[self.slot].thumb.fd(),
            md_fd: self.md_info.fd(),
            reference_fd: self.recons[reference_slot].pixel.fd(),
            reference_thumb_fd: self.recons[reference_slot].thumb.fd(),
            state,
            output_capacity: logical_output_size(&self.config)?,
            rc,
        };
        let registers = build_registers(&job)?;
        dump_job_registers(&job, &registers);

        // The SOC returns the number of encoded bytes in STATUS[0]. Reading it
        // back is mandatory: ioctl and poll completion alone do not prove that
        // a valid NAL was written.
        let mut status = vec![0u32; STATUS_WORDS];
        let mut hw_status = [0u32; 1];
        let mut batch = Batch::new();
        batch.reg_write(&registers.ctl, CTL_OFFSET);
        batch.reg_write(&registers.base, BASE_OFFSET);
        batch.reg_write(&registers.rc_klut, RCKLUT_OFFSET);
        batch.reg_write(&registers.param, PARAM_OFFSET);
        batch.reg_write(&registers.rdo, RDO_OFFSET);
        batch.reg_write(&registers.scl, SCL_OFFSET);
        batch.reg_write(&registers.osd, OSD_OFFSET);
        batch.reg_offsets(&registers.offsets);
        batch.reg_read(&mut hw_status, HW_STATUS_OFFSET);
        batch.reg_read(&mut status, STATUS_OFFSET);
        self.svc
            .submit_batch(&mut batch)
            .map_err(|e| format!("H.264 register submit: {e}"))?;
        self.svc
            .poll_hw_finish()
            .map_err(|e| format!("H.264 poll: {e}"))?;

        let errors = hw_status[0] & HW_ERROR_MASK;
        if errors != 0 {
            return Err(format!(
                "VEPU580 H.264 hardware error mask 0x{errors:03x} (int_sta=0x{:08x})",
                hw_status[0]
            ));
        }
        if hw_status[0] & 1 == 0 {
            return Err(format!(
                "VEPU580 H.264 completed ioctl without enc_done (int_sta=0x{:08x})",
                hw_status[0]
            ));
        }

        let payload_len = status[0] as usize;
        if payload_len < 8 || payload_len > self.output.len() {
            return Err(format!(
                "invalid VEPU580 H.264 length {payload_len} (capacity {})",
                self.output.len()
            ));
        }
        self.output
            .sync_read_begin()
            .map_err(|e| format!("H.264 output invalidate: {e}"))?;
        let payload = self.output.as_slice()[..payload_len].to_vec();
        let nal_validation = validate_nal_bytes(&payload, if state.is_idr { 5 } else { 1 });
        self.output
            .sync_read_end()
            .map_err(|e| format!("H.264 output sync end: {e}"))?;
        nal_validation?;

        // SPS and PPS precede an IDR only; a P picture rides on the parameters it
        // already established, which is what `mpi_enc_test` streams too.
        let headers = if state.is_idr {
            self.headers()?
        } else {
            Vec::new()
        };
        let mut annex_b = Vec::with_capacity(headers.len() + payload.len());
        annex_b.extend_from_slice(&headers);
        annex_b.extend_from_slice(&payload);
        let statistics = Statistics::from_words(&status);
        dump_statistics(&job.state, &status);
        if let (Some(model), Some(rc)) = (self.rc.as_mut(), job.rc) {
            let frame = if state.is_idr {
                rc_model::FrameType::Intra
            } else {
                rc_model::FrameType::InterP
            };
            // Everything the model weighs comes out of the same STATUS window, with
            // the same divisors the HAL uses before handing it to the reference's
            // rate control: the coded size (`bs_lgth_l32` * 8), the mean QP over
            // 16x16 blocks, and the two block-difference averages.
            let mbs = ((self.config.coded_width() / 16) * (self.config.coded_height() / 16)) as i32;
            let measured = rc_model::Measured {
                bits: statistics.bitstream_bytes as i32 * 8,
                quality: statistics.qp_sum as i32 / mbs,
                madi: if statistics.num_b16 > 0 {
                    (statistics.madi / statistics.num_b16) as i32
                } else {
                    0
                },
                madp: if statistics.num_ctu > 0 {
                    (statistics.madp / statistics.num_ctu) as i32
                } else {
                    0
                },
            };
            model.end_with(frame, rc.bit_target, measured);
        }
        Ok(EncodedFrame {
            annex_b,
            payload,
            headers,
            is_idr: state.is_idr,
            frame_num: state.frame_num,
            poc: state.poc,
            statistics,
        })
    }

    /// Encode a single IDR and return it with its parameter sets.
    pub fn encode_idr(&mut self, nv12: &[u8]) -> Result<EncodedIdr, String> {
        let frame = self.encode_frame(nv12)?;
        Ok(EncodedIdr {
            annex_b: frame.annex_b,
            payload: frame.payload,
            headers: frame.headers,
        })
    }
}

/// Write one job's register image to `RKMPP_REG_DUMP` in the same text layout
/// `tools/mpp_ioctl_trace.c` prints for the C encoder, so
/// `tools/vepu580_trace_diff.py` compares a Rust job against a captured C job
/// word for word instead of against the recorded template. One line per section,
/// no addresses hidden: two runs of the same session differ in dma-buf fds, and
/// the diff reports those as what they are.
fn dump_job_registers(job: &Job<'_>, file: &RegisterFile) {
    let Some(path) = std::env::var_os("RKMPP_REG_DUMP") else {
        return;
    };
    let mut text = String::new();
    for (name, offset, words) in [
        ("ctl", CTL_OFFSET, file.ctl.as_slice()),
        ("base", BASE_OFFSET, file.base.as_slice()),
        ("rc_klut", RCKLUT_OFFSET, file.rc_klut.as_slice()),
        ("param", PARAM_OFFSET, file.param.as_slice()),
        ("rdo", RDO_OFFSET, file.rdo.as_slice()),
        ("scl", SCL_OFFSET, file.scl.as_slice()),
        ("osd", OSD_OFFSET, file.osd.as_slice()),
    ] {
        let _ = name;
        text.push_str(&format!(
            "mpp_trace before cmd=200 flag=00 size={} offset={} job={}\n",
            words.len() * 4,
            offset,
            job.state.index
        ));
        text.push_str("mpp_trace write words=");
        for word in words {
            text.push_str(&format!(" {word:08x}"));
        }
        text.push('\n');
    }
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(text.as_bytes());
    }
}

/// `atr_wgt` of `hal_h264e_vepu580_tune.c`: the inter ATR weights per
/// `scene_motion_flag` row. Rows 0 and 2 are equal, which is why the same fixture
/// cannot distinguish the two scene modes.
const ATR_WEIGHTS: [[u32; 9]; 4] = [
    [22, 19, 16, 22, 19, 18, 22, 19, 16],
    [19, 19, 19, 19, 19, 19, 19, 19, 19],
    [22, 19, 16, 22, 19, 18, 22, 19, 16],
    [20, 20, 20, 20, 20, 20, 20, 20, 20],
];

/// `cime_multi` and `rime_multi` of the same file.
const CIME_MULTI: [[u32; 4]; 4] = [[4, 8, 24, 24], [4, 7, 20, 20], [4, 7, 20, 20], [4, 4, 4, 4]];
const RIME_MULTI: [[u32; 3]; 4] = [[4, 32, 128], [4, 16, 64], [4, 16, 64], [4, 4, 4]];

/// `scene_motion_flag` for a session that has not detected a scene change:
/// `ap_motion_flag * 2 + curr_scene_motion_flag`, where `ap_motion_flag` is the
/// scene mode (1 for the default, 0 for IPC) and the current-motion flag is the
/// detector's output. The detector - `vepu580_h264e_tune_stat_update`, which
/// reads the block statistics out of the STATUS window - is not ported yet, so
/// this is the flag's initial value, 0, and the tables are the no-change rows.
pub const SCENE_MOTION_FLAG_DEFAULT: usize = 2;

/// The STATUS window (`Vepu580Status_t`), read back after every job.
///
/// Field positions are the absolute register words of `Vepu580Status_t` minus
/// reg4096, which is where the window starts (`VEPU580_STATUS_OFFSET`). They are
/// not taken on trust: the ioctl tracer records the C encoder's own read-back of
/// this window, and `scripts/board_h264_status_vs_mpp.sh` diffs the two, so a
/// wrong offset fails a comparison rather than feeding a silent zero to the
/// motion and rate-control logic downstream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Statistics {
    /// `bs_lgth_l32`: bytes the hardware wrote for this picture.
    pub bitstream_bytes: u32,
    /// `st_bnum_b16.num_b16`, reg4171 - the 16x16 block count the statistics below
    /// are averages over.
    pub num_b16: u32,
    /// `madi`, reg4170 - mean absolute difference of the intra prediction.
    pub madi: u32,
    /// `madp`, reg4168 - mean absolute difference of the inter prediction.
    pub madp: u32,
    /// `st_bnum_cme.num_ctu`, reg4169 - the CME block count `madp` averages over.
    /// Zero on t6's block, which is why the motion level never leaves "still".
    pub num_ctu: u32,
    /// `md_sad_b16num0..3`, reg4173..4176.
    pub md_sad_b16num: [u32; 4],
    /// `madi_b16num0..3`, reg4177..4180.
    pub madi_b16num: [u32; 4],
    /// `qp_sum`, reg4099 - the QP sum rate control divides by the CTU count.
    pub qp_sum: u32,
}

impl Statistics {
    /// Interpret a status readback whose first word is reg4096.
    pub fn from_words(words: &[u32]) -> Self {
        let at = |word: usize| -> u32 {
            word.checked_sub(4096)
                .and_then(|i| words.get(i).copied())
                .unwrap_or(0)
        };
        Self {
            bitstream_bytes: at(4096),
            num_b16: at(4171) & 0x7f_ffff,
            madi: at(4170),
            madp: at(4168),
            num_ctu: at(4169) & 0x1f_ffff,
            md_sad_b16num: [at(4173), at(4174), at(4175), at(4176)],
            madi_b16num: [at(4177), at(4178), at(4179), at(4180)],
            qp_sum: at(4099),
        }
    }

    /// The window as the trace format prints it, for `scripts/` to diff against
    /// the C encoder's read-back.
    pub fn to_trace_words(&self, words: &[u32]) -> String {
        let mut out = String::new();
        for word in words {
            out.push_str(&format!("{word:08x} "));
        }
        let _ = self;
        out
    }
}

/// Write our STATUS readback in the same text shape the ioctl tracer prints for
/// the C encoder's, so the two can be diffed word by word.
fn dump_statistics(state: &FrameState, status: &[u32]) {
    let Some(path) = std::env::var_os("RKMPP_STAT_DUMP") else {
        return;
    };
    let mut text = format!(
        "mpp_trace read_complete[1]@{} job={} words={}",
        STATUS_OFFSET,
        state.index,
        status.len()
    );
    for word in status {
        text.push_str(&format!(" {word:08x}"));
    }
    text.push('\n');
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(text.as_bytes());
    }
}

/// The chroma KLUT weights of `hal_h264e_vepu580.c`. An I picture loads the
/// table from word 0 and a P picture from word 4, 24 words each.
const CHROMA_KLUT_WEIGHT: [u32; 30] = [
    0x0a00_0010,
    0x0006_4000,
    0x1400_0020,
    0x000c_8000,
    0x2800_0040,
    0x0019_4000,
    0x5080_0080,
    0x0032_c000,
    0xa100_0100,
    0x0065_8000,
    0x4280_0200,
    0x00cb_0001,
    0x8500_0400,
    0x0196_4002,
    0x0a00_0800,
    0x032c_8005,
    0x1400_1000,
    0x0659_400a,
    0x2880_2000,
    0x0cb2_c014,
    0x5100_4000,
    0x1965_c028,
    0xa280_8000,
    0x32cb_c050,
    0x4500_ffff,
    0x6597_80a1,
    0x8a81_fffe,
    0xcc00_0142,
    0xff83_ffff,
    0x0000_01ff,
];

/// `h264_aq_tthd_default`, the same for both slice types.
const AQ_THRESHOLD_DEFAULT: [u8; 16] = [0, 0, 0, 0, 3, 3, 5, 5, 8, 8, 8, 15, 15, 20, 25, 35];

/// `h264_I_aq_step_default` and `h264_P_aq_step_default`, which differ in one
/// entry - the six against the seven at index 14. That single byte is what the
/// C encoder's P job changes in `aq_step`, and the only rate-control table the
/// fixed-QP path still cares about.
const AQ_STEP_I: [i8; 16] = [-8, -7, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 8];
const AQ_STEP_P: [i8; 16] = [-8, -7, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 7, 8];

/// Words of the chroma KLUT copied per picture (`CHROMA_KLUT_TAB_SIZE`).
const CHROMA_KLUT_WORDS: usize = 24;

pub fn build_registers(job: &Job<'_>) -> Result<RegisterFile, String> {
    let config = job.config;
    let input_fd = job.input_fd;
    let output_fd = job.output_fd;
    let output_capacity = job.output_capacity;
    validate_reference_config(config)?;
    let reconstruction_fd = job.current_fd;
    let thumb_fd = job.current_thumb_fd;
    let reference_fd = job.reference_fd;
    let reference_thumb_fd = job.reference_thumb_fd;
    if input_fd < 0
        || output_fd < 0
        || reconstruction_fd < 0
        || thumb_fd < 0
        || reference_fd < 0
        || reference_thumb_fd < 0
    {
        return Err("H.264 register file received an invalid dma-buf fd".into());
    }
    let expected_min_output = input_size(config)? + REFERENCE_OUTPUT_MARGIN;
    if output_capacity < expected_min_output {
        return Err(format!(
            "H.264 output capacity {output_capacity} is below {expected_min_output}"
        ));
    }

    let mut ctl = template::CTL_WORDS;
    let mut base = template::BASE_WORDS;
    let rc_klut = template::RCKLUT_WORDS;
    let param = template::PARAM_WORDS;
    let rdo = template::RDO_WORDS;
    let scl = template::SCL_WORDS;
    let osd = template::OSD_WORDS;

    clear_dynamic_addresses(&mut ctl, &mut base);
    let input = input_fd as u32;
    let output = output_fd as u32;
    let recon = reconstruction_fd as u32;
    let thumb = thumb_fd as u32;
    base[BASE_ADR_SRC0] = input;
    base[BASE_ADR_SRC1] = input;
    base[BASE_ADR_SRC2] = input;
    base[BASE_RFPW_H] = recon;
    base[BASE_RFPW_B] = recon;
    base[BASE_DSPW] = thumb;
    // The reference surfaces are the *previous* slot, which for an IDR is this
    // one - matching `setup_vepu580_recn_refr`, where an IDR leaves `rfpr_*` on
    // whatever the buffer setup gave the first slot.
    let reference = reference_fd as u32;
    base[BASE_RFPR_H] = reference;
    base[BASE_RFPR_B] = reference;
    base[BASE_DSPR] = reference_thumb_fd as u32;
    base[BASE_BSBT] = output;
    base[BASE_BSBB] = output;
    base[BASE_BSBR] = output;
    base[BASE_ADR_BSBS] = output;
    base[BASE_MEIW] = job.md_fd as u32;
    patch_reference_layout(&mut base, config)?;

    let luma_bytes = config.coded_width() * config.coded_height();
    let offsets = vec![
        RegOffsetInfo {
            reg_idx: REG_SRC_1_OFFSET,
            offset: luma_bytes as u32,
        },
        RegOffsetInfo {
            reg_idx: REG_SRC_2_OFFSET,
            offset: luma_bytes as u32,
        },
        RegOffsetInfo {
            reg_idx: REG_BSBT_END,
            // The installed t6 MPP traces the logical packet size here
            // (119296 for the 320x240 reference), not dma-heap's rounded size.
            offset: output_capacity as u32,
        },
        RegOffsetInfo {
            reg_idx: REG_BSBS_START,
            offset: 0,
        },
        RegOffsetInfo {
            reg_idx: REG_RFPW_B,
            offset: FBC_HEADER_SIZE as u32,
        },
        RegOffsetInfo {
            reg_idx: REG_RFPR_B,
            offset: FBC_HEADER_SIZE as u32,
        },
    ];

    let mut file = RegisterFile {
        ctl,
        base,
        rc_klut,
        param,
        rdo,
        scl,
        osd,
        offsets,
    };
    // The first piece of the register file that stops being a recording: the QP
    // words of `setup_vepu580_rc_base`. Everything else is still the captured
    // image, and `validate_reference_config` refuses what it depends on.
    match job.rc {
        Some(rc) => file.apply_rate_control(config, job.state.is_idr, &rc)?,
        None => file.apply_fixed_qp(u32::from(config.qp_init))?,
    }
    file.apply_profile(config)?;
    file.apply_picture(job)?;
    // The reference job was captured with a harness that asked for motion
    // derivation info, so the recorded image has `enc_pic.mei_stor` set. Writing
    // side information needs the MD buffer `setup_hal_bufs` allocates for it,
    // which this driver does not implement; leaving the enable on without that
    // buffer would have the hardware write to a surface nobody owns. That is not a
    // guess: with the enable set and `meiw_addr` at zero the encoder returns
    // `int_sta = 0x100` instead of encoding.
    // The reference harness passes a motion info buffer for every picture, so
    // `setup_vepu580_start` leaves `mei_stor` set for I and P alike. Matching
    // it is what `md_info` is for: with the enable set and `meiw_addr` at zero
    // the encoder answers `int_sta = 0x100` instead of encoding - which is what
    // the attempt to clear this bit turned out to be measuring.
    file.set(&vepu580_regs::BASE_ENC_PIC_MEI_STOR, 1)?;
    file.apply_tune(job)?;
    Ok(file)
}

impl RegisterFile {
    /// Write a field that lives in the section-3 tuning window.
    fn set_param(&mut self, field: &vepu580_regs::Field, value: u32) -> Result<(), String> {
        let index = field
            .word
            .checked_sub(1472)
            .ok_or_else(|| format!("{} is before section 3", field.name))?;
        let word = self
            .param
            .get_mut(index)
            .ok_or_else(|| format!("{} is past the captured section 3", field.name))?;
        let mask = (1u32 << field.width) - 1;
        if value & !mask != 0 {
            return Err(format!(
                "{} takes {} bits, {value} does not fit",
                field.name, field.width
            ));
        }
        *word = (*word & !(mask << field.shift)) | (value << field.shift);
        Ok(())
    }

    /// Mutable word of an absolute register index.
    fn rc_klut_word_mut(&mut self, word: usize) -> Result<&mut u32, String> {
        let index = word
            .checked_sub(1024)
            .ok_or_else(|| format!("word {word} is before the RC lookup table"))?;
        self.rc_klut
            .get_mut(index)
            .ok_or_else(|| format!("word {word} is past the RC lookup table"))
    }

    /// Write a byte table such as `aq_step` the way the reference does, through a
    /// byte pointer into the register array.
    fn put_bytes(&mut self, array: &vepu580_regs::Array, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() != array.count * array.elem_bytes {
            return Err(format!(
                "{} takes {} bytes, got {}",
                array.name,
                array.count * array.elem_bytes,
                bytes.len()
            ));
        }
        for (index, byte) in bytes.iter().enumerate() {
            let word = self.rc_klut_word_mut(array.word + index / 4)?;
            let shift = (index % 4) * 8;
            *word = (*word & !(0xffu32 << shift)) | (u32::from(*byte) << shift);
        }
        Ok(())
    }

    /// `vepu580_h264e_tune_reg_patch`, the pass that runs last and re-writes the
    /// inter ATR weights and the search multipliers from the scene tables. The
    /// current-motion branch is unreachable while the detector is missing, so it
    /// is not silently dropped - it is here, keyed on a flag that stays 0.
    fn apply_tune(&mut self, job: &Job<'_>) -> Result<(), String> {
        if !job.state.is_idr {
            self.set_atr_weights(&ATR_WEIGHTS[SCENE_MOTION_FLAG_DEFAULT])?;
        }
        // The search multipliers the reference re-writes in the same pass
        // (`cime_multi`, `rime_multi`) are deliberately not reproduced field by
        // field: that part of section 3 moved between the 2024-04 build this crate
        // is verified against and current master, and the captured image already
        // ends up carrying the rows the reference writes there, so those words
        // match today. Rewriting them from a guessed layout would trade a real
        // match for a cosmetic one in the port's shape.
        Ok(())
    }
    /// The nine Lvl16 / Lvl8 / Lvl4 ATR weight fields, in declaration order.
    fn set_atr_weights(&mut self, weights: &[u32; 9]) -> Result<(), String> {
        let fields = [
            &vepu580_regs::S3_LVL16_ATR_WGT_LVL16_ATR_WGT0,
            &vepu580_regs::S3_LVL16_ATR_WGT_LVL16_ATR_WGT1,
            &vepu580_regs::S3_LVL16_ATR_WGT_LVL16_ATR_WGT2,
            &vepu580_regs::S3_LVL8_ATR_WGT_LVL8_ATR_WGT0,
            &vepu580_regs::S3_LVL8_ATR_WGT_LVL8_ATR_WGT1,
            &vepu580_regs::S3_LVL8_ATR_WGT_LVL8_ATR_WGT2,
            &vepu580_regs::S3_LVL4_ATR_WGT_LVL4_ATR_WGT0,
            &vepu580_regs::S3_LVL4_ATR_WGT_LVL4_ATR_WGT1,
            &vepu580_regs::S3_LVL4_ATR_WGT_LVL4_ATR_WGT2,
        ];
        for (field, value) in fields.iter().zip(weights) {
            self.set_param(field, *value)?;
        }
        Ok(())
    }

    /// The fields of `setup_vepu580_codec` and `setup_vepu580_me` that say which
    /// picture this is, rather than what the sequence looks like.
    ///
    /// `sli_type` is 2 for an I slice and 0 for a P slice in the hardware's own
    /// encoding (`slice_type == H264_I_SLICE ? 2 : 0`). `idr_pid` is only written
    /// by an IDR slice; the reference passes -1 for a P slice, which the field
    /// width turns into all ones. `pmv_mdst_*` is the motion-vector co-located
    /// distance: zero without a reference, five for a P picture.
    fn apply_picture(&mut self, job: &Job<'_>) -> Result<(), String> {
        let config = job.config;
        let state = job.state;
        let poc_lsb_bits = job.config.log2_max_poc_lsb_minus4 + 4;
        self.set(
            &vepu580_regs::H264_SYNT_SLI0_SLI_TYPE,
            if state.is_idr { 2 } else { 0 },
        )?;
        self.set(&vepu580_regs::H264_SYNT_SLI0_FRM_NUM, state.frame_num)?;
        self.set(
            &vepu580_regs::H264_SYNT_SLI1_IDR_PID,
            if state.is_idr {
                state.idr_pic_id
            } else {
                // all ones in the field's own width
                (1 << 16) - 1
            },
        )?;
        self.set(
            &vepu580_regs::H264_SYNT_SLI1_POC_LSB,
            (state.poc as u32) & ((1 << poc_lsb_bits) - 1),
        )?;
        // `setup_vepu580_codec` takes the NAL header from the slice: an I slice
        // is type 5 with nal_ref_idc 3, a P slice is type 1 with 2. Without this
        // the hardware stamps every P picture as an IDR.
        self.set(
            &vepu580_regs::H264_SYNT_NAL_NAL_REF_IDC,
            if state.is_idr { 3 } else { 2 },
        )?;
        self.set(
            &vepu580_regs::H264_SYNT_NAL_NAL_UNIT_TYPE,
            if state.is_idr { 5 } else { 1 },
        )?;

        // `setup_vepu580_rdo_pred` swaps the chroma KLUT between two thirds of the
        // same table depending on the slice type, and moves the offset with it.
        self.set(
            &vepu580_regs::RCKLUT_KLUT_OFST_CHRM_KLUT_OFST,
            if state.is_idr { 0 } else { 3 },
        )?;
        let klut_offset = if state.is_idr { 0 } else { 4 };
        let first = vepu580_regs::RCKLUT_KLUT_WGT0_CHRM_KLUT_WGT0.word;
        for index in 0..CHROMA_KLUT_WORDS {
            let word = self.rc_klut_word_mut(first + index)?;
            *word = CHROMA_KLUT_WEIGHT[klut_offset + index];
        }

        // The adaptive-quantisation tables: thresholds are common, the step table
        // is per slice type, both written through a byte pointer as the reference
        // does.
        self.put_bytes(&vepu580_regs::RCKLUT_AQ_TTHD, &AQ_THRESHOLD_DEFAULT)?;
        let step = if state.is_idr { &AQ_STEP_I } else { &AQ_STEP_P };
        let step: Vec<u8> = step.iter().map(|v| (*v as u8) & 0x3f).collect();
        self.put_bytes(&vepu580_regs::RCKLUT_AQ_STEP, &step)?;

        let mdst = if state.is_idr { 0 } else { 5 };
        self.set(&vepu580_regs::H264_ME_CFG_PMV_MDST_H, mdst)?;
        self.set(&vepu580_regs::H264_ME_CFG_PMV_MDST_V, mdst)?;

        // The quantisation bias and the ATR thresholds follow the slice type:
        // `setup_vepu580_rdo_pred` writes one set for an I slice and another for
        // inter, in the same pass. Taking them from the recorded image instead
        // gives every P picture the I picture's thresholds, which is how this
        // driver's P NAL came out five bytes long.
        let quant_bias_i = if state.is_idr { 683 } else { 583 };
        self.set_param(&vepu580_regs::S3_RDO_QUANT_QUANT_F_BIAS_I, quant_bias_i)?;
        self.set_param(&vepu580_regs::S3_RDO_QUANT_QUANT_F_BIAS_P, 171)?;
        let (thd0_0, thd0_1, thd1_2) = if state.is_idr {
            (1, 4, 36)
        } else {
            (4, 16, 81)
        };
        self.set_param(&vepu580_regs::S3_ATR_THD0_ATR_THD0, thd0_0)?;
        self.set_param(&vepu580_regs::S3_ATR_THD0_ATR_THD1, thd0_1)?;
        self.set_param(&vepu580_regs::S3_ATR_THD1_ATR_THD2, thd1_2)?;
        self.set_param(&vepu580_regs::S3_ATR_THD1_ATR_THDQP, 45)?;
        let weights: [u32; 9] = if state.is_idr {
            [16, 16, 16, 22, 21, 20, 20, 18, 16]
        } else {
            [25, 20, 16, 25, 20, 18, 25, 20, 16]
        };
        self.set_atr_weights(&weights)?;
        // `setup_vepu580_me` bounds the motion-vector range by the level.
        self.set(
            &vepu580_regs::H264_ME_CFG_MV_LIMIT,
            if config.level_idc > 20 {
                2
            } else if config.level_idc >= 11 {
                1
            } else {
                0
            },
        )?;
        Ok(())
    }
}

fn clear_dynamic_addresses(ctl: &mut [u32; 24], base: &mut [u32; 94]) {
    // The status readback lands in CTL[11]. Clear the slot before submission.
    ctl[11] = 0;
    for index in [
        BASE_ADR_SRC0,
        BASE_ADR_SRC1,
        BASE_ADR_SRC2,
        BASE_RFPW_H,
        BASE_RFPW_B,
        BASE_RFPR_H,
        BASE_RFPR_B,
        BASE_DSPW,
        BASE_DSPR,
        BASE_BSBT,
        BASE_BSBB,
        BASE_BSBR,
        BASE_ADR_BSBS,
        BASE_ADR_MEI,
    ] {
        base[index] = 0;
    }
}

fn patch_reference_layout(base: &mut [u32; 94], config: &EncoderConfig) -> Result<(), String> {
    let coded_width = config.coded_width();
    let coded_height = config.coded_height();
    let width_mbs = coded_width / 16;
    let height_mbs = coded_height / 16;

    let width_field = field(coded_width / 8 - 1, 11, "H.264 encoded width")?;
    let height_field = field(coded_height / 8 - 1, 11, "H.264 encoded height")?;
    base[BASE_ENC_RSL] = width_field | (height_field << 16);

    let width_fill = field(coded_width - config.width, 6, "H.264 source width fill")?;
    let height_fill = field(coded_height - config.height, 6, "H.264 source height fill")?;
    base[BASE_SRC_FILL] = width_fill | (height_fill << 16);

    let stride = field(coded_width, 17, "H.264 source stride")?;
    base[BASE_SRC_STRD0] = stride;
    base[BASE_SRC_STRD1] = stride;

    // `sw212 rc_cfg` is left alone deliberately. Both the 2024-04 vintage and
    // master return from `setup_vepu580_rc_base` as soon as they have written the
    // fixed-QP QPs, so `rc_en`, `aq_en` and `rc_ctu_num` stay zero in that mode;
    // `apply_fixed_qp` writes the three QP fields that do change. Setting them, as
    // this file used to, runs adaptive quantisation on a picture the reference
    // encodes without it - 861 bytes against the reference's 861-byte... no: see
    // M7 in ROADMAP.md, where the byte counts are recorded from the board.

    base[BASE_ME_RNGE] = build_me_range(width_mbs, height_mbs)?;
    base[BASE_ME_CACH] = build_me_cache(coded_width, base[BASE_ME_RNGE])?;
    Ok(())
}

fn field(value: usize, bits: u32, name: &str) -> Result<u32, String> {
    let max = (1usize << bits) - 1;
    if value > max {
        return Err(format!("{name} {value} does not fit in {bits} bits"));
    }
    Ok(value as u32)
}

fn build_me_range(width_mbs: usize, height_mbs: usize) -> Result<u32, String> {
    let mut search_blocks_w = 44usize;
    let mut search_blocks_h = 28usize;

    if search_blocks_w / 4 * 2 > (width_mbs * 2 + 1) / 2 {
        search_blocks_w = (width_mbs * 2 + 1) / 2 / 2 * 4;
    }
    let aligned_coded_height = align(height_mbs * 16, 64);
    if search_blocks_h / 4 > aligned_coded_height / 128 * 4 {
        search_blocks_h = aligned_coded_height / 128 * 16;
    }

    let search_w = field(search_blocks_w / 4, 4, "H.264 CIME width")?;
    let search_h = field(search_blocks_h / 4, 4, "H.264 CIME height")?;
    Ok(search_w | (search_h << 4) | (7 << 8) | (5 << 11))
}

fn build_me_cache(coded_width: usize, me_range: u32) -> Result<u32, String> {
    let search_w = ((me_range & 0xf) * 4) as usize;
    let search_h = (((me_range >> 4) & 0xf) * 4) as usize;
    let picture_width_64 = align(coded_width, 64) / 64;
    let line_buffer_w = picture_width_64;

    let search_16_w = if search_w % 16 == 0 {
        (search_w * 2 + 15) / 16 + 1
    } else {
        (31 - search_w % 16 + search_w * 2) / 16 + 1
    }
    .min(line_buffer_w);
    let search_4_h = (search_h * 2 + 3) / 4 + 1;
    let search_max = search_4_h;

    let cache_limit = 2464usize;
    let mut cache_h = 1usize;
    let mut ramb_h = search_4_h;
    while cache_h < search_max
        && cache_limit > ((cache_h - 1) * line_buffer_w * 4 + ramb_h * 4 * search_16_w)
    {
        cache_h += 1;
        if ramb_h > 2 {
            ramb_h -= 1;
        } else {
            ramb_h = 1;
        }
    }
    if search_4_h == 1 {
        cache_h += 1;
        ramb_h = 0;
    }
    if cache_limit < ((cache_h - 1) * line_buffer_w * 4 + ramb_h * 4 * search_16_w) {
        cache_h -= 1;
    }

    let rama_max = (cache_h - 1) * line_buffer_w + search_16_w;
    let rama_max = field(rama_max, 11, "H.264 CIME RAM width")?;
    let cache_height = field(cache_h, 5, "H.264 CIME RAM height")?;
    let line_buffer_w = field(line_buffer_w, 9, "H.264 CIME line buffer")?;
    Ok(rama_max | (cache_height << 11) | (line_buffer_w << 18))
}

/// The configurations with a byte-exact reference on the board.
///
/// `scripts/board_h264_qp_vs_mpp.sh` compares our SPS, PPS and IDR against the C
/// encoder, NAL by NAL, at QP 10, 18, 26, 32, 42 and 51 on the reference size -
/// both ends of the range and four points inside it - and every one of them came
/// out identical. QP reaches exactly three register fields, so the whole
/// 0..=51 range is generated from the same code those six runs covered. The
/// recorded-image refusals above still stand: profile, level, GOP and entropy
/// coding are properties of the image, not parameters yet.
fn verified_on_board(config: &EncoderConfig) -> Result<(), String> {
    if config.qp_init > 51 {
        return Err(format!("H.264 QP {} is outside 0..=51", config.qp_init));
    }
    // GOP > 1 is a verified configuration: a 1-IDR-plus-5-P encode is byte-identical
    // to the C encoder on the board, on both a still and a moving fixture
    // (`scripts/board_h264_qp_vs_mpp.sh`). What is *not* ported is the scene-change
    // detector `vepu580_h264e_tune_stat_update`, which reads the block statistics
    // out of the STATUS window and, once three consecutive pictures flag large
    // motion, moves the ATR weights and the quant bias to another row of the same
    // tables. `scene_motion_flag` is therefore pinned at its initial value; on a
    // sequence that trips the detector the two encoders would part there.
    Ok(())
}

/// `rc_model_v2.c`'s configuration, from ours plus `h264e_api_v2.c`'s defaults.
fn rc_model_config(config: &EncoderConfig) -> Result<rc_model::Config, String> {
    if config.gop < 2 {
        return Err("CBR needs a GOP longer than one picture".into());
    }
    let (Ok(target), Ok(min), Ok(max)) = (
        i32::try_from(config.bps_target),
        i32::try_from(config.bps_min),
        i32::try_from(config.bps_max),
    ) else {
        return Err("CBR bit rates do not fit a signed 32-bit field".into());
    };
    if target <= 0 {
        return Err("CBR needs a positive bps target".into());
    }
    Ok(rc_model::Config {
        igop: config.gop as i32,
        fps_num: config.fps_num as i32,
        fps_den: config.fps_den as i32,
        bps_target: target,
        bps_min: min,
        bps_max: max,
        init_ip_ratio: 160,
        min_i_bit_prop: 10,
        max_i_bit_prop: 30,
        stats_time: 3,
        // A caller that never set an initial quality says so with
        // `quality_auto`, and `rc_model_v2_hal_start` derives the first QP from
        // the bit budget; anything at or below zero reaches the same branch.
        quality_init: if config.quality_auto {
            -1
        } else {
            i32::from(config.qp_init)
        },
        quality_min: 8,
        quality_max: 48,
        i_quality_delta: 2,
        total_mb: (config.coded_width() / 16) as i32 * (config.coded_height() / 16) as i32,
        mode: match config.rc_mode {
            RcMode::Vbr => rc_model::Mode::Vbr,
            RcMode::Avbr => rc_model::Mode::Avbr,
            _ => rc_model::Mode::Cbr,
        },
    })
}

fn validate_reference_config(config: &EncoderConfig) -> Result<(), String> {
    config.validate().map_err(|e| e.to_string())?;
    if config.width < 16 || config.height < 16 || config.width % 2 != 0 || config.height % 2 != 0 {
        return Err(format!(
            "VEPU580 H.264 requires even dimensions of at least 16x16, got {}x{}",
            config.width, config.height
        ));
    }
    if config.coded_width() > 4096 {
        return Err(
            "VEPU580 bootstrap does not yet allocate the >4096-pixel CIME extension lines".into(),
        );
    }
    if config.qp_init > 51 {
        return Err(format!("H.264 QP {} is outside 0..=51", config.qp_init));
    }
    if config.level_idc != 30 {
        return Err(
            "the VEPU580 register image is recorded at level 3.0; the fields the level does \
             reach (rect_size, mv_limit) are derived, but the level has not been compared \
             against the C encoder at any other value"
                .into(),
        );
    }
    Ok(())
}

fn reconstruction_sizes(config: &EncoderConfig) -> Result<(usize, usize), String> {
    let aligned_w = align(config.width, 64);
    let aligned_h = align(config.height, 16) + 16;
    let header = align(
        aligned_w
            .checked_mul(aligned_h)
            .ok_or_else(|| "H.264 FBC header overflow".to_string())?
            / 64,
        8192,
    );
    let body = aligned_w
        .checked_mul(aligned_h)
        .and_then(|v| v.checked_mul(3))
        .map(|v| v / 2)
        .ok_or_else(|| "H.264 FBC body overflow".to_string())?;
    let thumb = align(aligned_w / 64 * aligned_h / 64 * 256, 8192);
    Ok((header + body, thumb))
}

/// The motion derivation info buffer, `mdinfo_size` of the C test path: one
/// 16-byte record per 64x16 macroblock block of the coded size. The reference
/// harness passes this for every picture, and `setup_vepu580_recn_refr`'s
/// sibling sets `mei_stor` from its presence - with the enable set and no
/// surface behind it the encoder returns `int_sta = 0x100` instead of encoding.
fn mdinfo_size(config: &EncoderConfig) -> Result<usize, String> {
    let blocks_x = align(config.coded_width(), 64) / 64;
    let blocks_y = align(config.coded_height(), 16) / 16;
    blocks_x
        .checked_mul(blocks_y)
        .and_then(|v| v.checked_mul(16))
        .ok_or_else(|| "H.264 motion info size overflow".into())
}

pub fn input_size(config: &EncoderConfig) -> Result<usize, String> {
    config
        .coded_width()
        .checked_mul(config.coded_height())
        .and_then(|luma| luma.checked_mul(3).map(|bytes| bytes / 2))
        .ok_or_else(|| "H.264 input size overflow".into())
}

fn logical_output_size(config: &EncoderConfig) -> Result<usize, String> {
    input_size(config)?
        .checked_add(REFERENCE_OUTPUT_MARGIN)
        .ok_or_else(|| "H.264 output size overflow".into())
}

/// The hardware writes the slice's own NAL header, so the payload has to open
/// with an IDR for an IDR picture and a non-IDR slice otherwise. Checking for an
/// IDR always would reject every P picture the encoder produces.
fn validate_nal_bytes(data: &[u8], expected_type: u8) -> Result<(), String> {
    let valid = data
        .windows(4)
        .position(|window| window == [0, 0, 0, 1])
        .map(|offset| {
            data.get(offset + 4)
                .is_some_and(|header| header & 0x1f == expected_type)
        })
        .unwrap_or(false);
    if !valid {
        return Err(format!(
            "VEPU580 output does not open with an Annex-B NAL of type {expected_type}"
        ));
    }
    Ok(())
}

fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

/// The exact NV12 frame used by `tools/h264_enc_probe.c`.
pub fn probe_nv12(width: usize, height: usize) -> Vec<u8> {
    let stride = align(width, 16);
    let ver_stride = align(height, 16);
    let mut frame = vec![16; stride * ver_stride * 3 / 2];
    for y in 0..height {
        for x in 0..width {
            frame[y * stride + x] =
                (16 + (x * 200 / (width - 1) + y * 200 / (height - 1)) / 2) as u8;
        }
    }
    let uv = stride * ver_stride;
    frame[uv..uv + stride * ver_stride / 2].fill(128);
    for y in 0..height / 2 {
        for x in 0..width / 2 {
            let offset = uv + y * stride + x * 2;
            frame[offset] = (48 + x * 128 / (width / 2 - 1)) as u8;
            frame[offset + 1] = (208 - y * 128 / (height / 2 - 1)) as u8;
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EncoderConfig {
        EncoderConfig::new(REFERENCE_WIDTH, REFERENCE_HEIGHT)
    }

    /// A job on stand-in fds, which the register builder only copies around.
    fn job(config: &EncoderConfig, state: FrameState) -> Job<'_> {
        Job {
            config,
            input_fd: 7,
            output_fd: 9,
            current_fd: 11,
            current_thumb_fd: 13,
            reference_fd: 11,
            reference_thumb_fd: 13,
            md_fd: 15,
            rc: None,
            state,
            output_capacity: 119_296,
        }
    }

    fn registers_at(config: &EncoderConfig, state: FrameState) -> RegisterFile {
        build_registers(&job(config, state)).unwrap()
    }

    /// The same job at a chosen output capacity, for the capacity checks.
    fn job_with_output(config: &EncoderConfig, capacity: usize) -> Job<'_> {
        Job {
            output_capacity: capacity,
            ..job(config, FrameState::default())
        }
    }

    #[test]
    fn captured_job_has_the_c_struct_section_sizes() {
        assert_eq!(template::CTL_WORDS.len(), 24);
        assert_eq!(template::BASE_WORDS.len(), 94);
        assert_eq!(template::RCKLUT_WORDS.len(), 57);
        assert_eq!(template::PARAM_WORDS.len(), 180);
        assert_eq!(template::RDO_WORDS.len(), 122);
        assert_eq!(template::SCL_WORDS.len(), 679);
        assert_eq!(template::OSD_WORDS.len(), 288);
    }

    /// Spot values of the recorded image, which is now re-generated from a run of
    /// the 2024-04 MPP build on the board (`tools/patches/mpi_enc_test-
    /// constrained-baseline.patch` pins the harness to the same profile), so a
    /// future re-record that changes a control value shows up here.
    #[test]
    fn captured_job_keeps_the_reference_control_values() {
        assert_eq!(template::CTL_WORDS[4], 0x100);
        assert_eq!(template::CTL_WORDS[8], 0x1f7);
        assert_eq!(template::CTL_WORDS[21], 7);
        // enc_pic: cur_frm_ref and mei_stor set, QP 26.
        assert_eq!(template::BASE_WORDS[32], 0x1a1c);
        assert_eq!(template::BASE_WORDS[36], 0x1d0027);
        // Fixed QP leaves the rate-control lookup table alone.
        assert_eq!(template::RCKLUT_WORDS[0], 0);
        let registers = registers_at(&config(), FrameState::default());
        assert_eq!(
            registers.get(&vepu580_regs::BASE_ENC_PIC_MEI_STOR).unwrap(),
            1,
            "the enable is legal only because md_fd points at a real surface"
        );
        assert_eq!(registers.base[BASE_MEIW], 15, "meiw_addr carries the md fd");
    }

    #[test]
    fn dma_addresses_and_mpp_offsets_are_rebuilt() {
        let registers = registers_at(&config(), FrameState::default());
        assert_eq!(&registers.base[..3], &[7, 7, 7]);
        assert_eq!(&registers.base[3..7], &[11, 11, 11, 11]);
        assert_eq!(&registers.base[9..11], &[13, 13]);
        assert_eq!(&registers.base[12..16], &[9, 9, 9, 9]);
        assert_eq!(
            registers.offsets,
            [
                RegOffsetInfo {
                    reg_idx: 161,
                    offset: 76_800
                },
                RegOffsetInfo {
                    reg_idx: 162,
                    offset: 76_800
                },
                RegOffsetInfo {
                    reg_idx: 172,
                    offset: 0x1d200
                },
                RegOffsetInfo {
                    reg_idx: 175,
                    offset: 0
                },
                RegOffsetInfo {
                    reg_idx: 164,
                    offset: 8192
                },
                RegOffsetInfo {
                    reg_idx: 166,
                    offset: 8192
                },
            ]
        );
    }

    #[test]
    fn buffer_sizes_match_hal_h264e_vepu580_setup_hal_bufs() {
        assert_eq!(input_size(&config()).unwrap(), 115_200);
        assert_eq!(logical_output_size(&config()).unwrap(), 119_296);
        assert_eq!(reconstruction_sizes(&config()).unwrap(), (131_072, 8192));
    }

    #[test]
    fn dimension_fields_match_the_t6_mpp_traces() {
        for (width, height, expected) in [
            (64, 64, [0x07_0007, 64, 64, 0, 0x0000_2f02, 0x0004_1002]),
            (320, 240, [0x1d_0027, 320, 320, 0, 0x0000_2f7a, 0x0014_784b]),
            (640, 480, [0x3b_004f, 640, 640, 0, 0x0000_2f7b, 0x0028_7893]),
            (
                1280,
                720,
                [0x59_009f, 1280, 1280, 0, 0x0000_2f7b, 0x0050_791f],
            ),
        ] {
            let config = EncoderConfig::new(width, height);
            let output_size = logical_output_size(&config).unwrap();
            let registers = build_registers(&job_with_output(&config, output_size)).unwrap();
            assert_eq!(
                [
                    registers.base[BASE_ENC_RSL],
                    registers.base[BASE_SRC_STRD0],
                    registers.base[BASE_SRC_STRD1],
                    registers.base[BASE_RC_CFG],
                    registers.base[BASE_ME_RNGE],
                    registers.base[BASE_ME_CACH],
                ],
                expected,
                "{width}x{height}"
            );
            assert_eq!(registers.base[BASE_SRC_FILL], 0);
        }
    }

    #[test]
    fn unsupported_geometry_is_rejected_before_opening_hardware() {
        for config in [
            EncoderConfig::new(15, 16),
            EncoderConfig::new(16, 15),
            EncoderConfig::new(17, 16),
            EncoderConfig::new(16, 17),
            EncoderConfig::new(4098, 16),
        ] {
            assert!(build_registers(&job_with_output(
                &config,
                config.coded_width() * config.coded_height() * 2
            ))
            .is_err());
        }
    }

    /// The GOP walk the slice header has to report. PocType 20 without B frames
    /// is a recurrence of +2 per picture, reset at an IDR, and idr_pic_id
    /// alternates across IDR pictures - the same numbers `h264e_dpb` and
    /// `h264e_slice_init` produce, and the same ones the C encoder's second
    /// picture carried (`frame_num = 1`, `poc_lsb = 2`).
    #[test]
    fn frame_state_walks_the_gop() {
        let gop = 3;
        let mut state = FrameState::default();
        let mut seen = Vec::new();
        for _ in 0..7 {
            seen.push((state.is_idr, state.frame_num, state.poc, state.idr_pic_id));
            state = state.next(gop);
        }
        assert_eq!(
            seen,
            vec![
                (true, 0, 0, 0),  // IDR
                (false, 1, 2, 0), // P
                (false, 2, 4, 0), // P
                (true, 0, 0, 1),  // IDR, idr_pic_id alternates
                (false, 1, 2, 1),
                (false, 2, 4, 1),
                (true, 0, 0, 0),
            ]
        );
    }

    /// What a P picture may change, and nothing else. The set is the one the C
    /// encoder's own P job touches - slice header, NAL header, the co-located
    /// motion distance, the reference surfaces, and the two tables
    /// `setup_vepu580_rdo_pred` swaps by slice type - each verified by value below.
    #[test]
    fn p_picture_changes_only_the_fields_the_reference_does() {
        let mut config = config();
        config.gop = 4;
        let idr = registers_at(&config, FrameState::default());
        let p_state = FrameState {
            index: 1,
            is_idr: false,
            frame_num: 1,
            idr_pic_id: 0,
            poc: 2,
        };
        let p = build_registers(&Job {
            reference_fd: 21,
            reference_thumb_fd: 22,
            ..job(&config, p_state)
        })
        .unwrap();

        let image = |f: &RegisterFile| -> Vec<(usize, u32)> {
            let mut all = Vec::new();
            all.extend(f.ctl.iter().enumerate().map(|(i, w)| (i, *w)));
            all.extend(f.base.iter().enumerate().map(|(i, w)| (160 + i, *w)));
            all.extend(f.rc_klut.iter().enumerate().map(|(i, w)| (1024 + i, *w)));
            all
        };
        let allowed: Vec<usize> = (165..=170)
            .chain([221, 236, 239, 240])
            .chain(1045..=1048) // aq_step, four words
            .chain(1056..=1080) // klut_ofst and the 24 weight words
            .collect();
        let changed: Vec<usize> = image(&idr)
            .into_iter()
            .zip(image(&p))
            .filter(|((_, a), (_, b))| a != b)
            .map(|((word, _), _)| word)
            .collect();
        assert!(
            changed.iter().all(|word| allowed.contains(word)),
            "P picture changed unaccounted words: {:?}",
            changed
                .iter()
                .filter(|w| !allowed.contains(w))
                .collect::<Vec<_>>()
        );
        for required in [221, 236, 239, 240, 1056] {
            assert!(
                changed.contains(&required),
                "word {required} did not change"
            );
        }

        // Values, straight from the reference's rules.
        assert_eq!(p.get(&vepu580_regs::H264_SYNT_SLI0_SLI_TYPE).unwrap(), 0);
        assert_eq!(p.get(&vepu580_regs::H264_SYNT_SLI0_FRM_NUM).unwrap(), 1);
        assert_eq!(p.get(&vepu580_regs::H264_SYNT_SLI1_POC_LSB).unwrap(), 2);
        assert_eq!(p.get(&vepu580_regs::H264_ME_CFG_PMV_MDST_H).unwrap(), 5);
        assert_eq!(p.get(&vepu580_regs::H264_ME_CFG_PMV_MDST_V).unwrap(), 5);
        assert_eq!(p.get(&vepu580_regs::H264_SYNT_NAL_NAL_REF_IDC).unwrap(), 2);
        assert_eq!(
            p.get(&vepu580_regs::H264_SYNT_NAL_NAL_UNIT_TYPE).unwrap(),
            1
        );
        assert_eq!(
            p.get(&vepu580_regs::H264_SYNT_SLI1_IDR_PID).unwrap(),
            (1 << vepu580_regs::H264_SYNT_SLI1_IDR_PID.width) - 1,
            "a P slice passes -1 for idr_pic_id"
        );
        assert_eq!(
            p.get(&vepu580_regs::RCKLUT_KLUT_OFST_CHRM_KLUT_OFST)
                .unwrap(),
            3
        );
        // The P picture's KLUT starts four words into the table.
        assert_eq!(
            p.rc_klut[vepu580_regs::RCKLUT_KLUT_WGT0_CHRM_KLUT_WGT0.word - 1024],
            CHROMA_KLUT_WEIGHT[4]
        );
        // ... and the one differing aq_step entry is seven rather than six.
        assert_eq!(AQ_STEP_I[14], 6);
        assert_eq!(AQ_STEP_P[14], 7);
        assert_eq!(
            (p.rc_klut[1048 - 1024] >> ((14 % 4) * 8)) & 0xff,
            7,
            "aq_step entry 14"
        );
        // The IDR keeps its own values.
        assert_eq!(
            idr.get(&vepu580_regs::H264_SYNT_NAL_NAL_UNIT_TYPE).unwrap(),
            5
        );
        assert_eq!(
            idr.rc_klut[vepu580_regs::RCKLUT_KLUT_WGT0_CHRM_KLUT_WGT0.word - 1024],
            CHROMA_KLUT_WEIGHT[0]
        );
    }

    /// The STATUS field positions come from `Vepu580Status_t`, but what makes them
    /// trustworthy is the C encoder's own read-back of the same window, captured on
    /// t6 and kept as a fixture. A wrong offset here would hand the motion and rate
    /// control logic a silent zero instead of failing this test.
    #[test]
    fn statistics_match_the_c_encoders_readback() {
        let text = std::fs::read_to_string("tests/data/vepu580_status_c_fixqp_2f.txt")
            .expect("C STATUS capture");
        let jobs: Vec<Statistics> = text
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                Statistics::from_words(
                    &line
                        .split_whitespace()
                        .map(|w| u32::from_str_radix(w, 16).expect("hex word"))
                        .collect::<Vec<u32>>(),
                )
            })
            .collect();
        assert_eq!(jobs.len(), 2, "the IDR job and the P job");

        // Values read straight from the C encoder's own read-back on t6: 20x15
        // macroblocks, QP 26 over 300 blocks, and the second picture being a repeat
        // of the first.
        assert_eq!(jobs[0].bitstream_bytes, 825, "the IDR the C encoder wrote");
        assert_eq!(jobs[0].qp_sum, 7800, "qp_sum at reg4099");
        assert_eq!(jobs[0].num_b16, 300, "st_bnum_b16.num_b16 at reg4171");
        assert_eq!(jobs[0].madi, 600, "madi at reg4170");
        assert_eq!(jobs[0].madi_b16num, [300, 0, 0, 0], "reg4177..4180");
        assert_eq!(
            jobs[0].md_sad_b16num,
            [0, 0, 0, 0],
            "no inter search on an IDR"
        );
        assert_eq!(jobs[0].madp, 0);
        assert_eq!(jobs[0].num_ctu, 0, "no CME blocks on an IDR");

        // The P picture populates the CME side, and its mean absolute inter
        // difference is zero because the two pictures are identical - which is the
        // mechanism holding the scene motion level at "still" in this fixture, not a
        // property of the hardware: `num_ctu`, the divisor, is a live 75.
        assert_eq!(jobs[1].bitstream_bytes, 62, "the P picture");
        assert_eq!(jobs[1].qp_sum, 7800);
        assert_eq!(jobs[1].num_b16, 300);
        assert_eq!(jobs[1].madi, 600);
        assert_eq!(
            jobs[1].md_sad_b16num,
            [300, 0, 0, 0],
            "md_sad_b16num0 at reg4173"
        );
        assert_eq!(jobs[1].madi_b16num, [300, 0, 0, 0]);
        assert_eq!(
            jobs[1].madp, 0,
            "identical pictures, so no inter difference"
        );
        assert_eq!(jobs[1].num_ctu, 75, "num_ctu at reg4169");
    }

    #[test]
    fn probe_input_matches_the_c_probe_layout() {
        let input = probe_nv12(REFERENCE_WIDTH, REFERENCE_HEIGHT);
        assert_eq!(input.len(), 115_200);
        // Both planes use the reference probe's integer truncation. In
        // particular, x=0..4 all round to the same luma value.
        assert_eq!(&input[..4], &[16, 16, 16, 16]);
        assert_eq!(input[4], 17);
        assert_eq!(input[REFERENCE_WIDTH * REFERENCE_HEIGHT], 48);
        assert_eq!(input[REFERENCE_WIDTH * REFERENCE_HEIGHT + 1], 208);
    }

    /// Fixed QP as the reference encoder actually programs it, read straight out
    /// of the recorded words rather than through a write of our own. The three QP
    /// fields carry `qp_target`; `rc_en`, `aq_en`, `rc_ctu_num` and `rc_qp_range`
    /// stay zero because `setup_vepu580_rc_base` returns as soon as it has written
    /// them, and with them the `rc_adj` ladder, the `rc_dthd` thresholds, the ROI
    /// area thresholds and `qpmap_mode`. If any generated field position were
    /// wrong, this is the test that says so against recorded hardware state.
    #[test]
    fn the_generated_field_positions_decode_the_recorded_job() {
        let read = |field: &vepu580_regs::Field| -> u32 {
            let word = match field.word {
                160..254 => template::BASE_WORDS[field.word - 160],
                1024..1081 => template::RCKLUT_WORDS[field.word - 1024],
                other => panic!("recorded job does not cover word {other}"),
            };
            (word >> field.shift) & ((1 << field.width) - 1)
        };

        for field in [
            &vepu580_regs::BASE_ENC_PIC_PIC_QP,
            &vepu580_regs::BASE_RC_QP_RC_MIN_QP,
            &vepu580_regs::BASE_RC_QP_RC_MAX_QP,
        ] {
            assert_eq!(
                read(field),
                26,
                "{} of the recorded FIXQP 26 job",
                field.name
            );
        }
        for field in [
            &vepu580_regs::BASE_RC_CFG_RC_EN,
            &vepu580_regs::BASE_RC_CFG_AQ_EN,
            &vepu580_regs::BASE_RC_CFG_AQ_MODE,
            &vepu580_regs::BASE_RC_CFG_RC_CTU_NUM,
            &vepu580_regs::BASE_RC_QP_RC_QP_RANGE,
            &vepu580_regs::RCKLUT_RC_ADJ0_QP_ADJ0,
            &vepu580_regs::RCKLUT_RC_ADJ0_QP_ADJ4,
            &vepu580_regs::RCKLUT_RC_ADJ1_QP_ADJ8,
            &vepu580_regs::RCKLUT_ROI_QTHD0_QPMIN_AREA0,
            &vepu580_regs::RCKLUT_ROI_QTHD3_QPMAX_AREA7,
            &vepu580_regs::RCKLUT_ROI_QTHD3_QPMAP_MODE,
        ] {
            assert_eq!(read(field), 0, "{} stays clear under fixed QP", field.name);
        }
    }

    /// QP has to reach the picture QP and the two RC bounds, and nothing else.
    /// Writing wider than the reference does - into the ROI thresholds, say - is
    /// exactly the failure a recorded image hides, because the picture still
    /// decodes; it just stops being the reference's picture.
    #[test]
    fn qp_moves_the_qp_words_and_nothing_else() {
        let words_of = |file: &RegisterFile| -> Vec<u32> {
            let mut all = Vec::new();
            all.extend_from_slice(&file.ctl);
            all.extend_from_slice(&file.base);
            all.extend_from_slice(&file.rc_klut);
            all.extend_from_slice(&file.param);
            all.extend_from_slice(&file.rdo);
            all.extend_from_slice(&file.scl);
            all.extend_from_slice(&file.osd);
            all
        };
        let absolute_word = |index: usize| -> usize {
            if index < 24 {
                index
            } else if index < 24 + 94 {
                160 + index - 24
            } else if index < 24 + 94 + 57 {
                1024 + index - 24 - 94
            } else if index < 24 + 94 + 57 + 180 {
                1472 + index - 24 - 94 - 57
            } else if index < 24 + 94 + 57 + 180 + 122 {
                2048 + index - 24 - 94 - 57 - 180
            } else if index < 24 + 94 + 57 + 180 + 122 + 679 {
                2176 + index - 24 - 94 - 57 - 180 - 122
            } else {
                3072 + index - 24 - 94 - 57 - 180 - 122 - 679
            }
        };
        let base = words_of(&registers_at(&config(), FrameState::default()));
        for qp in [0u8, 12, 20, 26, 33, 45, 51] {
            let mut at_qp = config();
            at_qp.qp_init = qp;
            let file = registers_at(&at_qp, FrameState::default());
            let moved = words_of(&file);
            let differences: Vec<usize> = (0..base.len())
                .filter(|index| base[*index] != moved[*index])
                .map(|index| absolute_word(index))
                .collect();
            assert!(
                differences.iter().all(|word| *word == 192 || *word == 213),
                "QP {qp} also changed words {differences:?}"
            );
            for field in [
                &vepu580_regs::BASE_ENC_PIC_PIC_QP,
                &vepu580_regs::BASE_RC_QP_RC_MIN_QP,
                &vepu580_regs::BASE_RC_QP_RC_MAX_QP,
            ] {
                assert_eq!(file.get(field).unwrap(), u32::from(qp), "{}", field.name);
            }
        }
    }

    /// QP is a generator input and an accepted configuration across the whole
    /// H.264 range. The recorded-image refusals are unchanged.
    #[test]
    fn qp_is_generated_and_accepted_and_the_rest_of_the_image_is_not() {
        for qp in [0u8, 10, 26, 51] {
            let mut at_qp = config();
            at_qp.qp_init = qp;
            assert_eq!(at_qp.validate().map_err(|e| e.to_string()), Ok(()));
            assert!(
                build_registers(&job(&at_qp, FrameState::default())).is_ok(),
                "QP {qp} generates"
            );
            assert!(verified_on_board(&at_qp).is_ok(), "QP {qp} is accepted");
        }
        let mut too_high = config();
        too_high.qp_init = 52;
        assert!(too_high.validate().is_err(), "QP 52 is not an H.264 QP");

        // Baseline with CABAC is not a thing to generate; validate says so.
        let mut cabac_baseline = config();
        cabac_baseline.entropy_coding = true;
        assert!(
            cabac_baseline.validate().is_err(),
            "Baseline cannot use CABAC"
        );
        assert!(build_registers(&job(&cabac_baseline, FrameState::default())).is_err());

        // GOP is a parameter now - a picture after the first is a P slice with the
        // previous reconstruction as its reference. Level is the one property of the
        // recorded image that is still not compared against the C encoder.
        // A longer GOP is no longer held back: its P pictures match the reference
        // byte for byte. `p_picture_changes_only_the_fields_the_reference_does`
        // pins which fields move between an I picture and an inter one.
        let mut gop = config();
        gop.gop = 30;
        assert!(
            build_registers(&job(&gop, FrameState::default())).is_ok(),
            "a longer GOP generates"
        );
        assert!(
            verified_on_board(&gop).is_ok(),
            "a longer GOP is byte-identical to the reference on the board"
        );
        let mut level = config();
        level.level_idc = 40;
        assert!(
            build_registers(&job(&level, FrameState::default())).is_err(),
            "level has no reference at any other value yet"
        );
    }

    /// Profile and entropy coding are generator inputs now, and the fields
    /// `setup_vepu580_codec` derives from them move accordingly - including
    /// `cbc_init_idc`, which the reference leaves as -1 (3 in the register) under
    /// CAVLC and sets from `cabac_init_idc` under CABAC.
    #[test]
    fn profile_and_entropy_reach_the_codec_fields() {
        let read = |file: &RegisterFile, field: &vepu580_regs::Field| {
            file.get(field)
                .unwrap_or_else(|e| panic!("{}: {e}", field.name))
        };

        let baseline = registers_at(&config(), FrameState::default());
        assert_eq!(read(&baseline, &vepu580_regs::H264_SYNT_PPS_ETPY_MODE), 0);
        assert_eq!(read(&baseline, &vepu580_regs::H264_RDO_CFG_VLC_LMT), 1);
        assert_eq!(read(&baseline, &vepu580_regs::H264_RDO_CFG_RECT_SIZE), 1);
        assert_eq!(
            read(&baseline, &vepu580_regs::H264_SYNT_SLI0_CBC_INIT_IDC),
            3
        );

        let mut main = config();
        main.profile = encoder::PROFILE_MAIN;
        main.entropy_coding = true;
        let main = registers_at(&main, FrameState::default());
        assert_eq!(read(&main, &vepu580_regs::H264_SYNT_PPS_ETPY_MODE), 1);
        assert_eq!(read(&main, &vepu580_regs::H264_RDO_CFG_VLC_LMT), 0);
        assert_eq!(read(&main, &vepu580_regs::H264_RDO_CFG_RECT_SIZE), 0);
        assert_eq!(read(&main, &vepu580_regs::H264_SYNT_SLI0_CBC_INIT_IDC), 0);
        assert_eq!(read(&main, &vepu580_regs::H264_SYNT_PPS_TRNS_8X8), 0);

        let mut high = config();
        high.profile = encoder::PROFILE_HIGH;
        high.entropy_coding = true;
        high.transform_8x8 = true;
        let high = registers_at(&high, FrameState::default());
        assert_eq!(read(&high, &vepu580_regs::H264_SYNT_PPS_TRNS_8X8), 1);
        assert_eq!(read(&high, &vepu580_regs::H264_SYNT_PPS_ETPY_MODE), 1);
    }
}

/// The string MPP would fill with its own `git log` line. Ours says what it is:
/// the framing, UUID, placement and count match MPP's, the content does not
/// borrow another project's build identity.
pub fn version_info(config: &EncoderConfig) -> String {
    format!(
        "{} {} VEPU580 H.264 qp {} gop {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        config.qp_init,
        config.gop
    )
}

/// The register half of rate control, checked against the C encoder's own job.
///
/// `tests/data/vepu580_rc_c_move6_cbr_regs.txt` is a capture of the C encoder
/// encoding six frames at 320x240 CBR 300 kbps. Our fixed-QP image for that size
/// is already byte-identical to the C encoder's fixed-QP job, and the difference
/// between a fixed-QP job and a CBR job is the set `setup_vepu580_rc_base` writes,
/// so rebuilding the image with that branch applied has to land on the captured
/// words. Anything else that moves here is a field this generator touched and
/// should not have.
#[cfg(test)]
mod rate_control_registers {
    use super::*;

    fn fixture() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/vepu580_rc_c_move6_cbr_regs.txt"
        ))
        .expect("the captured CBR jobs")
    }

    /// The windows of one captured job, keyed by section name.
    fn traced_job(job: usize) -> std::collections::HashMap<&'static str, Vec<u32>> {
        let mut out = std::collections::HashMap::new();
        let mut current: Option<usize> = None;
        for line in fixture().lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split_whitespace();
            match fields.next() {
                Some("job") => current = Some(fields.next().unwrap().parse().unwrap()),
                Some(name) if current == Some(job) => {
                    let words: Vec<u32> = fields
                        .map(|w| u32::from_str_radix(w, 16).unwrap())
                        .collect();
                    // The keys are literals rather than `name`, which borrows the
                    // fixture string this function does not outlive.
                    for key in ["BASE", "RCKLUT", "RDO"] {
                        if name == key {
                            out.insert(key, words.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        assert_eq!(out.len(), 3, "job {job} of the capture");
        out
    }

    fn get(words: &[u32], first_word: usize, field: &vepu580_regs::Field) -> u32 {
        let word = words[field.word - first_word];
        (word >> field.shift) & ((1 << field.width) - 1)
    }

    /// The picture's bit target, recovered from the `ctu_ebit` the capture shows -
    /// the only form in which the model's decision reaches the ioctl log. Any
    /// `bit_target` in the same sixteenth-of-a-bit band produces the same word.
    fn bit_target_for(ctu_ebit: i32, mbs: i32) -> i32 {
        (ctu_ebit * mbs + 15) / 16
    }

    fn check(job: usize, is_idr: bool) {
        let traced = traced_job(job);
        let base = &traced["BASE"];
        let mut config = EncoderConfig::new(320, 240);
        // The capture is `mpi_enc_test` at its default intra GOP of 60 over six
        // frames, so only picture 0 is an IDR. Left at the crate default of 1, the
        // GOP walk would make every picture one.
        config.gop = 60;
        let rate = RateControl {
            bit_target: bit_target_for(
                get(base, 160, &vepu580_regs::BASE_RC_TGT_CTU_EBIT) as i32,
                (config.coded_width() / 16 * config.coded_height() / 16) as i32,
            ),
            qp_target: get(base, 160, &vepu580_regs::BASE_ENC_PIC_PIC_QP),
            qp_min: get(base, 160, &vepu580_regs::BASE_RC_QP_RC_MIN_QP),
            qp_max: get(base, 160, &vepu580_regs::BASE_RC_QP_RC_MAX_QP),
            qp_delta_row: 2,
            qp_delta_row_i: 2,
        };

        let mut file = build_registers(&Job {
            config: &config,
            input_fd: 11,
            output_fd: 12,
            current_fd: 13,
            current_thumb_fd: 14,
            reference_fd: 13,
            reference_thumb_fd: 14,
            md_fd: 15,
            rc: None,
            // Job 1 of the capture is the P picture that follows the IDR, so it
            // needs the state the GOP walk would be in by then - `apply_picture`
            // owns the slice-type fields, and rate control only the RC ones.
            state: if is_idr {
                FrameState::default()
            } else {
                FrameState::default().next(config.gop)
            },
            output_capacity: logical_output_size(&config).unwrap(),
        })
        .unwrap();
        file.apply_rate_control(&config, is_idr, &rate).unwrap();

        // The addresses are dma-buf fds, which are per-process by nature; the
        // capture's are the reference's and ours are the fds above.
        let addresses = [
            BASE_ADR_SRC0,
            BASE_ADR_SRC1,
            BASE_ADR_SRC2,
            BASE_RFPW_H,
            BASE_RFPW_B,
            BASE_RFPR_H,
            BASE_RFPR_B,
            BASE_DSPW,
            BASE_DSPR,
            BASE_BSBT,
            BASE_BSBB,
            BASE_BSBR,
            BASE_ADR_BSBS,
            BASE_ADR_MEI,
        ];
        let mut differences = Vec::new();
        for (name, first_word, ours) in [
            ("BASE", 160usize, file.base.as_slice()),
            ("RCKLUT", 1024usize, file.rc_klut.as_slice()),
            ("RDO", 2048usize, file.rdo.as_slice()),
        ] {
            let want = &traced[name];
            for index in 0..ours.len() {
                let is_address = name == "BASE" && addresses.contains(&index);
                if ours[index] != want[index] && !is_address {
                    differences.push(format!(
                        "{name}[{index}] sw{} ours={:08x} c={:08x}",
                        first_word + index,
                        ours[index],
                        want[index]
                    ));
                }
            }
        }
        assert!(
            differences.is_empty(),
            "job {job} differs in {} words:\n{}",
            differences.len(),
            differences.join("\n")
        );
    }

    #[test]
    fn the_c_encoder_s_cbr_intra_job_is_reproduced() {
        check(0, true);
    }

    #[test]
    fn the_c_encoder_s_cbr_p_job_is_reproduced() {
        check(1, false);
    }

    /// The sixteenth-of-a-bit scaling is not decoration: it is what lets a target
    /// of a few thousand bits per picture reach the hardware without rounding away
    /// the fraction, and the reference truncates twice on the way.
    #[test]
    fn the_target_is_scaled_before_it_is_divided_and_the_thresholds_after() {
        let config = EncoderConfig::new(320, 240);
        let mut file = build_registers(&Job {
            config: &config,
            input_fd: 11,
            output_fd: 12,
            current_fd: 13,
            current_thumb_fd: 14,
            reference_fd: 13,
            reference_thumb_fd: 14,
            md_fd: 15,
            rc: None,
            state: FrameState::default(),
            output_capacity: logical_output_size(&config).unwrap(),
        })
        .unwrap();
        let rate = RateControl {
            bit_target: 10_000,
            qp_target: 26,
            qp_min: 10,
            qp_max: 51,
            qp_delta_row: 2,
            qp_delta_row_i: 2,
        };
        file.apply_rate_control(&config, false, &rate).unwrap();

        // (10000 << 4) / (20 * 15) = 533.
        let ebit = &vepu580_regs::BASE_RC_TGT_CTU_EBIT;
        let word = file.base[ebit.word - 160] >> ebit.shift & ((1 << ebit.width) - 1);
        assert_eq!(word, 533, "the scaled target reaches the register");

        // (533 * 20) >> 4 = 666 per macroblock, so the ladder is placed 666 * 5 / 16
        // = 208 either side of zero, with the outer pair four times out.
        let first = vepu580_regs::RCKLUT_RC_DTHD_0_8.word - 1024;
        let dthd: Vec<u32> = (0..4).map(|i| file.rc_klut[first + i]).collect();
        assert_eq!(
            dthd,
            [(-832i32) as u32, (-208i32) as u32, 208, 832,],
            "the thresholds are the C truncations, negatives and all"
        );
    }
}
