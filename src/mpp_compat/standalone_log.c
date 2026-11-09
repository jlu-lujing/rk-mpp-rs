#include <stdarg.h>
#include <stdio.h>

static void emit(FILE *stream, int level, const char *tag,
                 const char *fmt, const char *func, va_list args)
{
    (void)level;
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
