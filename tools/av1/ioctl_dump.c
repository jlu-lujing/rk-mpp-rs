/*
 * LD_PRELOAD probe for mpp_service ioctls.
 *
 * Build on an aarch64 board:
 *   cc -shared -fPIC -O2 -o libmpp_ioctl_dump.so ioctl_dump.c -ldl
 *
 * Run:
 *   MPP_IOCTL_DUMP_DIR=/tmp/mpp_ioctl \
 *       LD_PRELOAD=$PWD/libmpp_ioctl_dump.so ./mpi_dec_test ...
 *
 * The wrapper writes each MPP_CMD_SET_REG_WRITE payload before the kernel sees
 * it. It also writes MPP_CMD_SET_REG_ADDR_OFFSET entries. Files are numbered in
 * ioctl order so a multi-message command can be compared with the complete C
 * hardware packet.
 */
#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MPP_IOC_CFG_V1 0x40047601UL
#define MPP_CMD_SET_REG_WRITE 0x200U
#define MPP_CMD_SET_REG_ADDR_OFFSET 0x202U
#define MPP_FLAGS_MULTI_MSG 0x1U
#define MPP_FLAGS_LAST_MSG 0x2U
#define MAX_REQ_NUM 16U

struct mpp_request {
    uint32_t command;
    uint32_t flags;
    uint32_t size;
    uint32_t offset;
    uint64_t data;
};

struct reg_offset {
    uint32_t reg_idx;
    uint32_t offset;
};

typedef int (*real_ioctl_fn)(int, unsigned long, ...);

static real_ioctl_fn real_ioctl;
static unsigned long dump_sequence;

static void ensure_directory(const char *path)
{
    char command[1024];

    if (!path || !*path)
        path = "/tmp/mpp_ioctl";
    snprintf(command, sizeof(command), "mkdir -p '%s'", path);
    if (system(command) != 0)
        fprintf(stderr, "mpp ioctl dump: cannot create %s\n", path);
}

static const char *dump_directory(void)
{
    static const char *directory;
    static int initialized;

    if (!initialized) {
        directory = getenv("MPP_IOCTL_DUMP_DIR");
        if (!directory || !*directory)
            directory = "/tmp/mpp_ioctl";
        ensure_directory(directory);
        initialized = 1;
    }
    return directory;
}

static FILE *open_dump(const char *name)
{
    char path[1024];

    snprintf(path, sizeof(path), "%s/%s", dump_directory(), name);
    return fopen(path, "we");
}

static void dump_registers(const struct mpp_request *request,
                           unsigned long sequence)
{
    const uint32_t *words = (const uint32_t *)(uintptr_t)request->data;
    uint32_t count;
    uint32_t index;
    char name[64];
    FILE *file;

    if (!words || request->size == 0 || request->size % sizeof(uint32_t))
        return;

    count = request->size / sizeof(uint32_t);
    snprintf(name, sizeof(name), "reg_%04lu_in.txt", sequence);
    file = open_dump(name);
    if (!file) {
        fprintf(stderr, "mpp ioctl dump: cannot open %s: %s\n",
                name, strerror(errno));
        return;
    }
    for (index = 0; index < count; ++index)
        fprintf(file, "reg[%03u] = %08x\n", index, words[index]);
    fclose(file);
}

static void dump_offsets(const struct mpp_request *request,
                         unsigned long sequence)
{
    const struct reg_offset *entries =
        (const struct reg_offset *)(uintptr_t)request->data;
    uint32_t count;
    uint32_t index;
    char name[64];
    FILE *file;

    if (!entries || request->size == 0 ||
        request->size % sizeof(struct reg_offset))
        return;

    count = request->size / sizeof(struct reg_offset);
    snprintf(name, sizeof(name), "offsets_%04lu.txt", sequence);
    file = open_dump(name);
    if (!file)
        return;
    for (index = 0; index < count; ++index)
        fprintf(file, "reg[%03u] += 0x%08x\n",
                entries[index].reg_idx, entries[index].offset);
    fclose(file);
}

static void inspect_request(const struct mpp_request *first)
{
    const struct mpp_request *request = first;
    unsigned long sequence = dump_sequence++;
    unsigned count;

    for (count = 0; count < MAX_REQ_NUM; ++count) {
        if (request->command == MPP_CMD_SET_REG_WRITE)
            dump_registers(request, sequence);
        else if (request->command == MPP_CMD_SET_REG_ADDR_OFFSET)
            dump_offsets(request, sequence);

        if (!(request->flags & MPP_FLAGS_MULTI_MSG) ||
            (request->flags & MPP_FLAGS_LAST_MSG))
            break;
        ++request;
    }
}

int ioctl(int fd, unsigned long request, ...)
{
    struct mpp_request *message;
    va_list arguments;

    va_start(arguments, request);
    message = va_arg(arguments, struct mpp_request *);
    va_end(arguments);

    if (!real_ioctl) {
        real_ioctl = (real_ioctl_fn)dlsym(RTLD_NEXT, "ioctl");
        if (!real_ioctl) {
            errno = ENOSYS;
            return -1;
        }
    }

    if (request == MPP_IOC_CFG_V1 && message)
        inspect_request(message);

    return real_ioctl(fd, request, message);
}
