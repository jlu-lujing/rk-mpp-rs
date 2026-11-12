/*
 * LD_PRELOAD shim for generating VDPP2 register traces without client 29.
 *
 * Buffer allocation and address translation still go through the vendor MPP
 * library. Only mpp_service control ioctls are accepted locally so the register
 * builder can run on t6, which has no VDPP client.
 */

#define _GNU_SOURCE

#include <dlfcn.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>

#define MPP_IOC_CFG_V1 0x40047601UL

typedef struct {
    uint32_t cmd;
    uint32_t flag;
    uint32_t size;
    uint32_t offset;
    uint64_t data_ptr;
} MppReqV1;

typedef int (*ioctl_fn)(int, unsigned long, ...);
static ioctl_fn real_ioctl;

static void dump_words(const char *tag, const void *data, uint32_t size)
{
    const uint32_t *words = data;
    uint32_t count = size / sizeof(*words);

    fprintf(stderr, "mpp_trace %s words=%u", tag, count);
    for (uint32_t i = 0; i < count; ++i)
        fprintf(stderr, " %08x", words[i]);
    fputc('\n', stderr);
}

static int fake_control(MppReqV1 *request)
{
    void *data = (void *)(uintptr_t)request->data_ptr;

    fprintf(stderr,
            "mpp_trace fake cmd=%03x flag=%02x size=%u offset=%u\n",
            request->cmd, request->flag, request->size, request->offset);
    switch (request->cmd) {
    case 0x100:
        return 0;
    case 0x200:
        dump_words("write", data, request->size);
        return 0;
    case 0x201:
        if (request->size >= 44)
            ((uint32_t *)data)[10] = 1;
        dump_words("read_complete", data, request->size);
        return 0;
    case 0x202:
        dump_words("offset", data, request->size);
        return 0;
    case 0x300:
        return 0;
    default:
        return 0;
    }
}

static int fake_chain(MppReqV1 *chain)
{
    for (unsigned int index = 0; index < 16; ++index) {
        int ret = fake_control(&chain[index]);

        if (ret)
            return ret;
        if (!(chain[index].flag & 0x1) || (chain[index].flag & 0x2))
            break;
    }
    return 0;
}

int ioctl(int fd, unsigned long request, ...)
{
    va_list args;
    void *arg;

    va_start(args, request);
    arg = va_arg(args, void *);
    va_end(args);
    if (!real_ioctl)
        real_ioctl = (ioctl_fn)dlsym(RTLD_NEXT, "ioctl");
    if (request == MPP_IOC_CFG_V1 && arg)
        return fake_chain(arg);
    return real_ioctl(fd, request, arg);
}
