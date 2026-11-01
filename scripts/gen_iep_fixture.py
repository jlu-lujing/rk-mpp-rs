#!/usr/bin/env python3
"""Deterministic NV12 (YUV420-SP) fixture for the IEP2 deinterlace comparison.

    python3 scripts/gen_iep_fixture.py <out.yuv> <width> <height> [frames]

The pattern is row- and frame-indexed so a wrong field order or a one-frame
slip changes bytes; I5O2 wants prev/current/next, so three frames by default.
"""
import sys


def main() -> int:
    if len(sys.argv) < 4:
        print(__doc__)
        return 2
    path, w, h = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    n_frames = int(sys.argv[4]) if len(sys.argv) > 4 else 3
    with open(path, "wb") as f:
        for n in range(n_frames):
            for y in range(h):
                f.write(bytes(((y * 3 + n * 7) & 0xFF) for _ in range(w)))
            for cy in range(h // 2):
                f.write(
                    bytes((128 + ((cy * 5 + n * 3) & 0x1F) - 16) & 0xFF for _ in range(w))
                )
    return 0


if __name__ == "__main__":
    sys.exit(main())
