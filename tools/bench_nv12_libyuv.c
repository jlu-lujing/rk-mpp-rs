#define _POSIX_C_SOURCE 200809L

/*
 * Compare libyuv NV12ToI420 with examples/bench_nv12_planar.rs on the same
 * tightly packed NV12 session. Build with libyuv headers and a runtime library:
 *
 * cc -O2 -I/usr/include /path/bench_nv12_libyuv.c -lyuv -o bench_nv12_libyuv
 *
 * The benchmark allocates a frame output per picture on both sides, then
 * prints an FNV-1a checksum over the complete planar output for verification.
 */

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "libyuv.h"

static double now_ms(void)
{
    struct timespec ts;

    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000.0 + ts.tv_nsec / 1000000.0;
}

static int compare_double(const void *left, const void *right)
{
    double a = *(const double *)left;
    double b = *(const double *)right;
    return (a > b) - (a < b);
}

int main(int argc, char **argv)
{
    if (argc < 5) {
        fprintf(stderr, "usage: %s <session.nv12> <width> <height> <frames> [samples]\n",
                argv[0]);
        return 2;
    }

    const char *path = argv[1];
    int width = atoi(argv[2]);
    int height = atoi(argv[3]);
    int frames = atoi(argv[4]);
    int samples = argc > 5 ? atoi(argv[5]) : 7;
    size_t pixels = (size_t)width * height;
    size_t frame_size = pixels * 3 / 2;
    size_t needed = frame_size * frames;
    FILE *input = fopen(path, "rb");
    if (!input) {
        fprintf(stderr, "%s: %s\n", path, strerror(errno));
        return 1;
    }
    uint8_t *source = malloc(needed);
    if (!source || fread(source, 1, needed, input) != needed) {
        fprintf(stderr, "%s: need %zu bytes\n", path, needed);
        free(source);
        fclose(input);
        return 1;
    }
    fclose(input);
    if (samples < 1 || frames < 1 || width < 2 || height < 2) {
        fprintf(stderr, "invalid geometry or sample count\n");
        free(source);
        return 1;
    }

    double *times = calloc(samples, sizeof(*times));
    if (!times) {
        free(source);
        return 1;
    }
    for (int sample = 0; sample < samples; sample++) {
        uint64_t checksum = 0;
        double started = now_ms();

        for (int frame = 0; frame < frames; frame++) {
            const uint8_t *nv12 = source + frame * frame_size;
            uint8_t *planar = malloc(frame_size);
            uint8_t *u = planar + pixels;
            uint8_t *v = u + pixels / 4;

            if (!planar) {
                free(times);
                free(source);
                return 1;
            }
            if (NV12ToI420(nv12, width, nv12 + pixels, width,
                           planar, width, u, width / 2, v, width / 2,
                           width, height)) {
                fprintf(stderr, "NV12ToI420 failed\n");
                free(planar);
                free(times);
                free(source);
                return 1;
            }
            checksum ^= planar[0] ^ planar[pixels / 2] ^ planar[frame_size - 1];
            free(planar);
        }

        times[sample] = now_ms() - started;
        if (checksum == UINT64_MAX)
            fprintf(stderr, "unreachable checksum %llu\n",
                    (unsigned long long)checksum);
    }
    qsort(times, samples, sizeof(*times), compare_double);
    double median = times[samples / 2];
    uint64_t checksum = UINT64_C(0xcbf29ce484222325);
    for (int frame = 0; frame < frames; frame++) {
        const uint8_t *nv12 = source + frame * frame_size;
        uint8_t *planar = malloc(frame_size);
        uint8_t *u = planar + pixels;
        uint8_t *v = u + pixels / 4;

        if (!planar ||
            NV12ToI420(nv12, width, nv12 + pixels, width,
                       planar, width, u, width / 2, v, width / 2,
                       width, height)) {
            fprintf(stderr, "checksum conversion failed\n");
            free(planar);
            free(times);
            free(source);
            return 1;
        }
        for (size_t index = 0; index < frame_size; index++) {
            checksum ^= planar[index];
            checksum *= UINT64_C(0x100000001b3);
        }
        free(planar);
    }
    printf("libyuv NV12ToI420 %dx%d %d frames: %.3f ms (%.1f fps), fnv=%016llx\n",
           width, height, frames, median, frames * 1000.0 / median,
           (unsigned long long)checksum);

    free(times);
    free(source);
    return 0;
}
