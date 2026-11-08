#include "rk_mpp_rs-v1.9-encoder.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define WIDTH 320u
#define HEIGHT 240u
#define FRAME_SIZE ((size_t)WIDTH * HEIGHT * 3 / 2)
#define FRAMES 2u

static int read_file(const char *path, uint8_t **data, size_t *size) {
    FILE *file = fopen(path, "rb");
    long length;

    if (!file) {
        fprintf(stderr, "%s: %s\n", path, strerror(errno));
        return -1;
    }
    if (fseek(file, 0, SEEK_END) || (length = ftell(file)) < 0 ||
        fseek(file, 0, SEEK_SET)) {
        fclose(file);
        return -1;
    }
    *data = malloc((size_t)length);
    if (!*data) {
        fclose(file);
        return -1;
    }
    if (fread(*data, 1, (size_t)length, file) != (size_t)length) {
        fclose(file);
        free(*data);
        *data = NULL;
        return -1;
    }
    fclose(file);
    *size = (size_t)length;
    return 0;
}

static int drain(rk_mpp_encoder_t *encoder, rk_mpp_encoded_packet_t *packet,
                 FILE *out, unsigned *packets) {
    for (;;) {
        int status = rk_mpp_encoder_receive_packet(encoder, packet);

        if (status == RK_MPP_EOS || status == RK_MPP_AGAIN)
            return 0;
        if (status != RK_MPP_OK)
            return -1;
        if (out && fwrite(packet->data, 1, (size_t)packet->size, out) !=
                       (size_t)packet->size) {
            return -1;
        }
        (*packets)++;
        rk_mpp_encoded_packet_release(encoder, packet);
    }
}

int main(int argc, char **argv) {
    rk_mpp_encoder_config_t config;
    rk_mpp_encoder_advanced_config_t advanced;
    rk_mpp_encoded_packet_t packet;
    rk_mpp_encoder_t *encoder = NULL;
    uint8_t *input = NULL;
    size_t input_size = 0;
    FILE *out = NULL;
    unsigned packets = 0;
    unsigned index;
    int ret = 1;

    if (argc != 3 || read_file(argv[1], &input, &input_size) ||
        input_size < FRAME_SIZE * FRAMES) {
        fprintf(stderr, "usage: %s <2-frame.nv12> <out.264>\n", argv[0]);
        return 2;
    }
    if (rk_mpp_api_version() < RK_MPP_API_VERSION)
        goto done;
    if (rk_mpp_encoder_config_init(&config, RK_MPP_CODEC_H264, WIDTH, HEIGHT,
                                   RK_MPP_PIXEL_FORMAT_NV12) != RK_MPP_OK)
        goto done;
    config.gop = 6;
    config.qp = 26;
    if (rk_mpp_encoder_advanced_config_init(
            &advanced, RK_MPP_RATE_CONTROL_CBR) != RK_MPP_OK)
        goto done;
    advanced.rate_control.bps_target = 300000;
    advanced.rate_control.bps_min = 250000;
    advanced.rate_control.bps_max = 350000;
    advanced.rate_control.initial_qp = 26;
    advanced.rate_control.min_qp = 10;
    advanced.rate_control.max_qp = 51;
    if (rk_mpp_encoder_create_advanced(&config, &advanced, &encoder) !=
            RK_MPP_OK ||
        !encoder || rk_mpp_encoded_packet_init(&packet) != RK_MPP_OK)
        goto done;
    out = fopen(argv[2], "wb");
    if (!out)
        goto done;
    for (index = 0; index < FRAMES; index++) {
        if (rk_mpp_encoder_submit_frame(
                encoder, input + (size_t)index * FRAME_SIZE, FRAME_SIZE,
                index, 0) != RK_MPP_OK ||
            drain(encoder, &packet, out, &packets))
            goto done;
    }
    if (rk_mpp_encoder_flush(encoder) != RK_MPP_OK ||
        drain(encoder, &packet, out, &packets) || !packets || fflush(out))
        goto done;
    printf("PASS V1.9 advanced consumer packets=%u api=%u\n", packets,
           rk_mpp_api_version());
    ret = 0;

done:
    if (out)
        fclose(out);
    if (encoder)
        rk_mpp_encoder_close(encoder);
    free(input);
    return ret;
}
