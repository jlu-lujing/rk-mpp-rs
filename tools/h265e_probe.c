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
 *   h265e_probe in.nv12 out.265 w h <0|1|2|3> qp [bps[:min:max]]
 *               [split mode arg] [splitout value] [roi x y w h intra qpmode qp]...
 *     mode 0 = fixed QP, 1 = CBR, 2 = VBR, 3 = AVBR. qp is written to
 *     rc:qp_init for the rate-controlled modes.
 *   `split 1|2 arg` is MPP_ENC_SPLIT_BY_BYTE with a byte cap or
 *   MPP_ENC_SPLIT_BY_CTU with 64x64 CTUs per slice. `splitout 1` enables
 *   MPP_ENC_SPLIT_OUT_LOWDELAY; packets are then drained through EOI.
 *   Build with -DH265E_ROI and link mpp_enc_roi_utils.c to enable the ROI
 *   arguments using the reference helper's VEPU580 H.265 table generation.
 *
 * FRAMES/GOP enable multi-picture runs. REF_CFG=1 installs a short-term ref
 * config so C's scheduler reaches the GDR refresh path; REFRESH_EN/MODE/NUM
 * configure the refresh keys.
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
#include "mpp_packet.h"
#include "rk_mpi.h"
#include "rk_venc_cfg.h"

#ifdef H265E_ROI
#include "mpp_enc_roi_utils.h"
#endif

#define CHECK(call)                                                             \
    do {                                                                        \
        MPP_RET check_ret = (call);                                            \
        if (check_ret) {                                                        \
            fprintf(stderr, "%s:%d: %s failed: %d\n", __FILE__, __LINE__,      \
                    #call, check_ret);                                         \
            goto cleanup;                                                       \
        }                                                                       \
    } while (0)

typedef struct PacketReaderCtx_t {
    MppCtx ctx;
    MppApi *mpi;
    FILE *output;
    int frame;
    int low_delay;
    MPP_RET ret;
    unsigned packets;
} PacketReaderCtx;

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
            fprintf(stderr, "no packet for frame %d part %u: ret=%d\n",
                    reader->frame, reader->packets, ret);
            return NULL;
        }

        data = mpp_packet_get_pos(packet);
        len = mpp_packet_get_length(packet);
        eoi = mpp_packet_is_eoi(packet);
        fprintf(stderr, "picture %d packet %u: %zu bytes eoi=%u\n",
                reader->frame, reader->packets, len, eoi);
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
    const char *output_path = argc > 2 ? argv[2] : "/tmp/h265e_probe.265";
    int width = argc > 3 ? atoi(argv[3]) : 320;
    int height = argc > 4 ? atoi(argv[4]) : 240;
    int mode = argc > 5 ? atoi(argv[5]) : 0; /* 0 fixqp 1 cbr 2 vbr 3 avbr */
    int qp = argc > 6 ? atoi(argv[6]) : 26;
    int bps_target = 300000, bps_min = 250000, bps_max = 350000;
    int have_split = 0, split_mode = 0, split_arg = 0, split_out = 0;
    int bit_depth = getenv("BIT_DEPTH") ? atoi(getenv("BIT_DEPTH")) : 8;
    MppFrameFormat input_format =
        bit_depth == 10 ? MPP_FMT_YUV420SP_10BIT : MPP_FMT_YUV420SP;
    int hor_stride = (width + 15) & ~15;
    int ver_stride = (height + 15) & ~15;
    size_t frame_size =
        (size_t)hor_stride * ver_stride * 3 / 2 * (bit_depth > 8 ? 2 : 1);

    MppCtx ctx = NULL;
    MppApi *mpi = NULL;
    MppEncCfg cfg = NULL;
    MppEncRefCfg ref_cfg = NULL;
    MppBufferGroup group = NULL;
    MppBuffer frame_buf = NULL;
    MppBuffer packet_buf = NULL;
    MppBuffer md_info = NULL;
#ifdef H265E_ROI
    MppEncRoiCtx roi_ctx = NULL;
    int roi_regions = 0;
#endif
    FILE *input = NULL;
    FILE *output = NULL;
    MPP_RET ret = MPP_NOK;

    int arg = 7;
    if (arg < argc && argv[arg][0] >= '0' && argv[arg][0] <= '9') {
        int parts = sscanf(argv[arg], "%d:%d:%d", &bps_target, &bps_min, &bps_max);
        if (parts < 1) {
            fprintf(stderr, "bad bps %s\n", argv[arg]);
            return 2;
        }
        if (parts == 1) {
            bps_min = bps_target * 3 / 4;
            bps_max = bps_target * 5 / 4;
        }
        arg++;
    }
    for (; arg < argc; arg++) {
        if (!strcmp(argv[arg], "split")) {
            if (arg + 2 >= argc) {
                fprintf(stderr, "split wants mode arg\n");
                return 2;
            }
            have_split = 1;
            split_mode = atoi(argv[++arg]);
            split_arg = atoi(argv[++arg]);
        } else if (!strcmp(argv[arg], "splitout")) {
            if (arg + 1 >= argc) {
                fprintf(stderr, "splitout wants value\n");
                return 2;
            }
            split_out = atoi(argv[++arg]);
#ifdef H265E_ROI
        } else if (!strcmp(argv[arg], "roi")) {
            roi_regions++;
            if (arg + 7 >= argc) {
                fprintf(stderr, "roi wants x y w h fi qpmode qpval\n");
                return 2;
            }
            arg += 7;
#endif
        } else {
            fprintf(stderr, "unknown option %s\n", argv[arg]);
            return 2;
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
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:format", input_format));
    CHECK(mpp_enc_cfg_set_s32(
        cfg, "rc:mode",
        mode == 0 ? MPP_ENC_RC_MODE_FIXQP
                  : (mode == 1 ? MPP_ENC_RC_MODE_CBR
                               : (mode == 2 ? MPP_ENC_RC_MODE_VBR
                                            : MPP_ENC_RC_MODE_AVBR))));
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
    if (have_split) {
        CHECK(mpp_enc_cfg_set_s32(cfg, "split:mode", split_mode));
        CHECK(mpp_enc_cfg_set_s32(cfg, "split:arg", split_arg));
        CHECK(mpp_enc_cfg_set_s32(cfg, "split:out", split_out));
    }
    if (getenv("DBLK_DISABLE"))
        CHECK(mpp_enc_cfg_set_u32(cfg, "h265:dblk_disable",
                                  atoi(getenv("DBLK_DISABLE"))));
    if (getenv("DBLK_BETA"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "h265:dblk_alpha",
                                  atoi(getenv("DBLK_BETA"))));
    if (getenv("DBLK_TC"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "h265:dblk_beta",
                                  atoi(getenv("DBLK_TC"))));
    if (getenv("SAO_LUMA_DISABLE"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "h265:sao_luma_disable",
                                  atoi(getenv("SAO_LUMA_DISABLE"))));
    if (getenv("SAO_CHROMA_DISABLE"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "h265:sao_chroma_disable",
                                  atoi(getenv("SAO_CHROMA_DISABLE"))));
    if (getenv("LPF_ACS_SLI"))
        CHECK(mpp_enc_cfg_set_u32(cfg, "h265:lpf_acs_sli_en",
                                  atoi(getenv("LPF_ACS_SLI"))));
    if (getenv("LPF_ACS_TILE_DISABLE"))
        CHECK(mpp_enc_cfg_set_u32(cfg, "h265:lpf_acs_tile_disable",
                                  atoi(getenv("LPF_ACS_TILE_DISABLE"))));
    if (getenv("AUTO_TILE"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "h265:auto_tile",
                                  atoi(getenv("AUTO_TILE"))));
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

#ifdef H265E_ROI
    if (roi_regions)
        CHECK(mpp_enc_roi_init(&roi_ctx, width, height, MPP_VIDEO_CodingHEVC,
                               roi_regions));
#endif

    for (int picture = 0; picture < frames; picture++) {
        MppFrame frame = NULL;
        MppPacket packet = NULL;
        MppMeta meta = NULL;
        PacketReaderCtx reader;
        pthread_t reader_thread;
        int reader_started = 0;
        MPP_RET put_ret;
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
        mpp_frame_set_fmt(frame, input_format);
        mpp_frame_set_buffer(frame, frame_buf);

        meta = mpp_frame_get_meta(frame);
        CHECK(mpp_packet_init_with_buffer(&packet, packet_buf));
        mpp_packet_set_length(packet, 0);
        mpp_meta_set_packet(meta, KEY_OUTPUT_PACKET, packet);
        mpp_meta_set_buffer(meta, KEY_MOTION_INFO, md_info);

#ifdef H265E_ROI
        if (roi_ctx) {
            int at = 7;
            if (at < argc && argv[at][0] >= '0' && argv[at][0] <= '9')
                at++;
            for (; at < argc; at++) {
                RoiRegionCfg region;
                if (strcmp(argv[at], "roi"))
                    continue;
                region.x = (RK_U16)atoi(argv[at + 1]);
                region.y = (RK_U16)atoi(argv[at + 2]);
                region.w = (RK_U16)atoi(argv[at + 3]);
                region.h = (RK_U16)atoi(argv[at + 4]);
                region.force_intra = atoi(argv[at + 5]);
                region.qp_mode = atoi(argv[at + 6]);
                region.qp_val = atoi(argv[at + 7]);
                CHECK(mpp_enc_roi_add_region(roi_ctx, &region));
                at += 7;
            }
            CHECK(mpp_enc_roi_setup_meta(roi_ctx, meta));
        }
#endif

        reader.ctx = ctx;
        reader.mpi = mpi;
        reader.output = output;
        reader.frame = picture;
        reader.low_delay = (split_out & 1) != 0;
        reader.ret = MPP_NOK;
        reader.packets = 0;
        if (reader.low_delay) {
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
            fprintf(stderr, "packet reader failed for frame %d\n", picture);
            goto cleanup;
        }
    }
    ret = MPP_OK;

cleanup:
#ifdef H265E_ROI
    if (roi_ctx)
        mpp_enc_roi_deinit(roi_ctx);
#endif
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
