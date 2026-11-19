/* Exercise MppEncCfg scalar/array string APIs on KmppVencStCfg. */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mpp_enc_cfg.h"
#include "rk_venc_cfg.h"
#include "rk_venc_kcfg.h"

static int run_format(MppEncCfg cfg, const char *name, MppCfgStrFmt format,
                      const rk_s32 *hierarchy, const rk_s32 *aq_step)
{
    char *text = NULL;
    rk_s32 got_hierarchy[4] = { 0 };
    rk_s32 got_aq_step[16] = { 0 };
    rk_s32 mode = 0;
    int ret = 2;

    if (mpp_enc_cfg_set_s32(cfg, "rc:mode", 7) ||
        mpp_enc_cfg_set_st(cfg, "rc:hier_qp_delta", (rk_s32 *)hierarchy) ||
        mpp_enc_cfg_set_st(cfg, "hw:aq_step_i", (rk_s32 *)aq_step) ||
        mpp_enc_cfg_extract(cfg, format, &text) ||
        !text) {
        fprintf(stderr, "%s seed/extract failed\n", name);
        goto done;
    }

    if (mpp_enc_cfg_set_s32(cfg, "rc:mode", 0) ||
        mpp_enc_cfg_set_st(cfg, "rc:hier_qp_delta", (rk_s32[4]){ 0 }) ||
        mpp_enc_cfg_set_st(cfg, "hw:aq_step_i", (rk_s32[16]){ 0 }) ||
        mpp_enc_cfg_apply(cfg, format, text)) {
        fprintf(stderr, "%s reset/apply failed\n", name);
        ret = 3;
        goto done;
    }

    if (mpp_enc_cfg_get_s32(cfg, "rc:mode", &mode) ||
        mode != 7 ||
        mpp_enc_cfg_get_st(cfg, "rc:hier_qp_delta", got_hierarchy) ||
        mpp_enc_cfg_get_st(cfg, "hw:aq_step_i", got_aq_step) ||
        memcmp(got_hierarchy, hierarchy, sizeof(got_hierarchy)) ||
        memcmp(got_aq_step, aq_step, sizeof(got_aq_step))) {
        fprintf(stderr, "%s roundtrip mismatch: mode=%d\n", name, mode);
        ret = 4;
        goto done;
    }

    printf("PASS mpp/enc-cfg kcfg scalar+array roundtrip (%s)\n", name);
    ret = 0;

done:
    free(text);
    return ret;
}

int main(void)
{
    MppEncCfg cfg = NULL;
    const rk_s32 hierarchy[4] = { 1, 2, 3, 4 };
    const rk_s32 aq_step[16] = {
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15
    };
    int ret = 0;

    if (mpp_venc_kcfg_init((MppVencKcfg *)&cfg, MPP_VENC_KCFG_TYPE_ST_CFG) ||
        !cfg) {
        fprintf(stderr, "mpp_venc_kcfg_init failed\n");
        return 1;
    }

    if (run_format(cfg, "log", MPP_CFG_STR_FMT_LOG, hierarchy, aq_step) ||
        run_format(cfg, "json", MPP_CFG_STR_FMT_JSON, hierarchy, aq_step) ||
        run_format(cfg, "toml", MPP_CFG_STR_FMT_TOML, hierarchy, aq_step))
        ret = 2;

    if (mpp_enc_cfg_deinit(cfg) && !ret)
        ret = 3;
    return ret;
}
