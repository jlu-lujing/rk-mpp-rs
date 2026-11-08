#include "rk_mpp_rs.h"

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static void usage(const char *program) {
    fprintf(stderr,
            "usage: %s <h264|h265|mpeg2|mpeg4|h263|mjpeg> "
            "<nv12|i420|i420p10> <chunk> <out> <in>...\n",
            program);
}

static int expect_status(const char *what, int actual, int expected) {
    if (actual == expected)
        return 0;
    fprintf(stderr, "%s: got %d (%s), expected %d (%s)\n", what, actual,
            rk_mpp_strerror(actual), expected, rk_mpp_strerror(expected));
    return -1;
}

static int parse_codec(const char *name, uint32_t *codec) {
    if (!strcmp(name, "h264")) {
        *codec = RK_MPP_CODEC_H264;
        return 0;
    }
    if (!strcmp(name, "mjpeg")) {
        *codec = RK_MPP_CODEC_MJPEG;
        return 0;
    }
    if (!strcmp(name, "h265")) {
        *codec = RK_MPP_CODEC_H265;
        return 0;
    }
    if (!strcmp(name, "mpeg2")) {
        *codec = RK_MPP_CODEC_MPEG2;
        return 0;
    }
    if (!strcmp(name, "mpeg4")) {
        *codec = RK_MPP_CODEC_MPEG4;
        return 0;
    }
    if (!strcmp(name, "h263")) {
        *codec = RK_MPP_CODEC_H263;
        return 0;
    }
    return -1;
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

static int append_file(uint8_t **data, size_t *size, size_t *capacity,
                       const char *path) {
    FILE *file = fopen(path, "rb");
    long length;
    size_t needed;

    if (!file) {
        fprintf(stderr, "%s: %s\n", path, strerror(errno));
        return -1;
    }
    if (fseek(file, 0, SEEK_END) || (length = ftell(file)) < 0 ||
        fseek(file, 0, SEEK_SET)) {
        fprintf(stderr, "%s: seek failed\n", path);
        fclose(file);
        return -1;
    }
    needed = *size + (size_t)length;
    if (needed > *capacity) {
        size_t next = *capacity ? *capacity : 4096;
        uint8_t *grown;

        while (next < needed)
            next *= 2;
        grown = realloc(*data, next);
        if (!grown) {
            fclose(file);
            return -1;
        }
        *data = grown;
        *capacity = next;
    }
    if (fread(*data + *size, 1, (size_t)length, file) != (size_t)length) {
        fprintf(stderr, "%s: read failed\n", path);
        fclose(file);
        return -1;
    }
    fclose(file);
    *size = needed;
    return 0;
}

static int write_frame(FILE *out, const rk_mpp_frame_t *frame,
                       uint64_t *bytes_written) {
    uint32_t plane;

    for (plane = 0; plane < 3; plane++) {
        uint32_t row_bytes;
        uint32_t rows;
        uint32_t row;

        if (!frame->plane_data[plane] || !frame->plane_sizes[plane] ||
            !frame->plane_strides[plane])
            continue;
        if (plane == 0) {
            row_bytes = frame->width;
            rows = frame->height;
        } else {
            row_bytes = frame->width / 2;
            rows = frame->height / 2;
        }
        if (frame->format == RK_MPP_PIXEL_FORMAT_NV12 && plane == 1)
            row_bytes = (frame->width / 2) * 2;
        if (frame->format == RK_MPP_PIXEL_FORMAT_I420P10)
            row_bytes *= 2;
        for (row = 0; row < rows; row++) {
            uint64_t offset = (uint64_t)row * frame->plane_strides[plane];
            const uint8_t *data = frame->plane_data[plane] + offset;

            if (offset + row_bytes > frame->plane_sizes[plane] ||
                fwrite(data, 1, row_bytes, out) != row_bytes)
                return -1;
            *bytes_written += row_bytes;
        }
    }
    return 0;
}

static int validate_dmabuf(rk_mpp_decoder_t *decoder, uint32_t codec,
                           uint32_t format) {
    uint32_t plane;

    if (!getenv("REQUIRE_DMABUF")) {
        if (getenv("EXPECT_NO_DMABUF")) {
            int fd = -1;

            if (rk_mpp_decoder_export_frame_dmabuf(
                    decoder, 0, &fd, NULL, NULL, NULL) !=
                    RK_MPP_ERR_UNSUPPORTED ||
                fd != -1) {
                fprintf(stderr, "CPU frame unexpectedly exported DMA-BUF\n");
                return -1;
            }
        }
        return 0;
    }
    if (codec != RK_MPP_CODEC_MJPEG || format != RK_MPP_PIXEL_FORMAT_NV12) {
        fprintf(stderr, "REQUIRE_DMABUF only supports MJPEG NV12\n");
        return -1;
    }
    for (plane = 0; plane < 2; plane++) {
        int fd = -1;
        uint64_t offset = 0;
        uint64_t size = 0;
        uint32_t stride = 0;
        struct stat status;
        int result = rk_mpp_decoder_export_frame_dmabuf(
            decoder, plane, &fd, &offset, &size, &stride);

        if (result != RK_MPP_OK || fd < 0 || !size || !stride ||
            size > UINT64_MAX - offset ||
            fstat(fd, &status) || status.st_size < 0 ||
            (uint64_t)status.st_size < offset + size) {
            fprintf(stderr, "DMA-BUF plane %u export failed: %s\n", plane,
                    rk_mpp_strerror(result));
            if (fd >= 0)
                close(fd);
            return -1;
        }
        close(fd);
    }
    {
        int fd = -1;

        if (rk_mpp_decoder_export_frame_dmabuf(decoder, 2, &fd, NULL, NULL,
                                               NULL) !=
            RK_MPP_ERR_UNSUPPORTED) {
            fprintf(stderr, "empty DMA-BUF plane was not rejected\n");
            return -1;
        }
    }
    return 0;
}

int main(int argc, char **argv) {
    rk_mpp_decoder_config_t config;
    rk_mpp_decoder_t *decoder = NULL;
    rk_mpp_frame_t frame;
    uint32_t codec;
    uint32_t format;
    size_t chunk;
    uint8_t *input = NULL;
    size_t input_size = 0;
    size_t input_capacity = 0;
    size_t offset = 0;
    FILE *out = NULL;
    uint64_t bytes_written = 0;
    uint64_t frames = 0;
    uint64_t reuse_frames = 0;
    int ret = 1;
    int index;
    int saw_eos = 0;

    if (argc < 6 || parse_codec(argv[1], &codec) ||
        parse_format(argv[2], &format)) {
        usage(argv[0]);
        return 2;
    }
    chunk = (size_t)strtoul(argv[3], NULL, 0);
    if (!chunk) {
        usage(argv[0]);
        return 2;
    }
    for (index = 5; index < argc; index++) {
        if (append_file(&input, &input_size, &input_capacity, argv[index]))
            goto done;
    }
    if (rk_mpp_api_version() != RK_MPP_API_VERSION) {
        fprintf(stderr, "API version mismatch: %u\n", rk_mpp_api_version());
        goto done;
    }
    if (!rk_mpp_api_version_string() || !rk_mpp_strerror(RK_MPP_EOS)) {
        fprintf(stderr, "API metadata unavailable\n");
        goto done;
    }
    if (expect_status("null config init",
                      rk_mpp_decoder_config_init(
                          NULL, RK_MPP_CODEC_H264, RK_MPP_PIXEL_FORMAT_NV12),
                      RK_MPP_ERR_INVALID) ||
        expect_status("null frame init", rk_mpp_frame_init(NULL),
                      RK_MPP_ERR_INVALID))
        goto done;
    if (rk_mpp_decoder_config_init(&config, codec, format) != RK_MPP_OK) {
        fprintf(stderr, "config init failed\n");
        goto done;
    }
    if (getenv("REQUIRE_DMABUF"))
        config.flags |= RK_MPP_DECODER_FLAG_DMABUF_OUTPUT;
    if (config.struct_size != sizeof(config)) {
        fprintf(stderr, "config struct size mismatch\n");
        goto done;
    }
    {
        rk_mpp_decoder_config_t invalid = config;

        if (expect_status("invalid codec init",
                          rk_mpp_decoder_config_init(&invalid, UINT32_MAX,
                                                     format),
                          RK_MPP_ERR_INVALID))
            goto done;
        invalid = config;
        invalid.struct_size--;
        decoder = (rk_mpp_decoder_t *)(uintptr_t)1;
        if (expect_status("decoder create with wrong struct size",
                          rk_mpp_decoder_create(&invalid, &decoder),
                          RK_MPP_ERR_INVALID) ||
            decoder != NULL) {
            fprintf(stderr, "invalid create left a decoder pointer\n");
            goto done;
        }
        if (codec == RK_MPP_CODEC_H264 || codec == RK_MPP_CODEC_MJPEG) {
            invalid = config;
            invalid.output_format = RK_MPP_PIXEL_FORMAT_I420P10;
            if (expect_status("incompatible output format",
                              rk_mpp_decoder_create(&invalid, &decoder),
                              RK_MPP_ERR_UNSUPPORTED) ||
                decoder != NULL) {
                fprintf(stderr, "unsupported create left a decoder pointer\n");
                goto done;
            }
        }
    }
    if (expect_status("null config create",
                      rk_mpp_decoder_create(NULL, &decoder),
                      RK_MPP_ERR_INVALID) ||
        expect_status("null output create",
                      rk_mpp_decoder_create(&config, NULL),
                      RK_MPP_ERR_INVALID))
        goto done;
    if (expect_status("decoder create", rk_mpp_decoder_create(&config, &decoder),
                      RK_MPP_OK) ||
        !decoder) {
        fprintf(stderr, "create failed\n");
        goto done;
    }
    if (expect_status("frame init", rk_mpp_frame_init(&frame), RK_MPP_OK)) {
        fprintf(stderr, "frame init failed\n");
        goto done;
    }
    if (expect_status("receive before input",
                      rk_mpp_decoder_receive_frame(decoder, &frame),
                      RK_MPP_AGAIN) ||
        expect_status("null packet payload",
                      rk_mpp_decoder_submit_packet(decoder, NULL, 1, 0, 0, 0),
                      RK_MPP_ERR_INVALID) ||
        expect_status("unknown packet flags",
                      rk_mpp_decoder_submit_packet(decoder, NULL, 0, 0, 0,
                                                   UINT32_C(1) << 31),
                      RK_MPP_ERR_INVALID))
        goto done;
    {
        uint32_t status_a = 0;
        uint32_t status_b = 0;
        uint32_t status_c = 0;
        int status = rk_mpp_decoder_status(decoder, &status_a, &status_b,
                                           &status_c);
        int expected = codec == RK_MPP_CODEC_H264 || codec == RK_MPP_CODEC_H265
                           ? RK_MPP_OK
                           : RK_MPP_ERR_UNSUPPORTED;

        if (expect_status("initial status", status, expected) ||
            expect_status("null status decoder",
                          rk_mpp_decoder_status(NULL, NULL, NULL, NULL),
                          RK_MPP_ERR_INVALID))
            goto done;
    }
    out = fopen(argv[4], "wb");
    if (!out) {
        fprintf(stderr, "%s: %s\n", argv[4], strerror(errno));
        goto done;
    }

    while (offset < input_size) {
        size_t take = input_size - offset < chunk ? input_size - offset : chunk;
        uint32_t flags = offset + take == input_size ? RK_MPP_PACKET_FLAG_EOS : 0;
        int status;

        status = rk_mpp_decoder_submit_packet(decoder, input + offset, take,
                                              (int64_t)frames, 0, flags);
        if (status != RK_MPP_OK) {
            fprintf(stderr, "submit failed: %s\n", rk_mpp_strerror(status));
            goto done;
        }
        offset += take;
        for (;;) {
            status = rk_mpp_decoder_receive_frame(decoder, &frame);
            if (status == RK_MPP_EOS) {
                saw_eos = 1;
                break;
            }
            if (status == RK_MPP_AGAIN)
                break;
            if (status != RK_MPP_OK) {
                fprintf(stderr, "receive failed: %s\n", rk_mpp_strerror(status));
                goto done;
            }
            if (!frames) {
                if (frame.width == 0 || frame.height == 0 ||
                    frame.format != format || !frame.plane_data[0] ||
                    !frame.plane_sizes[0]) {
                    fprintf(stderr, "invalid first frame metadata\n");
                    goto done;
                }
                if (expect_status("receive before release",
                                  rk_mpp_decoder_receive_frame(decoder, &frame),
                                  RK_MPP_ERR_STATE))
                    goto done;
            }
            if (validate_dmabuf(decoder, codec, format)) {
                fprintf(stderr, "DMA-BUF validation failed\n");
                goto done;
            }
            if (write_frame(out, &frame, &bytes_written)) {
                fprintf(stderr, "write failed\n");
                goto done;
            }
            frames++;
            rk_mpp_frame_release(decoder, &frame);
        }
    }
    {
        int status = rk_mpp_decoder_flush(decoder);
        if (status != RK_MPP_OK) {
            fprintf(stderr, "flush failed: %s\n", rk_mpp_strerror(status));
            goto done;
        }
    }
    for (;;) {
        int status = rk_mpp_decoder_receive_frame(decoder, &frame);
        if (status == RK_MPP_EOS) {
            saw_eos = 1;
            break;
        }
        if (status == RK_MPP_AGAIN)
            break;
        if (status != RK_MPP_OK) {
            fprintf(stderr, "flush receive failed: %s\n", rk_mpp_strerror(status));
            goto done;
        }
        if (write_frame(out, &frame, &bytes_written)) {
            fprintf(stderr, "flush write failed\n");
            goto done;
        }
        frames++;
        rk_mpp_frame_release(decoder, &frame);
    }
    if (!saw_eos) {
        fprintf(stderr, "stream did not reach EOS\n");
        goto done;
    }
    if (expect_status("submit after EOS",
                      rk_mpp_decoder_submit_packet(decoder, NULL, 0, 0, 0, 0),
                      RK_MPP_ERR_STATE))
        goto done;
    if (codec == RK_MPP_CODEC_MJPEG) {
        static const uint8_t malformed_jpeg[] = {0xff, 0xd8, 0xff, 0xd9};

        if (expect_status("reset before malformed JPEG",
                          rk_mpp_decoder_reset(decoder), RK_MPP_OK) ||
            expect_status(
                "malformed JPEG",
                rk_mpp_decoder_submit_packet(
                    decoder, malformed_jpeg, sizeof(malformed_jpeg), 0, 0,
                    RK_MPP_PACKET_FLAG_EOS),
                RK_MPP_ERR_PARSE) ||
            expect_status("recover after malformed JPEG",
                          rk_mpp_decoder_recover(decoder), RK_MPP_OK))
            goto done;
    }
    if (rk_mpp_decoder_reset(decoder) != RK_MPP_OK) {
        fprintf(stderr, "reset failed\n");
        goto done;
    }
    if (!frames || fflush(out)) {
        fprintf(stderr, "no frames or output flush failed\n");
        goto done;
    }
    if (expect_status(
            "reuse after reset",
            rk_mpp_decoder_submit_packet(decoder, input, input_size, 0, 0,
                                         RK_MPP_PACKET_FLAG_EOS),
            RK_MPP_OK))
        goto done;
    for (;;) {
        int status = rk_mpp_decoder_receive_frame(decoder, &frame);

        if (status == RK_MPP_EOS)
            break;
        if (status != RK_MPP_OK) {
            fprintf(stderr, "reuse receive failed: %s\n",
                    rk_mpp_strerror(status));
            goto done;
        }
        reuse_frames++;
        rk_mpp_frame_release(decoder, &frame);
    }
    if (reuse_frames != frames) {
        fprintf(stderr, "reset reuse produced %llu frames, expected %llu\n",
                (unsigned long long)reuse_frames,
                (unsigned long long)frames);
        goto done;
    }
    printf("PASS frames=%llu bytes=%llu api=%u\n",
           (unsigned long long)frames, (unsigned long long)bytes_written,
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
