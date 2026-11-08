#include "rk_mpp_rs.h"

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

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

static int parse_format(const char *name, uint32_t *format) {
    if (!strcmp(name, "nv12")) {
        *format = RK_MPP_PIXEL_FORMAT_NV12;
        return 0;
    }
    if (!strcmp(name, "i420")) {
        *format = RK_MPP_PIXEL_FORMAT_I420;
        return 0;
    }
    return -1;
}

static int parse_codec(const char *name, uint32_t *codec) {
    if (!strcmp(name, "h264")) {
        *codec = RK_MPP_CODEC_H264;
        return 0;
    }
    if (!strcmp(name, "h265")) {
        *codec = RK_MPP_CODEC_H265;
        return 0;
    }
    if (!strcmp(name, "jpeg")) {
        *codec = RK_MPP_CODEC_MJPEG;
        return 0;
    }
    return -1;
}

static int parse_rate_mode(const char *name, uint32_t *mode) {
    if (!strcmp(name, "fixqp")) {
        *mode = RK_MPP_RATE_CONTROL_FIXED_QP;
        return 0;
    }
    if (!strcmp(name, "cbr")) {
        *mode = RK_MPP_RATE_CONTROL_CBR;
        return 0;
    }
    if (!strcmp(name, "vbr")) {
        *mode = RK_MPP_RATE_CONTROL_VBR;
        return 0;
    }
    if (!strcmp(name, "avbr")) {
        *mode = RK_MPP_RATE_CONTROL_AVBR;
        return 0;
    }
    return -1;
}

static int drain_packets(rk_mpp_encoder_t *encoder,
                         rk_mpp_encoded_packet_t *packet, FILE *out,
                         uint64_t *packets, uint64_t *keyframes,
                         int *saw_eos) {
    for (;;) {
        int status = rk_mpp_encoder_receive_packet(encoder, packet);

        if (status == RK_MPP_AGAIN)
            return 0;
        if (status == RK_MPP_EOS) {
            *saw_eos = 1;
            return 0;
        }
        if (status != RK_MPP_OK) {
            fprintf(stderr, "receive packet failed: %s\n",
                    rk_mpp_strerror(status));
            return -1;
        }
        if (!packet->data || !packet->size) {
            fprintf(stderr, "empty encoded packet\n");
            return -1;
        }
        if (*packets == 0 &&
            rk_mpp_encoder_receive_packet(encoder, packet) != RK_MPP_ERR_STATE) {
            fprintf(stderr, "double encoder receive was not rejected\n");
            return -1;
        }
        if (out &&
            fwrite(packet->data, 1, (size_t)packet->size, out) !=
                (size_t)packet->size) {
            fprintf(stderr, "packet write failed\n");
            return -1;
        }
        if (packet->flags & RK_MPP_ENCODED_PACKET_FLAG_KEYFRAME)
            (*keyframes)++;
        (*packets)++;
        rk_mpp_encoded_packet_release(encoder, packet);
    }
}

int main(int argc, char **argv) {
    rk_mpp_encoder_config_t config;
    rk_mpp_encoder_advanced_config_t advanced;
    rk_mpp_encoder_t *encoder = NULL;
    rk_mpp_encoded_packet_t packet;
    uint8_t *input = NULL;
    size_t input_size = 0;
    size_t frame_size;
    uint32_t format;
    uint32_t codec;
    uint32_t width;
    uint32_t height;
    uint32_t qp;
    uint32_t frames;
    uint32_t gop;
    uint32_t rate_mode = RK_MPP_RATE_CONTROL_FIXED_QP;
    FILE *out = NULL;
    uint64_t packets = 0;
    uint64_t keyframes = 0;
    int saw_eos = 0;
    int ret = 1;
    uint32_t index;
    const char *rate_mode_name = getenv("RATE_CONTROL_MODE");

    if (argc != 10 || parse_codec(argv[1], &codec) ||
        parse_format(argv[2], &format)) {
        fprintf(stderr,
                "usage: %s <h264|h265|jpeg> <nv12|i420> <out> <width> <height> "
                "<qp> <frames> <gop> <in>\n",
                argv[0]);
        return 2;
    }
    width = (uint32_t)strtoul(argv[4], NULL, 0);
    height = (uint32_t)strtoul(argv[5], NULL, 0);
    qp = (uint32_t)strtoul(argv[6], NULL, 0);
    frames = (uint32_t)strtoul(argv[7], NULL, 0);
    gop = (uint32_t)strtoul(argv[8], NULL, 0);
    frame_size = (size_t)width * height * 3 / 2;
    if (!width || !height || !frames || !gop || !frame_size ||
        read_file(argv[9], &input, &input_size)) {
        fprintf(stderr, "invalid encoder arguments or input\n");
        return 2;
    }
    if (input_size < frame_size * frames) {
        fprintf(stderr, "input has %zu bytes, needs %zu\n", input_size,
                frame_size * frames);
        goto done;
    }
    if (rk_mpp_encoder_config_init(&config, codec, width, height,
                                   format) != RK_MPP_OK)
        goto done;
    config.qp = qp;
    config.gop = gop;
    if (codec == RK_MPP_CODEC_MJPEG)
        config.jpeg_quality = qp;
    if (codec == RK_MPP_CODEC_MJPEG) {
        const char *restart = getenv("JPEG_RESTART_MCUS");

        if (restart)
            config.jpeg_restart_mcus = (uint32_t)strtoul(restart, NULL, 0);
    }
    if (rate_mode_name) {
        const char *target = getenv("RATE_TARGET_BPS");
        const char *minimum = getenv("RATE_MIN_BPS");
        const char *maximum = getenv("RATE_MAX_BPS");
        const char *initial_qp = getenv("RATE_INITIAL_QP");
        const char *min_qp = getenv("RATE_MIN_QP");
        const char *max_qp = getenv("RATE_MAX_QP");
        int advanced_status;

        if (parse_rate_mode(rate_mode_name, &rate_mode) ||
            rk_mpp_encoder_advanced_config_init(&advanced, rate_mode) !=
                RK_MPP_OK ||
            (rate_mode != RK_MPP_RATE_CONTROL_FIXED_QP &&
             (!target || !minimum || !maximum))) {
            fprintf(stderr, "invalid rate-control environment\n");
            goto done;
        }
        if (target)
            advanced.rate_control.bps_target = strtoll(target, NULL, 0);
        if (minimum)
            advanced.rate_control.bps_min = strtoll(minimum, NULL, 0);
        if (maximum)
            advanced.rate_control.bps_max = strtoll(maximum, NULL, 0);
        if (initial_qp)
            advanced.rate_control.initial_qp =
                (uint32_t)strtoul(initial_qp, NULL, 0);
        if (min_qp)
            advanced.rate_control.min_qp =
                (uint32_t)strtoul(min_qp, NULL, 0);
        if (max_qp)
            advanced.rate_control.max_qp =
                (uint32_t)strtoul(max_qp, NULL, 0);
        advanced_status =
            rk_mpp_encoder_create_advanced(&config, &advanced, &encoder);
        if (advanced_status != RK_MPP_OK || !encoder) {
            fprintf(stderr, "advanced encoder create failed: %s\n",
                    rk_mpp_strerror(advanced_status));
            goto done;
        }
    } else if (rk_mpp_encoder_create(&config, &encoder) != RK_MPP_OK ||
               !encoder) {
        fprintf(stderr, "encoder create failed\n");
        goto done;
    }
    if (rk_mpp_encoded_packet_init(&packet) != RK_MPP_OK)
        goto done;
    out = fopen(argv[3], "wb");
    if (!out) {
        fprintf(stderr, "%s: %s\n", argv[3], strerror(errno));
        goto done;
    }
    for (index = 0; index < frames; index++) {
        int status = rk_mpp_encoder_submit_frame(
            encoder, input + (size_t)index * frame_size, frame_size, index, 0);
        if (status != RK_MPP_OK) {
            fprintf(stderr, "submit frame failed: %s\n",
                    rk_mpp_strerror(status));
            goto done;
        }
        if (drain_packets(encoder, &packet, out, &packets, &keyframes,
                          &saw_eos))
            goto done;
    }
    if (rk_mpp_encoder_flush(encoder) != RK_MPP_OK ||
        drain_packets(encoder, &packet, out, &packets, &keyframes,
                      &saw_eos) ||
        !saw_eos || fflush(out)) {
        fprintf(stderr, "encoder flush or EOS failed\n");
        goto done;
    }
    if (rk_mpp_encoder_submit_frame(encoder, input, frame_size, 0, 0) !=
        RK_MPP_ERR_STATE) {
        fprintf(stderr, "submit after encoder EOS was not rejected\n");
        goto done;
    }
    {
        uint64_t before = packets;

        if (rk_mpp_encoder_reset(encoder) != RK_MPP_OK ||
            rk_mpp_encoder_submit_frame(encoder, input, frame_size, 0, 0) !=
                RK_MPP_OK ||
            drain_packets(encoder, &packet, NULL, &packets, &keyframes,
                          &saw_eos) ||
            packets == before) {
            fprintf(stderr, "encoder reset/reuse failed\n");
            goto done;
        }
    }
    if (keyframes < 2) {
        fprintf(stderr, "encoder reset/reuse did not emit two keyframes\n");
        goto done;
    }
    printf("PASS %s API packets=%llu api=%u\n", argv[1],
           (unsigned long long)packets, rk_mpp_api_version());
    ret = 0;

done:
    if (out)
        fclose(out);
    if (encoder)
        rk_mpp_encoder_close(encoder);
    free(input);
    return ret;
}
