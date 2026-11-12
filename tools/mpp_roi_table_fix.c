/*
 * LD_PRELOAD helper for capturing and normalising the H.264 ROI table.
 *
 * MPP's H.264 GDR path builds its force-intra table on the CPU immediately
 * before mpp_buffer_sync_end(). The stock implementation leaves several
 * bitfield members of its local table record uninitialised, so repeated C
 * runs can produce different bitstreams. This helper can capture each small
 * synced buffer whose words carry the ROI table's high flag bits and, with
 * MPP_ROI_TABLE_FIX=1, retain only the declared force_intra/qp_adj_en/amv_en
 * bits before the DMA sync. That gives a deterministic C oracle for the table
 * the vendor intended to submit.
 */

#define _GNU_SOURCE

#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

typedef void *MppBuffer;
typedef int MPP_RET;
typedef int RK_S32;

typedef MPP_RET (*sync_end_fn)(MppBuffer, RK_S32, const char *);
typedef void *(*get_ptr_fn)(MppBuffer, const char *);
typedef int (*get_fd_fn)(MppBuffer, const char *);
typedef size_t (*get_size_fn)(MppBuffer, const char *);

static sync_end_fn real_sync_end;
static get_ptr_fn real_get_ptr;
static get_fd_fn real_get_fd;
static get_size_fn real_get_size;

static int load_symbols(void)
{
    if (!real_sync_end)
        real_sync_end = (sync_end_fn)dlsym(RTLD_NEXT, "mpp_buffer_sync_end_f");
    if (!real_get_ptr)
        real_get_ptr =
            (get_ptr_fn)dlsym(RTLD_NEXT, "mpp_buffer_get_ptr_with_caller");
    if (!real_get_fd)
        real_get_fd =
            (get_fd_fn)dlsym(RTLD_NEXT, "mpp_buffer_get_fd_with_caller");
    if (!real_get_size)
        real_get_size =
            (get_size_fn)dlsym(RTLD_NEXT, "mpp_buffer_get_size_with_caller");
    return real_sync_end && real_get_ptr && real_get_fd && real_get_size;
}

static int looks_like_roi_table(const uint8_t *data, size_t size)
{
    const uint64_t flag_mask = UINT64_C(0xe000000000000000);
    size_t words = size / sizeof(uint64_t);

    if (size < 8 || size > 65536 || words * sizeof(uint64_t) != size)
        return 0;
    for (size_t index = 0; index < words; ++index) {
        uint64_t value;

        __builtin_memcpy(&value, data + index * sizeof(value), sizeof(value));
        if ((value & flag_mask) == UINT64_C(0x4000000000000000) ||
            (value & flag_mask) == UINT64_C(0x2000000000000000) ||
            (value & flag_mask) == UINT64_C(0x6000000000000000))
            return 1;
    }
    return 0;
}

static void normalize_roi_table(uint8_t *data, size_t size)
{
    const uint64_t declared_mask = UINT64_C(0xe000000000000000);

    for (size_t offset = 0; offset < size; offset += sizeof(uint64_t)) {
        uint64_t value;

        __builtin_memcpy(&value, data + offset, sizeof(value));
        value &= declared_mask;
        __builtin_memcpy(data + offset, &value, sizeof(value));
    }
}

MPP_RET mpp_buffer_sync_end_f(MppBuffer buffer, RK_S32 ro, const char *caller)
{
    const char *directory = getenv("MPP_BUFFER_DUMP");
    uint8_t *data;
    size_t size;
    int fd;

    if (!load_symbols())
        return -1;
    data = real_get_ptr(buffer, caller);
    fd = real_get_fd(buffer, caller);
    size = real_get_size(buffer, caller);
    if (data && size && looks_like_roi_table(data, size)) {
        char path[256];
        FILE *file;
        const char *fix = getenv("MPP_ROI_TABLE_FIX");
        int enabled = fix && fix[0] && fix[0] != '0';

        if (enabled)
            normalize_roi_table(data, size);
        if (directory) {
            mkdir(directory, 0700);
            snprintf(path, sizeof(path), "%s/roi_%d_%d_%zu.bin",
                     directory, (int)getpid(), fd, size);
            file = fopen(path, "wb");
            if (file) {
                fwrite(data, 1, size, file);
                fclose(file);
                fprintf(stderr,
                        "mpp_buffer_dump fd=%d size=%zu fixed=%d path=%s\n",
                        fd, size, enabled, path);
            }
        }
    }
    return real_sync_end(buffer, ro, caller);
}
