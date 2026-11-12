//! RK3576 VDPP2 (client 29) register generation.
//!
//! The hardware-independent part is shared between the board driver and the
//! register tests. It covers NV12 input scaling to NV12 or NV12-planar/YUV444
//! output plus the DCI histogram side path. Enhancement blocks are disabled in
//! this first port so their large parameter banks cannot silently alter the
//! scale result.

use super::vdpp2_coeff::{TAP6, TAP8};

pub const COMMON_WORDS: usize = 28;
pub const HIST_WORDS: usize = 5;
pub const DMSR_WORDS: usize = 21;
pub const ES_WORDS: usize = 32;
pub const SHARP_WORDS: usize = 172;
pub const ZME_COMMON_WORDS: usize = 18;
pub const ZME_COEFF_WORDS: usize = 68;

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
        if self.src_width_vir & 15 != 0
            || self.dst_width_vir & 15 != 0
            || self.src_height_vir & 7 != 0
            || self.dst_height_vir & 1 != 0
        {
            return Err("VDPP2 stride alignment is invalid".to_string());
        }
        if self.src_width & 1 != 0
            || self.src_height & 1 != 0
            || self.dst_width & 1 != 0
            || self.dst_height & 1 != 0
        {
            return Err("VDPP2 visible geometry must be even".to_string());
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
            | field(u32::from(config.histogram.enable), 25, 1);
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

        // Keep the fields intentionally if later code enables them. The first
        // scale/histogram slice leaves all three enhancement blocks disabled.
        let dmsr = [0u32; DMSR_WORDS];
        let es = [0u32; ES_WORDS];
        let sharp = [0u32; SHARP_WORDS];
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
        let cfg = Vdpp2Config::nv12_to_yuv444(720, 480, 1920, 1080);
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
}
