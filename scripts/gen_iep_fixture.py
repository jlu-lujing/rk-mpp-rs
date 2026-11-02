#!/usr/bin/env python3
"""Deterministic YUV fixture for the IEP2 deinterlace comparison.

    python3 scripts/gen_iep_fixture.py <out.yuv> <width> <height>
                                       [frames] [420sp|420spvu|420p|422sp|422spvu|422p]

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
    fmt = sys.argv[5] if len(sys.argv) > 5 else "420sp"
    if fmt not in {"420sp", "420spvu", "420p", "422sp", "422spvu", "422p"}:
        print(f"unknown format {fmt}")
        return 2

    planar = fmt.endswith("p")
    fmt422 = fmt.startswith("422")
    vu = fmt.startswith("420spvu") or fmt.startswith("422spvu")
    chroma_h = h if fmt422 else h // 2
    chroma_w = w // 2 if planar else w

    with open(path, "wb") as f:
        for n in range(n_frames):
            for y in range(h):
                f.write(bytes(((y * 3 + n * 7) & 0xFF) for _ in range(w)))
            if fmt == "420sp":
                # Preserve the original I5O2 fixture byte for byte.
                for cy in range(h // 2):
                    f.write(
                        bytes(
                            (128 + ((cy * 5 + n * 3) & 0x1F) - 16) & 0xFF
                            for _ in range(w)
                        )
                    )
                continue
            u_plane = [
                bytes((64 + ((cy * 5 + n * 3 + cx) & 0x3F)) for cx in range(chroma_w))
                for cy in range(chroma_h)
            ]
            v_plane = [
                bytes((192 - ((cy * 3 + n * 5 + cx) & 0x3F)) for cx in range(chroma_w))
                for cy in range(chroma_h)
            ]
            if planar:
                for row in u_plane:
                    f.write(row)
                for row in v_plane:
                    f.write(row)
            else:
                for u_row, v_row in zip(u_plane, v_plane):
                    pairs = bytearray()
                    for u, v in zip(u_row, v_row):
                        pairs.extend((v, u) if vu else (u, v))
                    f.write(pairs)
    return 0


if __name__ == "__main__":
    sys.exit(main())
