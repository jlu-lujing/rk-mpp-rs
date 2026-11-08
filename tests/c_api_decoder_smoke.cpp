#include "rk_mpp_rs.h"

#include <cstdint>
#include <cstring>

int main() {
    rk_mpp_decoder_config_t config{};
    rk_mpp_frame_t frame{};

    if (sizeof(config) != 32 || sizeof(frame) != 112)
        return 1;
    if (RK_MPP_CODEC_H265 != 3 || RK_MPP_PIXEL_FORMAT_I420P10 != 3)
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
    rk_mpp_decoder_close(nullptr);
    return rk_mpp_api_version() == RK_MPP_API_VERSION ? 0 : 2;
}
