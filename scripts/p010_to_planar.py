#!/usr/bin/env python3
"""Rewrite rkvdec's 10-bit output into fully planar yuv420p10le.

The hardware writes semi-planar with the 10-bit samples LSB-aligned in
little-endian 16-bit words (probe: the first Y words read 0x012e, 0x0129;
the first chroma pairs interleave U,V). FFmpeg's p010 is MSB-aligned, so
its conversion shifts every sample down by six bits; this one does not.

The Rust decoder's own output is the planar form this script produces, so
the matrix can compare the result byte for byte.

    python3 scripts/p010_to_planar.py in.raw out.planar width height

Odd widths and heights are handled the way the decoder does it: chroma
planes cover ceil(w/2) x ceil(h/2) samples.
"""
import sys


def main() -> int:
    if len(sys.argv) != 5:
        print(__doc__)
        return 2
    src, dst, width, height = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
    data = open(src, "rb").read()
    y_w, y_h = width * 2, height
    c_w = ((width + 1) // 2) * 2
    c_h = (height + 1) // 2
    frame = y_w * y_h + c_w * c_h * 2
    out = bytearray()
    for start in range(0, len(data) - frame + 1, frame):
        y_end = start + width * 2 * height
        chroma = data[y_end : start + frame]
        out += data[start:y_end]
        for plane in (0, 1):
            for row in range(c_h):
                base = row * c_w * 2
                for col in range((width + 1) // 2):
                    word = base + col * 4 + plane * 2
                    if word + 2 <= len(chroma):
                        out += chroma[word : word + 2]
    open(dst, "wb").write(bytes(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
