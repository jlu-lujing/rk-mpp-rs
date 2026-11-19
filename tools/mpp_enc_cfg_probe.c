// SPDX-License-Identifier: Apache-2.0 OR MIT

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mpp_enc_cfg.h"
#include "rk_mpp_cfg.h"
#include "rk_mpi.h"

static int set_scalar(MppEncCfg cfg)
{
    int ret = 0;

    ret = mpp_enc_cfg_set_s32(cfg, "rc:mode", 2);
    if (!ret)
        ret = mpp_enc_cfg_set_s32(cfg, "rc:bps_target", 800000);
    if (!ret)
        ret = mpp_enc_cfg_set_u32(cfg, "prep:width", 1920);
    if (!ret)
        ret = mpp_enc_cfg_set_s32(cfg, "codec:type", 0x01000004);
    return ret;
}

static int write_fmt(const char *prefix, const char *suffix,
                     MppCfgStrFmt format)
{
    MppEncCfg cfg = NULL;
    char path[512];
    char *text = NULL;
    FILE *file;
    int ret;

    ret = mpp_enc_cfg_init(&cfg);
    if (!ret)
        ret = set_scalar(cfg);
    if (!ret)
        ret = mpp_enc_cfg_extract(cfg, format, &text);
    if (ret || !text) {
        mpp_enc_cfg_deinit(cfg);
        return 1;
    }
    snprintf(path, sizeof(path), "%s.%s", prefix, suffix);
    file = fopen(path, "w");
    if (!file) {
        free(text);
        mpp_enc_cfg_deinit(cfg);
        return 2;
    }
    ret = fputs(text, file) < 0;
    if (fclose(file))
        ret = 1;
    free(text);
    mpp_enc_cfg_deinit(cfg);
    return ret ? 3 : 0;
}

static int apply_fmt(const char *name, MppCfgStrFmt format, const char *path)
{
    MppEncCfg cfg = NULL;
    int mode = 0, bps = 0, codec = 0;
    unsigned width = 0;
    int ret;

    ret = mpp_enc_cfg_init(&cfg);
    if (!ret)
        ret = mpp_enc_cfg_apply_file(cfg, format, path);
    if (!ret)
        ret = mpp_enc_cfg_get_s32(cfg, "rc:mode", &mode);
    if (!ret)
        ret = mpp_enc_cfg_get_s32(cfg, "rc:bps_target", &bps);
    if (!ret)
        ret = mpp_enc_cfg_get_u32(cfg, "prep:width", &width);
    if (!ret)
        ret = mpp_enc_cfg_get_s32(cfg, "codec:type", &codec);
    if (ret) {
        mpp_enc_cfg_deinit(cfg);
        return 4;
    }
    printf("%s mode=%d bps=%d width=%u codec=%08x\n",
           name, mode, bps, width, (unsigned)codec);
    mpp_enc_cfg_deinit(cfg);
    return 0;
}

int main(int argc, char **argv)
{
    if (argc > 2 && !strcmp(argv[1], "dump")) {
        if (write_fmt(argv[2], "log", MPP_CFG_STR_FMT_LOG) ||
            write_fmt(argv[2], "json", MPP_CFG_STR_FMT_JSON) ||
            write_fmt(argv[2], "toml", MPP_CFG_STR_FMT_TOML))
            return 1;
        return 0;
    }
    if (argc > 3 && !strcmp(argv[1], "apply")) {
        MppCfgStrFmt format;
        if (!strcmp(argv[2], "log"))
            format = MPP_CFG_STR_FMT_LOG;
        else if (!strcmp(argv[2], "json"))
            format = MPP_CFG_STR_FMT_JSON;
        else if (!strcmp(argv[2], "toml"))
            format = MPP_CFG_STR_FMT_TOML;
        else
            return 1;
        return apply_fmt(argv[2], format, argv[3]);
    }
    fprintf(stderr, "usage: %s dump <prefix> | apply <fmt> <path>\n", argv[0]);
    return 2;
}
