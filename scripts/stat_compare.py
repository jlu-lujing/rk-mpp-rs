#!/usr/bin/env python3
"""Compare the C encoder's STATUS read-back with this driver's, word by word.

    stat_compare.py <c_trace_log> <our_dump>

Both files hold the same text shape: a `read_complete` line naming the window's
byte offset, followed by `words=` and the window as hex. Only the fields the
encoder's downstream logic actually reads are required to agree; the raw word
count is reported, not asserted, because the window also carries per-picture
results (bitstream length, SSE, QP sums) that legitimately differ.
"""
import re
import sys

FIELDS = [
    ("bitstream_bytes", 4096),
    ("qp_sum", 4099),
    ("madp", 4168),
    ("num_ctu", 4169),
    ("madi", 4170),
    ("num_b16", 4171),
    ("md_sad_b16num0", 4173),
    ("md_sad_b16num1", 4174),
    ("md_sad_b16num2", 4175),
    ("md_sad_b16num3", 4176),
    ("madi_b16num0", 4177),
    ("madi_b16num1", 4178),
    ("madi_b16num2", 4179),
    ("madi_b16num3", 4180),
]


def windows(path, want_offset):
    out = []
    try:
        lines = open(path, errors="replace").read().splitlines()
    except OSError as error:
        print(f"cannot read {path}: {error}")
        sys.exit(1)
    # One line per read in both formats: the tracer's `dump_words` and our own dump
    # print tag, count and words together. Parse line by line - reading ahead into
    # neighbouring lines is what produced duplicate windows last time.
    for line in lines:
        if f"read_complete[1]@{want_offset}" not in line:
            continue
        _, _, after = line.partition("words=")
        fields = after.split()
        # The first field is the word count, not a value: treating it as data
        # shifts the whole window by one word.
        if fields and fields[0].isdigit() and int(fields[0]) <= 4096:
            fields = fields[1:]
        words = [int(w, 16) for w in fields if re.fullmatch(r"[0-9a-fA-F]{8}", w)]
        if len(words) > 100:
            out.append(words)
    return out


def main() -> int:
    c = windows(sys.argv[1], 16384)
    ours = windows(sys.argv[2], 16384)
    print(f"C jobs with STATUS: {len(c)}, ours: {len(ours)}")
    if not c or not ours:
        return 1
    if len(c) != len(ours):
        # Comparing the first min(n) windows would let one side's duplicate line
        # stand in for two jobs and report a match that never happened.
        print(f"   job counts differ: {len(c)} vs {len(ours)}")
        return 1
    bad = 0
    for job in range(min(len(c), len(ours))):
        cw, rw = c[job], ours[job]
        # Every field the encoder's downstream logic reads has to agree. reg4107,
        # `st_bsb.bsbw_addr` - the bitstream address the hardware wrote to - is
        # reported but not required to match: two processes map different dma-bufs.
        for name, reg in FIELDS:
            idx = reg - 4096
            cv = cw[idx] if idx < len(cw) else None
            rv = rw[idx] if idx < len(rw) else None
            if cv != rv:
                print(f"   job {job} {name}: C={cv} ours={rv}")
                bad += 1
        raw = [
            f"reg{4096 + i}(C={cw[i]:#x} ours={rw[i]:#x})"
            for i in range(min(len(cw), len(rw)))
            if cw[i] != rw[i] and 4096 + i != 4107
        ]
        print(f"  job {job}: {len(raw)} words differ apart from the bitstream address"
              + (f": {'; '.join(raw)}" if raw else ""))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
