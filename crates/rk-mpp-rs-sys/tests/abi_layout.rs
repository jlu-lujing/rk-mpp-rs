use core::mem::{offset_of, size_of};
use rk_mpp_rs_sys::*;

macro_rules! assert_layout {
    ($type:ty, $size:expr) => {
        assert_eq!(
            size_of::<$type>(),
            $size,
            "{} layout changed",
            stringify!($type)
        );
    };
}

#[test]
fn types_match_the_v1_13_c_header() {
    assert_eq!(RK_MPP_API_VERSION, 0x1000d);
    assert_eq!(RK_MPP_MAX_ROI_REGIONS, 8);
    assert_eq!(RK_MPP_DECODER_FLAG_DMABUF_OUTPUT, 1);

    assert_layout!(rk_mpp_decoder_config_t, 32);
    assert_layout!(rk_mpp_frame_t, 112);
    assert_layout!(rk_mpp_encoder_config_t, 68);
    assert_layout!(rk_mpp_encoder_rate_control_t, 64);
    assert_layout!(rk_mpp_encoder_advanced_config_t, 104);
    assert_layout!(rk_mpp_encoder_roi_region_t, 40);
    assert_layout!(rk_mpp_encoder_gdr_t, 16);
    assert_layout!(rk_mpp_encoder_slice_split_t, 24);
    assert_layout!(rk_mpp_encoder_advanced_config_v2_t, 472);
    assert_layout!(rk_mpp_encoder_h264_deblock_t, 32);
    assert_layout!(rk_mpp_encoder_h264_syntax_t, 48);
    assert_layout!(rk_mpp_encoder_advanced_config_v3_t, 552);
    assert_layout!(rk_mpp_encoded_packet_t, 72);
}

#[test]
fn critical_field_offsets_match_the_c_header() {
    assert_eq!(offset_of!(rk_mpp_decoder_config_t, codec), 4);
    assert_eq!(offset_of!(rk_mpp_decoder_config_t, output_format), 8);
    assert_eq!(offset_of!(rk_mpp_decoder_config_t, max_reorder_frames), 16);

    assert_eq!(offset_of!(rk_mpp_frame_t, pts), 24);
    assert_eq!(offset_of!(rk_mpp_frame_t, plane_data), 48);
    assert_eq!(offset_of!(rk_mpp_frame_t, plane_sizes), 72);
    assert_eq!(offset_of!(rk_mpp_frame_t, plane_strides), 96);

    assert_eq!(offset_of!(rk_mpp_encoder_config_t, width), 12);
    assert_eq!(offset_of!(rk_mpp_encoder_config_t, qp), 32);
    assert_eq!(offset_of!(rk_mpp_encoder_config_t, jpeg_quality), 44);
    assert_eq!(offset_of!(rk_mpp_encoder_config_t, flags), 52);

    assert_eq!(offset_of!(rk_mpp_encoder_rate_control_t, bps_target), 8);
    assert_eq!(offset_of!(rk_mpp_encoder_rate_control_t, initial_qp), 32);
    assert_eq!(
        offset_of!(rk_mpp_encoder_advanced_config_t, rate_control),
        8
    );

    assert_eq!(offset_of!(rk_mpp_encoder_roi_region_t, width), 16);
    assert_eq!(offset_of!(rk_mpp_encoder_advanced_config_v2_t, gdr), 72);
    assert_eq!(offset_of!(rk_mpp_encoder_advanced_config_v2_t, rois), 120);
    assert_eq!(
        offset_of!(rk_mpp_encoder_advanced_config_v3_t, h264_deblock),
        440
    );
    assert_eq!(
        offset_of!(rk_mpp_encoder_advanced_config_v3_t, h264_syntax),
        472
    );

    assert_eq!(offset_of!(rk_mpp_encoded_packet_t, pts), 16);
    assert_eq!(offset_of!(rk_mpp_encoded_packet_t, data), 40);
    assert_eq!(offset_of!(rk_mpp_encoded_packet_t, size), 48);
}
