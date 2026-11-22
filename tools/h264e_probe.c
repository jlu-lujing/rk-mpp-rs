/*
 * Drive the reference H.264 encoder with the configuration knobs the board
 * comparisons need and `mpi_enc_test` either hard-codes or does not have.
 *
 * Cells covered, one invocation each:
 *
 *   * a caller that never set an initial quality (the `rc_model_v2` flag is
 *     `init_quality <= 0`, which `rc_model_v2_start` folds onto -1 and
 *     `rc_model_v2_hal_start` derives from the bit budget);
 *   * the `MppEncH264Cfg` deblocking triple - `mpi_enc_test` of this vintage
 *     never touches `h264:dblk_*`, so the reference stream under test has to be
 *     produced by a caller that does;
 *   * ROI regions, through the same `mpp_enc_roi_*` helpers `mpi_enc_test`
 *     links, so the region-to-CU-map conversion is the reference's own.
 *
 * Build on RK3588 against the reference build, not the installed library, and link
 * the reference's own ROI helper:
 *   cc -O2 -w -I$MPP/inc $MPP/utils/mpp_enc_roi_utils.c /tmp/h264e_probe.c \
 *      $MPP/build/mpp/librockchip_mpp.so.1 -o /tmp/h264e_probe
 *
 * Usage:
 *   h264e_probe in.nv12 out.264 w h frames <0|1|2|3> qp [bps[:min:max]]
 *               [dblk disable alpha beta] [roi x y w h fi qpmode qpval]...
 *               [gop n] [hier d0:d1:d2:d3:n0:n1:n2:n3]
 *               [aqstep s0:s1:...:s15]
 *               [gdr mode num] [split mode arg] [splitout value] [seioff]
 *               [forceidr frame[,frame...]] [headereach]
 *
 *   mode 0 = fixed QP, 1 = CBR, 2 = VBR, 3 = AVBR.
 *   `gop` overrides the hard-coded intra GOP of 60.
 *   `gdr 0|1 n` is `rc:refresh_en=1, refresh_mode=0(row)|1(col), refresh_num=n`;
 *   the reference derives the refresh length itself in `h264e_proc_cfg`.
 *   `split 1|2 n` is `MPP_ENC_SPLIT_BY_BYTE` with an n-byte cap or
 *   `MPP_ENC_SPLIT_BY_CTU` with n macroblocks per slice, at the reference
 *   `splitout 1` enables MPP_ENC_SPLIT_OUT_LOWDELAY; packets are then drained
 *   through EOI. The default of zero keeps one access unit per picture.
 *   `hier` enables hierarchical QP with four layer deltas and frame counts.
 *   `aqstep` supplies the 16-entry `hw:aq_step_i` table.
 *   `seioff` sends MPP_ENC_SEI_MODE_DISABLE for streams intended to compare
 *   only the SPS/PPS prefix.
 *   `forceidr` sends MPP_ENC_SET_IDR_FRAME immediately before each listed
 *   zero-based input frame.
 *   `headereach` requests MPP_ENC_HEADER_MODE_EACH_IDR.
 *   DUMP_META=1 prints every packet's encoder metadata and the FNV-1a hash of
 *   the caller-provided KEY_MOTION_INFO buffer.
 *   qp: the value written to the initial-quality field; pass -1 under a rate
 *       control to say "the caller left it unset" (written as -1 so the model
 *       takes its derivation branch), ignored beyond that.
 *   The rc-cfg key is tried as `rc:qp_init` first and `rc:quality_init` next,
 *   and the deblocking keys as `h264:dblk_*` then `h264:deblock_*`, because the
 *   trie renames between 2024-04 and master are exactly the kind of thing this
 *   probe should not have to know.
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
#include "mpp_rc_api.h"
#include "rk_mpi.h"
#include "rk_venc_cfg.h"

#define CUSTOM_RC_CODING MPP_VIDEO_CodingAVC
#include "custom_rc_probe.h"

#ifndef H264E_PROBE_NO_ROI
#include "mpp_enc_roi_utils.h"
#else
typedef MppEncROIRegion StandaloneRoiRegion;
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

/* Try a sequence of config keys and keep the one the library knows. */
static MPP_RET set_s32_any(MppEncCfg cfg, int *tried, const char *key,
                           int value)
{
    MPP_RET ret = mpp_enc_cfg_set_s32(cfg, key, value);
    if (tried && *tried && ret)
        return MPP_NOK;
    if (!ret && tried)
        *tried = 1;
    return ret;
}

static MPP_RET set_quality_init(MppEncCfg cfg, int value)
{
    int tried = 0;
    MPP_RET ret = set_s32_any(cfg, &tried, "rc:qp_init", value);
    if (ret)
        ret = set_s32_any(cfg, &tried, "rc:quality_init", value);
    return ret;
}

static MPP_RET set_deblock(MppEncCfg cfg, int disable, int alpha, int beta)
{
    int tried = 0;
    MPP_RET ret = set_s32_any(cfg, &tried, "h264:dblk_disable", disable);
    if (ret)
        ret = set_s32_any(cfg, &tried, "h264:deblock_disable", disable);
    if (!ret)
        ret = set_s32_any(cfg, &tried, "h264:dblk_alpha", alpha);
    if (ret)
        ret = set_s32_any(cfg, &tried, "h264:deblock_offset_alpha", alpha);
    if (!ret)
        ret = set_s32_any(cfg, &tried, "h264:dblk_beta", beta);
    if (ret)
        ret = set_s32_any(cfg, &tried, "h264:deblock_offset_beta", beta);
    return ret;
}

static int force_idr_contains(const char *spec, int frame)
{
    const char *cursor = spec;

    while (cursor && *cursor) {
        char *end = NULL;
        long value = strtol(cursor, &end, 10);

        if (end == cursor)
            break;
        if (value == frame)
            return 1;
        if (*end != ',')
            break;
        cursor = end + 1;
    }
    return 0;
}

typedef struct PacketReaderCtx_t {
    MppCtx ctx;
    MppApi *mpi;
    FILE *output;
    int frame;
    int low_delay;
    MPP_RET ret;
    unsigned packets;
} PacketReaderCtx;

static void dump_meta_s32(MppMeta meta, MppMetaKey key, const char *name)
{
    RK_S32 value = 0;

    if (mpp_meta_get_s32(meta, key, &value) == MPP_OK)
        fprintf(stderr, " %s=%d", name, value);
    else
        fprintf(stderr, " %s=-", name);
}

static void dump_motion_meta(MppMeta meta)
{
    MppBuffer buffer = NULL;
    unsigned char bytes[256];
    uint64_t hash = UINT64_C(1469598103934665603);
    size_t offset = 0;
    size_t size;

    if (mpp_meta_get_buffer(meta, KEY_MOTION_INFO, &buffer) != MPP_OK) {
        fprintf(stderr, " motion=-");
        return;
    }
    if (!buffer) {
        fprintf(stderr, " motion=null");
        return;
    }
    size = mpp_buffer_get_size(buffer);
    if (mpp_buffer_sync_begin(buffer) != MPP_OK) {
        fprintf(stderr, " motion=sync-error");
        return;
    }
    while (offset < size) {
        size_t chunk = size - offset < sizeof(bytes) ? size - offset : sizeof(bytes);
        size_t index;

        if (mpp_buffer_read(buffer, offset, bytes, chunk) != MPP_OK) {
            mpp_buffer_sync_end(buffer);
            fprintf(stderr, " motion=read-error");
            return;
        }
        for (index = 0; index < chunk; index++) {
            hash ^= bytes[index];
            hash *= UINT64_C(1099511628211);
        }
        offset += chunk;
    }
    mpp_buffer_sync_end(buffer);
    fprintf(stderr, " motion=%zu:%016llx", size, (unsigned long long)hash);
}

static void dump_packet_meta(MppPacket packet, int frame, unsigned part)
{
    MppMeta meta;
    RK_S64 sse = 0;

    if (!getenv("DUMP_META"))
        return;

    meta = mpp_packet_get_meta(packet);
    if (!meta) {
        fprintf(stderr, "picture %d packet %u meta: unavailable\n",
                frame, part);
        return;
    }

    fprintf(stderr, "picture %d packet %u meta:", frame, part);
    dump_meta_s32(meta, KEY_LVL64_INTER_NUM, "l64p");
    dump_meta_s32(meta, KEY_LVL32_INTER_NUM, "l32p");
    dump_meta_s32(meta, KEY_LVL16_INTER_NUM, "l16p");
    dump_meta_s32(meta, KEY_LVL8_INTER_NUM, "l8p");
    dump_meta_s32(meta, KEY_LVL32_INTRA_NUM, "l32i");
    dump_meta_s32(meta, KEY_LVL16_INTRA_NUM, "l16i");
    dump_meta_s32(meta, KEY_LVL8_INTRA_NUM, "l8i");
    dump_meta_s32(meta, KEY_LVL4_INTRA_NUM, "l4i");
    if (mpp_meta_get_s64(meta, KEY_ENC_SSE, &sse) == MPP_OK)
        fprintf(stderr, " sse=%lld", (long long)sse);
    else
        fprintf(stderr, " sse=-");
    dump_meta_s32(meta, KEY_OUTPUT_PSKIP, "pskip");
#ifdef H264E_DUMP_RT_BITS
    dump_meta_s32(meta, KEY_ENC_BPS_RT, "rt_bits");
#endif
    dump_meta_s32(meta, KEY_ENC_USE_LTR, "use_ltr");
    dump_meta_s32(meta, KEY_OUTPUT_INTRA, "intra");
    dump_meta_s32(meta, KEY_ENC_START_QP, "start_qp");
    dump_meta_s32(meta, KEY_ENC_AVERAGE_QP, "avg_qp");
    dump_motion_meta(meta);
    fprintf(stderr, "\n");
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
            fprintf(stderr, "no packet for frame %d part %u: ret=%d\n",
                    reader->frame, reader->packets, ret);
            return NULL;
        }

        data = mpp_packet_get_pos(packet);
        len = mpp_packet_get_length(packet);
        eoi = mpp_packet_is_eoi(packet);
        fprintf(stderr, "picture %d packet %u: %zu bytes eoi=%u\n",
                reader->frame, reader->packets, len, eoi);
        dump_packet_meta(packet, reader->frame, reader->packets);
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
    const char *output_path = argc > 2 ? argv[2] : "/tmp/h264e_probe.264";
    int width = argc > 3 ? atoi(argv[3]) : 320;
    int height = argc > 4 ? atoi(argv[4]) : 240;
    int frames = argc > 5 ? atoi(argv[5]) : 1;
    int mode = argc > 6 ? atoi(argv[6]) : 0; /* 0 fixqp 1 cbr 2 vbr 3 avbr */
    int qp = argc > 7 ? atoi(argv[7]) : 26;
    int bps_target = 300000, bps_min = 250000, bps_max = 350000;
    int have_dblk = 0, dblk_disable = 0, dblk_alpha = 0, dblk_beta = 0;
    int gop = 60;
    int have_gdr = 0, gdr_mode = 0, gdr_num = 0;
    int have_split = 0, split_mode = 0, split_arg = 0, split_out = 0;
    int have_hier = 0;
    int have_aqstep = 0;
    int sei_off = 0;
    int header_each = 0;
    int profile = getenv("H264_PROFILE") ? atoi(getenv("H264_PROFILE")) : 66;
    int input_format = getenv("MPP_INPUT_FORMAT")
                           ? atoi(getenv("MPP_INPUT_FORMAT"))
                           : MPP_FMT_YUV420SP;
    MppBufferType input_buffer_type =
        getenv("CPU_BUFFER")
            ? MPP_BUFFER_TYPE_NORMAL
            : (MppBufferType)(MPP_BUFFER_TYPE_DMA_HEAP |
                              MPP_BUFFER_FLAGS_CACHABLE);
    const char *force_idr = NULL;
    int hier_qp_delta[4] = { 0, 0, 0, 0 };
    int hier_frame_num[4] = { 0, 0, 0, 0 };
    int aq_step[16] = { 0 };
    int hor_stride = (width + 15) & ~15;
    int ver_stride = (height + 15) & ~15;
    size_t frame_size = (size_t)hor_stride * ver_stride * 3 / 2;

    MppCtx ctx = NULL;
    MppApi *mpi = NULL;
    MppEncCfg cfg = NULL;
    MppBufferGroup group = NULL;
    MppBuffer frame_buf = NULL;
    MppBuffer packet_buf = NULL;
#ifndef H264E_PROBE_NO_ROI
    MppEncRoiCtx roi_ctx = NULL;
    int roi_regions = 0;
#else
    StandaloneRoiRegion roi_regions_legacy[8];
    MppEncROICfg roi_cfg_legacy;
    int roi_regions = 0;
#endif
    FILE *input = NULL;
    FILE *output = NULL;
    MPP_RET ret = MPP_NOK;
    int i, arg;
    int tried_qp_ip = 0;

    /* Optional `bps:min:max` or bare bps - recognised as the only leading
     * token that starts with a digit - then the optional dblk, roi, gop, gdr
     * and split option groups in any order after it. */
    arg = 8;
    if (arg < argc && argv[arg][0] >= '0' && argv[arg][0] <= '9') {
        int parts = sscanf(argv[arg], "%d:%d:%d", &bps_target, &bps_min,
                           &bps_max);
        if (parts < 1) {
            fprintf(stderr, "bad bps %s\n", argv[arg]);
            return 2;
        }
        if (parts == 1) {
            bps_min = bps_target * 15 / 16;
            bps_max = bps_target * 17 / 16;
        }
        arg++;
    }
    for (; arg < argc; arg++) {
        if (!strcmp(argv[arg], "dblk")) {
            if (arg + 3 >= argc) {
                fprintf(stderr, "dblk wants disable alpha beta\n");
                return 2;
            }
            have_dblk = 1;
            dblk_disable = atoi(argv[++arg]);
            dblk_alpha = atoi(argv[++arg]);
            dblk_beta = atoi(argv[++arg]);
#ifndef H264E_PROBE_NO_ROI
        } else if (!strcmp(argv[arg], "roi")) {
            roi_regions++;
            arg += 7;
            if (arg >= argc) {
                fprintf(stderr, "roi wants x y w h fi qpmode qpval\n");
                return 2;
            }
            arg--;
#else
        } else if (!strcmp(argv[arg], "roi")) {
            StandaloneRoiRegion *region;

            if (roi_regions >= 8 || arg + 7 >= argc) {
                fprintf(stderr, "roi wants x y w h fi qpmode qpval\n");
                return 2;
            }
            region = &roi_regions_legacy[roi_regions++];
            memset(region, 0, sizeof(*region));
            region->x = (RK_U16)atoi(argv[arg + 1]);
            region->y = (RK_U16)atoi(argv[arg + 2]);
            region->w = (RK_U16)atoi(argv[arg + 3]);
            region->h = (RK_U16)atoi(argv[arg + 4]);
            region->intra = atoi(argv[arg + 5]);
            region->abs_qp_en = atoi(argv[arg + 6]);
            region->quality = atoi(argv[arg + 7]);
            region->area_map_en = 1;
            arg += 7;
#endif
        } else if (!strcmp(argv[arg], "gop")) {
            if (arg + 1 >= argc) {
                fprintf(stderr, "gop wants n\n");
                return 2;
            }
            gop = atoi(argv[++arg]);
        } else if (!strcmp(argv[arg], "gdr")) {
            if (arg + 2 >= argc) {
                fprintf(stderr, "gdr wants mode num\n");
                return 2;
            }
            have_gdr = 1;
            gdr_mode = atoi(argv[++arg]);
            gdr_num = atoi(argv[++arg]);
        } else if (!strcmp(argv[arg], "split")) {
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
        } else if (!strcmp(argv[arg], "hier")) {
            if (arg + 1 >= argc ||
                sscanf(argv[++arg], "%d:%d:%d:%d:%d:%d:%d:%d",
                       &hier_qp_delta[0], &hier_qp_delta[1],
                       &hier_qp_delta[2], &hier_qp_delta[3],
                       &hier_frame_num[0], &hier_frame_num[1],
                       &hier_frame_num[2], &hier_frame_num[3]) != 8) {
                fprintf(stderr, "hier wants d0:d1:d2:d3:n0:n1:n2:n3\n");
                return 2;
            }
            have_hier = 1;
        } else if (!strcmp(argv[arg], "aqstep")) {
            if (arg + 1 >= argc ||
                sscanf(argv[++arg],
                       "%d:%d:%d:%d:%d:%d:%d:%d:%d:%d:%d:%d:%d:%d:%d:%d",
                       &aq_step[0], &aq_step[1], &aq_step[2], &aq_step[3],
                       &aq_step[4], &aq_step[5], &aq_step[6], &aq_step[7],
                       &aq_step[8], &aq_step[9], &aq_step[10], &aq_step[11],
                       &aq_step[12], &aq_step[13], &aq_step[14],
                       &aq_step[15]) != 16) {
                fprintf(stderr, "aqstep wants 16 colon-separated values\n");
                goto cleanup;
            }
            have_aqstep = 1;
        } else if (!strcmp(argv[arg], "seioff")) {
            sei_off = 1;
        } else if (!strcmp(argv[arg], "forceidr")) {
            if (arg + 1 >= argc) {
                fprintf(stderr, "forceidr wants a frame list\n");
                return 2;
            }
            force_idr = argv[++arg];
        } else if (!strcmp(argv[arg], "headereach")) {
            header_each = 1;
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

    CHECK(mpp_buffer_group_get_internal(&group, input_buffer_type));
    CHECK(mpp_buffer_get(group, &frame_buf, frame_size));
    CHECK(mpp_buffer_get(group, &packet_buf, frame_size + 4096));

    CHECK(mpp_create(&ctx, &mpi));
    CHECK(mpp_init(ctx, MPP_CTX_ENC, MPP_VIDEO_CodingAVC));
    if (getenv("CUSTOM_RC")) {
        RcApiBrief brief = {
            .name = "custom_probe",
            .type = MPP_VIDEO_CodingAVC,
        };

        CHECK(mpi->control(ctx, MPP_ENC_SET_RC_API_CFG,
                           (void *)&custom_rc_api));
        CHECK(mpi->control(ctx, MPP_ENC_SET_RC_API_CURRENT, &brief));
    }
    CHECK(mpp_enc_cfg_init(&cfg));
    CHECK(mpi->control(ctx, MPP_ENC_GET_CFG, cfg));

    CHECK(mpp_enc_cfg_set_s32(cfg, "codec:type", MPP_VIDEO_CodingAVC));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:width", width));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:height", height));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:hor_stride", hor_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:ver_stride", ver_stride));
    CHECK(mpp_enc_cfg_set_s32(cfg, "prep:format", input_format));
    if (getenv("SCENE_MODE"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "tune:scene_mode",
                                  atoi(getenv("SCENE_MODE"))));
    if (getenv("QBIAS_EN"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "hw:qbias_en",
                                  atoi(getenv("QBIAS_EN"))));
    if (getenv("QBIAS_I"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "hw:qbias_i",
                                  atoi(getenv("QBIAS_I"))));
    if (getenv("QBIAS_P"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "hw:qbias_p",
                                  atoi(getenv("QBIAS_P"))));
    CHECK(mpp_enc_cfg_set_s32(cfg,
                              "rc:mode",
                              mode == 0 ? MPP_ENC_RC_MODE_FIXQP
                                        : (mode == 1 ? MPP_ENC_RC_MODE_CBR
                                                     : (mode == 2
                                                            ? MPP_ENC_RC_MODE_VBR
                                                            : MPP_ENC_RC_MODE_AVBR))));
    CHECK(set_quality_init(cfg, qp));
    /* Fixed QP ignores the budget for quantisation but the `uuid_rc_cfg` SEI
     * prints all three figures, so the probe writes them in every mode; the
     * board scripts pass the size-derived target and the library's bounds,
     * which is what the harness leaves behind for a fixed-QP stream. */
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:bps_target", bps_target));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:bps_min", bps_min));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:bps_max", bps_max));
    if (mode == 0) {
        /* `mpi_enc_test -rc 2 -qc q:q:q:q:q` - the fixed-QP harness the crate's
         * matrix is built on - collapses every bound onto the QP and leaves the
         * I/P delta at the zero its config struct started from. The library's
         * own defaults are 8..48 and 2, so without this the fixed-QP cell would
         * compare our collapsed line against the default one. */
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_min_i", qp));
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:qp_max_i", qp));
        CHECK(set_s32_any(cfg, &tried_qp_ip, "rc:qp_ip", 0));
    }
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:gop", gop));
    if (have_hier) {
        CHECK(mpp_enc_cfg_set_s32(cfg, "rc:hier_qp_en", 1));
        CHECK(mpp_enc_cfg_set_st(cfg, "rc:hier_qp_delta", hier_qp_delta));
        CHECK(mpp_enc_cfg_set_st(cfg, "rc:hier_frame_num", hier_frame_num));
    }
    if (have_aqstep)
        CHECK(mpp_enc_cfg_set_st(cfg, "hw:aq_step_i", aq_step));
    if (have_gdr) {
        /* The refresh trio of `MppEncRcCfg`, registered as U32 keys.
         * `h264e_proc_cfg` derives `refresh_length` from these on SET_CFG,
         * which is exactly why the probe drives the cfg-key path and not a
         * raw `MPP_ENC_SET_RC_CFG` struct. */
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:refresh_en", 1));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:refresh_mode", gdr_mode));
        CHECK(mpp_enc_cfg_set_u32(cfg, "rc:refresh_num", gdr_num));
    }
    if (have_split) {
        /* `split:mode/split:arg`; `split:out=1` keeps each completed slice. */
        CHECK(mpp_enc_cfg_set_s32(cfg, "split:mode", split_mode));
        CHECK(mpp_enc_cfg_set_s32(cfg, "split:arg", split_arg));
        CHECK(mpp_enc_cfg_set_s32(cfg, "split:out", split_out));
    }
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_flex", 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_num", 30));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_in_denom", 1));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_flex", 0));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_num", 30));
    CHECK(mpp_enc_cfg_set_s32(cfg, "rc:fps_out_denom", 1));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:profile", profile));
    CHECK(mpp_enc_cfg_set_s32(cfg, "h264:level", 30));
    CHECK(mpp_enc_cfg_set_s32(
        cfg, "h264:cabac_en",
        getenv("H264_CABAC") ? atoi(getenv("H264_CABAC"))
                             : (profile >= 77 ? 1 : 0)));
    CHECK(mpp_enc_cfg_set_s32(
        cfg, "h264:trans8x8",
        getenv("H264_TRANS8X8") ? atoi(getenv("H264_TRANS8X8"))
                                : (profile >= 100 ? 1 : 0)));
    if (getenv("H264_CABAC_IDC"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "h264:cabac_idc",
                                  atoi(getenv("H264_CABAC_IDC"))));
    if (getenv("H264_CONSTRAINT_SET"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "h264:constraint_set",
                                  strtol(getenv("H264_CONSTRAINT_SET"), NULL, 0)));
    if (getenv("H264_BASE_LAYER_PID"))
        CHECK(mpp_enc_cfg_set_s32(cfg, "h264:base_layer_pid",
                                  atoi(getenv("H264_BASE_LAYER_PID"))));
    if (have_dblk)
        CHECK(set_deblock(cfg, dblk_disable, dblk_alpha, dblk_beta));
    CHECK(mpi->control(ctx, MPP_ENC_SET_CFG, cfg));
    if (sei_off) {
        MppEncSeiMode sei_mode = MPP_ENC_SEI_MODE_DISABLE;

        CHECK(mpi->control(ctx, MPP_ENC_SET_SEI_CFG, &sei_mode));
    }
    if (header_each) {
        MppEncHeaderMode header_mode = MPP_ENC_HEADER_MODE_EACH_IDR;

        CHECK(mpi->control(ctx, MPP_ENC_SET_HEADER_MODE, &header_mode));
    }

    /* The ROI regions are re-read from the tail of the command line here, the
     * same way mpi_enc_test builds them per frame: the same regions every
     * picture, sent as metadata. */
#ifndef H264E_PROBE_NO_ROI
    if (roi_regions) {
        CHECK(mpp_enc_roi_init(&roi_ctx, width, height, MPP_VIDEO_CodingAVC,
                               roi_regions));
    }
#endif

    for (i = 0; i < frames; i++) {
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
        mpp_frame_set_fmt(frame, input_format);
        mpp_frame_set_buffer(frame, frame_buf);

        meta = mpp_frame_get_meta(frame);
        CHECK(mpp_packet_init_with_buffer(&packet, packet_buf));
        mpp_packet_set_length(packet, 0);
        mpp_meta_set_packet(meta, KEY_OUTPUT_PACKET, packet);

#ifndef H264E_PROBE_NO_ROI
        if (roi_ctx) {
            int at = 8;
            /* walk past the bps token to the first roi group again;
             * `mpp_enc_roi_setup_meta` resets the pending count itself, so the
             * same init serves every frame, as it does in mpi_enc_test */
            if (at < argc && argv[at][0] >= '0' && argv[at][0] <= '9')
                at++;
            for (; at < argc; at++) {
                if (strcmp(argv[at], "roi"))
                    continue;
                {
                    RoiRegionCfg region;
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
            }
            CHECK(mpp_enc_roi_setup_meta(roi_ctx, meta));
        }
#else
        if (roi_regions) {
            roi_cfg_legacy.number = roi_regions;
            roi_cfg_legacy.regions = roi_regions_legacy;
            CHECK(mpp_meta_set_ptr(meta, KEY_ROI_DATA, &roi_cfg_legacy));
        }
#endif

        reader.ctx = ctx;
        reader.mpi = mpi;
        reader.output = output;
        reader.frame = i;
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

        if (force_idr_contains(force_idr, i))
            CHECK(mpi->control(ctx, MPP_ENC_SET_IDR_FRAME, NULL));

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
    }
    ret = MPP_OK;

cleanup:
#ifndef H264E_PROBE_NO_ROI
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
    if (frame_buf)
        mpp_buffer_put(frame_buf);
    if (packet_buf)
        mpp_buffer_put(packet_buf);
    if (group)
        mpp_buffer_group_put(group);
    return ret;
}
