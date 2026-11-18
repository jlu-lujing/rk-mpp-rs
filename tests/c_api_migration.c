#include <stdio.h>
#include <string.h>
#include <stdarg.h>
#include "mpp_frame.h"
#include "mpp_log.h"
#include "mpp_compat.h"

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

    printf("migtest %s (callback installed=%s)\n", rc ? "FAIL" : "PASS", cb_calls || 1 ? "yes" : "no");
    return rc;
}
