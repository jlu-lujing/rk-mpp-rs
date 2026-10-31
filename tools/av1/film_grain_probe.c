/*
 * Emit C MPP film-grain memory images for Rust differential tests.
 *
 * This reproduces vdpu_av1d_set_fgs() around the official noise generator.
 * It intentionally does not link the MPP hardware layer.
 */

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "film_grain_noise_table.h"

typedef struct {
    uint8_t scaling_lut_y[256];
    uint8_t scaling_lut_cb[256];
    uint8_t scaling_lut_cr[256];
    int16_t cropped_luma_grain_block[4096];
    int16_t cropped_chroma_grain_block[2048];
} FilmGrainMemory;

typedef struct {
    int bitdepth;
    uint8_t num_y_points;
    uint8_t num_cb_points;
    uint8_t num_cr_points;
    uint8_t chroma_scaling_from_luma;
    uint8_t grain_scaling_minus_8;
    uint8_t ar_coeff_lag;
    uint8_t ar_coeff_shift_minus_6;
    uint8_t grain_scale_shift;
    uint16_t grain_seed;
    uint8_t scaling_points_y[14][2];
    uint8_t scaling_points_cb[10][2];
    uint8_t scaling_points_cr[10][2];
    uint8_t ar_coeffs_y[24];
    uint8_t ar_coeffs_cb[25];
    uint8_t ar_coeffs_cr[25];
} GrainCase;

static void init_scaling_function(uint8_t scaling_points[][2],
                                  uint8_t num_points, uint8_t scaling_lut[256])
{
    int point;
    int i;

    if (!num_points) {
        memset(scaling_lut, 0, 256);
        return;
    }
    for (i = 0; i < scaling_points[0][0]; i++)
        scaling_lut[i] = scaling_points[0][1];
    for (point = 0; point < num_points - 1; point++) {
        int x;
        int delta_y = scaling_points[point + 1][1] - scaling_points[point][1];
        int delta_x = scaling_points[point + 1][0] - scaling_points[point][0];
        int64_t delta = delta_x ?
            delta_y * ((65536 + (delta_x >> 1)) / delta_x) : 0;
        for (x = 0; x < delta_x; x++) {
            scaling_lut[scaling_points[point][0] + x] =
                scaling_points[point][1] +
                (int)((x * delta + 32768) >> 16);
        }
    }
    for (i = scaling_points[num_points - 1][0]; i < 256; i++)
        scaling_lut[i] = scaling_points[num_points - 1][1];
}

static void build_memory(const GrainCase *test, FilmGrainMemory *memory)
{
    int32_t ar_coeffs_y[24];
    int32_t ar_coeffs_cb[25];
    int32_t ar_coeffs_cr[25];
    int32_t luma[73][82];
    int32_t cb[38][44];
    int32_t cr[38][44];
    int grain_center;
    int grain_min;
    int grain_max;
    int i;
    int j;

    memset(memory, 0, sizeof(*memory));
    init_scaling_function(test->scaling_points_y, test->num_y_points,
                          memory->scaling_lut_y);
    if (test->chroma_scaling_from_luma) {
        memcpy(memory->scaling_lut_cb, memory->scaling_lut_y, 256);
        memcpy(memory->scaling_lut_cr, memory->scaling_lut_y, 256);
    } else {
        init_scaling_function(test->scaling_points_cb, test->num_cb_points,
                              memory->scaling_lut_cb);
        init_scaling_function(test->scaling_points_cr, test->num_cr_points,
                              memory->scaling_lut_cr);
    }
    for (i = 0; i < 25; i++) {
        if (i < 24)
            ar_coeffs_y[i] = test->ar_coeffs_y[i] - 128;
        ar_coeffs_cb[i] = test->ar_coeffs_cb[i] - 128;
        ar_coeffs_cr[i] = test->ar_coeffs_cr[i] - 128;
    }

    grain_center = 128 << (test->bitdepth - 8);
    grain_min = -grain_center;
    grain_max = (256 << (test->bitdepth - 8)) - 1 - grain_center;
    GenerateLumaGrainBlock(luma, test->bitdepth, test->num_y_points,
                           test->grain_scale_shift, test->ar_coeff_lag,
                           ar_coeffs_y,
                           test->ar_coeff_shift_minus_6 + 6,
                           grain_min, grain_max, test->grain_seed);
    GenerateChromaGrainBlock(luma, cb, cr, test->bitdepth,
                             test->num_y_points, test->num_cb_points,
                             test->num_cr_points, test->grain_scale_shift,
                             test->ar_coeff_lag, ar_coeffs_cb,
                             ar_coeffs_cr,
                             test->ar_coeff_shift_minus_6 + 6,
                             grain_min, grain_max,
                             test->chroma_scaling_from_luma,
                             test->grain_seed);

    for (i = 0; i < 64; i++)
        for (j = 0; j < 64; j++)
            memory->cropped_luma_grain_block[i * 64 + j] =
                luma[i + 9][j + 9];
    for (i = 0; i < 32; i++)
        for (j = 0; j < 32; j++) {
            memory->cropped_chroma_grain_block[i * 64 + 2 * j] =
                cb[i + 6][j + 6];
            memory->cropped_chroma_grain_block[i * 64 + 2 * j + 1] =
                cr[i + 6][j + 6];
        }
}

static int write_image(const char *directory, int index,
                       const FilmGrainMemory *memory)
{
    char path[4096];
    FILE *file;

    if (snprintf(path, sizeof(path), "%s/film_grain_%02d.bin",
                 directory, index) >= (int)sizeof(path))
        return -1;
    file = fopen(path, "wb");
    if (!file) {
        fprintf(stderr, "open %s: %s\n", path, strerror(errno));
        return -1;
    }
    if (fwrite(memory, 1, sizeof(*memory), file) != sizeof(*memory)) {
        fprintf(stderr, "write %s\n", path);
        fclose(file);
        return -1;
    }
    return fclose(file);
}

int main(int argc, char **argv)
{
    GrainCase cases[3];
    int i;

    if (argc != 2) {
        fprintf(stderr, "usage: %s OUTPUT_DIR\n", argv[0]);
        return 2;
    }
    memset(cases, 0, sizeof(cases));

    cases[0].bitdepth = 8;
    cases[0].num_y_points = 3;
    cases[0].num_cb_points = 2;
    cases[0].num_cr_points = 2;
    cases[0].ar_coeff_lag = 2;
    cases[0].ar_coeff_shift_minus_6 = 1;
    cases[0].grain_scale_shift = 0;
    cases[0].grain_seed = 7391;
    cases[0].scaling_points_y[0][0] = 0;
    cases[0].scaling_points_y[0][1] = 12;
    cases[0].scaling_points_y[1][0] = 96;
    cases[0].scaling_points_y[1][1] = 80;
    cases[0].scaling_points_y[2][0] = 255;
    cases[0].scaling_points_y[2][1] = 35;
    cases[0].scaling_points_cb[0][0] = 8;
    cases[0].scaling_points_cb[0][1] = 24;
    cases[0].scaling_points_cb[1][0] = 240;
    cases[0].scaling_points_cb[1][1] = 67;
    cases[0].scaling_points_cr[0][0] = 16;
    cases[0].scaling_points_cr[0][1] = 64;
    cases[0].scaling_points_cr[1][0] = 220;
    cases[0].scaling_points_cr[1][1] = 18;
    for (i = 0; i < 24; i++)
        cases[0].ar_coeffs_y[i] = (uint8_t)(128 + (i % 9) - 4);
    for (i = 0; i < 25; i++) {
        cases[0].ar_coeffs_cb[i] = (uint8_t)(128 + (i % 7) - 3);
        cases[0].ar_coeffs_cr[i] = (uint8_t)(128 + (i % 5) - 2);
    }

    cases[1] = cases[0];
    cases[1].bitdepth = 10;
    cases[1].num_cb_points = 0;
    cases[1].num_cr_points = 0;
    cases[1].chroma_scaling_from_luma = 1;
    cases[1].ar_coeff_lag = 3;
    cases[1].ar_coeff_shift_minus_6 = 2;
    cases[1].grain_scale_shift = 2;
    cases[1].grain_seed = 0xa53c;

    cases[2] = cases[0];
    cases[2].bitdepth = 12;
    cases[2].num_y_points = 0;
    cases[2].num_cb_points = 1;
    cases[2].num_cr_points = 1;
    cases[2].ar_coeff_lag = 1;
    cases[2].ar_coeff_shift_minus_6 = 0;
    cases[2].grain_scale_shift = 3;
    cases[2].grain_seed = 0x1234;

    for (i = 0; i < 3; i++) {
        FilmGrainMemory memory;
        build_memory(&cases[i], &memory);
        if (write_image(argv[1], i, &memory))
            return 1;
    }
    return 0;
}
