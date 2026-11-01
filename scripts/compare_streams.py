#!/usr/bin/env python3
"""Compare two Annex-B encoder outputs NAL unit by NAL unit.

    compare_streams.py <c_stream> <our_stream>

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


def nals(path):
    data = open(path, "rb").read()
    starts = [m.start() for m in re.finditer(b"\x00\x00\x00\x01", data)] + [len(data)]
    return [data[starts[k] + 4:starts[k + 1]] for k in range(len(starts) - 1)
            if data[starts[k] + 4:starts[k + 1]]]


def fields(body):
    """(nal header, payload type, payload size, remainder start) of an SEI NAL.

    Decodes the 0xff continuation form of both fields the way
    `h264e_sei_to_packet` writes them, so the uuid position is derived rather
    than assumed.
    """
    at = 1
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
    return (body[0], ptype, size, (at, body[at:at + 16]))


def is_version_sei(body):
    return (body[0] & 0x1F) == 6 and UUID_VERSION in body


def kinds(units):
    return [(u[0] & 0x1F, "version" if is_version_sei(u) else "") for u in units]


def main() -> int:
    c, ours = nals(sys.argv[1]), nals(sys.argv[2])
    if kinds(c) != kinds(ours):
        print(f"    NAL sequence differs: C {kinds(c)} vs ours {kinds(ours)}")
        return 1
    for a, b in zip(c, ours):
        if is_version_sei(a):
            # Framing, not text: NAL header, payload type, the size in 0xff
            # continuation form, the uuid that follows it, and the RBSP trailing
            # byte. Comparing a fixed byte count instead would reach into the
            # payload text, which is the one thing that cannot match by nature.
            ca, cb = fields(a), fields(b)
            if ca[:2] != cb[:2] or ca[3][1] != cb[3][1] or a[-1] != 0x80 or b[-1] != 0x80:
                print(f"    version SEI framing differs: header/type {ca[:2]} vs {cb[:2]}, "
                      f"uuid {ca[3][1].hex()} vs {cb[3][1].hex()}")
                return 1
            # Everything after the header, the two length fields and the uuid is
            # payload plus the RBSP trailing byte, so the size has to account for
            # exactly that. C's own version SEI: 3 + 110 + 1 = 114 body bytes.
            for name, body, (header, ptype, size, (at, uuid)) in (
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
            print(f"    NAL type {a[0] & 0x1F} differs at byte {first} "
                  f"({len(a)} vs {len(b)} bytes)")
            return 1
    print(f"    {len(c)} NAL units identical (version SEI: framing only)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
