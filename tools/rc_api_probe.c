/*
 * Exercise MPP's user-defined rate-control API control plane without
 * encoding a frame. The same binary links to the vendor MPP oracle and to
 * librk_mpp_rs.so so their query/register/select behavior can be compared.
 */

#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "rk_mpi.h"
#include "mpp_rc_api.h"

#define MAX_BRIEFS 16

static const RcImplApi test_h264_api = {
    "test_h264e_rc",
    MPP_VIDEO_CodingAVC,
    0,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
};

static const RcImplApi direct_h264_api = {
    "direct_h264e_rc",
    MPP_VIDEO_CodingAVC,
    0,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
};

static void show_briefs(const char *tag, const RcApiBrief *briefs, RK_S32 count)
{
    RK_S32 i;

    printf("%s count=%d\n", tag, count);
    for (i = 0; i < count; i++)
        printf("%s brief=%d name=%s type=%d\n", tag, i,
               briefs[i].name ? briefs[i].name : "(null)", briefs[i].type);
}

static int query_all(MppApi *mpi, MppCtx ctx, const char *tag)
{
    RcApiBrief briefs[MAX_BRIEFS];
    RcApiQueryAll query;
    MPP_RET ret;

    memset(briefs, 0, sizeof(briefs));
    query.brief = briefs;
    query.max_count = MAX_BRIEFS;
    query.count = -1;
    ret = mpi->control(ctx, MPP_ENC_GET_RC_API_ALL, &query);
    printf("%s ret=%d\n", tag, ret);
    show_briefs(tag, briefs, query.count);
    return ret;
}

static int query_type(MppApi *mpi, MppCtx ctx, const char *tag, MppCodingType type)
{
    RcApiBrief briefs[MAX_BRIEFS];
    RcApiQueryType query;
    MPP_RET ret;

    memset(briefs, 0, sizeof(briefs));
    query.brief = briefs;
    query.max_count = MAX_BRIEFS;
    query.type = type;
    query.count = -1;
    ret = mpi->control(ctx, MPP_ENC_GET_RC_API_BY_TYPE, &query);
    printf("%s ret=%d\n", tag, ret);
    show_briefs(tag, briefs, query.count);
    return ret;
}

static int run_direct(void)
{
    RcApiBrief briefs[MAX_BRIEFS];
    RcApiQueryAll all;
    RcApiQueryType typed;
    MPP_RET ret;

    memset(briefs, 0, sizeof(briefs));
    all.brief = briefs;
    all.max_count = MAX_BRIEFS;
    all.count = -1;
    ret = rc_brief_get_all(&all);
    printf("direct-all ret=%d\n", ret);
    show_briefs("direct-all", briefs, all.count);

    ret = rc_api_add(&direct_h264_api);
    printf("direct-register ret=%d\n", ret);
    ret = rc_api_add(&direct_h264_api);
    printf("direct-replace ret=%d\n", ret);
    memset(briefs, 0, sizeof(briefs));
    all.count = -1;
    ret = rc_brief_get_all(&all);
    printf("direct-replaced ret=%d\n", ret);
    show_briefs("direct-replaced", briefs, all.count);

    memset(briefs, 0, sizeof(briefs));
    typed.brief = briefs;
    typed.max_count = MAX_BRIEFS;
    typed.type = MPP_VIDEO_CodingAVC;
    typed.count = -1;
    ret = rc_brief_get_by_type(&typed);
    printf("direct-avc ret=%d\n", ret);
    show_briefs("direct-avc", briefs, typed.count);
    return ret ? 4 : 0;
}

int main(int argc, char **argv)
{
    MppCtx ctx = NULL;
    MppApi *mpi = NULL;
    RcApiBrief current;
    RcApiBrief selected;
    MPP_RET ret;

    if (argc == 2 && !strcmp(argv[1], "direct"))
        return run_direct();

    ret = mpp_create(&ctx, &mpi);
    printf("mpp_create ret=%d\n", ret);
    if (ret)
        return 1;
    ret = mpp_init(ctx, MPP_CTX_ENC, MPP_VIDEO_CodingAVC);
    printf("mpp_init ret=%d\n", ret);
    if (ret)
        return 2;

    query_all(mpi, ctx, "all-before");
    query_type(mpi, ctx, "avc-before", MPP_VIDEO_CodingAVC);

    memset(&current, 0, sizeof(current));
    ret = mpi->control(ctx, MPP_ENC_GET_RC_API_CURRENT, &current);
    printf("get-current-before ret=%d name=%s type=%d\n", ret,
           current.name ? current.name : "(null)", current.type);

    ret = mpi->control(ctx, MPP_ENC_SET_RC_API_CFG, (void *)&test_h264_api);
    printf("register-custom ret=%d\n", ret);
    query_all(mpi, ctx, "all-after");
    ret = mpi->control(ctx, MPP_ENC_SET_RC_API_CFG, (void *)&test_h264_api);
    printf("replace-custom ret=%d\n", ret);
    query_all(mpi, ctx, "all-replaced");
    query_type(mpi, ctx, "avc-after", MPP_VIDEO_CodingAVC);

    selected.name = "test_h264e_rc";
    selected.type = MPP_VIDEO_CodingAVC;
    ret = mpi->control(ctx, MPP_ENC_SET_RC_API_CURRENT, &selected);
    printf("set-current-custom ret=%d\n", ret);
    memset(&current, 0, sizeof(current));
    ret = mpi->control(ctx, MPP_ENC_GET_RC_API_CURRENT, &current);
    printf("get-current-custom ret=%d name=%s type=%d\n", ret,
           current.name ? current.name : "(null)", current.type);

    selected.name = "default";
    selected.type = MPP_VIDEO_CodingAVC;
    ret = mpi->control(ctx, MPP_ENC_SET_RC_API_CURRENT, &selected);
    printf("set-current-default ret=%d\n", ret);
    memset(&current, 0, sizeof(current));
    ret = mpi->control(ctx, MPP_ENC_GET_RC_API_CURRENT, &current);
    printf("get-current-default ret=%d name=%s type=%d\n", ret,
           current.name ? current.name : "(null)", current.type);

    ret = mpp_destroy(ctx);
    printf("mpp_destroy ret=%d\n", ret);
    return ret ? 3 : 0;
}
