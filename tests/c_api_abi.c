#include "rk_mpp_rs.h"

#include <stddef.h>

_Static_assert(RK_MPP_API_VERSION == 0x10008u, "API version changed");

_Static_assert(RK_MPP_CODEC_H264 == 1, "codec id changed");
_Static_assert(RK_MPP_CODEC_MJPEG == 2, "codec id changed");
_Static_assert(RK_MPP_CODEC_H265 == 3, "codec id changed");
_Static_assert(RK_MPP_CODEC_VP9 == 4, "codec id changed");
_Static_assert(RK_MPP_CODEC_AV1 == 5, "codec id changed");
_Static_assert(RK_MPP_CODEC_VP8 == 6, "codec id changed");
_Static_assert(RK_MPP_CODEC_MPEG2 == 7, "codec id changed");
_Static_assert(RK_MPP_CODEC_MPEG4 == 8, "codec id changed");
_Static_assert(RK_MPP_CODEC_H263 == 9, "codec id changed");

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

_Static_assert(sizeof(rk_mpp_encoded_packet_t) == 72,
               "encoded packet layout changed");
_Static_assert(offsetof(rk_mpp_encoded_packet_t, pts) == 16,
               "encoded packet layout changed");
_Static_assert(offsetof(rk_mpp_encoded_packet_t, data) == 40,
               "encoded packet layout changed");
_Static_assert(offsetof(rk_mpp_encoded_packet_t, size) == 48,
               "encoded packet layout changed");

int main(void) {
    return 0;
}
