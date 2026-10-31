/*
 * Emit C MPP's exact AV1 CDF ABI metadata and default table snapshots.
 *
 * Build this against the C MPP tree rather than the Rust crate. The resulting
 * files are independent test oracles for the Rust #[repr(C)] implementation.
 */

#include <errno.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "av1d_parser.h"

static int write_file(const char *dir, const char *name,
                      const void *data, size_t size)
{
    char path[4096];
    FILE *file;

    if (snprintf(path, sizeof(path), "%s/%s", dir, name) >= (int)sizeof(path)) {
        fprintf(stderr, "path too long: %s/%s\n", dir, name);
        return -1;
    }
    file = fopen(path, "wb");
    if (!file) {
        fprintf(stderr, "open %s: %s\n", path, strerror(errno));
        return -1;
    }
    if (fwrite(data, 1, size, file) != size) {
        fprintf(stderr, "write %s: %s\n", path, strerror(errno));
        fclose(file);
        return -1;
    }
    if (fclose(file) != 0) {
        fprintf(stderr, "close %s: %s\n", path, strerror(errno));
        return -1;
    }
    return 0;
}

int main(int argc, char **argv)
{
    static const unsigned q_indices[] = {0, 20, 21, 60, 61, 96, 120, 121};
    AV1CDFs cdfs;
    Av1MvCDFs ndvc;
    size_t i;

    if (argc != 2) {
        fprintf(stderr, "usage: %s OUTPUT_DIR\n", argv[0]);
        return 2;
    }

    printf("sizeof(AV1CDFs)=%zu\n", sizeof(AV1CDFs));
    printf("alignof(AV1CDFs)=%zu\n", _Alignof(AV1CDFs));
    printf("offsetof(AV1CDFs,mv_cdf)=%zu\n", offsetof(AV1CDFs, mv_cdf));
    printf("sizeof(Av1MvCDFs)=%zu\n", sizeof(Av1MvCDFs));
    printf("offsetof(AV1CDFs,obmc_cdf)=%zu\n", offsetof(AV1CDFs, obmc_cdf));
    printf("offsetof(AV1CDFs,dummy0)=%zu\n", offsetof(AV1CDFs, dummy0));
    printf("offsetof(AV1CDFs,palette_y_color_index_cdf)=%zu\n",
           offsetof(AV1CDFs, palette_y_color_index_cdf));
    printf("offsetof(AV1CDFs,tx_type_intra0_cdf)=%zu\n",
           offsetof(AV1CDFs, tx_type_intra0_cdf));
    printf("offsetof(AV1CDFs,txb_skip_cdf)=%zu\n",
           offsetof(AV1CDFs, txb_skip_cdf));
    printf("offsetof(AV1CDFs,coeff_br_cdf)=%zu\n",
           offsetof(AV1CDFs, coeff_br_cdf));
    printf("offsetof(AV1CDFs,dummy2)=%zu\n",
           offsetof(AV1CDFs, dummy2));
    printf("sizeof(DXVA_PicParams_AV1)=%zu\n", sizeof(DXVA_PicParams_AV1));
    printf("alignof(DXVA_PicParams_AV1)=%zu\n",
           _Alignof(DXVA_PicParams_AV1));
    printf("offsetof(DXVA_PicParams_AV1,width)=%zu\n",
           offsetof(DXVA_PicParams_AV1, width));
    printf("offsetof(DXVA_PicParams_AV1,coding)=%zu\n",
           offsetof(DXVA_PicParams_AV1, coding));
    printf("offsetof(DXVA_PicParams_AV1,tiles)=%zu\n",
           offsetof(DXVA_PicParams_AV1, tiles));
    printf("offsetof(DXVA_PicParams_AV1,format)=%zu\n",
           offsetof(DXVA_PicParams_AV1, format));
    printf("offsetof(DXVA_PicParams_AV1,loop_filter)=%zu\n",
           offsetof(DXVA_PicParams_AV1, loop_filter));
    printf("offsetof(DXVA_PicParams_AV1,segmentation)=%zu\n",
           offsetof(DXVA_PicParams_AV1, segmentation));
    printf("offsetof(DXVA_PicParams_AV1,film_grain)=%zu\n",
           offsetof(DXVA_PicParams_AV1, film_grain));
    printf("offsetof(DXVA_PicParams_AV1,ref_frame_idx)=%zu\n",
           offsetof(DXVA_PicParams_AV1, ref_frame_idx));
    printf("offsetof(DXVA_PicParams_AV1,frame_refs)=%zu\n",
           offsetof(DXVA_PicParams_AV1, frame_refs));
    printf("offsetof(DXVA_PicParams_AV1,frame_ref_state)=%zu\n",
           offsetof(DXVA_PicParams_AV1, frame_ref_state));
    printf("offsetof(DXVA_PicParams_AV1,ref_frame_sign_bias)=%zu\n",
           offsetof(DXVA_PicParams_AV1, ref_frame_sign_bias));
    printf("offsetof(DXVA_PicParams_AV1,upscaled_width)=%zu\n",
           offsetof(DXVA_PicParams_AV1, upscaled_width));
    printf("offsetof(DXVA_PicParams_AV1,cdfs)=%zu\n",
           offsetof(DXVA_PicParams_AV1, cdfs));
    printf("offsetof(DXVA_PicParams_AV1,tile_cols_log2)=%zu\n",
           offsetof(DXVA_PicParams_AV1, tile_cols_log2));

    memset(&cdfs, 0, sizeof(cdfs));
    memset(&ndvc, 0, sizeof(ndvc));
    if (av1d_set_default_cdfs(&cdfs, &ndvc) != MPP_OK) {
        fprintf(stderr, "av1d_set_default_cdfs failed\n");
        return 1;
    }
    if (write_file(argv[1], "cdf_noncoeff.bin", &cdfs, sizeof(cdfs)) ||
        write_file(argv[1], "cdf_ndvc.bin", &ndvc, sizeof(ndvc)))
        return 1;

    for (i = 0; i < sizeof(q_indices) / sizeof(q_indices[0]); i++) {
        char name[64];

        memset(&cdfs, 0, sizeof(cdfs));
        if (av1d_set_default_cdfs(&cdfs, &ndvc) != MPP_OK ||
            av1d_set_default_coeff_probs(q_indices[i], &cdfs) != MPP_OK) {
            fprintf(stderr, "initialize q=%u failed\n", q_indices[i]);
            return 1;
        }
        snprintf(name, sizeof(name), "cdf_q%03u.bin", q_indices[i]);
        if (write_file(argv[1], name, &cdfs, sizeof(cdfs)))
            return 1;
    }

    return 0;
}
