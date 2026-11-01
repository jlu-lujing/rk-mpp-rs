#!/usr/bin/env python3
"""Check restart-marker structure in a VEPU2 JPEG stream.

    jpege_markers.py <file.jpg> [mcu_count] [restart_interval]

Without the two numbers the tool reports what it finds: the DRI marker (value
and position), the SOF dimensions, and every RSTn marker. With them it also
checks the C-derived contract:

  * a DRI marker exists and equals restart_interval;
  * markers are the cycle FFD0, FFD1, ... FFD7 repeating, in order;
  * there are exactly (mcu_count - 1) // restart_interval of them, because
    the encoder must not emit a marker after the final MCU;
  * no marker sits inside the fixed header, all of them after SOS.

Entropy data cannot hide a false marker: the JPEG bitstream writer stuffs
every 0xFF with a following 0x00, so an FFD0..FFD7 pair inside a scan is a
real marker. The walk therefore consumes the scan byte-wise, skipping
stuffed FF00 pairs, instead of skipping the payload blindly.
"""
import sys


def parse(data):
    """(sof, dri, sos_at, markers) with markers = [(offset, marker)]."""
    sof = None
    dri = None
    sos_at = None
    markers = []
    # Header segment walk: markers carry a length and are followed by either
    # another segment or the entropy-coded scan.
    at = 2  # skip SOI
    n = len(data)
    while at + 1 < n and data[at] == 0xFF and data[at + 1] not in (0x00, 0xDA):
        marker = data[at + 1]
        if at + 3 >= n:
            break
        length = int.from_bytes(data[at + 2:at + 4], "big")
        if marker == 0xC0:
            height = int.from_bytes(data[at + 5:at + 7], "big")
            width = int.from_bytes(data[at + 7:at + 9], "big")
            sof = (width, height)
        elif marker == 0xDD:
            dri = (at, int.from_bytes(data[at + 4:at + 6], "big"))
        if marker == 0xD9:
            return sof, dri, sos_at, markers
        at += 2 + length
    if at + 1 >= n or data[at + 1] != 0xDA:
        return sof, dri, sos_at, markers

    # Entropy walk after SOS: stuffed FF00 pairs are data, only RSTn, the
    # next SOS and EOI end a scan. A padding 0xFF at a scan boundary can
    # precede the marker FF byte; those are skipped as filler, not markers.
    sos_at = at + 2 + int.from_bytes(data[at + 2:at + 4], "big")
    at = sos_at
    while at + 1 < n:
        if data[at] != 0xFF:
            at += 1
            continue
        marker = data[at + 1]
        if marker == 0x00:
            at += 2
            continue
        if marker == 0xFF:
            at += 1
            continue
        if 0xD0 <= marker <= 0xD7:
            markers.append((at, marker))
            at += 2
            continue
        break
    return sof, dri, sos_at, markers


def main():
    path = sys.argv[1]
    want_mcu = int(sys.argv[2]) if len(sys.argv) > 2 else None
    want_ri = int(sys.argv[3]) if len(sys.argv) > 3 else None
    data = open(path, "rb").read()
    fail = 0

    if data[:2] != b"\xff\xd8":
        print("FAIL: missing SOI")
        return 1

    sof, dri, sos_at, markers = parse(data)

    print(f"{path}: SOF {sof[0]}x{sof[1]}" if sof else f"{path}: no SOF")
    print(f"DRI {dri[1]} at {dri[0]}" if dri else "DRI none")
    names = ", ".join(f"FFD{m - 0xD0:X}@{a}" for a, m in markers)
    print(f"restart markers {len(markers)}: {names[:200]}")

    if want_ri is not None:
        if dri is None or dri[1] != want_ri:
            print(f"FAIL: DRI {dri and dri[1]} != expected {want_ri}")
            fail = 1

    if want_mcu is not None:
        ri = want_ri if want_ri is not None else (dri[1] if dri else 0)
        expected = (want_mcu - 1) // ri if ri else 0
        if len(markers) != expected:
            print(f"FAIL: {len(markers)} markers, expected {expected}")
            fail = 1
        for index, (_, marker) in enumerate(markers):
            if marker != 0xD0 + index % 8:
                print(f"FAIL: marker {index} is FFD{marker - 0xD0:X}, expected FFD{index % 8:X}")
                fail = 1
                break
        if sos_at and markers and markers[0][0] < sos_at:
            print("FAIL: marker inside the fixed header")
            fail = 1

    if data[-2:] != b"\xff\xd9":
        print("FAIL: missing hardware EOI")
        fail = 1

    print("PASS" if not fail else "FAILED")
    return fail


if __name__ == "__main__":
    sys.exit(main())
