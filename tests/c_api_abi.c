#include "rk_mpp_rs.h"

#include <stddef.h>

_Static_assert(RK_MPP_API_VERSION == 0x1000bu, "API version changed");

_Static_assert(RK_MPP_CODEC_H264 == 1, "codec id changed");
_Static_assert(RK_MPP_CODEC_MJPEG == 2, "codec id changed");
_Static_assert(RK_MPP_CODEC_H265 == 3, "codec id changed");
_Static_assert(RK_MPP_CODEC_VP9 == 4, "codec id changed");
_Static_assert(RK_MPP_CODEC_AV1 == 5, "codec id changed");
_Static_assert(RK_MPP_CODEC_VP8 == 6, "codec id changed");
_Static_assert(RK_MPP_CODEC_MPEG2 == 7, "codec id changed");
_Static_assert(RK_MPP_CODEC_MPEG4 == 8, "codec id changed");
_Static_assert(RK_MPP_CODEC_H263 == 9, "codec id changed");
_Static_assert(RK_MPP_DECODER_FLAG_DMABUF_OUTPUT == 1u,
               "decoder flags changed");

_Static_assert(sizeof(rk_mpp_decoder_config_t) == 32,
               "decoder config layout changed");
_Static_assert(offsetof(rk_mpp_decoder_config_t, codec) == 4,
               "decoder config layout changed");
_Static_assert(offsetof(rk_mpp_decoder_config_t, output_format) == 8,
               "decoder config layout changed");
_Static_assert(offsetof(rk_mpp_decoder_config_t, max_reorder_frames) == 16,
               "decoder config layout changed");

_Static_assert(sizeof(rk_mpp_frame_t) == 112, "frame layout changed");
_Static_assert(offsetof(rk_mpp_frame_t, pts) == 24, "frame layout changed");
_Static_assert(offsetof(rk_mpp_frame_t, plane_data) == 48,
               "frame layout changed");
_Static_assert(offsetof(rk_mpp_frame_t, plane_sizes) == 72,
               "frame layout changed");
_Static_assert(offsetof(rk_mpp_frame_t, plane_strides) == 96,
               "frame layout changed");

_Static_assert(sizeof(rk_mpp_encoder_config_t) == 68,
               "encoder config layout changed");
_Static_assert(offsetof(rk_mpp_encoder_config_t, width) == 12,
               "encoder config layout changed");
_Static_assert(offsetof(rk_mpp_encoder_config_t, qp) == 32,
               "encoder config layout changed");
_Static_assert(offsetof(rk_mpp_encoder_config_t, jpeg_quality) == 44,
               "encoder config layout changed");
_Static_assert(offsetof(rk_mpp_encoder_config_t, flags) == 52,
               "encoder config layout changed");

_Static_assert(RK_MPP_RATE_CONTROL_FIXED_QP == 0, "rate mode changed");
_Static_assert(RK_MPP_RATE_CONTROL_CBR == 1, "rate mode changed");
_Static_assert(RK_MPP_RATE_CONTROL_VBR == 2, "rate mode changed");
_Static_assert(RK_MPP_RATE_CONTROL_AVBR == 3, "rate mode changed");
_Static_assert(sizeof(rk_mpp_encoder_rate_control_t) == 64,
               "rate-control config layout changed");
_Static_assert(offsetof(rk_mpp_encoder_rate_control_t, bps_target) == 8,
               "rate-control config layout changed");
_Static_assert(offsetof(rk_mpp_encoder_rate_control_t, initial_qp) == 32,
               "rate-control config layout changed");
_Static_assert(sizeof(rk_mpp_encoder_advanced_config_t) == 104,
               "advanced encoder config layout changed");
_Static_assert(offsetof(rk_mpp_encoder_advanced_config_t, rate_control) == 8,
               "advanced encoder config layout changed");
_Static_assert(sizeof(rk_mpp_encoder_roi_region_t) == 40,
               "ROI layout changed");
_Static_assert(offsetof(rk_mpp_encoder_roi_region_t, width) == 16,
               "ROI layout changed");
_Static_assert(sizeof(rk_mpp_encoder_gdr_t) == 16, "GDR layout changed");
_Static_assert(sizeof(rk_mpp_encoder_slice_split_t) == 24,
               "slice split layout changed");
_Static_assert(sizeof(rk_mpp_encoder_advanced_config_v2_t) == 472,
               "V2 advanced encoder config layout changed");
_Static_assert(offsetof(rk_mpp_encoder_advanced_config_v2_t, gdr) == 72,
               "V2 advanced encoder config layout changed");
_Static_assert(offsetof(rk_mpp_encoder_advanced_config_v2_t, rois) == 120,
               "V2 advanced encoder config layout changed");

_Static_assert(sizeof(rk_mpp_encoded_packet_t) == 72,
               "encoded packet layout changed");
_Static_assert(offsetof(rk_mpp_encoded_packet_t, pts) == 16,
               "encoded packet layout changed");
_Static_assert(offsetof(rk_mpp_encoded_packet_t, data) == 40,
               "encoded packet layout changed");
_Static_assert(offsetof(rk_mpp_encoded_packet_t, size) == 48,
               "encoded packet layout changed");

int main(void) {
    rk_mpp_encoder_rate_control_t rate_control;
    rk_mpp_encoder_advanced_config_t advanced;
    rk_mpp_encoder_advanced_config_v2_t advanced_v2;
    rk_mpp_encoder_roi_region_t roi;
    rk_mpp_encoder_gdr_t gdr;
    rk_mpp_encoder_slice_split_t split;

    if (rk_mpp_encoder_rate_control_init(
            &rate_control, RK_MPP_RATE_CONTROL_CBR) != RK_MPP_OK ||
        rate_control.mode != RK_MPP_RATE_CONTROL_CBR ||
        rate_control.initial_qp != 26 ||
        rate_control.min_qp != 8 ||
        rate_control.max_qp != 48) {
        return 1;
    }
    if (rk_mpp_encoder_advanced_config_init(
            &advanced, RK_MPP_RATE_CONTROL_VBR) != RK_MPP_OK ||
        advanced.rate_control.mode != RK_MPP_RATE_CONTROL_VBR) {
        return 2;
    }
    if (rk_mpp_encoder_rate_control_init(&rate_control, 99) !=
        RK_MPP_ERR_INVALID) {
        return 3;
    }
    if (rk_mpp_encoder_advanced_config_v2_init(
            &advanced_v2, RK_MPP_RATE_CONTROL_FIXED_QP) != RK_MPP_OK ||
        rk_mpp_encoder_roi_region_init(&roi) != RK_MPP_OK ||
        rk_mpp_encoder_gdr_init(&gdr, RK_MPP_GDR_ROW, 4) != RK_MPP_OK ||
        rk_mpp_encoder_slice_split_init(
            &split, RK_MPP_SLICE_SPLIT_BY_CTU, 96,
            RK_MPP_SLICE_OUTPUT_LOW_DELAY_PACKETS) != RK_MPP_OK) {
        return 4;
    }
    if (rk_mpp_encoder_gdr_init(&gdr, RK_MPP_GDR_DISABLED, 1) !=
        RK_MPP_ERR_INVALID) {
        return 5;
    }
    if (rk_mpp_decoder_export_frame_dmabuf(NULL, 0, NULL, NULL, NULL, NULL) !=
        RK_MPP_ERR_INVALID) {
        return 6;
    }
    return 0;
}
