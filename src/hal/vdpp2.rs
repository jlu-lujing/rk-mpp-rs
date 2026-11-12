//! RK3576 VDPP2 (client 29) register generation.
//!
//! The hardware-independent part is shared between the board driver and the
//! register tests. It covers NV12 input scaling to NV12 or NV12-planar/YUV444
//! output plus the DCI histogram side path. The default enhancement banks can
//! be disabled for scale-only comparisons and debugging.

use super::vdpp2_coeff::{TAP6, TAP8};
use super::vdpp2_enhance::{build_dmsr, build_es};
pub use super::vdpp2_enhance::{Vdpp2DmsrConfig, Vdpp2EsConfig};
use super::vdpp2_sharp::build_sharp;
pub use super::vdpp2_sharp::Vdpp2SharpConfig;

pub const COMMON_WORDS: usize = 28;
pub const HIST_WORDS: usize = 5;
pub const HIST_LOCAL_BINS: usize = 16 * 16 * 16;
pub const HIST_GLOBAL_BINS: usize = 256;
pub const HIST_PACKED_BYTES: usize = 10_240;
pub const DMSR_WORDS: usize = 21;
pub const ES_WORDS: usize = 32;
pub const SHARP_WORDS: usize = 172;
pub const ZME_COMMON_WORDS: usize = 18;
pub const ZME_COEFF_WORDS: usize = 68;

const HIST_LOCAL_PACKED_BYTES: usize = HIST_LOCAL_BINS / 4 * 9;
const STATUS_FRAME_DONE: u32 = 1 << 0;
const STATUS_BUS_ERROR: u32 = 1 << 4;
const STATUS_TIMEOUT: u32 = 1 << 5;
const STATUS_CONFIG_ERROR: u32 = 1 << 6;

pub const REG_OFF_DMSR: u32 = 0x0080;
pub const REG_OFF_DCI: u32 = 0x00e0;
pub const REG_OFF_ES: u32 = 0x0100;
pub const REG_OFF_SHARP: u32 = 0x0200;
pub const REG_OFF_YRGB_HOR_COE: u32 = 0x2000;
pub const REG_OFF_YRGB_VER_COE: u32 = 0x2200;
pub const REG_OFF_CBCR_HOR_COE: u32 = 0x2400;
pub const REG_OFF_CBCR_VER_COE: u32 = 0x2600;
pub const REG_OFF_ZME_COMMON: u32 = 0x2800;

pub const VEP_MAX_WIDTH: usize = 1920;
pub const VEP_MAX_HEIGHT: usize = 2048;
pub const HIST_MAX_WIDTH: usize = 4096;
pub const HIST_MAX_HEIGHT: usize = 4096;
pub const MODE_MIN_WIDTH: usize = 128;
pub const MODE_MIN_HEIGHT: usize = 128;

const SCALE_FACTOR_DN_SHIFT: u32 = 12;
const SCALE_FACTOR_UP_SHIFT: u32 = 16;
const SCALE_THRESHOLDS: [u32; 9] = [2667, 2000, 1500, 1000, 833, 700, 500, 330, 250];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Vdpp2Format {
    Yuv444 = 0,
    Yuv420 = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Vdpp2Swap {
    SpUv = 0,
    SpVu = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Vdpp2Mode {
    Vep = 2,
    Histogram = 3,
}

pub fn check_completion_status(status: u32) -> Result<(), String> {
    let errors = status & (STATUS_BUS_ERROR | STATUS_TIMEOUT | STATUS_CONFIG_ERROR);
    if errors != 0 {
        return Err(format!("VDPP2 hardware error status: {status:08x}"));
    }
    if status & STATUS_FRAME_DONE == 0 {
        return Err(format!(
            "VDPP2 did not report frame completion: {status:08x}"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vdpp2HistogramConfig {
    pub enable: bool,
    pub horizontal_downsample: u32,
    pub vertical_downsample: u32,
    pub gather_num: u32,
    pub gather_enable: bool,
    pub full_range: bool,
}

impl Default for Vdpp2HistogramConfig {
    fn default() -> Self {
        Self {
            enable: true,
            horizontal_downsample: 0,
            vertical_downsample: 0,
            gather_num: 0,
            gather_enable: false,
            full_range: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vdpp2Histogram {
    pub local: [u32; HIST_LOCAL_BINS],
    pub global: [u32; HIST_GLOBAL_BINS],
}

impl Vdpp2Histogram {
    pub fn parse(packed: &[u8]) -> Result<Self, String> {
        if packed.len() < HIST_PACKED_BYTES {
            return Err(format!(
                "VDPP2 packed histogram is {} bytes, expected at least {HIST_PACKED_BYTES}",
                packed.len()
            ));
        }

        let mut local = [0u32; HIST_LOCAL_BINS];
        for (packed, values) in packed[..HIST_LOCAL_PACKED_BYTES]
            .chunks_exact(9)
            .zip(local.chunks_exact_mut(4))
        {
            values[0] = u32::from(packed[0])
                | (u32::from(packed[1]) << 8)
                | ((u32::from(packed[2]) & 0x03) << 16);
            values[1] = (u32::from(packed[2]) >> 2)
                | (u32::from(packed[3]) << 6)
                | ((u32::from(packed[4]) & 0x0f) << 14);
            values[2] = (u32::from(packed[4]) >> 4)
                | (u32::from(packed[5]) << 4)
                | ((u32::from(packed[6]) & 0x3f) << 12);
            values[3] = (u32::from(packed[6]) >> 6)
                | (u32::from(packed[7]) << 2)
                | (u32::from(packed[8]) << 10);
        }

        let mut global = [0u32; HIST_GLOBAL_BINS];
        for (packed, value) in packed[HIST_LOCAL_PACKED_BYTES..HIST_PACKED_BYTES]
            .chunks_exact(4)
            .zip(global.iter_mut())
        {
            *value = u32::from_le_bytes(
                packed
                    .try_into()
                    .expect("VDPP2 global histogram chunk is four bytes"),
            );
        }

        Ok(Self { local, global })
    }

    /// Return the vendor U10 mean luma, or `None` when the histogram is empty.
    pub fn mean_luma_u10(&self) -> Option<u32> {
        let mut luma_sum = 0u64;
        let mut pixel_count = 0u64;
        for (bin, count) in self.global.iter().enumerate() {
            luma_sum += u64::from(*count) * (bin as u64 * 8 + 3);
            pixel_count += u64::from(*count);
        }
        (pixel_count != 0).then_some((luma_sum / pixel_count / 2) as u32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vdpp2Config {
    pub src_width: usize,
    pub src_height: usize,
    pub src_width_vir: usize,
    pub src_height_vir: usize,
    pub src_swap: Vdpp2Swap,
    pub dst_width: usize,
    pub dst_height: usize,
    pub dst_width_vir: usize,
    pub dst_height_vir: usize,
    pub dst_format: Vdpp2Format,
    pub dst_swap: Vdpp2Swap,
    pub yuv_out_diff: bool,
    pub dst_c_width: usize,
    pub dst_c_height: usize,
    pub dst_c_width_vir: usize,
    pub dst_c_height_vir: usize,
    pub mode: Vdpp2Mode,
    /// Use the vendor default DMSR/ES/sharp register banks. Disable this for
    /// a deterministic scale-only path.
    pub enhancement: bool,
    pub dmsr: Vdpp2DmsrConfig,
    pub es: Vdpp2EsConfig,
    pub sharp: Vdpp2SharpConfig,
    pub histogram: Vdpp2HistogramConfig,
}

impl Vdpp2Config {
    pub fn nv12_to_nv12(
        src_width: usize,
        src_height: usize,
        dst_width: usize,
        dst_height: usize,
    ) -> Self {
        Self {
            src_width,
            src_height,
            src_width_vir: align(src_width, 16),
            src_height_vir: align(src_height, 8),
            src_swap: Vdpp2Swap::SpUv,
            dst_width,
            dst_height,
            dst_width_vir: align(dst_width, 16),
            dst_height_vir: align(dst_height, 2),
            dst_format: Vdpp2Format::Yuv420,
            dst_swap: Vdpp2Swap::SpUv,
            yuv_out_diff: false,
            dst_c_width: dst_width,
            dst_c_height: dst_height,
            dst_c_width_vir: align(dst_width, 16),
            dst_c_height_vir: align(dst_height, 2),
            mode: Vdpp2Mode::Vep,
            enhancement: true,
            dmsr: Vdpp2DmsrConfig::default(),
            es: Vdpp2EsConfig::default(),
            sharp: Vdpp2SharpConfig::default(),
            histogram: Vdpp2HistogramConfig::default(),
        }
    }

    pub fn nv12_to_yuv444(
        src_width: usize,
        src_height: usize,
        dst_width: usize,
        dst_height: usize,
    ) -> Self {
        Self {
            dst_format: Vdpp2Format::Yuv444,
            ..Self::nv12_to_nv12(src_width, src_height, dst_width, dst_height)
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.yuv_out_diff {
            return Err("VDPP2 split Y/UV output is not wired yet".to_string());
        }
        if self.src_width == 0 || self.src_height == 0 {
            return Err("VDPP2 source has a zero dimension".to_string());
        }
        if self.src_width_vir < self.src_width
            || self.src_height_vir < self.src_height
            || self.src_width_vir < MODE_MIN_WIDTH
            || self.src_height_vir < MODE_MIN_HEIGHT
            || self.src_width < MODE_MIN_WIDTH
            || self.src_height < MODE_MIN_HEIGHT
        {
            return Err(format!(
                "VDPP2 source geometry {}x{} / vir {}x{} is outside the supported range",
                self.src_width, self.src_height, self.src_width_vir, self.src_height_vir
            ));
        }
        match self.mode {
            Vdpp2Mode::Vep => {
                if self.src_width_vir > VEP_MAX_WIDTH
                    || self.src_height_vir > VEP_MAX_HEIGHT
                    || self.dst_width_vir > VEP_MAX_WIDTH
                    || self.dst_height_vir > VEP_MAX_HEIGHT
                {
                    return Err("VDPP2 VEP geometry exceeds 1920x2048".to_string());
                }
                if self.dst_width < MODE_MIN_WIDTH
                    || self.dst_height < MODE_MIN_HEIGHT
                    || self.dst_width_vir < MODE_MIN_WIDTH
                    || self.dst_height_vir < MODE_MIN_HEIGHT
                {
                    return Err("VDPP2 destination is below the 128x128 minimum".to_string());
                }
                if self.src_width_vir & 15 != 0
                    || self.dst_width_vir & 15 != 0
                    || self.src_height_vir & 7 != 0
                    || self.dst_height_vir & 1 != 0
                {
                    return Err("VDPP2 VEP stride alignment is invalid".to_string());
                }
                if self.src_width & 1 != 0
                    || self.src_height & 1 != 0
                    || self.dst_width & 1 != 0
                    || self.dst_height & 1 != 0
                {
                    return Err("VDPP2 VEP visible geometry must be even".to_string());
                }
            }
            Vdpp2Mode::Histogram => {
                if self.src_width_vir > HIST_MAX_WIDTH || self.src_height_vir > HIST_MAX_HEIGHT {
                    return Err("VDPP2 histogram source exceeds 4096x4096".to_string());
                }
                if self.src_width_vir & 3 != 0 {
                    return Err("VDPP2 histogram source stride must be 4-byte aligned".to_string());
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Vdpp2Job {
    pub src_fd: i32,
    pub src_uv_offset: u32,
    pub dst_fd: i32,
    pub dst_uv_offset: u32,
    pub hist_fd: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vdpp2Registers {
    pub yrgb_hor_coe: [u32; ZME_COEFF_WORDS],
    pub yrgb_ver_coe: [u32; ZME_COEFF_WORDS],
    pub cbcr_hor_coe: [u32; ZME_COEFF_WORDS],
    pub cbcr_ver_coe: [u32; ZME_COEFF_WORDS],
    pub zme_common: [u32; ZME_COMMON_WORDS],
    pub dmsr: [u32; DMSR_WORDS],
    pub hist: [u32; HIST_WORDS],
    pub es: [u32; ES_WORDS],
    pub sharp: [u32; SHARP_WORDS],
    pub common: [u32; COMMON_WORDS],
}

impl Vdpp2Registers {
    pub fn build(config: &Vdpp2Config, job: &Vdpp2Job) -> Result<Self, String> {
        config.validate()?;
        let mut common = [0u32; COMMON_WORDS];
        common[0] = 1;
        common[1] = field(3, 0, 2)
            | field(config.src_swap as u32, 4, 2)
            | field(config.dst_format as u32, 8, 2)
            | field(u32::from(config.yuv_out_diff), 10, 1)
            | field(config.dst_swap as u32, 12, 2)
            | field(u32::from(config.histogram.enable), 25, 1)
            | field(u32::from(config.enhancement && config.dmsr.enabled), 24, 1);
        common[2] = config.mode as u32;
        common[4] = 0x0fff;
        common[8] =
            field(1, 0, 1) | field(1, 1, 1) | field(1, 4, 1) | field(1, 5, 1) | field(1, 6, 1);
        common[12] = (config.src_width_vir / 4) as u32;
        let dst_c_stride = if config.yuv_out_diff {
            config.dst_c_width_vir / 4
        } else {
            0
        };
        common[13] =
            field(config.dst_width_vir as u32 / 4, 0, 16) | field(dst_c_stride as u32, 16, 16);

        let src_right = redundant(config.src_width, 16);
        let src_down = redundant(config.src_height, 8);
        let dst_right = redundant(config.dst_width, 16);
        common[14] = field((config.src_width + src_right - 1) as u32, 0, 11)
            | field(src_right as u32, 12, 4)
            | field((config.src_height + src_down - 1) as u32, 16, 11)
            | field(src_down as u32, 28, 3);
        common[15] = field((config.dst_width + dst_right - 1) as u32, 0, 11)
            | field(dst_right as u32, 12, 4)
            | field((config.dst_height - 1) as u32, 16, 11);
        if config.yuv_out_diff {
            let dst_c_right = redundant(config.dst_c_width, 16);
            common[16] = field((config.dst_c_width + dst_c_right - 1) as u32, 0, 11)
                | field(dst_c_right as u32, 12, 4)
                | field((config.dst_c_height - 1) as u32, 16, 11);
        }
        common[20] = 0x88ff_ffff;
        common[24] = job.src_fd as u32;
        common[25] = job.src_fd as u32;
        common[26] = job.dst_fd as u32;
        common[27] = job.dst_fd as u32;

        let mut hist = [0u32; HIST_WORDS];
        build_histogram(config, job, &mut hist, &mut common);

        let (yrgb_hor_coe, yrgb_ver_coe, cbcr_hor_coe, cbcr_ver_coe, zme_common) =
            if config.mode == Vdpp2Mode::Vep {
                build_zme(config)
            } else {
                (
                    [0; ZME_COEFF_WORDS],
                    [0; ZME_COEFF_WORDS],
                    [0; ZME_COEFF_WORDS],
                    [0; ZME_COEFF_WORDS],
                    [0; ZME_COMMON_WORDS],
                )
            };

        // Keep every bank in the submission so scale-only and the vendor default
        // enhancement path exercise the same register-window layout.
        let (dmsr, es, sharp) = if config.enhancement {
            (
                build_dmsr(&config.dmsr),
                build_es(&config.es),
                build_sharp(&config.sharp),
            )
        } else {
            ([0u32; DMSR_WORDS], [0u32; ES_WORDS], [0u32; SHARP_WORDS])
        };
        Ok(Self {
            yrgb_hor_coe,
            yrgb_ver_coe,
            cbcr_hor_coe,
            cbcr_ver_coe,
            zme_common,
            dmsr,
            hist,
            es,
            sharp,
            common,
        })
    }
}

fn build_histogram(
    config: &Vdpp2Config,
    job: &Vdpp2Job,
    hist: &mut [u32; HIST_WORDS],
    common: &mut [u32; COMMON_WORDS],
) {
    let hist_cfg = config.histogram;
    let mut hsd = hist_cfg.horizontal_downsample & 1;
    let mut vsd = hist_cfg.vertical_downsample & 3;
    if hsd & 1 == 0 && config.src_width_vir > 1920 {
        hsd = 1;
    }
    if vsd & 1 == 0 && config.src_height_vir > 1080 {
        vsd = 1;
    }
    if vsd & 2 == 0 && config.src_height_vir > 2160 {
        vsd = 2;
    }
    let hsd_sample = if hsd == 1 { 4 } else { 2 };
    let vsd_sample = match vsd {
        1 => 2,
        2 => 4,
        _ => 1,
    };
    let block_h = if config.src_width < 1080 {
        config.src_width / (16 * hsd_sample)
    } else {
        (config.src_width + 16 * hsd_sample - 1) / (16 * hsd_sample)
    };
    let block_v = if config.src_height < 1080 {
        config.src_height / (16 * vsd_sample)
    } else {
        (config.src_height + 16 * vsd_sample - 1) / (16 * vsd_sample)
    };
    hist[0] = job.src_fd as u32;
    hist[1] = field((config.src_width_vir / 4) as u32, 0, 16)
        | field(hist_cfg.gather_num, 16, 4)
        | field(u32::from(hist_cfg.gather_enable), 20, 1);
    hist[2] = field(
        (config.src_width / hsd_sample * hsd_sample - 1) as u32,
        0,
        12,
    ) | field(
        (config.src_height / vsd_sample * vsd_sample - 1) as u32,
        16,
        12,
    );
    hist[3] = field(4, 0, 3)
        | field(u32::from(hist_cfg.full_range), 3, 1)
        | field(u32::from(config.mode == Vdpp2Mode::Histogram) * vsd, 4, 2)
        | field(u32::from(config.mode == Vdpp2Mode::Histogram) * hsd, 6, 1)
        | field(block_h as u32, 16, 8)
        | field(block_v as u32, 24, 8);
    hist[4] = job.hist_fd as u32;
    common[1] |= field(u32::from(hist_cfg.enable), 25, 1);
}

#[derive(Debug, Clone, Copy)]
struct ScaleInfo {
    act_width: u16,
    act_height: u16,
    dst_width: u16,
    dst_height: u16,
    xsd: bool,
    xsu: bool,
    ysd: bool,
    ysu: bool,
    dering: bool,
    x_factor: u16,
    y_factor: u16,
    x_gt: bool,
    x_gt_mode: u8,
    y_gt: bool,
    y_gt_mode: u8,
    xsd_bypass: bool,
    xsu_bypass: bool,
    ys_bypass: bool,
    x_scaling: &'static [[i16; 8]; 17],
    y_scaling: &'static [[i16; 8]; 17],
}

fn scale_info(act_width: u32, act_height: u32, dst_width: u32, dst_height: u32) -> ScaleInfo {
    let mut act_width = act_width;
    let mut x_gt = false;
    let mut x_gt_mode = 0;
    if act_width >= dst_width * 14 {
        act_width /= 4;
        x_gt = true;
        x_gt_mode = 3;
    } else if act_width >= dst_width * 7 {
        act_width /= 2;
        x_gt = true;
        x_gt_mode = 1;
    }
    let (xsd, xsu, x_factor) = scale_factor(act_width, dst_width);
    let mut y_gt = false;
    let mut y_gt_mode = 0;
    if act_height >= dst_height * 6 {
        y_gt = true;
        y_gt_mode = 3;
    }
    let act_height_scaled = act_height;
    let (ysd, ysu, y_factor) = scale_factor(act_height_scaled, dst_height);
    let coeff = select_tap8(act_width, dst_width, xsu);
    let y_coeff = select_tap6(act_height_scaled, dst_height, ysu);
    ScaleInfo {
        act_width: act_width as u16,
        act_height: act_height_scaled as u16,
        dst_width: dst_width as u16,
        dst_height: dst_height as u16,
        xsd,
        xsu,
        ysd,
        ysu,
        dering: true,
        x_factor,
        y_factor,
        x_gt,
        x_gt_mode,
        y_gt,
        y_gt_mode,
        xsd_bypass: !xsd,
        xsu_bypass: !xsu,
        ys_bypass: !(ysd || ysu),
        x_scaling: coeff,
        y_scaling: y_coeff,
    }
}

fn scale_factor(src: u32, dst: u32) -> (bool, bool, u16) {
    if src > dst {
        (
            true,
            false,
            (((src - 1) << SCALE_FACTOR_DN_SHIFT) / (dst - 1)) as u16,
        )
    } else if src < dst {
        (
            false,
            true,
            (((src - 1) << SCALE_FACTOR_UP_SHIFT) / (dst - 1)) as u16,
        )
    } else {
        (false, false, 1 << SCALE_FACTOR_DN_SHIFT)
    }
}

fn scale_row(src: u32, dst: u32, up: bool) -> usize {
    let factor = if up {
        let scaled = (src as u64 * 65_536) / dst as u64;
        1_000 * 65_536 / scaled
    } else {
        let scaled = (src as u64 * 4_096) / dst as u64;
        1_000 * 4_096 / scaled
    };
    if factor >= u64::from(SCALE_THRESHOLDS[0]) {
        0
    } else if factor >= u64::from(SCALE_THRESHOLDS[1]) {
        1
    } else if factor >= u64::from(SCALE_THRESHOLDS[2]) {
        2
    } else if factor > u64::from(SCALE_THRESHOLDS[3]) {
        3
    } else if factor == u64::from(SCALE_THRESHOLDS[3]) {
        4
    } else if factor >= u64::from(SCALE_THRESHOLDS[4]) {
        5
    } else if factor >= u64::from(SCALE_THRESHOLDS[5]) {
        6
    } else if factor >= u64::from(SCALE_THRESHOLDS[6]) {
        7
    } else if factor >= u64::from(SCALE_THRESHOLDS[7]) {
        8
    } else if factor >= u64::from(SCALE_THRESHOLDS[8]) {
        9
    } else {
        10
    }
}

fn select_tap8(src: u32, dst: u32, up: bool) -> &'static [[i16; 8]; 17] {
    &TAP8[scale_row(src, dst, up)]
}

fn select_tap6(src: u32, dst: u32, up: bool) -> &'static [[i16; 8]; 17] {
    &TAP6[scale_row(src, dst, up)]
}

fn build_zme(
    config: &Vdpp2Config,
) -> (
    [u32; ZME_COEFF_WORDS],
    [u32; ZME_COEFF_WORDS],
    [u32; ZME_COEFF_WORDS],
    [u32; ZME_COEFF_WORDS],
    [u32; ZME_COMMON_WORDS],
) {
    let y = scale_info(
        config.src_width as u32,
        config.src_height as u32,
        config.dst_width as u32,
        config.dst_height as u32,
    );
    let (chroma_dst_w, chroma_dst_h) = match config.dst_format {
        Vdpp2Format::Yuv444 => (config.dst_width, config.dst_height),
        Vdpp2Format::Yuv420 => (config.dst_width / 2, config.dst_height / 2),
    };
    let c = scale_info(
        (config.src_width / 2) as u32,
        (config.src_height / 2) as u32,
        chroma_dst_w as u32,
        chroma_dst_h as u32,
    );
    let mut common = [0u32; ZME_COMMON_WORDS];
    common[0] = field(1, 31, 1)
        | field(4, 4, 4)
        | field(
            if config.dst_format == Vdpp2Format::Yuv444 {
                6
            } else {
                4
            },
            8,
            4,
        );
    common[3] = field(config.src_width as u32, 0, 16) | field(config.src_height as u32, 16, 16);
    common[4] = zme_flags(y);
    common[5] = field(y.act_width as u32 - 1, 0, 16) | field(y.act_height as u32 - 1, 16, 16);
    common[6] = field(y.dst_width as u32 - 1, 0, 16) | field(y.dst_height as u32 - 1, 16, 16);
    common[7] = field(16, 0, 5) | field(4, 8, 5) | field(16, 16, 5) | field(13, 24, 5);
    common[8] = field(y.x_factor as u32, 0, 16);
    common[9] = field(y.y_factor as u32, 0, 16);
    common[12] = zme_flags(c);
    common[13] = field(c.act_width as u32 - 1, 0, 16) | field(c.act_height as u32 - 1, 16, 16);
    common[14] = field(c.dst_width as u32 - 1, 0, 16) | field(c.dst_height as u32 - 1, 16, 16);
    common[15] = field(16, 0, 5) | field(4, 8, 5) | field(16, 16, 5) | field(13, 24, 5);
    common[16] = field(c.x_factor as u32, 0, 16);
    common[17] = field(c.y_factor as u32, 0, 16);
    (
        pack_coefficients(y.x_scaling),
        pack_coefficients(y.y_scaling),
        pack_coefficients(c.x_scaling),
        pack_coefficients(c.y_scaling),
        common,
    )
}

fn zme_flags(info: ScaleInfo) -> u32 {
    field(u32::from(info.xsd), 0, 1)
        | field(u32::from(info.xsu), 1, 1)
        | field(3, 2, 2)
        | field(u32::from(info.ysd), 4, 1)
        | field(u32::from(info.ysu), 5, 1)
        | field(3, 6, 2)
        | field(u32::from(info.dering), 8, 1)
        | field(u32::from(info.y_gt), 9, 1)
        | field(info.y_gt_mode as u32, 10, 2)
        | field(u32::from(info.x_gt), 12, 1)
        | field(info.x_gt_mode as u32, 14, 2)
        | field(u32::from(info.xsd_bypass), 16, 1)
        | field(u32::from(info.ys_bypass), 17, 1)
        | field(u32::from(info.xsu_bypass), 18, 1)
}

fn pack_coefficients(table: &[[i16; 8]; 17]) -> [u32; ZME_COEFF_WORDS] {
    let mut words = [0u32; ZME_COEFF_WORDS];
    for (row, coefficients) in table.iter().enumerate() {
        for pair in 0..4 {
            words[row * 4 + pair] = field((coefficients[pair * 2] as u32) & 0x3ff, 0, 10)
                | field((coefficients[pair * 2 + 1] as u32) & 0x3ff, 16, 10);
        }
    }
    words
}

const fn align(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) / alignment * alignment
}

const fn redundant(value: usize, alignment: usize) -> usize {
    if value % alignment == 0 {
        0
    } else {
        alignment - value % alignment
    }
}

const fn field(value: u32, shift: u32, width: u32) -> u32 {
    (value & ((1u32 << width) - 1)) << shift
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_geometry_validates_alignment_and_limits() {
        assert!(Vdpp2Config::nv12_to_nv12(1920, 1080, 640, 640)
            .validate()
            .is_ok());
        assert!(Vdpp2Config::nv12_to_nv12(127, 240, 640, 640)
            .validate()
            .is_err());
        assert!(Vdpp2Config::nv12_to_nv12(1920, 1080, 642, 640)
            .validate()
            .is_ok());
        let mut oversized = Vdpp2Config::nv12_to_nv12(1920, 1080, 320, 240);
        oversized.src_width_vir = 1936;
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn histogram_geometry_only_requires_a_four_byte_source_stride() {
        let mut cfg = Vdpp2Config::nv12_to_yuv444(132, 130, 128, 128);
        cfg.mode = Vdpp2Mode::Histogram;
        cfg.src_width_vir = 132;
        cfg.src_height_vir = 130;
        assert!(cfg.validate().is_ok());

        cfg.src_width_vir = 130;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn completion_status_rejects_hardware_errors() {
        assert!(check_completion_status(STATUS_FRAME_DONE).is_ok());
        assert!(check_completion_status(0).is_err());
        for error in [STATUS_BUS_ERROR, STATUS_TIMEOUT, STATUS_CONFIG_ERROR] {
            assert!(check_completion_status(STATUS_FRAME_DONE | error).is_err());
        }
    }

    #[test]
    fn scale_factor_tables_match_the_c_thresholds() {
        assert_eq!(scale_factor(1920, 320), (true, false, 24_640));
        assert_eq!(scale_factor(128, 1920), (false, true, 4_337));
        assert_eq!(scale_factor(640, 640), (false, false, 4096));
    }

    #[test]
    fn common_registers_encode_geometry_and_timeout() {
        let cfg = Vdpp2Config::nv12_to_nv12(720, 480, 1920, 1080);
        let regs = Vdpp2Registers::build(
            &cfg,
            &Vdpp2Job {
                src_fd: 7,
                src_uv_offset: 720 * 480,
                dst_fd: 8,
                dst_uv_offset: 1920 * 1080,
                hist_fd: 9,
            },
        )
        .unwrap();
        assert_eq!(regs.common[0], 1);
        assert_eq!(regs.common[2], Vdpp2Mode::Vep as u32);
        assert_eq!(regs.common[12], 720 / 4);
        assert_eq!(regs.common[24], 7);
        assert_eq!(regs.common[26], 8);
        assert_eq!(regs.common[20] & 0x8000_0000, 0x8000_0000);
    }

    #[test]
    fn dmsr_enable_controls_the_common_enhancement_bit() {
        let mut cfg = Vdpp2Config::nv12_to_nv12(720, 480, 1920, 1080);
        cfg.dmsr.enabled = false;
        let regs = Vdpp2Registers::build(
            &cfg,
            &Vdpp2Job {
                src_fd: 7,
                src_uv_offset: 720 * 480,
                dst_fd: 8,
                dst_uv_offset: 1920 * 1080,
                hist_fd: 9,
            },
        )
        .unwrap();
        assert_eq!(regs.common[1] & (1 << 24), 0);
    }

    #[test]
    fn histogram_word_encodes_yuv420_y_plane() {
        let mut cfg = Vdpp2Config::nv12_to_nv12(640, 480, 640, 480);
        cfg.mode = Vdpp2Mode::Histogram;
        cfg.histogram.enable = true;
        let regs = Vdpp2Registers::build(
            &cfg,
            &Vdpp2Job {
                src_fd: 11,
                src_uv_offset: 640 * 480,
                dst_fd: 12,
                dst_uv_offset: 640 * 480,
                hist_fd: 13,
            },
        )
        .unwrap();
        assert_eq!(regs.hist[0], 11);
        assert_eq!(regs.hist[3] & 7, 4);
        assert_eq!(regs.hist[4], 13);
    }

    #[test]
    fn coefficient_pack_preserves_signed_taps() {
        let packed = pack_coefficients(&TAP8[0]);
        assert_eq!(packed[0] & 0x3ff, 4);
        assert_eq!((packed[0] >> 16) & 0x3ff, (-12i16 as u32) & 0x3ff);
    }

    #[test]
    fn vendor_c_720_to_1920_scale_words_are_locked() {
        let mut cfg = Vdpp2Config::nv12_to_yuv444(720, 480, 1920, 1080);
        cfg.enhancement = false;
        let regs = Vdpp2Registers::build(
            &cfg,
            &Vdpp2Job {
                src_fd: 7,
                src_uv_offset: 720 * 480,
                dst_fd: 8,
                dst_uv_offset: 1920 * 1080,
                hist_fd: 9,
            },
        )
        .unwrap();
        assert_eq!(regs.yrgb_hor_coe[0], 0x03e8_0008);
        assert_eq!(regs.yrgb_ver_coe[0], 0x0028_03ec);
        assert_eq!(regs.cbcr_hor_coe[0], 0x03f4_0004);
        assert_eq!(regs.cbcr_ver_coe[0], 0x0014_03f4);
        assert_eq!(
            regs.zme_common[..10],
            [
                0x8000_0640,
                0,
                0,
                0x01e0_02d0,
                0x0001_01ee,
                0x01df_02cf,
                0x0437_077f,
                0x0d10_0410,
                0x0000_5fea,
                0x0000_71a5,
            ]
        );
        assert_eq!(regs.common[1], 0x0200_0003);
        assert_eq!(regs.common[14], 0x01df_02cf);
        assert_eq!(regs.common[15], 0x0437_077f);
        assert_eq!(regs.common[20], 0x88ff_ffff);
    }

    fn pack_local_histogram(values: [u32; 4]) -> [u8; 9] {
        let mut packed = [0u8; 9];
        packed[0] = values[0] as u8;
        packed[1] = (values[0] >> 8) as u8;
        packed[2] = ((values[0] >> 16) & 0x03) as u8 | ((values[1] & 0x3f) << 2) as u8;
        packed[3] = (values[1] >> 6) as u8;
        packed[4] = ((values[1] >> 14) & 0x0f) as u8 | ((values[2] & 0x0f) << 4) as u8;
        packed[5] = (values[2] >> 4) as u8;
        packed[6] = ((values[2] >> 12) & 0x3f) as u8 | ((values[3] & 0x03) << 6) as u8;
        packed[7] = (values[3] >> 2) as u8;
        packed[8] = (values[3] >> 10) as u8;
        packed
    }

    #[test]
    fn packed_histogram_matches_the_vendor_layout() {
        let mut packed = [0u8; HIST_PACKED_BYTES];
        let first = [0x0000_0001, 0x0001_2345, 0x0002_abcd, 0x0003_ffff];
        let last = [0x0000_1111, 0x0001_2222, 0x0002_3333, 0x0003_4444];
        packed[..9].copy_from_slice(&pack_local_histogram(first));
        let last_offset = HIST_LOCAL_PACKED_BYTES - 9;
        packed[last_offset..HIST_LOCAL_PACKED_BYTES].copy_from_slice(&pack_local_histogram(last));

        let global_offset = HIST_LOCAL_PACKED_BYTES;
        packed[global_offset..global_offset + 4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        let global_last = global_offset + (HIST_GLOBAL_BINS - 1) * 4;
        packed[global_last..global_last + 4].copy_from_slice(&0x89ab_cdefu32.to_le_bytes());

        let histogram = Vdpp2Histogram::parse(&packed).unwrap();
        assert_eq!(&histogram.local[..4], &first);
        assert_eq!(&histogram.local[HIST_LOCAL_BINS - 4..], &last);
        assert_eq!(histogram.global[0], 0x1234_5678);
        assert_eq!(histogram.global[HIST_GLOBAL_BINS - 1], 0x89ab_cdef);
    }

    #[test]
    fn histogram_mean_luma_matches_the_c_u10_formula() {
        let mut packed = [0u8; HIST_PACKED_BYTES];
        let global_offset = HIST_LOCAL_PACKED_BYTES;
        packed[global_offset..global_offset + 4].copy_from_slice(&2u32.to_le_bytes());
        let global_last = global_offset + (HIST_GLOBAL_BINS - 1) * 4;
        packed[global_last..global_last + 4].copy_from_slice(&2u32.to_le_bytes());

        let histogram = Vdpp2Histogram::parse(&packed).unwrap();
        assert_eq!(histogram.mean_luma_u10(), Some(511));
    }

    #[test]
    fn packed_histogram_rejects_truncated_buffers() {
        let error = Vdpp2Histogram::parse(&[0u8; HIST_PACKED_BYTES - 1]).unwrap_err();
        assert!(error.contains("expected at least 10240"));
    }
}
