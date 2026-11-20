#!/usr/bin/env python3
"""Compare two Annex-B encoder outputs.

    compare_streams.py [--sequence-only] <c_stream> <our_stream>

`--sequence-only` compares NAL types without requiring equal payload bytes.
Both H.264 and H.265 Annex-B streams are recognised.

The SEI NALUs are part of the comparison, not filtered out: step 17 of
`mpp_enc_impl.cpp` puts a `uuid_version` and then a `uuid_rc_cfg` unregistered
user-data SEI ahead of every IDR at the default SEI mode, so an access unit
without them is a different stream from the one being measured against.

One exception is made deliberately: the version SEI's payload is the build
identity of whichever encoder produced it - MPP writes its own `git log` line
there - so that NAL is compared for framing (start code, NAL header, payload
type, the 16-byte uuid, the RBSP trailing byte) and not for text. Everything
else, including the rc-cfg SEI, has to match byte for byte.
"""
import re
import sys

UUID_VERSION = bytes.fromhex("3d076d45730f41a8b1c425d7976bf1ac")
SEI_USER_DATA_UNREGISTERED = 5
HEVC_VPS = 32
HEVC_SPS = 33
HEVC_PPS = 34
HEVC_PREFIX_SEI = 39


def nals(path):
    data = open(path, "rb").read()
    starts = [m.start() for m in re.finditer(b"\x00\x00\x00\x01", data)] + [len(data)]
    return [data[starts[k] + 4:starts[k + 1]] for k in range(len(starts) - 1)
            if data[starts[k] + 4:starts[k + 1]]]


def stream_codec(units):
    # H.264 type-14 prefix NALs can decode to an HEVC-looking 39 when the same
    # bits are interpreted with HEVC's layer-id layout. Annex-B encoders both
    # start with a parameter-set VPS/SPS, so classify from the first unit only.
    first = units[0] if units else b""
    if (
        len(first) >= 2
        and (first[0] & 1) == 0
        and ((first[0] >> 1) & 0x3F)
        in (HEVC_VPS, HEVC_SPS, HEVC_PPS, HEVC_PREFIX_SEI)
    ):
        return "hevc"
    return "h264"


def nal_type(body, codec):
    if codec == "hevc":
        return (body[0] >> 1) & 0x3F
    return body[0] & 0x1F


def fields(body, codec):
    """(nal header, payload type, payload size, remainder start) of an SEI NAL.

    Decodes the 0xff continuation form of both fields the way MPP writes them,
    so the uuid position is derived rather than assumed.
    """
    at = 2 if codec == "hevc" else 1
    ptype = 0
    while body[at] == 0xFF:
        ptype += 255
        at += 1
    ptype += body[at]
    at += 1
    size = 0
    while body[at] == 0xFF:
        size += 255
        at += 1
    size += body[at]
    at += 1
    return (body[:at - 1], ptype, size, (at, body[at:at + 16]))


def is_version_sei(body, codec):
    expected = HEVC_PREFIX_SEI if codec == "hevc" else 6
    return nal_type(body, codec) == expected and UUID_VERSION in body


def kinds(units, codec):
    return [
        (nal_type(unit, codec), "version" if is_version_sei(unit, codec) else "")
        for unit in units
    ]


def main() -> int:
    arguments = sys.argv[1:]
    sequence_only = arguments[:1] == ["--sequence-only"]
    if sequence_only:
        arguments = arguments[1:]
    if len(arguments) != 2:
        print("usage: compare_streams.py [--sequence-only] <c_stream> <our_stream>",
              file=sys.stderr)
        return 2
    c, ours = nals(arguments[0]), nals(arguments[1])
    c_codec = stream_codec(c)
    ours_codec = stream_codec(ours)
    if c_codec != ours_codec:
        print(f"    codec differs: C {c_codec} vs ours {ours_codec}")
        return 1
    c_kinds = kinds(c, c_codec)
    ours_kinds = kinds(ours, ours_codec)
    if c_kinds != ours_kinds:
        print(f"    NAL sequence differs: C {c_kinds} vs ours {ours_kinds}")
        return 1
    if sequence_only:
        print(f"    {len(c)} NAL types identical")
        return 0
    for a, b in zip(c, ours):
        if is_version_sei(a, c_codec):
            # Framing, not text: NAL header, payload type, the size in 0xff
            # continuation form, the uuid that follows it, and the RBSP trailing
            # byte. Comparing a fixed byte count instead would reach into the
            # payload text, which is the one thing that cannot match by nature.
            ca, cb = fields(a, c_codec), fields(b, ours_codec)
            if (
                ca[:2] != cb[:2]
                or ca[3][1] != cb[3][1]
                or a[-1] != 0x80
                or b[-1] != 0x80
            ):
                print(f"    version SEI framing differs: header/type {ca[:2]} vs {cb[:2]}, "
                      f"uuid {ca[3][1].hex()} vs {cb[3][1].hex()}")
                return 1
            # Everything after the header, the two length fields and the uuid is
            # payload plus the RBSP trailing byte, so the size has to account for
            # exactly that. C's own version SEI: 3 + 110 + 1 = 114 body bytes.
            for name, body, (header, ptype, size, (at, _uuid)) in (
                ("C", a, ca),
                ("ours", b, cb),
            ):
                # A size is only worth checking once the NAL says what it is, and
                # a payload size is only meaningful if it accounts for exactly the
                # uuid, the text and the trailing one_bits.
                if ptype != SEI_USER_DATA_UNREGISTERED or len(body) != at + size + 1:
                    print(f"    {name} version SEI is malformed: header/type {(header, ptype)}, "
                          f"size {size}, body {len(body)}, bytes before the payload {at}")
                    return 1
            continue
        if a != b:
            first = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y),
                         min(len(a), len(b)))
            print(f"    NAL type {nal_type(a, c_codec)} differs at byte {first} "
                  f"({len(a)} vs {len(b)} bytes)")
            return 1
    print(f"    {len(c)} NAL units identical (version SEI: framing only)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
