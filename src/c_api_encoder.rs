//! C ABI for the stable synchronous encoder SDK.

use std::ffi::c_int;
use std::mem::size_of;

use crate::encoder;
use crate::sdk;

pub const RK_MPP_OK: c_int = 0;
pub const RK_MPP_AGAIN: c_int = 1;
pub const RK_MPP_EOS: c_int = 2;
pub const RK_MPP_ERR_INVALID: c_int = -1;
pub const RK_MPP_ERR_UNSUPPORTED: c_int = -2;
pub const RK_MPP_ERR_PARSE: c_int = -3;
pub const RK_MPP_ERR_HARDWARE: c_int = -4;
pub const RK_MPP_ERR_RESOURCE: c_int = -5;
pub const RK_MPP_ERR_STATE: c_int = -6;
pub const RK_MPP_ERR_TOO_LARGE: c_int = -8;

pub const RK_MPP_ENCODED_PACKET_FLAG_KEYFRAME: u32 = 1 << 0;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RkMppEncoderConfig {
    pub struct_size: u32,
    pub codec: u32,
    pub input_format: u32,
    pub width: u32,
    pub height: u32,
    pub profile: u32,
    pub level: u32,
    pub gop: u32,
    pub qp: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub jpeg_quality: u32,
    pub jpeg_restart_mcus: u32,
    pub flags: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RkMppEncoderRateControl {
    pub struct_size: u32,
    pub mode: u32,
    pub bps_target: i64,
    pub bps_min: i64,
    pub bps_max: i64,
    pub initial_qp: u32,
    pub min_qp: u32,
    pub max_qp: u32,
    pub reserved: [u32; 5],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RkMppEncoderAdvancedConfig {
    pub struct_size: u32,
    pub flags: u32,
    pub rate_control: RkMppEncoderRateControl,
    pub reserved: [u32; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RkMppEncoderRoiRegion {
    pub struct_size: u32,
    pub flags: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub qp: i32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RkMppEncoderGdr {
    pub struct_size: u32,
    pub mode: u32,
    pub refresh_blocks: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RkMppEncoderSliceSplit {
    pub struct_size: u32,
    pub mode: u32,
    pub argument: u32,
    pub output_mode: u32,
    pub reserved: [u32; 2],
}

const MAX_ROI_REGIONS: usize = 8;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RkMppEncoderAdvancedConfigV2 {
    pub struct_size: u32,
    pub flags: u32,
    pub rate_control: RkMppEncoderRateControl,
    pub gdr: RkMppEncoderGdr,
    pub slice_split: RkMppEncoderSliceSplit,
    pub roi_count: u32,
    pub reserved0: u32,
    pub rois: [RkMppEncoderRoiRegion; MAX_ROI_REGIONS],
    pub reserved: [u32; 8],
}

#[repr(C)]
pub struct RkMppEncodedPacket {
    pub struct_size: u32,
    pub flags: u32,
    pub reserved0: u32,
    pub reserved1: u32,
    pub pts: i64,
    pub dts: i64,
    pub frame_num: i32,
    pub poc: i32,
    pub data: *const u8,
    pub size: u64,
    pub reserved: [u64; 2],
}

pub struct RkMppEncoder {
    inner: encoder::Encoder,
    last_packet: Option<encoder::Packet>,
}

#[derive(Default)]
struct AdvancedOptions {
    rate_control: Option<encoder::RateControlConfig>,
    roi: Option<encoder::RoiConfig>,
    gdr: Option<encoder::GdrConfig>,
    slice_split: Option<encoder::SliceSplitConfig>,
}

fn error_code(error: &sdk::Error) -> c_int {
    match error {
        sdk::Error::InvalidInput(_) => RK_MPP_ERR_INVALID,
        sdk::Error::Unsupported(_) => RK_MPP_ERR_UNSUPPORTED,
        sdk::Error::Parse(_) => RK_MPP_ERR_PARSE,
        sdk::Error::Hardware(_) => RK_MPP_ERR_HARDWARE,
        sdk::Error::Resource(_) => RK_MPP_ERR_RESOURCE,
        sdk::Error::State(_) => RK_MPP_ERR_STATE,
        sdk::Error::TooLarge => RK_MPP_ERR_TOO_LARGE,
    }
}

fn clear_packet(packet: &mut RkMppEncodedPacket) {
    packet.flags = 0;
    packet.reserved0 = 0;
    packet.reserved1 = 0;
    packet.pts = 0;
    packet.dts = 0;
    packet.frame_num = 0;
    packet.poc = 0;
    packet.data = std::ptr::null();
    packet.size = 0;
    packet.reserved = [0; 2];
}

fn init_rate_control(rate_control: &mut RkMppEncoderRateControl, mode: u32) -> Result<(), c_int> {
    match mode {
        0..=3 => {}
        _ => return Err(RK_MPP_ERR_INVALID),
    }
    *rate_control = RkMppEncoderRateControl {
        struct_size: size_of::<RkMppEncoderRateControl>() as u32,
        mode,
        bps_target: 0,
        bps_min: 0,
        bps_max: 0,
        initial_qp: 26,
        min_qp: 8,
        max_qp: 48,
        reserved: [0; 5],
    };
    Ok(())
}

fn parse_rate_control(
    rate_control: &RkMppEncoderRateControl,
) -> Result<encoder::RateControlConfig, c_int> {
    if rate_control.struct_size < size_of::<RkMppEncoderRateControl>() as u32 {
        return Err(RK_MPP_ERR_INVALID);
    }
    if rate_control.reserved != [0; 5] {
        return Err(RK_MPP_ERR_INVALID);
    }
    let mode =
        encoder::RateControlMode::from_c_value(rate_control.mode).ok_or(RK_MPP_ERR_INVALID)?;
    let initial_qp = u8::try_from(rate_control.initial_qp).map_err(|_| RK_MPP_ERR_INVALID)?;
    let min_qp = u8::try_from(rate_control.min_qp).map_err(|_| RK_MPP_ERR_INVALID)?;
    let max_qp = u8::try_from(rate_control.max_qp).map_err(|_| RK_MPP_ERR_INVALID)?;
    let parsed = encoder::RateControlConfig::new(
        mode,
        rate_control.bps_target,
        rate_control.bps_min,
        rate_control.bps_max,
    )
    .with_initial_qp(initial_qp)
    .with_qp_range(min_qp, max_qp);
    parsed.validate().map_err(|error| error_code(&error))?;
    Ok(parsed)
}

fn parse_roi_config(
    roi_count: u32,
    rois: &[RkMppEncoderRoiRegion; MAX_ROI_REGIONS],
) -> Result<Option<encoder::RoiConfig>, c_int> {
    let count = usize::try_from(roi_count).map_err(|_| RK_MPP_ERR_INVALID)?;
    if count > MAX_ROI_REGIONS {
        return Err(RK_MPP_ERR_INVALID);
    }
    if count == 0 {
        return Ok(None);
    }
    let mut parsed = Vec::with_capacity(count);
    for region in &rois[..count] {
        if region.struct_size < size_of::<RkMppEncoderRoiRegion>() as u32
            || region.flags & !0x3 != 0
            || region.reserved != [0; 3]
        {
            return Err(RK_MPP_ERR_INVALID);
        }
        parsed.push(
            encoder::RoiRegion::new(region.x, region.y, region.width, region.height, region.qp)
                .with_force_intra(region.flags & 0x1 != 0)
                .with_absolute_qp(region.flags & 0x2 != 0),
        );
    }
    encoder::RoiConfig::new(&parsed)
        .map(Some)
        .map_err(|error| error_code(&error))
}

fn parse_gdr_config(gdr: &RkMppEncoderGdr) -> Result<Option<encoder::GdrConfig>, c_int> {
    if gdr.struct_size < size_of::<RkMppEncoderGdr>() as u32 || gdr.reserved != 0 {
        return Err(RK_MPP_ERR_INVALID);
    }
    match gdr.mode {
        0 if gdr.refresh_blocks == 0 => Ok(None),
        0 => Err(RK_MPP_ERR_INVALID),
        1 | 2 if gdr.refresh_blocks != 0 => Ok(Some(encoder::GdrConfig::new(
            if gdr.mode == 1 {
                encoder::GdrMode::Row
            } else {
                encoder::GdrMode::Col
            },
            gdr.refresh_blocks,
        ))),
        _ => Err(RK_MPP_ERR_INVALID),
    }
}

fn parse_slice_config(
    split: &RkMppEncoderSliceSplit,
) -> Result<Option<encoder::SliceSplitConfig>, c_int> {
    if split.struct_size < size_of::<RkMppEncoderSliceSplit>() as u32 || split.reserved != [0; 2] {
        return Err(RK_MPP_ERR_INVALID);
    }
    if split.mode == 0 {
        return if split.argument == 0 && split.output_mode == 0 {
            Ok(None)
        } else {
            Err(RK_MPP_ERR_INVALID)
        };
    }
    let mode = match split.mode {
        1 => encoder::SliceSplitMode::ByCtu,
        2 => encoder::SliceSplitMode::ByByte,
        _ => return Err(RK_MPP_ERR_INVALID),
    };
    let output = match split.output_mode {
        0 => encoder::SliceOutputMode::CompleteAccessUnit,
        1 => encoder::SliceOutputMode::LowDelayPackets,
        _ => return Err(RK_MPP_ERR_INVALID),
    };
    Ok(Some(
        encoder::SliceSplitConfig::new(mode, split.argument).with_output(output),
    ))
}

fn parse_advanced_v2(advanced: &RkMppEncoderAdvancedConfigV2) -> Result<AdvancedOptions, c_int> {
    if advanced.struct_size < size_of::<RkMppEncoderAdvancedConfigV2>() as u32
        || advanced.flags != 0
        || advanced.reserved0 != 0
        || advanced.reserved != [0; 8]
    {
        return Err(RK_MPP_ERR_INVALID);
    }
    Ok(AdvancedOptions {
        rate_control: Some(parse_rate_control(&advanced.rate_control)?),
        roi: parse_roi_config(advanced.roi_count, &advanced.rois)?,
        gdr: parse_gdr_config(&advanced.gdr)?,
        slice_split: parse_slice_config(&advanced.slice_split)?,
    })
}

/// # Safety
/// `config` must point to writable storage for one encoder config.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_config_init(
    config: *mut RkMppEncoderConfig,
    codec: u32,
    width: u32,
    height: u32,
    input_format: u32,
) -> c_int {
    if config.is_null()
        || sdk::Codec::from_c_value(codec).is_none()
        || sdk::PixelFormat::from_c_value(input_format).is_none()
        || width == 0
        || height == 0
    {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable config storage.
    unsafe {
        config.write(RkMppEncoderConfig {
            struct_size: size_of::<RkMppEncoderConfig>() as u32,
            codec,
            input_format,
            width,
            height,
            profile: 0,
            level: 30,
            gop: 1,
            qp: 26,
            fps_num: 30,
            fps_den: 1,
            jpeg_quality: 10,
            jpeg_restart_mcus: 0,
            flags: 0,
            reserved: [0; 3],
        });
    }
    RK_MPP_OK
}

/// Initialize a rate-control descriptor.
///
/// # Safety
/// `rate_control` must point to writable storage for one descriptor.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_rate_control_init(
    rate_control: *mut RkMppEncoderRateControl,
    mode: u32,
) -> c_int {
    if rate_control.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable descriptor storage.
    let rate_control = unsafe { &mut *rate_control };
    match init_rate_control(rate_control, mode) {
        Ok(()) => RK_MPP_OK,
        Err(code) => code,
    }
}

/// Initialize an advanced encoder configuration.
///
/// # Safety
/// `config` must point to writable storage for one advanced configuration.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_advanced_config_init(
    config: *mut RkMppEncoderAdvancedConfig,
    rate_control_mode: u32,
) -> c_int {
    if config.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    let mut rate_control = RkMppEncoderRateControl {
        struct_size: 0,
        mode: 0,
        bps_target: 0,
        bps_min: 0,
        bps_max: 0,
        initial_qp: 0,
        min_qp: 0,
        max_qp: 0,
        reserved: [0; 5],
    };
    if let Err(code) = init_rate_control(&mut rate_control, rate_control_mode) {
        return code;
    }
    // SAFETY: the caller guarantees writable advanced-config storage.
    unsafe {
        config.write(RkMppEncoderAdvancedConfig {
            struct_size: size_of::<RkMppEncoderAdvancedConfig>() as u32,
            flags: 0,
            rate_control,
            reserved: [0; 8],
        });
    }
    RK_MPP_OK
}

/// Initialize one ROI descriptor.
///
/// # Safety
/// `region` must point to writable storage for one ROI descriptor.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_roi_region_init(
    region: *mut RkMppEncoderRoiRegion,
) -> c_int {
    if region.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable ROI storage.
    unsafe {
        region.write(RkMppEncoderRoiRegion {
            struct_size: size_of::<RkMppEncoderRoiRegion>() as u32,
            flags: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            qp: 0,
            reserved: [0; 3],
        });
    }
    RK_MPP_OK
}

/// Initialize a GDR descriptor.
///
/// # Safety
/// `gdr` must point to writable storage for one descriptor.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_gdr_init(
    gdr: *mut RkMppEncoderGdr,
    mode: u32,
    refresh_blocks: u32,
) -> c_int {
    if gdr.is_null()
        || mode > 2
        || (mode == 0 && refresh_blocks != 0)
        || (mode != 0 && refresh_blocks == 0)
    {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable GDR storage.
    unsafe {
        gdr.write(RkMppEncoderGdr {
            struct_size: size_of::<RkMppEncoderGdr>() as u32,
            mode,
            refresh_blocks,
            reserved: 0,
        });
    }
    RK_MPP_OK
}

/// Initialize a slice-split descriptor.
///
/// # Safety
/// `split` must point to writable storage for one descriptor.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_slice_split_init(
    split: *mut RkMppEncoderSliceSplit,
    mode: u32,
    argument: u32,
    output_mode: u32,
) -> c_int {
    if split.is_null()
        || mode > 2
        || output_mode > 1
        || (mode == 0 && (argument != 0 || output_mode != 0))
        || (mode != 0 && argument == 0)
    {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable slice-split storage.
    unsafe {
        split.write(RkMppEncoderSliceSplit {
            struct_size: size_of::<RkMppEncoderSliceSplit>() as u32,
            mode,
            argument,
            output_mode,
            reserved: [0; 2],
        });
    }
    RK_MPP_OK
}

/// Initialize the V1.10 advanced encoder configuration.
///
/// # Safety
/// `config` must point to writable storage for one V2 configuration.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_advanced_config_v2_init(
    config: *mut RkMppEncoderAdvancedConfigV2,
    rate_control_mode: u32,
) -> c_int {
    if config.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    let mut rate_control = RkMppEncoderRateControl {
        struct_size: 0,
        mode: 0,
        bps_target: 0,
        bps_min: 0,
        bps_max: 0,
        initial_qp: 0,
        min_qp: 0,
        max_qp: 0,
        reserved: [0; 5],
    };
    if let Err(code) = init_rate_control(&mut rate_control, rate_control_mode) {
        return code;
    }
    let roi = RkMppEncoderRoiRegion {
        struct_size: size_of::<RkMppEncoderRoiRegion>() as u32,
        flags: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        qp: 0,
        reserved: [0; 3],
    };
    // SAFETY: the caller guarantees writable V2 configuration storage.
    unsafe {
        config.write(RkMppEncoderAdvancedConfigV2 {
            struct_size: size_of::<RkMppEncoderAdvancedConfigV2>() as u32,
            flags: 0,
            rate_control,
            gdr: RkMppEncoderGdr {
                struct_size: size_of::<RkMppEncoderGdr>() as u32,
                mode: 0,
                refresh_blocks: 0,
                reserved: 0,
            },
            slice_split: RkMppEncoderSliceSplit {
                struct_size: size_of::<RkMppEncoderSliceSplit>() as u32,
                mode: 0,
                argument: 0,
                output_mode: 0,
                reserved: [0; 2],
            },
            roi_count: 0,
            reserved0: 0,
            rois: [roi; MAX_ROI_REGIONS],
            reserved: [0; 8],
        });
    }
    RK_MPP_OK
}

/// # Safety
/// `packet` must point to writable storage for one packet descriptor.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoded_packet_init(packet: *mut RkMppEncodedPacket) -> c_int {
    if packet.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable packet storage.
    unsafe {
        packet.write(RkMppEncodedPacket {
            struct_size: size_of::<RkMppEncodedPacket>() as u32,
            flags: 0,
            reserved0: 0,
            reserved1: 0,
            pts: 0,
            dts: 0,
            frame_num: 0,
            poc: 0,
            data: std::ptr::null(),
            size: 0,
            reserved: [0; 2],
        });
    }
    RK_MPP_OK
}

unsafe fn create_encoder(
    config: *const RkMppEncoderConfig,
    advanced: &AdvancedOptions,
    out_encoder: *mut *mut RkMppEncoder,
) -> c_int {
    // SAFETY: the caller validated all pointers before entering this helper.
    unsafe {
        *out_encoder = std::ptr::null_mut();
        let config = &*config;
        if config.struct_size < size_of::<RkMppEncoderConfig>() as u32 {
            return RK_MPP_ERR_INVALID;
        }
        if config.flags != 0 || config.reserved != [0; 3] {
            return RK_MPP_ERR_INVALID;
        }
        let Some(codec) = sdk::Codec::from_c_value(config.codec) else {
            return RK_MPP_ERR_INVALID;
        };
        let Some(input_format) = sdk::PixelFormat::from_c_value(config.input_format) else {
            return RK_MPP_ERR_INVALID;
        };
        let mut encoder_config =
            encoder::EncoderConfig::new(codec, config.width as usize, config.height as usize)
                .with_input_format(input_format)
                .with_profile(u8::try_from(config.profile).unwrap_or(u8::MAX))
                .with_level(u8::try_from(config.level).unwrap_or(u8::MAX))
                .with_gop(config.gop)
                .with_qp(u8::try_from(config.qp).unwrap_or(u8::MAX))
                .with_fps(config.fps_num, config.fps_den)
                .with_jpeg_quality(config.jpeg_quality)
                .with_jpeg_restart_mcus(config.jpeg_restart_mcus);
        if let Some(rate_control) = advanced.rate_control {
            encoder_config = encoder_config.with_rate_control(rate_control);
        }
        if let Some(roi) = advanced.roi {
            encoder_config = encoder_config.with_roi(roi);
        }
        if let Some(gdr) = advanced.gdr {
            encoder_config = encoder_config.with_gdr(gdr);
        }
        if let Some(slice_split) = advanced.slice_split {
            encoder_config = encoder_config.with_slice_split(slice_split);
        }
        match encoder::Encoder::new(encoder_config) {
            Ok(inner) => {
                *out_encoder = Box::into_raw(Box::new(RkMppEncoder {
                    inner,
                    last_packet: None,
                }));
                RK_MPP_OK
            }
            Err(error) => error_code(&error),
        }
    }
}

/// # Safety
/// `config` must point to a valid config and `out_encoder` to writable pointer
/// storage.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_create(
    config: *const RkMppEncoderConfig,
    out_encoder: *mut *mut RkMppEncoder,
) -> c_int {
    if config.is_null() || out_encoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: both pointers were validated and the caller owns their storage.
    unsafe { create_encoder(config, &AdvancedOptions::default(), out_encoder) }
}

/// Create an encoder using a separately versioned advanced configuration.
///
/// # Safety
/// `config` and `advanced_config` must point to valid descriptors and
/// `out_encoder` to writable pointer storage.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_create_advanced(
    config: *const RkMppEncoderConfig,
    advanced_config: *const RkMppEncoderAdvancedConfig,
    out_encoder: *mut *mut RkMppEncoder,
) -> c_int {
    if config.is_null() || advanced_config.is_null() || out_encoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: all pointers were validated and the caller owns their storage.
    unsafe {
        let advanced = &*advanced_config;
        if advanced.struct_size < size_of::<RkMppEncoderAdvancedConfig>() as u32
            || advanced.flags != 0
            || advanced.reserved != [0; 8]
        {
            return RK_MPP_ERR_INVALID;
        }
        let rate_control = match parse_rate_control(&advanced.rate_control) {
            Ok(rate_control) => rate_control,
            Err(code) => return code,
        };
        create_encoder(
            config,
            &AdvancedOptions {
                rate_control: Some(rate_control),
                ..AdvancedOptions::default()
            },
            out_encoder,
        )
    }
}

/// Create an encoder using the V1.10 advanced configuration.
///
/// # Safety
/// `config` and `advanced_config` must point to valid descriptors and
/// `out_encoder` to writable pointer storage.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_create_advanced_v2(
    config: *const RkMppEncoderConfig,
    advanced_config: *const RkMppEncoderAdvancedConfigV2,
    out_encoder: *mut *mut RkMppEncoder,
) -> c_int {
    if config.is_null() || advanced_config.is_null() || out_encoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: all pointers were validated and the caller owns their storage.
    unsafe {
        let advanced = match parse_advanced_v2(&*advanced_config) {
            Ok(advanced) => advanced,
            Err(code) => return code,
        };
        create_encoder(config, &advanced, out_encoder)
    }
}

/// Submit one tightly packed visible-size NV12/I420 frame.
///
/// # Safety
/// `encoder` must be live. `data` must address `size` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_submit_frame(
    encoder: *mut RkMppEncoder,
    data: *const u8,
    size: u64,
    pts: i64,
    dts: i64,
) -> c_int {
    if encoder.is_null() || data.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    let Ok(size) = usize::try_from(size) else {
        return RK_MPP_ERR_TOO_LARGE;
    };
    // SAFETY: the caller guarantees `size` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data, size) };
    // SAFETY: encoder is live and exclusively used for this call.
    match unsafe { &mut *encoder }
        .inner
        .submit_frame(encoder::InputFrame::new(bytes).with_pts(pts).with_dts(dts))
    {
        Ok(_) => RK_MPP_OK,
        Err(error) => error_code(&error),
    }
}

/// Submit one NV12 frame from an externally allocated dma-buf.
///
/// The fd points at the luma plane (offset zero). `size` is the full imported
/// allocation length; `hor_stride`/`ver_stride` describe the coded NV12 layout.
///
/// # Safety
/// `encoder` must be live and `fd` must reference a readable dma-buf.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_submit_frame_dmabuf(
    encoder: *mut RkMppEncoder,
    fd: c_int,
    size: u64,
    hor_stride: u32,
    ver_stride: u32,
    pts: i64,
    dts: i64,
) -> c_int {
    if encoder.is_null() || fd < 0 {
        return RK_MPP_ERR_INVALID;
    }
    let Ok(size) = usize::try_from(size) else {
        return RK_MPP_ERR_TOO_LARGE;
    };
    let Ok(hor_stride) = usize::try_from(hor_stride) else {
        return RK_MPP_ERR_INVALID;
    };
    let Ok(ver_stride) = usize::try_from(ver_stride) else {
        return RK_MPP_ERR_INVALID;
    };
    // SAFETY: encoder is live, the synchronous call keeps the caller's fd valid,
    // and the C API contract requires that fd to describe `size` readable bytes.
    match unsafe {
        (&mut *encoder)
            .inner
            .submit_dmabuf_frame_borrowed(fd, size, hor_stride, ver_stride, pts, dts, None)
    } {
        Ok(_) => RK_MPP_OK,
        Err(error) => error_code(&error),
    }
}

/// # Safety
/// `encoder` must be live and `packet` writable initialized storage.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_receive_packet(
    encoder: *mut RkMppEncoder,
    packet: *mut RkMppEncodedPacket,
) -> c_int {
    if encoder.is_null() || packet.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: both pointers are validated.
    let encoder = unsafe { &mut *encoder };
    let packet = unsafe { &mut *packet };
    if packet.struct_size != size_of::<RkMppEncodedPacket>() as u32 {
        return RK_MPP_ERR_INVALID;
    }
    if encoder.last_packet.is_some() {
        return RK_MPP_ERR_STATE;
    }
    clear_packet(packet);
    let Some(encoded) = encoder.inner.receive_packet() else {
        return if encoder.inner.is_finished() {
            RK_MPP_EOS
        } else {
            RK_MPP_AGAIN
        };
    };
    packet.flags = if encoded.is_keyframe() {
        RK_MPP_ENCODED_PACKET_FLAG_KEYFRAME
    } else {
        0
    };
    packet.pts = encoded.pts();
    packet.dts = encoded.dts();
    packet.frame_num = encoded.frame_num() as i32;
    packet.poc = encoded.poc();
    packet.data = encoded.data().as_ptr();
    packet.size = encoded.data().len() as u64;
    encoder.last_packet = Some(encoded);
    RK_MPP_OK
}

/// # Safety
/// `encoder` must be live; `packet` may be null.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoded_packet_release(
    encoder: *mut RkMppEncoder,
    packet: *mut RkMppEncodedPacket,
) {
    if encoder.is_null() {
        return;
    }
    // SAFETY: encoder is live under the call contract.
    let encoder = unsafe { &mut *encoder };
    encoder.last_packet = None;
    if !packet.is_null() {
        // SAFETY: caller guarantees writable packet storage.
        clear_packet(unsafe { &mut *packet });
    }
}

/// # Safety
/// `encoder` must be live.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_flush(encoder: *mut RkMppEncoder) -> c_int {
    if encoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: encoder is live.
    unsafe { &mut *encoder }.inner.flush();
    RK_MPP_OK
}

/// # Safety
/// `encoder` must be live.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_reset(encoder: *mut RkMppEncoder) -> c_int {
    if encoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: encoder is live.
    let encoder = unsafe { &mut *encoder };
    encoder.last_packet = None;
    match encoder.inner.reset() {
        Ok(()) => RK_MPP_OK,
        Err(error) => error_code(&error),
    }
}

/// # Safety
/// `encoder` must come from create and not be used after this call.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_close(encoder: *mut RkMppEncoder) {
    if !encoder.is_null() {
        // SAFETY: caller transfers ownership.
        drop(unsafe { Box::from_raw(encoder) });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RkMppEncodedPacket, RkMppEncoderAdvancedConfig, RkMppEncoderAdvancedConfigV2,
        RkMppEncoderConfig, RkMppEncoderGdr, RkMppEncoderRateControl, RkMppEncoderRoiRegion,
        RkMppEncoderSliceSplit,
    };
    use std::mem::size_of;

    #[test]
    fn encoder_structs_match_the_v1_header_layout() {
        assert_eq!(size_of::<RkMppEncoderConfig>(), 68);
        assert_eq!(size_of::<RkMppEncoderRateControl>(), 64);
        assert_eq!(size_of::<RkMppEncoderAdvancedConfig>(), 104);
        assert_eq!(size_of::<RkMppEncoderRoiRegion>(), 40);
        assert_eq!(size_of::<RkMppEncoderGdr>(), 16);
        assert_eq!(size_of::<RkMppEncoderSliceSplit>(), 24);
        assert_eq!(size_of::<RkMppEncoderAdvancedConfigV2>(), 472);
        assert_eq!(size_of::<RkMppEncodedPacket>(), 72);
    }
}
