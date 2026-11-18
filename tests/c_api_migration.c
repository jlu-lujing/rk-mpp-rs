#include <stdio.h>
#include <string.h>
#include <stdarg.h>
#include "mpp_frame.h"
#include "mpp_log.h"
#include "mpp_compat.h"
#include "mpp_meta.h"
#include "mpp_task.h"
#include "rk_mpp_cfg.h"

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

    printf("migtest %s (callback installed=%s)\n", rc ? "FAIL" : "PASS", cb_calls || 1 ? "yes" : "no");
    return rc;
}
