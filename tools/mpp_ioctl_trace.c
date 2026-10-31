/*
 * LD_PRELOAD tracer for the Rockchip mpp_service ioctl ABI.
 *
 * This is a diagnostic tool for comparing the official C stack with the Rust
 * driver. It records every request in the contiguous MULTI_MSG chain and dumps
 * register or offset payloads as little-endian 32-bit words.
 */

#define _GNU_SOURCE

#include <dlfcn.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>

#define MPP_IOC_CFG_V1 0x40047601UL
#define MPP_FLAGS_MULTI_MSG 0x00000001U
#define MPP_FLAGS_LAST_MSG 0x00000002U

typedef struct {
    uint32_t cmd;
    uint32_t flag;
    uint32_t size;
    uint32_t offset;
    uint64_t data_ptr;
} MppReqV1;

typedef int (*ioctl_fn)(int, unsigned long, ...);

static ioctl_fn real_ioctl;

typedef struct {
    const void *ptr;
    uint32_t size;
    uint32_t offset;
} ReadRequest;

static ReadRequest pending_reads[32];
static unsigned int pending_read_count;

static void dump_words(const char *tag, const void *data, uint32_t size)
{
    const uint32_t *words = data;
    uint32_t count = size / sizeof(*words);

    fprintf(stderr, "mpp_trace %s words=%u", tag, count);
    for (uint32_t i = 0; i < count; ++i)
        fprintf(stderr, " %08x", words[i]);
    fputc('\n', stderr);
}

static void log_request(const MppReqV1 *req, const char *phase)
{
    const void *data = (const void *)(uintptr_t)req->data_ptr;

    fprintf(stderr,
            "mpp_trace %s cmd=%03x flag=%02x size=%u offset=%u ptr=%p\n",
            phase, req->cmd, req->flag, req->size, req->offset, data);

    if (!data)
        return;

    switch (req->cmd) {
    case 0x200: /* SET_REG_WRITE */
        if (req->size >= 4)
            dump_words("write", data, req->size);
        break;
    case 0x201: /* SET_REG_READ */
        if (phase[0] == 'b' && pending_read_count < 32) {
            pending_reads[pending_read_count++] =
                (ReadRequest){ .ptr = data, .size = req->size, .offset = req->offset };
        }
        if (req->size >= 4)
            dump_words("read_before", data, req->size);
        break;
    case 0x202: /* SET_REG_ADDR_OFFSET; pairs of (reg, offset) */
    case 0x203: /* SET_RCB_INFO; pairs of (reg, size) */
        if (req->size >= 4)
            dump_words(req->cmd == 0x202 ? "offset" : "rcb", data, req->size);
        break;
    default:
        break;
    }
}

static void dump_completed_reads(void)
{
    for (unsigned int i = 0; i < pending_read_count; ++i) {
        char tag[64];
        snprintf(tag, sizeof(tag), "read_complete[%u]@%u", i,
                 pending_reads[i].offset);
        dump_words(tag, pending_reads[i].ptr, pending_reads[i].size);
    }
    pending_read_count = 0;
}

static void trace_chain(const MppReqV1 *chain, const char *phase)
{
    if (phase[0] == 'b' && chain[0].cmd != 0x300)
        pending_read_count = 0;
    for (unsigned int i = 0; i < 16; ++i) {
        const MppReqV1 *req = &chain[i];
        log_request(req, phase);
        if (req->flag & MPP_FLAGS_LAST_MSG)
            break;
        if (!(req->flag & MPP_FLAGS_MULTI_MSG))
            break;
    }
}

int ioctl(int fd, unsigned long request, ...)
{
    va_list args;
    void *arg;

    va_start(args, request);
    arg = va_arg(args, void *);
    va_end(args);

    if (!real_ioctl) {
        real_ioctl = (ioctl_fn)dlsym(RTLD_NEXT, "ioctl");
        if (!real_ioctl)
            return -1;
    }

    if (request != MPP_IOC_CFG_V1 || !arg)
        return real_ioctl(fd, request, arg);

    MppReqV1 *chain = arg;
    trace_chain(chain, "before");
    int ret = real_ioctl(fd, request, arg);
    if (chain[0].cmd == 0x300 && ret == 0)
        dump_completed_reads();
    trace_chain(chain, "after");
    return ret;
}
