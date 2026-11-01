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

use crate::codec::h264::encoder::{self, EncoderConfig};
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

const REG_SRC_1_OFFSET: u32 = 161;
const REG_SRC_2_OFFSET: u32 = 162;
const REG_BSBT_END: u32 = 172;
const REG_BSBS_START: u32 = 175;
const REG_RFPW_B: u32 = 164;
const REG_RFPR_B: u32 = 166;

const FBC_HEADER_SIZE: usize = 8192;
const STATUS_WORDS: usize = 180;
const HW_ERROR_MASK: u32 = 0x3f0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedIdr {
    /// Complete Annex-B access unit: SPS, PPS and the hardware IDR.
    pub annex_b: Vec<u8>,
    /// Hardware-produced NAL, including its start code.
    pub payload: Vec<u8>,
    pub headers: Vec<u8>,
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
    fn apply_fixed_qp(&mut self, qp: u32) -> Result<(), String> {
        self.set(&vepu580_regs::BASE_ENC_PIC_PIC_QP, qp)?;
        self.set(&vepu580_regs::BASE_RC_QP_RC_MIN_QP, qp)?;
        self.set(&vepu580_regs::BASE_RC_QP_RC_MAX_QP, qp)?;
        Ok(())
    }
}

pub struct Vepu580H264 {
    svc: MppService,
    config: EncoderConfig,
    input: DmaBuf,
    output: DmaBuf,
    reconstruction: DmaBuf,
    thumb: DmaBuf,
    encoded: bool,
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
            input: DmaBuf::alloc("system", input_size, false)
                .map_err(|e| format!("allocate H.264 input: {e}"))?,
            output: DmaBuf::alloc("system", output_size, false)
                .map_err(|e| format!("allocate H.264 output: {e}"))?,
            reconstruction: DmaBuf::alloc("system", pixel_size, false)
                .map_err(|e| format!("allocate H.264 reconstruction: {e}"))?,
            thumb: DmaBuf::alloc("system", thumb_size, false)
                .map_err(|e| format!("allocate H.264 thumb: {e}"))?,
            config,
            encoded: false,
        })
    }

    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    pub fn headers(&self) -> Result<Vec<u8>, String> {
        encoder::build_headers(&self.config).map_err(|e| e.to_string())
    }

    pub fn encode_idr(&mut self, nv12: &[u8]) -> Result<EncodedIdr, String> {
        if self.encoded {
            return Err("the current VEPU580 bootstrap path supports one IDR per encoder".into());
        }
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

        let registers = build_registers(
            &self.config,
            self.input.fd(),
            self.output.fd(),
            self.reconstruction.fd(),
            self.thumb.fd(),
            logical_output_size(&self.config)?,
        )?;

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
        let nal_validation = validate_nal_bytes(&payload);
        self.output
            .sync_read_end()
            .map_err(|e| format!("H.264 output sync end: {e}"))?;
        nal_validation?;

        let headers = self.headers()?;
        let mut annex_b = Vec::with_capacity(headers.len() + payload.len());
        annex_b.extend_from_slice(&headers);
        annex_b.extend_from_slice(&payload);
        self.encoded = true;
        Ok(EncodedIdr {
            annex_b,
            payload,
            headers,
        })
    }
}

pub fn build_registers(
    config: &EncoderConfig,
    input_fd: i32,
    output_fd: i32,
    reconstruction_fd: i32,
    thumb_fd: i32,
    output_capacity: usize,
) -> Result<RegisterFile, String> {
    validate_reference_config(config)?;
    if input_fd < 0 || output_fd < 0 || reconstruction_fd < 0 || thumb_fd < 0 {
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
    base[BASE_RFPR_H] = recon;
    base[BASE_RFPR_B] = recon;
    base[BASE_DSPW] = thumb;
    base[BASE_DSPR] = thumb;
    base[BASE_BSBT] = output;
    base[BASE_BSBB] = output;
    base[BASE_BSBR] = output;
    base[BASE_ADR_BSBS] = output;
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
    file.apply_fixed_qp(u32::from(config.qp_init))?;
    // The reference job was captured with a harness that asked for motion
    // derivation info, so the recorded image has `enc_pic.mei_stor` set. Writing
    // side information needs the MD buffer `setup_hal_bufs` allocates for it,
    // which this driver does not implement; leaving the enable on without that
    // buffer would have the hardware write to a surface nobody owns.
    file.set(&vepu580_regs::BASE_ENC_PIC_MEI_STOR, 0)?;
    Ok(file)
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
    Ok(())
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
    if config.profile != encoder::PROFILE_BASELINE {
        return Err("VEPU580 bootstrap supports Baseline profile only".into());
    }
    if config.qp_init > 51 {
        return Err(format!("H.264 QP {} is outside 0..=51", config.qp_init));
    }
    if config.level_idc != 30 || config.gop != 1 || config.entropy_coding || config.transform_8x8 {
        return Err(
            "the VEPU580 register image is still recorded at level 3.0, GOP 1, CAVLC and no \
             transform-8x8; those remain fixed"
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

fn input_size(config: &EncoderConfig) -> Result<usize, String> {
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

fn validate_nal_bytes(data: &[u8]) -> Result<(), String> {
    let valid = data
        .windows(4)
        .position(|window| window == [0, 0, 0, 1])
        .map(|offset| {
            data.get(offset + 4)
                .is_some_and(|header| header & 0x1f == 5)
        })
        .unwrap_or(false);
    if !valid {
        return Err("VEPU580 output does not contain an Annex-B IDR NAL".into());
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
        let registers = build_registers(&config(), 7, 9, 11, 13, 119_296).unwrap();
        assert_eq!(
            registers.get(&vepu580_regs::BASE_ENC_PIC_MEI_STOR).unwrap(),
            0,
            "MD info is not implemented, so the enable is cleared"
        );
    }

    #[test]
    fn dma_addresses_and_mpp_offsets_are_rebuilt() {
        let registers = build_registers(&config(), 7, 9, 11, 13, 119_296).unwrap();
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
            let registers = build_registers(&config, 1, 2, 3, 4, output_size).unwrap();
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
            assert!(build_registers(
                &config,
                1,
                2,
                3,
                4,
                config.coded_width() * config.coded_height() * 2
            )
            .is_err());
        }
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
        let base = words_of(&build_registers(&config(), 7, 9, 11, 13, 119_296).unwrap());
        for qp in [0u8, 12, 20, 26, 33, 45, 51] {
            let mut at_qp = config();
            at_qp.qp_init = qp;
            let file = build_registers(&at_qp, 7, 9, 11, 13, 119_296).unwrap();
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
                build_registers(&at_qp, 1, 2, 3, 4, 119_296).is_ok(),
                "QP {qp} generates"
            );
            assert!(verified_on_board(&at_qp).is_ok(), "QP {qp} is accepted");
        }
        let mut too_high = config();
        too_high.qp_init = 52;
        assert!(too_high.validate().is_err(), "QP 52 is not an H.264 QP");

        let mut cabac = config();
        cabac.entropy_coding = true;
        assert!(build_registers(&cabac, 1, 2, 3, 4, 119_296).is_err());
        let mut gop = config();
        gop.gop = 30;
        assert!(build_registers(&gop, 1, 2, 3, 4, 119_296).is_err());
    }
}
