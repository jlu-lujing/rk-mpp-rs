#!/usr/bin/env python3
"""Read the rate-control part out of an encoder ioctl trace.

    rc_trace_report.py <trace.log> [--against <other.log>] [--all]

`tools/mpp_ioctl_trace.c` is LD_PRELOAD'ed over the C encoder and prints every
register window it writes, which is the only way to see what MPP's rate control
actually puts in front of the hardware. This narrows that to the rate-control
question: the per-job values of the named fields in the BASE, RCKLUT and RDO
windows, plus the statistics the encoder read back afterwards - bit count and QP
sum - because the model's decision is only checkable against what the hardware
reported.

By default only the fields that move between jobs are printed. Rate control is a
state machine, and a fixed-QP job and a CBR job differ in a handful of words; a
full dump of 151 words per job hides that. `--all` prints every non-zero field.

`--dump-job N` prints job N's rate-control windows as raw words, which is the
shape a golden fixture needs: the register half of rate control is verified by
rebuilding those words and requiring they are identical.

`--against` compares the first job of this trace with the first job of another,
which is how a fixed-QP capture is turned into "these are the words the VBR path
adds".
"""
import argparse
import re
import sys
from pathlib import Path

# Byte offset of each control window -> (name, first absolute register word).
SECTIONS = {
    0: ("CTL", 0),
    640: ("BASE", 160),
    4096: ("RCKLUT", 1024),
    5888: ("PARAM", 1472),
    8192: ("RDO", 2048),
    8704: ("SCL", 2176),
    12288: ("OSD", 3072),
}
# The windows rate control writes. SCL and OSD are excluded on purpose: they are
# the scan tables and the overlay, and they never move with the bit rate.
RC_SECTIONS = ("BASE", "RCKLUT", "RDO")
# The window the encoder reads back after a job. Note the tracer prints each
# register write twice - once under the `before cmd=200` line and once on its
# own - so counting `write words=` lines finds twice as many jobs as there are.
# Keying on the `before` line, as parse() does, is what keeps the count honest.
STATUS_OFFSET = 16384
# Absolute register word of each STATUS field, as `Statistics::from_words` reads
# them (see scripts/stat_compare.py, which checks the same offsets).
STATUS = {"bits": 4096, "qp_sum": 4099, "madp": 4168, "num_ctu": 4169}


REPO = Path(__file__).resolve().parent.parent


def field_names():
    """absolute word -> {field name: (shift, width)} from the generated map."""
    source = (REPO / "src/hal/vepu580_regs.rs").read_text()
    names = {}
    for word, shift, width, name in re.findall(
        r"word:\s*(\d+),\s*shift:\s*(\d+),\s*width:\s*(\d+),\s*\"?name:\s*\"([^\"]+)\"",
        source,
        re.S,
    ):
        names.setdefault(int(word), {})[name] = (int(shift), int(width))
    return names


def parse(path):
    """The jobs of a trace, in order, plus the statistics each one read back."""
    jobs = []
    current = {}
    lines = Path(path).read_text(errors="replace").splitlines()
    for index, line in enumerate(lines):
        if "mpp_trace before cmd=200" in line and "offset=" in line:
            fields = dict(
                item.split("=", 1)
                for item in line.split()
                if "=" in item and item.split("=", 1)[0] in {"size", "offset"}
            )
            offset = int(fields["offset"], 0)
            if offset not in SECTIONS:
                continue
            if offset == 0 and current:
                jobs.append(current)
                current = {}
            words_line = lines[index + 1] if index + 1 < len(lines) else ""
            if "mpp_trace write words=" not in words_line:
                continue
            # The token after `words=` is the word count, not data.
            values = words_line.split()[3:]
            name, _ = SECTIONS[offset]
            current[name] = [int(v, 16) for v in values]
        elif f"read_complete[1]@{STATUS_OFFSET}" in line:
            after = line.partition("words=")[2].split()
            if after and after[0].isdigit() and int(after[0]) <= 4096:
                after = after[1:]
            current["STATUS"] = [int(v, 16) for v in after]
            jobs.append(current)
            current = {}
    if current:
        jobs.append(current)
    return jobs


def values(job, names):
    """field name -> value, over the rate-control windows of one job."""
    out = {}
    for section in RC_SECTIONS:
        words = job.get(section)
        if not words:
            continue
        _, base = next(v for v in SECTIONS.values() if v[0] == section)
        for index, word in enumerate(words):
            for name, (shift, width) in names.get(base + index, {}).items():
                out[name] = (word >> shift) & ((1 << width) - 1)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("log")
    ap.add_argument("--against", help="another trace to compare the first job with")
    ap.add_argument("--all", action="store_true", help="print every non-zero field")
    ap.add_argument(
        "--dump-job",
        type=int,
        help="print one job's rate-control windows as words, for a golden fixture",
    )
    args = ap.parse_args()

    names = field_names()
    jobs = parse(args.log)
    if not jobs:
        print("no register writes in the trace")
        return 1

    # Names rather than paths, so the output can be committed as a fixture without
    # recording where somebody's copy of the log happened to live. The count is not
    # printed for --dump-job, whose whole output is fixture material.
    if args.dump_job is None:
        print(f"{len(jobs)} encoder jobs in {Path(args.log).name}")
    if args.against:
        other = parse(args.against)
        if not other:
            print(f"no jobs in {args.against}")
            return 1
        before, after = values(other[0], names), values(jobs[0], names)
        keys = sorted(k for k in set(before) | set(after) if before.get(k) != after.get(k))
        print(f"first job differs from {Path(args.against).name} in {len(keys)} named fields:")
        for key in keys:
            print(f"   {key:<40} {before.get(key)} -> {after.get(key)}")
        print()

    if args.dump_job is not None:
        job = jobs[args.dump_job]
        for section in RC_SECTIONS:
            words = job.get(section)
            if words:
                print(f"{section} " + " ".join(f"{w:08x}" for w in words))
        return 0

    previous = {}
    for index, job in enumerate(jobs):
        current = values(job, names)
        status = job.get("STATUS")
        read = {}
        if status:
            for key, word in STATUS.items():
                offset = word - STATUS_OFFSET // 4
                read[key] = status[offset] if offset < len(status) else None
        moved = {
            k: (previous.get(k), v)
            for k, v in current.items()
            if k in previous and previous[k] != v
        }
        shown = sorted(moved) if not args.all else sorted(k for k, v in current.items() if v)
        bits = read.get("bits")
        qp_sum, ctus = read.get("qp_sum"), read.get("num_ctu")
        # `qp_sum` accumulates one QP per 16x16 block while `num_ctu` counts 32x32
        # blocks, four of which sit in each one: fixed QP 26 at 320x240 reads back
        # 7800 = 26 * 20 * 15 against num_ctu = 75. Dividing by the wrong one of
        # those two gives 104, which is not a QP. An intra job reports no ctu count
        # at all, so its mean is left out rather than guessed.
        blocks = 4 * ctus if ctus else 0
        mean = f"{qp_sum / blocks:.2f}" if qp_sum is not None and blocks else "-"
        print(
            f"job {index:>2} bits={bits if bits is not None else '-':<9} "
            f"qp_sum={str(read.get('qp_sum')):<7} num_ctu={str(read.get('num_ctu')):<5} "
            f"mean_qp={mean}"
        )
        for key in shown:
            if args.all:
                print(f"      {key:<40} {current[key]}")
            else:
                old, new = moved[key]
                print(f"      {key:<40} {old} -> {new}")
        previous = current
    return 0


if __name__ == "__main__":
    sys.exit(main())
