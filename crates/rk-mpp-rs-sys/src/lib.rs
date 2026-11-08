//! Raw FFI declarations for the `rk-mpp-rs` V1.10 C ABI.
//!
//! Every function in this crate is unsafe: callers must follow the ownership,
//! struct-size, threading and lifetime rules documented in
//! `include/rk_mpp_rs.h`.

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use core::ffi::{c_char, c_int};

pub const RK_MPP_API_VERSION_MAJOR: u32 = 1;
pub const RK_MPP_API_VERSION_MINOR: u32 = 10;
pub const RK_MPP_API_VERSION: u32 = (RK_MPP_API_VERSION_MAJOR << 16) | RK_MPP_API_VERSION_MINOR;

pub const RK_MPP_OK: c_int = 0;
pub const RK_MPP_AGAIN: c_int = 1;
pub const RK_MPP_EOS: c_int = 2;
pub const RK_MPP_ERR_INVALID: c_int = -1;
pub const RK_MPP_ERR_UNSUPPORTED: c_int = -2;
pub const RK_MPP_ERR_PARSE: c_int = -3;
pub const RK_MPP_ERR_HARDWARE: c_int = -4;
pub const RK_MPP_ERR_RESOURCE: c_int = -5;
pub const RK_MPP_ERR_STATE: c_int = -6;
pub const RK_MPP_ERR_IO: c_int = -7;
pub const RK_MPP_ERR_TOO_LARGE: c_int = -8;

pub const RK_MPP_CODEC_H264: u32 = 1;
pub const RK_MPP_CODEC_MJPEG: u32 = 2;
pub const RK_MPP_CODEC_H265: u32 = 3;
pub const RK_MPP_CODEC_VP9: u32 = 4;
pub const RK_MPP_CODEC_AV1: u32 = 5;
pub const RK_MPP_CODEC_VP8: u32 = 6;
pub const RK_MPP_CODEC_MPEG2: u32 = 7;
pub const RK_MPP_CODEC_MPEG4: u32 = 8;
pub const RK_MPP_CODEC_H263: u32 = 9;

pub const RK_MPP_PIXEL_FORMAT_NV12: u32 = 1;
pub const RK_MPP_PIXEL_FORMAT_I420: u32 = 2;
pub const RK_MPP_PIXEL_FORMAT_I420P10: u32 = 3;

pub const RK_MPP_PACKET_FLAG_EOS: u32 = 1 << 0;
pub const RK_MPP_PACKET_FLAG_COMPLETE_ACCESS_UNIT: u32 = 1 << 1;

pub const RK_MPP_RATE_CONTROL_FIXED_QP: u32 = 0;
pub const RK_MPP_RATE_CONTROL_CBR: u32 = 1;
pub const RK_MPP_RATE_CONTROL_VBR: u32 = 2;
pub const RK_MPP_RATE_CONTROL_AVBR: u32 = 3;

pub const RK_MPP_MAX_ROI_REGIONS: usize = 8;
pub const RK_MPP_ROI_FLAG_FORCE_INTRA: u32 = 1 << 0;
pub const RK_MPP_ROI_FLAG_ABSOLUTE_QP: u32 = 1 << 1;

pub const RK_MPP_GDR_DISABLED: u32 = 0;
pub const RK_MPP_GDR_ROW: u32 = 1;
pub const RK_MPP_GDR_COL: u32 = 2;

pub const RK_MPP_SLICE_SPLIT_NONE: u32 = 0;
pub const RK_MPP_SLICE_SPLIT_BY_CTU: u32 = 1;
pub const RK_MPP_SLICE_SPLIT_BY_BYTE: u32 = 2;

pub const RK_MPP_SLICE_OUTPUT_COMPLETE_ACCESS_UNIT: u32 = 0;
pub const RK_MPP_SLICE_OUTPUT_LOW_DELAY_PACKETS: u32 = 1;

pub const RK_MPP_ENCODED_PACKET_FLAG_KEYFRAME: u32 = 1 << 0;

#[repr(C)]
pub struct rk_mpp_decoder {
    _private: [u8; 0],
}
pub type rk_mpp_decoder_t = rk_mpp_decoder;

#[repr(C)]
pub struct rk_mpp_encoder {
    _private: [u8; 0],
}
pub type rk_mpp_encoder_t = rk_mpp_encoder;

#[repr(C)]
pub struct RkDec {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct rk_mpp_decoder_config {
    pub struct_size: u32,
    pub codec: u32,
    pub output_format: u32,
    pub flags: u32,
    pub max_reorder_frames: u32,
    pub reserved: [u32; 3],
}
pub type rk_mpp_decoder_config_t = rk_mpp_decoder_config;

impl Default for rk_mpp_decoder_config {
    fn default() -> Self {
        Self {
            struct_size: 0,
            codec: 0,
            output_format: 0,
            flags: 0,
            max_reorder_frames: 0,
            reserved: [0; 3],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct rk_mpp_frame {
    pub struct_size: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub flags: u32,
    pub reserved0: u32,
    pub pts: i64,
    pub dts: i64,
    pub poc: i32,
    pub err_info: u32,
    pub plane_data: [*const u8; 3],
    pub plane_sizes: [u64; 3],
    pub plane_strides: [u32; 3],
}
pub type rk_mpp_frame_t = rk_mpp_frame;

impl Default for rk_mpp_frame {
    fn default() -> Self {
        Self {
            struct_size: 0,
            width: 0,
            height: 0,
            format: 0,
            flags: 0,
            reserved0: 0,
            pts: 0,
            dts: 0,
            poc: 0,
            err_info: 0,
            plane_data: [core::ptr::null(); 3],
            plane_sizes: [0; 3],
            plane_strides: [0; 3],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct rk_mpp_encoder_config {
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
pub type rk_mpp_encoder_config_t = rk_mpp_encoder_config;

impl Default for rk_mpp_encoder_config {
    fn default() -> Self {
        Self {
            struct_size: 0,
            codec: 0,
            input_format: 0,
            width: 0,
            height: 0,
            profile: 0,
            level: 0,
            gop: 0,
            qp: 0,
            fps_num: 0,
            fps_den: 0,
            jpeg_quality: 0,
            jpeg_restart_mcus: 0,
            flags: 0,
            reserved: [0; 3],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct rk_mpp_encoder_rate_control {
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
pub type rk_mpp_encoder_rate_control_t = rk_mpp_encoder_rate_control;

impl Default for rk_mpp_encoder_rate_control {
    fn default() -> Self {
        Self {
            struct_size: 0,
            mode: 0,
            bps_target: 0,
            bps_min: 0,
            bps_max: 0,
            initial_qp: 0,
            min_qp: 0,
            max_qp: 0,
            reserved: [0; 5],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct rk_mpp_encoder_advanced_config {
    pub struct_size: u32,
    pub flags: u32,
    pub rate_control: rk_mpp_encoder_rate_control,
    pub reserved: [u32; 8],
}
pub type rk_mpp_encoder_advanced_config_t = rk_mpp_encoder_advanced_config;

impl Default for rk_mpp_encoder_advanced_config {
    fn default() -> Self {
        Self {
            struct_size: 0,
            flags: 0,
            rate_control: rk_mpp_encoder_rate_control::default(),
            reserved: [0; 8],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct rk_mpp_encoder_roi_region {
    pub struct_size: u32,
    pub flags: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub qp: i32,
    pub reserved: [u32; 3],
}
pub type rk_mpp_encoder_roi_region_t = rk_mpp_encoder_roi_region;

impl Default for rk_mpp_encoder_roi_region {
    fn default() -> Self {
        Self {
            struct_size: 0,
            flags: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            qp: 0,
            reserved: [0; 3],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct rk_mpp_encoder_gdr {
    pub struct_size: u32,
    pub mode: u32,
    pub refresh_blocks: u32,
    pub reserved: u32,
}
pub type rk_mpp_encoder_gdr_t = rk_mpp_encoder_gdr;

impl Default for rk_mpp_encoder_gdr {
    fn default() -> Self {
        Self {
            struct_size: 0,
            mode: 0,
            refresh_blocks: 0,
            reserved: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct rk_mpp_encoder_slice_split {
    pub struct_size: u32,
    pub mode: u32,
    pub argument: u32,
    pub output_mode: u32,
    pub reserved: [u32; 2],
}
pub type rk_mpp_encoder_slice_split_t = rk_mpp_encoder_slice_split;

impl Default for rk_mpp_encoder_slice_split {
    fn default() -> Self {
        Self {
            struct_size: 0,
            mode: 0,
            argument: 0,
            output_mode: 0,
            reserved: [0; 2],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct rk_mpp_encoder_advanced_config_v2 {
    pub struct_size: u32,
    pub flags: u32,
    pub rate_control: rk_mpp_encoder_rate_control,
    pub gdr: rk_mpp_encoder_gdr,
    pub slice_split: rk_mpp_encoder_slice_split,
    pub roi_count: u32,
    pub reserved0: u32,
    pub rois: [rk_mpp_encoder_roi_region; RK_MPP_MAX_ROI_REGIONS],
    pub reserved: [u32; 8],
}
pub type rk_mpp_encoder_advanced_config_v2_t = rk_mpp_encoder_advanced_config_v2;

impl Default for rk_mpp_encoder_advanced_config_v2 {
    fn default() -> Self {
        Self {
            struct_size: 0,
            flags: 0,
            rate_control: rk_mpp_encoder_rate_control::default(),
            gdr: rk_mpp_encoder_gdr::default(),
            slice_split: rk_mpp_encoder_slice_split::default(),
            roi_count: 0,
            reserved0: 0,
            rois: [rk_mpp_encoder_roi_region::default(); RK_MPP_MAX_ROI_REGIONS],
            reserved: [0; 8],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct rk_mpp_encoded_packet {
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
pub type rk_mpp_encoded_packet_t = rk_mpp_encoded_packet;

impl Default for rk_mpp_encoded_packet {
    fn default() -> Self {
        Self {
            struct_size: 0,
            flags: 0,
            reserved0: 0,
            reserved1: 0,
            pts: 0,
            dts: 0,
            frame_num: 0,
            poc: 0,
            data: core::ptr::null(),
            size: 0,
            reserved: [0; 2],
        }
    }
}

unsafe extern "C" {
    pub fn rk_mpp_api_version() -> u32;
    pub fn rk_mpp_api_version_string() -> *const c_char;
    pub fn rk_mpp_strerror(code: c_int) -> *const c_char;

    pub fn rk_mpp_decoder_config_init(
        config: *mut rk_mpp_decoder_config_t,
        codec: u32,
        output_format: u32,
    ) -> c_int;
    pub fn rk_mpp_frame_init(frame: *mut rk_mpp_frame_t) -> c_int;
    pub fn rk_mpp_decoder_create(
        config: *const rk_mpp_decoder_config_t,
        out_decoder: *mut *mut rk_mpp_decoder_t,
    ) -> c_int;
    pub fn rk_mpp_decoder_submit_packet(
        decoder: *mut rk_mpp_decoder_t,
        data: *const u8,
        size: u64,
        pts: i64,
        dts: i64,
        flags: u32,
    ) -> c_int;
    pub fn rk_mpp_decoder_receive_frame(
        decoder: *mut rk_mpp_decoder_t,
        frame: *mut rk_mpp_frame_t,
    ) -> c_int;
    pub fn rk_mpp_frame_release(decoder: *mut rk_mpp_decoder_t, frame: *mut rk_mpp_frame_t);
    pub fn rk_mpp_decoder_flush(decoder: *mut rk_mpp_decoder_t) -> c_int;
    pub fn rk_mpp_decoder_reset(decoder: *mut rk_mpp_decoder_t) -> c_int;
    pub fn rk_mpp_decoder_recover(decoder: *mut rk_mpp_decoder_t) -> c_int;
    pub fn rk_mpp_decoder_status(
        decoder: *mut rk_mpp_decoder_t,
        status_a: *mut u32,
        status_b: *mut u32,
        status_c: *mut u32,
    ) -> c_int;
    pub fn rk_mpp_decoder_close(decoder: *mut rk_mpp_decoder_t);

    pub fn rk_mpp_encoder_config_init(
        config: *mut rk_mpp_encoder_config_t,
        codec: u32,
        width: u32,
        height: u32,
        input_format: u32,
    ) -> c_int;
    pub fn rk_mpp_encoder_rate_control_init(
        rate_control: *mut rk_mpp_encoder_rate_control_t,
        mode: u32,
    ) -> c_int;
    pub fn rk_mpp_encoder_advanced_config_init(
        config: *mut rk_mpp_encoder_advanced_config_t,
        rate_control_mode: u32,
    ) -> c_int;
    pub fn rk_mpp_encoder_roi_region_init(region: *mut rk_mpp_encoder_roi_region_t) -> c_int;
    pub fn rk_mpp_encoder_gdr_init(
        gdr: *mut rk_mpp_encoder_gdr_t,
        mode: u32,
        refresh_blocks: u32,
    ) -> c_int;
    pub fn rk_mpp_encoder_slice_split_init(
        split: *mut rk_mpp_encoder_slice_split_t,
        mode: u32,
        argument: u32,
        output_mode: u32,
    ) -> c_int;
    pub fn rk_mpp_encoder_advanced_config_v2_init(
        config: *mut rk_mpp_encoder_advanced_config_v2_t,
        rate_control_mode: u32,
    ) -> c_int;
    pub fn rk_mpp_encoded_packet_init(packet: *mut rk_mpp_encoded_packet_t) -> c_int;
    pub fn rk_mpp_encoder_create(
        config: *const rk_mpp_encoder_config_t,
        out_encoder: *mut *mut rk_mpp_encoder_t,
    ) -> c_int;
    pub fn rk_mpp_encoder_create_advanced(
        config: *const rk_mpp_encoder_config_t,
        advanced_config: *const rk_mpp_encoder_advanced_config_t,
        out_encoder: *mut *mut rk_mpp_encoder_t,
    ) -> c_int;
    pub fn rk_mpp_encoder_create_advanced_v2(
        config: *const rk_mpp_encoder_config_t,
        advanced_config: *const rk_mpp_encoder_advanced_config_v2_t,
        out_encoder: *mut *mut rk_mpp_encoder_t,
    ) -> c_int;
    pub fn rk_mpp_encoder_submit_frame(
        encoder: *mut rk_mpp_encoder_t,
        data: *const u8,
        size: u64,
        pts: i64,
        dts: i64,
    ) -> c_int;
    pub fn rk_mpp_encoder_receive_packet(
        encoder: *mut rk_mpp_encoder_t,
        packet: *mut rk_mpp_encoded_packet_t,
    ) -> c_int;
    pub fn rk_mpp_encoded_packet_release(
        encoder: *mut rk_mpp_encoder_t,
        packet: *mut rk_mpp_encoded_packet_t,
    );
    pub fn rk_mpp_encoder_flush(encoder: *mut rk_mpp_encoder_t) -> c_int;
    pub fn rk_mpp_encoder_reset(encoder: *mut rk_mpp_encoder_t) -> c_int;
    pub fn rk_mpp_encoder_close(encoder: *mut rk_mpp_encoder_t);

    pub fn rk_jpeg_open(
        path: *const c_char,
        dst_w: u32,
        dst_h: u32,
        out_w: *mut u32,
        out_h: *mut u32,
    ) -> *mut RkDec;
    pub fn rk_jpeg_frame(
        handle: *mut RkDec,
        dst: *mut u8,
        pad_left: *mut i32,
        pad_top: *mut i32,
        scale: *mut f32,
    ) -> c_int;
    pub fn rk_jpeg_close(handle: *mut RkDec);
}
