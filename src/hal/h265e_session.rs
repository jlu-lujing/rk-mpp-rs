//! VEPU580 H.265 encoder session: one fixed-QP IDR, submitted to the hardware.
//!
//! This is the first path that actually submits an H.265 job. It owns every
//! surface the job points at - input, output, two FBC reconstructions (pixel,
//! display thumb, motion-vector thumb), and the motion-info buffer the C harness
//! passes - so the address words in `h265e`'s image are ours rather than the
//! oracle's.
//!
//! The layout and the multi-offset table are `vepu580_h265_set_hw_address`:
//! registers 160..162 are the input plane (one fd, chroma reached through the
//! 161/162 offsets), 163/164 the reconstructed surface (164 offset by the FBC
//! header length), 165/166 the reference, 167..170 the motion-vector and display
//! thumbs, 171 the motion-info buffer, and 172..175 the bitstream buffer.

use crate::service::dma_heap::DmaBuf;
use crate::service::{Batch, Client, MppService, RegOffsetInfo};

use super::h265e::{build_registers, IdrJob, Picture};
use super::h265e_regs as regs;

const CTL_OFFSET: u32 = 0;
const BASE_OFFSET: u32 = 640;
const RCKLUT_OFFSET: u32 = 4096;
const WGT_OFFSET: u32 = 5888;
const RDO_OFFSET: u32 = 8192;
const OSD_OFFSET: u32 = 12288;
const STATUS_OFFSET: u32 = 16384;
/// `VEPU580_REG_BASE_HW_STATUS` = 0x2c bytes.
const HW_STATUS_OFFSET: u32 = 44;
const STATUS_WORDS: usize = 180;
const HW_ERROR_MASK: u32 = 0x3f0;
const OUTPUT_MARGIN: usize = 4096;
/// `h265e_init`'s intra GOP default, which the harness keeps. The rate-control SEI
/// prints it as `-g:60`, so a different value would change the prefix bytes.
const DEFAULT_IGOP: u32 = 60;

/// The three buffers of one DPB slot: the FBC pixel surface (header + body), the
/// display thumb, and the motion-vector thumb.
struct Surface {
    pixel: DmaBuf,
    dsp: DmaBuf,
    cmv: DmaBuf,
}

impl Surface {
    fn alloc(sizes: (usize, usize, usize)) -> Result<Self, String> {
        Ok(Self {
            pixel: DmaBuf::alloc("system", sizes.0, false)
                .map_err(|e| format!("allocate H.265 reconstruction: {e}"))?,
            dsp: DmaBuf::alloc("system", sizes.1, false)
                .map_err(|e| format!("allocate H.265 display thumb: {e}"))?,
            cmv: DmaBuf::alloc("system", sizes.2, false)
                .map_err(|e| format!("allocate H.265 motion-vector thumb: {e}"))?,
        })
    }
}

pub struct Vepu580H265 {
    service: MppService,
    width: usize,
    height: usize,
    qp: u32,
    /// Emit the GOP SPS variant (long-term reference entry) and let `encode_frame`
    /// produce P pictures.
    long_term: bool,
    input: DmaBuf,
    output: DmaBuf,
    /// Two DPB slots. Slot 0 is the opening IDR's reconstruction; the next picture
    /// reconstructs into the other slot and reads slot 0 as its reference.
    surfaces: [Surface; 2],
    slot: usize,
    /// Pictures this session has encoded; 0 means the next one is the IDR.
    frames: u32,
    md_info: DmaBuf,
}

impl Vepu580H265 {
    /// A single-picture session: the IDR gate.
    pub fn open(width: usize, height: usize, qp: u32) -> Result<Self, String> {
        Self::open_inner(width, height, qp, false)
    }

    /// A GOP session: the SPS carries the long-term reference entry the harness
    /// writes when it is given an explicit `-g`, and later pictures are P.
    pub fn open_gop(width: usize, height: usize, qp: u32) -> Result<Self, String> {
        Self::open_inner(width, height, qp, true)
    }

    fn open_inner(width: usize, height: usize, qp: u32, long_term: bool) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("H.265 session needs a non-zero picture".into());
        }
        if qp > 51 {
            return Err(format!("H.265 QP {qp} is outside 0..=51"));
        }
        let sizes = surface_sizes(width, height);
        let input_size = luma_bytes(width, height)
            .checked_mul(3)
            .map(|v| v / 2)
            .ok_or_else(|| "H.265 input size overflow".to_string())?;
        let output_size = input_size
            .checked_add(OUTPUT_MARGIN)
            .ok_or_else(|| "H.265 output size overflow".to_string())?;
        Ok(Self {
            service: MppService::open(Client::RkvEnc as u32)
                .map_err(|e| format!("open rkv encoder: {e}"))?,
            width,
            height,
            qp,
            long_term,
            input: DmaBuf::alloc("system", input_size, false)
                .map_err(|e| format!("allocate H.265 input: {e}"))?,
            output: DmaBuf::alloc("system", output_size, false)
                .map_err(|e| format!("allocate H.265 output: {e}"))?,
            surfaces: [Surface::alloc(sizes)?, Surface::alloc(sizes)?],
            slot: 0,
            frames: 0,
            md_info: DmaBuf::alloc("system", md_info_size(width, height), false)
                .map_err(|e| format!("allocate H.265 motion info: {e}"))?,
        })
    }

    /// Encode the next picture: the first call is an IDR, every later call a P.
    /// Only the opening IDR carries the VPS/SPS/PPS and the two SEIs, which is what
    /// `MppEncHeaderStatus` makes the C do once per sequence.
    pub fn encode_frame(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        let (picture, prefix) = if self.frames == 0 {
            (Picture::Idr, true)
        } else {
            let poc = self.frames;
            (Picture::P { poc_lsb: poc }, false)
        };
        let stream = self.encode_picture(nv12, picture, prefix)?;
        self.slot = 1 - self.slot;
        self.frames += 1;
        Ok(stream)
    }

    /// One IDR and nothing else: the board gate's entry point.
    pub fn encode_idr(&mut self, nv12: &[u8]) -> Result<Vec<u8>, String> {
        assert_eq!(self.frames, 0, "encode_idr is the opening picture");
        let stream = self.encode_picture(nv12, Picture::Idr, true)?;
        self.frames = 1;
        Ok(stream)
    }

    fn encode_picture(
        &mut self,
        nv12: &[u8],
        picture: Picture,
        with_prefix: bool,
    ) -> Result<Vec<u8>, String> {
        // dma-heap rounds the allocation up to a page, so the usable picture is
        // the logical size, not `self.input.len()`.
        let input_size = luma_bytes(self.width, self.height) * 3 / 2;
        if nv12.len() < input_size {
            return Err(format!(
                "H.265 input is {} bytes, the {}x{} surface needs {}",
                nv12.len(),
                self.width,
                self.height,
                input_size
            ));
        }
        self.input
            .sync_write_begin()
            .map_err(|e| format!("H.265 input sync: {e}"))?;
        self.input
            .as_mut_slice()
            .get_mut(..input_size)
            .ok_or_else(|| "H.265 input buffer shorter than the picture".to_string())?
            .copy_from_slice(&nv12[..input_size]);
        self.input
            .sync_write_end()
            .map_err(|e| format!("H.265 input flush: {e}"))?;

        // The GOP's long-term reference is the opening IDR (`used_by_lt_flg0` and
        // the SPS LT entry): every P predicts from slot 0 and reconstructs into
        // slot 1. An IDR reconstructs into and reads from slot 0.
        let (current, reference) = if matches!(picture, Picture::Idr) {
            (&self.surfaces[self.slot], &self.surfaces[self.slot])
        } else {
            (&self.surfaces[1], &self.surfaces[0])
        };
        let job = IdrJob {
            width: self.width,
            height: self.height,
            qp: self.qp,
            qp_min: self.qp,
            qp_max: self.qp,
            picture,
            rc: None,
            input_fd: self.input.fd(),
            output_fd: self.output.fd(),
            current_fd: current.pixel.fd(),
            current_thumb_fd: current.dsp.fd(),
            reference_fd: reference.pixel.fd(),
            reference_thumb_fd: reference.dsp.fd(),
        };
        let mut registers = build_registers(&job)?;
        let header = fbc_header_len(self.width, self.height);
        let luma = luma_bytes(self.width, self.height) as u32;

        // `vepu580_h265_set_hw_address`: the input plane shares one fd and reaches
        // its chroma through the 161/162 offsets; the two FBC surfaces reach their
        // body through the header offset; 167..170 are the thumbs; 171 is the
        // motion-info buffer this session owns (the C harness passes one, which is
        // why the traced image has `mei_stor` set).
        let base = &mut registers.base;
        let input = self.input.fd() as u32;
        let output = self.output.fd() as u32;
        base[0] = input; // 160 adr_src0
        base[1] = input; // 161 adr_src1
        base[2] = input; // 162 adr_src2
        base[3] = current.pixel.fd() as u32; // 163 rfpw_h
        base[4] = current.pixel.fd() as u32; // 164 rfpw_b
        base[5] = reference.pixel.fd() as u32; // 165 rfpr_h
        base[6] = reference.pixel.fd() as u32; // 166 rfpr_b
        base[7] = current.cmv.fd() as u32; // 167 cmvw
        base[8] = reference.cmv.fd() as u32; // 168 cmvr
        base[9] = current.dsp.fd() as u32; // 169 dspw
        base[10] = reference.dsp.fd() as u32; // 170 dspr
        base[11] = self.md_info.fd() as u32; // 171 meiw
        base[12] = output; // 172 bsbt
        base[13] = output; // 173 bsbb
        base[14] = output; // 174 bsbr
        base[15] = output; // 175 adr_bsbs
        registers.set(&regs::BASE_ENC_PIC_MEI_STOR, 1)?;

        registers.offsets = vec![
            RegOffsetInfo {
                reg_idx: 161,
                offset: luma,
            },
            RegOffsetInfo {
                reg_idx: 162,
                offset: luma,
            },
            RegOffsetInfo {
                reg_idx: 164,
                offset: header as u32,
            },
            RegOffsetInfo {
                reg_idx: 166,
                offset: header as u32,
            },
            RegOffsetInfo {
                reg_idx: 175,
                offset: 0,
            },
            RegOffsetInfo {
                reg_idx: 172,
                offset: self.output.len() as u32,
            },
        ];

        let mut status = vec![0u32; STATUS_WORDS];
        let mut hw_status = [0u32; 1];
        let mut batch = Batch::new();
        batch.reg_write(&registers.ctl, CTL_OFFSET);
        batch.reg_write(&registers.base, BASE_OFFSET);
        batch.reg_write(&registers.rc_klut, RCKLUT_OFFSET);
        batch.reg_write(&registers.wgt, WGT_OFFSET);
        batch.reg_write(&registers.rdo, RDO_OFFSET);
        batch.reg_write(&registers.osd, OSD_OFFSET);
        batch.reg_offsets(&registers.offsets);
        batch.reg_read(&mut hw_status, HW_STATUS_OFFSET);
        batch.reg_read(&mut status, STATUS_OFFSET);
        self.service
            .submit_batch(&mut batch)
            .map_err(|e| format!("H.265 register submit: {e}"))?;
        self.service
            .poll_hw_finish()
            .map_err(|e| format!("H.265 poll: {e}"))?;

        let errors = hw_status[0] & HW_ERROR_MASK;
        if errors != 0 {
            return Err(format!(
                "VEPU580 H.265 hardware error mask 0x{errors:03x} (int_sta=0x{:08x})",
                hw_status[0]
            ));
        }
        if hw_status[0] & 1 == 0 {
            return Err(format!(
                "VEPU580 H.265 completed ioctl without enc_done (int_sta=0x{:08x})",
                hw_status[0]
            ));
        }

        let length = status[0] as usize;
        if length == 0 || length > self.output.len() {
            return Err(format!(
                "invalid VEPU580 H.265 length {length} (capacity {})",
                self.output.len()
            ));
        }
        self.output
            .sync_read_begin()
            .map_err(|e| format!("H.265 output invalidate: {e}"))?;
        let slice = self.output.as_slice()[..length].to_vec();
        self.output
            .sync_read_end()
            .map_err(|e| format!("H.265 output sync end: {e}"))?;
        // The hardware writes the slice NAL; the parameter sets and the two SEIs are
        // software output, exactly as `mpp_enc_impl.cpp` prepends them - once, ahead
        // of the opening IDR.
        if !with_prefix {
            return Ok(slice);
        }
        let mut config = crate::codec::h265::encoder::EncoderConfig::new(self.width, self.height);
        config.long_term_refs = self.long_term;
        let mut stream = crate::codec::h265::encoder::build_prefix(&config, self.qp, DEFAULT_IGOP)
            .map_err(|e| format!("H.265 prefix: {e:?}"))?;
        stream.extend_from_slice(&slice);
        Ok(stream)
    }
}

fn align(value: usize, to: usize) -> usize {
    (value + to - 1) & !(to - 1)
}

/// `MPP_ALIGN(width, 16) * MPP_ALIGN(height, 16)`: the luma plane of the input,
/// which is also the u and v offset of a 4:2:0 semi-planar frame.
fn luma_bytes(width: usize, height: usize) -> usize {
    align(width, 16) * align(height, 16)
}

/// `vepu580_h265_get_buffer`'s three sizes. The block count adds a row
/// (`mb_h64 = (h + 63) / 64 + 1`), which is what makes the 320x240 header 8192:
/// 5x5 blocks of 64 bytes rounded up to 8 KiB.
fn surface_sizes(width: usize, height: usize) -> (usize, usize, usize) {
    let blocks = ((width + 63) / 64) * ((height + 63) / 64 + 1);
    let header = fbc_header_len(width, height);
    let body = (blocks << 12) * 3 / 2;
    (header + body, blocks << 8, align(blocks * 16 * 6, 256))
}

fn fbc_header_len(width: usize, height: usize) -> usize {
    let blocks = ((width + 63) / 64) * ((height + 63) / 64 + 1);
    align(blocks << 6, 8192)
}

/// `p->mdinfo_size` for HEVC: one 16-byte record per 32x32 block.
fn md_info_size(width: usize, height: usize) -> usize {
    (align(width, 32) >> 5) * (align(height, 32) >> 5) * 16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_captured_geometry_gets_the_caps_sizes() {
        // 320x240: 5x5 blocks of 64 -> header 8192, body 153600, pixel 161792,
        // display thumb 6400, mv thumb 2560 (C `vepu580_h265_get_buffer`).
        assert_eq!(fbc_header_len(320, 240), 8192);
        assert_eq!(surface_sizes(320, 240), (161792, 6400, 2560));
        assert_eq!(luma_bytes(320, 240), 76800);
        assert_eq!(md_info_size(320, 240), 1280);
    }

    #[test]
    fn odd_sizes_round_the_way_the_hal_rounds() {
        assert_eq!(luma_bytes(322, 242), 336 * 256);
        assert_eq!(md_info_size(322, 242), (352 >> 5) * (256 >> 5) * 16);
        // 11x13 blocks = 143; 143*64 = 9152 -> 16384.
        assert_eq!(fbc_header_len(640, 736), 16384);
    }
}
