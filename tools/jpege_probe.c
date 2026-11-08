/*
 * Drive the reference VEPU2 JPEG encoder with the low-delay slice split
 * settings that mpi_enc_test of the pinned 2024-04 vintage does not expose.
 *
 * Usage:
 *   jpege_probe in.raw out.jpg width height qfactor split_arg low_delay
 *               [mpp_input_format]
 *
 * `split_arg` is MPP_ENC_SPLIT_BY_CTU's MCU count. Zero disables the split.
 * `low_delay` sets base:low_delay and split:out=1. Packets are drained to EOI
 * on a worker thread so MPP's fixed output-task pool cannot deadlock a
 * synchronous put-then-get caller.
 * `mpp_input_format` defaults to MPP_FMT_YUV420SP. It accepts the 8-bit
 * NV12/NV21/I420, NV16/NV61/422P, four packed 4:2:2 orders, and NV24/444P
 * families used by the RK3576 VPU720 gate.
 */

#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "mpp_buffer.h"
#include "mpp_err.h"
#include "mpp_frame.h"
#include "mpp_meta.h"
#include "mpp_packet.h"
#include "rk_mpi.h"
#include "rk_venc_cfg.h"

#define CHECK(call)                                                             \
    do {                                                                        \
        MPP_RET check_ret = (call);                                            \
        if (check_ret) {                                                        \
            fprintf(stderr, "%s:%d: %s failed: %d\n", __FILE__, __LINE__,      \
                    #call, check_ret);                                          \
            goto cleanup;                                                       \
        }                                                                       \
    } while (0)

typedef struct PacketReaderCtx_t {
    MppCtx ctx;
    MppApi *mpi;
    FILE *output;
    int low_delay;
    MPP_RET ret;
    unsigned packets;
} PacketReaderCtx;

static int input_frame_factor(MppFrameFormat format, unsigned *numerator,
                              unsigned *denominator)
{
    switch (format) {
    case MPP_FMT_YUV420SP:
    case MPP_FMT_YUV420SP_VU:
    case MPP_FMT_YUV420P:
        *numerator = 3;
        *denominator = 2;
        return 0;
    case MPP_FMT_YUV422SP:
    case MPP_FMT_YUV422SP_VU:
    case MPP_FMT_YUV422P:
        *numerator = 2;
        *denominator = 1;
        return 0;
    case MPP_FMT_YUV444SP:
    case MPP_FMT_YUV444P:
        *numerator = 3;
        *denominator = 1;
        return 0;
    case MPP_FMT_YUV400:
        *numerator = 1;
        *denominator = 1;
        return 0;
    case MPP_FMT_YUV422_YUYV:
    case MPP_FMT_YUV422_YVYU:
    case MPP_FMT_YUV422_UYVY:
    case MPP_FMT_YUV422_VYUY:
        *numerator = 1;
        *denominator = 1;
        return 0;
    default:
        return -1;
    }
}

static void *read_packets(void *opaque)
{
    PacketReaderCtx *reader = (PacketReaderCtx *)opaque;
    unsigned eoi = 0;

    reader->ret = MPP_NOK;
    reader->packets = 0;
    do {
        MppPacket packet = NULL;
        void *data;
        size_t len;
        MPP_RET ret;

        do {
            ret = reader->mpi->encode_get_packet(reader->ctx, &packet);
            if (ret || packet)
                break;
            usleep(1000);
        } while (1);

        if (ret || !packet) {
            fprintf(stderr, "no packet part %u: ret=%d\n",
                    reader->packets, ret);
            return NULL;
        }

        data = mpp_packet_get_pos(packet);
        len = mpp_packet_get_length(packet);
        eoi = mpp_packet_is_eoi(packet);
        fprintf(stderr, "picture 0 packet %u: %zu bytes eoi=%u\n",
                reader->packets, len, eoi);
        if (data && len)
            fwrite(data, 1, len, reader->output);

        mpp_packet_deinit(&packet);
        reader->packets++;
    } while (reader->low_delay && !eoi);

    reader->ret = MPP_OK;
    return NULL;
}

int main(int argc, char **argv)
{
    const char *input_path = argc > 1 ? argv[1] : "";
    const char *output_path = argc > 2 ? argv[2] : "/tmp/jpege_probe.jpg";
    int width = argc > 3 ? atoi(argv[3]) : 320;
    int height = argc > 4 ? atoi(argv[4]) : 240;
    int qfactor = argc > 5 ? atoi(argv[5]) : 50;
    int split_arg = argc > 6 ? atoi(argv[6]) : 0;
    int low_delay = argc > 7 ? atoi(argv[7]) : 0;
    MppFrameFormat input_format =
        argc > 8 ? (MppFrameFormat)atoi(argv[8]) : MPP_FMT_YUV420SP;
    int packed_yuv422 = input_format == MPP_FMT_YUV422_YUYV ||
                        input_format == MPP_FMT_YUV422_YVYU ||
                        input_format == MPP_FMT_YUV422_UYVY ||
                        input_format == MPP_FMT_YUV422_VYUY;
    int bytes_per_pixel = packed_yuv422 ? 2 : 1;
    int hor_stride = (width * bytes_per_pixel + 15) & ~15;
    int ver_stride = (height + 15) & ~15;
    unsigned frame_numerator;
    unsigned frame_denominator;
    size_t frame_size;
    size_t packet_size = (size_t)width * height * 3 + 64 * 1024;

    MppCtx ctx = NULL;
    MppApi *mpi = NULL;
    MppEncCfg cfg = NULL;
    MppBufferGroup group = NULL;
    MppBuffer frame_buf = NULL;
    MppBuffer packet_buf = NULL;
    MppFrame frame = NULL;
    MppPacket packet = NULL;
    MppMeta meta = NULL;
    PacketReaderCtx reader;
    pthread_t reader_thread;
    int reader_started = 0;
    FILE *input = NULL;
    FILE *output = NULL;
    MPP_RET ret = MPP_NOK;
    MPP_RET put_ret;

    if (input_frame_factor(input_format, &frame_numerator,
                           &frame_denominator)) {
        fprintf(stderr, "unsupported input format %d\n", input_format);
        return 2;
    }
    frame_size = (size_t)hor_stride * ver_stride * frame_numerator /
                 frame_denominator;

    if (!input_path[0] || !output_path[0] ||
        width < 16 || height < 16 || (width & 1) || (height & 1)) {
        fprintf(stderr,
                "usage: jpege_probe in.raw out.jpg width height "
                "qfactor split_arg low_delay [mpp_input_format]\n");
        return 2;
    }

    input = fopen(input_path, "rb");
    if (!input) {
        fprintf(stderr, "open %s: %s\n", input_path, strerror(errno));
        return 2;
    }
    output = fopen(output_path, "wb");
    if (!output) {
        fprintf(stderr, "open %s: %s\n", output_path, strerror(errno));
        fclose(input);
        return 2;
    }

    CHECK(mpp_buffer_group_get_internal(
        &group, (MppBufferType)(MPP_BUFFER_TYPE_DMA_HEAP |
                                MPP_BUFFER_FLAGS_CACHABLE)));
    CHECK(mpp_buffer_get(group, &frame_buf, frame_size));
    CHECK(mpp_buffer_get(group, &packet_buf, packet_size));

    mpp_buffer_sync_begin(frame_buf);
    if (fread(mpp_buffer_get_ptr(frame_buf), 1, frame_size, input) != frame_size) {
        fprintf(stderr, "short input, expected %zu bytes\n", frame_size);
        goto cleanup;
    }
    mpp_buffer_sync_end(frame_buf);

    CHECK(mpp_create(&ctx, &mpi));
    CHECK(mpp_init(ctx, MPP_CTX_ENC, MPP_VIDEO_CodingMJPEG));
    CHECK(mpp_enc_cfg_init(&cfg));
    CHECK(mpi->control(ctx, MPP_ENC_GET_CFG, cfg));

    CHECK(mpp_enc_cfg_set_s32(cfg, "codec:type", MPP_VIDEO_CodingMJPEG));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:width", width));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:height", height));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:hor_stride", hor_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:ver_stride", ver_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:format", input_format));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:mode", MPP_ENC_RC_MODE_FIXQP));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:gop", 60));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_flex", 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_num", 30));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_denom", 1));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_flex", 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_num", 30));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_denom", 1));
    CHECK(mpp_enc_cfg_set_s32(cfg, "jpeg:q_factor", qfactor));
    CHECK(mpp_enc_cfg_set_s32(cfg, "jpeg:qf_max", 99));
    CHECK(mpp_enc_cfg_set_s32(cfg, "jpeg:qf_min", 1));
    if (split_arg > 0) {
        CHECK(mpp_enc_cfg_set_s32(cfg, "split:mode", MPP_ENC_SPLIT_BY_CTU));
        CHECK(mpp_enc_cfg_set_s32(cfg, "split:arg", split_arg));
        CHECK(mpp_enc_cfg_set_s32(cfg, "split:out", low_delay ? 1 : 0));
    }
    if (low_delay) {
        CHECK(mpp_enc_cfg_set_s32(cfg, "base:low_delay", 1));
    }
    CHECK(mpi->control(ctx, MPP_ENC_SET_CFG, cfg));

    CHECK(mpp_frame_init(&frame));
    mpp_frame_set_width(frame, width);
    mpp_frame_set_height(frame, height);
    mpp_frame_set_hor_stride(frame, hor_stride);
    mpp_frame_set_ver_stride(frame, ver_stride);
    mpp_frame_set_fmt(frame, input_format);
    mpp_frame_set_buffer(frame, frame_buf);

    meta = mpp_frame_get_meta(frame);
    CHECK(mpp_packet_init_with_buffer(&packet, packet_buf));
    mpp_packet_set_length(packet, 0);
    mpp_meta_set_packet(meta, KEY_OUTPUT_PACKET, packet);
    packet = NULL;

    reader.ctx = ctx;
    reader.mpi = mpi;
    reader.output = output;
    reader.low_delay = low_delay;
    reader.ret = MPP_NOK;
    reader.packets = 0;
    if (low_delay) {
        int thread_ret = pthread_create(&reader_thread, NULL, read_packets,
                                        &reader);
        if (thread_ret) {
            fprintf(stderr, "pthread_create failed: %d\n", thread_ret);
            goto cleanup;
        }
        reader_started = 1;
    }

    put_ret = mpi->encode_put_frame(ctx, frame);
    mpp_frame_deinit(&frame);
    if (reader_started) {
        int thread_ret = pthread_join(reader_thread, NULL);
        if (thread_ret) {
            fprintf(stderr, "pthread_join failed: %d\n", thread_ret);
            goto cleanup;
        }
    } else {
        read_packets(&reader);
    }
    if (put_ret) {
        fprintf(stderr, "encode_put_frame failed: %d\n", put_ret);
        goto cleanup;
    }
    if (reader.ret) {
        fprintf(stderr, "packet reader failed: %d\n", reader.ret);
        goto cleanup;
    }
    ret = MPP_OK;

cleanup:
    if (packet)
        mpp_packet_deinit(&packet);
    if (frame)
        mpp_frame_deinit(&frame);
    if (output)
        fclose(output);
    if (input)
        fclose(input);
    if (ctx)
        mpp_destroy(ctx);
    if (cfg)
        mpp_enc_cfg_deinit(cfg);
    if (frame_buf)
        mpp_buffer_put(frame_buf);
    if (packet_buf)
        mpp_buffer_put(packet_buf);
    if (group)
        mpp_buffer_group_put(group);
    return ret;
}
