#!/usr/bin/env python3
"""Write a deterministic RGB input fixture for the JPEG encoder board runs.

    gen_rgb_fixture.py <out.bin> <width> <height> <rgb565|argb8888|bgra8888>

The row stride is the VEPU 8-pixel-aligned default the C harness picks
(align(width, 8) * bytes_per_pixel). Byte order follows the MPP format
documented meanings (inc_mpp_frame.h): MPP_FMT_RGB565 is what FFmpeg
calls rgb565be - the 565 value big-endian - and MPP_FMT_ARGB8888 is the
32-bit value 0xAARRGGBB stored in the little-endian machine's word order,
so its memory bytes are B,G,R,A (FFmpeg 'argb'). The layout only has to
be identical for both encoders to be comparable byte for byte; decoding
with FFmpeg keeps the convention honest.
"""
import struct
import sys


def main():
    path, width, height, fmt = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
    bpp = {"rgb565": 2, "argb8888": 4, "bgra8888": 4}[fmt]
    stride = ((width + 7) // 8 * 8) * bpp
    rows = []
    for y in range(height):
        row = bytearray(stride)
        for x in range(width):
            r = (x * 255) // (width - 1)
            g = (y * 255) // (height - 1)
            b = ((x + y) * 255) // (width + height - 2)
            off = x * bpp
            if fmt == "rgb565":
                value = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
                row[off:off + 2] = struct.pack(">H", value)
            elif fmt == "argb8888":
                row[off:off + 4] = struct.pack("<I", (255 << 24) | (r << 16) | (g << 8) | b)
            else:
                row[off:off + 4] = struct.pack("<I", (255 << 24) | (b << 16) | (g << 8) | r)
        rows.append(bytes(row))
    with open(path, "wb") as out:
        out.write(b"".join(rows))
    print(f"wrote {stride * height} bytes of {fmt} {width}x{height} to {path}")


if __name__ == "__main__":
    main()
