// SPDX-License-Identifier: Apache-2.0 OR MIT

#include <stdio.h>
#include <string.h>

#include "rk_mpi.h"
#include "rk_venc_kcfg.h"

static int set_init_keys(MppVencKcfg cfg)
{
    if (mpp_venc_kcfg_set_u32(cfg, "type", MPP_CTX_ENC) ||
        mpp_venc_kcfg_set_u32(cfg, "coding", MPP_VIDEO_CodingAVC) ||
        mpp_venc_kcfg_set_s32(cfg, "chan_id", 0) ||
        mpp_venc_kcfg_set_s32(cfg, "online", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "buf_size", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "max_strm_cnt", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "shared_buf_en", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "smart_en", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "max_width", 320) ||
        mpp_venc_kcfg_set_u32(cfg, "max_height", 240) ||
        mpp_venc_kcfg_set_u32(cfg, "max_lt_cnt", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "qpmap_en", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "chan_dup", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "tmvp_enable", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "only_smartp", 0) ||
        mpp_venc_kcfg_set_u32(cfg, "ntfy_mode", 0) ||
        mpp_venc_kcfg_set_s32(cfg, "input_timeout", MPP_POLL_BLOCK))
        return -1;

    return 0;
}

int main(void)
{
    MppCtx ctx = NULL;
    MppApi *mpi = NULL;
    MppVencKcfg cfg = NULL;
    int ret;

    if (mpp_create(&ctx, &mpi) || !ctx || !mpi)
        return 1;
    if (mpp_venc_kcfg_init(&cfg, MPP_VENC_KCFG_TYPE_INIT) || !cfg)
        return 2;
    if (set_init_keys(cfg))
        return 3;
    ret = mpi->control(ctx, MPP_SET_VENC_INIT_KCFG, cfg);
    if (ret)
        return 4;

    /* The compatible HAL must reject a type/coding disagreement without
     * discarding the pending config. */
    if (mpp_init(ctx, MPP_CTX_ENC, MPP_VIDEO_CodingHEVC) != MPP_ERR_VALUE)
        return 5;
    if (mpp_init(ctx, MPP_CTX_ENC, MPP_VIDEO_CodingAVC))
        return 6;
    mpp_destroy(ctx);
    mpp_venc_kcfg_deinit(cfg);

    ctx = NULL;
    mpi = NULL;
    cfg = NULL;
    if (mpp_create(&ctx, &mpi))
        return 7;
    if (mpp_venc_kcfg_init(&cfg, MPP_VENC_KCFG_TYPE_INIT))
        return 8;
    if (set_init_keys(cfg))
        return 9;
    if (mpp_venc_kcfg_set_u32(cfg, "not_a_vendor_key", 1))
        return 10;
    if (mpi->control(ctx, MPP_SET_VENC_INIT_KCFG, cfg) != MPP_NOK)
        return 11;
    mpp_destroy(ctx);
    mpp_venc_kcfg_deinit(cfg);

    puts("PASS venc init kcfg mapping, mismatch and unknown-key rejection");
    return 0;
}
