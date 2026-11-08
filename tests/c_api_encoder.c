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
    rk_mpp_encoder_t *encoder = NULL;
    rk_mpp_encoded_packet_t packet;
    uint8_t *input = NULL;
    size_t input_size = 0;
    size_t frame_size;
    uint32_t format;
    uint32_t width;
    uint32_t height;
    uint32_t qp;
    uint32_t frames;
    uint32_t gop;
    FILE *out = NULL;
    uint64_t packets = 0;
    uint64_t keyframes = 0;
    int saw_eos = 0;
    int ret = 1;
    uint32_t index;

    if (argc != 9 || parse_format(argv[1], &format)) {
        fprintf(stderr,
                "usage: %s <nv12|i420> <out> <width> <height> <qp> "
                "<frames> <gop> <in>\n",
                argv[0]);
        return 2;
    }
    width = (uint32_t)strtoul(argv[3], NULL, 0);
    height = (uint32_t)strtoul(argv[4], NULL, 0);
    qp = (uint32_t)strtoul(argv[5], NULL, 0);
    frames = (uint32_t)strtoul(argv[6], NULL, 0);
    gop = (uint32_t)strtoul(argv[7], NULL, 0);
    frame_size = (size_t)width * height * 3 / 2;
    if (!width || !height || !frames || !gop || !frame_size ||
        read_file(argv[8], &input, &input_size)) {
        fprintf(stderr, "invalid encoder arguments or input\n");
        return 2;
    }
    if (input_size < frame_size * frames) {
        fprintf(stderr, "input has %zu bytes, needs %zu\n", input_size,
                frame_size * frames);
        goto done;
    }
    if (rk_mpp_encoder_config_init(&config, RK_MPP_CODEC_H264, width, height,
                                   format) != RK_MPP_OK)
        goto done;
    config.qp = qp;
    config.gop = gop;
    if (rk_mpp_encoder_create(&config, &encoder) != RK_MPP_OK ||
        !encoder) {
        fprintf(stderr, "encoder create failed\n");
        goto done;
    }
    if (rk_mpp_encoded_packet_init(&packet) != RK_MPP_OK)
        goto done;
    out = fopen(argv[2], "wb");
    if (!out) {
        fprintf(stderr, "%s: %s\n", argv[2], strerror(errno));
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
    printf("PASS H.264 API packets=%llu api=%u\n",
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
