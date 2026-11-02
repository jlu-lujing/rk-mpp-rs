/*
 * LD_PRELOAD probe for dma-heap allocation and dma-buf sync calls.
 *
 * Build on an aarch64 board:
 *   cc -shared -fPIC -O2 -o libdma_heap_trace.so dma_heap_trace.c -ldl
 *
 * Run with DMA_TRACE_DIR set. One trace file is written per process.
 */
#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/types.h>
#include <unistd.h>

struct dma_heap_allocation_data {
    uint64_t len;
    uint32_t fd;
    uint32_t fd_flags;
    uint64_t heap_flags;
};

struct dma_buf_sync {
    uint64_t flags;
};

#define DMA_HEAP_IOCTL_ALLOC _IOWR('H', 0, struct dma_heap_allocation_data)
#define DMA_BUF_IOCTL_SYNC _IOW('b', 0, struct dma_buf_sync)

typedef int (*real_ioctl_fn)(int, unsigned long, ...);

static real_ioctl_fn real_ioctl;
static FILE *trace_file;
static unsigned long trace_sequence;

static void open_trace(void)
{
    const char *directory = getenv("DMA_TRACE_DIR");
    char path[1024];

    if (!directory || !*directory)
        directory = "/tmp/dma_trace";
    snprintf(path, sizeof(path), "mkdir -p '%s'", directory);
    if (system(path) != 0)
        return;
    snprintf(path, sizeof(path), "%s/dma_%d.txt", directory, (int)getpid());
    trace_file = fopen(path, "we");
}

static void ensure_real_ioctl(void)
{
    if (!real_ioctl)
        real_ioctl = (real_ioctl_fn)dlsym(RTLD_NEXT, "ioctl");
}

int ioctl(int fd, unsigned long request, ...)
{
    va_list arguments;
    void *argument;
    int result;

    va_start(arguments, request);
    argument = va_arg(arguments, void *);
    va_end(arguments);

    ensure_real_ioctl();
    if (!real_ioctl) {
        errno = ENOSYS;
        return -1;
    }
    result = real_ioctl(fd, request, argument);

    if (!trace_file)
        open_trace();
    if (!trace_file)
        return result;

    if (request == DMA_HEAP_IOCTL_ALLOC) {
        const struct dma_heap_allocation_data *allocation = argument;

        fprintf(trace_file,
                "%04lu ALLOC heap_fd=%d len=%llu fd=%u fd_flags=%#x "
                "heap_flags=%#llx ret=%d errno=%d\n",
                trace_sequence++, fd,
                (unsigned long long)allocation->len, allocation->fd,
                allocation->fd_flags,
                (unsigned long long)allocation->heap_flags,
                result, result < 0 ? errno : 0);
    } else if (request == DMA_BUF_IOCTL_SYNC) {
        const struct dma_buf_sync *sync = argument;

        fprintf(trace_file, "%04lu SYNC fd=%d flags=%#llx ret=%d errno=%d\n",
                trace_sequence++, fd,
                (unsigned long long)sync->flags,
                result, result < 0 ? errno : 0);
    } else {
        return result;
    }
    fflush(trace_file);
    return result;
}
