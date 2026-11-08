#include "rk_mpp_rs.h"

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static uint16_t le16(const uint8_t *p) {
    return (uint16_t)p[0] | (uint16_t)p[1] << 8;
}

static uint32_t le32(const uint8_t *p) {
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 |
           (uint32_t)p[3] << 24;
}

static uint64_t le64(const uint8_t *p) {
    return (uint64_t)le32(p) | (uint64_t)le32(p + 4) << 32;
}

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
    if (!strcmp(name, "i420p10")) {
        *format = RK_MPP_PIXEL_FORMAT_I420P10;
        return 0;
    }
    return -1;
}

static int parse_codec(const char *name, uint32_t *codec,
                       const char **fourcc) {
    if (!strcmp(name, "vp9")) {
        *codec = RK_MPP_CODEC_VP9;
        *fourcc = "VP90";
        return 0;
    }
    if (!strcmp(name, "av1")) {
        *codec = RK_MPP_CODEC_AV1;
        *fourcc = "AV01";
        return 0;
    }
    if (!strcmp(name, "vp8")) {
        *codec = RK_MPP_CODEC_VP8;
        *fourcc = "VP80";
        return 0;
    }
    return -1;
}

static int write_frame(FILE *out, const rk_mpp_frame_t *frame,
                       uint64_t *bytes) {
    uint32_t plane;

    for (plane = 0; plane < 3; plane++) {
        if (!frame->plane_data[plane] || !frame->plane_sizes[plane])
            continue;
        if (out &&
            fwrite(frame->plane_data[plane], 1,
                   (size_t)frame->plane_sizes[plane], out) !=
                (size_t)frame->plane_sizes[plane])
            return -1;
        *bytes += frame->plane_sizes[plane];
    }
    return 0;
}

static int drain(rk_mpp_decoder_t *decoder, rk_mpp_frame_t *frame,
                 uint32_t format, FILE *out, uint64_t *frames,
                 uint64_t *bytes, int *saw_eos) {
    for (;;) {
        int status = rk_mpp_decoder_receive_frame(decoder, frame);

        if (status == RK_MPP_EOS) {
            *saw_eos = 1;
            return 0;
        }
        if (status == RK_MPP_AGAIN)
            return 0;
        if (status != RK_MPP_OK) {
            fprintf(stderr, "receive failed: %s\n", rk_mpp_strerror(status));
            return -1;
        }
        if (!frame->width || !frame->height || frame->format != format ||
            !frame->plane_data[0] || !frame->plane_sizes[0]) {
            fprintf(stderr, "invalid packet-decoder frame metadata\n");
            return -1;
        }
        if (*frames == 0 &&
            rk_mpp_decoder_receive_frame(decoder, frame) != RK_MPP_ERR_STATE) {
            fprintf(stderr, "double receive was not rejected\n");
            return -1;
        }
        if (write_frame(out, frame, bytes)) {
            fprintf(stderr, "write failed\n");
            return -1;
        }
        (*frames)++;
        rk_mpp_frame_release(decoder, frame);
    }
}

static int run_ivf(rk_mpp_decoder_t *decoder, const uint8_t *data, size_t size,
                   const char *fourcc, uint32_t format, FILE *out,
                   uint64_t *frames, uint64_t *bytes) {
    rk_mpp_frame_t frame;
    size_t header_size;
    size_t offset;
    int saw_eos = 0;

    if (size < 32 || memcmp(data, "DKIF", 4) ||
        memcmp(data + 8, fourcc, 4)) {
        fprintf(stderr, "input does not match the requested IVF codec\n");
        return -1;
    }
    header_size = le16(data + 6);
    if (header_size < 32 || header_size > size) {
        fprintf(stderr, "invalid IVF header size\n");
        return -1;
    }
    if (rk_mpp_frame_init(&frame) != RK_MPP_OK)
        return -1;

    offset = header_size;
    while (offset < size) {
        uint32_t packet_size;
        uint64_t timestamp;
        size_t next;
        int status;

        if (size - offset < 12) {
            fprintf(stderr, "truncated IVF frame header\n");
            return -1;
        }
        packet_size = le32(data + offset);
        timestamp = le64(data + offset + 4);
        next = offset + 12;
        if (packet_size > size - next) {
            fprintf(stderr, "truncated IVF packet\n");
            return -1;
        }
        status = rk_mpp_decoder_submit_packet(
            decoder, data + next, packet_size, (int64_t)timestamp, 0,
            next + packet_size == size ? RK_MPP_PACKET_FLAG_EOS : 0);
        if (status != RK_MPP_OK) {
            fprintf(stderr, "packet submit failed: %s\n",
                    rk_mpp_strerror(status));
            return -1;
        }
        offset = next + packet_size;
        if (drain(decoder, &frame, format, out, frames, bytes, &saw_eos))
            return -1;
    }
    if (!saw_eos) {
        if (rk_mpp_decoder_submit_packet(decoder, NULL, 0, 0, 0,
                                         RK_MPP_PACKET_FLAG_EOS) != RK_MPP_OK ||
            drain(decoder, &frame, format, out, frames, bytes, &saw_eos))
            return -1;
    }
    if (!saw_eos) {
        fprintf(stderr, "stream did not reach EOS\n");
        return -1;
    }
    if (rk_mpp_decoder_flush(decoder) != RK_MPP_OK ||
        rk_mpp_decoder_submit_packet(decoder, NULL, 0, 0, 0, 0) !=
            RK_MPP_ERR_STATE) {
        fprintf(stderr, "post-EOS state check failed\n");
        return -1;
    }
    return 0;
}

int main(int argc, char **argv) {
    rk_mpp_decoder_config_t config;
    rk_mpp_decoder_t *decoder = NULL;
    uint8_t *input = NULL;
    size_t input_size = 0;
    uint32_t format;
    uint32_t codec;
    const char *fourcc = NULL;
    FILE *out = NULL;
    uint64_t frames = 0;
    uint64_t bytes = 0;
    uint64_t reuse_frames = 0;
    uint64_t reuse_bytes = 0;
    uint32_t status_a;
    uint32_t status_b;
    uint32_t status_c;
    int ret = 1;

    if (argc != 5 || parse_codec(argv[1], &codec, &fourcc) ||
        parse_format(argv[2], &format) ||
        read_file(argv[4], &input, &input_size)) {
        fprintf(stderr,
                "usage: %s <vp8|vp9|av1> <nv12|i420|i420p10> <out> "
                "<in.ivf>\n",
                argv[0]);
        return 2;
    }
    if (rk_mpp_decoder_config_init(&config, codec, format) !=
            RK_MPP_OK ||
        rk_mpp_decoder_create(&config, &decoder) != RK_MPP_OK) {
        fprintf(stderr, "packet decoder create failed\n");
        goto done;
    }
    if (rk_mpp_decoder_status(decoder, &status_a, &status_b, &status_c) !=
        (codec == RK_MPP_CODEC_VP8 ? RK_MPP_ERR_UNSUPPORTED : RK_MPP_OK)) {
        fprintf(stderr, "packet decoder status query failed\n");
        goto done;
    }
    out = fopen(argv[3], "wb");
    if (!out) {
        fprintf(stderr, "%s: %s\n", argv[3], strerror(errno));
        goto done;
    }
    if (run_ivf(decoder, input, input_size, fourcc, format, out, &frames,
                &bytes) ||
        !frames || fflush(out))
        goto done;
    if (rk_mpp_decoder_reset(decoder) != RK_MPP_OK ||
        run_ivf(decoder, input, input_size, fourcc, format, NULL,
                &reuse_frames, &reuse_bytes)) {
        fprintf(stderr, "packet decoder reset/reuse failed\n");
        goto done;
    }
    if (frames != reuse_frames || bytes != reuse_bytes) {
        fprintf(stderr, "reuse mismatch: %llu/%llu frames, %llu/%llu bytes\n",
                (unsigned long long)frames, (unsigned long long)reuse_frames,
                (unsigned long long)bytes, (unsigned long long)reuse_bytes);
        goto done;
    }
    printf("PASS %s frames=%llu bytes=%llu api=%u\n", argv[1],
           (unsigned long long)frames, (unsigned long long)bytes,
           rk_mpp_api_version());
    ret = 0;

done:
    if (out)
        fclose(out);
    if (decoder)
        rk_mpp_decoder_close(decoder);
    free(input);
    return ret;
}
