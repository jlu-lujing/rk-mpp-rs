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

use std::io::Read;

use crate::codec::h264::encoder::{self, EncoderConfig, GdrMode, RcMode, SplitMode};
use crate::codec::h264::sei;
use crate::codec::rc::model as rc_model;
use crate::reference::cabac::Context;
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{require_video_encode, VideoEncodeHal};
use crate::service::{Batch, Client, MppService, RcbInfo, RegOffsetInfo, SliceInfo};

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
#[cfg(test)]
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

const STATUS_WORDS: usize = 180;
const HW_ERROR_MASK: u32 = 0x3f0;

/// One encoded picture. `headers` is non-empty only when the picture is an IDR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub annex_b: Vec<u8>,
    /// Output packets in emission order. Normally one complete access unit;
    /// with low-delay slicing, one packet per completed hardware slice and the
    /// first also carries `headers`.
    pub packets: Vec<Vec<u8>>,
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

/// Lay out a completed picture the way `mpp_enc_impl.c` emits low-delay
/// partitions: headers ride with the first packet, every hardware slice gets an
/// entry, and a zero-length slice remains a zero-length packet because the
/// kernel can use it to carry the last flag.
fn output_packets(
    headers: &[u8],
    slice_prefix: &[u8],
    payload: &[u8],
    slice_lengths: &[usize],
) -> Result<Vec<Vec<u8>>, String> {
    if slice_lengths.is_empty() {
        let mut packet = Vec::with_capacity(headers.len() + slice_prefix.len() + payload.len());
        packet.extend_from_slice(headers);
        packet.extend_from_slice(slice_prefix);
        packet.extend_from_slice(payload);
        return Ok(vec![packet]);
    }

    let total = slice_lengths
        .iter()
        .try_fold(0usize, |total, length| total.checked_add(*length));
    match total {
        Some(total) if total == payload.len() => {}
        Some(total) => {
            return Err(format!(
                "H.264 slice lengths sum to {total}, STATUS says {}",
                payload.len()
            ))
        }
        None => return Err("H.264 slice length sum overflowed usize".into()),
    }

    let mut packets = Vec::with_capacity(slice_lengths.len());
    let mut offset = 0;
    for (index, length) in slice_lengths.iter().copied().enumerate() {
        let mut packet = if index == 0 {
            headers.to_vec()
        } else {
            Vec::new()
        };
        if length != 0 {
            packet.extend_from_slice(slice_prefix);
        }
        packet.extend_from_slice(&payload[offset..offset + length]);
        packets.push(packet);
        offset += length;
    }
    Ok(packets)
}

fn prefix_slices(
    payload: &[u8],
    slice_lengths: &[usize],
    slice_prefix: &[u8],
) -> Result<Vec<u8>, String> {
    if slice_prefix.is_empty() {
        return Ok(payload.to_vec());
    }
    if slice_lengths.is_empty() {
        let mut prefixed = Vec::with_capacity(slice_prefix.len() + payload.len());
        prefixed.extend_from_slice(slice_prefix);
        prefixed.extend_from_slice(payload);
        return Ok(prefixed);
    }
    let total = slice_lengths
        .iter()
        .try_fold(0usize, |total, length| total.checked_add(*length));
    match total {
        Some(total) if total == payload.len() => {}
        Some(total) => {
            return Err(format!(
                "H.264 slice lengths sum to {total}, STATUS says {}",
                payload.len()
            ))
        }
        None => return Err("H.264 slice length sum overflowed usize".into()),
    }
    let prefix_bytes = slice_prefix
        .len()
        .checked_mul(slice_lengths.len())
        .ok_or_else(|| "H.264 slice prefix length overflowed usize".to_string())?;
    let mut prefixed = Vec::with_capacity(payload.len() + prefix_bytes);
    let mut offset = 0;
    for length in slice_lengths {
        if *length != 0 {
            prefixed.extend_from_slice(slice_prefix);
            prefixed.extend_from_slice(&payload[offset..offset + *length]);
            offset += *length;
        }
    }
    Ok(prefixed)
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
    /// `EncFrmStatus.ref_mode == REF_TO_PREV_INTRA`: a smart-GOP virtual-intra
    /// P picture, predicted from the retained IDR.
    pub is_vi: bool,
    pub frame_num: u32,
    pub idr_pic_id: u32,
    pub poc: i32,
    /// `EncFrmStatus.is_i_refresh`: this picture carries a GDR force-intra band.
    pub is_i_refresh: bool,
    /// `EncFrmStatus.is_i_recovery`: this picture starts a GDR sweep, the frame
    /// the reference would prefix with a recovery point SEI.
    pub is_i_recovery: bool,
    /// `seq_idx % gop`, the index `setup_vepu580_intra_refresh` slides the band
    /// with. Meaningful only when [`FrameState::is_i_refresh`].
    pub refresh_idx: u32,
}

impl Default for FrameState {
    fn default() -> Self {
        Self {
            index: 0,
            is_idr: true,
            is_vi: false,
            frame_num: 0,
            idr_pic_id: 0,
            poc: 0,
            is_i_refresh: false,
            is_i_recovery: false,
            refresh_idx: 0,
        }
    }
}

impl FrameState {
    /// The state of the next picture. `gop` is the IDR interval: picture 0 and
    /// every `gop`-th picture after it opens a new GOP with an IDR.
    ///
    /// `refresh_length` is [`EncoderConfig::gdr_refresh_length`], zero when GDR
    /// is off. With it set the reference stops closing GOPs with IDRs at all -
    /// `mpp_enc_refs.cpp` skips `cleanup_cpb` whenever `refresh_length` is set,
    /// so only picture zero is an IDR and every IGOP boundary is a refresh
    /// sweep instead. The two flags follow `set_frm_refresh_flag` exactly:
    ///
    /// * `is_i_refresh = (seq_idx % igop) < refresh_length && seq_cnt > 1`;
    /// * `is_i_recovery = !(seq_idx % igop) && seq_cnt > 1`;
    ///
    /// with `seq_cnt` the IGOP counter `update_hdr` recomputes as
    /// `seq_idx / igop + 1` once `seq_idx` reaches the IGOP - so `seq_cnt > 1`
    /// is `index >= gop`, which is why the refresh window only starts *after*
    /// the first full IGOP, not inside it.
    pub fn next(&self, gop: u32, refresh_length: usize) -> FrameState {
        let gop = gop.max(1) as usize;
        let index = self.index + 1;
        let is_idr = refresh_length == 0 && index % gop == 0;
        if is_idr {
            return FrameState {
                index,
                is_idr: true,
                is_vi: false,
                frame_num: 0,
                // `h264e_slice_init` increments idr_pic_id across IDR pictures;
                // a P picture carries the id of the IDR that opened its GOP.
                idr_pic_id: self.idr_pic_id + 1,
                poc: 0,
                ..FrameState::default()
            };
        }
        let frame_num = self.frame_num + 1;
        let refresh_idx = (index % gop) as u32;
        let past_first_igop = refresh_length > 0 && index >= gop;
        FrameState {
            index,
            is_idr: false,
            is_vi: false,
            frame_num,
            idr_pic_id: self.idr_pic_id,
            poc: self.poc + 2 * (frame_num - self.frame_num) as i32,
            is_i_refresh: past_first_igop && (refresh_idx as usize) < refresh_length,
            is_i_recovery: past_first_igop && refresh_idx == 0,
            refresh_idx,
        }
    }
}

fn next_picture_state(
    state: FrameState,
    gop: u32,
    refresh_length: usize,
    tsvc_reference_step: bool,
    last_non_reference: bool,
    non_reference: bool,
) -> FrameState {
    let mut next = state.next(gop, refresh_length);
    if tsvc_reference_step && last_non_reference && !non_reference && !next.is_idr {
        // TSVC2 encodes a disposable temporal-1 P picture and the following
        // temporal-0 reference P picture as one coded frame, so the reference P
        // reuses frame_num. Their POCs still advance independently.
        next.frame_num = state.frame_num;
    }
    next
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
    /// The scene-motion state as it stands before this picture - the register
    /// patch reads it, the next status update moves it.
    pub tune: &'a SceneTune,
    /// The ROI tables' dma-buf fds, `roi_addr` and `roi_qp_addr`, or -1 when
    /// the configuration carries no regions.
    pub roi_fd: i32,
    pub roi_qp_fd: i32,
    /// The GDR force-intra base table's fd, or -1 when GDR is off. The register
    /// writes happen only when `state.is_i_refresh` - which is when C runs
    /// `setup_vepu580_intra_refresh`, after `setup_vepu580_roi` and with its own
    /// buffer overwriting `roi_addr`.
    pub gdr_fd: i32,
    /// The >4096 CIME extension line buffer's fd and the offset its second
    /// (`ebufb`) read starts at, both zero when the coded width needs no
    /// extension (`setup_vepu580_ext_line_buf`).
    pub ext_fd: i32,
    pub ext_offset: u32,
    /// `save_pass1`: write a linear reconstruction into the two-pass buffer.
    /// `use_pass1`: read that reconstruction as this job's source instead.
    pub save_pass1: bool,
    pub use_pass1: bool,
    pub non_reference: bool,
    pub current_long_term: bool,
    pub pass1_fd: i32,
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
    /// The `MPP_DEV_RCB_INFO` entries of `setup_vepu580_ext_line_buf`, empty
    /// for every width the extension lines do not reach.
    pub rcb: Vec<RcbInfo>,
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
            // C reads `sps->level_idc`, which `h264e_sps_init` has already raised to
            // what the coded size needs; these fields follow that value, not the raw
            // request - at 1280x720 a request of 1.1 becomes 3.1 on both sides, which
            // drops `rect_size` and widens the CIME window.
            u32::from(
                config.profile == encoder::PROFILE_BASELINE && config.effective_level_idc() <= 30,
            ),
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

    /// The deblocking fields of `setup_vepu580_codec`, which reach the slice
    /// header the hardware emits. Two statements, matching the reference's two:
    ///
    /// * `synt_sli2.dis_dblk_idc = slice->disable_deblocking_filter_idc` - a
    ///   `RK_U32 : 2` bitfield assignment, so anything above three truncates the
    ///   way C does rather than being refused;
    /// * `synt_sli2.sli_alph_ofst = slice->slice_alpha_c0_offset_div2` - a signed
    ///   value into a 4-bit field, stored as two's complement, so -3 reads back
    ///   as 13 exactly as the C's bitfield would.
    ///
    /// There is deliberately no beta statement: the VEPU580 HAL never writes
    /// `sli_beta_ofst` even though the register declares it, so the field stays
    /// at the zero the register set was memset to whatever the caller asked for.
    /// `deblock_offset_beta` therefore travels with the configuration and changes
    /// nothing on VEPU580 - the unit test below pins that, because matching the
    /// reference means matching what it *does*, not what it ought to do.
    fn apply_deblock(&mut self, config: &EncoderConfig) -> Result<(), String> {
        self.set(
            &vepu580_regs::H264_SYNT_SLI2_DIS_DBLK_IDC,
            config.deblock_disable & 3,
        )?;
        self.set_signed(
            &vepu580_regs::H264_SYNT_SLI2_SLI_ALPH_OFST,
            config.deblock_offset_alpha,
        )?;
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
    /// Slot zero retains the IDR for smart-GOP VI pictures; the two working
    /// slots alternate after it.
    recons: [Recon; 3],
    md_info: DmaBuf,
    state: FrameState,
    slot: usize,
    /// A PSKIP consumes input/POC but neither advances frame_num nor becomes a
    /// reference. The next accepted picture therefore reuses the PSKIP attempt's
    /// current slot and its previous accepted reference slot.
    next_slot_override: Option<usize>,
    next_reference_override: Option<usize>,
    next_non_reference_override: Option<bool>,
    next_prefix_temporal_id_override: Option<u8>,
    next_reference_reorder_override: Option<crate::encoder::ReferenceReorder>,
    next_current_long_term_override: Option<u32>,
    last_non_reference: bool,
    /// The rate-control model, present only in CBR mode.
    rc: Option<rc_model::Model>,
    /// The scene-motion detector, updated from every picture's STATUS window.
    tune: SceneTune,
    input_fd_override: Option<i32>,
    input_prepared: bool,
    forced_reference_slot: Option<usize>,
    /// The ROI tables, written once: the regions do not move between
    /// pictures, exactly as in the reference harness that adds the same
    /// regions to every frame's meta.
    roi: Option<(DmaBuf, DmaBuf)>,
    /// The GDR force-intra base table, rewritten before every refresh picture
    /// - the band moves, so unlike the ROI tables this one is not static. The
    /// reference's `ctx->roi_base_cfg_buf`, filled by `setup_vepu580_intra_refresh`
    /// and cache-flushed with `mpp_buffer_sync_end` on every such frame.
    gdr_base: Option<DmaBuf>,
    /// The >4096-pixel CIME extension line buffer and its half size: one
    /// allocation whose second half the `ebufb` register reaches through the
    /// offset table, `ctx->ext_line_buf` of the HAL.
    ext_line: Option<(DmaBuf, usize)>,
    /// `buf_pass1`, VEPU580's linear NV12 reconstruction for two-pass debreath.
    pass1: Option<DmaBuf>,
    /// `rc_info_prev`: the accepted picture's RC request, reused by the next
    /// pass1 job before the normal intra RC start runs.
    last_rate_control: Option<RateControl>,
    /// The `MppEncHeaderStatus` added-bits mirror: once the parameter sets and
    /// SEIs have been emitted, `mpp_enc_impl.cpp` refuses to add them again
    /// until a forced IDR or a header-affecting reconfiguration clears the bits
    /// (`HDR_ADDED_MASK`). `mpp_enc_refs.cpp` never clears them on a GDR sweep
    /// boundary, which is why recovery frames after the opening IDR carry no
    /// prefix in the reference's own steady state.
    hdr_added: bool,
}

impl Vepu580H264 {
    pub fn open(config: EncoderConfig) -> Result<Self, String> {
        require_video_encode(VideoEncodeHal::Vepu580)?;
        validate_reference_config(&config)?;
        verified_on_board(&config)?;
        let input_size = input_size(&config)?;
        let output_size = input_size
            .checked_add(REFERENCE_OUTPUT_MARGIN)
            .ok_or_else(|| "H.264 output size overflow".to_string())?;
        let (pixel_size, thumb_size) = reconstruction_sizes(&config)?;
        if config.gdr && config.rc_mode != RcMode::FixedQp {
            // Under rate control the refresh pictures are a fourth frame type
            // - `INTRA_RFH_FRAME`, with its own bit-allocation window `i_refresh_bit`
            // in `rc_model_v2.c`. The register path below is mode-independent,
            // but the bit budget a refresh picture gets is not, and that part of
            // `rc_model_v2` is not ported; claiming the pair would hand a refresh
            // picture the plain-P budget the model does not know it should not
            // have. Until the window is ported, GDR is a fixed-QP feature.
            return Err(
                "GDR is only wired under fixed QP; the i_refresh_bit window \
                        of rc_model_v2 is not ported"
                    .into(),
            );
        }
        let gdr_base = if config.gdr {
            let size = gdr_base_table(&config, 0).len();
            Some(
                DmaBuf::alloc("system", size, false)
                    .map_err(|e| format!("allocate GDR base table: {e}"))?,
            )
        } else {
            None
        };
        let ext_line = match ext_line_buf_size(&config) {
            0 => None,
            size => Some((
                DmaBuf::alloc("system", size, false)
                    .map_err(|e| format!("allocate CIME extension lines: {e}"))?,
                size,
            )),
        };

        Ok(Self {
            svc: MppService::open(Client::RkvEnc as u32)
                .map_err(|e| format!("open rkv encoder: {e}"))?,
            started: false,
            input: DmaBuf::alloc("system-uncached", input_size, false)
                .map_err(|e| format!("allocate H.264 input: {e}"))?,
            output: DmaBuf::alloc("system", output_size, false)
                .map_err(|e| format!("allocate H.264 output: {e}"))?,
            recons: [
                Recon::alloc(pixel_size, thumb_size)?,
                Recon::alloc(pixel_size, thumb_size)?,
                Recon::alloc(pixel_size, thumb_size)?,
            ],
            md_info: DmaBuf::alloc("system", mdinfo_size(&config)?, false)
                .map_err(|e| format!("allocate H.264 motion info: {e}"))?,
            roi: if config.rois.is_empty() {
                None
            } else {
                let (base, qp) = roi_tables(&config);
                let mut base_buf = DmaBuf::alloc("system", base.len(), false)
                    .map_err(|e| format!("allocate ROI base table: {e}"))?;
                let mut qp_buf = DmaBuf::alloc("system", qp.len(), false)
                    .map_err(|e| format!("allocate ROI qp table: {e}"))?;
                base_buf
                    .sync_write_begin()
                    .map_err(|e| format!("ROI base write: {e}"))?;
                base_buf.as_mut_slice()[..base.len()].copy_from_slice(&base);
                base_buf
                    .sync_write_end()
                    .map_err(|e| format!("ROI base flush: {e}"))?;
                qp_buf
                    .sync_write_begin()
                    .map_err(|e| format!("ROI qp write: {e}"))?;
                qp_buf.as_mut_slice()[..qp.len()].copy_from_slice(&qp);
                qp_buf
                    .sync_write_end()
                    .map_err(|e| format!("ROI qp flush: {e}"))?;
                Some((base_buf, qp_buf))
            },
            gdr_base,
            ext_line,
            pass1: if config.debreath_en {
                Some(
                    DmaBuf::alloc("system", pass1_luma_size(&config) * 3 / 2, false)
                        .map_err(|e| format!("allocate H.264 pass1 reconstruction: {e}"))?,
                )
            } else {
                None
            },
            last_rate_control: None,
            hdr_added: false,
            state: FrameState::default(),
            slot: 0,
            next_slot_override: None,
            next_reference_override: None,
            next_non_reference_override: None,
            next_prefix_temporal_id_override: None,
            next_reference_reorder_override: None,
            next_current_long_term_override: None,
            last_non_reference: false,
            tune: SceneTune::new(false),
            input_fd_override: None,
            input_prepared: false,
            forced_reference_slot: None,
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

    pub fn force_reference_slot(&mut self, slot: Option<usize>) -> Result<(), String> {
        if slot.is_some_and(|slot| slot >= self.recons.len()) {
            return Err("H.264 reference slot is outside the reconstruction pool".to_string());
        }
        self.forced_reference_slot = slot;
        Ok(())
    }

    pub fn force_next_picture_layout(
        &mut self,
        current_slot: usize,
        reference_slot: usize,
        non_reference: bool,
        temporal_id: Option<u8>,
        reference_reorder: Option<crate::encoder::ReferenceReorder>,
        current_long_term_index: Option<u32>,
    ) -> Result<(), String> {
        if current_slot >= self.recons.len() || reference_slot >= self.recons.len() {
            return Err("H.264 picture layout uses a slot outside the reconstruction pool".into());
        }
        if temporal_id.is_some_and(|id| id > 7) {
            return Err("H.264 temporal_id is outside the three-bit SVC range".into());
        }
        self.next_slot_override = Some(current_slot);
        self.next_reference_override = Some(reference_slot);
        self.next_non_reference_override = Some(non_reference);
        self.next_prefix_temporal_id_override = temporal_id;
        self.next_reference_reorder_override = reference_reorder;
        self.next_current_long_term_override = current_long_term_index;
        Ok(())
    }

    /// Read one complete input picture directly into the preallocated DMA
    /// mapping. No staging `Vec` or second copy is involved.
    pub fn load_input_frame<R: Read>(
        &mut self,
        expected: usize,
        reader: &mut R,
    ) -> Result<bool, String> {
        if expected > self.input.len() {
            return Err(format!(
                "H.264 input surface is {} bytes, needs {expected}",
                self.input.len()
            ));
        }
        self.input_prepared = false;
        self.input
            .sync_write_begin()
            .map_err(|error| format!("H.264 input sync begin: {error}"))?;
        let read_result = (|| {
            let mut filled = 0usize;
            let buffer = &mut self.input.as_mut_slice()[..expected];
            while filled < expected {
                match reader.read(&mut buffer[filled..]) {
                    Ok(0) => return Ok(false),
                    Ok(count) => filled += count,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            Ok(true)
        })();
        self.input
            .sync_write_end()
            .map_err(|error| format!("H.264 input flush: {error}"))?;
        let complete = read_result?;
        self.input_prepared = complete;
        Ok(complete)
    }

    pub fn encode_prepared_frame(&mut self) -> Result<EncodedFrame, String> {
        if !self.input_prepared {
            return Err("H.264 input frame was not prepared".to_string());
        }
        self.encode_frame(&[])
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
        out.extend(self.idr_seis()?);
        Ok(out)
    }

    /// The two user-data SEI NALUs `mpp_enc_add_sw_header` emits ahead of every
    /// IDR, independently of whether the SPS/PPS header has already been added.
    fn idr_seis(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        out.extend(sei::user_data(
            &sei::UUID_VERSION,
            version_info(&self.config).as_bytes(),
        ));
        let mut line = match self.config.rc_mode {
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
            })
            .with_quality(self.config.rc_qp_min, self.config.rc_qp_max),
        };
        // A caller who left the initial quality unset hands the reference a -1
        // sentinel (`rc:qp_init = -1`), and `update_rc_cfg_log` prints what the
        // caller set, not what the bit budget derived - `cal_first_i_start_qp`
        // only ever moves the encoder, never this line.
        if self.config.quality_auto {
            line.init_quality = -1;
        }
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

    /// Submit one VEPU580 job and collect its payload and STATUS. The normal
    /// picture loop and the deflicker pass1 path have different RC ownership but
    /// the same register/ioctl lifecycle, so they share this executor.
    fn run_hardware_job(
        &mut self,
        state: FrameState,
        rc: Option<RateControl>,
        current_slot: usize,
        reference_slot: usize,
        save_pass1: bool,
        use_pass1: bool,
        non_reference: bool,
        reference_pattern_active: bool,
        reference_reorder: Option<crate::encoder::ReferenceReorder>,
        current_long_term_index: Option<u32>,
        input_fd: i32,
    ) -> Result<(Vec<u8>, Vec<usize>, Statistics), String> {
        let pass1_fd = self.pass1.as_ref().map_or(-1, |b| b.fd());
        let current_fd = if save_pass1 {
            pass1_fd
        } else {
            self.recons[current_slot].pixel.fd()
        };
        let job = Job {
            config: &self.config,
            input_fd,
            output_fd: self.output.fd(),
            current_fd,
            current_thumb_fd: self.recons[current_slot].thumb.fd(),
            md_fd: self.md_info.fd(),
            reference_fd: self.recons[reference_slot].pixel.fd(),
            reference_thumb_fd: self.recons[reference_slot].thumb.fd(),
            state,
            output_capacity: logical_output_size(&self.config)?,
            rc,
            tune: &self.tune,
            roi_fd: self.roi.as_ref().map_or(-1, |b| b.0.fd()),
            roi_qp_fd: self.roi.as_ref().map_or(-1, |b| b.1.fd()),
            gdr_fd: self.gdr_base.as_ref().map_or(-1, |b| b.fd()),
            ext_fd: self.ext_line.as_ref().map_or(-1, |b| b.0.fd()),
            ext_offset: self.ext_line.as_ref().map_or(0, |b| b.1 as u32),
            save_pass1,
            use_pass1,
            non_reference,
            current_long_term: current_long_term_index.is_some(),
            pass1_fd,
        };
        if self.config.gdr && state.is_i_refresh {
            let table = gdr_base_table(&self.config, state.refresh_idx);
            let buffer = self
                .gdr_base
                .as_mut()
                .ok_or_else(|| "GDR enabled without a base table".to_string())?;
            buffer
                .sync_write_begin()
                .map_err(|e| format!("GDR table sync begin: {e}"))?;
            buffer.as_mut_slice()[..table.len()].copy_from_slice(&table);
            buffer
                .sync_write_end()
                .map_err(|e| format!("GDR table flush: {e}"))?;
        }
        let registers = build_registers(&job)?;
        dump_job_registers(&job, &registers);

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
        if !registers.rcb.is_empty() {
            batch.rcb_info(&registers.rcb);
        }
        batch.reg_read(&mut hw_status, HW_STATUS_OFFSET);
        batch.reg_read(&mut status, STATUS_OFFSET);
        self.svc
            .submit_batch_recovering(&mut batch)
            .map_err(|e| format!("H.264 register submit: {e}"))?;
        let mut slice_lengths = Vec::new();
        if self.config.split_out & encoder::MPP_ENC_SPLIT_OUT_LOWDELAY != 0 && !save_pass1 {
            let mut complete = false;
            for _ in 0..=4096 {
                let mut info = [SliceInfo::default()];
                let count = self
                    .svc
                    .poll_irq(&mut info)
                    .map_err(|e| format!("H.264 slice poll: {e}"))?;
                for slice in &info[..count] {
                    let (length, last) = slice.vepu580();
                    slice_lengths.push(length as usize);
                    complete |= last;
                }
                if complete {
                    break;
                }
            }
            if !complete {
                return Err("H.264 slice FIFO did not report the last slice".into());
            }
        } else {
            self.svc
                .poll_hw_finish_recovering()
                .map_err(|e| format!("H.264 poll: {e}"))?;
        }

        let errors = hw_status[0] & HW_ERROR_MASK;
        if errors != 0 {
            let _ = self.svc.restart_after_hardware_error();
            return Err(format!(
                "VEPU580 H.264 hardware error mask 0x{errors:03x} (int_sta=0x{:08x})",
                hw_status[0]
            ));
        }
        if hw_status[0] & 1 == 0 {
            let _ = self.svc.restart_after_hardware_error();
            return Err(format!(
                "VEPU580 H.264 completed ioctl without enc_done (int_sta=0x{:08x})",
                hw_status[0]
            ));
        }

        let payload_len = status[0] as usize;
        if payload_len < 8 || payload_len > self.output.len() {
            let _ = self.svc.restart_after_hardware_error();
            return Err(format!(
                "invalid VEPU580 H.264 length {payload_len} (capacity {})",
                self.output.len()
            ));
        }
        self.output
            .sync_read_begin()
            .map_err(|e| format!("H.264 output invalidate: {e}"))?;
        let mut payload = self.output.as_slice()[..payload_len].to_vec();
        let nal_validation = validate_nal_bytes(&payload, if state.is_idr { 5 } else { 1 });
        self.output
            .sync_read_end()
            .map_err(|e| format!("H.264 output sync end: {e}"))?;
        nal_validation?;
        if self.forced_reference_slot == Some(0) && !reference_pattern_active {
            payload = amend_default_reference_slice(
                payload,
                &self.config,
                &state,
                &self.parameter_sets()?,
            )?;
        } else if reference_reorder.is_some() || current_long_term_index.is_some() {
            payload = amend_reference_slice(
                payload,
                &self.config,
                &state,
                &self.parameter_sets()?,
                reference_reorder,
                current_long_term_index,
            )?;
        }

        let statistics = Statistics::from_words(&status);
        dump_statistics(&state, &status);
        self.tune.update(&statistics);
        Ok((payload, slice_lengths, statistics))
    }

    /// Encode one picture, continuing the GOP: the first call of a session is an
    /// IDR and every `gop`-th picture after it opens a new one.
    pub fn encode_frame_fd(&mut self, input_fd: i32) -> Result<EncodedFrame, String> {
        if input_fd < 0 {
            return Err("H.264 input fd is invalid".to_string());
        }
        self.input_fd_override = Some(input_fd);
        self.encode_frame(&[])
    }

    pub fn encode_frame(&mut self, nv12: &[u8]) -> Result<EncodedFrame, String> {
        let previous_state = self.state;
        let previous_slot = self.slot;
        let non_reference = self.next_non_reference_override.take().unwrap_or(false);
        let prefix_temporal_id = self.next_prefix_temporal_id_override.take();
        let reference_reorder = self.next_reference_reorder_override.take();
        let current_long_term = self.next_current_long_term_override.take();
        if self.state.index != 0 || self.started {
            self.state = next_picture_state(
                self.state,
                self.config.gop,
                self.config.gdr_refresh_length(),
                prefix_temporal_id.is_some(),
                self.last_non_reference,
                non_reference,
            );
            self.state.is_vi = self.config.smart_p_vgop().is_some_and(|vi_len| {
                !self.state.is_idr
                    && self.state.index % self.config.gop.max(1) as usize == vi_len as usize
            });
            self.slot = self.next_slot_override.take().unwrap_or_else(|| {
                if self.state.is_idr {
                    0
                } else if previous_slot == 1 {
                    2
                } else {
                    1
                }
            });
        } else {
            self.started = true;
        }
        let state = self.state;
        let expected = input_size(&self.config)?;
        let input_fd = if let Some(input_fd) = self.input_fd_override.take() {
            if input_fd < 0 {
                return Err("H.264 input fd is invalid".to_string());
            }
            input_fd
        } else if self.input_prepared {
            self.input_prepared = false;
            self.input.fd()
        } else {
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
            self.input.fd()
        };

        let override_slot = self.next_reference_override.take();
        let reference_slot = override_slot
            .or(self.forced_reference_slot)
            .unwrap_or_else(|| {
                if state.is_idr || state.is_vi || state.index <= 1 {
                    0
                } else {
                    previous_slot
                }
            });
        // The model decides the picture's bits and QP before the register file is
        // built; under fixed QP there is no decision to make. The C re-encodes
        // the same picture in the same reconstruction slot when the first result
        // overshoots, so the loop reuses `state`, input and reference surfaces and
        // only replaces the RC decision and output payload.
        let frame = picture_frame_type(&state);
        let use_pass1 = if self.config.debreath_en && state.is_idr && state.index > 0 {
            let pass_state = FrameState {
                index: state.index,
                is_idr: false,
                is_vi: false,
                frame_num: previous_state.frame_num + 1,
                idr_pic_id: previous_state.idr_pic_id,
                poc: previous_state.poc + 2,
                is_i_refresh: false,
                is_i_recovery: false,
                refresh_idx: 0,
            };
            self.run_hardware_job(
                pass_state,
                self.last_rate_control,
                self.slot,
                previous_slot,
                true,
                false,
                false,
                false,
                None,
                None,
                input_fd,
            )?;
            true
        } else {
            false
        };
        let (bit_target, mut quality_target) = match self.rc.as_mut() {
            Some(model) => model.start(frame),
            None => (0, i32::from(self.config.qp_init)),
        };
        let (payload, slice_lengths, statistics, measured, disposition) = loop {
            let rc = self.rc.as_ref().map(|_| RateControl {
                bit_target,
                qp_target: quality_target as u32,
                qp_min: self.config.rc_qp_min,
                qp_max: self.config.rc_qp_max,
                qp_delta_row: 2,
                qp_delta_row_i: 2,
            });
            let (payload, slice_lengths, statistics) = self.run_hardware_job(
                state,
                rc,
                self.slot,
                reference_slot,
                false,
                use_pass1,
                non_reference,
                prefix_temporal_id.is_some(),
                reference_reorder,
                current_long_term,
                input_fd,
            )?;
            let mbs = ((self.config.coded_width() / 16) * (self.config.coded_height() / 16)) as u32;
            let measured = rc_model::Measured {
                bits: statistics.bitstream_bytes as i32 * 8,
                quality: statistics.qp_sum as i32 / mbs as i32,
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
                iblk4_prop: statistics.iblk4_prop(mbs),
            };
            if let Some(model) = self.rc.as_mut() {
                if model.drops_super_frame(frame, measured) || model.requests_drop(frame, measured)
                {
                    let mut dropped_measurement = measured;
                    // `mpp_enc_reenc_drop` reports the requested QP as the
                    // dropped picture's quality and zero committed bits, not the
                    // rejected hardware result.
                    dropped_measurement.bits = 0;
                    dropped_measurement.quality = quality_target;
                    break (
                        Vec::new(),
                        Vec::new(),
                        statistics,
                        dropped_measurement,
                        PictureDisposition::Dropped,
                    );
                }
                if model.requests_pskip(frame, measured) {
                    let payload = pskip_nal(&self.config, &state)?;
                    let pskip_measurement = rc_model::Measured {
                        // The reference's software-slice task counter reports
                        // the slice-header bit count, not the packet length:
                        // 13 bits for CAVLC and 15 for CABAC (`cabac_init_idc`
                        // plus the alignment bits). Board tracing pins both.
                        bits: if self.config.entropy_coding { 15 } else { 13 },
                        quality: quality_target,
                        madi: measured.madi,
                        madp: measured.madp,
                        iblk4_prop: measured.iblk4_prop,
                    };
                    break (
                        payload,
                        Vec::new(),
                        statistics,
                        pskip_measurement,
                        PictureDisposition::Pskip,
                    );
                }
                if let Some(next_quality) = model.reencode_quality(
                    frame,
                    bit_target,
                    quality_target,
                    measured,
                    self.config.max_reencode_times,
                ) {
                    quality_target = next_quality;
                    continue;
                }
            }
            break (
                payload,
                slice_lengths,
                statistics,
                measured,
                PictureDisposition::Accepted,
            );
        };

        // SPS and PPS precede an IDR only; a P picture rides on the parameters it
        // already established, which is what `mpi_enc_test` streams too.
        //
        // The GDR exception is measured, not inferred: `scripts/board_h264_gdr_vs_mpp.sh`
        // shows the reference re-prefixing SPS, PPS and a recovery-point SEI (payload
        // `06 02 24 40`, not the version or rc-cfg SEI) at every intra refresh - the
        // IGOP boundary that opens the next sweep - while emitting no IDR there. A
        // `hdr_status` that never re-armed between IGOPs would repeat nothing, which
        // is what this driver did until that cell said otherwise.
        let sweep_open = self.config.gdr
            && self.config.gdr_refresh_length() > 0
            && state.index > 0
            && state.index % self.config.gop as usize == 0;
        let headers = if disposition != PictureDisposition::Accepted {
            Vec::new()
        } else if state.is_idr {
            let mut out = self.parameter_sets()?;
            self.hdr_added = true;
            out.extend(self.idr_seis()?);
            out
        } else if sweep_open {
            self.hdr_added = true;
            // The measured prefix at a sweep opening is the bare parameter sets
            // plus the recovery-point SEI - the uuid_version and uuid_rc_cfg
            // accompany the IDR only, once per session.
            let mut out = self.parameter_sets()?;
            out.extend(sei::recovery_point(
                (self.config.gdr_refresh_length() - 1) as u32,
            ));
            out
        } else {
            Vec::new()
        };
        let slice_prefix = prefix_temporal_id
            .map(|temporal_id| {
                svc_prefix_nal(
                    state.is_idr,
                    if state.is_idr {
                        3
                    } else if non_reference {
                        0
                    } else {
                        2
                    },
                    temporal_id,
                )
            })
            .unwrap_or_default();
        let prefixed_payload = prefix_slices(&payload, &slice_lengths, &slice_prefix)?;
        let mut annex_b = Vec::with_capacity(headers.len() + payload.len());
        annex_b.extend_from_slice(&headers);
        annex_b.extend_from_slice(&prefixed_payload);
        let packets = output_packets(&headers, &slice_prefix, &payload, &slice_lengths)?;
        self.last_rate_control = self.rc.as_ref().map(|_| RateControl {
            bit_target,
            qp_target: quality_target as u32,
            qp_min: self.config.rc_qp_min,
            qp_max: self.config.rc_qp_max,
            qp_delta_row: 2,
            qp_delta_row_i: 2,
        });
        if let Some(model) = self.rc.as_mut() {
            model.end_with(frame, bit_target, measured);
            if disposition != PictureDisposition::Accepted {
                // The reference's drop and software-PSKIP paths reach
                // `rc_model_v2_end` twice for the replaced picture.
                model.end_with(frame, bit_target, measured);
            } else if disposition == PictureDisposition::Accepted {
                model.accept_frame();
            }
        }
        if disposition == PictureDisposition::Dropped {
            // `mpp_enc_refs_rollback`: a dropped input does not advance the
            // encoded sequence, frame number, POC or reference slot.
            self.state = previous_state;
            self.slot = previous_slot;
        } else if disposition == PictureDisposition::Pskip {
            // A non-reference PSKIP consumes sequence/POC but does not advance
            // frame_num, and the next accepted picture reuses its working slot.
            self.state = FrameState {
                frame_num: state.frame_num.saturating_sub(1),
                ..state
            };
            self.next_slot_override = Some(self.slot);
            self.next_reference_override = Some(reference_slot);
        }
        if disposition != PictureDisposition::Dropped {
            self.last_non_reference = non_reference;
        }
        Ok(EncodedFrame {
            annex_b,
            packets,
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

fn picture_frame_type(state: &FrameState) -> rc_model::FrameType {
    if state.is_idr {
        rc_model::FrameType::Intra
    } else if state.is_vi {
        rc_model::FrameType::InterVi
    } else {
        rc_model::FrameType::InterP
    }
}

fn svc_prefix_nal(is_idr: bool, nal_ref_idc: u8, temporal_id: u8) -> Vec<u8> {
    debug_assert!(nal_ref_idc <= 3);
    debug_assert!(temporal_id <= 7);
    let mut prefix = Vec::with_capacity(if nal_ref_idc == 0 { 4 } else { 5 });
    prefix.extend_from_slice(&[0, 0, 0, 1]);
    prefix.push((nal_ref_idc << 5) | 14);
    prefix.push(0x80 | (u8::from(is_idr) << 6) | temporal_id);
    prefix.push(0x80);
    prefix.push((temporal_id << 5) | 0x07);
    if nal_ref_idc != 0 {
        prefix.push(0x20);
    }
    prefix
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PictureDisposition {
    Accepted,
    Dropped,
    Pskip,
}

fn put_bits(bits: &mut Vec<bool>, value: u32, width: u32) {
    for shift in (0..width).rev() {
        bits.push((value >> shift) & 1 != 0);
    }
}

fn put_ue(bits: &mut Vec<bool>, value: u32) {
    let code = u64::from(value) + 1;
    let width = 64 - code.leading_zeros();
    for _ in 1..width {
        bits.push(false);
    }
    for shift in (0..width).rev() {
        bits.push((code >> shift) & 1 != 0);
    }
}

fn put_se(bits: &mut Vec<bool>, value: i32) {
    let mapped = if value > 0 {
        (value as u32) * 2 - 1
    } else {
        (-value as u32) * 2
    };
    put_ue(bits, mapped);
}

/// The arithmetic coder half of `h264e_slice_write_pskip`.
///
/// The C encoder deliberately carries its own compact queue implementation
/// rather than using the general-purpose CABAC type. A skip-only slice has a
/// single context and every bin is one, so keeping that exact state machine
/// here is both small and removes any chance that trailing-bit conventions
/// diverge from MPP.
struct SkipCabac {
    context: Context,
    low: i32,
    range: i32,
    queue: i32,
    outstanding: i32,
    bits: Vec<bool>,
}

impl SkipCabac {
    fn new(qp: u8, bits: Vec<bool>) -> Self {
        let state = 23 * i32::from(qp) / 16 + 33;
        let state = state.clamp(1, 126) as u8;
        Self {
            context: Context::new(63 - state.min(127 - state), state >> 6),
            low: 0,
            range: 0x01fe,
            queue: -9,
            outstanding: 0,
            bits,
        }
    }

    fn put_bits(&mut self, value: i32, width: u32) {
        for shift in (0..width).rev() {
            self.bits.push((value >> shift) & 1 != 0);
        }
    }

    fn put_byte(&mut self) {
        if self.queue < 0 {
            return;
        }
        let out = self.low >> (self.queue + 10);
        self.low &= (0x400 << self.queue) - 1;
        self.queue -= 8;

        if (out & 0xff) == 0xff {
            self.outstanding += 1;
            return;
        }

        let carry = out >> 8;
        if self.queue > 0 {
            self.put_bits(carry, (self.queue & 7) as u32);
        }
        while self.outstanding > 0 {
            self.put_bits(carry - 1, 8);
            self.outstanding -= 1;
        }
        self.put_bits(out, 8.min(8 - self.queue) as u32);
    }

    fn renorm(&mut self) {
        while self.range < 256 {
            self.range <<= 1;
            self.low <<= 1;
            self.queue += 1;
        }
        self.put_byte();
    }

    fn skip_flag(&mut self) {
        let lps = self.context.lps_range(self.range as u32) as i32;
        self.range -= lps;
        let lps_bin = self.context.mps == 0;
        if lps_bin {
            self.low += self.range;
            self.range = lps;
        }
        self.context.update(lps_bin);
        self.renorm();
    }

    fn terminal(&mut self) {
        self.range -= 2;
        self.renorm();
    }

    fn finish(mut self) -> Vec<bool> {
        self.low += self.range - 2;
        self.low |= 1;
        self.low <<= 9;
        self.queue += 9;
        self.put_byte();
        self.put_byte();

        self.low <<= -self.queue;
        self.low |= 1 << 10;
        self.queue = 0;
        self.put_byte();
        while self.outstanding > 0 {
            self.put_bits(0xff, 8);
            self.outstanding -= 1;
        }
        self.bits
    }
}

fn write_skip_cabac(bits: Vec<bool>, qp: u8, mb_count: usize) -> Vec<bool> {
    let mut cabac = SkipCabac::new(qp, bits);
    for index in 0..mb_count {
        if index != 0 {
            cabac.terminal();
        }
        cabac.skip_flag();
    }
    cabac.finish()
}

/// `h264e_slice_write_pskip`: a non-reference P slice covering the whole
/// picture. CAVLC writes one skip run; CABAC writes one skip flag per
/// macroblock, separated by `end_of_slice_flag = 0`, then flushes the final
/// terminating bin.
fn pskip_nal(config: &EncoderConfig, state: &FrameState) -> Result<Vec<u8>, String> {
    let mut bits = Vec::new();

    put_ue(&mut bits, 0); // first_mb_in_slice
    put_ue(&mut bits, 0); // P slice
    put_ue(&mut bits, 0); // pps_id
    put_bits(
        &mut bits,
        state.frame_num & ((1 << (config.log2_max_frame_num_minus4 + 4)) - 1),
        config.log2_max_frame_num_minus4 as u32 + 4,
    );
    put_bits(
        &mut bits,
        (state.poc as u32) & ((1 << (config.log2_max_poc_lsb_minus4 + 4)) - 1),
        config.log2_max_poc_lsb_minus4 as u32 + 4,
    );
    put_bits(&mut bits, 0, 1); // num_ref_idx_active_override_flag
    put_bits(&mut bits, 0, 1); // ref_pic_list_modification_flag_l0
    if config.entropy_coding {
        put_ue(&mut bits, 0); // cabac_init_idc
    }
    put_se(&mut bits, 0); // slice_qp_delta
    put_ue(&mut bits, 0); // disable_deblocking_filter_idc
    put_se(&mut bits, 0); // slice_alpha_c0_offset_div2
    put_se(&mut bits, 0); // slice_beta_offset_div2

    let mb_count = config.coded_width() / 16 * config.coded_height() / 16;
    if config.entropy_coding {
        while bits.len() % 8 != 0 {
            bits.push(true); // cabac_alignment_one_bit
        }
        bits = write_skip_cabac(bits, config.qp_init, mb_count);
    } else {
        put_ue(&mut bits, mb_count as u32);
        bits.push(true); // rbsp_stop_one_bit
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
    }

    let mut rbsp = Vec::with_capacity(bits.len() / 8);
    for chunk in bits.chunks(8) {
        rbsp.push(
            chunk
                .iter()
                .fold(0u8, |byte, bit| (byte << 1) | u8::from(*bit)),
        );
    }
    let mut escaped = Vec::with_capacity(rbsp.len() + 4);
    let mut zeroes = 0;
    for byte in rbsp {
        if zeroes >= 2 && byte <= 3 {
            escaped.push(3);
            zeroes = 0;
        }
        escaped.push(byte);
        zeroes = if byte == 0 { zeroes + 1 } else { 0 };
    }
    let mut nal = vec![0, 0, 0, 1, 0x01];
    if nal.len() + escaped.len() < 8 {
        return Err("software PSKIP produced an invalid NAL".into());
    }
    nal.extend_from_slice(&escaped);
    Ok(nal)
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

/// The tune state of a session that has seen nothing, shared by the register
/// builders in the test modules: every byte-verified capture is such a session.
#[cfg(test)]
fn quiet_tune() -> &'static SceneTune {
    static TUNE: std::sync::OnceLock<SceneTune> = std::sync::OnceLock::new();
    TUNE.get_or_init(|| SceneTune::new(false))
}

/// `scene_motion_flag` for a session that has not detected a scene change:
/// `ap_motion_flag * 2 + curr_scene_motion_flag`, where `ap_motion_flag` is the
/// scene mode (1 for the default, 0 for IPC) and the current-motion flag is the
/// detector's output. [`SceneTune`] runs the detector; until it raises the
/// current flag this is the value the recorded C images carry, and the register
/// patch stays on the rows the template was captured with.
pub const SCENE_MOTION_FLAG_DEFAULT: usize = 2;

/// Window lengths of `vepu580_tune.h`; the comment in `hal_h264e_vepu580_tune.c`
/// says `FRAME_CONTENT_ANALYSIS_NUM >= 5`, `MD_WIN_LEN >= 3` and
/// `MD_SHOW_LEN == 4`, and the reference build ships 5/5/4.
const MD_WIN_LEN: usize = 5;
const MD_FLAG_MATRIX_LEN: usize = 4;
const SCENE_FLAG_MATRIX_LEN: usize = 5;

/// `MotionLevel` of `vepu580_tune.h`.
const MOTION_STILL: i32 = 0;
const MOTION_LEVEL_MOTION: i32 = 1;
const MOTION_LEVEL_BIG: i32 = 2;
const MOTION_LEVEL_UNKNOWN: i32 = 3;

/// `mb_avg_madp_thd`: the per-mean-madp gates, three rows for the default scene
/// mode and the same three again for IPC. The rows are identical in pairs, but
/// the base index moves between the modes, so they stay spelled out.
const MB_AVG_MADP_THD: [i32; 6] = [192, 128, 64, 192, 128, 64];

/// `ctu_madp_cnt_thd` of `common/vepu580_tune.c`, the eight per-row count
/// thresholds the motion level is decided against.
const CTU_MADP_CNT_THD: [[i32; 8]; 6] = [
    [50, 100, 130, 50, 100, 550, 500, 550],
    [100, 150, 200, 80, 120, 500, 450, 550],
    [150, 200, 250, 100, 150, 450, 400, 450],
    [50, 100, 130, 50, 100, 550, 500, 550],
    [100, 150, 200, 80, 120, 500, 450, 550],
    [150, 200, 250, 100, 150, 450, 400, 450],
];

/// `madp_num_map`: how the four `md_sad_b16num` histograms are folded into the
/// five count windows the level rules read. Row 4 - the all-ones row - is the
/// "anything at all happened" test the level decision gates on.
const MADP_NUM_MAP: [[i32; 4]; 5] = [
    [0, 0, 0, 1],
    [0, 0, 1, 0],
    [0, 0, 1, 1],
    [1, 0, 0, 0],
    [1, 1, 1, 1],
];

/// `intra_atf_wgt` and `skip_atf_wgt` of `hal_h264e_vepu580_tune.c`, re-written
/// by the register patch on the `scene_motion_flag` row. Row 2 - the default
/// scene mode without a detected change - is what every captured image carries;
/// the skip row is all sixteens in all four rows.
const INTRA_ATF_WEIGHTS: [[u32; 12]; 4] = [
    [24, 22, 21, 22, 21, 20, 20, 19, 18, 16, 16, 16],
    [16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16],
    [22, 21, 20, 21, 20, 19, 20, 19, 18, 16, 16, 16],
    [16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16],
];
const SKIP_ATF_WEIGHTS: [[u32; 13]; 4] = [[16; 13], [16; 13], [16; 13], [16; 13]];

/// `HalH264eVepu580Tune` of `hal_h264e_vepu580_tune.c`: the per-session state of
/// `vepu580_h264e_tune_stat_update`, fed by the STATUS window of each finished
/// picture and read by the register patch of the next one.
///
/// The rule the state machine implements: per picture, fold the `md_sad_b16num`
/// histograms into five count windows, compare the mean inter difference
/// against a row of `mb_avg_madp_thd` and the windows against scaled row
/// thresholds to get a motion level, score a sliding window of the last five
/// levels, and raise the current-motion flag when a majority of the last four
/// scored pictures flagged large motion. Three consecutive large-motion pictures
/// (`md_flag_matrix[0..3]` all set) pin the flag at 1 outright - that is the
/// "hard cut has happened" case the HANDOFF describes.
///
/// The `txtr_madi` array of the C exists but is never written or read there; it
/// is left out rather than carried as decoration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneTune {
    /// `scene_mode == MPP_ENC_SCENE_MODE_IPC`, which flips `ap_motion_flag`
    /// from the default scene mode's 1 to 0.
    ipc_scene_mode: bool,
    curr_scene_motion_flag: i32,
    ap_motion_flag: i32,
    md_madp: [i32; MD_WIN_LEN],
    md_flag_matrix: [i32; MD_FLAG_MATRIX_LEN],
    scene_motion_flag_matrix: [i32; SCENE_FLAG_MATRIX_LEN],
    pre_madp: [i32; 2],
    pre_madi: [i32; 2],
}

impl SceneTune {
    pub fn new(ipc_scene_mode: bool) -> SceneTune {
        SceneTune {
            ipc_scene_mode,
            curr_scene_motion_flag: 0,
            ap_motion_flag: if ipc_scene_mode { 0 } else { 1 },
            md_madp: [0; MD_WIN_LEN],
            md_flag_matrix: [0; MD_FLAG_MATRIX_LEN],
            scene_motion_flag_matrix: [0; SCENE_FLAG_MATRIX_LEN],
            // -1 until the first picture, exactly as `tune_init` leaves them.
            pre_madp: [-1, -1],
            pre_madi: [-1, -1],
        }
    }

    /// `curr_scene_motion_flag`, as the register patch reads it.
    pub fn curr_scene_motion_flag(&self) -> i32 {
        self.curr_scene_motion_flag
    }

    /// `scene_motion_flag = ap_motion_flag * 2 + curr_scene_motion_flag`, the
    /// row selector for the weight and multiplier tables.
    pub fn scene_motion_flag(&self) -> usize {
        (self.ap_motion_flag * 2 + self.curr_scene_motion_flag) as usize
    }

    /// `vepu580_h264e_tune_stat_update`, driven by the STATUS window the
    /// finished picture wrote. The statistics arrive exactly as the C reads them
    /// out of `regs->reg_st` - same fields, same divisors, same zero guards.
    pub fn update(&mut self, stats: &Statistics) {
        self.ap_motion_flag = if self.ipc_scene_mode { 0 } else { 1 };
        let mut cnt = [0i64; 5];
        for (i, window) in MADP_NUM_MAP.iter().enumerate() {
            for (k, weight) in window.iter().enumerate() {
                cnt[i] += i64::from(stats.md_sad_b16num[k]) * i64::from(*weight);
            }
        }

        self.pre_madi[0] = if stats.num_b16 == 0 {
            0
        } else {
            (stats.madi / stats.num_b16) as i32
        };
        self.pre_madp[0] = if stats.num_ctu == 0 {
            0
        } else {
            (stats.madp / stats.num_ctu) as i32
        };

        let mut mb_num: i64 = stats.madi_b16num.iter().map(|v| i64::from(*v)).sum();
        if mb_num == 0 {
            mb_num = 1;
        }
        let mvbit = if self.ap_motion_flag != 0 { 15 } else { 10 };

        let mut madp = MOTION_STILL;
        if cnt[4] != 0 {
            let mut base = (self.ap_motion_flag * 3) as usize;
            for _ in 0..3 {
                if self.pre_madp[0] >= MB_AVG_MADP_THD[base] {
                    let thd = CTU_MADP_CNT_THD[base];
                    let scaled = |i: usize| (mb_num * i64::from(thd[i])) >> mvbit;
                    if cnt[0] > scaled(0) || cnt[1] > scaled(1) || cnt[2] > scaled(2) {
                        madp = MOTION_LEVEL_BIG;
                    } else if ((cnt[0] > scaled(3) || cnt[1] > scaled(4)) && cnt[3] < scaled(5))
                        || cnt[3] < scaled(6)
                    {
                        // The C nests the third `cnt[3] <` rule in an `else if`
                        // of its own; collapsing it into the second rule's
                        // `||` as written here keeps the semantics only because
                        // the second rule's conclusion is the same - both
                        // branches mean large motion.
                        madp = MOTION_LEVEL_BIG;
                    } else if cnt[3] < scaled(7) {
                        madp = MOTION_LEVEL_MOTION;
                    }
                    break;
                }
                base += 1;
            }
        } else {
            madp = MOTION_LEVEL_UNKNOWN;
        }

        let mut score_t = ((MD_WIN_LEN as i32 - 2) * 6 + 2 * 8 + 2 * 11 + 2 * 13) / 2;
        let mut n_score = if madp != MOTION_LEVEL_UNKNOWN {
            madp * 13 + self.md_madp[0] * 11 + self.md_madp[1] * 8
        } else {
            score_t -= 13;
            self.md_madp[0] * 11 + self.md_madp[1] * 8
        };
        n_score += self.md_madp[2..].iter().map(|v| v * 6).sum::<i32>();
        let md_flag = if n_score >= score_t { 1 } else { 0 };

        self.curr_scene_motion_flag = 0;
        if self.md_flag_matrix[0] != 0 && self.md_flag_matrix[1] != 0 && self.md_flag_matrix[2] != 0
        {
            self.curr_scene_motion_flag = 1;
        } else if (self.md_flag_matrix[0] != 0 && self.md_flag_matrix[1] != 0)
            || (self.md_flag_matrix[1] != 0
                && self.md_flag_matrix[2] != 0
                && self.md_flag_matrix[3] != 0)
        {
            self.curr_scene_motion_flag = md_flag;
        }

        if madp != MOTION_LEVEL_UNKNOWN {
            for j in (0..MD_WIN_LEN - 1).rev() {
                self.md_madp[j + 1] = self.md_madp[j];
            }
            self.md_madp[0] = madp;
        }
        for j in (0..MD_FLAG_MATRIX_LEN - 1).rev() {
            self.md_flag_matrix[j + 1] = self.md_flag_matrix[j];
        }
        self.md_flag_matrix[0] = md_flag;
        for j in (0..SCENE_FLAG_MATRIX_LEN - 1).rev() {
            self.scene_motion_flag_matrix[j + 1] = self.scene_motion_flag_matrix[j];
        }
        self.scene_motion_flag_matrix[0] = self.curr_scene_motion_flag;
        self.pre_madi[1] = self.pre_madi[0];
        self.pre_madp[1] = self.pre_madp[0];
    }
}

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
    /// reg4165..reg4167, the I16/I8/I4 block counts used by debreath.
    pub pnum_i16: u32,
    pub pnum_i8: u32,
    pub pnum_i4: u32,
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
            pnum_i16: at(4165) & 0x1f_ffff,
            pnum_i8: at(4166) & 0x7f_ffff,
            pnum_i4: at(4167) & 0x7f_ffff,
        }
    }

    fn iblk4_prop(&self, mbs: u32) -> i32 {
        if mbs == 0 {
            0
        } else {
            (self.pnum_i4 + self.pnum_i8 + self.pnum_i16).wrapping_mul(256) as i32 / mbs as i32
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
    if (job.save_pass1 || job.use_pass1) && job.pass1_fd < 0 {
        return Err("H.264 two-pass job has no pass1 dma-buf fd".into());
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
        // The FBC body offset has to be the offset the surface was built with.
        RegOffsetInfo {
            reg_idx: REG_RFPW_B,
            offset: fbc_header_size(config)? as u32,
        },
        RegOffsetInfo {
            reg_idx: REG_RFPR_B,
            offset: fbc_header_size(config)? as u32,
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
        rcb: Vec::new(),
    };
    // The first piece of the register file that stops being a recording: the QP
    // words of `setup_vepu580_rc_base`. Everything else is still the captured
    // image, and `validate_reference_config` refuses what it depends on.
    match job.rc {
        Some(rc) => file.apply_rate_control(config, job.state.is_idr, &rc)?,
        None => file.apply_fixed_qp(u32::from(config.qp_init))?,
    }
    file.apply_profile(config)?;
    file.apply_deblock(config)?;
    file.apply_picture(job)?;
    file.apply_roi(job)?;
    // `setup_vepu580_split` runs after the ROI and the recn/reference setup,
    // before the ME block; `setup_vepu580_intra_refresh` runs after the ME and
    // overwrites the CIME search fields it leaves; `setup_vepu580_ext_line_buf`
    // is the last register pass before the tune patch. The order is kept so a
    // refresh frame's `cme_srch_v` = 1 overwrites `calc_cime_parameter` exactly
    // where the C's does.
    file.apply_split(config)?;
    file.apply_gdr(job)?;
    // `setup_vepu580_ext_line_buf`: widths above 4096 coded pixels feed the
    // CIME line store from an extension buffer, top and bottom halves of one
    // allocation. Both registers carry the same fd and the offset table carries
    // the second half's start; the RCB sizes belong to the submit batch. The
    // no-buffer branch of the C leaves both registers at the zero the register
    // set was cleared to, which is what the recorded image carries.
    if job.ext_fd >= 0 && job.ext_offset > 0 {
        file.base[182 - 160] = job.ext_fd as u32;
        file.base[183 - 160] = job.ext_fd as u32;
        file.offsets.push(RegOffsetInfo {
            reg_idx: 182,
            offset: job.ext_offset,
        });
        // The two `MPP_DEV_RCB_INFO` ioctls of the C - the whole buffer size
        // for the reg183 half, zero for reg182's own - ride the same batch.
        file.rcb = vec![
            crate::service::RcbInfo {
                reg_idx: 183,
                size: job.ext_offset,
            },
            crate::service::RcbInfo {
                reg_idx: 182,
                size: 0,
            },
        ];
    }
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
    file.apply_pass1(job)?;
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
    /// inter ATR weights and the search multipliers from the `scene_motion_flag`
    /// row, plus the harder intra cost thresholds, the 341 quant bias and the
    /// flattened search limits whenever the detector has flagged a scene change.
    ///
    /// While the flag is at its no-change default the recorded image already
    /// carries every row this pass would write - the capture *is* a run through
    /// this patch - so those writes are kept out of the quiet path and pinned by
    /// `tune_default_rows_match_the_captured_image` instead of being repeated
    /// from the tables. The moment the detector moves, the writes happen.
    /// `vepu580_h264e_save_pass1_patch` and
    /// `vepu580_h264e_use_pass1_patch`.
    fn apply_pass1(&mut self, job: &Job<'_>) -> Result<(), String> {
        let config = job.config;
        let width = align(config.width, 64);
        let height = align(config.height, 16);
        let luma = width * height;

        if job.save_pass1 {
            let fd = job.pass1_fd as u32;
            self.set(&vepu580_regs::BASE_ENC_PIC_CUR_FRM_REF, 1)?;
            self.base[BASE_RFPW_H] = fd;
            self.base[BASE_RFPW_B] = fd;
            self.set(&vepu580_regs::BASE_ENC_PIC_REC_FBC_DIS, 1)?;
            if let Some(offset) = self.offsets.iter_mut().find(|o| o.reg_idx == REG_RFPW_B) {
                offset.offset = luma as u32;
            }
            self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT, 0)?;
            self.set(&vepu580_regs::BASE_ENC_PIC_SLEN_FIFO, 0)?;
        }

        if job.use_pass1 {
            let fd = job.pass1_fd as u32;
            self.set(&vepu580_regs::BASE_SRC_FMT_SRC_CFMT, 6)?;
            self.set(&vepu580_regs::BASE_SRC_FMT_ALPHA_SWAP, 0)?;
            self.set(&vepu580_regs::BASE_SRC_FMT_RBUV_SWAP, 0)?;
            self.set(&vepu580_regs::BASE_SRC_FMT_OUT_FMT, 1)?;
            self.set(&vepu580_regs::BASE_SRC_PROC_AFBCD_EN, 0)?;
            self.set(&vepu580_regs::BASE_SRC_STRD0_SRC_STRD0, width as u32)?;
            self.set(&vepu580_regs::BASE_SRC_STRD1_SRC_STRD1, width as u32)?;
            self.set(&vepu580_regs::BASE_SRC_PROC_SRC_MIRR, 0)?;
            self.set(&vepu580_regs::BASE_SRC_PROC_SRC_ROT, 0)?;
            self.set(&vepu580_regs::BASE_SRC_PROC_TXA_EN, 0)?;
            self.base[BASE_ADR_SRC0] = fd;
            self.base[BASE_ADR_SRC1] = fd;
            self.base[BASE_ADR_SRC2] = fd;
            for reg_idx in [REG_SRC_1_OFFSET, REG_SRC_2_OFFSET] {
                if let Some(offset) = self.offsets.iter_mut().find(|o| o.reg_idx == reg_idx) {
                    offset.offset = luma as u32;
                }
            }
        }
        Ok(())
    }

    fn apply_tune(&mut self, job: &Job<'_>) -> Result<(), String> {
        let tune = job.tune;
        let curr = tune.curr_scene_motion_flag() != 0;
        let flag = tune.scene_motion_flag();
        if !job.state.is_idr {
            // `src = (curr || SMTRC) ? &klut_weight[0] : &klut_weight[4]`; this
            // driver never runs SMTRC, so only the current-motion flag moves it.
            // `apply_picture` picked offset four, the no-change row.
            if curr {
                let first = vepu580_regs::RCKLUT_KLUT_WGT0_CHRM_KLUT_WGT0.word;
                for index in 0..CHROMA_KLUT_WORDS {
                    let word = self.rc_klut_word_mut(first + index)?;
                    *word = CHROMA_KLUT_WEIGHT[index];
                }
            }
            self.set_atr_weights(&ATR_WEIGHTS[flag])?;
        }
        if curr {
            self.set_param(
                &vepu580_regs::S3_LVL16_INTRA_UL_CST_THD_LVL16_INTRA_UL_CST_THLD,
                2501,
            )?;
            self.set_param(&vepu580_regs::S3_RDO_QUANT_QUANT_F_BIAS_P, 341)?;
            self.set(&vepu580_regs::H264_IPRD_CSTS_VTHD_Y, 0)?;
            self.set(&vepu580_regs::H264_IPRD_CSTS_VTHD_C, 0)?;
            self.set(&vepu580_regs::RCKLUT_KLUT_OFST_CHRM_KLUT_OFST, 0)?;
            self.set(&vepu580_regs::H264_RDO_CFG_ATF_INTRA_E, 0)?;
            self.set(&vepu580_regs::RDO_RDO_SQI_CFG_ATF_PSKIP_EN, 0)?;
            self.set_param(&vepu580_regs::S3_ATR_THD1_ATR_THDQP, 51)?;
            self.set_param(&vepu580_regs::S3_CIME_SQI_CFG_CIME_PMV_SET_ZERO, 0)?;
            self.set_param(&vepu580_regs::S3_RIME_SQI_THD_CIME_SAD_TH0, 0)?;
            self.set_param(&vepu580_regs::S3_FME_SQI_THD0_CIME_SAD_PU16_TH, 0)?;
            self.set_param(&vepu580_regs::S3_FME_SQI_THD1_MOVE_LAMBDA, 8)?;
        }
        // The reference patch writes every one of these rows on every picture,
        // regardless of whether the detector moved. Generating the quiet row as
        // well keeps the captured image from acting as a hidden input.
        self.set(
            &vepu580_regs::RDO_RDO_INTRA_CIME_THD0_ATF_RDO_INTRA_CIME_THD0,
            28,
        )?;
        self.set(
            &vepu580_regs::RDO_RDO_INTRA_CIME_THD0_ATF_RDO_INTRA_CIME_THD1,
            44,
        )?;
        self.set(
            &vepu580_regs::RDO_RDO_INTRA_CIME_THD1_ATF_RDO_INTRA_CIME_THD2,
            72,
        )?;
        self.set(
            &vepu580_regs::RDO_RDO_SKIP_CIME_THD0_ATF_RDO_SKIP_CIME_THD0,
            10,
        )?;
        self.set(
            &vepu580_regs::RDO_RDO_SKIP_CIME_THD0_ATF_RDO_SKIP_CIME_THD1,
            8,
        )?;
        self.set(
            &vepu580_regs::RDO_RDO_SKIP_CIME_THD1_ATF_RDO_SKIP_CIME_THD2,
            15,
        )?;
        self.set(
            &vepu580_regs::RDO_RDO_SKIP_CIME_THD1_ATF_RDO_SKIP_CIME_THD3,
            25,
        )?;
        let intra_fields = [
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT0_ATF_RDO_INTRA_WGT00,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT0_ATF_RDO_INTRA_WGT01,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT0_ATF_RDO_INTRA_WGT02,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT1_ATF_RDO_INTRA_WGT10,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT1_ATF_RDO_INTRA_WGT11,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT1_ATF_RDO_INTRA_WGT12,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT2_ATF_RDO_INTRA_WGT20,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT2_ATF_RDO_INTRA_WGT21,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT2_ATF_RDO_INTRA_WGT22,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT3_ATF_RDO_INTRA_WGT30,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT3_ATF_RDO_INTRA_WGT31,
            &vepu580_regs::RDO_RDO_INTRA_ATF_WGT3_ATF_RDO_INTRA_WGT32,
        ];
        for (field, value) in intra_fields.iter().zip(INTRA_ATF_WEIGHTS[flag]) {
            self.set(field, value)?;
        }
        let skip_fields = [
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT0_ATF_RDO_SKIP_ATF_WGT00,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT0_ATF_RDO_SKIP_ATF_WGT10,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT0_ATF_RDO_SKIP_ATF_WGT11,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT0_ATF_RDO_SKIP_ATF_WGT12,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT1_ATF_RDO_SKIP_ATF_WGT20,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT1_ATF_RDO_SKIP_ATF_WGT21,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT1_ATF_RDO_SKIP_ATF_WGT22,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT2_ATF_RDO_SKIP_ATF_WGT30,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT2_ATF_RDO_SKIP_ATF_WGT31,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT2_ATF_RDO_SKIP_ATF_WGT32,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT3_ATF_RDO_SKIP_ATF_WGT40,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT3_ATF_RDO_SKIP_ATF_WGT41,
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT3_ATF_RDO_SKIP_ATF_WGT42,
        ];
        for (field, value) in skip_fields.iter().zip(SKIP_ATF_WEIGHTS[flag]) {
            self.set(field, value)?;
        }
        let cime = &CIME_MULTI[flag];
        self.set_param(&vepu580_regs::S3_CIME_SQI_MULTI0_CIME_MULTI0, cime[0])?;
        self.set_param(&vepu580_regs::S3_CIME_SQI_MULTI0_CIME_MULTI1, cime[1])?;
        self.set_param(&vepu580_regs::S3_CIME_SQI_MULTI1_CIME_MULTI2, cime[2])?;
        self.set_param(&vepu580_regs::S3_CIME_SQI_MULTI1_CIME_MULTI3, cime[3])?;
        let rime = &RIME_MULTI[flag];
        self.set_param(&vepu580_regs::S3_RIME_SQI_MULTI_RIME_MULTI0, rime[0])?;
        self.set_param(&vepu580_regs::S3_RIME_SQI_MULTI_RIME_MULTI1, rime[1])?;
        self.set_param(&vepu580_regs::S3_RIME_SQI_MULTI_RIME_MULTI2, rime[2])?;
        Ok(())
    }
    /// `setup_vepu580_roi`, which runs whenever the task carries ROI data:
    /// the base table's fd into `roi_addr` with `enc_pic.roi_en` set, and - the
    /// reference's `ROI_TYPE_2` helper always turns QP adjustment on - the qp
    /// table's fd into `roi_qp_addr` with `roi_en.roi_qp_en`. The enable checks
    /// compare the buffer sizes against `mb_w * mb_h * 8` with `mb_w` aligned
    /// to 64 pixels, which the tables built here always satisfy; the guard is
    /// kept because a table that failed it would silently disable the feature
    /// in the reference, and this driver must not diverge on the same input.
    fn apply_roi(&mut self, job: &Job<'_>) -> Result<(), String> {
        if job.roi_fd < 0 {
            return Ok(());
        }
        let (mb_w, mb_h, stride_h, stride_v) = roi_grid(job.config);
        let stride_blocks = (stride_h * stride_v) as i64;
        let checked_w = (align(job.config.width, 64) / 16) as i64;
        let checked_h = (align(job.config.height, 16) / 16) as i64;
        let _ = mb_w;
        let _ = mb_h;
        // The tables are `stride_blocks` records long; the reference enables
        // only if that many bytes-times-eight reaches its own product.
        if stride_blocks * 8 < checked_w * checked_h * 8
            || stride_blocks * 2 < checked_w * checked_h * 2
        {
            return Err("ROI tables smaller than the encoder's enable check".into());
        }
        self.set(&vepu580_regs::BASE_ENC_PIC_ROI_EN, 1)?;
        let base_index = 178 - 160;
        self.base[base_index] = job.roi_fd as u32;
        self.set(&vepu580_regs::H264_ROI_EN_ROI_QP_EN, 1)?;
        self.base[179 - 160] = job.roi_qp_fd as u32;
        Ok(())
    }

    /// `setup_vepu580_split`. With `split_out = 0` the hardware lays every
    /// slice down contiguously and one STATUS length covers the picture. The
    /// low-delay output bit arms `slen_fifo`; the submission path then drains
    /// slice lengths through `POLL_HW_IRQ` instead of the normal completion
    /// poll.
    ///
    /// The two enabled branches are transcribed statement by statement, the
    /// magic numbers included: `sli_max_num_m1 = 500` and `sli_flsh = 1` appear
    /// in both, `slc_done_en` is unconditional for `BY_BYTE` and threshold-gated
    /// for `BY_CTU` at `VEPU580_SLICE_FIFO_LEN = 32` slices, and `sli_splt_cpst`
    /// stays 0 in every branch.
    fn apply_split(&mut self, config: &EncoderConfig) -> Result<(), String> {
        // `MPP_ENC_SPLIT_NONE` writes zeros to all eight fields; the recorded
        // image the file starts from carries exactly that (asserted by
        // `split_none_writes_only_what_the_recorded_image_already_has`), so the
        // zeros are written to keep the branch honest rather than to change the
        // quiet picture's bytes.
        self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT, 0)?;
        self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT_MODE, 0)?;
        self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT_CPST, 0)?;
        self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_MAX_NUM_M1, 0)?;
        self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_FLSH, 0)?;
        self.set(&vepu580_regs::BASE_SLI_CNUM_SLI_SPLT_CNUM_M1, 0)?;
        self.set(&vepu580_regs::BASE_SLI_BYTE_SLI_SPLT_BYTE, 0)?;
        self.set(&vepu580_regs::BASE_ENC_PIC_SLEN_FIFO, 0)?;
        self.set(&vepu580_regs::CTL_INT_EN_SLC_DONE_EN, 0)?;
        match config.split_mode {
            SplitMode::None => {}
            SplitMode::ByByte => {
                self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT, 1)?;
                self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_MAX_NUM_M1, 500)?;
                self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_FLSH, 1)?;
                self.set(&vepu580_regs::BASE_SLI_BYTE_SLI_SPLT_BYTE, config.split_arg)?;
                let split_out = config.split_out != 0;
                self.set(&vepu580_regs::BASE_ENC_PIC_SLEN_FIFO, split_out as u32)?;
                self.set(&vepu580_regs::CTL_INT_EN_SLC_DONE_EN, 1)?;
            }
            SplitMode::ByCtu => {
                let mb_w = (config.coded_width() / 16) as u32;
                let mb_h = (config.coded_height() / 16) as u32;
                self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT, 1)?;
                self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT_MODE, 1)?;
                self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_MAX_NUM_M1, 500)?;
                self.set(&vepu580_regs::BASE_SLI_SPLT_SLI_FLSH, 1)?;
                self.set(
                    &vepu580_regs::BASE_SLI_CNUM_SLI_SPLT_CNUM_M1,
                    config.split_arg - 1,
                )?;
                // `slice_num > VEPU580_SLICE_FIFO_LEN` arms the slice interrupt
                // so the length FIFO cannot fill while nobody drains it.
                let slice_num = (mb_w * mb_h + config.split_arg - 1) / config.split_arg;
                let split_out = config.split_out != 0;
                self.set(&vepu580_regs::BASE_ENC_PIC_SLEN_FIFO, split_out as u32)?;
                if split_out || slice_num > 32 {
                    self.set(&vepu580_regs::CTL_INT_EN_SLC_DONE_EN, 1)?;
                }
            }
        }
        Ok(())
    }

    /// `setup_vepu580_intra_refresh`, on a picture the frame schedule flagged
    /// `is_i_refresh`. It turns ROI on over its own force-intra table (`roi_en`
    /// and `roi_addr`, *without* `roi_qp_en` - the QP table stays whatever the
    /// ROI pass left it), and it clamps the CIME search window on the refresh
    /// axis to 1: `cme_srch_v = 1` for row refresh, `cme_srch_h = 1` for column
    /// refresh, written after `calc_cime_parameter` set the field to the
    /// picture-derived value.
    fn apply_gdr(&mut self, job: &Job<'_>) -> Result<(), String> {
        if job.gdr_fd < 0 || !job.state.is_i_refresh {
            return Ok(());
        }
        self.set(&vepu580_regs::BASE_ENC_PIC_ROI_EN, 1)?;
        self.base[178 - 160] = job.gdr_fd as u32;
        match job.config.gdr_mode {
            GdrMode::Row => self.set(&vepu580_regs::BASE_ME_RNGE_CME_SRCH_V, 1)?,
            GdrMode::Col => self.set(&vepu580_regs::BASE_ME_RNGE_CME_SRCH_H, 1)?,
        }
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
        // A smart-GOP VI picture reorders reference list 0 so its retained IDR
        // becomes index zero. The traced job carries `rodr_pic_idx=2` and
        // `ref_list0_rodr=1` in `synt_sli2`.
        self.set(
            &vepu580_regs::H264_SYNT_SLI2_RODR_PIC_IDX,
            if state.is_vi { 2 } else { 0 },
        )?;
        self.set(
            &vepu580_regs::H264_SYNT_SLI2_REF_LIST0_RODR,
            u32::from(state.is_vi),
        )?;
        // The smart reference schedule marks the opening IDR long-term. This is
        // the IDR's `long_term_reference_flag` in dec_ref_pic_marking.
        self.set(
            &vepu580_regs::H264_SYNT_REFM0_LTRF_FLG,
            u32::from(state.is_idr && (config.smart_p_vgop().is_some() || job.current_long_term)),
        )?;
        // `setup_vepu580_codec` takes the NAL header from the slice: an I slice
        // is type 5 with nal_ref_idc 3, a P slice is type 1 with 2. Without this
        // the hardware stamps every P picture as an IDR.
        self.set(
            &vepu580_regs::H264_SYNT_NAL_NAL_REF_IDC,
            if state.is_idr {
                3
            } else if job.non_reference {
                0
            } else if job.save_pass1 {
                1
            } else {
                2
            },
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
        // `setup_vepu580_me` bounds the motion-vector range by the level - the
        // raised one, because the HAL reads the SPS the syntax layer has derived.
        let level_idc = config.effective_level_idc();
        self.set(
            &vepu580_regs::H264_ME_CFG_MV_LIMIT,
            if level_idc > 20 {
                2
            } else if level_idc >= 11 {
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

    base[BASE_ME_RNGE] = build_me_range(width_mbs, height_mbs, config.effective_level_idc())?;
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

fn build_me_range(width_mbs: usize, height_mbs: usize, level_idc: u8) -> Result<u32, String> {
    // `calc_cime_parameter` step 1: the level caps the vertical search before the
    // picture size caps it. Levels 1.0 and 1-b get a 12-block window; every level
    // from 1.1 up gets the full 28, which is why this only shows a difference at
    // the two smallest levels - and why `cme_srch_v` is 3 rather than 7 there.
    let mut search_blocks_w = 44usize;
    let mut search_blocks_h = if level_idc <= 10 { 12 } else { 28 };

    // Step 1's own clamp by the hardware's CIME window (176x112 quarters).
    if 176 < search_blocks_w * 4 {
        search_blocks_w = 176 / 4;
    }
    if 112 < search_blocks_h * 4 {
        search_blocks_h = 112 / 4;
    }

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
pub(crate) fn rc_model_config(config: &EncoderConfig) -> Result<rc_model::Config, String> {
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
        quality_min: config.rc_qp_min as i32,
        quality_max: config.rc_qp_max as i32,
        i_quality_delta: 2,
        total_mb: (config.coded_width() / 16) as i32 * (config.coded_height() / 16) as i32,
        mode: match config.rc_mode {
            RcMode::Vbr => rc_model::Mode::Vbr,
            RcMode::Avbr => rc_model::Mode::Avbr,
            _ => rc_model::Mode::Cbr,
        },
        smart_p_vgop: config.smart_p_vgop().unwrap_or(0) as i32,
        vi_quality_delta: if config.smart_p_vgop().is_some() {
            2
        } else {
            0
        },
        super_mode: config.super_mode,
        super_i_thd: config.super_i_thd,
        super_p_thd: config.super_p_thd,
        rc_priority: config.rc_priority,
        debreath_en: config.debreath_en,
        debreath_strength: config.debreath_strength,
        drop_mode: config.drop_mode,
        drop_threshold: config.drop_threshold,
        drop_gap: config.drop_gap,
        hier_qp_enabled: config.hier_qp_enabled,
        hier_qp_delta: config.hier_qp_delta,
        hier_frame_num: config.hier_frame_num,
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
    // Widths that need CIME extension lines are no longer refused:
    // `ext_line_buf_size` sizes the buffer the way `setup_hal_bufs` does and
    // `setup_vepu580_ext_line_buf`'s registers, reg-offset entry and RCB pair
    // are wired in `build_registers`. The SPS level and the geometry fields are
    // derived, not frozen, so a >4K picture reaches the hardware the way the
    // reference gets it there.
    if config.qp_init > 51 {
        return Err(format!("H.264 QP {} is outside 0..=51", config.qp_init));
    }
    // The level is no longer a frozen field: `mv_limit`, the CIME search window and
    // the SPS's own `level_idc` (including MPP's bump-to-what-the-size-needs rule) are
    // all derived from it. The values verified against the reference encoder are
    // listed in ROADMAP; anything outside that set still gets through, because the
    // derivation covers it, but it is the reference's own behaviour that decides.
    if !(9..=52).contains(&config.level_idc) {
        return Err(format!(
            "H.264 level {} is outside the 1.0..5.2 range the SPS can carry",
            config.level_idc
        ));
    }
    Ok(())
}

/// ROI grid of the VEPU580 path: 16x16 blocks over the *visible* picture,
/// rows padded to a multiple of four, as `gen_vepu580_roi_h264` indexes it.
fn roi_grid(config: &EncoderConfig) -> (usize, usize, usize, usize) {
    let mb_w = (align(config.width, 16) / 16).max(1);
    let mb_h = (align(config.height, 16) / 16).max(1);
    (mb_w, mb_h, align(mb_w, 4), align(mb_h, 4))
}

/// Build the two ROI DMA tables the VEPU580 reads, `Vepu580RoiH264BsCfg`
/// (eight bytes per 16x16 block) and `Vepu580RoiQpCfg` (two bytes), from the
/// configured regions.
///
/// The transcription follows `roi_data_update_cfg` then
/// `gen_vepu580_roi_h264`: a default record covers the whole grid, each region
/// overwrites the blocks it touches - its origin rounded *up* to the grid,
/// `(x + 15) / 16`, which the VEPU54x helper `vepu5xx_h264_set_one_roi` does
/// not do - and only blocks whose map entry got set produce non-zero records.
/// The bit positions are the C's little-endian bitfields: `force_intra` at bit
/// 61 and `qp_adj_en` at bit 62 of the eight-byte record, `qp_adj` in bits
/// 8..14 of the two-byte one with the absolute mode at bit 15. `qp_adj_en`
/// follows `!!cu_cfg->qp_adj` - a region at relative QP zero enables nothing,
/// and `force_intra` still reaches the block.
fn roi_tables(config: &EncoderConfig) -> (Vec<u8>, Vec<u8>) {
    let (mb_w, mb_h, stride_h, stride_v) = roi_grid(config);
    let blocks = stride_h * stride_v;
    // Records as the tmp map holds them: (force_intra, 7-bit qp, absolute).
    let mut tmp = vec![(false, 0u16, false); stride_h * mb_h];
    let mut cu_map = vec![0u8; stride_h * mb_h];

    for region in &config.rois {
        let qp_adj = (region.qp_val as u16) & 0x7f;
        let cfg = (region.force_intra, qp_adj, region.qp_absolute);
        let roi_width = (region.w + 15) / 16;
        let roi_height = (region.h + 15) / 16;
        let pos_x_init = (region.x + 15) / 16;
        let pos_y_init = (region.y + 15) / 16;
        let pos_x_end = (pos_x_init + roi_width).min(mb_w as i32) as usize;
        let pos_y_end = (pos_y_init + roi_height).min(mb_h as i32) as usize;
        // The C `mpp_assert`s the region fits the grid after the frame check;
        // a region clipped by the two `min`s above is exactly the case that
        // assert would trap on, so no byte comparison can observe the
        // difference and this port clips instead of crashing.
        for j in (pos_y_init.max(0) as usize)..pos_y_end {
            for x in (pos_x_init.max(0) as usize)..pos_x_end {
                tmp[j * stride_h + x] = cfg;
                cu_map[j * stride_h + x] = 1;
            }
        }
    }

    let mut base = vec![0u8; blocks * 8];
    let mut qp = vec![0u8; blocks * 2];
    for j in 0..mb_h {
        for k in 0..stride_h {
            if cu_map[j * stride_h + k] == 0 {
                continue;
            }
            let (force_intra, qp_adj, absolute) = tmp[j * stride_h + k];
            let index = j * stride_h + k;
            let mut record = 0u64;
            if force_intra {
                record |= 1 << 61;
            }
            if qp_adj != 0 {
                record |= 1 << 62;
            }
            base[index * 8..index * 8 + 8].copy_from_slice(&record.to_le_bytes());
            let mut word = qp_adj << 8;
            if absolute {
                word |= 1 << 15;
            }
            qp[index * 2..index * 2 + 2].copy_from_slice(&word.to_le_bytes());
        }
    }
    (base, qp)
}

/// The force-intra base table of one refresh band, `setup_vepu580_intra_refresh`
/// plus `vepu580_h264_set_one_roi` in one pass: every record of the
/// `align(mb, 4)`-strided grid starts at `force_intra = 0, qp_adj_en = 1`, and
/// the band's records - `refresh_num` macroblock lines tall along the refresh
/// axis, shifted back by two lines and grown by two on every band but the
/// first, `refresh_idx * 16 * n - 32` with height `16 * n + 32` against the
/// first band's `y = 0, h = 16 * n` - are overwritten with `force_intra` set.
///
/// The origin is truncated to `RK_U16` before any arithmetic, because
/// `MppEncROIRegion.x/.y` *are* `RK_U16`: at `refresh_num == 1` the second band's
/// `16 * n - 32` is negative, C stores 0xFFF0, and the very same clamp that saves
/// the bottom-overrun band then leaves an empty range - the picture gets no forced
/// intra at all, which is what the `row_n1` cell shows byte for byte. Clamping the
/// signed value instead (so -16 lands on row 0) writes two extra macroblock lines
/// and costs 836 bytes of difference on that one picture.
///
/// `region->quality` and `qp_area_idx` of the C region are dropped along with
/// the QP table the function never allocates: the refresh path enables
/// `roi_en`, not `roi_qp_en`, so no QP surface is read and the band records'
/// `qp_adj_en` bit - left uninitialised by the C's `set_one_roi` local - has
/// nothing to select. Carrying the base fill's value is the difference between
/// a defined bit and stack contents, and on this board pair both encode the
/// force-intra bit the same.
fn gdr_base_table(config: &EncoderConfig, refresh_idx: u32) -> Vec<u8> {
    let mb_w = (config.coded_width() / 16).max(1);
    let mb_h = (config.coded_height() / 16).max(1);
    let stride_h = align(mb_w, 4);
    let stride_v = align(mb_h, 4);
    let _ = stride_v; // the grid is indexed row-strided; padding rows stay at the base fill.
    let base_fill: u64 = 1 << 62; // qp_adj_en, force_intra clear
    let band: u64 = (1 << 61) | (1 << 62); // force_intra over the band

    let n = config.gdr_num as i32;
    let (region_x, region_y, region_w, region_h) = match config.gdr_mode {
        GdrMode::Row => (
            0,
            if refresh_idx > 0 {
                refresh_idx as i32 * 16 * n - 32
            } else {
                0
            },
            mb_w as i32 * 16,
            if refresh_idx > 0 { 16 * n + 32 } else { 16 * n },
        ),
        GdrMode::Col => (
            if refresh_idx > 0 {
                refresh_idx as i32 * 16 * n - 32
            } else {
                0
            },
            0,
            if refresh_idx > 0 { 16 * n + 32 } else { 16 * n },
            mb_h as i32 * 16,
        ),
    };

    // The struct's own width first: whatever the arithmetic produced is stored in 16
    // unsigned bits, so a negative origin becomes a large positive and the clip
    // below empties the range instead of pulling it back to the origin.
    let (region_x, region_y, region_w, region_h) = (
        u16::try_from(region_x).unwrap_or(region_x as u16) as i32,
        u16::try_from(region_y).unwrap_or(region_y as u16) as i32,
        u16::try_from(region_w).unwrap_or(region_w as u16) as i32,
        u16::try_from(region_h).unwrap_or(region_h as u16) as i32,
    );

    // `vepu580_h264_set_one_roi`'s block arithmetic: origin divided by 16 with C's
    // truncation, extent rounded up, both ends clipped to the grid.
    let roi_width = (region_w + 15) / 16;
    let roi_height = (region_h + 15) / 16;
    let pos_x_init = region_x / 16;
    let pos_y_init = region_y / 16;
    let pos_x_end = (pos_x_init + roi_width).min(mb_w as i32);
    let pos_y_end = (pos_y_init + roi_height).min(mb_h as i32);
    let x_start = pos_x_init.max(0) as usize;
    let y_start = pos_y_init.max(0) as usize;

    let mut table = vec![0u8; stride_h * stride_v * 8];
    for record in table.chunks_exact_mut(8) {
        record.copy_from_slice(&base_fill.to_le_bytes());
    }
    let x_end = (pos_x_end.max(0) as usize).max(x_start);
    let y_end = (pos_y_end.max(0) as usize).max(y_start);
    for y in y_start..y_end {
        for x in x_start..x_end {
            let index = y * stride_h + x;
            table[index * 8..index * 8 + 8].copy_from_slice(&band.to_le_bytes());
        }
    }
    table
}

/// `setup_hal_bufs`' extension-line sizing: 480 bytes per coded 64-pixel column
/// beyond 3072, allocated whenever the width aligned to 64 clears 4096. The
/// division before the multiplication is the C's.
fn ext_line_buf_size(config: &EncoderConfig) -> usize {
    let aligned_w = align(config.width, 64);
    if aligned_w > 4096 {
        (aligned_w - 3 * 1024) / 64 * 480
    } else {
        0
    }
}

/// `pixel_buf_fbc_hdr_size` of `hal_h264e_vepu580_prepare`: the FBC header of a
/// reconstruction surface occupies `align(aligned_w * aligned_h / 64, 8K)` bytes
/// and the body starts behind it. The number is a function of the picture - it
/// crosses from 8192 to 16384 anywhere the coded area passes 524288 pixels, so
/// 1280x720 is the first size where it is not 8192 - which is why the allocation
/// and the two `REG_RFP{W,R}_B` offsets have to come from this one function
/// rather than from a constant. Feeding the hardware a body offset of 8192 over a
/// surface whose body starts at 16384 leaves the reference picture read from the
/// wrong place: every P picture then spends bits the reference does not, while the
/// IDR in the same run stays byte-identical, because an IDR reads no reference.
fn fbc_header_size(config: &EncoderConfig) -> Result<usize, String> {
    let aligned_w = align(config.width, 64);
    let aligned_h = align(config.height, 16) + 16;
    Ok(align(
        aligned_w
            .checked_mul(aligned_h)
            .ok_or_else(|| "H.264 FBC header overflow".to_string())?
            / 64,
        8192,
    ))
}

fn reconstruction_sizes(config: &EncoderConfig) -> Result<(usize, usize), String> {
    let aligned_w = align(config.width, 64);
    let aligned_h = align(config.height, 16) + 16;
    let header = fbc_header_size(config)?;
    let body = aligned_w
        .checked_mul(aligned_h)
        .and_then(|v| v.checked_mul(3))
        .map(|v| v / 2)
        .ok_or_else(|| "H.264 FBC body overflow".to_string())?;
    let thumb = align(aligned_w / 64 * aligned_h / 64 * 256, 8192);
    Ok((header + body, thumb))
}

/// VEPU580's debreath reconstruction is linear NV12 aligned to 64x16, unlike
/// the normal FBC surfaces.
fn pass1_luma_size(config: &EncoderConfig) -> usize {
    align(config.width, 64) * align(config.height, 16)
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

/// The reference encoder patches its baseline slice header after VEPU580 has
/// produced the payload. The default CPB keeps the opening IDR long-term and
/// points later P pictures at long-term index zero. The hardware's reference
/// address can be redirected by the HAL, but those slice-header bits are a
/// software amendment, so replay them here for the official MPP object path.
fn amend_default_reference_slice(
    payload: Vec<u8>,
    config: &EncoderConfig,
    state: &FrameState,
    parameter_sets: &[u8],
) -> Result<Vec<u8>, String> {
    let reorder = (state.index >= 2).then_some(crate::encoder::ReferenceReorder::LongTerm {
        long_term_pic_num: 2,
    });
    amend_reference_slice(
        payload,
        config,
        state,
        parameter_sets,
        reorder,
        state.is_idr.then_some(0),
    )
}

fn amend_reference_slice(
    payload: Vec<u8>,
    config: &EncoderConfig,
    state: &FrameState,
    parameter_sets: &[u8],
    reorder: Option<crate::encoder::ReferenceReorder>,
    current_long_term_index: Option<u32>,
) -> Result<Vec<u8>, String> {
    if config.entropy_coding {
        return Ok(payload);
    }
    let hardware_nals = crate::codec::h264::split_annex_b(&payload);
    let vcl_count = hardware_nals
        .iter()
        .filter(|nal| nal.unit_type == 1 || nal.unit_type == 5)
        .count();
    if vcl_count != 1 {
        return Err(format!(
            "H.264 amendment expects one slice NAL, found {vcl_count}"
        ));
    }
    let hardware_slice = hardware_nals
        .iter()
        .find(|nal| nal.unit_type == 1 || nal.unit_type == 5)
        .ok_or_else(|| "H.264 amendment found no slice NAL".to_string())?;
    let headers = crate::codec::h264::split_annex_b(parameter_sets);
    let sps_nal = headers
        .iter()
        .find(|nal| nal.unit_type == 7)
        .ok_or_else(|| "H.264 amendment found no SPS".to_string())?;
    let pps_nal = headers
        .iter()
        .find(|nal| nal.unit_type == 8)
        .ok_or_else(|| "H.264 amendment found no PPS".to_string())?;
    let sps = crate::codec::h264::parse_sps(&sps_nal.rbsp)
        .map_err(|error| format!("H.264 amendment SPS: {error}"))?;
    let pps = crate::codec::h264::parse_pps(&pps_nal.rbsp)
        .map_err(|error| format!("H.264 amendment PPS: {error}"))?;
    let header = crate::codec::h264::parse_slice_header(
        &hardware_slice.rbsp,
        &sps,
        &pps,
        hardware_slice.unit_type,
        hardware_slice.ref_idc,
    )
    .map_err(|error| format!("H.264 amendment slice: {error}"))?;
    if header.weights.is_some() {
        return Err("H.264 amendment does not support weighted prediction".into());
    }
    let source_bits = bytes_to_bits(&hardware_slice.rbsp);
    if header.data_offset_bits > source_bits.len() {
        return Err("H.264 amendment slice data offset overflow".into());
    }

    let mut bits = Vec::new();
    put_ue(&mut bits, header.first_mb);
    put_ue(&mut bits, header.slice_type);
    put_ue(&mut bits, header.pps_id);
    let frame_num_bits = config.log2_max_frame_num_minus4 as u32 + 4;
    put_bits(
        &mut bits,
        header.frame_num & ((1 << frame_num_bits) - 1),
        frame_num_bits,
    );
    if header.slice_type % 5 == 2 {
        put_ue(&mut bits, header.idr_pic_id.unwrap_or(0));
    }
    if sps.poc_type == 0 {
        let poc_bits = config.log2_max_poc_lsb_minus4 as u32 + 4;
        put_bits(
            &mut bits,
            header.poc_lsb.unwrap_or(state.poc as u32) & ((1 << poc_bits) - 1),
            poc_bits,
        );
    }
    if header.redundant_pic_cnt.is_some() {
        put_ue(&mut bits, header.redundant_pic_cnt.unwrap());
    }
    if header.slice_type % 5 == 0 {
        let override_active = header.ref_idx_l0_active != pps.num_ref_idx_l0;
        put_bits(&mut bits, u32::from(override_active), 1);
        if override_active {
            put_ue(&mut bits, header.ref_idx_l0_active);
        }
        match reorder {
            Some(reorder) => {
                put_bits(&mut bits, 1, 1);
                match reorder {
                    crate::encoder::ReferenceReorder::LongTerm { long_term_pic_num } => {
                        put_ue(&mut bits, 2);
                        put_ue(&mut bits, long_term_pic_num);
                    }
                    crate::encoder::ReferenceReorder::ShortTerm(order) => {
                        let max_pic_num = 1u32 << (config.log2_max_frame_num_minus4 as u32 + 4);
                        let forward = order
                            .current_frame_num()
                            .wrapping_add(max_pic_num)
                            .wrapping_sub(order.reference_frame_num())
                            % max_pic_num;
                        let (direction, distance) = if forward <= max_pic_num / 2 {
                            (0, forward)
                        } else {
                            (1, max_pic_num - forward)
                        };
                        if distance == 0 {
                            return Err(
                                "H.264 reference reorder resolves to the current frame".into()
                            );
                        }
                        put_ue(&mut bits, direction);
                        put_ue(&mut bits, distance - 1);
                    }
                }
                put_ue(&mut bits, 3); // end of ref_pic_list_modification
            }
            None => put_bits(&mut bits, 0, 1),
        }
    } else if header.slice_type % 5 == 1 {
        return Err("H.264 amendment does not support B slices".into());
    }
    if header.nal_ref_idc != 0 {
        if header.slice_type % 5 == 2 {
            put_bits(&mut bits, u32::from(header.no_output_of_prior_pics), 1);
            put_bits(&mut bits, u32::from(current_long_term_index.is_some()), 1);
        } else {
            if header.adaptive_ref_pic_buffering || !header.mmco.is_empty() {
                return Err("H.264 amendment does not support adaptive marking".into());
            }
            if let Some(long_term_index) = current_long_term_index {
                put_bits(&mut bits, 1, 1); // adaptive_ref_pic_buffering_mode
                put_ue(&mut bits, 6); // memory_management_control_operation
                put_ue(&mut bits, long_term_index);
                put_ue(&mut bits, 0); // end of memory_management_control_operation
            } else {
                put_bits(&mut bits, 0, 1); // adaptive_ref_pic_buffering_mode
            }
        }
    }
    put_se(&mut bits, header.qp_delta);
    if pps.deblocking_filter_control {
        put_ue(&mut bits, header.deblocking_disable_idc);
        if header.deblocking_disable_idc != 1 {
            put_se(&mut bits, header.deblocking_alpha);
            put_se(&mut bits, header.deblocking_beta);
        }
    }
    bits.extend_from_slice(&source_bits[header.data_offset_bits..]);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    let rbsp = bits_to_bytes(&bits);
    let mut escaped = Vec::with_capacity(rbsp.len() + 4);
    let mut zeroes = 0usize;
    for byte in rbsp {
        if zeroes >= 2 && byte <= 3 {
            escaped.push(3);
            zeroes = 0;
        }
        escaped.push(byte);
        if byte == 0 {
            zeroes += 1;
        } else {
            zeroes = 0;
        }
    }
    while escaped.last() == Some(&0) {
        escaped.pop();
    }
    let mut amended = Vec::with_capacity(escaped.len() + 4);
    amended.extend_from_slice(&[0, 0, 0, 1]);
    amended.push((header.nal_ref_idc << 5) | hardware_slice.unit_type);
    amended.extend_from_slice(&escaped);
    Ok(amended)
}

fn bytes_to_bits(bytes: &[u8]) -> Vec<bool> {
    bytes
        .iter()
        .flat_map(|byte| (0..8).rev().map(move |bit| byte & (1 << bit) != 0))
        .collect()
}

fn bits_to_bytes(bits: &[bool]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .fold(0u8, |byte, bit| (byte << 1) | u8::from(*bit))
        })
        .collect()
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

    #[test]
    fn software_pskip_matches_the_c_slice_bytes() {
        let config = config();
        for (frame_num, poc, expected) in [
            (1, 2, "0000000101e000200047804b60"),
            (2, 6, "0000000101e0004000c7804b60"),
            (3, 10, "0000000101e000600147804b60"),
        ] {
            let state = FrameState {
                index: frame_num as usize,
                is_idr: false,
                is_vi: false,
                frame_num,
                idr_pic_id: 0,
                poc,
                is_i_refresh: false,
                is_i_recovery: false,
                refresh_idx: frame_num,
            };
            assert_eq!(
                pskip_nal(&config, &state).unwrap(),
                expected
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn software_cabac_pskip_matches_the_c_slice_bytes() {
        let mut config = config();
        config.profile = encoder::PROFILE_MAIN;
        config.entropy_coding = true;
        for (frame_num, poc, expected) in [
            (1, 2, "0000000101e000200047ff00000979"),
            (2, 6, "0000000101e0004000c7ff00000979"),
            (3, 10, "0000000101e000600147ff00000979"),
        ] {
            let state = FrameState {
                index: frame_num as usize,
                is_idr: false,
                is_vi: false,
                frame_num,
                idr_pic_id: 0,
                poc,
                is_i_refresh: false,
                is_i_recovery: false,
                refresh_idx: frame_num,
            };
            assert_eq!(
                pskip_nal(&config, &state).unwrap(),
                expected
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn debreath_patches_the_linear_pass1_buffer_and_second_pass_source() {
        let config = config();
        let p_state = FrameState {
            index: 1,
            is_idr: false,
            frame_num: 1,
            poc: 2,
            refresh_idx: 1,
            ..FrameState::default()
        };

        let mut save = job(&config, p_state);
        save.pass1_fd = 29;
        save.save_pass1 = true;
        let saved = build_registers(&save).unwrap();
        assert_eq!(saved.base[BASE_RFPW_H], 29);
        assert_eq!(saved.base[BASE_RFPW_B], 29);
        assert_eq!(
            saved.get(&vepu580_regs::BASE_ENC_PIC_CUR_FRM_REF).unwrap(),
            1
        );
        assert_eq!(
            saved.get(&vepu580_regs::BASE_ENC_PIC_REC_FBC_DIS).unwrap(),
            1
        );
        assert_eq!(
            saved.get(&vepu580_regs::H264_SYNT_NAL_NAL_REF_IDC).unwrap(),
            1,
            "pass1 is a non-reference P picture"
        );
        let luma = pass1_luma_size(&config) as u32;
        assert_eq!(
            saved
                .offsets
                .iter()
                .find(|o| o.reg_idx == REG_RFPW_B)
                .unwrap()
                .offset,
            luma
        );

        let mut use_first = job(&config, FrameState::default());
        use_first.pass1_fd = 29;
        use_first.use_pass1 = true;
        let used = build_registers(&use_first).unwrap();
        assert_eq!(used.base[BASE_ADR_SRC0], 29);
        assert_eq!(used.base[BASE_ADR_SRC1], 29);
        assert_eq!(used.base[BASE_ADR_SRC2], 29);
        assert_eq!(used.get(&vepu580_regs::BASE_SRC_FMT_SRC_CFMT).unwrap(), 6);
        assert_eq!(used.get(&vepu580_regs::BASE_SRC_FMT_OUT_FMT).unwrap(), 1);
        for reg_idx in [REG_SRC_1_OFFSET, REG_SRC_2_OFFSET] {
            assert_eq!(
                used.offsets
                    .iter()
                    .find(|o| o.reg_idx == reg_idx)
                    .unwrap()
                    .offset,
                luma
            );
        }
    }

    /// A job on stand-in fds, which the register builder only copies around.
    fn job(config: &EncoderConfig, state: FrameState) -> Job<'_> {
        Job {
            tune: quiet_tune(),
            roi_fd: -1,
            roi_qp_fd: -1,
            gdr_fd: -1,
            ext_fd: -1,
            ext_offset: 0,
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
            save_pass1: false,
            use_pass1: false,
            non_reference: false,
            current_long_term: false,
            pass1_fd: -1,
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

    /// `pixel_buf_fbc_hdr_size` grows with the picture and the two FBC body
    /// offsets ride that same number. 640x480 is well past the sizes the P path
    /// matched at and still lands in the first 8K row, 800x600 bigger still does
    /// too, and 1280x720 is the first size across into the second - which is
    /// precisely where the P pictures started to diverge. Telling the hardware the
    /// body begins at 8192 over a surface whose body begins at 16384 leaves the
    /// reference read from the wrong place, so every P spends bits the reference
    /// does not while the IDR - which reads no reference - stays byte-identical.
    /// That is how this hid behind a matrix of IDR pictures.
    #[test]
    fn the_fbc_body_offset_follows_the_surface_it_points_into() {
        assert_eq!(
            fbc_header_size(&EncoderConfig::new(320, 240)).unwrap(),
            8_192
        );
        assert_eq!(
            fbc_header_size(&EncoderConfig::new(640, 480)).unwrap(),
            8_192
        );
        assert_eq!(
            fbc_header_size(&EncoderConfig::new(800, 600)).unwrap(),
            8_192
        );
        assert_eq!(
            fbc_header_size(&EncoderConfig::new(1280, 720)).unwrap(),
            16_384
        );

        let wide = EncoderConfig::new(1280, 720);
        let capacity = logical_output_size(&wide).unwrap();
        let file = build_registers(&job_with_output(&wide, capacity)).unwrap();
        let offset_of = |idx: u32| {
            file.offsets
                .iter()
                .find(|o| o.reg_idx == idx)
                .map(|o| o.offset)
        };
        assert_eq!(offset_of(REG_RFPW_B), Some(16_384));
        assert_eq!(offset_of(REG_RFPR_B), Some(16_384));
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
        ] {
            assert!(build_registers(&job_with_output(
                &config,
                config.coded_width() * config.coded_height() * 2
            ))
            .is_err());
        }
        // Widths above 4096 are no longer on this list: the CIME extension
        // lines they need are allocated by `Vepu580H264::open` and wired by
        // `build_registers`, the way `setup_hal_bufs` and
        // `setup_vepu580_ext_line_buf` do it for the reference.
        let wide = EncoderConfig::new(4098, 16);
        assert!(validate_reference_config(&wide).is_ok());
    }

    /// The GOP walk the slice header has to report. PocType 20 without B frames
    /// is a recurrence of +2 per picture, reset at an IDR, and idr_pic_id
    /// increments across IDR pictures - the same numbers `h264e_dpb` and
    /// `h264e_slice_init` produce, and the same ones the C encoder's second
    /// picture carried (`frame_num = 1`, `poc_lsb = 2`).
    #[test]
    fn frame_state_walks_the_gop() {
        let gop = 3;
        let mut state = FrameState::default();
        let mut seen = Vec::new();
        for _ in 0..7 {
            seen.push((state.is_idr, state.frame_num, state.poc, state.idr_pic_id));
            state = state.next(gop, 0);
        }
        assert_eq!(
            seen,
            vec![
                (true, 0, 0, 0),  // IDR
                (false, 1, 2, 0), // P
                (false, 2, 4, 0), // P
                (true, 0, 0, 1),  // IDR
                (false, 1, 2, 1),
                (false, 2, 4, 1),
                (true, 0, 0, 2), // IDR
            ]
        );
    }

    #[test]
    fn tsvc2_reference_picture_reuses_frame_num_but_not_poc() {
        let idr = FrameState::default();
        let p1 = next_picture_state(idr, 60, 0, true, false, true);
        let p2 = next_picture_state(p1, 60, 0, true, true, false);
        let p3 = next_picture_state(p2, 60, 0, true, false, true);
        let p4 = next_picture_state(p3, 60, 0, true, true, false);
        assert_eq!(
            [p1, p2, p3, p4].map(|state| (state.frame_num, state.poc)),
            [(1, 2), (1, 4), (2, 6), (2, 8)]
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
            ..FrameState::default()
        };
        let p = build_registers(&Job {
            tune: quiet_tune(),
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
        // The level was the last property frozen into the recorded image; it is a
        // generator input now, and what it reaches is asserted below rather than
        // assumed here.
        let mut level = config();
        level.level_idc = 40;
        assert!(
            build_registers(&job(&level, FrameState::default())).is_ok(),
            "a level other than the recorded 3.0 generates"
        );
    }

    /// The level reaches exactly the fields `setup_vepu580_codec`,
    /// `calc_cime_parameter` and the SPS writer let it reach - and nothing else.
    #[test]
    fn the_level_reaches_the_fields_the_reference_lets_it() {
        let read = |file: &RegisterFile, field: &vepu580_regs::Field| {
            file.get(field)
                .unwrap_or_else(|e| panic!("{}: {e}", field.name))
        };
        let at = |level: u8| {
            let mut cfg = config();
            cfg.level_idc = level;
            registers_at(&cfg, FrameState::default())
        };

        // `rect_size` is "Baseline and level <= 3.0", so 3.1 already drops it.
        assert_eq!(read(&at(30), &vepu580_regs::H264_RDO_CFG_RECT_SIZE), 1);
        let mut baseline_31 = config();
        baseline_31.level_idc = 31;
        assert_eq!(
            read(
                &registers_at(&baseline_31, FrameState::default()),
                &vepu580_regs::H264_RDO_CFG_RECT_SIZE
            ),
            0
        );

        // `mv_limit` follows the reference's switch. The default 320x240 raises
        // 1.0 to 1.1, so 1.1 is the first value that reads through untouched.
        assert_eq!(read(&at(11), &vepu580_regs::H264_ME_CFG_MV_LIMIT), 1);
        assert_eq!(read(&at(21), &vepu580_regs::H264_ME_CFG_MV_LIMIT), 2);
        assert_eq!(read(&at(51), &vepu580_regs::H264_ME_CFG_MV_LIMIT), 2);

        // CIME's vertical search window: the generator takes the raised level,
        // and 1.1 up gets the full 28 blocks (`cme_srch_v` is in quarters).
        let srch_v = |file: &RegisterFile| (file.base[BASE_ME_RNGE] >> 4) & 0xf;
        assert_eq!(srch_v(&at(11)), 7, "28 blocks / 4");
        assert_eq!(srch_v(&at(40)), 7);
        // The 12-block branch is what the switch gives 1.0 and 1-b before the
        // size raise can cover it; at a picture small enough not to raise it,
        // the step-2 clamp is not on either.
        assert_eq!(
            (build_me_range(20, 15, 10).unwrap() >> 4) & 0xf,
            3,
            "12 blocks / 4"
        );

        // A level the SPS cannot carry is refused rather than truncated.
        let mut absurd = config();
        absurd.level_idc = 60;
        assert!(build_registers(&job(&absurd, FrameState::default())).is_err());
    }

    #[test]
    fn rate_control_uses_the_configured_quality_band() {
        let mut cfg = config();
        cfg.rc_mode = RcMode::Cbr;
        cfg.gop = 60;
        cfg.quality_auto = true;
        cfg.rc_qp_min = 10;
        cfg.rc_qp_max = 51;
        let model = rc_model_config(&cfg).unwrap();
        assert_eq!(model.quality_min, 10);
        assert_eq!(model.quality_max, 51);

        cfg.rc_qp_min = 40;
        cfg.rc_qp_max = 20;
        assert!(cfg.validate().is_err());
    }

    /// The HAL reads the level `h264e_sps_update` has already raised to what the
    /// coded size needs, not the raw request: a 1.0 request at 320x240 is a 1.1
    /// stream (mv_limit 1, not 0), and below the 3.1 a 720p picture needs every
    /// request reads as 3.1.
    #[test]
    fn the_hal_fields_follow_the_raised_level_not_the_request() {
        let mv = |file: &RegisterFile| {
            file.get(&vepu580_regs::H264_ME_CFG_MV_LIMIT)
                .expect("mv_limit")
        };
        let rect = |file: &RegisterFile| {
            file.get(&vepu580_regs::H264_RDO_CFG_RECT_SIZE)
                .expect("rect_size")
        };
        let srch_v = |file: &RegisterFile| (file.base[BASE_ME_RNGE] >> 4) & 0xf;
        let at = |width: usize, height: usize, level: u8| {
            let mut cfg = EncoderConfig::new(width, height);
            cfg.level_idc = level;
            build_registers(&Job {
                output_capacity: 4_000_000,
                ..job(&cfg, FrameState::default())
            })
            .unwrap()
        };

        // 320x240 raises 1.0 to 1.1 (board-probed: the reference SPS carries 11),
        // which lifts mv_limit off zero while keeping `rect_size`.
        assert_eq!(mv(&at(320, 240, 10)), 1, "1.0 is raised to 1.1");
        assert_eq!(rect(&at(320, 240, 10)), 1);
        assert_eq!(
            srch_v(&at(320, 240, 10)),
            7,
            "raised to 1.1, which gets all 28 blocks"
        );

        // 1280x720 raises everything below 3.1 to 3.1, so the fields there equal
        // the fields a 3.1 request itself produces.
        let raised = at(1280, 720, 10);
        let native = at(1280, 720, 31);
        assert_eq!(mv(&raised), mv(&native));
        assert_eq!(rect(&raised), rect(&native));
        assert_eq!(srch_v(&raised), srch_v(&native));
        assert_eq!(mv(&raised), 2);
        assert_eq!(rect(&raised), 0, "3.1 is past the level <= 3.0 condition");
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

    /// The deblocking fields, pinned at the shape `setup_vepu580_codec` writes.
    /// It has exactly two deblocking statements - `dis_dblk_idc` and
    /// `sli_alph_ofst` - and the alpha one is a signed value into a 4-bit
    /// bitfield, which the C stores as two's complement, so -3 must read back as
    /// 13. The beta field exists in the register but is never written, so a
    /// configured beta has to leave it at zero: the reference cannot be matched
    /// by being *more* faithful than it is.
    #[test]
    fn deblock_writes_the_two_fields_the_reference_writes_and_no_beta() {
        let mut at = config();
        at.deblock_disable = 2;
        at.deblock_offset_alpha = -3;
        at.deblock_offset_beta = 5;
        let registers = registers_at(&at, FrameState::default());
        assert_eq!(
            registers
                .get(&vepu580_regs::H264_SYNT_SLI2_DIS_DBLK_IDC)
                .unwrap(),
            2
        );
        assert_eq!(
            registers
                .get(&vepu580_regs::H264_SYNT_SLI2_SLI_ALPH_OFST)
                .unwrap(),
            13,
            "two's complement in four bits, as the RK_U32 : 4 stores it"
        );
        assert_eq!(
            registers
                .get(&vepu580_regs::H264_SYNT_SLI2_SLI_BETA_OFST)
                .unwrap(),
            0,
            "VEPU580's HAL never writes sli_beta_ofst"
        );

        // The defaults - every byte-verified stream so far - write the zeros the
        // recorded image already carries, which is why this commit changes
        // nothing for them.
        let registers = registers_at(&config(), FrameState::default());
        for field in [
            &vepu580_regs::H264_SYNT_SLI2_DIS_DBLK_IDC,
            &vepu580_regs::H264_SYNT_SLI2_SLI_ALPH_OFST,
            &vepu580_regs::H264_SYNT_SLI2_SLI_BETA_OFST,
        ] {
            assert_eq!(registers.get(field).unwrap(), 0, "{}", field.name);
        }

        // `disable_deblocking_filter_idc` is a 2-bit unsigned field: an
        // out-of-range idc truncates through the low two bits the way the
        // bitfield assignment does. 4 lands on 0, 7 on 3, and MAX on 3.
        for (value, want) in [(4u32, 0u32), (7, 3), (u32::MAX, 3)] {
            let mut at = config();
            at.deblock_disable = value;
            let registers = registers_at(&at, FrameState::default());
            assert_eq!(
                registers
                    .get(&vepu580_regs::H264_SYNT_SLI2_DIS_DBLK_IDC)
                    .unwrap(),
                want,
                "idc {value} truncates through the field width"
            );
        }

        // The full alpha range the reference documentation allows, both signs.
        for alpha in -6..=6i32 {
            let mut at = config();
            at.deblock_offset_alpha = alpha;
            let registers = registers_at(&at, FrameState::default());
            assert_eq!(
                registers
                    .get(&vepu580_regs::H264_SYNT_SLI2_SLI_ALPH_OFST)
                    .unwrap(),
                (alpha & 0xf) as u32,
                "alpha {alpha}"
            );
        }
    }

    /// The ROI region arithmetic, block by block. The two regions are the
    /// `mpi_enc_test` pair scaled to fit 320x240 - an absolute-QP-24 region
    /// first, an absolute-QP-10 force-intra region added second, overlapping
    /// the first at blocks x 10..11, y 4..11 where the second must win.
    ///
    /// The expected block coordinates are derived from the grid rules, not
    /// copied: origins round up (`(x + 15) / 16`), sizes round up
    /// (`(w + 15) / 16`), the row stride pads the macroblock count to four
    /// (20 columns, 16 rows of records for a 20x15 grid), and a block record
    /// carries `force_intra` at bit 61 and `qp_adj_en` - non-zero `qp_adj`
    /// only - at bit 62, with the signed QP in bits 8..14 of the two-byte
    /// companion and the absolute mode at bit 15.
    #[test]
    fn roi_regions_map_to_the_block_records_the_encoder_reads() {
        let mut config = config();
        config.rois = vec![
            encoder::RoiRegion {
                x: 48,
                y: 32,
                w: 128,
                h: 160,
                force_intra: false,
                qp_absolute: true,
                qp_val: 24,
            },
            encoder::RoiRegion {
                x: 160,
                y: 64,
                w: 96,
                h: 128,
                force_intra: true,
                qp_absolute: true,
                qp_val: 10,
            },
        ];
        let (base, qp) = roi_tables(&config);
        assert_eq!(base.len(), 20 * 16 * 8, "stride 20, rows padded to 16");
        assert_eq!(qp.len(), 20 * 16 * 2);
        let record = |x: usize, y: usize| {
            u64::from_le_bytes(
                base[(y * 20 + x) * 8..(y * 20 + x) * 8 + 8]
                    .try_into()
                    .unwrap(),
            )
        };
        let word = |x: usize, y: usize| {
            u16::from_le_bytes(
                qp[(y * 20 + x) * 2..(y * 20 + x) * 2 + 2]
                    .try_into()
                    .unwrap(),
            )
        };

        // Outside both regions: nothing set.
        assert_eq!(record(0, 0), 0);
        assert_eq!(word(0, 0), 0);
        // Inside region one only: qp_adj enabled by a non-zero adjustment, no
        // force-intra, absolute mode, the QP itself in bits 8..14.
        assert_eq!(record(3, 2), 1 << 62);
        assert_eq!(word(3, 2), 24 << 8 | 1 << 15);
        // The overlap: the later region wins outright, in both tables.
        assert_eq!(record(10, 4), 1 << 61 | 1 << 62);
        assert_eq!(word(10, 4), 10 << 8 | 1 << 15);
        // Just past region one's bottom edge (y rounds up to 10 rows from row
        // 2) and region two's right edge (x rounds up to 6 columns from 10).
        assert_eq!(record(5, 12), 0);
        assert_eq!(record(16, 4), 0);
        assert_eq!(
            record(9, 4),
            1 << 62,
            "still region one at the overlap edge"
        );
        // The padding rows below the grid stay zero.
        assert_eq!(record(0, 15), 0, "the padding row carries no region");
    }

    /// `base_cfg->qp_adj_en = !!cu_cfg->qp_adj`: a force-intra region at
    /// relative QP zero sets the intra bit and nothing else, and its companion
    /// word is zero. A relative adjustment of -3 wraps into the 7-bit signed
    /// field as 125 - which is also what makes it non-zero, and therefore
    /// enables the adjustment.
    #[test]
    fn roi_qp_adjustment_is_the_seven_bit_wrap_not_the_sign() {
        let mut config = config();
        config.rois = vec![encoder::RoiRegion {
            x: 0,
            y: 0,
            w: 16,
            h: 16,
            force_intra: true,
            qp_absolute: false,
            qp_val: 0,
        }];
        let (base, qp) = roi_tables(&config);
        assert_eq!(u64::from_le_bytes(base[..8].try_into().unwrap()), 1 << 61);
        assert_eq!(u16::from_le_bytes(qp[..2].try_into().unwrap()), 0);

        config.rois[0].qp_val = -3;
        let (base, qp) = roi_tables(&config);
        assert_eq!(
            u64::from_le_bytes(base[..8].try_into().unwrap()),
            1 << 61 | 1 << 62,
            "a wrapped -3 is a non-zero qp_adj and enables it"
        );
        assert_eq!(
            u16::from_le_bytes(qp[..2].try_into().unwrap()),
            125 << 8,
            "relative mode: no bit 15, the wrap in bits 8..14"
        );
    }

    /// The register side: with regions configured the two fds, `roi_en` and
    /// `roi_qp_en` reach the image - the `setup_vepu580_roi` pair; without
    /// them the fields and the address words stay exactly as recorded, which
    /// is what every byte-verified stream carries.
    #[test]
    fn roi_registers_follow_the_configuration_and_nothing_else() {
        let state = FrameState::default();
        let mut with_rois = config();
        with_rois.rois = vec![encoder::RoiRegion {
            x: 0,
            y: 0,
            w: 32,
            h: 32,
            force_intra: true,
            qp_absolute: true,
            qp_val: 10,
        }];
        let file = build_registers(&Job {
            roi_fd: 20,
            roi_qp_fd: 21,
            ..job(&with_rois, state)
        })
        .unwrap();
        assert_eq!(
            file.get(&vepu580_regs::BASE_ENC_PIC_ROI_EN).unwrap(),
            1,
            "enc_pic.roi_en"
        );
        assert_eq!(
            file.get(&vepu580_regs::H264_ROI_EN_ROI_QP_EN).unwrap(),
            1,
            "the ROI_TYPE_2 helper always enables the qp table"
        );
        assert_eq!(file.base[178 - 160], 20, "roi_addr");
        assert_eq!(file.base[179 - 160], 21, "roi_qp_addr");

        let quiet = registers_at(&config(), state);
        assert_eq!(quiet.get(&vepu580_regs::BASE_ENC_PIC_ROI_EN).unwrap(), 0);
        assert_eq!(quiet.get(&vepu580_regs::H264_ROI_EN_ROI_QP_EN).unwrap(), 0);
        assert_eq!(quiet.base[178 - 160], 0);
        assert_eq!(quiet.base[179 - 160], 0);
    }

    /// The validation `roi_data_update_cfg` performs before touching a block:
    /// geometry inside the visible picture, absolute QPs up to 51, relative
    /// adjustments within +-51, no empty or negative-extent regions.
    #[test]
    fn roi_regions_are_the_reference_checks_not_a_new_policy() {
        let fits = encoder::RoiRegion {
            x: 0,
            y: 0,
            w: 32,
            h: 32,
            force_intra: true,
            qp_absolute: true,
            qp_val: 10,
        };
        let mut at = config();
        at.rois = vec![fits];
        assert_eq!(at.validate().map_err(|e| e.to_string()), Ok(()));

        at.rois[0].x = 300;
        assert!(at.validate().is_err(), "300 + 32 crosses 320");
        at.rois[0].x = 0;
        at.rois[0].qp_val = 52;
        assert!(at.validate().is_err(), "absolute QP 52");
        at.rois[0].qp_absolute = false;
        at.rois[0].qp_val = 52;
        assert!(at.validate().is_err(), "relative +52");
        at.rois[0].qp_val = -51;
        assert!(at.validate().is_ok(), "relative -51 is the edge");
        at.rois[0].w = 0;
        assert!(at.validate().is_err(), "an empty region is not a region");
    }
    /// The rows `vepu580_h264e_tune_reg_patch` writes on every picture - the
    /// ATR weights, the search multipliers and the two ATF weight sets - match
    /// the captured image while the detector is silent. That agreement is what
    /// lets the quiet path skip those writes; this test is the guard for the
    /// day it stops being true, and it also pins the positions of the fields
    /// the generator only learned to reach with the `Vepu580WgtCommon` pass:
    /// the CIME/RIME words of the captured job carry exactly the row the
    /// 2024-04 build computed for a no-change default-scene session.
    #[test]
    fn tune_default_rows_match_the_captured_image() {
        let p = registers_at(&config(), FrameState::default().next(60, 0));
        for (field, want) in [
            (
                &vepu580_regs::S3_CIME_SQI_MULTI0_CIME_MULTI0,
                CIME_MULTI[2][0],
            ),
            (
                &vepu580_regs::S3_CIME_SQI_MULTI0_CIME_MULTI1,
                CIME_MULTI[2][1],
            ),
            (
                &vepu580_regs::S3_CIME_SQI_MULTI1_CIME_MULTI2,
                CIME_MULTI[2][2],
            ),
            (
                &vepu580_regs::S3_CIME_SQI_MULTI1_CIME_MULTI3,
                CIME_MULTI[2][3],
            ),
            (
                &vepu580_regs::S3_RIME_SQI_MULTI_RIME_MULTI0,
                RIME_MULTI[2][0],
            ),
            (
                &vepu580_regs::S3_RIME_SQI_MULTI_RIME_MULTI1,
                RIME_MULTI[2][1],
            ),
            (
                &vepu580_regs::S3_RIME_SQI_MULTI_RIME_MULTI2,
                RIME_MULTI[2][2],
            ),
            (
                &vepu580_regs::RDO_RDO_INTRA_ATF_WGT0_ATF_RDO_INTRA_WGT00,
                INTRA_ATF_WEIGHTS[2][0],
            ),
            (
                &vepu580_regs::RDO_RDO_INTRA_ATF_WGT2_ATF_RDO_INTRA_WGT21,
                INTRA_ATF_WEIGHTS[2][7],
            ),
            (
                &vepu580_regs::RDO_RDO_INTRA_ATF_WGT3_ATF_RDO_INTRA_WGT32,
                INTRA_ATF_WEIGHTS[2][11],
            ),
            (
                &vepu580_regs::RDO_RDO_SKIP_ATF_WGT0_ATF_RDO_SKIP_ATF_WGT00,
                SKIP_ATF_WEIGHTS[2][0],
            ),
            (
                &vepu580_regs::S3_LVL16_ATR_WGT_LVL16_ATR_WGT0,
                ATR_WEIGHTS[2][0],
            ),
            (
                &vepu580_regs::S3_LVL4_ATR_WGT_LVL4_ATR_WGT2,
                ATR_WEIGHTS[2][8],
            ),
        ] {
            assert_eq!(
                p.get(field).unwrap(),
                want,
                "{} carries the row the C build wrote",
                field.name
            );
        }
    }

    #[test]
    fn tune_default_rows_are_rebuilt_instead_of_replayed() {
        let cfg = config();
        let mut file = registers_at(&cfg, FrameState::default().next(60, 0));
        file.set(
            &vepu580_regs::RDO_RDO_INTRA_CIME_THD0_ATF_RDO_INTRA_CIME_THD0,
            0,
        )
        .unwrap();
        file.set(
            &vepu580_regs::RDO_RDO_SKIP_CIME_THD0_ATF_RDO_SKIP_CIME_THD0,
            0,
        )
        .unwrap();
        file.set(&vepu580_regs::RDO_RDO_INTRA_ATF_WGT0_ATF_RDO_INTRA_WGT00, 0)
            .unwrap();
        file.set(
            &vepu580_regs::RDO_RDO_SKIP_ATF_WGT0_ATF_RDO_SKIP_ATF_WGT00,
            0,
        )
        .unwrap();
        file.set_param(&vepu580_regs::S3_CIME_SQI_MULTI0_CIME_MULTI0, 0)
            .unwrap();
        file.set_param(&vepu580_regs::S3_RIME_SQI_MULTI_RIME_MULTI0, 0)
            .unwrap();

        file.apply_tune(&job(&cfg, FrameState::default().next(60, 0)))
            .unwrap();
        for (field, want) in [
            (
                &vepu580_regs::RDO_RDO_INTRA_CIME_THD0_ATF_RDO_INTRA_CIME_THD0,
                28,
            ),
            (
                &vepu580_regs::RDO_RDO_SKIP_CIME_THD0_ATF_RDO_SKIP_CIME_THD0,
                10,
            ),
            (
                &vepu580_regs::RDO_RDO_INTRA_ATF_WGT0_ATF_RDO_INTRA_WGT00,
                INTRA_ATF_WEIGHTS[SCENE_MOTION_FLAG_DEFAULT][0],
            ),
            (
                &vepu580_regs::RDO_RDO_SKIP_ATF_WGT0_ATF_RDO_SKIP_ATF_WGT00,
                SKIP_ATF_WEIGHTS[SCENE_MOTION_FLAG_DEFAULT][0],
            ),
            (
                &vepu580_regs::S3_CIME_SQI_MULTI0_CIME_MULTI0,
                CIME_MULTI[SCENE_MOTION_FLAG_DEFAULT][0],
            ),
            (
                &vepu580_regs::S3_RIME_SQI_MULTI_RIME_MULTI0,
                RIME_MULTI[SCENE_MOTION_FLAG_DEFAULT][0],
            ),
        ] {
            assert_eq!(file.get(field).unwrap(), want, "{}", field.name);
        }
    }

    /// The C's own STATUS fixture - a still sequence, IDR then a repeated P -
    /// keeps the detector exactly where the port pinned it before it existed:
    /// no motion level above still, no scene flag, and the next picture's
    /// registers word-for-word what a quiet session would build.
    #[test]
    fn the_c_status_fixture_keeps_the_detector_quiet() {
        let text = std::fs::read_to_string("tests/data/vepu580_status_c_fixqp_2f.txt")
            .expect("C STATUS capture");
        let seen: Vec<Statistics> = text
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .map(|line| {
                Statistics::from_words(
                    &line
                        .split_whitespace()
                        .map(|w| u32::from_str_radix(w, 16).unwrap())
                        .collect::<Vec<u32>>(),
                )
            })
            .collect();

        let mut tune = SceneTune::new(false);
        for stats in &seen {
            tune.update(stats);
            assert_eq!(tune.curr_scene_motion_flag(), 0, "a still fixture");
        }
        assert_eq!(tune.scene_motion_flag(), SCENE_MOTION_FLAG_DEFAULT);

        let state = FrameState::default().next(60, 0);
        let with_fixture_state = build_registers(&Job {
            tune: &tune,
            ..job(&config(), state)
        })
        .unwrap();
        let quiet = registers_at(&config(), state);
        for (ours, theirs, name) in [
            (
                with_fixture_state.base.as_slice(),
                quiet.base.as_slice(),
                "base",
            ),
            (
                with_fixture_state.rc_klut.as_slice(),
                quiet.rc_klut.as_slice(),
                "rc_klut",
            ),
            (
                with_fixture_state.param.as_slice(),
                quiet.param.as_slice(),
                "param",
            ),
            (
                with_fixture_state.rdo.as_slice(),
                quiet.rdo.as_slice(),
                "rdo",
            ),
        ] {
            assert_eq!(ours, theirs, "{name} moved on a still sequence");
        }
    }

    /// Four pictures of large motion, one per `update`, against the detector's
    /// own rules: the score `2*13 = 26` clears nothing on the first picture,
    /// `26 + 2*11 = 48` clears the 41-point threshold on the second, and the
    /// flag rises only once two *scored* pictures already sit in the matrix -
    /// which, the matrix being pushed after the check, is the fourth picture.
    /// That trajectory, not the end state, is what pins the window arithmetic;
    /// the register patch that follows has to move every table to the row
    /// `ap * 2 + 1 = 3` and run the current-motion branch.
    #[test]
    fn three_consecutive_large_motion_pictures_switch_the_scene_mode() {
        // Statistics chosen against the rules at the default scene mode:
        // mean madp 200 clears the 192 gate, 100 counted blocks and one block
        // in the top `md_sad` histogram exceed `100 * 50 >> 15` = 0.
        let big = Statistics {
            bitstream_bytes: 1000,
            num_b16: 300,
            madi: 600,
            madp: 200,
            num_ctu: 1,
            md_sad_b16num: [0, 0, 0, 1],
            madi_b16num: [100, 0, 0, 0],
            qp_sum: 7800,
            pnum_i16: 0,
            pnum_i8: 0,
            pnum_i4: 0,
        };

        let mut tune = SceneTune::new(false);
        let flags: Vec<i32> = (0..4)
            .map(|_| {
                tune.update(&big);
                tune.curr_scene_motion_flag()
            })
            .collect();
        assert_eq!(
            flags,
            vec![0, 0, 0, 1],
            "the flag rises on the fourth picture"
        );
        assert_eq!(tune.scene_motion_flag(), 3);

        let state = FrameState::default().next(60, 0);
        let p = build_registers(&Job {
            tune: &tune,
            ..job(&config(), state)
        })
        .unwrap();
        for (field, want) in [
            (
                &vepu580_regs::S3_LVL16_INTRA_UL_CST_THD_LVL16_INTRA_UL_CST_THLD,
                2501,
            ),
            (&vepu580_regs::S3_RDO_QUANT_QUANT_F_BIAS_P, 341),
            (&vepu580_regs::H264_IPRD_CSTS_VTHD_Y, 0),
            (&vepu580_regs::H264_IPRD_CSTS_VTHD_C, 0),
            (&vepu580_regs::RCKLUT_KLUT_OFST_CHRM_KLUT_OFST, 0),
            (&vepu580_regs::H264_RDO_CFG_ATF_INTRA_E, 0),
            (&vepu580_regs::RDO_RDO_SQI_CFG_ATF_PSKIP_EN, 0),
            (&vepu580_regs::S3_ATR_THD1_ATR_THDQP, 51),
            (&vepu580_regs::S3_CIME_SQI_CFG_CIME_PMV_SET_ZERO, 0),
            (&vepu580_regs::S3_RIME_SQI_THD_CIME_SAD_TH0, 0),
            (&vepu580_regs::S3_FME_SQI_THD0_CIME_SAD_PU16_TH, 0),
            (&vepu580_regs::S3_FME_SQI_THD1_MOVE_LAMBDA, 8),
        ] {
            assert_eq!(
                p.get(field).unwrap(),
                want,
                "{} after the switch",
                field.name
            );
        }
        // The tables moved to row 3: uniform 20s for ATR, 4s for the searches.
        assert_eq!(
            p.get(&vepu580_regs::S3_LVL16_ATR_WGT_LVL16_ATR_WGT0)
                .unwrap(),
            ATR_WEIGHTS[3][0]
        );
        assert_eq!(
            p.get(&vepu580_regs::S3_CIME_SQI_MULTI0_CIME_MULTI1)
                .unwrap(),
            CIME_MULTI[3][1]
        );
        assert_eq!(
            p.get(&vepu580_regs::S3_RIME_SQI_MULTI_RIME_MULTI2).unwrap(),
            RIME_MULTI[3][2]
        );
        assert_eq!(
            p.get(&vepu580_regs::RDO_RDO_INTRA_ATF_WGT0_ATF_RDO_INTRA_WGT00)
                .unwrap(),
            INTRA_ATF_WEIGHTS[3][0]
        );
        // The inter chroma KLUT was pulled back to the table's first row.
        assert_eq!(
            p.rc_klut[vepu580_regs::RCKLUT_KLUT_WGT0_CHRM_KLUT_WGT0.word - 1024],
            CHROMA_KLUT_WEIGHT[0],
            "the current-motion row of the chroma weights"
        );
    }

    // ---- GDR / progressive intra refresh ----

    fn gdr_config(mode: GdrMode, num: u32, gop: u32) -> EncoderConfig {
        let mut config = config();
        config.gdr = true;
        config.gdr_mode = mode;
        config.gdr_num = num;
        config.gop = gop;
        config
    }

    /// `h264e_proc_cfg`'s `(mb_lines + n - 1) / n` over the refresh axis,
    /// capped at the GOP: 320x240 has 15 coded rows and 20 coded columns, so
    /// row refresh at 4 rows a picture is 4 pictures, column refresh is 5, and
    /// a GOP shorter than the sweep caps the sweep, not the other way round.
    #[test]
    fn gdr_refresh_length_follows_the_derivation_and_the_cap() {
        assert_eq!(gdr_config(GdrMode::Row, 4, 60).gdr_refresh_length(), 4);
        assert_eq!(gdr_config(GdrMode::Row, 4, 3).gdr_refresh_length(), 3);
        assert_eq!(gdr_config(GdrMode::Col, 4, 60).gdr_refresh_length(), 5);
        // 15 rows divided by 4 is 4 with the round-up the C's form performs.
        assert_eq!(gdr_config(GdrMode::Row, 5, 60).gdr_refresh_length(), 3);
        // Off: no sweep at all, whatever the numbers say.
        assert_eq!(config().gdr_refresh_length(), 0);
        // A zero count is refused rather than divided by.
        let zero = gdr_config(GdrMode::Row, 0, 60);
        assert!(zero.validate().is_err());
    }

    /// The first refresh band sits at row 0 and is `16 * n` tall; every later
    /// band slides to `refresh_idx * 16 * n - 32` and grows to `16 * n + 32`,
    /// the two-line overlap `setup_vepu580_intra_refresh` writes unconditionally.
    /// `set_one_roi` clamps the negative origin to row 0 and rounds the height
    /// up, so band 1 of a 2-rows-a-picture refresh covers rows 0..4.
    #[test]
    fn gdr_row_bands_slide_with_the_two_line_overlap() {
        let cfg = gdr_config(GdrMode::Row, 2, 60);
        // 320x240: mb_w 20, stride_h 20, five grid rows.
        let first = gdr_base_table(&cfg, 0);
        let record = |table: &[u8], row: usize, col: usize| -> u64 {
            let index = row * 20 + col;
            u64::from_le_bytes(table[index * 8..index * 8 + 8].try_into().unwrap())
        };
        assert_eq!(record(&first, 0, 0), (1 << 61) | (1 << 62));
        assert_eq!(record(&first, 1, 19), (1 << 61) | (1 << 62));
        assert_eq!(record(&first, 2, 0), 1 << 62, "band 0 covers rows 0..2");
        assert_eq!(record(&first, 4, 10), 1 << 62);

        // Band 1: region y = 1*32 - 32 = 0, h = 32 + 32 = 64 -> rows 0..4.
        let second = gdr_base_table(&cfg, 1);
        for row in 0..4 {
            assert_eq!(record(&second, row, 7), (1 << 61) | (1 << 62), "row {row}");
        }
        assert_eq!(record(&second, 4, 7), 1 << 62, "row 4 is below the band");

        // Band 3: y = 96 - 32 = 64 -> row 4; h = 64 -> rows 4..8, clipped by
        // the five-row grid to row 4 alone, the whole width.
        let fourth = gdr_base_table(&cfg, 3);
        assert_eq!(record(&fourth, 3, 15), 1 << 62);
        assert_eq!(record(&fourth, 4, 0), (1 << 61) | (1 << 62));
        assert_eq!(record(&fourth, 4, 19), (1 << 61) | (1 << 62));
    }

    /// `refresh_num == 1` is the corner where `16 * n - 32` goes negative, and the
    /// answer follows from `MppEncROIRegion.y` being `RK_U16`: C stores 0xFFF0, the
    /// clip then empties the band, and the picture forces nothing. Clamping the
    /// signed -16 up to row 0 instead forces two macroblock lines, which is the 836
    /// bytes the `row_n1` board cell caught (1098 against the reference's 262).
    #[test]
    fn gdr_single_line_refresh_drops_the_wrapping_origin_band() {
        let record = |table: &[u8], row: usize, col: usize| -> u64 {
            let index = row * 20 + col;
            u64::from_le_bytes(table[index * 8..index * 8 + 8].try_into().unwrap())
        };
        let band: u64 = (1 << 61) | (1 << 62);
        let base: u64 = 1 << 62;

        // Band 0: y = 0, h = 16 -> row 0 and nothing below it.
        let rows = gdr_config(GdrMode::Row, 1, 60);
        let first = gdr_base_table(&rows, 0);
        assert_eq!(record(&first, 0, 0), band);
        assert_eq!(record(&first, 0, 19), band);
        assert_eq!(record(&first, 1, 0), base, "one line a picture");

        // Band 1: y = 16 - 32 = -16 -> 0xFFF0 -> an empty range over the grid.
        let second = gdr_base_table(&rows, 1);
        for row in 0..5 {
            for col in [0usize, 7, 19] {
                assert_eq!(record(&second, row, col), base, "row {row} col {col}");
            }
        }

        // Band 2: y = 32 - 32 = 0 -> a plain band, h = 48 -> rows 0..3.
        let third = gdr_base_table(&rows, 2);
        for row in 0..3 {
            assert_eq!(record(&third, row, 12), band, "row {row}");
        }
        assert_eq!(record(&third, 3, 12), base);

        // Column mode is the same rule sideways.
        let cols = gdr_config(GdrMode::Col, 1, 60);
        let second_col = gdr_base_table(&cols, 1);
        for row in 0..5 {
            assert_eq!(record(&second_col, row, 0), base, "col refresh, band 1");
        }
    }

    /// Column refresh is the row rule turned on its side: the band spans the
    /// full height and slides along the columns, the same two-column overlap.
    #[test]
    fn gdr_column_bands_span_the_height_and_slide() {
        let cfg = gdr_config(GdrMode::Col, 2, 60);
        let table = gdr_base_table(&cfg, 1);
        let record = |table: &[u8], row: usize, col: usize| -> u64 {
            let index = row * 20 + col;
            u64::from_le_bytes(table[index * 8..index * 8 + 8].try_into().unwrap())
        };
        // x = 1*32 - 32 = 0 -> columns 0..4 across every grid row.
        for row in 0..5 {
            for col in 0..4 {
                assert_eq!(record(&table, row, col), (1 << 61) | (1 << 62));
            }
            assert_eq!(record(&table, row, 4), 1 << 62);
        }
    }

    /// The register consequences, `setup_vepu580_intra_refresh` transcribed:
    /// `roi_en` and the GDR table's `roi_addr`, the search window on the refresh
    /// axis clamped to 1, and nothing at all on a picture the schedule did not
    /// flag - the quiet picture must stay the recorded image's picture.
    #[test]
    fn gdr_refresh_frame_writes_roi_and_clamps_the_search_axis() {
        let mut cfg = gdr_config(GdrMode::Row, 4, 60);
        cfg.gop = 60;
        let quiet = registers_at(&cfg, FrameState::default().next(60, 4));
        // Frame 1: inside the first IGOP, not a refresh picture (`seq_cnt > 1`
        // gates the whole window out of the first GOP).
        assert_eq!(quiet.get(&vepu580_regs::BASE_ENC_PIC_ROI_EN).unwrap(), 0);

        // The first refresh picture: one full GOP in.
        let state = (0..60).fold(FrameState::default(), |s, _| s.next(60, 4));
        assert!(state.is_i_refresh && state.is_i_recovery && state.refresh_idx == 0);
        let mut j = job(&cfg, state);
        j.gdr_fd = 44;
        let file = build_registers(&j).unwrap();
        assert_eq!(file.get(&vepu580_regs::BASE_ENC_PIC_ROI_EN).unwrap(), 1);
        assert_eq!(file.base[178 - 160], 44, "roi_addr is the GDR table");
        assert_eq!(
            file.get(&vepu580_regs::H264_ROI_EN_ROI_QP_EN).unwrap(),
            0,
            "the refresh path never enables the QP table"
        );
        // Row refresh clamps the vertical search; the horizontal keeps the
        // picture-derived value.
        assert_eq!(file.get(&vepu580_regs::BASE_ME_RNGE_CME_SRCH_V).unwrap(), 1);
        let v_quiet = quiet.get(&vepu580_regs::BASE_ME_RNGE_CME_SRCH_V).unwrap();
        assert!(
            v_quiet > 1,
            "the quiet picture keeps the derived window ({v_quiet})"
        );

        // Column refresh clamps the other axis.
        let mut cfg_col = gdr_config(GdrMode::Col, 4, 60);
        cfg_col.gop = 60;
        let mut j = job(&cfg_col, state);
        j.gdr_fd = 45;
        let file = build_registers(&j).unwrap();
        assert_eq!(file.get(&vepu580_regs::BASE_ME_RNGE_CME_SRCH_H).unwrap(), 1);
        assert!(
            file.get(&vepu580_regs::BASE_ME_RNGE_CME_SRCH_V).unwrap() > 1,
            "column refresh leaves the vertical search to `calc_cime_parameter`"
        );
    }

    /// The reference's window does not start inside the first IGOP: `seq_cnt`
    /// is 1 until `seq_idx` reaches the IGOP, so pictures 1..gop-1 code no band,
    /// and only from picture `gop` on do the sweep pictures appear - recovery
    /// at the boundary, refresh for `refresh_length` pictures, plain P after.
    #[test]
    fn gdr_schedule_defers_the_window_past_the_first_igop() {
        let mut states = vec![FrameState::default()];
        for _ in 1..24 {
            states.push(states.last().unwrap().next(12, 4));
        }
        // Picture 0 is the session's only IDR; the IGOP boundary no longer is.
        assert!(states[0].is_idr);
        assert!(!states[12].is_idr, "the sweep replaces the IDR");
        // The first IGOP codes no band at all.
        for s in &states[1..12] {
            assert!(!s.is_i_refresh && !s.is_i_recovery);
        }
        // The second IGOP starts the sweep: refresh 0..4, recovery at 12.
        assert!(states[12].is_i_refresh && states[12].is_i_recovery);
        assert_eq!(states[12].refresh_idx, 0);
        for idx in 1..4 {
            assert!(states[12 + idx].is_i_refresh && !states[12 + idx].is_i_recovery);
            assert_eq!(states[12 + idx].refresh_idx, idx as u32);
        }
        assert!(!states[16].is_i_refresh && !states[16].is_i_recovery);
        // And so on, one full frame_num/poc run per IGOP, no reset.
        assert_eq!(states[12].frame_num, 12);
        assert_eq!(states[23].poc, 46);
        assert!(!states[23].is_i_refresh);
    }

    // ---- slice split ----

    /// `setup_vepu580_split` with `split_out = 0`: BY_CTU is the cnum rule and
    /// BY_BYTE the byte rule, both with `sli_max_num_m1 = 500` and `sli_flsh = 1`,
    /// and `slc_done_en` armed unconditionally for BY_BYTE, only past the
    /// 32-entry slice FIFO for BY_CTU. `slen_fifo` stays down - that bit feeds
    /// the per-slice output modes this driver does not run.
    #[test]
    fn split_modes_write_the_reference_combinations() {
        let mut cfg = config();
        cfg.split_mode = SplitMode::ByCtu;
        cfg.split_arg = 96;
        let file = registers_at(&cfg, FrameState::default());
        assert_eq!(file.get(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT).unwrap(), 1);
        assert_eq!(
            file.get(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT_MODE)
                .unwrap(),
            1
        );
        assert_eq!(
            file.get(&vepu580_regs::BASE_SLI_SPLT_SLI_MAX_NUM_M1)
                .unwrap(),
            500
        );
        assert_eq!(file.get(&vepu580_regs::BASE_SLI_SPLT_SLI_FLSH).unwrap(), 1);
        assert_eq!(
            file.get(&vepu580_regs::BASE_SLI_CNUM_SLI_SPLT_CNUM_M1)
                .unwrap(),
            95
        );
        assert_eq!(
            file.get(&vepu580_regs::BASE_SLI_BYTE_SLI_SPLT_BYTE)
                .unwrap(),
            0
        );
        assert_eq!(file.get(&vepu580_regs::BASE_ENC_PIC_SLEN_FIFO).unwrap(), 0);
        // 300 macroblocks over 96 is 4 slices - under the FIFO, no slice IRQ.
        assert_eq!(file.get(&vepu580_regs::CTL_INT_EN_SLC_DONE_EN).unwrap(), 0);

        // Past 32 slices the same mode arms the interrupt instead.
        let mut tight = config();
        tight.split_mode = SplitMode::ByCtu;
        tight.split_arg = 8;
        let file = registers_at(&tight, FrameState::default());
        assert_eq!(file.get(&vepu580_regs::CTL_INT_EN_SLC_DONE_EN).unwrap(), 1);

        cfg.split_mode = SplitMode::ByByte;
        cfg.split_arg = 1024;
        let file = registers_at(&cfg, FrameState::default());
        assert_eq!(file.get(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT).unwrap(), 1);
        assert_eq!(
            file.get(&vepu580_regs::BASE_SLI_SPLT_SLI_SPLT_MODE)
                .unwrap(),
            0
        );
        assert_eq!(
            file.get(&vepu580_regs::BASE_SLI_SPLT_SLI_MAX_NUM_M1)
                .unwrap(),
            500
        );
        assert_eq!(file.get(&vepu580_regs::BASE_SLI_SPLT_SLI_FLSH).unwrap(), 1);
        assert_eq!(
            file.get(&vepu580_regs::BASE_SLI_CNUM_SLI_SPLT_CNUM_M1)
                .unwrap(),
            0
        );
        assert_eq!(
            file.get(&vepu580_regs::BASE_SLI_BYTE_SLI_SPLT_BYTE)
                .unwrap(),
            1024
        );
        assert_eq!(file.get(&vepu580_regs::CTL_INT_EN_SLC_DONE_EN).unwrap(), 1);
    }

    #[test]
    fn low_delay_split_arms_the_slice_fifo_for_both_byte_and_ctu_modes() {
        for (mode, arg) in [(SplitMode::ByCtu, 96), (SplitMode::ByByte, 1024)] {
            let mut cfg = config();
            cfg.split_mode = mode;
            cfg.split_arg = arg;
            cfg.split_out = encoder::MPP_ENC_SPLIT_OUT_LOWDELAY;
            let file = registers_at(&cfg, FrameState::default());
            assert_eq!(file.get(&vepu580_regs::BASE_ENC_PIC_SLEN_FIFO).unwrap(), 1);
            assert_eq!(file.get(&vepu580_regs::CTL_INT_EN_SLC_DONE_EN).unwrap(), 1);
        }
    }

    #[test]
    fn output_packets_preserve_low_delay_slice_layout() {
        let payload: Vec<u8> = (0..9).collect();
        let packets = output_packets(&[0xaa, 0xbb], &[0xcc], &payload, &[4, 5, 0]).unwrap();
        assert_eq!(
            packets,
            vec![
                vec![0xaa, 0xbb, 0xcc, 0, 1, 2, 3],
                vec![0xcc, 4, 5, 6, 7, 8],
                Vec::<u8>::new(),
            ]
        );

        let whole = output_packets(&[0xaa], &[0xcc], &payload, &[]).unwrap();
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].len(), payload.len() + 2);
        assert_eq!(whole[0][0], 0xaa);
        assert_eq!(whole[0][1], 0xcc);
        assert_eq!(&whole[0][2..], payload);
    }

    #[test]
    fn output_packets_reject_a_partial_slice_sum() {
        let error = output_packets(&[], &[], &[0; 9], &[4, 4]).unwrap_err();
        assert!(error.contains("sum to 8"));
        assert!(error.contains("STATUS says 9"));
    }

    #[test]
    fn tsvc_prefix_matches_the_reference_bit_layout() {
        assert_eq!(
            svc_prefix_nal(true, 3, 0),
            [0, 0, 0, 1, 0x6e, 0xc0, 0x80, 0x07, 0x20]
        );
        assert_eq!(
            svc_prefix_nal(false, 0, 1),
            [0, 0, 0, 1, 0x0e, 0x81, 0x80, 0x27]
        );
        assert_eq!(
            svc_prefix_nal(false, 2, 0),
            [0, 0, 0, 1, 0x4e, 0x80, 0x80, 0x07, 0x20]
        );
    }

    #[test]
    fn slice_prefix_is_repeated_before_each_non_empty_low_delay_slice() {
        let prefixed = prefix_slices(&[1, 2, 3, 4, 5], &[2, 3, 0], &[0xaa]).unwrap();
        assert_eq!(prefixed, [0xaa, 1, 2, 0xaa, 3, 4, 5]);
    }

    #[test]
    fn split_none_writes_only_what_the_recorded_image_already_has() {
        let file = registers_at(&config(), FrameState::default());
        for field in [
            &vepu580_regs::BASE_SLI_SPLT_SLI_SPLT,
            &vepu580_regs::BASE_SLI_SPLT_SLI_SPLT_MODE,
            &vepu580_regs::BASE_SLI_SPLT_SLI_SPLT_CPST,
            &vepu580_regs::BASE_SLI_SPLT_SLI_MAX_NUM_M1,
            &vepu580_regs::BASE_SLI_SPLT_SLI_FLSH,
            &vepu580_regs::BASE_SLI_CNUM_SLI_SPLT_CNUM_M1,
            &vepu580_regs::BASE_SLI_BYTE_SLI_SPLT_BYTE,
            &vepu580_regs::BASE_ENC_PIC_SLEN_FIFO,
            &vepu580_regs::CTL_INT_EN_SLC_DONE_EN,
        ] {
            assert_eq!(file.get(field).unwrap(), 0, "{}", field.name);
        }
    }

    #[test]
    fn split_arguments_are_bounded_by_their_registers() {
        let mut cfg = config();
        cfg.split_mode = SplitMode::ByCtu;
        for arg in [0u32, 1_048_577] {
            cfg.split_arg = arg;
            assert!(cfg.validate().is_err(), "mb count {arg}");
        }
        cfg.split_arg = 300;
        assert!(cfg.validate().is_ok());
        cfg.split_mode = SplitMode::ByByte;
        for arg in [0u32, 1_048_576] {
            cfg.split_arg = arg;
            assert!(cfg.validate().is_err(), "byte target {arg}");
        }
        cfg.split_arg = 1024;
        assert!(cfg.validate().is_ok());

        cfg.split_out = encoder::MPP_ENC_SPLIT_OUT_LOWDELAY;
        assert!(cfg.validate().is_ok());
        cfg.split_out = 1 << 1;
        assert!(cfg.validate().is_err(), "SEGMENT output is not implemented");
        cfg.split_out = encoder::MPP_ENC_SPLIT_OUT_LOWDELAY;
        cfg.split_mode = SplitMode::None;
        assert!(cfg.validate().is_err(), "split_out needs a split mode");
    }

    // ---- >4096 CIME extension lines ----

    /// `setup_hal_bufs`: the decision is on the width aligned to 64 clearing
    /// 4096, but the size counts 480 bytes per 64-pixel column of the *whole*
    /// width beyond 3072 - `(aligned_w - 3*1024) / 64 * 480`, the C's
    /// division-first order. 4100 aligns to 4160: seventeen columns past 3072,
    /// 8160 bytes, even though only one column is past 4096.
    #[test]
    fn ext_line_size_is_480_bytes_per_column_beyond_3072() {
        assert_eq!(ext_line_buf_size(&EncoderConfig::new(4096, 16)), 0);
        // A width aligned to 64 exactly at 4096 does not reach the buffer.
        assert_eq!(ext_line_buf_size(&EncoderConfig::new(4090, 16)), 0);
        assert_eq!(ext_line_buf_size(&EncoderConfig::new(4098, 16)), 8160);
        assert_eq!(ext_line_buf_size(&EncoderConfig::new(4100, 16)), 8160);
        assert_eq!(ext_line_buf_size(&EncoderConfig::new(4160, 64)), 8160);
        // 4608 -> (4608 - 3072) / 64 * 480 = 24 * 480.
        assert_eq!(ext_line_buf_size(&EncoderConfig::new(4608, 256)), 11_520);
    }

    /// The register side: both extension registers take the fd, the offset
    /// table gets the second half's start under reg 182 - `ebuft` and `ebufb`
    /// share one allocation - and the RCB pair says "183 knows the size, 182
    /// is the base". A width below 4096 carries none of it.
    #[test]
    fn ext_line_registers_offsets_and_rcb_are_wired() {
        let wide = EncoderConfig::new(4608, 256);
        // The >4096 refusal is gone: the geometry reaches the register pass.
        assert!(validate_reference_config(&wide).is_ok());
        let mut j = job(&wide, FrameState::default());
        j.ext_fd = 77;
        j.ext_offset = ext_line_buf_size(&wide) as u32;
        j.output_capacity = logical_output_size(&wide).unwrap();
        let file = build_registers(&j).unwrap();
        assert_eq!(file.base[182 - 160], 77, "ebuft_addr");
        assert_eq!(file.base[183 - 160], 77, "ebufb_addr");
        assert!(
            file.offsets
                .iter()
                .any(|o| o.reg_idx == 182 && o.offset == 11_520),
            "the ebufb half's offset entry"
        );
        let rcb: Vec<(u32, u32)> = file.rcb.iter().map(|r| (r.reg_idx, r.size)).collect();
        assert_eq!(rcb, vec![(183, 11_520), (182, 0)], "the C's two RCB calls");

        let narrow_cfg = config();
        let narrow = job(&narrow_cfg, FrameState::default());
        let file = build_registers(&narrow).unwrap();
        assert_eq!(file.base[182 - 160], 0);
        assert_eq!(file.base[183 - 160], 0);
        assert!(file.rcb.is_empty());
        assert!(
            !file.offsets.iter().any(|o| o.reg_idx == 182),
            "no reg182 entry below the threshold"
        );
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
                FrameState::default().next(config.gop, 0)
            },
            output_capacity: logical_output_size(&config).unwrap(),
            tune: quiet_tune(),
            roi_fd: -1,
            roi_qp_fd: -1,
            gdr_fd: -1,
            ext_fd: -1,
            ext_offset: 0,
            save_pass1: false,
            use_pass1: false,
            non_reference: false,
            current_long_term: false,
            pass1_fd: -1,
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
            tune: quiet_tune(),
            roi_fd: -1,
            roi_qp_fd: -1,
            gdr_fd: -1,
            ext_fd: -1,
            ext_offset: 0,
            save_pass1: false,
            use_pass1: false,
            non_reference: false,
            current_long_term: false,
            pass1_fd: -1,
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
