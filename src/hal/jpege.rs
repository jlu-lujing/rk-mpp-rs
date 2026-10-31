//! VEPU2 JPEG encoder.
//!
//! The register layout follows `hal_jpege_vepu2_v2.c`; the JPEG markers follow
//! `jpege_api_v2.c` plus `hal_jpege_hdr.c`. RK3588's VEPU2 derives the JPEG
//! chroma sampling from its input format and has no `format_out` register, so
//! this backend accepts NV12 and packed YUYV/UYVY input, but every successful
//! encode is a baseline 4:2:0 JPEG. Requests for other output sampling are
//! rejected instead of writing a header that the entropy stream cannot match.

use std::io;

use crate::service::dma_heap::DmaBuf;
use crate::service::{Batch, Client, MppService, RegOffsetInfo};

use super::jpege_tables as tables;

pub const REG_COUNT: usize = 184;
pub const INPUT_Y: usize = 48;
pub const INPUT_U: usize = 49;
pub const INPUT_V: usize = 50;
pub const STREAM_MSB: usize = 51;
pub const STREAM_LSB: usize = 52;
pub const STREAM_CAPACITY: usize = 53;
pub const BUS_CONFIG: usize = 54;
pub const STREAM_START: usize = 60;
pub const INPUT_STRIDE: usize = 61;
pub const INPUT_FORMAT: usize = 74;
pub const OUTPUT_ADDR: usize = 77;
pub const COEFF_AB: usize = 95;
pub const COEFF_CE: usize = 96;
pub const COEFF_F: usize = 97;
pub const RGB_MASK: usize = 98;
pub const ENCODE_CONFIG: usize = 103;
pub const BYTE_SWAP: usize = 105;
pub const PART_CONFIG: usize = 107;
pub const IRQ_CONFIG: usize = 109;

const VEPU_FMT_YUV420SP: u32 = 1;
const VEPU_FMT_YUYV422: u32 = 2;
const VEPU_FMT_UYVY422: u32 = 3;
const NV12_SWAP_BITS: u32 = 0xfc00_0000;
const PACKED_YUV422_SWAP_BITS: u32 = 0xfc00_0000;
const DEFAULT_HW_STATUS_OFFSET: u32 = 0xffd0;
const STATUS_MASK: u32 = 0x70;
const OUTPUT_CAPACITY_MARGIN: usize = 64 * 1024;

const SOI: u16 = 0xffd8;
const APP0: u16 = 0xffe0;
const DQT: u16 = 0xffdb;
const SOF0: u16 = 0xffc0;
const DHT: u16 = 0xffc4;
const SOS: u16 = 0xffda;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// MPP's eleven built-in tables, from lowest quality (0) to best (10).
    Quant(u8),
    /// The JPEG quality-factor formula used by MPP, from 1 through 100.
    QFactor(u8),
}

impl Default for Quality {
    fn default() -> Self {
        Self::Quant(10)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChromaFormat {
    Gray,
    #[default]
    Yuv420,
    Yuv422,
    Yuv444,
}

impl ChromaFormat {
    fn components(self) -> &'static [(u8, u8, u8, u8)] {
        match self {
            // (component id, h sampling, v sampling, quant/Huffman selector)
            Self::Gray => &[(1, 1, 1, 0)],
            Self::Yuv420 => &[(1, 2, 2, 0), (2, 1, 1, 1), (3, 1, 1, 1)],
            Self::Yuv422 => &[(1, 2, 1, 0), (2, 1, 1, 1), (3, 1, 1, 1)],
            Self::Yuv444 => &[(1, 1, 1, 0), (2, 1, 1, 1), (3, 1, 1, 1)],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputFormat {
    #[default]
    Nv12,
    /// Packed Y0 U0 Y1 V0 input, two bytes per pixel.
    Yuyv,
    /// Packed U0 Y0 V0 Y1 input, two bytes per pixel.
    Uyvy,
}

impl InputFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Nv12 => 1,
            Self::Yuyv | Self::Uyvy => 2,
        }
    }

    fn vepu_format(self) -> u32 {
        match self {
            Self::Nv12 => VEPU_FMT_YUV420SP,
            Self::Yuyv => VEPU_FMT_YUYV422,
            Self::Uyvy => VEPU_FMT_UYVY422,
        }
    }

    fn swap_bits(self) -> u32 {
        match self {
            Self::Nv12 => NV12_SWAP_BITS,
            Self::Yuyv | Self::Uyvy => PACKED_YUV422_SWAP_BITS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderConfig {
    pub width: usize,
    pub height: usize,
    /// Input row stride in bytes.
    pub hor_stride: usize,
    /// Input frame height, including allocation padding. Must cover MPP's
    /// 16-row coded height.
    pub ver_stride: usize,
    /// Bytes reserved for the complete JPEG stream.
    pub output_capacity: usize,
    pub quality: Quality,
    pub input: InputFormat,
    pub chroma: ChromaFormat,
}

impl EncoderConfig {
    pub fn new(width: usize, height: usize) -> Self {
        let output_capacity = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(3))
            .and_then(|bytes| bytes.checked_add(OUTPUT_CAPACITY_MARGIN))
            .unwrap_or(usize::MAX);
        Self {
            width,
            height,
            hor_stride: align(width, 8),
            ver_stride: align(height, 16),
            output_capacity,
            quality: Quality::default(),
            input: InputFormat::default(),
            chroma: ChromaFormat::default(),
        }
    }

    pub fn new_for_input(width: usize, height: usize, input: InputFormat) -> Self {
        let mut config = Self::new(width, height);
        config.input = input;
        config.hor_stride = align(width * input.bytes_per_pixel(), 8);
        config.chroma = ChromaFormat::Yuv420;
        config
    }

    pub fn validate(self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("JPEG width and height must be non-zero".into());
        }
        if self.width > u16::MAX as usize || self.height > u16::MAX as usize {
            return Err("baseline JPEG dimensions must fit in 16 bits".into());
        }
        if self.chroma != ChromaFormat::Yuv420 {
            return Err(format!(
                "VEPU2 JPEG output is always 4:2:0; {:?} output is unsupported on this hardware",
                self.chroma
            ));
        }
        let minimum_stride = self
            .width
            .checked_mul(self.input.bytes_per_pixel())
            .ok_or_else(|| "JPEG input stride overflow".to_string())?;
        let stride_alignment = self.input.bytes_per_pixel() * 8;
        if self.hor_stride < minimum_stride || self.hor_stride % stride_alignment != 0 {
            return Err(format!(
                "{:?} input stride must cover {minimum_stride} bytes and be {stride_alignment}-byte aligned",
                self.input
            ));
        }
        let vepu_pixel_stride = self.hor_stride / self.input.bytes_per_pixel();
        if vepu_pixel_stride >= (1 << 15) {
            return Err("VEPU2 pixel stride must be less than 32768".into());
        }
        if self.ver_stride < self.coded_height() || self.ver_stride & 1 != 0 {
            return Err("input frame height must cover the 16-row coded height and be even".into());
        }
        if self.output_capacity < 1024 {
            return Err("JPEG output capacity is too small".into());
        }
        Ok(())
    }

    pub fn input_size(self) -> Result<usize, String> {
        self.validate()?;
        let rows = self
            .hor_stride
            .checked_mul(self.ver_stride)
            .ok_or_else(|| "JPEG input size overflow".to_string())?;
        match self.input {
            InputFormat::Nv12 => rows
                .checked_mul(3)
                .map(|bytes| bytes / 2)
                .ok_or_else(|| "NV12 input size overflow".into()),
            InputFormat::Yuyv | InputFormat::Uyvy => Ok(rows),
        }
    }

    /// MPP programs the VEPU2 coded height independently of the allocation
    /// stride. See `hal_jpege_vepu2_gen_regs()` in the C reference.
    pub fn coded_height(self) -> usize {
        align(self.height, 16)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantTables {
    pub y: [u8; 64],
    pub u: [u8; 64],
    pub v: [u8; 64],
}

impl QuantTables {
    pub fn for_quality(quality: Quality) -> Result<Self, String> {
        match quality {
            Quality::Quant(level) => {
                let level = level as usize;
                if level >= tables::QTABLE_Y.len() {
                    return Err("JPEG quantization level must be between 0 and 10".into());
                }
                Ok(Self {
                    y: tables::QTABLE_Y[level],
                    u: tables::QTABLE_C[level],
                    v: tables::QTABLE_C[level],
                })
            }
            Quality::QFactor(qfactor) => {
                if !(1..=100).contains(&qfactor) {
                    return Err("JPEG quality factor must be between 1 and 100".into());
                }
                let qfactor = qfactor as usize;
                let scale = if qfactor < 50 {
                    5000 / qfactor
                } else {
                    200 - qfactor * 2
                };
                let mut y = [0u8; 64];
                let mut c = [0u8; 64];
                for i in 0..64 {
                    y[i] = clip_quantizer(tables::LUMA_QUANTIZER[i], scale);
                    c[i] = clip_quantizer(tables::CHROMA_QUANTIZER[i], scale);
                }
                Ok(Self { y, u: c, v: c })
            }
        }
    }
}

fn clip_quantizer(base: u8, scale: usize) -> u8 {
    ((base as usize * scale + 50) / 100).clamp(1, 255) as u8
}

#[derive(Debug, Clone)]
pub struct PreparedHeader {
    pub bytes: Vec<u8>,
    pub quant: QuantTables,
}

#[derive(Debug, Clone)]
pub struct RegisterFile {
    pub words: [u32; REG_COUNT],
    pub offsets: Vec<RegOffsetInfo>,
    pub header_len: usize,
}

pub struct Vepu2Jpeg {
    svc: MppService,
    input: Option<DmaBuf>,
    output: Option<DmaBuf>,
}

impl Vepu2Jpeg {
    pub fn open() -> io::Result<Self> {
        Ok(Self {
            svc: MppService::open(Client::Vepu2Jpeg as u32)?,
            input: None,
            output: None,
        })
    }

    pub fn encode_nv12(&mut self, input: &[u8], config: EncoderConfig) -> Result<Vec<u8>, String> {
        if config.input != InputFormat::Nv12 || config.chroma != ChromaFormat::Yuv420 {
            return Err("encode_nv12 requires NV12 input and 4:2:0 output".into());
        }
        self.encode(input, config)
    }

    pub fn encode(&mut self, input: &[u8], config: EncoderConfig) -> Result<Vec<u8>, String> {
        let input_size = config.input_size()?;
        if input.len() < input_size {
            return Err(format!(
                "JPEG input is {} bytes, expected at least {input_size}",
                input.len()
            ));
        }

        if self.input.as_ref().map(DmaBuf::len).unwrap_or(0) < input_size {
            self.input =
                Some(DmaBuf::alloc("system", input_size, false).map_err(|e| e.to_string())?);
        }
        let input_fd = {
            let input_buf = self.input.as_mut().unwrap();
            input_buf
                .sync_write_begin()
                .map_err(|e| format!("input dma-buf write sync: {e}"))?;
            input_buf.as_mut_slice()[..input_size].copy_from_slice(&input[..input_size]);
            input_buf
                .sync_write_end()
                .map_err(|e| format!("input dma-buf sync: {e}"))?;
            input_buf.fd()
        };

        let output = self.encode_fd(input_fd, config)?;
        Ok(output)
    }

    /// Encode an externally allocated input dma-buf. The output is still owned
    /// by this encoder so its final hardware-reported length can be read safely.
    ///
    /// The caller must finish CPU writes and flush the input dma-buf before
    /// calling this method.
    pub fn encode_fd(&mut self, input_fd: i32, config: EncoderConfig) -> Result<Vec<u8>, String> {
        config.validate()?;
        let prepared = prepare_header(config)?;
        if self.output.as_ref().map(DmaBuf::len).unwrap_or(0) < config.output_capacity {
            self.output = Some(
                DmaBuf::alloc("system", config.output_capacity, false)
                    .map_err(|e| e.to_string())?,
            );
        }
        let output = self.output.as_mut().unwrap();
        output
            .sync_write_begin()
            .map_err(|e| format!("output dma-buf write sync: {e}"))?;
        output.as_mut_slice()[..prepared.bytes.len()].copy_from_slice(&prepared.bytes);
        output
            .sync_write_end()
            .map_err(|e| format!("output dma-buf flush: {e}"))?;

        let mut registers = build_registers(
            input_fd,
            output.fd(),
            config,
            prepared.bytes.len(),
            &output.as_slice()[..prepared.bytes.len()],
            &prepared.quant,
        )?;

        let mut batch = Batch::new();
        batch.reg_write(&registers.words, 0);
        batch.reg_offsets(&registers.offsets);
        batch.reg_read(&mut registers.words, 0);
        self.svc
            .submit_batch(&mut batch)
            .map_err(|e| format!("JPEG encoder submit: {e}"))?;
        self.svc
            .poll_hw_finish()
            .map_err(|e| format!("JPEG encoder poll: {e}"))?;

        let status = registers.words[IRQ_CONFIG] & STATUS_MASK;
        if status != 0 {
            return Err(format!(
                "VEPU2 JPEG hardware error 0x{status:02x} (reg109=0x{:08x})",
                registers.words[IRQ_CONFIG]
            ));
        }

        let hardware_bits = registers.words[STREAM_CAPACITY];
        if hardware_bits & 7 != 0 {
            return Err(format!(
                "VEPU2 JPEG returned a non-byte stream length ({hardware_bits} bits)"
            ));
        }
        let stream_len = (prepared.bytes.len() & !7)
            .checked_add(hardware_bits as usize / 8)
            .ok_or_else(|| "JPEG stream length overflow".to_string())?;
        if stream_len <= prepared.bytes.len() || stream_len > config.output_capacity {
            return Err(format!(
                "invalid VEPU2 JPEG output length {stream_len} (header {}, capacity {})",
                prepared.bytes.len(),
                config.output_capacity
            ));
        }

        output
            .sync_read_begin()
            .map_err(|e| format!("output dma-buf invalidate: {e}"))?;
        let result = output.as_slice()[..stream_len].to_vec();
        output
            .sync_read_end()
            .map_err(|e| format!("output dma-buf read sync end: {e}"))?;
        Ok(result)
    }
}

fn prepare_header(config: EncoderConfig) -> Result<PreparedHeader, String> {
    config.validate()?;
    let quant = QuantTables::for_quality(config.quality)?;
    let mut bytes = Vec::with_capacity(1024);

    // `jpege_start()` writes SOI and this exact JFIF APP0 prefix.
    push_u16(&mut bytes, SOI);
    push_u16(&mut bytes, APP0);
    bytes.extend_from_slice(&[
        0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x02, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
        0x00,
    ]);

    write_dqt(&mut bytes, &quant.y, 0);
    let components = config.chroma.components();
    if components.len() > 1 {
        write_dqt(&mut bytes, &quant.u, 1);
    }
    write_sof0(&mut bytes, config.width, config.height, components);
    write_dht(
        &mut bytes,
        0,
        0,
        tables::DC_COUNTS.iter().map(|v| v.0).collect::<Vec<_>>(),
        &tables::DC_SYMBOLS,
    );
    write_dht(
        &mut bytes,
        1,
        0,
        tables::AC_COUNTS.iter().map(|v| v.0).collect::<Vec<_>>(),
        &tables::AC_SYMBOLS_Y,
    );
    if components.len() > 1 {
        write_dht(
            &mut bytes,
            0,
            1,
            tables::DC_COUNTS.iter().map(|v| v.1).collect::<Vec<_>>(),
            &tables::DC_SYMBOLS,
        );
        write_dht(
            &mut bytes,
            1,
            1,
            tables::AC_COUNTS.iter().map(|v| v.1).collect::<Vec<_>>(),
            &tables::AC_SYMBOLS_C,
        );
    }
    write_sos(&mut bytes, components);

    Ok(PreparedHeader { bytes, quant })
}

fn write_dqt(out: &mut Vec<u8>, table: &[u8; 64], index: u8) {
    push_u16(out, DQT);
    push_u16(out, 67);
    out.push(index & 0x0f);
    for &index in &tables::ZIGZAG {
        out.push(table[index]);
    }
}

fn write_sof0(out: &mut Vec<u8>, width: usize, height: usize, components: &[(u8, u8, u8, u8)]) {
    push_u16(out, SOF0);
    push_u16(out, (8 + 3 * components.len()) as u16);
    out.push(8);
    push_u16(out, height as u16);
    push_u16(out, width as u16);
    out.push(components.len() as u8);
    for &(id, h, v, selector) in components {
        out.push(id);
        out.push((h << 4) | v);
        out.push(selector);
    }
}

fn write_dht(out: &mut Vec<u8>, class: u8, selector: u8, counts: Vec<u8>, symbols: &[u8]) {
    push_u16(out, DHT);
    push_u16(out, (2 + 1 + counts.len() + symbols.len()) as u16);
    out.push((class << 4) | (selector & 0x0f));
    out.extend_from_slice(&counts);
    out.extend_from_slice(symbols);
}

fn write_sos(out: &mut Vec<u8>, components: &[(u8, u8, u8, u8)]) {
    push_u16(out, SOS);
    push_u16(out, (6 + 2 * components.len()) as u16);
    out.push(components.len() as u8);
    for &(id, _, _, selector) in components {
        out.push(id);
        out.push((selector << 4) | selector);
    }
    out.extend_from_slice(&[0, 63, 0]);
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn build_registers(
    input_fd: i32,
    output_fd: i32,
    config: EncoderConfig,
    header_len: usize,
    output_prefix: &[u8],
    quant: &QuantTables,
) -> Result<RegisterFile, String> {
    config.validate()?;
    if header_len == 0 || header_len >= config.output_capacity {
        return Err("JPEG header does not fit in the output buffer".into());
    }
    if output_prefix.len() < header_len {
        return Err("JPEG output prefix is shorter than the prepared header".into());
    }

    let width_align = align(config.width, 16);
    let x_fill = (width_align - config.width) / 4;
    let coded_height = config.coded_height();
    let y_fill = coded_height - config.height;
    let (msb, lsb) = initial_stream_words(output_prefix, header_len);

    let mut words = [0u32; REG_COUNT];
    words[INPUT_Y] = input_fd as u32;
    words[INPUT_U] = input_fd as u32;
    words[INPUT_V] = input_fd as u32;
    words[STREAM_MSB] = msb;
    words[STREAM_LSB] = lsb;
    words[STREAM_CAPACITY] = (config.output_capacity - header_len) as u32;
    words[BUS_CONFIG] = 16 << 8;
    words[STREAM_START] =
        (((header_len & 7) * 8) as u32) << 16 | (x_fill as u32) << 4 | y_fill as u32;
    words[INPUT_STRIDE] = (config.hor_stride / config.input.bytes_per_pixel()) as u32;
    words[INPUT_FORMAT] = config.input.vepu_format() << 4;
    words[OUTPUT_ADDR] = output_fd as u32;
    words[COEFF_AB] = 19589 | (38443 << 16);
    words[COEFF_CE] = 7504 | (37008 << 16);
    words[COEFF_F] = 46740;
    words[RGB_MASK] = 0;
    words[ENCODE_CONFIG] = ((width_align >> 4) as u32) << 8
        | ((coded_height >> 4) as u32) << 20
        | (1 << 6)
        | (2 << 4)
        | 1;
    words[BYTE_SWAP] = config.input.swap_bits();
    words[PART_CONFIG] = DEFAULT_HW_STATUS_OFFSET;
    words[IRQ_CONFIG] = (1 << 12) | (1 << 10);

    for (reg_base, table) in [(0, &quant.y), (16, &quant.u)] {
        for i in 0..16 {
            let src = i * 4;
            words[reg_base + i] = (table[tables::QP_REORDER[src]] as u32) << 24
                | (table[tables::QP_REORDER[src + 1]] as u32) << 16
                | (table[tables::QP_REORDER[src + 2]] as u32) << 8
                | table[tables::QP_REORDER[src + 3]] as u32;
        }
    }

    let mut offsets = Vec::with_capacity(3);
    if config.input == InputFormat::Nv12 {
        let luma_size = config.hor_stride * config.ver_stride;
        offsets.push(RegOffsetInfo {
            reg_idx: INPUT_U as u32,
            offset: luma_size as u32,
        });
        offsets.push(RegOffsetInfo {
            reg_idx: INPUT_V as u32,
            offset: luma_size as u32,
        });
    }
    offsets.push(RegOffsetInfo {
        reg_idx: OUTPUT_ADDR as u32,
        offset: header_len as u32,
    });

    Ok(RegisterFile {
        words,
        offsets,
        header_len,
    })
}

fn initial_stream_words(output: &[u8], bytepos: usize) -> (u32, u32) {
    let start = bytepos & !7;
    let left = bytepos & 7;
    let mut word = [0u8; 8];
    let available = output.len().saturating_sub(start).min(8);
    word[..available].copy_from_slice(&output[start..start + available]);
    if left != 0 {
        word[left..].fill(0);
    }
    let msb = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
    let lsb = if left > 4 {
        (word[4] as u32) << 24 | (word[5] as u32) << 16 | (word[6] as u32) << 8
    } else {
        0
    };
    (msb, lsb)
}

fn align(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) / alignment * alignment
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv1a(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf29ce484222325u64, |mut hash, byte| {
            hash ^= u64::from(*byte);
            hash.wrapping_mul(0x100000001b3)
        })
    }

    #[test]
    fn tables_match_the_c_defaults() {
        let y: Vec<u8> = tables::QTABLE_Y.into_iter().flatten().collect();
        let c: Vec<u8> = tables::QTABLE_C.into_iter().flatten().collect();
        assert_eq!(fnv1a(&y), 0x8bcbd7bc08df3454);
        assert_eq!(fnv1a(&c), 0x1b8512d6bfcc4506);
        assert_eq!(tables::QTABLE_Y[10], [1; 64]);
        assert_eq!(tables::QTABLE_C[10], [1; 64]);
    }

    #[test]
    fn qfactor_50_is_the_standard_jpeg_table() {
        let q = QuantTables::for_quality(Quality::QFactor(50)).unwrap();
        assert_eq!(q.y, tables::LUMA_QUANTIZER);
        assert_eq!(q.u, tables::CHROMA_QUANTIZER);
        assert_eq!(q.v, tables::CHROMA_QUANTIZER);
    }

    #[test]
    fn header_matches_the_mpp_marker_layout() {
        let config = EncoderConfig {
            quality: Quality::Quant(10),
            ..EncoderConfig::new(640, 640)
        };
        let header = prepare_header(config).unwrap();
        assert_eq!(header.bytes.len(), 623);
        assert_eq!(
            &header.bytes[..20],
            b"\xff\xd8\xff\xe0\x00\x10JFIF\0\x01\x02\0\0\x01\0\x01\0\0"
        );
        assert_eq!(&header.bytes[20..22], &[0xff, 0xdb]);
        assert_eq!(&header.bytes[158..160], &[0xff, 0xc0]);
        assert_eq!(&header.bytes[177..179], &[0xff, 0xc4]);
        assert_eq!(&header.bytes[609..611], &[0xff, 0xda]);
        assert_eq!(&header.bytes[611..615], &[0x00, 0x0c, 0x03, 0x01]);
    }

    #[test]
    fn register_file_is_the_vepu2_layout() {
        let config = EncoderConfig {
            quality: Quality::Quant(10),
            ..EncoderConfig::new(640, 640)
        };
        let header = prepare_header(config).unwrap();
        let regs = build_registers(
            7,
            9,
            config,
            header.bytes.len(),
            &header.bytes,
            &header.quant,
        )
        .unwrap();
        assert_eq!(regs.words.len(), 184);
        assert_eq!(regs.header_len, 623);
        assert_eq!(regs.words[INPUT_Y], 7);
        assert_eq!(regs.words[INPUT_U], 7);
        assert_eq!(regs.words[INPUT_V], 7);
        assert_eq!(
            regs.words[STREAM_CAPACITY] as usize,
            config.output_capacity - 623
        );
        assert_eq!(regs.words[BUS_CONFIG], 0x1000);
        assert_eq!(regs.words[STREAM_START], (623 & 7) as u32 * 8 << 16);
        assert_eq!(regs.words[INPUT_STRIDE], 640);
        assert_eq!(regs.words[INPUT_FORMAT], 0x10);
        assert_eq!(regs.words[OUTPUT_ADDR], 9);
        assert_eq!(regs.words[ENCODE_CONFIG], 0x0280_2861);
        assert_eq!(regs.words[BYTE_SWAP], 0xfc00_0000);
        assert_eq!(regs.words[PART_CONFIG], 0xffd0);
        assert_eq!(regs.words[IRQ_CONFIG], 0x1400);
        assert_eq!(
            regs.offsets,
            vec![
                RegOffsetInfo {
                    reg_idx: 49,
                    offset: 640 * 640,
                },
                RegOffsetInfo {
                    reg_idx: 50,
                    offset: 640 * 640,
                },
                RegOffsetInfo {
                    reg_idx: 77,
                    offset: 623,
                },
            ]
        );
    }

    #[test]
    fn rejects_invalid_nv12_geometry() {
        let mut config = EncoderConfig::new(640, 640);
        config.hor_stride = 639;
        assert!(config.validate().is_err());
        config = EncoderConfig::new(640, 640);
        config.ver_stride = 639;
        assert!(config.validate().is_err());
    }

    #[test]
    fn packed_yuv_input_uses_a_pixel_stride_and_no_planar_offsets() {
        for (input, format) in [
            (InputFormat::Yuyv, VEPU_FMT_YUYV422),
            (InputFormat::Uyvy, VEPU_FMT_UYVY422),
        ] {
            let config = EncoderConfig {
                quality: Quality::Quant(10),
                ..EncoderConfig::new_for_input(320, 240, input)
            };
            let header = prepare_header(config).unwrap();
            let regs = build_registers(
                7,
                9,
                config,
                header.bytes.len(),
                &header.bytes,
                &header.quant,
            )
            .unwrap();

            assert_eq!(regs.words[INPUT_STRIDE], 320);
            assert_eq!(regs.words[INPUT_FORMAT], format << 4);
            assert_eq!(
                regs.offsets.last(),
                Some(&RegOffsetInfo {
                    reg_idx: OUTPUT_ADDR as u32,
                    offset: header.bytes.len() as u32,
                })
            );
            assert_eq!(regs.offsets.len(), 1);
            assert_eq!(config.input_size().unwrap(), 320 * 240 * 2);
        }
    }

    #[test]
    fn odd_dimensions_use_vepu_fill_fields() {
        let config = EncoderConfig {
            quality: Quality::Quant(10),
            ..EncoderConfig::new(321, 241)
        };
        assert_eq!((config.hor_stride, config.ver_stride), (328, 256));
        let header = prepare_header(config).unwrap();
        let regs = build_registers(
            7,
            9,
            config,
            header.bytes.len(),
            &header.bytes,
            &header.quant,
        )
        .unwrap();
        assert_eq!(regs.words[STREAM_START] & 0xf, 15);
        assert_eq!((regs.words[STREAM_START] >> 4) & 0xf, 3);
        assert_eq!(regs.words[INPUT_STRIDE], 328);
    }

    #[test]
    fn custom_ver_stride_does_not_change_the_coded_height() {
        let config = EncoderConfig {
            ver_stride: 256,
            ..EncoderConfig::new(320, 240)
        };
        let header = prepare_header(config).unwrap();
        let regs = build_registers(
            7,
            9,
            config,
            header.bytes.len(),
            &header.bytes,
            &header.quant,
        )
        .unwrap();

        assert_eq!((regs.words[ENCODE_CONFIG] >> 20) & 0xf, 240 >> 4);
        assert_eq!(regs.words[STREAM_START] & 0xf, 0);
        assert_eq!(regs.offsets[0].offset, 320 * 256);

        let too_short = EncoderConfig {
            ver_stride: 242,
            ..EncoderConfig::new(320, 241)
        };
        assert!(too_short.validate().is_err());
    }

    #[test]
    fn rejects_invalid_quality() {
        assert!(QuantTables::for_quality(Quality::Quant(11)).is_err());
        assert!(QuantTables::for_quality(Quality::QFactor(0)).is_err());
        assert!(QuantTables::for_quality(Quality::QFactor(101)).is_err());
    }
}
