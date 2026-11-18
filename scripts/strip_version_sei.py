#!/usr/bin/env python3
"""Drop the encoder build-identity SEI NAL from an Annex-B stream.

Usage: strip_version_sei.py <in> <out>

The C MPP encoder writes a `uuid_version` user-data-unregistered SEI ahead of
every IDR whose text is its own build identity; this crate writes its own. The
NAL is therefore compared for framing in the regression scripts, not text. This
helper removes exactly that NAL (matched by the 16-byte uuid) so two streams can
still be byte-compared for the rc-cfg SEI and the coded slices.

Handles H.264 (NAL type 6) and H.265 (prefix/suffix SEI, types 39/40).
"""
import sys

VERSION_UUID = bytes.fromhex("3d076d45730f41a8b1c425d7976bf1ac")


def nals(data):
    starts = []
    i = 0
    while True:
        j = data.find(b"\x00\x00\x01", i)
        if j < 0:
            break
        starts.append(j + 3)
        i = j + 3
    for k, payload in enumerate(starts):
        end = starts[k + 1] - 3 if k + 1 < len(starts) else len(data)
        yield max(0, payload - 3), payload, end


def sei_uuid(body, h265):
    # Skip start code if present in body; callers pass the NAL payload only.
    at = 1 if not h265 else 2
    if at >= len(body):
        return None
    ptype = 0
    while at < len(body) and body[at] == 0xFF:
        ptype += 255
        at += 1
    if at >= len(body):
        return None
    ptype += body[at]
    at += 1
    size = 0
    while at < len(body) and body[at] == 0xFF:
        size += 255
        at += 1
    if at >= len(body):
        return None
    at += 1
    if ptype != 5 or at + 16 > len(body):
        return None
    return body[at:at + 16]


def main():
    src, dst = sys.argv[1], sys.argv[2]
    data = open(src, "rb").read()
    out = bytearray()
    for start, payload, end in nals(data):
        body = data[payload:end]
        if not body:
            continue
        h264 = (body[0] & 0x1F) == 6
        h265 = ((body[0] >> 1) & 0x3F) in (39, 40)
        if h264 or h265:
            uuid = sei_uuid(body, h265 and not h264)
            if uuid == VERSION_UUID:
                continue
        out += data[start:end]
    open(dst, "wb").write(bytes(out))


if __name__ == "__main__":
    main()
