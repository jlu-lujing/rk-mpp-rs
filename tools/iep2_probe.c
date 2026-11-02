/*
 * Minimal IEP2 client-28 probe for deinterlace mode comparisons.
 *
 * Usage:
 *   iep2_probe in.yuv out.yuv width height mode field_order
 *
 * mode is the numeric enum IEP2_DIL_MODE; field_order is 0 (TFF) or 1 (BFF).
 * The input must contain three NV12/YUV420SP frames. Both destination buffers
 * are zeroed before the job and written verbatim afterwards, so an unused slot
 * remains observable instead of being inferred from the selected mode.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mpp_buffer.h"
#include "mpp_err.h"
#include "rk_mpi.h"

#include "iep_api.h"
#include "iep2_api.h"

#define ADD_OFFSET(base, offset) ((RK_U32)(base) + ((RK_U32)(offset) << 10))
static size_t frame_size(int w, int h)
{
    return (size_t)w * h * 3 / 2;
}

static void set_image(IepImg *img, int w, int h, RK_U32 base)
{
    size_t y_size = (size_t)w * h;
    img->mem_addr = base;
    img->uv_addr = ADD_OFFSET(base, y_size);
    img->v_addr = ADD_OFFSET(base, y_size + y_size / 4);
    img->act_w = w;
    img->act_h = h;
    img->x_off = 0;
    img->y_off = 0;
    img->vir_w = w;
    img->vir_h = h;
    img->format = IEP2_FMT_YUV420;
}

int main(int argc, char **argv)
{
    if (argc != 7) {
        fprintf(stderr, "usage: %s in.yuv out.yuv width height mode field_order\n",
                argv[0]);
        return 2;
    }
    const char *input_path = argv[1];
    const char *output_path = argv[2];
    int width = atoi(argv[3]);
    int height = atoi(argv[4]);
    int mode = atoi(argv[5]);
    int field_order = atoi(argv[6]);
    size_t size = frame_size(width, height);
    MppBufferGroup group = NULL;
    MppBuffer src[3] = { NULL, NULL, NULL };
    MppBuffer dst[2] = { NULL, NULL };
    FILE *input = NULL;
    FILE *output = NULL;
    iep_com_ctx *ctx = NULL;
    MPP_RET ret = MPP_NOK;

    if (width <= 0 || height <= 0 || width & 1 || height & 1)
        return 2;

    input = fopen(input_path, "rb");
    output = fopen(output_path, "wb");
    if (!input || !output) {
        perror("open");
        goto cleanup;
    }
    ctx = rockchip_iep2_api_alloc_ctx();
    if (!ctx)
        goto cleanup;

    if (mpp_buffer_group_get_internal(&group, MPP_BUFFER_TYPE_DRM))
        goto cleanup;
    for (int i = 0; i < 3; i++)
        if (mpp_buffer_get(group, &src[i], size))
            goto cleanup;
    for (int i = 0; i < 2; i++)
        if (mpp_buffer_get(group, &dst[i], size))
            goto cleanup;

    ctx->ops->init(&ctx->priv);

    // The API slots are current, next, previous. The test file is in temporal
    // order, so frame 0 lands in src[2], frame 1 in src[0].
    if (fread(mpp_buffer_get_ptr(src[2]), 1, size, input) != size ||
        fread(mpp_buffer_get_ptr(src[0]), 1, size, input) != size)
        goto cleanup;

    struct iep2_api_params params;
    memset(&params, 0, sizeof(params));
    params.ptype = IEP2_PARAM_TYPE_MODE;
    params.param.mode.dil_mode = (enum IEP2_DIL_MODE)mode;
    params.param.mode.out_mode = IEP2_OUT_MODE_LINE;
    params.param.mode.dil_order = (enum IEP2_FIELD_ORDER)field_order;
    if (ctx->ops->control(ctx->priv, IEP_CMD_SET_DEI_CFG, &params))
        goto cleanup;

    memset(&params, 0, sizeof(params));
    params.ptype = IEP2_PARAM_TYPE_COM;
    params.param.com.sfmt = IEP2_FMT_YUV420;
    params.param.com.sswap = IEP2_YUV_SWAP_SP_UV;
    params.param.com.dfmt = IEP2_FMT_YUV420;
    params.param.com.dswap = IEP2_YUV_SWAP_SP_UV;
    params.param.com.width = width;
    params.param.com.height = height;
    params.param.com.hor_stride = width;
    if (ctx->ops->control(ctx->priv, IEP_CMD_SET_DEI_CFG, &params))
        goto cleanup;

    if (fread(mpp_buffer_get_ptr(src[1]), 1, size, input) != size)
        goto cleanup;

    for (int i = 0; i < 3; i++) {
        IepImg image;
        memset(&image, 0, sizeof(image));
        set_image(&image, width, height, mpp_buffer_get_fd(src[i]));
        IepCmd cmd = i == 0 ? IEP_CMD_SET_SRC :
                     (i == 1 ? IEP_CMD_SET_DEI_SRC1 : IEP_CMD_SET_DEI_SRC2);
        if (ctx->ops->control(ctx->priv, cmd, &image))
            goto cleanup;
    }
    for (int i = 0; i < 2; i++) {
        IepImg image;
        memset(&image, 0, sizeof(image));
        set_image(&image, width, height, mpp_buffer_get_fd(dst[i]));
        IepCmd cmd = i == 0 ? IEP_CMD_SET_DST : IEP_CMD_SET_DEI_DST1;
        if (ctx->ops->control(ctx->priv, cmd, &image))
            goto cleanup;
    }

    memset(mpp_buffer_get_ptr(dst[0]), 0, size);
    memset(mpp_buffer_get_ptr(dst[1]), 0, size);

    {
        struct iep2_api_info info;
        memset(&info, 0, sizeof(info));
        if (ctx->ops->control(ctx->priv, IEP_CMD_RUN_SYNC, &info))
            goto cleanup;
    }

    if (fwrite(mpp_buffer_get_ptr(dst[0]), 1, size, output) != size ||
        fwrite(mpp_buffer_get_ptr(dst[1]), 1, size, output) != size)
        goto cleanup;
    ret = MPP_OK;

cleanup:
    if (ctx) {
        ctx->ops->deinit(ctx->priv);
        rockchip_iep2_api_release_ctx(ctx);
    }
    if (output)
        fclose(output);
    if (input)
        fclose(input);
    for (int i = 0; i < 2; i++)
        if (dst[i])
            mpp_buffer_put(dst[i]);
    for (int i = 0; i < 3; i++)
        if (src[i])
            mpp_buffer_put(src[i]);
    if (group)
        mpp_buffer_group_put(group);
    return ret == MPP_OK ? 0 : 1;
}
