/*
 * LD_PRELOAD probe for encoder KEY_MOTION_INFO buffers.
 *
 * `mpi_enc_test` allocates its own 1200-byte H.264 or 1280-byte H.265 motion
 * buffer but never writes it to disk. This shim records those allocations and
 * dumps the final contents before the test releases them, so the native C MPP
 * and rk-mpp-rs compatibility paths can be compared byte-for-byte.
 */

#define _GNU_SOURCE

#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include <rk_mpi.h>

#ifdef mpp_buffer_put
#undef mpp_buffer_put
#endif

typedef MPP_RET (*buffer_get_fn)(MppBufferGroup, MppBuffer *, size_t,
                                 const char *, const char *);
typedef MPP_RET (*buffer_put_fn)(MppBuffer);
typedef MPP_RET (*buffer_put_caller_fn)(MppBuffer, const char *);
typedef void *(*buffer_ptr_fn)(MppBuffer, const char *);

struct tracked_buffer {
    MppBuffer buffer;
    size_t size;
};

static struct tracked_buffer tracked[8];
static unsigned int tracked_count;

static buffer_get_fn real_buffer_get;
static buffer_put_fn real_buffer_put;
static buffer_put_caller_fn real_buffer_put_caller;
static buffer_ptr_fn real_buffer_ptr;
static int trace_enabled;

static void resolve_symbols(void)
{
    if (!real_buffer_get)
        real_buffer_get = (buffer_get_fn)dlsym(RTLD_NEXT,
                                                "mpp_buffer_get_with_tag");
    if (!real_buffer_put)
        real_buffer_put = (buffer_put_fn)dlsym(RTLD_NEXT, "mpp_buffer_put");
    if (!real_buffer_put_caller)
        real_buffer_put_caller = (buffer_put_caller_fn)dlsym(
            RTLD_NEXT, "mpp_buffer_put_with_caller");
    if (!real_buffer_ptr)
        real_buffer_ptr = (buffer_ptr_fn)dlsym(
            RTLD_NEXT, "mpp_buffer_get_ptr_with_caller");
}

MPP_RET mpp_buffer_get_with_tag(MppBufferGroup group, MppBuffer *buffer,
                                size_t size, const char *tag,
                                const char *caller)
{
    resolve_symbols();
    trace_enabled = getenv("MPP_MD_INFO_TRACE") != NULL;
    if (!real_buffer_get)
        return MPP_NOK;

    MPP_RET ret = real_buffer_get(group, buffer, size, tag, caller);
    if (trace_enabled)
        fprintf(stderr, "mdinfo_probe get size=%zu caller=%s ret=%d\n", size,
                caller ? caller : "", ret);
    if (ret == MPP_OK && buffer && *buffer &&
        (size == 1200 || size == 1280) && tracked_count < 8) {
        tracked[tracked_count].buffer = *buffer;
        tracked[tracked_count].size = size;
        tracked_count++;
    }
    return ret;
}

MPP_RET mpp_buffer_put_with_caller(MppBuffer buffer, const char *caller)
{
    resolve_symbols();
    if (!real_buffer_put_caller && real_buffer_put)
        return real_buffer_put(buffer);
    if (!real_buffer_put_caller)
        return MPP_NOK;

    trace_enabled = getenv("MPP_MD_INFO_TRACE") != NULL;
    if (trace_enabled)
        fprintf(stderr, "mdinfo_probe put buffer=%p tracked=%u\n", buffer,
                tracked_count);
    const char *path = getenv("MPP_MD_INFO_DUMP");
    if (path && real_buffer_ptr) {
        for (unsigned int i = 0; i < tracked_count; ++i) {
            if (tracked[i].buffer != buffer)
                continue;

            void *data = real_buffer_ptr(buffer, __func__);
            if (data) {
                FILE *output = fopen(path, "wb");
                if (output) {
                    fwrite(data, 1, tracked[i].size, output);
                    fclose(output);
                }
            }
            tracked[i] = tracked[tracked_count - 1];
            tracked_count--;
            break;
        }
    }

    return real_buffer_put_caller(buffer, caller);
}

MPP_RET mpp_buffer_put(MppBuffer buffer)
{
    return mpp_buffer_put_with_caller(buffer, __func__);
}
