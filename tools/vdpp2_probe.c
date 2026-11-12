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
    if (argc < 7 || argc > 10) {
        fprintf(stderr,
                "usage: %s <in.nv12> <out.yuv> <src_w> <src_h> <dst_w> <dst_h> [420] [hist] [enh] [custom]\n",
                argv[0]);
        return 2;
    }
    int src_w = atoi(argv[3]);
    int src_h = atoi(argv[4]);
    int dst_w = atoi(argv[5]);
    int dst_h = atoi(argv[6]);
    int dst_yuv420 = 0;
    int histogram_mode = 0;
    int enhancement = 0;
    int custom_enhancement = 0;
    int custom_sharpness = 0;
    for (int arg = 7; arg < argc; ++arg) {
        dst_yuv420 |= !strcmp(argv[arg], "420");
        histogram_mode |= !strcmp(argv[arg], "hist");
        enhancement |= !strcmp(argv[arg], "enh");
        custom_enhancement |= !strcmp(argv[arg], "custom");
        custom_sharpness |= !strcmp(argv[arg], "sharp");
    }
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
    FILE *hist_out = NULL;
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
    content.com2.cfg_set = enhancement ? 0 : 7u << 16;
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
    if (custom_enhancement) {
        memset(&content, 0, sizeof(content));
        content.dmsr.enable = true;
        content.dmsr.str_pri_y = 17;
        content.dmsr.str_sec_y = 9;
        content.dmsr.dumping_y = 11;
        content.dmsr.wgt_pri_gain_odd_1 = 7;
        content.dmsr.wgt_pri_gain_even_1 = 13;
        content.dmsr.wgt_pri_gain_odd_2 = 21;
        content.dmsr.wgt_pri_gain_even_2 = 29;
        content.dmsr.wgt_sec_gain = 11;
        content.dmsr.blk_flat_th = 123;
        content.dmsr.contrast_to_conf_map_x0 = 100;
        content.dmsr.contrast_to_conf_map_x1 = 900;
        content.dmsr.contrast_to_conf_map_y0 = 7;
        content.dmsr.contrast_to_conf_map_y1 = 60000;
        content.dmsr.diff_core_th0 = 3;
        content.dmsr.diff_core_th1 = 23;
        content.dmsr.diff_core_wgt0 = 9;
        content.dmsr.diff_core_wgt1 = 17;
        content.dmsr.diff_core_wgt2 = 25;
        for (int index = 0; index < 7; ++index) {
            content.dmsr.edge_th_low_arr[index] = index + 1;
            content.dmsr.edge_th_high_arr[index] = index + 3 + index * 9;
        }
        params.ptype = VDPP_PARAM_TYPE_DMSR;
        params.param = content;
        if (vdpp2_control(ctx, VDPP_CMD_SET_DMSR_CFG, &params)) {
            fprintf(stderr, "VDPP2 DMSR config failed\n");
            goto cleanup;
        }

        memset(&content, 0, sizeof(content));
        content.es.es_bEnabledES = 1;
        content.es.es_iAngleDelta = 20;
        content.es.es_iAngleDeltaExtra = 3;
        content.es.es_iGradNoDirTh = 41;
        content.es.es_iGradFlatTh = 99;
        content.es.es_iWgtGain = 200;
        content.es.es_iWgtDecay = 77;
        content.es.es_iLowConfTh = 111;
        content.es.es_iLowConfRatio = 22;
        content.es.es_iConfCntTh = 7;
        content.es.es_iWgtLocalTh = 100;
        content.es.es_iK1 = 0x2345;
        content.es.es_iK2 = 0x3456;
        content.es.es_iDeltaLimit = 0x4567;
        const RK_U32 custom_x[9] = {0, 500, 1000, 1500, 2000, 3000, 4000, 5000, 60000};
        const RK_U32 custom_y[9] = {0, 20, 50, 90, 140, 180, 220, 240, 255};
        memcpy(content.es.es_iDiff2conf_lut_x, custom_x, sizeof(custom_x));
        memcpy(content.es.es_iDiff2conf_lut_y, custom_y, sizeof(custom_y));
        content.es.es_bEndpointCheckEnable = 1;
        params.ptype = VDPP_PARAM_TYPE_ES;
        params.param = content;
        if (vdpp2_control(ctx, VDPP_CMD_SET_ES, &params)) {
            fprintf(stderr, "VDPP2 ES config failed\n");
            goto cleanup;
        }
    }
    if (custom_sharpness) {
        memset(&content, 0, sizeof(content));
        memcpy(&content.sharp, &api->params.shp_params, sizeof(content.sharp));
        content.sharp.sharp_enable = 1;
        content.sharp.sharp_coloradj_bypass_en = 0;
        content.sharp.lti_h_enable = 1;
        content.sharp.lti_h_slope = 123;
        content.sharp.lti_h_thresold = 37;
        content.sharp.lti_h_gain = 17;
        content.sharp.lti_h_noise_thr_pos = 900;
        content.sharp.lti_h_noise_thr_neg = 800;
        content.sharp.lti_v_enable = 1;
        content.sharp.lti_v_radius = 0;
        content.sharp.lti_v_slope = 111;
        content.sharp.lti_v_thresold = 41;
        content.sharp.lti_v_gain = 19;
        content.sharp.lti_v_noise_thr_pos = 901;
        content.sharp.lti_v_noise_thr_neg = 799;
        content.sharp.cti_h_enable = 1;
        content.sharp.cti_h_slope = 99;
        content.sharp.cti_h_thresold = 31;
        content.sharp.cti_h_gain = 11;
        content.sharp.cti_h_noise_thr_pos = 777;
        content.sharp.cti_h_noise_thr_neg = 666;
        content.sharp.peaking_gain = 333;
        for (int index = 0; index < 7; ++index) {
            content.sharp.peaking_coring_zero[index] = index + 2;
            content.sharp.peaking_coring_thr[index] = 20 + index;
            content.sharp.peaking_coring_ratio[index] = 900 + index * 10;
            content.sharp.peaking_gain_pos[index] = 300 + index * 20;
            content.sharp.peaking_gain_neg[index] = 280 + index * 20;
            content.sharp.peaking_limit_ctrl_pos0[index] = 60 + index;
            content.sharp.peaking_limit_ctrl_pos1[index] = 116 + index;
            content.sharp.peaking_limit_ctrl_neg0[index] = 58 + index;
            content.sharp.peaking_limit_ctrl_neg1[index] = 114 + index;
            content.sharp.peaking_limit_ctrl_ratio[index] = 96 + index;
        }
        content.sharp.peaking_filt_core_H0[0] = 5;
        content.sharp.peaking_filt_core_H0[1] = 15;
        content.sharp.peaking_filt_core_H0[2] = 25;
        content.sharp.peaking_filt_core_H1[0] = -8;
        content.sharp.peaking_filt_core_H1[1] = 1;
        content.sharp.peaking_filt_core_H1[2] = 31;
        content.sharp.peaking_filt_core_V0[0] = 0;
        content.sharp.peaking_filt_core_V0[1] = 5;
        content.sharp.peaking_filt_core_V0[2] = 7;
        content.sharp.peaking_edge_ctrl_enable = 1;
        content.sharp.peaking_edge_ctrl_non_dir_thr = 21;
        content.sharp.peaking_edge_ctrl_dir_cmp_ratio = 5;
        content.sharp.peaking_edge_ctrl_non_dir_wgt_offset = 73;
        content.sharp.peaking_edge_ctrl_non_dir_wgt_ratio = 19;
        content.sharp.peaking_edge_ctrl_dir_cnt_thr = 4;
        content.sharp.peaking_edge_ctrl_dir_cnt_avg = 5;
        content.sharp.peaking_edge_ctrl_dir_cnt_offset = 3;
        content.sharp.peaking_edge_ctrl_diag_dir_thr = 17;
        for (int index = 0; index < 8; ++index)
            content.sharp.peaking_edge_ctrl_diag_adj_gain_tab[index] = 5 + index;
        content.sharp.peaking_estc_delta_offset_h = 5;
        content.sharp.peaking_estc_alpha_over_h = 9;
        content.sharp.peaking_estc_alpha_under_h = 17;
        content.sharp.peaking_estc_alpha_over_unlimit_h = 65;
        content.sharp.peaking_estc_alpha_under_unlimit_h = 111;
        content.sharp.shootctrl_delta_offset = 17;
        content.sharp.shootctrl_alpha_over = 9;
        content.sharp.shootctrl_alpha_under = 10;
        content.sharp.shootctrl_alpha_over_unlimit = 109;
        content.sharp.shootctrl_alpha_under_unlimit = 108;
        content.sharp.global_gain_enable = 1;
        content.sharp.global_gain_lum_mode = 1;
        const RK_S32 lum_grd[] = {0, 100, 400, 700, 900, 1023};
        const RK_S32 lum_val[] = {20, 40, 60, 80, 100, 120};
        const RK_S32 adp_grd[] = {0, 20, 80, 200, 400, 1023};
        const RK_S32 adp_val[] = {30, 40, 50, 60, 70, 80};
        const RK_S32 var_grd[] = {0, 50, 120, 220, 520, 1023};
        const RK_S32 var_val[] = {25, 35, 45, 55, 65, 75};
        memcpy(content.sharp.global_gain_lum_grd, lum_grd, sizeof(lum_grd));
        memcpy(content.sharp.global_gain_lum_val, lum_val, sizeof(lum_val));
        memcpy(content.sharp.global_gain_adp_grd, adp_grd, sizeof(adp_grd));
        memcpy(content.sharp.global_gain_adp_val, adp_val, sizeof(adp_val));
        memcpy(content.sharp.global_gain_var_grd, var_grd, sizeof(var_grd));
        memcpy(content.sharp.global_gain_var_val, var_val, sizeof(var_val));
        content.sharp.color_ctrl_enable = 1;
        content.sharp.color_ctrl_p0_scaling_coef = 2;
        content.sharp.color_ctrl_p0_point_u = 110;
        content.sharp.color_ctrl_p0_point_v = 150;
        content.sharp.color_ctrl_p1_scaling_coef = 3;
        content.sharp.color_ctrl_p1_point_u = 80;
        content.sharp.color_ctrl_p1_point_v = 130;
        content.sharp.color_ctrl_p2_scaling_coef = 4;
        content.sharp.color_ctrl_p2_point_u = 120;
        content.sharp.color_ctrl_p2_point_v = 140;
        content.sharp.color_ctrl_p3_scaling_coef = 5;
        content.sharp.color_ctrl_p3_point_u = 130;
        content.sharp.color_ctrl_p3_point_v = 150;
        for (int index = 0; index < 16; ++index) {
            content.sharp.color_ctrl_p0_roll_tab[index] = index;
            content.sharp.color_ctrl_p1_roll_tab[index] = 31 - index;
            content.sharp.color_ctrl_p2_roll_tab[index] = 8 + index / 2;
            content.sharp.color_ctrl_p3_roll_tab[index] = 24 - index / 2;
        }
        content.sharp.tex_adj_enable = 1;
        content.sharp.tex_adj_y_mode_select = 2;
        content.sharp.tex_adj_mode_select = 1;
        const RK_S32 tex_grd[] = {0, 100, 300, 500, 700, 1023};
        const RK_S32 tex_val[] = {30, 45, 60, 75, 90, 110};
        memcpy(content.sharp.tex_adj_grd, tex_grd, sizeof(tex_grd));
        memcpy(content.sharp.tex_adj_val, tex_val, sizeof(tex_val));
        params.ptype = VDPP_PARAM_TYPE_SHARP;
        params.param = content;
        if (vdpp2_control(ctx, VDPP_CMD_SET_SHARP, &params)) {
            fprintf(stderr, "VDPP2 SHARP config failed\n");
            goto cleanup;
        }
    }
    memset(&src, 0, sizeof(src));
    memset(&dst, 0, sizeof(dst));
    src.mem_addr = src.uv_addr = mpp_buffer_get_fd(src_buf);
    src.uv_off = (uint32_t)src_w_vir * src_h_vir;
    dst.mem_addr = dst.uv_addr = mpp_buffer_get_fd(dst_buf);
    dst.uv_off = (uint32_t)dst_w_vir * dst_h_vir;
    memset(mpp_buffer_get_ptr(hist_buf), 0, 10240);
    mpp_buffer_sync_end(hist_buf);
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
    if (histogram_mode) {
        const char *hist_path = getenv("VDPP_HIST_PACKED_OUT");

        if (hist_path) {
            hist_out = fopen(hist_path, "wb");
            if (!hist_out)
                goto cleanup;
            mpp_buffer_sync_begin(hist_buf);
            size_t written = fwrite(mpp_buffer_get_ptr(hist_buf), 1, 10240, hist_out);
            mpp_buffer_sync_end(hist_buf);
            if (written != 10240) {
                fprintf(stderr, "histogram dump failed\n");
                goto cleanup;
            }
            fclose(hist_out);
            hist_out = NULL;
        }
    }
    ret = 0;

cleanup:
    if (initialized)
        vdpp2_deinit(ctx);
    if (hist_out)
        fclose(hist_out);
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
