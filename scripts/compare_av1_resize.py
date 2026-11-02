#!/usr/bin/env python3
"""Compare an AV1 resolution-change stream after per-segment NV12 conversion."""

from __future__ import annotations

import sys
from pathlib import Path

SEGMENTS = {
    "av1_resize_320_to_640.ivf": [(320, 240, 10), (640, 480, 10)],
    "av1_resize_640_to_320.ivf": [(640, 480, 10), (320, 240, 10)],
}


def nv12_to_planar(data: bytes, width: int, height: int, frames: int) -> bytes:
    chroma_width = width // 2
    chroma_height = height // 2
    luma_size = width * height
    chroma_size = chroma_width * chroma_height
    frame_size = luma_size + 2 * chroma_size
    if len(data) != frame_size * frames:
        raise ValueError(
            f"{width}x{height}x{frames}: got {len(data)}, expected {frame_size * frames}"
        )
    output = bytearray()
    for frame in range(frames):
        source = data[frame * frame_size : (frame + 1) * frame_size]
        output += source[:luma_size]
        chroma = source[luma_size:]
        for component in range(2):
            for row in range(chroma_height):
                start = row * chroma_width * 2 + component
                output += chroma[start : start + chroma_width * 2 : 2]
    return bytes(output)


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__)
        return 2
    c_path = Path(sys.argv[1])
    rust_path = Path(sys.argv[2])
    name = sys.argv[3]
    try:
        c_data = c_path.read_bytes()
        planar = bytearray()
        offset = 0
        for width, height, frames in SEGMENTS[name]:
            frame_bytes = width * height * 3 // 2
            end = offset + frame_bytes * frames
            planar += nv12_to_planar(c_data[offset:end], width, height, frames)
            offset = end
        if offset != len(c_data):
            raise ValueError(f"C output has {len(c_data) - offset} trailing bytes")
        rust = rust_path.read_bytes()
        if bytes(planar) != rust:
            raise ValueError(
                f"planar bytes differ: C={len(planar)} Rust={len(rust)}"
            )
    except (OSError, ValueError) as error:
        print(f"FAIL {name}: {error}", file=sys.stderr)
        return 1
    print(f"PASS {name}: per-segment NV12 conversion, {len(planar)} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
