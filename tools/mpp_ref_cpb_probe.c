// SPDX-License-Identifier: Apache-2.0 OR MIT

#include <stdio.h>

#include "mpp_enc_ref.h"
#include "rk_mpi.h"

static int run_case(const char *name, const MppEncRefStFrmCfg *st_cfg,
                    int st_count, const MppEncRefLtFrmCfg *lt_cfg,
                    int lt_count)
{
    MppEncRefCfg ref = NULL;
    MppEncCpbInfo *info;
    int ret;

    ret = mpp_enc_ref_cfg_init(&ref);
    if (ret || !ref)
        return 1;
    ret = mpp_enc_ref_cfg_set_cfg_cnt(ref, lt_count, st_count);
    if (!ret && st_count)
        ret = mpp_enc_ref_cfg_add_st_cfg(ref, st_count, (void *)st_cfg);
    if (!ret && lt_count)
        ret = mpp_enc_ref_cfg_add_lt_cfg(ref, lt_count, (void *)lt_cfg);
    if (!ret)
        ret = mpp_enc_ref_cfg_check(ref);
    info = mpp_enc_ref_cfg_get_cpb_info(ref);
    if (ret || !info) {
        mpp_enc_ref_cfg_deinit(&ref);
        return 2;
    }

    printf("%s dpb=%d lt=%d st=%d lt_idx=%d tid=%d lt_gop=%d st_gop=%d\n",
           name, info->dpb_size, info->max_lt_cnt, info->max_st_cnt,
           info->max_lt_idx, info->max_st_tid, info->lt_gop, info->st_gop);
    mpp_enc_ref_cfg_deinit(&ref);
    return 0;
}

static int run_json(const char *path)
{
    MppEncRefCfg ref = NULL;
    MppEncCpbInfo *info;
    int ret;

    ret = mpp_enc_ref_cfg_init(&ref);
    if (!ret)
        ret = mpp_enc_ref_cfg_apply_file(ref, MPP_CFG_STR_FMT_JSON, path);
    info = mpp_enc_ref_cfg_get_cpb_info(ref);
    if (ret || !info) {
        mpp_enc_ref_cfg_deinit(&ref);
        return 2;
    }
    printf("json dpb=%d lt=%d st=%d lt_idx=%d tid=%d lt_gop=%d st_gop=%d\n",
           info->dpb_size, info->max_lt_cnt, info->max_st_cnt,
           info->max_lt_idx, info->max_st_tid, info->lt_gop, info->st_gop);
    mpp_enc_ref_cfg_deinit(&ref);
    return 0;
}

int main(int argc, char **argv)
{
    const MppEncRefStFrmCfg ippp[] = {
        { .is_non_ref = 0, .temporal_id = 0, .ref_mode = REF_TO_PREV_REF_FRM },
    };
    const MppEncRefStFrmCfg tsvc2[] = {
        { .is_non_ref = 0, .temporal_id = 0, .ref_mode = REF_TO_TEMPORAL_LAYER },
        { .is_non_ref = 1, .temporal_id = 1, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 0, .temporal_id = 0, .ref_mode = REF_TO_PREV_REF_FRM },
    };
    const MppEncRefStFrmCfg tsvc3[] = {
        { .is_non_ref = 0, .temporal_id = 0, .ref_mode = REF_TO_TEMPORAL_LAYER },
        { .is_non_ref = 1, .temporal_id = 2, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 0, .temporal_id = 1, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 1, .temporal_id = 2, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 0, .temporal_id = 0, .ref_mode = REF_TO_TEMPORAL_LAYER },
    };
    const MppEncRefStFrmCfg tsvc4[] = {
        { .is_non_ref = 0, .temporal_id = 0, .ref_mode = REF_TO_TEMPORAL_LAYER },
        { .is_non_ref = 1, .temporal_id = 3, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 0, .temporal_id = 2, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 1, .temporal_id = 3, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 0, .temporal_id = 1, .ref_mode = REF_TO_PREV_LT_REF },
        { .is_non_ref = 1, .temporal_id = 3, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 0, .temporal_id = 2, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 1, .temporal_id = 3, .ref_mode = REF_TO_PREV_REF_FRM },
        { .is_non_ref = 0, .temporal_id = 0, .ref_mode = REF_TO_TEMPORAL_LAYER },
    };
    const MppEncRefLtFrmCfg tsvc4_lt[] = {
        {
            .lt_idx = 0,
            .temporal_id = 0,
            .ref_mode = REF_TO_PREV_LT_REF,
            .lt_gap = 8,
        },
    };

    if (run_case("ippp", ippp, 1, NULL, 0) ||
        run_case("tsvc2", tsvc2, 3, NULL, 0) ||
        run_case("tsvc3", tsvc3, 5, NULL, 0) ||
        run_case("tsvc4", tsvc4, 9, tsvc4_lt, 1) ||
        (argc > 1 && run_json(argv[1])))
        return 1;

    return 0;
}
