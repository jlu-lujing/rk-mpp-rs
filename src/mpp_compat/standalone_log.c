#include <stdarg.h>
#include <stdio.h>

/* Implemented in Rust: current MPP log level and optional callback. */
extern int rk_mpp_log_level_value(void);
extern void *rk_mpp_log_cb_ctx(void);
extern void *rk_mpp_log_cb_fn(void);

typedef void (*MppLogCb)(void *ctx, int level, const char *tag,
                         const char *fmt, const char *func, va_list args);

static void emit(FILE *stream, int level, const char *tag,
                 const char *fmt, const char *func, va_list args)
{
    if (level > rk_mpp_log_level_value())
        return;
    void *cb = rk_mpp_log_cb_fn();
    if (cb) {
        ((MppLogCb)cb)(rk_mpp_log_cb_ctx(), level, tag, fmt, func, args);
        return;
    }
    if (tag && tag[0])
        fprintf(stream, "%s: ", tag);
    if (func && func[0])
        fprintf(stream, "%s: ", func);
    vfprintf(stream, fmt, args);
}

void rk_mpp_vlog_l(int level, const char *tag, const char *fmt,
                   const char *func, ...)
{
    va_list args;

    va_start(args, func);
    emit(stderr, level, tag, fmt, func, args);
    va_end(args);
}

void rk_mpp_vlog(const char *tag, const char *fmt, const char *func, ...)
{
    va_list args;

    va_start(args, func);
    emit(stderr, 3, tag, fmt, func, args);
    va_end(args);
}

void rk_mpp_vllog(int level, const char *tag, const char *fmt,
                  const char *func, ...)
{
    va_list args;

    va_start(args, func);
    emit(stderr, level, tag, fmt, func, args);
    va_end(args);
}
