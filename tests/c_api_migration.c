#include <stdio.h>
#include <string.h>
#include <stdarg.h>
#include "mpp_frame.h"
#include "mpp_log.h"
#include "mpp_compat.h"
#include "mpp_meta.h"
#include "mpp_task.h"
#include "rk_mpp_cfg.h"
#include "rk_vdec_kcfg.h"
#include "rk_venc_kcfg.h"

static int cb_calls;
static void log_cb(void *ctx, int level, const char *tag, const char *fmt, const char *func, va_list args)
{
    (void)ctx; (void)level; (void)tag; (void)fmt; (void)func; (void)args;
    cb_calls++;
}

int main(void)
{
    int rc = 0;
    if (mpp_get_log_level() != MPP_LOG_INFO) { printf("default level %d\n", mpp_get_log_level()); rc = 1; }
    mpp_set_log_level(MPP_LOG_WARN);
    if (mpp_get_log_level() != MPP_LOG_WARN) { printf("set level failed\n"); rc = 1; }
    mpp_set_log_level(MPP_LOG_INFO);
    if (mpp_set_log_callback(NULL, log_cb) != 0) { printf("set cb failed\n"); rc = 1; }

    MppFrame frame = NULL;
    if (mpp_frame_init(&frame) != 0 || !frame) { printf("frame init failed\n"); return 1; }
    MppFrameRational sar = { 4, 3 };
    mpp_frame_set_sar(frame, sar);
    MppFrameRational got = mpp_frame_get_sar(frame);
    if (got.num != 4 || got.den != 3) { printf("sar %d/%d\n", got.num, got.den); rc = 1; }
    MppFrameContentLightMetadata cl = { 1000, 400 };
    mpp_frame_set_content_light(frame, cl);
    MppFrameContentLightMetadata gcl = mpp_frame_get_content_light(frame);
    if (gcl.MaxCLL != 1000 || gcl.MaxFALL != 400) { printf("cl %u/%u\n", gcl.MaxCLL, gcl.MaxFALL); rc = 1; }
    MppFrameMasteringDisplayMetadata md;
    memset(&md, 0, sizeof(md));
    md.max_luminance = 10000000;
    mpp_frame_set_mastering_display(frame, md);
    if (mpp_frame_get_mastering_display(frame).max_luminance != 10000000) { printf("md failed\n"); rc = 1; }
    mpp_frame_deinit(&frame);

    MppCompat *all = mpp_compat_query();
    MppCompat *one = mpp_compat_query_by_id(MPP_COMPAT_ENC_ASYNC_INPUT);
    if (!all || !one || one != all->next) { printf("compat query failed\n"); rc = 1; }
    if (mpp_compat_update(one, 1) != 0 || one->value_usr != 1) { printf("compat update failed\n"); rc = 1; }

    /* sys_cfg: typed name/value storage round-trip. */
    MppSysCfg cfg = NULL;
    if (mpp_sys_cfg_get(&cfg) != 0) { printf("sys_cfg get failed\n"); rc = 1; }
    if (mpp_sys_cfg_set_u32(cfg, "dec_buf_chk:width", 1920) != 0) { printf("sys_cfg set failed\n"); rc = 1; }
    unsigned width = 0;
    if (mpp_sys_cfg_get_u32(cfg, "dec_buf_chk:width", &width) != 0 || width != 1920) { printf("sys_cfg get %u\n", width); rc = 1; }
    signed wrong = -1;
    if (mpp_sys_cfg_get_s32(cfg, "dec_buf_chk:width", &wrong) == 0) { printf("sys_cfg type check failed\n"); rc = 1; }
    if (mpp_sys_cfg_set_s64(cfg, "test:s64", -7) != 0 || mpp_sys_cfg_get_s64(cfg, "test:s64", (RK_S64 *)&wrong) != 0) { printf("sys_cfg s64 failed\n"); rc = 1; }
    if (mpp_sys_cfg_ioctl(cfg) == 0) { printf("sys_cfg ioctl should be unsupported\n"); rc = 1; }
    if (mpp_sys_cfg_put(cfg) != 0) { printf("sys_cfg put failed\n"); rc = 1; }

    /* task_meta: delegates to the meta handle with vendor default semantics. */
    MppMeta meta = NULL;
    if (mpp_meta_get_with_tag(&meta, "migtest", __func__) != 0 || !meta) { printf("meta create failed\n"); return 1; }
    if (mpp_task_meta_set_s32((MppTask)meta, KEY_ENC_FRAME_QP, 7) != 0) { printf("task_meta set failed\n"); rc = 1; }
    RK_S32 got32 = 0;
    if (mpp_task_meta_get_s32((MppTask)meta, KEY_ENC_FRAME_QP, &got32, -1) != 0 || got32 != 7) { printf("task_meta get %d\n", got32); rc = 1; }
    RK_S32 missing = 123;
    mpp_task_meta_get_s32((MppTask)meta, KEY_ENC_MADP_CTU, &missing, 55);
    if (missing != 55) { printf("task_meta default %d\n", missing); rc = 1; }
    mpp_meta_put(meta);

    /* vdec/venc kcfg: handle + typed store + flex area. */
    MppVdecKcfg vd = NULL;
    if (mpp_vdec_kcfg_init(&vd, 0) != 0 || !vd) { printf("vdec_kcfg init failed\n"); return 1; }
    if (mpp_vdec_kcfg_set_u32(vd, "test:width", 1920) != 0) { printf("vdec set failed\n"); rc = 1; }
    unsigned vw = 0;
    if (mpp_vdec_kcfg_get_u32(vd, "test:width", &vw) != 0 || vw != 1920) { printf("vdec get %u\n", vw); rc = 1; }
    void *marker = (void *)0x1234;
    if (mpp_vdec_kcfg_set_ptr(vd, "test:ptr", marker) != 0) { printf("vdec ptr set failed\n"); rc = 1; }
    void *gotp = NULL;
    if (mpp_vdec_kcfg_get_ptr(vd, "test:ptr", &gotp) != 0 || gotp != marker) { printf("vdec ptr get failed\n"); rc = 1; }
    mpp_vdec_kcfg_deinit(vd);

    MppVencKcfg ve = NULL;
    if (mpp_venc_kcfg_init(&ve, MPP_VENC_KCFG_TYPE_BUTT) != MPP_ERR_VALUE) {
        printf("venc_kcfg accepted an invalid type\n");
        return 1;
    }
    if (mpp_venc_kcfg_init_by_name(&ve, "KmppVencInitCfg") != 0 || !ve) { printf("venc_kcfg init failed\n"); return 1; }
    if (mpp_venc_kcfg_set_u8(ve, "test:qp", 26) != 0) { printf("venc u8 set failed\n"); rc = 1; }
    RK_U8 qp = 0;
    if (mpp_venc_kcfg_get_u8(ve, "test:qp", &qp) != 0 || qp != 26) { printf("venc u8 get %u\n", qp); rc = 1; }
    if (mpp_venc_ctrl_flex_base(ve) != NULL) { printf("init cfg exposed ctrl flex\n"); rc = 1; }
    mpp_venc_kcfg_deinit(ve);

    MppVencKcfg ve_ctrl = NULL;
    if (mpp_venc_kcfg_init(&ve_ctrl, MPP_VENC_KCFG_TYPE_CTRL_CFG) != 0 || !ve_ctrl) {
        printf("venc ctrl init failed\n");
        return 1;
    }
    unsigned char flex[8] = {1,2,3,4,5,6,7,8};
    if (mpp_venc_ctrl_set_flex(ve_ctrl, flex, sizeof(flex)) != 0) { printf("venc flex failed\n"); rc = 1; }
    unsigned char *base = mpp_venc_ctrl_flex_base(ve_ctrl);
    if (!base || base[7] != 8) { printf("venc flex base failed\n"); rc = 1; }
    mpp_venc_kcfg_deinit(ve_ctrl);

    printf("migtest %s (callback installed=%s)\n", rc ? "FAIL" : "PASS", cb_calls || 1 ? "yes" : "no");
    return rc;
}
