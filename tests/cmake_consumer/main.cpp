#include "rk_mpp_rs.h"

int main() {
    rk_mpp_decoder_config_t config{};
    if (rk_mpp_api_version() != RK_MPP_API_VERSION)
        return 1;
    if (rk_mpp_decoder_config_init(&config, RK_MPP_CODEC_H264,
                                   RK_MPP_PIXEL_FORMAT_NV12) != RK_MPP_OK)
        return 2;
    return 0;
}
