#ifndef RK_MPP_CUSTOM_RC_PROBE_H
#define RK_MPP_CUSTOM_RC_PROBE_H

#ifndef CUSTOM_RC_CODING
#define CUSTOM_RC_CODING MPP_VIDEO_CodingHEVC
#endif

#ifndef CUSTOM_RC_HAS_BIT_TARGET_FIX
#define CUSTOM_RC_HAS_BIT_TARGET_FIX 1
#endif

typedef struct CustomRcCtx_t {
    FILE *log;
    int fps;
    int frame_bits;
    unsigned frame;
} CustomRcCtx;

static MPP_RET custom_rc_init(void *opaque, RcCfg *cfg)
{
    CustomRcCtx *ctx = (CustomRcCtx *)opaque;
    const char *path = getenv("CUSTOM_RC_LOG");

    if (!ctx->log)
        ctx->log = path ? fopen(path, "w") : stderr;
    ctx->fps = cfg->fps.fps_out_num / cfg->fps.fps_out_denom;
    ctx->frame_bits = cfg->bps_target / ctx->fps;
    fprintf(ctx->log, "init size=%dx%d mode=%d fps=%d bits=%d\n",
            cfg->width, cfg->height, cfg->mode, ctx->fps, ctx->frame_bits);
    return MPP_OK;
}

static MPP_RET custom_rc_deinit(void *opaque)
{
    CustomRcCtx *ctx = (CustomRcCtx *)opaque;

    if (ctx->log && ctx->log != stderr)
        fclose(ctx->log);
    ctx->log = NULL;
    return MPP_OK;
}

static MPP_RET custom_rc_task(void *opaque, EncRcTask *task, const char *stage)
{
    CustomRcCtx *ctx = (CustomRcCtx *)opaque;

#if CUSTOM_RC_HAS_BIT_TARGET_FIX
    fprintf(ctx->log,
            "%s seq=%d intra=%u idr=%u bit=%d bit_fix=%d "
            "qp=%d qp_min=%d qp_max=%d "
            "real_bits=%d real_qp=%d\n",
            stage, task->cpb.seq_idx, task->frm.is_intra, task->frm.is_idr,
            task->info.bit_target, task->info.bit_target_fix,
            task->info.quality_target,
            task->info.quality_min, task->info.quality_max,
            task->info.bit_real, task->info.quality_real);
#else
    fprintf(ctx->log,
            "%s seq=%d intra=%u idr=%u bit=%d "
            "qp=%d qp_min=%d qp_max=%d "
            "real_bits=%d real_qp=%d\n",
            stage, task->cpb.seq_idx, task->frm.is_intra, task->frm.is_idr,
            task->info.bit_target, task->info.quality_target,
            task->info.quality_min, task->info.quality_max,
            task->info.bit_real, task->info.quality_real);
#endif
    return MPP_OK;
}

static MPP_RET custom_rc_check_drop(void *opaque, EncRcTask *task)
{
    return custom_rc_task(opaque, task, "drop");
}

static MPP_RET custom_rc_check_reenc(void *opaque, EncRcTask *task)
{
    return custom_rc_task(opaque, task, "reenc");
}

static MPP_RET custom_rc_frame_start(void *opaque, EncRcTask *task)
{
    CustomRcCtx *ctx = (CustomRcCtx *)opaque;
    int qp_min = getenv("CUSTOM_RC_QP_MIN") ? atoi(getenv("CUSTOM_RC_QP_MIN")) : 26;
    int qp_max = getenv("CUSTOM_RC_QP_MAX") ? atoi(getenv("CUSTOM_RC_QP_MAX")) : 26;

    task->info.frame_type = task->frm.is_intra ? INTRA_FRAME : INTER_P_FRAME;
    task->info.bit_target = ctx->frame_bits;
    task->info.bit_max = ctx->frame_bits;
    task->info.bit_min = ctx->frame_bits;
    task->info.quality_target = 26;
    task->info.quality_max = qp_max;
    task->info.quality_min = qp_min;
    ctx->frame++;
    return custom_rc_task(opaque, task, "frame-start");
}

static MPP_RET custom_rc_frame_end(void *opaque, EncRcTask *task)
{
    return custom_rc_task(opaque, task, "frame-end");
}

static MPP_RET custom_rc_hal_start(void *opaque, EncRcTask *task)
{
    return custom_rc_task(opaque, task, "hal-start");
}

static MPP_RET custom_rc_hal_end(void *opaque, EncRcTask *task)
{
    return custom_rc_task(opaque, task, "hal-end");
}

static const RcImplApi custom_rc_api = {
    "custom_probe",
    CUSTOM_RC_CODING,
    sizeof(CustomRcCtx),
    custom_rc_init,
    custom_rc_deinit,
    custom_rc_check_drop,
    custom_rc_check_reenc,
    custom_rc_frame_start,
    custom_rc_frame_end,
    custom_rc_hal_start,
    custom_rc_hal_end,
};

#endif
