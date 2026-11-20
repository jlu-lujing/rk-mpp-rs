#define _GNU_SOURCE

#include "rk_mpp_rs.h"

#include <errno.h>
#include <fcntl.h>
#include <linux/dma-buf.h>
#include <linux/dma-heap.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

struct dma_input {
    int fd;
    void *mapping;
    size_t size;
};

static int dma_input_open(struct dma_input *input, size_t size) {
    struct dma_heap_allocation_data allocation = {
        .len = size,
        .fd_flags = O_RDWR | O_CLOEXEC,
    };
    int heap;

    memset(input, 0, sizeof(*input));
    input->fd = -1;
    heap = open("/dev/dma_heap/system", O_RDWR | O_CLOEXEC);
    if (heap < 0)
        return -1;
    if (ioctl(heap, DMA_HEAP_IOCTL_ALLOC, &allocation) < 0) {
        close(heap);
        return -1;
    }
    close(heap);
    input->fd = (int)allocation.fd;
    input->size = size;
    input->mapping = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED,
                          input->fd, 0);
    if (input->mapping == MAP_FAILED) {
        input->mapping = NULL;
        close(input->fd);
        input->fd = -1;
        return -1;
    }
    memset(input->mapping, 0, size);
    return 0;
}

static int dma_input_write(struct dma_input *input, const uint8_t *data,
                           size_t size) {
    struct dma_buf_sync sync = {
        .flags = DMA_BUF_SYNC_START | DMA_BUF_SYNC_WRITE,
    };

    if (size > input->size ||
        ioctl(input->fd, DMA_BUF_IOCTL_SYNC, &sync))
        return -1;
    memcpy(input->mapping, data, size);
    sync.flags = DMA_BUF_SYNC_END | DMA_BUF_SYNC_WRITE;
    return ioctl(input->fd, DMA_BUF_IOCTL_SYNC, &sync) ? -1 : 0;
}

static void dma_input_close(struct dma_input *input) {
    if (input->mapping)
        munmap(input->mapping, input->size);
    if (input->fd >= 0)
        close(input->fd);
    memset(input, 0, sizeof(*input));
    input->fd = -1;
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

static int configure_rate_control(rk_mpp_encoder_rate_control_t *rate_control,
                                  const char *mode_name) {
    const char *target = getenv("RATE_TARGET_BPS");
    const char *minimum = getenv("RATE_MIN_BPS");
    const char *maximum = getenv("RATE_MAX_BPS");
    const char *initial_qp = getenv("RATE_INITIAL_QP");
    const char *min_qp = getenv("RATE_MIN_QP");
    const char *max_qp = getenv("RATE_MAX_QP");
    uint32_t mode;

    if (!mode_name)
        mode_name = "fixqp";
    if (parse_rate_mode(mode_name, &mode) ||
        rk_mpp_encoder_rate_control_init(rate_control, mode) != RK_MPP_OK ||
        (mode != RK_MPP_RATE_CONTROL_FIXED_QP &&
         (!target || !minimum || !maximum))) {
        return -1;
    }
    if (target)
        rate_control->bps_target = strtoll(target, NULL, 0);
    if (minimum)
        rate_control->bps_min = strtoll(minimum, NULL, 0);
    if (maximum)
        rate_control->bps_max = strtoll(maximum, NULL, 0);
    if (initial_qp)
        rate_control->initial_qp = (uint32_t)strtoul(initial_qp, NULL, 0);
    if (min_qp)
        rate_control->min_qp = (uint32_t)strtoul(min_qp, NULL, 0);
    if (max_qp)
        rate_control->max_qp = (uint32_t)strtoul(max_qp, NULL, 0);
    return 0;
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
    rk_mpp_encoder_advanced_config_v3_t advanced_v3;
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
    struct dma_input dma = {.fd = -1};
    uint32_t dma_hor_stride = 0;
    uint32_t dma_ver_stride = 0;
    size_t dma_size = 0;
    const char *rate_mode_name = getenv("RATE_CONTROL_MODE");
    const char *control_mode = getenv("CONTROL_MODE");
    int use_dmabuf = getenv("DMABUF_INPUT") != NULL;

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
    if (control_mode) {
        int advanced_status;

        if (rk_mpp_encoder_advanced_config_v3_init(
                &advanced_v3, RK_MPP_RATE_CONTROL_FIXED_QP) != RK_MPP_OK ||
            configure_rate_control(&advanced_v3.rate_control,
                                   rate_mode_name)) {
            fprintf(stderr, "invalid V3 advanced config\n");
            goto done;
        }
        if (!strcmp(control_mode, "slice")) {
            const char *mode = getenv("SLICE_MODE");
            const char *argument = getenv("SLICE_ARG");
            const char *low_delay = getenv("SLICE_LOW_DELAY");
            uint32_t slice_mode = mode && !strcmp(mode, "byte")
                                      ? RK_MPP_SLICE_SPLIT_BY_BYTE
                                      : RK_MPP_SLICE_SPLIT_BY_CTU;
            uint32_t output_mode =
                low_delay && strtoul(low_delay, NULL, 0)
                    ? RK_MPP_SLICE_OUTPUT_LOW_DELAY_PACKETS
                    : RK_MPP_SLICE_OUTPUT_COMPLETE_ACCESS_UNIT;

            if (!argument ||
                rk_mpp_encoder_slice_split_init(
                    &advanced_v3.slice_split, slice_mode,
                    (uint32_t)strtoul(argument, NULL, 0),
                    output_mode) != RK_MPP_OK) {
                goto done;
            }
        } else if (!strcmp(control_mode, "roi")) {
            const char *x = getenv("ROI_X");
            const char *y = getenv("ROI_Y");
            const char *roi_width = getenv("ROI_WIDTH");
            const char *roi_height = getenv("ROI_HEIGHT");
            const char *roi_qp = getenv("ROI_QP");
            const char *force_intra = getenv("ROI_FORCE_INTRA");
            const char *absolute = getenv("ROI_ABSOLUTE_QP");
            rk_mpp_encoder_roi_region_t *roi = &advanced_v3.rois[0];

            if (!x || !y || !roi_width || !roi_height || !roi_qp ||
                rk_mpp_encoder_roi_region_init(roi) != RK_MPP_OK) {
                goto done;
            }
            roi->x = (int32_t)strtol(x, NULL, 0);
            roi->y = (int32_t)strtol(y, NULL, 0);
            roi->width = (int32_t)strtol(roi_width, NULL, 0);
            roi->height = (int32_t)strtol(roi_height, NULL, 0);
            roi->qp = (int32_t)strtol(roi_qp, NULL, 0);
            if (force_intra && strtoul(force_intra, NULL, 0))
                roi->flags |= RK_MPP_ROI_FLAG_FORCE_INTRA;
            if (absolute && strtoul(absolute, NULL, 0))
                roi->flags |= RK_MPP_ROI_FLAG_ABSOLUTE_QP;
            advanced_v3.roi_count = 1;
        } else if (!strcmp(control_mode, "gdr")) {
            const char *mode = getenv("GDR_MODE");
            const char *refresh = getenv("GDR_REFRESH");
            uint32_t gdr_mode = mode && !strcmp(mode, "col")
                                    ? RK_MPP_GDR_COL
                                    : RK_MPP_GDR_ROW;

            if (!refresh ||
                rk_mpp_encoder_gdr_init(
                    &advanced_v3.gdr, gdr_mode,
                    (uint32_t)strtoul(refresh, NULL, 0)) != RK_MPP_OK) {
                goto done;
            }
        } else if (!strcmp(control_mode, "h264-syntax")) {
            const char *profile = getenv("H264_PROFILE");
            const char *cabac = getenv("H264_CABAC");
            const char *transform = getenv("H264_TRANS8X8");
            const char *cabac_idc = getenv("H264_CABAC_IDC");
            const char *constraint = getenv("H264_CONSTRAINT_SET");
            const char *base_layer_pid = getenv("H264_BASE_LAYER_PID");
            const char *dblk_disable = getenv("DBLK_DISABLE");
            const char *dblk_alpha = getenv("DBLK_ALPHA");
            const char *dblk_beta = getenv("DBLK_BETA");
            rk_mpp_encoder_h264_syntax_t *syntax = &advanced_v3.h264_syntax;
            rk_mpp_encoder_h264_deblock_t *deblock =
                &advanced_v3.h264_deblock;
            uint32_t constraint_raw;

            if (codec != RK_MPP_CODEC_H264 ||
                rk_mpp_encoder_h264_syntax_init(syntax) != RK_MPP_OK ||
                rk_mpp_encoder_h264_deblock_init(deblock) != RK_MPP_OK) {
                goto done;
            }
            if (profile)
                config.profile = (uint32_t)strtoul(profile, NULL, 0);
            if (cabac) {
                syntax->flags |= RK_MPP_H264_SYNTAX_ENTROPY_CODING;
                syntax->entropy_coding =
                    (uint32_t)(strtoul(cabac, NULL, 0) != 0);
            }
            if (transform) {
                syntax->flags |= RK_MPP_H264_SYNTAX_TRANSFORM_8X8;
                syntax->transform_8x8 =
                    (uint32_t)(strtoul(transform, NULL, 0) != 0);
            }
            if (cabac_idc) {
                syntax->flags |= RK_MPP_H264_SYNTAX_CABAC_INIT_IDC;
                syntax->cabac_init_idc =
                    (uint32_t)strtoul(cabac_idc, NULL, 0);
            }
            if (constraint) {
                constraint_raw = (uint32_t)strtoul(constraint, NULL, 0);
                syntax->flags |= RK_MPP_H264_SYNTAX_CONSTRAINT_SET;
                syntax->constraint_set_force = constraint_raw >> 16;
                syntax->constraint_set_value = constraint_raw & 0x3fu;
            }
            if (base_layer_pid) {
                syntax->flags |= RK_MPP_H264_SYNTAX_BASE_LAYER_PID;
                syntax->base_layer_pid =
                    (uint32_t)strtoul(base_layer_pid, NULL, 0);
            }
            if (dblk_disable || dblk_alpha || dblk_beta) {
                deblock->present = 1;
                if (dblk_disable)
                    deblock->disable =
                        (uint32_t)strtoul(dblk_disable, NULL, 0);
                if (dblk_alpha)
                    deblock->alpha_offset_div2 =
                        (int32_t)strtol(dblk_alpha, NULL, 0);
                if (dblk_beta)
                    deblock->beta_offset_div2 =
                        (int32_t)strtol(dblk_beta, NULL, 0);
            }
            if (!syntax->flags && !deblock->present) {
                fprintf(stderr, "h264-syntax mode needs at least one option\n");
                goto done;
            }
        } else if (!strcmp(control_mode, "h265-syntax")) {
            const char *sao_luma = getenv("H265_SAO_LUMA_DISABLE");
            const char *sao_chroma = getenv("H265_SAO_CHROMA_DISABLE");
            const char *deblock_disable = getenv("H265_DBLK_DISABLE");
            const char *deblock_beta = getenv("H265_DBLK_BETA");
            const char *deblock_tc = getenv("H265_DBLK_TC");
            const char *lpf_slices = getenv("H265_LPF_ACS_SLI");
            const char *lpf_tiles_disable = getenv("H265_LPF_ACS_TILE_DISABLE");
            const char *auto_tile = getenv("H265_AUTO_TILE");
            rk_mpp_encoder_h265_syntax_t *syntax = &advanced_v3.h265_syntax;

            if (codec != RK_MPP_CODEC_H265 ||
                rk_mpp_encoder_h265_syntax_init(syntax) != RK_MPP_OK) {
                goto done;
            }
            if (sao_luma) {
                syntax->flags |= RK_MPP_H265_SYNTAX_SAO_LUMA_DISABLE;
                if (strtoul(sao_luma, NULL, 0))
                    syntax->values |= RK_MPP_H265_VALUE_SAO_LUMA_DISABLE;
            }
            if (sao_chroma) {
                syntax->flags |= RK_MPP_H265_SYNTAX_SAO_CHROMA_DISABLE;
                if (strtoul(sao_chroma, NULL, 0))
                    syntax->values |= RK_MPP_H265_VALUE_SAO_CHROMA_DISABLE;
            }
            if (deblock_disable) {
                syntax->flags |= RK_MPP_H265_SYNTAX_DEBLOCK_DISABLE;
                if (strtoul(deblock_disable, NULL, 0))
                    syntax->values |= RK_MPP_H265_VALUE_DEBLOCK_DISABLE;
            }
            if (deblock_beta) {
                syntax->flags |= RK_MPP_H265_SYNTAX_DEBLOCK_BETA;
                syntax->deblock_beta_offset_div2 =
                    (int32_t)strtol(deblock_beta, NULL, 0);
            }
            if (deblock_tc) {
                syntax->flags |= RK_MPP_H265_SYNTAX_DEBLOCK_TC;
                syntax->deblock_tc_offset_div2 =
                    (int32_t)strtol(deblock_tc, NULL, 0);
            }
            if (lpf_slices) {
                syntax->flags |=
                    RK_MPP_H265_SYNTAX_LOOP_FILTER_ACROSS_SLICES;
                if (strtoul(lpf_slices, NULL, 0))
                    syntax->values |=
                        RK_MPP_H265_VALUE_LOOP_FILTER_ACROSS_SLICES;
            }
            if (lpf_tiles_disable) {
                syntax->flags |=
                    RK_MPP_H265_SYNTAX_LOOP_FILTER_ACROSS_TILES;
                if (!strtoul(lpf_tiles_disable, NULL, 0))
                    syntax->values |=
                        RK_MPP_H265_VALUE_LOOP_FILTER_ACROSS_TILES;
            }
            if (auto_tile) {
                syntax->flags |= RK_MPP_H265_SYNTAX_AUTO_TILE;
                if (strtoul(auto_tile, NULL, 0))
                    syntax->values |= RK_MPP_H265_VALUE_AUTO_TILE;
            }
            if (!syntax->flags) {
                fprintf(stderr, "h265-syntax mode needs at least one option\n");
                goto done;
            }
        } else {
            fprintf(stderr, "unknown CONTROL_MODE=%s\n", control_mode);
            goto done;
        }
        advanced_status =
            rk_mpp_encoder_create_advanced_v3(&config, &advanced_v3, &encoder);
        if (advanced_status != RK_MPP_OK || !encoder) {
            fprintf(stderr, "V3 encoder create failed: %s\n",
                    rk_mpp_strerror(advanced_status));
            goto done;
        }
    } else if (rate_mode_name) {
        int advanced_status;

        if (parse_rate_mode(rate_mode_name, &rate_mode) ||
            rk_mpp_encoder_advanced_config_init(&advanced, rate_mode) !=
                RK_MPP_OK ||
            configure_rate_control(&advanced.rate_control, rate_mode_name)) {
            fprintf(stderr, "invalid rate-control environment\n");
            goto done;
        }
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
    if (use_dmabuf) {
        uint32_t width_align = codec == RK_MPP_CODEC_MJPEG ? 8u : 16u;
        uint32_t height_align = codec == RK_MPP_CODEC_MJPEG ? 2u : 16u;

        if (format != RK_MPP_PIXEL_FORMAT_NV12 ||
            width % width_align || height % height_align) {
            fprintf(stderr,
                    "DMABUF_INPUT needs aligned NV12 dimensions\n");
            goto done;
        }
        dma_hor_stride = width;
        dma_ver_stride = height;
        dma_size = (size_t)dma_hor_stride * dma_ver_stride * 3 / 2;
        if (dma_input_open(&dma, dma_size)) {
            fprintf(stderr, "dma-heap input allocation failed: %s\n",
                    strerror(errno));
            goto done;
        }
    }
    if (rk_mpp_encoded_packet_init(&packet) != RK_MPP_OK)
        goto done;
    out = fopen(argv[3], "wb");
    if (!out) {
        fprintf(stderr, "%s: %s\n", argv[3], strerror(errno));
        goto done;
    }
    for (index = 0; index < frames; index++) {
        int status;

        if (use_dmabuf) {
            if (dma_input_write(&dma, input + (size_t)index * frame_size,
                                frame_size)) {
                fprintf(stderr, "dma-heap input write failed\n");
                goto done;
            }
            status = rk_mpp_encoder_submit_frame_dmabuf(
                encoder, dma.fd, dma.size, dma_hor_stride, dma_ver_stride,
                index, 0);
        } else {
            status = rk_mpp_encoder_submit_frame(
                encoder, input + (size_t)index * frame_size, frame_size,
                index, 0);
        }
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
    dma_input_close(&dma);
    if (out)
        fclose(out);
    if (encoder)
        rk_mpp_encoder_close(encoder);
    free(input);
    return ret;
}
