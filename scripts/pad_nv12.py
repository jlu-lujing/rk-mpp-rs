#!/usr/bin/env python3
"""Copy tight NV12 into a zero-padded row-stride buffer."""

import sys


def main() -> int:
    if len(sys.argv) != 6:
        print("usage: pad_nv12.py <in.nv12> <out.nv12> <width> <height> <stride>")
        return 2
    source, destination = sys.argv[1], sys.argv[2]
    width, height, stride = map(int, sys.argv[3:])
    if width <= 0 or height <= 0 or stride < width or (height & 1):
        print("invalid NV12 geometry", file=sys.stderr)
        return 2

    data = open(source, "rb").read()
    tight = width * height * 3 // 2
    if len(data) < tight:
        print(f"{source}: short NV12 file, {len(data)} < {tight}", file=sys.stderr)
        return 1

    output = bytearray(stride * height * 3 // 2)
    for row in range(height):
        src = row * width
        dst = row * stride
        output[dst : dst + width] = data[src : src + width]

    chroma_in = width * height
    chroma_out = stride * height
    for row in range(height // 2):
        src = chroma_in + row * width
        dst = chroma_out + row * stride
        output[dst : dst + width] = data[src : src + width]

    open(destination, "wb").write(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
