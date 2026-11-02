//! VEPU2 JPEG encoder.
//!
//! The register layout follows `hal_jpege_vepu2_v2.c`; the JPEG markers follow
//! `jpege_api_v2.c` plus `hal_jpege_hdr.c`. RK3588's VEPU2 derives the JPEG
//! chroma sampling from its input format and has no `format_out` register, so
//! this backend accepts NV12, packed YUYV/UYVY and the VEPU2 RGB input family,
//! but every successful encode is a baseline 4:2:0 JPEG. Requests for other
//! output sampling are rejected instead of writing a header that the entropy
//! stream cannot match.
//!
//! Input geometry (`width`, `height`, `hor_stride`, `ver_stride`) always
//! describes the *input* buffer. With 90/270-degree rotation the hardware
//! transposes while encoding, so the SOF dimensions are swapped while every
//! geometry register keeps the unrotated values - the same register image C
//! reaches by swapping its syntax fields back inside
//! `hal_jpege_vepu2_gen_regs()`.

use std::io;
use std::ops::Range;

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

/// Register 107 low half: the restart marker the hardware emits first, and
/// the entry point of the `jpege_restart_marker[8]` cycle (hal_jpege_base.c).
pub const RESTART_MARKER_FIRST: u32 = 0xffd0;
pub const RESTART_MARKERS: [u32; 8] = [
    0xffd0, 0xffd1, 0xffd2, 0xffd3, 0xffd4, 0xffd5, 0xffd6, 0xffd7,
];
/// Register 107 bit 24, "slice encode end by RST" in `gen_regs` /
/// `part_not_end` in `part_start`. Set with `SLICE_IRQ_ENABLE` when
/// low-delay slice mode is armed; not used by the single-job path.
pub const SLICE_END_BY_RST: u32 = 1 << 24;
/// Register 109 bit 16, slice interrupt enable (low-delay slice mode only).
pub const SLICE_IRQ_ENABLE: u32 = 1 << 16;

const VEPU_FMT_YUV420SP: u32 = 1;
const VEPU_FMT_YUYV422: u32 = 2;
const VEPU_FMT_UYVY422: u32 = 3;
const VEPU_FMT_RGB565: u32 = 4;
const VEPU_FMT_RGB555: u32 = 5;
const VEPU_FMT_RGB444: u32 = 6;
const VEPU_FMT_RGB888_32: u32 = 7;
const VEPU_FMT_RGB101010: u32 = 8;
const NV12_SWAP_BITS: u32 = 0xfc00_0000;
const PACKED_YUV422_SWAP_BITS: u32 = 0xfc00_0000;
/// All three in-swaps set (7 << 26 | swap_8 | swap_16 | swap_32) - the
/// `vepu_rgb_cfg` rows for the 16-bit and 101010 RGB formats.
const RGB_FULL_SWAP_BITS: u32 = 0xfc00_0000;
/// 7 << 26 with only swap_32_in set - the `vepu_rgb_cfg` rows for the
/// 32-bit ARGB/ABGR/BGRA/RGBA formats.
const RGB_32BIT_SWAP_BITS: u32 = 0x3c00_0000;
const STATUS_MASK: u32 = 0x70;
const OUTPUT_CAPACITY_MARGIN: usize = 64 * 1024;

const SOI: u16 = 0xffd8;
const APP0: u16 = 0xffe0;
const DQT: u16 = 0xffdb;
const SOF0: u16 = 0xffc0;
const DRI: u16 = 0xffdd;
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
    Rgb565,
    Bgr565,
    Rgb555,
    Bgr555,
    Rgb444,
    Bgr444,
    /// 30-bit RGB in one 32-bit word, the `MPP_FMT_RGB101010` entry.
    Rgb101010,
    /// 30-bit RGB in one 32-bit word, the `MPP_FMT_BGR101010` entry.
    Bgr101010,
    Argb8888,
    Abgr8888,
    Bgra8888,
    Rgba8888,
}

impl InputFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Nv12 => 1,
            Self::Yuyv
            | Self::Uyvy
            | Self::Rgb565
            | Self::Bgr565
            | Self::Rgb555
            | Self::Bgr555
            | Self::Rgb444
            | Self::Bgr444 => 2,
            Self::Rgb101010
            | Self::Bgr101010
            | Self::Argb8888
            | Self::Abgr8888
            | Self::Bgra8888
            | Self::Rgba8888 => 4,
        }
    }

    pub fn is_rgb(self) -> bool {
        !matches!(self, Self::Nv12 | Self::Yuyv | Self::Uyvy)
    }

    fn vepu_format(self) -> u32 {
        match self {
            Self::Nv12 => VEPU_FMT_YUV420SP,
            Self::Yuyv => VEPU_FMT_YUYV422,
            Self::Uyvy => VEPU_FMT_UYVY422,
            Self::Rgb565 | Self::Bgr565 => VEPU_FMT_RGB565,
            Self::Rgb555 | Self::Bgr555 => VEPU_FMT_RGB555,
            Self::Rgb444 | Self::Bgr444 => VEPU_FMT_RGB444,
            Self::Argb8888 | Self::Abgr8888 | Self::Bgra8888 | Self::Rgba8888 => VEPU_FMT_RGB888_32,
            Self::Rgb101010 | Self::Bgr101010 => VEPU_FMT_RGB101010,
        }
    }

    fn swap_bits(self) -> u32 {
        match self {
            Self::Nv12 => NV12_SWAP_BITS,
            Self::Yuyv | Self::Uyvy => PACKED_YUV422_SWAP_BITS,
            Self::Argb8888 | Self::Abgr8888 | Self::Bgra8888 | Self::Rgba8888 => {
                RGB_32BIT_SWAP_BITS
            }
            _ => RGB_FULL_SWAP_BITS,
        }
    }

    /// The (r, g, b) component MSB positions for register 98, from the
    /// `vepu_rgb_cfg` table in `vepu_common.c` (big-endian value layout rows,
    /// which are the rows the non-LE `MPP_FMT_*` values select). YUV input
    /// programs the register with zero.
    fn rgb_masks(self) -> Option<(u8, u8, u8)> {
        match self {
            Self::Rgb565 => Some((15, 10, 4)),
            Self::Bgr565 => Some((4, 10, 15)),
            Self::Rgb555 => Some((14, 9, 4)),
            Self::Bgr555 => Some((4, 9, 14)),
            Self::Rgb444 => Some((11, 7, 3)),
            Self::Bgr444 => Some((3, 7, 11)),
            Self::Argb8888 => Some((15, 23, 31)),
            Self::Abgr8888 => Some((31, 23, 15)),
            Self::Bgra8888 => Some((23, 15, 7)),
            Self::Rgba8888 => Some((7, 15, 23)),
            Self::Rgb101010 => Some((29, 19, 9)),
            Self::Bgr101010 => Some((9, 19, 29)),
            Self::Nv12 | Self::Yuyv | Self::Uyvy => None,
        }
    }
}

/// Input rotation. VEPU2 rotates during the encode read; the C HAL maps
/// `MPP_ENC_ROT_90` to 1 and `MPP_ENC_ROT_270` to 2 in the input-format
/// register (`hal_jpege_vepu2_gen_regs()`), and reports every other non-zero
/// rotation as unsupported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    #[default]
    None,
    Degree90,
    Degree270,
}

impl Rotation {
    fn vepu_bits(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Degree90 => 1,
            Self::Degree270 => 2,
        }
    }

    pub fn transposes_output(self) -> bool {
        matches!(self, Self::Degree90 | Self::Degree270)
    }
}

/// MCU-row split plan derived the way `jpege_proc_hal()` derives it from
/// `MppEncSliceSplit` in `MPP_ENC_SPLIT_BY_CTU` mode: the user argument counts
/// MCUs per band, the row count is that rounded up over the MCU columns, and
/// the restart interval is one band of MCUs, which is what makes the hardware
/// terminate every band with the next `RSTn` marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitPlan {
    pub part_rows: usize,
    pub restart_interval: usize,
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
    /// 90/270-degree rotation of the encoded picture. Geometry fields keep
    /// describing the unrotated input; only the SOF dimensions and the
    /// rotation bits change.
    pub rotation: Rotation,
    /// MCUs per band, the `MPP_ENC_SPLIT_BY_CTU` `split_arg`. `None` or an
    /// argument that yields no band disables the restart interval.
    pub split_mcus: Option<u16>,
    /// Expose each restart band as its own packet. The assembled JPEG is the
    /// same single-job stream; this is the output view C's low-delay part loop
    /// produces.
    pub low_delay: bool,
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
            rotation: Rotation::default(),
            split_mcus: None,
            low_delay: false,
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
        if let Some(plan) = self.split_plan() {
            if plan.restart_interval > u16::MAX as usize {
                return Err(format!(
                    "JPEG restart interval {} does not fit the 16-bit DRI field",
                    plan.restart_interval
                ));
            }
        }
        if self.low_delay && self.split_plan().is_none() {
            return Err("JPEG low-delay output requires a valid MCU split".into());
        }
        if self.low_delay && self.rotation != Rotation::None {
            return Err("C's VEPU2 JPEG part path does not apply rotated band offsets".into());
        }
        if self.low_delay && self.width.saturating_mul(self.height) > 1280 * 720 {
            return Err(
                "VEPU2 JPEG above 720p enters C's multi-core partition path, not the sequential part path"
                    .into(),
            );
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
            _ => Ok(rows),
        }
    }

    /// MPP programs the VEPU2 coded height independently of the allocation
    /// stride. See `hal_jpege_vepu2_gen_regs()` in the C reference.
    pub fn coded_height(self) -> usize {
        align(self.height, 16)
    }

    /// Encoded picture dimensions: the input dimensions, transposed for a
    /// 90/270-degree rotation. C reaches the same pair by having
    /// `mpp_enc_cfg_set_prep()` swap `width_set`/`height_set` into
    /// `syntax->width`/`syntax->height` and then swapping them back locally
    /// inside `hal_jpege_vepu2_gen_regs()`.
    pub fn output_size(self) -> (usize, usize) {
        if self.rotation.transposes_output() {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// MCU counts of the encoded picture. Baseline 4:2:0 sampling makes every
    /// MCU 16x16, matching `init_jpeg_component_info()`.
    pub fn mcu_counts(self) -> (usize, usize) {
        let (width, height) = self.output_size();
        (align(width, 16) / 16, align(height, 16) / 16)
    }

    /// The split plan and derived restart interval, mirroring the
    /// `MPP_ENC_SPLIT_BY_CTU` branch of `jpege_proc_hal()`: an argument of
    /// zero, above the MCU count, or covering every MCU row yields no plan.
    pub fn split_plan(self) -> Option<SplitPlan> {
        let arg = self.split_mcus? as usize;
        let (mcu_hor, mcu_ver) = self.mcu_counts();
        if arg == 0 || arg > mcu_hor * mcu_ver {
            return None;
        }
        let part_rows = (arg + mcu_hor - 1) / mcu_hor;
        if part_rows >= mcu_ver {
            return None;
        }
        Some(SplitPlan {
            part_rows,
            restart_interval: mcu_hor * part_rows,
        })
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedJpeg {
    /// Complete JPEG stream.
    pub bytes: Vec<u8>,
    /// Packet byte ranges. There is one full-stream range unless low-delay
    /// output is enabled.
    pub packets: Vec<Range<usize>>,
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
        self.encode_nv12_packets(input, config)
            .map(|encoded| encoded.bytes)
    }

    pub fn encode_nv12_packets(
        &mut self,
        input: &[u8],
        config: EncoderConfig,
    ) -> Result<EncodedJpeg, String> {
        if config.input != InputFormat::Nv12 || config.chroma != ChromaFormat::Yuv420 {
            return Err("encode_nv12 requires NV12 input and 4:2:0 output".into());
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

        self.encode_fd_packets(input_fd, config)
    }

    /// Encode an externally allocated input dma-buf. The output is still owned
    /// by this encoder so its final hardware-reported length can be read safely.
    ///
    /// The caller must finish CPU writes and flush the input dma-buf before
    /// calling this method.
    pub fn encode_fd(&mut self, input_fd: i32, config: EncoderConfig) -> Result<Vec<u8>, String> {
        self.encode_fd_packets(input_fd, config)
            .map(|encoded| encoded.bytes)
    }

    pub fn encode_fd_packets(
        &mut self,
        input_fd: i32,
        config: EncoderConfig,
    ) -> Result<EncodedJpeg, String> {
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
        let packets = packet_ranges(&result, config)?;
        Ok(EncodedJpeg {
            bytes: result,
            packets,
        })
    }
}

/// C's low-delay part loop emits the JPEG header and first band together, then
/// one packet per following restart band. The bytes are identical to the
/// single-job stream, so derive the packet views from its RST markers instead
/// of submitting the picture again band by band.
fn packet_ranges(stream: &[u8], config: EncoderConfig) -> Result<Vec<Range<usize>>, String> {
    if !config.low_delay {
        return Ok(vec![0..stream.len()]);
    }
    let plan = config
        .split_plan()
        .ok_or_else(|| "JPEG low-delay output requires a valid MCU split".to_string())?;
    let (_, mcu_ver) = config.mcu_counts();
    let expected_packets = (mcu_ver + plan.part_rows - 1) / plan.part_rows;

    let sos = find_marker(stream, SOS)
        .ok_or_else(|| "JPEG low-delay stream has no SOS marker".to_string())?;
    let sos_len = stream
        .get(sos + 2..sos + 4)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]) as usize)
        .ok_or_else(|| "JPEG low-delay stream has a truncated SOS segment".to_string())?;
    let entropy = sos
        .checked_add(2 + sos_len)
        .filter(|&start| start <= stream.len())
        .ok_or_else(|| "JPEG low-delay SOS length exceeds the stream".to_string())?;

    let mut boundaries = Vec::with_capacity(expected_packets - 1);
    let mut at = entropy;
    while at + 1 < stream.len() {
        if stream[at] != 0xff {
            at += 1;
            continue;
        }
        let marker = stream[at + 1];
        if (0xd0..=0xd7).contains(&marker) {
            boundaries.push(at + 2);
            at += 2;
        } else if marker == 0xd9 {
            break;
        } else {
            at += 2;
        }
    }

    if boundaries.len() + 1 != expected_packets {
        return Err(format!(
            "JPEG low-delay stream has {} restart markers, expected {}",
            boundaries.len(),
            expected_packets - 1
        ));
    }
    if stream.last_chunk::<2>() != Some(&[0xff, 0xd9]) {
        return Err("JPEG low-delay stream does not end with EOI".into());
    }
    boundaries.push(stream.len());

    let mut start = 0;
    let mut packets = Vec::with_capacity(boundaries.len());
    for end in boundaries {
        if end <= start {
            return Err("JPEG low-delay produced an empty packet".into());
        }
        packets.push(start..end);
        start = end;
    }
    Ok(packets)
}

fn find_marker(stream: &[u8], marker: u16) -> Option<usize> {
    let marker = marker.to_be_bytes();
    stream
        .windows(2)
        .position(|window| window == marker.as_slice())
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
    let (output_width, output_height) = config.output_size();
    write_sof0(&mut bytes, output_width, output_height, components);
    // `write_jpeg_header()` places the restart interval between SOF and DHT.
    if let Some(plan) = config.split_plan() {
        write_dri(&mut bytes, plan.restart_interval as u16);
    }
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

/// `write_jpeg_RestartInterval()`: DRI, length 4, restart interval in MCU
/// units. Emitted only when the interval is non-zero.
fn write_dri(out: &mut Vec<u8>, restart_interval: u16) {
    push_u16(out, DRI);
    push_u16(out, 4);
    push_u16(out, restart_interval);
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
    words[INPUT_FORMAT] = (config.input.vepu_format() << 4) | (config.rotation.vepu_bits() << 2);
    words[OUTPUT_ADDR] = output_fd as u32;
    // Color conversion coefficients (BT.601, `color_conversion_type` 0). The
    // hardware ignores them for YUV input and applies them for RGB input.
    words[COEFF_AB] = 19589 | (38443 << 16);
    words[COEFF_CE] = 7504 | (37008 << 16);
    words[COEFF_F] = 46740;
    words[RGB_MASK] = match config.input.rgb_masks() {
        Some((r, g, b)) => ((b as u32) & 0x1f) << 16 | ((g as u32) & 0x1f) << 8 | (r as u32) & 0x1f,
        None => 0,
    };
    words[ENCODE_CONFIG] = ((width_align >> 4) as u32) << 8
        | ((coded_height >> 4) as u32) << 20
        | (1 << 6)
        | (2 << 4)
        | 1;
    words[BYTE_SWAP] = config.input.swap_bits();
    words[PART_CONFIG] = config.split_plan().map_or(RESTART_MARKER_FIRST, |plan| {
        (((plan.part_rows & 0xff) as u32) << 16) | RESTART_MARKER_FIRST
    });
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

    #[test]
    fn rotation_sets_the_input_format_bits_and_transposes_the_sof() {
        for (rotation, bits) in [
            (Rotation::None, 0u32),
            (Rotation::Degree90, 1),
            (Rotation::Degree270, 2),
        ] {
            let config = EncoderConfig {
                quality: Quality::Quant(10),
                rotation,
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

            assert_eq!(
                regs.words[INPUT_FORMAT],
                (VEPU_FMT_YUV420SP << 4) | (bits << 2)
            );
            // SOF0 payload: P, height, width - transposed for 90/270.
            let sof = &header.bytes[158..177];
            assert_eq!(&sof[..2], &[0xff, 0xc0]);
            let (sof_height, sof_width) = if rotation.transposes_output() {
                (320u16, 240u16)
            } else {
                (240, 320)
            };
            assert_eq!(&sof[5..7], &sof_height.to_be_bytes());
            assert_eq!(&sof[7..9], &sof_width.to_be_bytes());
            // Geometry registers describe the unrotated input, as they do in
            // C after gen_regs swaps its local copies back.
            assert_eq!(regs.words[INPUT_STRIDE], 320);
            // 320/16 = 20 columns, 240/16 = 15 rows, unchanged by rotation.
            assert_eq!(regs.words[ENCODE_CONFIG], 0x00f0_1461);
        }
    }

    #[test]
    fn split_mcus_derives_the_c_restart_interval() {
        // 320x240 4:2:0 -> 20x15 MCUs, so jpege_proc_hal() maps the CTU split
        // argument onto whole MCU rows.
        for (arg, plan) in [
            (None, None),
            (Some(0), None),
            (Some(1), Some((1usize, 20usize))),
            (Some(20), Some((1, 20))),
            (Some(21), Some((2, 40))),
            (Some(39), Some((2, 40))),
            (Some(280), Some((14, 280))),
            (Some(300), None), // part_rows = 15 >= mcu_ver
            (Some(301), None), // beyond mcu_cnt = 300
            (Some(u16::MAX), None),
        ] {
            let config = EncoderConfig {
                split_mcus: arg,
                ..EncoderConfig::new(320, 240)
            };
            assert_eq!(
                config
                    .split_plan()
                    .map(|p| (p.part_rows, p.restart_interval)),
                plan,
                "split_mcus {arg:?}"
            );
        }
    }

    #[test]
    fn low_delay_packets_end_at_the_restart_markers() {
        let stream = [
            0xff, 0xd8, // SOI
            0xff, 0xda, 0x00, 0x02, // SOS, then entropy starts at byte 6
            1, 2, 0xff, 0xd0, 3, 0xff, 0xd1, 4, 0xff, 0xd2, 5, 0xff, 0xd3, 6, 7, 0xff, 0xd9,
        ];
        let config = EncoderConfig {
            split_mcus: Some(60),
            low_delay: true,
            ..EncoderConfig::new(320, 240)
        };
        assert_eq!(
            packet_ranges(&stream, config).unwrap(),
            vec![0..10, 10..13, 13..16, 16..19, 19..23]
        );
    }

    #[test]
    fn low_delay_packet_count_must_match_the_split_plan() {
        let short = [
            0xff, 0xd8, 0xff, 0xda, 0x00, 0x02, 1, 0xff, 0xd0, 2, 0xff, 0xd9,
        ];
        let config = EncoderConfig {
            split_mcus: Some(60),
            low_delay: true,
            ..EncoderConfig::new(320, 240)
        };
        assert!(packet_ranges(&short, config).is_err());
    }

    #[test]
    fn low_delay_output_needs_a_valid_split() {
        let mut config = EncoderConfig::new(320, 240);
        config.low_delay = true;
        assert!(config.validate().is_err());
        config.split_mcus = Some(60);
        assert!(config.validate().is_ok());
        config.rotation = Rotation::Degree90;
        assert!(config.validate().is_err());
        config.rotation = Rotation::None;
        config.width = 1920;
        config.height = 1080;
        config.hor_stride = 1920;
        config.ver_stride = 1080;
        assert!(config.validate().is_err());
    }

    #[test]
    fn dri_marker_sits_between_sof_and_dht() {
        let config = EncoderConfig {
            quality: Quality::Quant(10),
            split_mcus: Some(40),
            ..EncoderConfig::new(640, 640)
        };
        let plain = prepare_header(EncoderConfig {
            quality: Quality::Quant(10),
            ..EncoderConfig::new(640, 640)
        })
        .unwrap();
        let header = prepare_header(config).unwrap();
        assert_eq!(header.bytes.len(), plain.bytes.len() + 6);

        // Same SOF offset as the no-split layout, DRI right after it, then DHT.
        let sof_end = 158 + (8 + 3 * 3) + 2;
        // 640x640 -> 40x40 MCUs; split 40 -> 1 row -> interval 40 MCUs.
        assert_eq!(
            &header.bytes[sof_end..sof_end + 6],
            &[0xff, 0xdd, 0x00, 0x04, 0x00, 0x28]
        );
        assert_eq!(&header.bytes[sof_end + 6..sof_end + 8], &[0xff, 0xc4]);

        let regs = build_registers(
            7,
            9,
            config,
            header.bytes.len(),
            &header.bytes,
            &header.quant,
        )
        .unwrap();
        // part_rows 40 MCUs / 40 columns = 1 row, and the marker cycle
        // starts at RST0.
        assert_eq!(regs.words[PART_CONFIG], (1 << 16) | RESTART_MARKER_FIRST);
        assert_eq!(
            regs.words[STREAM_START] >> 16,
            ((header.bytes.len() & 7) * 8) as u32
        );
    }

    #[test]
    fn restart_interval_guard_keeps_dri_inside_16_bits() {
        let mut config = EncoderConfig::new_for_input(32_760, 544, InputFormat::Nv12);
        // 2048 MCU columns: the widest split argument (65,535 MCUs) makes
        // 32 bands and an interval of 65,536 - one past the DRI field that C
        // would silently truncate.
        config.split_mcus = Some(u16::MAX);
        let plan = config.split_plan().unwrap();
        assert_eq!((plan.part_rows, plan.restart_interval), (32, 65_536));
        assert!(plan.restart_interval > u16::MAX as usize);
        assert!(config.validate().is_err());
        config.split_mcus = Some(63_488);
        assert_eq!(config.split_plan().unwrap().restart_interval, 63_488);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rgb_inputs_program_format_masks_and_swaps() {
        // Words per format, straight from the vepu_rgb_cfg rows in
        // vepu_common.c that the non-LE MPP_FMT_* values select.
        let expectations: [(InputFormat, u32, u32); 12] = [
            (InputFormat::Rgb565, 4, 0x04_0a_0f),
            (InputFormat::Bgr565, 4, 0x0f_0a_04),
            (InputFormat::Rgb555, 5, 0x04_09_0e),
            (InputFormat::Bgr555, 5, 0x0e_09_04),
            (InputFormat::Rgb444, 6, 0x03_07_0b),
            (InputFormat::Bgr444, 6, 0x0b_07_03),
            (InputFormat::Argb8888, 7, 0x1f_17_0f),
            (InputFormat::Abgr8888, 7, 0x0f_17_1f),
            (InputFormat::Bgra8888, 7, 0x07_0f_17),
            (InputFormat::Rgba8888, 7, 0x17_0f_07),
            (InputFormat::Rgb101010, 8, 0x09_13_1d),
            (InputFormat::Bgr101010, 8, 0x1d_13_09),
        ];
        for (input, format, mask) in expectations {
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

            let (r, g, b) = input.rgb_masks().unwrap();
            assert_eq!(mask, ((b as u32) << 16) | ((g as u32) << 8) | r as u32);
            assert_eq!(regs.words[RGB_MASK], mask);
            assert_eq!(regs.words[INPUT_FORMAT], format << 4);
            assert_eq!(regs.words[BYTE_SWAP], config.input.swap_bits());
            assert_eq!(
                regs.words[INPUT_STRIDE],
                (config.hor_stride / input.bytes_per_pixel()) as u32
            );
            assert_eq!(config.input_size().unwrap(), config.hor_stride * 240);
            assert_eq!(
                regs.offsets,
                [RegOffsetInfo {
                    reg_idx: OUTPUT_ADDR as u32,
                    offset: header.bytes.len() as u32,
                }]
            );
        }
        // The 32-bit rows swap 32 in only; every other RGB row swaps all.
        for input in [
            InputFormat::Argb8888,
            InputFormat::Abgr8888,
            InputFormat::Bgra8888,
            InputFormat::Rgba8888,
        ] {
            assert_eq!(
                EncoderConfig::new_for_input(320, 240, input)
                    .input
                    .swap_bits(),
                RGB_32BIT_SWAP_BITS
            );
        }
        for input in [
            InputFormat::Rgb565,
            InputFormat::Bgr565,
            InputFormat::Rgb555,
            InputFormat::Bgr555,
            InputFormat::Rgb444,
            InputFormat::Bgr444,
            InputFormat::Rgb101010,
            InputFormat::Bgr101010,
        ] {
            assert_eq!(
                EncoderConfig::new_for_input(320, 240, input)
                    .input
                    .swap_bits(),
                RGB_FULL_SWAP_BITS
            );
        }
    }

    #[test]
    fn rgb_stride_rules_follow_the_vepu_pixel_alignment() {
        // 32-bit formats need an 8-pixel-aligned byte stride (32 bytes).
        let mut config = EncoderConfig::new_for_input(320, 240, InputFormat::Argb8888);
        assert_eq!(config.hor_stride, 320 * 4);
        assert!(config.validate().is_ok());
        config.hor_stride = 320 * 4 + 4;
        assert!(config.validate().is_err());

        let mut config = EncoderConfig::new_for_input(320, 240, InputFormat::Rgb565);
        assert_eq!(config.hor_stride, 320 * 2);
        assert!(config.validate().is_ok());
        config.hor_stride = 320 * 2 - 2;
        assert!(config.validate().is_err());
    }

    #[test]
    fn rotation_does_not_move_the_input_geometry() {
        let config = EncoderConfig {
            rotation: Rotation::Degree90,
            ..EncoderConfig::new(321, 241)
        };
        assert_eq!((config.hor_stride, config.ver_stride), (328, 256));
        assert_eq!(config.output_size(), (241, 321));
        assert_eq!(config.input_size().unwrap(), 328 * 256 * 3 / 2);
        // The MCU grid is counted from the transposed output.
        assert_eq!(
            config.mcu_counts(),
            (align(241, 16) / 16, align(321, 16) / 16)
        );
    }
}
