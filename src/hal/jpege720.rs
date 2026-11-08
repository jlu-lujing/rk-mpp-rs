//! VPU720 JPEG encoder for RK3576.
//!
//! The marker writer is shared with the VEPU2 encoder. VPU720 has a separate
//! 43-word base register window, a 384-byte transformed Q-table, and reports
//! the encoded ECS bit length through its status window.

use std::io;

use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{require_jpeg_encode, JpegEncodeHal};
use crate::service::{Batch, Client, MppService, RegOffsetInfo};

use super::jpege::{
    prepare_header, ChromaFormat, EncodedJpeg, EncoderConfig, InputFormat, PreparedHeader,
    Rotation, Vepu2Jpeg,
};

pub enum JpegEncoder {
    Vepu2(Vepu2Jpeg),
    Vpu720(Vpu720Jpeg),
}

impl JpegEncoder {
    pub fn open() -> io::Result<Self> {
        let profile = crate::service::soc::SocProfile::detect()?;
        match profile.jpeg_encode {
            JpegEncodeHal::Vepu2 => Ok(Self::Vepu2(Vepu2Jpeg::open()?)),
            JpegEncodeHal::Vpu720 => Ok(Self::Vpu720(Vpu720Jpeg::open()?)),
            other => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("no JPEG encode HAL for {other:?}"),
            )),
        }
    }

    pub fn encode_packets(
        &mut self,
        input: &[u8],
        config: EncoderConfig,
    ) -> Result<EncodedJpeg, String> {
        match self {
            Self::Vepu2(encoder) => encoder.encode_packets(input, config),
            Self::Vpu720(encoder) => encoder.encode_packets(input, config),
        }
    }

    pub fn encode_nv12(&mut self, input: &[u8], config: EncoderConfig) -> Result<Vec<u8>, String> {
        match self {
            Self::Vepu2(encoder) => encoder.encode_nv12(input, config),
            Self::Vpu720(encoder) => encoder.encode_nv12(input, config),
        }
    }

    pub fn encode(&mut self, input: &[u8], config: EncoderConfig) -> Result<Vec<u8>, String> {
        match self {
            Self::Vepu2(encoder) => encoder.encode(input, config),
            Self::Vpu720(encoder) => encoder.encode(input, config),
        }
    }
}

const QTABLE_SIZE: usize = 64 * 3;
const BASE_REG_COUNT: usize = 43;
const STATUS_REG_COUNT: usize = 31;
const INT_STATE_OFFSET: usize = 0x1c;
const STATUS_OFFSET: usize = 0xc0;
const HW_ERROR_MASK: u32 = 0x170;

pub struct Vpu720Jpeg {
    svc: MppService,
    input: Option<DmaBuf>,
    output: Option<DmaBuf>,
    qtable: DmaBuf,
}

impl Vpu720Jpeg {
    pub fn open() -> io::Result<Self> {
        require_jpeg_encode(JpegEncodeHal::Vpu720)?;
        Ok(Self {
            svc: MppService::open(Client::JpegEnc as u32)?,
            input: None,
            output: None,
            qtable: DmaBuf::alloc("system", QTABLE_SIZE * 2, false)?,
        })
    }

    pub fn encode_nv12(&mut self, input: &[u8], config: EncoderConfig) -> Result<Vec<u8>, String> {
        self.encode_nv12_packets(input, config)
            .map(|encoded| encoded.bytes)
    }

    pub fn encode_nv12_packets(
        &mut self,
        input: &[u8],
        config: EncoderConfig,
    ) -> Result<EncodedJpeg, String> {
        if config.input != InputFormat::Nv12 || config.chroma != ChromaFormat::Yuv420 {
            return Err("VPU720 initial path requires NV12 input and 4:2:0 output".into());
        }
        self.encode_packets(input, config)
    }

    pub fn encode(&mut self, input: &[u8], config: EncoderConfig) -> Result<Vec<u8>, String> {
        self.encode_packets(input, config)
            .map(|encoded| encoded.bytes)
    }

    pub fn encode_packets(
        &mut self,
        input: &[u8],
        config: EncoderConfig,
    ) -> Result<EncodedJpeg, String> {
        validate_config(config)?;
        let input_size = config.input_size()?;
        if input.len() < input_size {
            return Err(format!(
                "JPEG input is {} bytes, expected at least {input_size}",
                input.len()
            ));
        }
        let allocation_size = config.input_allocation_size()?;
        if self.input.as_ref().map(DmaBuf::len).unwrap_or(0) < allocation_size {
            self.input =
                Some(DmaBuf::alloc("system", allocation_size, false).map_err(|e| e.to_string())?);
        }
        let input_buf = self.input.as_mut().expect("input buffer was allocated");
        input_buf
            .sync_write_begin()
            .map_err(|e| format!("input dma-buf write sync: {e}"))?;
        input_buf.as_mut_slice()[..input_size].copy_from_slice(&input[..input_size]);
        input_buf
            .sync_write_end()
            .map_err(|e| format!("input dma-buf flush: {e}"))?;
        let input_fd = input_buf.fd();
        self.encode_fd_packets(input_fd, config)
    }

    pub fn encode_fd(&mut self, input_fd: i32, config: EncoderConfig) -> Result<Vec<u8>, String> {
        self.encode_fd_packets(input_fd, config)
            .map(|encoded| encoded.bytes)
    }

    pub fn encode_fd_packets(
        &mut self,
        input_fd: i32,
        config: EncoderConfig,
    ) -> Result<EncodedJpeg, String> {
        validate_config(config)?;
        let prepared = prepare_header(config)?;
        let header_len = prepared.bytes.len();
        self.write_qtable(&prepared)?;
        if self.output.as_ref().map(DmaBuf::len).unwrap_or(0) < config.output_capacity {
            self.output = Some(
                DmaBuf::alloc("system", config.output_capacity, false)
                    .map_err(|e| e.to_string())?,
            );
        }
        let output = self.output.as_mut().expect("output buffer was allocated");
        output
            .sync_write_begin()
            .map_err(|e| format!("output dma-buf write sync: {e}"))?;
        output.as_mut_slice()[..header_len].copy_from_slice(&prepared.bytes);
        output
            .sync_write_end()
            .map_err(|e| format!("output dma-buf flush: {e}"))?;

        let qtable_fd = self.qtable.fd();
        let output_fd = output.fd();
        let regs = build_registers(
            input_fd,
            output_fd,
            qtable_fd,
            output.len(),
            header_len,
            config,
        )?;
        let mut batch = Batch::new();
        let mut int_state = [0u32; 1];
        let mut status = [0u32; STATUS_REG_COUNT];
        batch
            .reg_write(&regs.words, 0)
            .reg_read(&mut int_state, INT_STATE_OFFSET as u32)
            .reg_read(&mut status, STATUS_OFFSET as u32)
            .reg_offsets(&regs.offsets);
        self.svc
            .submit_batch_recovering(&mut batch)
            .map_err(|e| format!("VPU720 submit: {e}"))?;
        self.svc
            .poll_hw_finish_recovering()
            .map_err(|e| format!("VPU720 poll: {e}"))?;
        if int_state[0] & HW_ERROR_MASK != 0 {
            let _ = self.svc.restart_after_hardware_error();
            return Err(format!("VPU720 hardware error 0x{:08x}", int_state[0]));
        }
        // The register is named `*_head_bits`, but C consumes it as a byte
        // count when it adds the hardware length to the header length.
        if status[1] != 0 {
            return Err(format!(
                "VPU720 hardware length is unexpectedly large: {status:?}"
            ));
        }
        let stream_len = header_len
            .checked_add(status[0] as usize)
            .ok_or_else(|| "VPU720 stream length overflow".to_string())?;
        if stream_len > config.output_capacity {
            return Err(format!(
                "VPU720 stream length {stream_len} exceeds output capacity"
            ));
        }
        output
            .sync_read_begin()
            .map_err(|e| format!("VPU720 output invalidate: {e}"))?;
        let bytes = output.as_slice()[..stream_len].to_vec();
        output
            .sync_read_end()
            .map_err(|e| format!("VPU720 output sync end: {e}"))?;
        Ok(EncodedJpeg {
            packets: vec![0..bytes.len()],
            bytes,
        })
    }

    fn write_qtable(&mut self, prepared: &PreparedHeader) -> Result<(), String> {
        let mut data = [0u32; QTABLE_SIZE];
        for i in 0..8 {
            for j in 0..8 {
                data[i * 8 + j] = 0x8000 / prepared.quant.y[j * 8 + i] as u32;
                data[64 + i * 8 + j] = 0x8000 / prepared.quant.u[j * 8 + i] as u32;
                data[128 + i * 8 + j] = 0x8000 / prepared.quant.v[j * 8 + i] as u32;
            }
        }
        self.qtable
            .sync_write_begin()
            .map_err(|e| format!("Q-table sync: {e}"))?;
        let bytes = self.qtable.as_mut_slice();
        for (index, word) in data.iter().enumerate() {
            let value = (*word as u16).to_le_bytes();
            bytes[index * 2..index * 2 + 2].copy_from_slice(&value);
        }
        self.qtable
            .sync_write_end()
            .map_err(|e| format!("Q-table flush: {e}"))
    }
}

#[derive(Clone)]
struct RegisterFile {
    words: [u32; BASE_REG_COUNT],
    offsets: Vec<RegOffsetInfo>,
}

fn build_registers(
    input_fd: i32,
    output_fd: i32,
    qtable_fd: i32,
    output_size: usize,
    header_len: usize,
    config: EncoderConfig,
) -> Result<RegisterFile, String> {
    let encode_width = align(config.width, 16);
    let encode_height = align(config.height, 16);
    let mcu_count = (encode_width / 16) * (encode_height / 16);
    let mut words = [0u32; BASE_REG_COUNT];
    words[1] = 1 << 8; // JPEG_VPU720_ENC_MODE_ONE_FRAME
    words[4] = 0x1fff;
    words[5] = 0x1fff;
    words[8] = 0x7;
    words[11] = (mcu_count as u32).saturating_mul(1000);
    words[16] = qtable_fd as u32;
    words[17] = output_fd as u32;
    words[18] = output_fd as u32;
    words[19] = output_fd as u32;
    words[20] = output_fd as u32;
    words[22] = input_fd as u32;
    words[23] = input_fd as u32;
    words[24] = input_fd as u32;
    words[26] = 0x3;
    words[29] = ((encode_width / 8 - 1) as u32) | (((encode_height / 8 - 1) as u32) << 16);
    words[30] = ((encode_width - config.width) as u32 & 0x3f)
        | (((encode_height - config.height) as u32 & 0x3f) << 16);
    let (input_format, uv_swap) = match config.input {
        InputFormat::Nv12 => (6, false),     // YUV420SP
        InputFormat::Nv16 => (4, false),     // YUV422SP
        InputFormat::Yuv420p => (7, false),  // YUV420P
        InputFormat::Nv21 => (6, true),      // YUV420SP with VU swap
        InputFormat::Yuv422p => (5, false),  // YUV422P
        InputFormat::Nv61 => (4, true),      // YUV422SP with VU swap
        InputFormat::Yuyv => (8, false),     // YUYV422
        InputFormat::Yvyu => (8, true),      // YUYV422 with U/V swap
        InputFormat::Uyvy => (9, false),     // UYVY422
        InputFormat::Vyuy => (9, true),      // UYVY422 with U/V swap
        InputFormat::Nv24 => (12, false),    // YUV444SP
        InputFormat::Yuv444p => (13, false), // YUV444P
        _ => return Err("unsupported VPU720 input format".into()),
    };
    let layout = config.input_layout()?;
    words[32] = (input_format << 2) | ((uv_swap as u32) << 1) | (1 << 6); // YUV420 output
    words[34] = config.hor_stride as u32;
    words[35] = layout.uv_stride as u32;
    let restart = config
        .split_plan()
        .map(|plan| plan.restart_interval)
        .unwrap_or(0);
    words[36] = restart as u32 | (1 << 29);
    words[42] = 0xf0f;
    let mut offsets = vec![
        RegOffsetInfo {
            reg_idx: 17,
            offset: output_size as u32,
        },
        RegOffsetInfo {
            reg_idx: 20,
            offset: header_len as u32,
        },
    ];
    if layout.u_offset != 0 {
        offsets.push(RegOffsetInfo {
            reg_idx: 23,
            offset: layout.u_offset as u32,
        });
    }
    if layout.v_offset != 0 {
        offsets.push(RegOffsetInfo {
            reg_idx: 24,
            offset: layout.v_offset as u32,
        });
    }
    Ok(RegisterFile { words, offsets })
}

fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

fn validate_config(config: EncoderConfig) -> Result<(), String> {
    config.validate()?;
    if config.rotation != Rotation::None {
        return Err("VPU720 does not support rotation".into());
    }
    if !config.input.supported_by_vpu720() {
        return Err(format!("VPU720 does not support {:?} input", config.input));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpu720_registers_encode_the_baseline_nv12_layout() {
        let config = EncoderConfig::new(320, 240);
        let regs = build_registers(1, 2, 3, 4096, 623, config).unwrap();
        assert_eq!(regs.words[1], 1 << 8);
        assert_eq!(regs.words[16], 3);
        assert_eq!(regs.words[22], 1);
        assert_eq!(regs.words[29], (39) | (29 << 16));
        assert_eq!(regs.words[32], (6 << 2) | (1 << 6));
        assert_eq!(regs.words[35], 320);
        assert_eq!(regs.words[42], 0xf0f);
        assert_eq!(
            regs.offsets,
            vec![
                RegOffsetInfo {
                    reg_idx: 17,
                    offset: 4096,
                },
                RegOffsetInfo {
                    reg_idx: 20,
                    offset: 623,
                },
                RegOffsetInfo {
                    reg_idx: 23,
                    offset: 76800,
                },
            ]
        );
    }

    #[test]
    fn vpu720_registers_describe_packed_yuv422_inputs() {
        for (input, source_format, swap) in [
            (InputFormat::Yuyv, 8, 0),
            (InputFormat::Yvyu, 8, 1),
            (InputFormat::Uyvy, 9, 0),
            (InputFormat::Vyuy, 9, 1),
        ] {
            let config = EncoderConfig::new_for_input(320, 240, input);
            let regs = build_registers(1, 2, 3, 4096, 623, config).unwrap();
            assert_eq!(
                regs.words[32],
                (source_format << 2) | (swap << 1) | (1 << 6)
            );
            assert_eq!(regs.words[34], 640);
            assert_eq!(regs.words[35], 0);
            assert!(regs
                .offsets
                .iter()
                .all(|offset| !matches!(offset.reg_idx, 23 | 24)));
        }
    }

    #[test]
    fn vpu720_registers_describe_planar_and_semiplanar_yuv_inputs() {
        for (input, source_format, uv_swap, uv_stride, u_offset, v_offset, input_size) in [
            (InputFormat::Nv16, 4, 0, 320, 76_800, 0, 153_600),
            (InputFormat::Yuv420p, 7, 0, 160, 76_800, 96_000, 115_200),
            (InputFormat::Nv21, 6, 1, 320, 76_800, 0, 115_200),
            (InputFormat::Yuv422p, 5, 0, 160, 76_800, 115_200, 153_600),
            (InputFormat::Nv61, 4, 1, 320, 76_800, 0, 153_600),
            (InputFormat::Nv24, 12, 0, 640, 76_800, 0, 230_400),
            (InputFormat::Yuv444p, 13, 0, 320, 76_800, 153_600, 230_400),
        ] {
            let config = EncoderConfig::new_for_input(320, 240, input);
            let regs = build_registers(1, 2, 3, 4096, 623, config).unwrap();
            assert_eq!(
                regs.words[32],
                (source_format << 2) | (uv_swap << 1) | (1 << 6)
            );
            assert_eq!(regs.words[34], 320);
            assert_eq!(regs.words[35], uv_stride);
            assert_eq!(config.input_size().unwrap(), input_size);
            assert_eq!(
                regs.offsets
                    .iter()
                    .find(|offset| offset.reg_idx == 23)
                    .map(|offset| offset.offset),
                (u_offset != 0).then_some(u_offset)
            );
            assert_eq!(
                regs.offsets
                    .iter()
                    .find(|offset| offset.reg_idx == 24)
                    .map(|offset| offset.offset),
                (v_offset != 0).then_some(v_offset)
            );
        }
    }
}
