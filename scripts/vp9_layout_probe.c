#include <stddef.h>
#include <stdio.h>

#include "vp9d_syntax.h"

int main(void)
{
    printf("sizeof=%zu\n", sizeof(DXVA_PicParams_VP9));
    printf("prob=%zu\n", offsetof(DXVA_PicParams_VP9, prob));
    printf("counts=%zu\n", offsetof(DXVA_PicParams_VP9, counts));
    printf("prob_flag_delta=%zu\n", offsetof(DXVA_PicParams_VP9, prob_flag_delta));
    printf("p_flag=%zu\n",
           offsetof(DXVA_PicParams_VP9, prob_flag_delta.p_flag));
    printf("p_delta=%zu\n",
           offsetof(DXVA_PicParams_VP9, prob_flag_delta.p_delta));
    printf("coef_flag=%zu\n",
           offsetof(DXVA_PicParams_VP9, prob_flag_delta.coef_flag));
    printf("coef_delta=%zu\n",
           offsetof(DXVA_PicParams_VP9, prob_flag_delta.coef_delta));
    printf("mvscale=%zu\n", offsetof(DXVA_PicParams_VP9, mvscale));
    printf("txmode=%zu\n", offsetof(DXVA_PicParams_VP9, txmode));
    printf("refmode=%zu\n", offsetof(DXVA_PicParams_VP9, refmode));
    printf("sizeof_prob=%zu\n", sizeof(((DXVA_PicParams_VP9 *)0)->prob));
    printf("sizeof_flag_delta=%zu\n",
           sizeof(((DXVA_PicParams_VP9 *)0)->prob_flag_delta));
    return 0;
}
