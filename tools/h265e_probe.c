/*
 * Drive the reference H.265 encoder with a fixed-QP single IDR, the smallest cell
 * M8 compares. It exists alongside mpi_enc_test -t 9 -rc 2 because the harness
 * hard-codes profile and level in this vintage, and the golden this crate is
 * checked against has to be produced by a caller that leaves them at the library
 * defaults h265e_init sets (Main / level 120 / tier 0 / max_cu 64 / tiles off),
 * not by a harness that rewrites them after -cfg.
 *
 * Build on t6 against the reference build, not the installed library:
 *   cc -O2 -w -I$MPP/inc /tmp/h265e_probe.c \
 *      $MPP/build/mpp/librockchip_mpp.so.1 -o /tmp/h265e_probe
 *
 * Usage:
 *   h265e_probe in.nv12 out.265 w h <0|1> qp [bps[:min:max]]
 *     mode 0 = fixed QP, 1 = CBR. qp is written to rc:qp_init.
 *
 * FRAMES/GOP enable multi-picture runs. REF_CFG=1 installs a short-term ref
 * config so C's scheduler reaches the GDR refresh path; REFRESH_EN/MODE/NUM
 * configure the refresh keys.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mpp_buffer.h"
#include "mpp_err.h"
#include "mpp_frame.h"
#include "mpp_packet.h"
#include "rk_mpi.h"
#include "rk_venc_cfg.h"

#define CHECK(call)                                                             \
    do {                                                                        \
        MPP_RET check_ret = (call);                                            \
        if (check_ret) {                                                        \
            fprintf(stderr, "%s:%d: %s failed: %d\n", __FILE__, __LINE__,      \
                    #call, check_ret);                                         \
            goto cleanup;                                                       \
        }                                                                       \
    } while (0)

int main(int argc, char **argv)
{
    const char *input_path = argc > 1 ? argv[1] : "";
    const char *output_path = argc > 2 ? argv[2] : "/tmp/h265e_probe.265";
    int width = argc > 3 ? atoi(argv[3]) : 320;
    int height = argc > 4 ? atoi(argv[4]) : 240;
    int mode = argc > 5 ? atoi(argv[5]) : 0; /* 0 fixqp 1 cbr */
    int qp = argc > 6 ? atoi(argv[6]) : 26;
    int bps_target = 300000, bps_min = 250000, bps_max = 350000;
    int hor_stride = (width + 15) & ~15;
    int ver_stride = (height + 15) & ~15;
    size_t frame_size = (size_t)hor_stride * ver_stride * 3 / 2;

    MppCtx ctx = NULL;
    MppApi *mpi = NULL;
    MppEncCfg cfg = NULL;
    MppEncRefCfg ref_cfg = NULL;
    MppBufferGroup group = NULL;
    MppBuffer frame_buf = NULL;
    MppBuffer packet_buf = NULL;
    MppBuffer md_info = NULL;
    FILE *input = NULL;
    FILE *output = NULL;
    MPP_RET ret = MPP_NOK;

    if (argc > 7 && argv[7][0] >= '0' && argv[7][0] <= '9') {
        int parts = sscanf(argv[7], "%d:%d:%d", &bps_target, &bps_min, &bps_max);
        if (parts < 1) {
            fprintf(stderr, "bad bps %s\n", argv[7]);
            return 2;
        }
        if (parts == 1) {
            bps_min = bps_target * 3 / 4;
            bps_max = bps_target * 5 / 4;
        }
    }

    input = fopen(input_path, "rb");
    if (!input) {
        fprintf(stderr, "open %s: %s\n", input_path, strerror(errno));
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
    CHECK(mpp_buffer_get(group, &packet_buf, frame_size + 4096));
    CHECK(mpp_buffer_get(group, &md_info,
                         (hor_stride + 63) / 64 * ((ver_stride + 63) / 64) * 32));

    CHECK(mpp_create(&ctx, &mpi));
    CHECK(mpp_init(ctx, MPP_CTX_ENC, MPP_VIDEO_CodingHEVC));
    CHECK(mpp_enc_cfg_init(&cfg));
    CHECK(mpi->control(ctx, MPP_ENC_GET_CFG, cfg));

    CHECK(mpp_enc_cfg_set_s32(cfg, "codec:type", MPP_VIDEO_CodingHEVC));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:width", width));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:height", height));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:hor_stride", hor_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:ver_stride", ver_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:format", MPP_FMT_YUV420SP));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:mode",
                              mode == 0 ? MPP_ENC_RC_MODE_FIXQP
                                        : MPP_ENC_RC_MODE_CBR));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:bps_target", bps_target));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:bps_min", bps_min));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:bps_max", bps_max));
    if (mode == 0) {
        /* mpi_enc_test -rc 2 collapses every bound onto the QP; the reference's
         * own defaults are 10..51, so the fixed-QP golden has to set them too or
         * it compares our collapsed line against the default one. */
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_init", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min_i", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max_i", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_ip", 0));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fqp_min_i", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fqp_max_i", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fqp_min_p", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fqp_max_p", qp));
    } else {
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_init", qp));
    }
    int gop = getenv("GOP") ? atoi(getenv("GOP")) : 60;
    int frames = getenv("FRAMES") ? atoi(getenv("FRAMES")) : 1;
    if (frames < 1) frames = 1;
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:gop", gop));
    if (getenv("QP_DELTA"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_ip", atoi(getenv("QP_DELTA"))));
    if (getenv("REFRESH_LEN"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:refresh_length", atoi(getenv("REFRESH_LEN"))));
    if (getenv("REFRESH_EN")) {
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:refresh_en", 1));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:refresh_mode",
                                  getenv("REFRESH_MODE") ? atoi(getenv("REFRESH_MODE")) : 0));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:refresh_num",
                                  getenv("REFRESH_NUM") ? atoi(getenv("REFRESH_NUM")) : 1));
    }
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_flex", 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_num", 30));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_denom", 1));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_flex", 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_num", 30));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_denom", 1));
    /* Leave h265:profile / h265:level at the library defaults on purpose: this is
     * the parameter the crate's writer derives, not one it copies, so the golden
     * must come from the same defaults. */
    CHECK(mpi->control(ctx, MPP_ENC_SET_CFG, cfg));

    if (getenv("REF_CFG")) {
        MppEncRefStFrmCfg st_ref;
        memset(&st_ref, 0, sizeof(st_ref));
        st_ref.is_non_ref = 0;
        st_ref.temporal_id = 0;
        st_ref.ref_mode = REF_TO_PREV_REF_FRM;
        st_ref.ref_arg = 0;
        st_ref.repeat = frames > 1 ? frames - 1 : 0;

        CHECK(mpp_enc_ref_cfg_init(&ref_cfg));
        CHECK(mpp_enc_ref_cfg_set_cfg_cnt(ref_cfg, 0, 1));
        CHECK(mpp_enc_ref_cfg_add_st_cfg(ref_cfg, 1, &st_ref));
        CHECK(mpp_enc_ref_cfg_check(ref_cfg));
        CHECK(mpi->control(ctx, MPP_ENC_SET_REF_CFG, ref_cfg));
    }

    for (int picture = 0; picture < frames; picture++) {
        MppFrame frame = NULL;
        MppPacket packet = NULL;
        MppMeta meta = NULL;
        size_t read;

        mpp_buffer_sync_begin(frame_buf);
        read = fread(mpp_buffer_get_ptr(frame_buf), 1, frame_size, input);
        if (read != frame_size) {
            fprintf(stderr, "short read: %zu of %zu\n", read, frame_size);
            goto cleanup;
        }
        mpp_buffer_sync_end(frame_buf);

        CHECK(mpp_frame_init(&frame));
        mpp_frame_set_width(frame, width);
        mpp_frame_set_height(frame, height);
        mpp_frame_set_hor_stride(frame, hor_stride);
        mpp_frame_set_ver_stride(frame, ver_stride);
        mpp_frame_set_fmt(frame, MPP_FMT_YUV420SP);
        mpp_frame_set_buffer(frame, frame_buf);

        meta = mpp_frame_get_meta(frame);
        CHECK(mpp_packet_init_with_buffer(&packet, packet_buf));
        mpp_packet_set_length(packet, 0);
        mpp_meta_set_packet(meta, KEY_OUTPUT_PACKET, packet);
        mpp_meta_set_buffer(meta, KEY_MOTION_INFO, md_info);

        CHECK(mpi->encode_put_frame(ctx, frame));
        mpp_frame_deinit(&frame);
        CHECK(mpi->encode_get_packet(ctx, &packet));
        if (!packet) {
            fprintf(stderr, "no packet\n");
            goto cleanup;
        }
        {
            void *data = mpp_packet_get_pos(packet);
            size_t len = mpp_packet_get_length(packet);
            fprintf(stderr, "picture %d: %zu bytes\n", picture, len);
            if (data && len)
                fwrite(data, 1, len, output);
        }
        mpp_packet_deinit(&packet);
    }
    ret = MPP_OK;

cleanup:
    if (output)
        fclose(output);
    if (input)
        fclose(input);
    if (ctx)
        mpp_destroy(ctx);
    if (cfg)
        mpp_enc_cfg_deinit(cfg);
    if (ref_cfg)
        mpp_enc_ref_cfg_deinit(&ref_cfg);
    if (frame_buf)
        mpp_buffer_put(frame_buf);
    if (packet_buf)
        mpp_buffer_put(packet_buf);
    if (md_info)
        mpp_buffer_put(md_info);
    if (group)
        mpp_buffer_group_put(group);
    return ret;
}
