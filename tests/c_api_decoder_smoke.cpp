#include "rk_mpp_rs.h"

#include <cstdint>
#include <cstring>

int main() {
    rk_mpp_decoder_config_t config{};
    rk_mpp_frame_t frame{};
    rk_mpp_encoder_config_t encoder_config{};
    rk_mpp_encoded_packet_t encoded_packet{};

    if (sizeof(config) != 32 || sizeof(frame) != 112 ||
        sizeof(encoder_config) != 68 || sizeof(encoded_packet) != 72)
        return 1;
    if (RK_MPP_CODEC_H265 != 3 || RK_MPP_CODEC_VP9 != 4 ||
        RK_MPP_CODEC_AV1 != 5 || RK_MPP_CODEC_VP8 != 6 ||
        RK_MPP_PIXEL_FORMAT_I420P10 != 3)
        return 1;
    if (!rk_mpp_api_version_string() || !rk_mpp_strerror(RK_MPP_EOS))
        return 1;
    if (rk_mpp_decoder_config_init(&config, RK_MPP_CODEC_H264,
                                   RK_MPP_PIXEL_FORMAT_NV12) != RK_MPP_OK)
        return 1;
    if (rk_mpp_decoder_config_init(nullptr, RK_MPP_CODEC_H264,
                                   RK_MPP_PIXEL_FORMAT_NV12) !=
        RK_MPP_ERR_INVALID)
        return 1;
    if (rk_mpp_frame_init(nullptr) != RK_MPP_ERR_INVALID)
        return 1;
    if (rk_mpp_encoder_config_init(&encoder_config, RK_MPP_CODEC_H264,
                                   320, 240,
                                   RK_MPP_PIXEL_FORMAT_NV12) != RK_MPP_OK)
        return 1;
    if (rk_mpp_encoded_packet_init(&encoded_packet) != RK_MPP_OK)
        return 1;
    if (rk_mpp_encoder_config_init(nullptr, RK_MPP_CODEC_H264, 320, 240,
                                   RK_MPP_PIXEL_FORMAT_NV12) !=
        RK_MPP_ERR_INVALID)
        return 1;
    rk_mpp_encoder_close(nullptr);
    rk_mpp_decoder_close(nullptr);
    return rk_mpp_api_version() == RK_MPP_API_VERSION ? 0 : 2;
}
