/*
 * Record what the reference library's MppDataV2 helpers actually compute, so the
 * Rust port can be checked against the implementation the encoder was verified
 * with rather than against a reading of rc_base.cpp.
 *
 * The functions are exported by librockchip_mpp, and the struct layout is public
 * in mpp/codec/rc/rc_base.h, so the probe can read the positions back too. That
 * matters: sum alone would not tell a wrong pos_w from a wrong sum.
 *
 * build on the board:
 *   cc -O2 /tmp/rc_data_probe.c /tmp/mpp-old/build/mpp/librockchip_mpp.so.0 \
 *      -o /tmp/rc_data_probe
 */
#include <stdio.h>
#include <stdlib.h>

typedef int RK_S32;
typedef long long RK_S64;

typedef struct MppDataV2_t {
    RK_S32 size;
    RK_S32 pos_r;
    RK_S32 pos_pw;
    RK_S32 pos_w;
    RK_S32 pos_ahead;
    RK_S64 sum;
    RK_S32 val[];
} MppDataV2;

extern int mpp_data_init_v2(MppDataV2 **p, RK_S32 len, RK_S32 value);
extern void mpp_data_reset_v2(MppDataV2 *p, RK_S32 val);
extern void mpp_data_preset_v2(MppDataV2 *p, RK_S32 val);
extern void mpp_data_update_v2(MppDataV2 *p, RK_S32 val);
extern RK_S32 mpp_data_sum_v2(MppDataV2 *p);
extern RK_S32 mpp_data_mean_v2(MppDataV2 *p);
extern RK_S32 mpp_data_get_pre_val_v2(MppDataV2 *p, RK_S32 idx);
extern RK_S32 mpp_data_sum_with_ratio_v2(MppDataV2 *p, RK_S32 len, RK_S32 num,
                                         RK_S32 denom);

/* The window lengths rc_model_v2.c actually allocates (2, 5, 8) plus two above
 * the 8-entry boundary where rc_base.h says the loop-save mode starts. */
static const int SIZES[] = {2, 5, 8, 16, 64};
static unsigned int g_seed = 20240425u;

static int next_val(int spread)
{
    g_seed = g_seed * 1103515245u + 12345u;
    return (int)((g_seed >> 7) % (unsigned)spread);
}

/* sum() truncates the 64-bit accumulator to 32 bits, so the probe records both
 * and a port that keeps 64 bits throughout would show up here. */
static void dump(MppDataV2 *p, const char *op, RK_S32 v)
{
    printf("%d %s %d "
           "sum64=%lld sum=%d mean=%d pre1=%d pre2=%d "
           "swr=%d,%d,%d pos_w=%d pos_r=%d pos_pw=%d pos_ahead=%d\n",
           p->size, op, v, p->sum, mpp_data_sum_v2(p), mpp_data_mean_v2(p),
           mpp_data_get_pre_val_v2(p, -1), mpp_data_get_pre_val_v2(p, -2),
           mpp_data_sum_with_ratio_v2(p, p->size, 3, 2),
           mpp_data_sum_with_ratio_v2(p, p->size, 2, 2),
           mpp_data_sum_with_ratio_v2(p, 1, 5, 4),
           p->pos_w, p->pos_r, p->pos_pw, p->pos_ahead);
}

int main(void)
{
    for (unsigned s = 0; s < sizeof(SIZES) / sizeof(SIZES[0]); s++) {
        MppDataV2 *p = NULL;
        if (mpp_data_init_v2(&p, SIZES[s], 0))
            return 1;
        printf("# size=%d\n", SIZES[s]);
        dump(p, "init", 0);

        /* Three passes over the window, preset and update interleaved so both
         * the pos_ahead path and the plain write path are covered at every
         * position, including the wrap. */
        for (int step = 0; step < SIZES[s] * 3; step++) {
            int v = next_val(4000) - 200;
            switch (step % 3) {
            case 0:
                mpp_data_preset_v2(p, v);
                dump(p, "preset", v);
                break;
            case 1:
                mpp_data_update_v2(p, v);
                dump(p, "update", v);
                break;
            default:
                mpp_data_update_v2(p, v * 2);
                dump(p, "update2", v * 2);
                break;
            }
        }

        mpp_data_reset_v2(p, 7);
        dump(p, "reset", 7);
        mpp_data_reset_v2(p, -3);
        dump(p, "reset_neg", -3);
        mpp_data_update_v2(p, next_val(1000));
        dump(p, "after_reset", 0);
        free(p);
    }
    return 0;
}
