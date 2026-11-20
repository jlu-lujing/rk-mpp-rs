/*
 * Drive the reference encoder's rate control directly, to read its state.
 *
 * The question this exists to answer: `rc_model_v2.c` keeps `i_sumbits`,
 * `p_sumbits` and the `i_scale` derived from them, and the bit target of every
 * picture is a function of those three. The Rust port reproduces four of six
 * captured `ctu_ebit` values and swaps two, which is not the shape of a unit or
 * scale error. Only the model's own prints can say where its state diverges, and
 * `mpi_enc_test` from this vintage prints nothing at all - not even its usage -
 * so the debug lines have to be coaxed out through a caller that does talk.
 *
 * Build on RK3588 against the reference build, not the installed library:
 *   cc -O2 -Wall -I/tmp/mpp-old/inc /tmp/rc_probe.c \
 *      /tmp/mpp-old/build/mpp/librockchip_mpp.so.0 -o /tmp/rc_probe
 * Run with the model's debug bit set:
 *   MPP_RC_DEBUG=0x40 LD_LIBRARY_PATH=/tmp/mpp-old/build/mpp \
 *     /tmp/rc_probe /tmp/fixture.nv12 /tmp/out.264 320 240 6 \
 *     300000 250000 350000
 *
 * Only mode, bit rate and GOP are configured - the same three `mpi_enc_test`
 * passes for `-rc 1 -bps a:b:c` - so the quality bounds stay at the library's
 * defaults and the trajectory is the one the harness produced.
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
    const char *output_path = argc > 2 ? argv[2] : "/tmp/rc_probe.264";
    int width = argc > 3 ? atoi(argv[3]) : 320;
    int height = argc > 4 ? atoi(argv[4]) : 240;
    int frames = argc > 5 ? atoi(argv[5]) : 6;
    int bps_target = argc > 6 ? atoi(argv[6]) : 300000;
    int bps_min = argc > 7 ? atoi(argv[7]) : 250000;
    int bps_max = argc > 8 ? atoi(argv[8]) : 350000;
    int mode = argc > 9 ? atoi(argv[9]) : 1; /* 0: VBR, 1: CBR, 2: AVBR, 3: FIXQP */
    int level = argc > 10 ? atoi(argv[10]) : 30;
    int mpi_quality = argc > 11 ? atoi(argv[11]) : 0;
    const char *hier = argc > 12 ? argv[12] : NULL;
    int hier_qp_delta[4] = { 0, 0, 0, 0 };
    int hier_frame_num[4] = { 0, 0, 0, 0 };
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
    FILE *input = NULL;
    FILE *output = NULL;
    MPP_RET ret = MPP_NOK;
    int i;

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

    CHECK(mpp_create(&ctx, &mpi));
    CHECK(mpp_init(ctx, MPP_CTX_ENC, MPP_VIDEO_CodingAVC));
    CHECK(mpp_enc_cfg_init(&cfg));
    CHECK(mpi->control(ctx, MPP_ENC_GET_CFG, cfg));

    CHECK(mpp_enc_cfg_set_s32(cfg, "codec:type", MPP_VIDEO_CodingAVC));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:width", width));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:height", height));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:hor_stride", hor_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:ver_stride", ver_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:format", MPP_FMT_YUV420SP));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:mode", mode == 1   ? MPP_ENC_RC_MODE_CBR
                                             : mode == 2 ? MPP_ENC_RC_MODE_AVBR
                                             : mode == 3 ? MPP_ENC_RC_MODE_FIXQP
                                                         : MPP_ENC_RC_MODE_VBR));
    if (mode == 3) {
        /* The same fixed-QP point the crate's matrix is built on. */
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_init", 26));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min", 26));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max", 26));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min_i", 26));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max_i", 26));
    } else if (mpi_quality) {
        /* `mpi_enc_test.c` leaves qp_init auto and widens each general/intra
         * band around MPP's library defaults. */
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_init", -1));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min", 10));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max", 51));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min_i", 10));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max_i", 51));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fqp_min_i", 10));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fqp_max_i", 51));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fqp_min_p", 10));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fqp_max_p", 51));
    }
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:bps_target", bps_target));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:bps_min", bps_min));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:bps_max", bps_max));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:gop",
                              getenv("RC_GOP") ? atoi(getenv("RC_GOP")) : 60));
    if (hier &&
        sscanf(hier, "%d:%d:%d:%d:%d:%d:%d:%d",
               &hier_qp_delta[0], &hier_qp_delta[1],
               &hier_qp_delta[2], &hier_qp_delta[3],
               &hier_frame_num[0], &hier_frame_num[1],
               &hier_frame_num[2], &hier_frame_num[3]) == 8) {
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:hier_qp_en", 1));
        CHECK(mpp_enc_cfg_set_st(cfg, "rc:hier_qp_delta", hier_qp_delta));
        CHECK(mpp_enc_cfg_set_st(cfg, "rc:hier_frame_num", hier_frame_num));
    } else if (hier) {
        fprintf(stderr, "bad hierarchical QP spec %s\n", hier);
        goto cleanup;
    }
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_flex", 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_num", 30));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_denom", 1));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_flex", 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_num", 30));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_denom", 1));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:profile",
                              getenv("H264_PROFILE") ?
                              atoi(getenv("H264_PROFILE")) : 66));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:level", level));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:cabac_en",
                              getenv("H264_CABAC") ?
                              atoi(getenv("H264_CABAC")) : 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:trans8x8", 0));
    if (getenv("MAX_REENC"))
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:max_reenc_times",
                                  atoi(getenv("MAX_REENC"))));
    if (getenv("SUPER_MODE")) {
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:super_mode",
                                  atoi(getenv("SUPER_MODE"))));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:super_i_thd",
                                  atoi(getenv("SUPER_I_THD"))));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:super_p_thd",
                                  atoi(getenv("SUPER_P_THD"))));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:priority",
                                  atoi(getenv("RC_PRIORITY"))));
    }
    if (getenv("DROP_MODE")) {
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:drop_mode",
                                  atoi(getenv("DROP_MODE"))));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:drop_thd",
                                  atoi(getenv("DROP_THD"))));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:drop_gap",
                                  atoi(getenv("DROP_GAP"))));
    }
    if (getenv("DEBREATH_EN")) {
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:debreath_en",
                                  atoi(getenv("DEBREATH_EN"))));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:debreath_strength",
                                  getenv("DEBREATH_STRENGTH") ?
                                  atoi(getenv("DEBREATH_STRENGTH")) : 0));
    }
    CHECK(mpi->control(ctx, MPP_ENC_SET_CFG, cfg));

    if (getenv("SMART_VI_LEN")) {
        int vi_len = atoi(getenv("SMART_VI_LEN"));
        int pos = 0;
        MppEncRefLtFrmCfg lt_ref;
        MppEncRefStFrmCfg st_ref[3];

        if (vi_len < 2) {
            fprintf(stderr, "SMART_VI_LEN must be at least 2\n");
            goto cleanup;
        }
        memset(&lt_ref, 0, sizeof(lt_ref));
        memset(st_ref, 0, sizeof(st_ref));
        lt_ref.lt_idx = 0;
        lt_ref.temporal_id = 0;
        lt_ref.ref_mode = REF_TO_PREV_LT_REF;
        lt_ref.lt_gap = 60;
        lt_ref.lt_delay = 0;

        st_ref[pos].is_non_ref = 0;
        st_ref[pos].temporal_id = 0;
        st_ref[pos].ref_mode = REF_TO_PREV_INTRA;
        pos++;
        if (vi_len > 1) {
            st_ref[pos].is_non_ref = 0;
            st_ref[pos].temporal_id = 0;
            st_ref[pos].ref_mode = REF_TO_PREV_REF_FRM;
            st_ref[pos].repeat = vi_len - 2;
            pos++;
        }
        st_ref[pos].is_non_ref = 0;
        st_ref[pos].temporal_id = 0;
        st_ref[pos].ref_mode = REF_TO_PREV_INTRA;
        pos++;

        CHECK(mpp_enc_ref_cfg_init(&ref_cfg));
        CHECK(mpp_enc_ref_cfg_set_cfg_cnt(ref_cfg, 1, 8));
        CHECK(mpp_enc_ref_cfg_add_lt_cfg(ref_cfg, 1, &lt_ref));
        CHECK(mpp_enc_ref_cfg_add_st_cfg(ref_cfg, pos, st_ref));
        CHECK(mpp_enc_ref_cfg_check(ref_cfg));
        CHECK(mpi->control(ctx, MPP_ENC_SET_REF_CFG, ref_cfg));
    }

    for (i = 0; i < frames; i++) {
        MppFrame frame = NULL;
        MppPacket packet = NULL;
        MppMeta meta = NULL;
        size_t read;

        mpp_buffer_sync_begin(frame_buf);
        read = fread(mpp_buffer_get_ptr(frame_buf), 1, frame_size, input);
        if (read != frame_size) {
            fprintf(stderr, "short read on frame %d: %zu of %zu\n", i, read,
                    frame_size);
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

        CHECK(mpi->encode_put_frame(ctx, frame));
        mpp_frame_deinit(&frame);
        CHECK(mpi->encode_get_packet(ctx, &packet));
        if (!packet) {
            fprintf(stderr, "no packet for frame %d\n", i);
            goto cleanup;
        }
        {
            void *data = mpp_packet_get_pos(packet);
            size_t len = mpp_packet_get_length(packet);
            fprintf(stderr, "picture %d: %zu bytes\n", i, len);
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
    if (group)
        mpp_buffer_group_put(group);
    return ret;
}
