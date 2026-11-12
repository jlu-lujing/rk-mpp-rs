/* Generate one VDPP2 parameter job for register or hardware comparison. */

#define _GNU_SOURCE

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mpp_buffer.h"
#include "vdpp2.h"

static int even_align(int value, int alignment)
{
    return (value + alignment - 1) / alignment * alignment;
}

int main(int argc, char **argv)
{
    if (argc != 7 && argc != 8) {
        fprintf(stderr,
                "usage: %s <in.nv12> <out.yuv> <src_w> <src_h> <dst_w> <dst_h> [420|444]\n",
                argv[0]);
        return 2;
    }
    int src_w = atoi(argv[3]);
    int src_h = atoi(argv[4]);
    int dst_w = atoi(argv[5]);
    int dst_h = atoi(argv[6]);
    int dst_yuv420 = argc == 8 && !strcmp(argv[7], "420");
    int histogram_mode = argc == 8 && !strcmp(argv[7], "hist");
    int src_w_vir = even_align(src_w, 16);
    int src_h_vir = even_align(src_h, 8);
    int dst_w_vir = even_align(dst_w, 16);
    int dst_h_vir = even_align(dst_h, 2);
    size_t src_size = (size_t)src_w_vir * src_h_vir * 3 / 2;
    size_t dst_size = (size_t)dst_w_vir * dst_h_vir * (dst_yuv420 ? 3 : 6) / 2;
    MppBufferGroup group = NULL;
    MppBuffer src_buf = NULL;
    MppBuffer dst_buf = NULL;
    MppBuffer hist_buf = NULL;
    FILE *in = fopen(argv[1], "rb");
    FILE *out = fopen(argv[2], "wb");
    struct vdpp2_api_ctx *api = calloc(1, sizeof(*api));
    VdppCtx ctx = api;
    union vdpp_api_content content;
    struct vdpp_api_params params;
    VdppImg src;
    VdppImg dst;
    int hist_fd;
    int initialized = 0;
    int ret = 1;

    if (!in || !out || !api) {
        fprintf(stderr, "open/alloc failed\n");
        goto cleanup;
    }
    if (mpp_buffer_group_get_internal(&group, MPP_BUFFER_TYPE_DRM) ||
        mpp_buffer_get(group, &src_buf, src_size) ||
        mpp_buffer_get(group, &dst_buf, dst_size) ||
        mpp_buffer_get(group, &hist_buf, 10240)) {
        fprintf(stderr, "buffer allocation failed\n");
        goto cleanup;
    }
    if (fread(mpp_buffer_get_ptr(src_buf), 1, src_size, in) != src_size) {
        fprintf(stderr, "short input\n");
        goto cleanup;
    }
    mpp_buffer_sync_end(src_buf);

    memset(&content, 0, sizeof(content));
    content.com2.sfmt = MPP_FMT_YUV420SP;
    content.com2.sswap = 0;
    content.com2.dfmt = dst_yuv420 ? VDPP_FMT_YUV420 : VDPP_FMT_YUV444;
    content.com2.dswap = 0;
    content.com2.src_width = src_w;
    content.com2.src_height = src_h;
    content.com2.src_width_vir = src_w_vir;
    content.com2.src_height_vir = src_h_vir;
    content.com2.dst_width = dst_w;
    content.com2.dst_height = dst_h;
    content.com2.dst_width_vir = dst_w_vir;
    content.com2.dst_height_vir = dst_h_vir;
    content.com2.hist_mode_en = histogram_mode;
    content.com2.cfg_set = 7u << 16;
    params.ptype = VDPP_PARAM_TYPE_COM2;
    params.param = content;

    if (vdpp2_init(&ctx)) {
        fprintf(stderr, "VDPP2 init failed\n");
        goto cleanup;
    }
    initialized = 1;
    if (vdpp2_control(ctx, VDPP_CMD_SET_COM2_CFG, &params)) {
        fprintf(stderr, "VDPP2 init/config failed\n");
        goto cleanup;
    }
    memset(&src, 0, sizeof(src));
    memset(&dst, 0, sizeof(dst));
    src.mem_addr = src.uv_addr = mpp_buffer_get_fd(src_buf);
    src.uv_off = (uint32_t)src_w_vir * src_h_vir;
    dst.mem_addr = dst.uv_addr = mpp_buffer_get_fd(dst_buf);
    dst.uv_off = (uint32_t)dst_w_vir * dst_h_vir;
    hist_fd = mpp_buffer_get_fd(hist_buf);
    vdpp2_control(ctx, VDPP_CMD_SET_SRC, &src);
    vdpp2_control(ctx, VDPP_CMD_SET_DST, &dst);
    vdpp2_control(ctx, VDPP_CMD_SET_HIST_FD, &hist_fd);
    if (vdpp2_control(ctx, VDPP_CMD_RUN_SYNC, NULL)) {
        fprintf(stderr, "VDPP2 run failed\n");
        goto cleanup;
    }
    mpp_buffer_sync_begin(dst_buf);
    fwrite(mpp_buffer_get_ptr(dst_buf), 1, dst_size, out);
    mpp_buffer_sync_end(dst_buf);
    ret = 0;

cleanup:
    if (initialized)
        vdpp2_deinit(ctx);
    if (in)
        fclose(in);
    if (out)
        fclose(out);
    if (src_buf)
        mpp_buffer_put(src_buf);
    if (dst_buf)
        mpp_buffer_put(dst_buf);
    if (hist_buf)
        mpp_buffer_put(hist_buf);
    if (group)
        mpp_buffer_group_put(group);
    free(api);
    return ret;
}
