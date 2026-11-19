/*
 * Minimal official-MPP H.264 encoder used only to produce board-local
 * golden streams and register dumps for the Rust implementation.
 *
 * Build on t6:
 *   cc -O0 -g -Wall -Wextra -o /tmp/h264_enc_probe h264_enc_probe.c \
 *      $(pkg-config --cflags --libs rockchip_mpp)
 *
 * Usage:
 *   h264_enc_probe <output.264> [width] [height] [profile]
 */

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <rockchip/mpp_buffer.h>
#include <rockchip/mpp_err.h>
#include <rockchip/mpp_frame.h>
#include <rockchip/mpp_meta.h>
#include <rockchip/mpp_packet.h>
#include <rockchip/rk_mpi.h>
#include <rockchip/rk_venc_cfg.h>

#define CHECK(call)                                                           \
    do {                                                                      \
        MPP_RET check_ret = (call);                                           \
        if (check_ret) {                                                      \
            fprintf(stderr, "%s:%d: %s failed: %d\n", __FILE__, __LINE__,    \
                    #call, check_ret);                                        \
            goto cleanup;                                                     \
        }                                                                     \
    } while (0)

static void fill_nv12(uint8_t *pixels, int width, int height, int hor_stride,
                      int ver_stride)
{
    memset(pixels, 16, (size_t)hor_stride * ver_stride);
    for (int y = 0; y < height; ++y) {
        for (int x = 0; x < width; ++x) {
            int value = 16 + (x * 200 / (width - 1) +
                              y * 200 / (height - 1)) /
                                 2;
            pixels[(size_t)y * hor_stride + x] = (uint8_t)value;
        }
    }

    uint8_t *uv = pixels + (size_t)hor_stride * ver_stride;
    memset(uv, 128, (size_t)hor_stride * ver_stride / 2);
    for (int y = 0; y < height / 2; ++y) {
        for (int x = 0; x < width / 2; ++x) {
            size_t offset = (size_t)y * hor_stride + x * 2;
            uv[offset] = (uint8_t)(48 + x * 128 / (width / 2 - 1));
            uv[offset + 1] = (uint8_t)(208 - y * 128 / (height / 2 - 1));
        }
    }
}

static void fill_i420(uint8_t *pixels, int width, int height, int hor_stride,
                      int ver_stride)
{
    memset(pixels, 16, (size_t)hor_stride * ver_stride);
    for (int y = 0; y < height; ++y) {
        for (int x = 0; x < width; ++x) {
            int value = 16 + (x * 200 / (width - 1) +
                              y * 200 / (height - 1)) /
                                 2;
            pixels[(size_t)y * hor_stride + x] = (uint8_t)value;
        }
    }

    size_t luma_size = (size_t)hor_stride * ver_stride;
    size_t chroma_plane = luma_size / 4;
    uint8_t *u = pixels + luma_size;
    uint8_t *v = u + chroma_plane;
    memset(u, 128, chroma_plane * 2);
    for (int y = 0; y < height / 2; ++y) {
        for (int x = 0; x < width / 2; ++x) {
            u[(size_t)y * (hor_stride / 2) + x] =
                (uint8_t)(48 + x * 128 / (width / 2 - 1));
            v[(size_t)y * (hor_stride / 2) + x] =
                (uint8_t)(208 - y * 128 / (height / 2 - 1));
        }
    }
}

int main(int argc, char **argv)
{
    const char *output_path = argc > 1 ? argv[1] : "/tmp/h264_golden.264";
    int width = argc > 2 ? atoi(argv[2]) : 320;
    int height = argc > 3 ? atoi(argv[3]) : 240;
    int profile = argc > 4 ? atoi(argv[4]) : 66;
    int hor_stride = (width + 15) & ~15;
    int ver_stride = (height + 15) & ~15;
    int input_format = getenv("MPP_INPUT_FORMAT")
                           ? atoi(getenv("MPP_INPUT_FORMAT"))
                           : MPP_FMT_YUV420SP;
    size_t frame_size = (size_t)hor_stride * ver_stride * 3 / 2;
    size_t packet_size = frame_size + 4096;

    MppCtx ctx = NULL;
    MppApi *mpi = NULL;
    MppEncCfg cfg = NULL;
    MppBufferGroup group = NULL;
    MppBuffer frame_buf = NULL;
    MppBuffer packet_buf = NULL;
    MppFrame frame = NULL;
    MppPacket packet = NULL;
    MppMeta meta = NULL;
    FILE *output = NULL;
    MPP_RET ret = MPP_NOK;
    int cabac = profile >= 77 ? 1 : 0;
    int trans8x8 = profile >= 100 ? 1 : 0;

    if (width < 16 || height < 16 || (width & 1) || (height & 1)) {
        fprintf(stderr, "invalid dimensions %dx%d\n", width, height);
        return 2;
    }

    output = fopen(output_path, "wb");
    if (!output) {
        fprintf(stderr, "open %s: %s\n", output_path, strerror(errno));
        return 2;
    }

    CHECK(mpp_buffer_group_get_internal(
        &group, (MppBufferType)(MPP_BUFFER_TYPE_DMA_HEAP |
                                MPP_BUFFER_FLAGS_CACHABLE)));
    CHECK(mpp_buffer_get(group, &frame_buf, frame_size));
    CHECK(mpp_buffer_get(group, &packet_buf, packet_size));

    mpp_buffer_sync_begin(frame_buf);
    if (input_format == MPP_FMT_YUV420P)
        fill_i420((uint8_t *)mpp_buffer_get_ptr(frame_buf), width, height,
                  hor_stride, ver_stride);
    else
        fill_nv12((uint8_t *)mpp_buffer_get_ptr(frame_buf), width, height,
                  hor_stride, ver_stride);
    mpp_buffer_sync_end(frame_buf);

    {
        const char *dump_path = getenv("H264_PROBE_DUMP_NV12");
        if (dump_path) {
            FILE *dump = fopen(dump_path, "wb");
            if (!dump ||
                fwrite(mpp_buffer_get_ptr(frame_buf), 1, frame_size, dump) !=
                    frame_size) {
                fprintf(stderr, "dump NV12 to %s failed\n", dump_path);
                if (dump)
                    fclose(dump);
                goto cleanup;
            }
            fclose(dump);
        }
    }

    CHECK(mpp_create(&ctx, &mpi));
    CHECK(mpp_init(ctx, MPP_CTX_ENC, MPP_VIDEO_CodingAVC));
    CHECK(mpp_enc_cfg_init(&cfg));
    CHECK(mpi->control(ctx, MPP_ENC_GET_CFG, cfg));

    CHECK(mpp_enc_cfg_set_s32(cfg, "codec:type", MPP_VIDEO_CodingAVC));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:width", width));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:height", height));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:hor_stride", hor_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:ver_stride", ver_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:format", input_format));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:mode", MPP_ENC_RC_MODE_FIXQP));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:gop", 1));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_init", 26));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min", 26));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max", 26));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min_i", 26));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max_i", 26));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:profile", profile));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:level", 30));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:cabac_en", cabac));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:cabac_idc", 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:trans8x8", trans8x8));
    CHECK(mpi->control(ctx, MPP_ENC_SET_CFG, cfg));

    CHECK(mpp_packet_init_with_buffer(&packet, packet_buf));
    mpp_packet_set_length(packet, 0);
    CHECK(mpi->control(ctx, MPP_ENC_GET_HDR_SYNC, packet));
    {
        void *data = mpp_packet_get_pos(packet);
        size_t len = mpp_packet_get_length(packet);
        if (!data || !len || fwrite(data, 1, len, output) != len)
            goto cleanup;
        fprintf(stderr, "header=%zu bytes\n", len);
    }
    mpp_packet_deinit(&packet);

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

    CHECK(mpi->encode_put_frame(ctx, frame));
    mpp_frame_deinit(&frame);
    CHECK(mpi->encode_get_packet(ctx, &packet));
    if (!packet)
        goto cleanup;
    {
        void *data = mpp_packet_get_pos(packet);
        size_t len = mpp_packet_get_length(packet);
        if (!data || !len || fwrite(data, 1, len, output) != len)
            goto cleanup;
        fprintf(stderr, "frame=%zu bytes\n", len);
    }
    mpp_packet_deinit(&packet);
    ret = MPP_OK;

cleanup:
    if (packet)
        mpp_packet_deinit(&packet);
    if (frame)
        mpp_frame_deinit(&frame);
    if (output)
        fclose(output);
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
