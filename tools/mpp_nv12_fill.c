#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static int parse_u32(const char *text, uint32_t *value)
{
    char *end = NULL;
    unsigned long parsed;

    errno = 0;
    parsed = strtoul(text, &end, 10);
    if (errno || !end || *end || parsed > UINT32_MAX)
        return -1;
    *value = (uint32_t)parsed;
    return 0;
}

int main(int argc, char **argv)
{
    uint32_t width, height, frames;
    size_t row_bytes;
    uint8_t *luma;
    uint8_t *chroma;
    FILE *output;
    uint32_t frame, x, y;

    if (argc != 5) {
        fprintf(stderr, "usage: %s <width> <height> <frames> <out.nv12>\n",
                argv[0]);
        return 2;
    }
    if (parse_u32(argv[1], &width) || parse_u32(argv[2], &height) ||
        parse_u32(argv[3], &frames) || !width || !height || !frames ||
        (width & 1) || (height & 1)) {
        fprintf(stderr, "width and height must be non-zero and even\n");
        return 2;
    }

    row_bytes = width;
    luma = malloc(row_bytes);
    chroma = malloc(row_bytes);
    if (!luma || !chroma) {
        fprintf(stderr, "allocation failed\n");
        free(luma);
        free(chroma);
        return 1;
    }

    output = fopen(argv[4], "wb");
    if (!output) {
        perror(argv[4]);
        free(luma);
        free(chroma);
        return 1;
    }

    for (frame = 0; frame < frames; frame++) {
        for (y = 0; y < height; y++) {
            for (x = 0; x < width; x++)
                luma[x] = (uint8_t)(x + y + frame * 3);
            if (fwrite(luma, 1, row_bytes, output) != row_bytes)
                goto write_error;
        }
        for (y = 0; y < height / 2; y++) {
            for (x = 0; x < width / 2; x++) {
                chroma[x * 2] = (uint8_t)(128 + y + frame * 2);
                chroma[x * 2 + 1] = (uint8_t)(64 + x + frame * 5);
            }
            if (fwrite(chroma, 1, row_bytes, output) != row_bytes)
                goto write_error;
        }
    }

    free(luma);
    free(chroma);
    if (fclose(output)) {
        perror("close");
        return 1;
    }
    return 0;

write_error:
    perror("write");
    free(luma);
    free(chroma);
    fclose(output);
    return 1;
}
