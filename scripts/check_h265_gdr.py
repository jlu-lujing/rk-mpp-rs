#!/usr/bin/env python3
"""Require the fixed 8-picture GDR shape emitted by the board cells."""

import re
import sys


def main() -> int:
    data = open(sys.argv[1], "rb").read()
    starts = [m.start() for m in re.finditer(b"\x00\x00\x00\x01", data)] + [len(data)]
    nals = []
    for index in range(len(starts) - 1):
        body = data[starts[index] + 4 : starts[index + 1]]
        if body:
            nals.append((body[0] >> 1) & 0x3F)

    vcl = [kind for kind in nals if kind <= 31]
    if vcl != [19] + [1] * 7:
        print(f"unexpected VCL sequence: {vcl}", file=sys.stderr)
        return 1
    recovery = sum(
        1
        for index in range(len(starts) - 1)
        if data[starts[index] + 4] >> 1 & 0x3F == 39
        and data[starts[index] + 6] == 6
    )
    if recovery != 1:
        print(f"expected one recovery-point SEI, found {recovery}", file=sys.stderr)
        return 1
    print(f"{len(nals)} NAL units, 1 IDR + 7 P, 1 recovery-point SEI")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
