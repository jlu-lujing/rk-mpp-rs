#ifndef RK_MPP_RS_V19_ENCODER_H
#define RK_MPP_RS_V19_ENCODER_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define RK_MPP_API_VERSION_MAJOR 1u
#define RK_MPP_API_VERSION_MINOR 9u
#define RK_MPP_API_VERSION \
    ((RK_MPP_API_VERSION_MAJOR << 16) | RK_MPP_API_VERSION_MINOR)

enum rk_mpp_status {
    RK_MPP_OK = 0,
    RK_MPP_AGAIN = 1,
    RK_MPP_EOS = 2,
    RK_MPP_ERR_INVALID = -1
};

enum rk_mpp_codec {
    RK_MPP_CODEC_H264 = 1
};

enum rk_mpp_pixel_format {
    RK_MPP_PIXEL_FORMAT_NV12 = 1
};

typedef struct rk_mpp_encoder_config {
    uint32_t struct_size;
    uint32_t codec;
    uint32_t input_format;
    uint32_t width;
    uint32_t height;
    uint32_t profile;
    uint32_t level;
    uint32_t gop;
    uint32_t qp;
    uint32_t fps_num;
    uint32_t fps_den;
    uint32_t jpeg_quality;
    uint32_t jpeg_restart_mcus;
    uint32_t flags;
    uint32_t reserved[3];
} rk_mpp_encoder_config_t;

enum rk_mpp_rate_control_mode {
    RK_MPP_RATE_CONTROL_FIXED_QP = 0,
    RK_MPP_RATE_CONTROL_CBR = 1,
    RK_MPP_RATE_CONTROL_VBR = 2,
    RK_MPP_RATE_CONTROL_AVBR = 3
};

typedef struct rk_mpp_encoder_rate_control {
    uint32_t struct_size;
    uint32_t mode;
    int64_t bps_target;
    int64_t bps_min;
    int64_t bps_max;
    uint32_t initial_qp;
    uint32_t min_qp;
    uint32_t max_qp;
    uint32_t reserved[5];
} rk_mpp_encoder_rate_control_t;

typedef struct rk_mpp_encoder_advanced_config {
    uint32_t struct_size;
    uint32_t flags;
    rk_mpp_encoder_rate_control_t rate_control;
    uint32_t reserved[8];
} rk_mpp_encoder_advanced_config_t;

enum rk_mpp_encoded_packet_flags {
    RK_MPP_ENCODED_PACKET_FLAG_KEYFRAME = 1u << 0
};

typedef struct rk_mpp_encoded_packet {
    uint32_t struct_size;
    uint32_t flags;
    uint32_t reserved0;
    uint32_t reserved1;
    int64_t pts;
    int64_t dts;
    int32_t frame_num;
    int32_t poc;
    const uint8_t *data;
    uint64_t size;
    uint64_t reserved[2];
} rk_mpp_encoded_packet_t;

typedef struct rk_mpp_encoder rk_mpp_encoder_t;

uint32_t rk_mpp_api_version(void);
const char *rk_mpp_strerror(int code);
int rk_mpp_encoder_config_init(rk_mpp_encoder_config_t *config,
                               uint32_t codec,
                               uint32_t width,
                               uint32_t height,
                               uint32_t input_format);
int rk_mpp_encoder_advanced_config_init(
    rk_mpp_encoder_advanced_config_t *config,
    uint32_t rate_control_mode);
int rk_mpp_encoded_packet_init(rk_mpp_encoded_packet_t *packet);
int rk_mpp_encoder_create_advanced(
    const rk_mpp_encoder_config_t *config,
    const rk_mpp_encoder_advanced_config_t *advanced_config,
    rk_mpp_encoder_t **out_encoder);
int rk_mpp_encoder_submit_frame(rk_mpp_encoder_t *encoder,
                                const uint8_t *data,
                                uint64_t size,
                                int64_t pts,
                                int64_t dts);
int rk_mpp_encoder_receive_packet(rk_mpp_encoder_t *encoder,
                                  rk_mpp_encoded_packet_t *packet);
void rk_mpp_encoded_packet_release(rk_mpp_encoder_t *encoder,
                                   rk_mpp_encoded_packet_t *packet);
int rk_mpp_encoder_flush(rk_mpp_encoder_t *encoder);
void rk_mpp_encoder_close(rk_mpp_encoder_t *encoder);

#ifdef __cplusplus
}
#endif

#endif
