#!/usr/bin/env python3
"""Enumerate the VP8 frame-tag profile distribution of a stream corpus.

Run on the board (or anywhere the corpus lives):

    python3 board_vp8_profile_probe.py ~/rk-mpp-oracles/corpus

For every IVF container whose fourcc is VP80, and for every bare file that at
least starts with a plausible VP8 frame tag, the version bits of each frame
tag are counted. VP8 has no separate profile field: the two low version bits
are the profile (0/2 are 4:2:0 8-bit and decodeable by the hardware today,
1/3 are the reserved profiles this project is closing), the bit above them is
reserved, and the key-frame start-code word carries the two scale factors
that C parses and never programs.

Output: one line per file, then a summary of which profiles exist and which
files carry anything but profile 0. An empty 'other profiles' list means the
corpus cannot answer the profile 1/3 board question and variants have to be
derived by patching the frame tags instead.
"""

import os
import struct
import sys


def ivf_frames(data):
    if len(data) < 32 or data[:4] != b"DKIF":
        return None
    fourcc = data[8:12]
    header_size = struct.unpack_from("<H", data, 6)[0]
    if header_size < 32 or header_size > len(data):
        return None
    frames = []
    offset = header_size
    while offset + 12 <= len(data):
        size = struct.unpack_from("<I", data, offset)[0]
        offset += 12
        if size == 0 or offset + size > len(data):
            return None
        frames.append(data[offset : offset + size])
        offset += size
    return fourcc, frames


def key_scale_bits(payload, key_frame):
    if not key_frame or len(payload) < 10:
        return (0, 0)
    # frame tag (3) + start code (3), then 14-bit width with 2 scale bits.
    width_word = payload[6] | (payload[7] << 8)
    height_word = payload[8] | (payload[9] << 8)
    return (width_word >> 14, height_word >> 14)


def probe(path):
    try:
        with open(path, "rb") as handle:
            data = handle.read()
    except OSError:
        return None
    parsed = ivf_frames(data)
    if parsed is not None:
        fourcc, frames = parsed
        if fourcc != b"VP80":
            return None
    else:
        # Bare VP8: accept only when the first tag looks like a key frame
        # (bit 0 clear, second and third bytes of the tag are the start-code
        # bytes 0x9d 0x01 0x2a at payload offset 0 of the following word).
        if len(data) < 10 or data[0] & 1 or data[3:6] != b"\x9d\x01\x2a":
            return None
        frames = [data]
    versions = {}
    scales = 0
    for payload in frames:
        if not payload:
            continue
        version = (payload[0] >> 1) & 7
        versions[version] = versions.get(version, 0) + 1
        hs, vs = key_scale_bits(payload, payload[0] & 1 == 0)
        if hs or vs:
            scales += 1
    return versions, scales


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/rk-mpp-oracles/corpus")
    non_zero = []
    scaled = []
    total = 0
    for dirpath, _, names in os.walk(root):
        for name in sorted(names):
            path = os.path.join(dirpath, name)
            result = probe(path)
            if result is None:
                continue
            versions, scales = result
            total += 1
            per = " ".join(f"v{v}:{c}" for v, c in sorted(versions.items()))
            print(f"{os.path.relpath(path, root)}  frames={sum(versions.values())}  {per}")
            if any(v != 0 for v in versions):
                non_zero.append(path)
            if scales:
                scaled.append((path, scales))
    print()
    print(f"vp8 streams found: {total}")
    print(f"streams carrying profiles other than 0: {len(non_zero)}")
    for path in non_zero:
        print(f"  {path}")
    print(f"streams with key-frame scale factors set: {len(scaled)}")
    for path, count in scaled:
        print(f"  {path} ({count} packets)")
    if not non_zero:
        print("no profile 1/3 stream exists: derive variants by patching frame tags instead")


if __name__ == "__main__":
    main()
