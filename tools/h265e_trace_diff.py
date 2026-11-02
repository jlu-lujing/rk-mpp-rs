#!/usr/bin/env python3
"""Diff a traced VEPU580 H.265 job against the image `hal::h265e` derives.

    python3 tools/h265e_trace_diff.py <trace.log>

`tools/mpp_ioctl_trace.c` is LD_PRELOAD'ed over the vintage C encoder
(`mpi_enc_test -t 16777220 -rc 2 -n 1`), and the derived side comes from
`rk-mpp-rs h265-regs <w> <h> <qp>` (override with `H265_REGS_CMD`). Differing
words are named through the generated field map, so a difference reads as
`WGT.pre_intra...` rather than as a bare index. That naming is the point: the
IDR payload gate is a register diff, and a diff of unnamed words is not evidence.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

# byte offset -> (section, first absolute word, word count). H.265 writes six
# windows: there is no SCL window at 8704, and `wgt` at 5888 is 374 words where
# H.264's `param` is 180.
SECTIONS = {
    0: ("CTL", 0, 24),
    640: ("BASE", 160, 94),
    4096: ("RC_KLUT", 1024, 57),
    5888: ("WGT", 1472, 374),
    8192: ("RDO", 2048, 807),
    12288: ("OSD", 3072, 288),
}


def traced_passes(log: Path) -> list[dict[str, list[int]]]:
    """Every register write broken into one dict per job.

    The tracer prints each section twice per job, so adjacent identical passes are
    collapsed; `--job N` then names a picture rather than a duplicate.
    """
    lines = log.read_text(errors="replace").splitlines()
    passes: list[dict[str, list[int]]] = []
    current: dict[str, list[int]] = {}
    for index, line in enumerate(lines):
        if "mpp_trace before cmd=200" not in line or "size=" not in line:
            continue
        attrs = dict(
            item.split("=", 1)
            for item in line.split()
            if "=" in item and item.split("=", 1)[0] in {"size", "offset"}
        )
        offset = int(attrs["offset"], 0)
        if offset not in SECTIONS:
            continue
        if index + 1 >= len(lines):
            continue
        words_line = lines[index + 1]
        if "mpp_trace write words=" not in words_line:
            continue
        words = [int(value, 16) for value in words_line.split()[3:]]
        name = SECTIONS[offset][0]
        if name == "CTL" and "CTL" in current:
            passes.append(current)
            current = {}
        current[name] = words
    if current:
        passes.append(current)
    deduped: list[dict[str, list[int]]] = []
    for entry in passes:
        if not deduped or deduped[-1] != entry:
            deduped.append(entry)
    return deduped


def traced_sections(log: Path, job: int | None = None) -> dict[str, list[int]]:
    passes = traced_passes(log)
    if job is None:
        return passes[-1] if passes else {}
    if job >= len(passes):
        return {}
    return passes[job]


def derived_sections() -> dict[str, list[int]]:
    cmd = os.environ.get("H265_REGS_CMD", "./target/release/rk-mpp-rs h265-regs 320 240 26")
    output = subprocess.run(
        cmd, shell=True, check=True, capture_output=True, text=True
    ).stdout.splitlines()
    sections: dict[str, list[int]] = {}
    current = None
    known = {name for name, _, _ in SECTIONS.values()}
    for line in output:
        head = line.split()
        # A hex word made only of a-f is alphabetic, so match the section names
        # rather than "looks like a word".
        if len(head) == 3 and head[0].upper() in known and head[1].isdigit():
            current = head[0].upper()
            sections[current] = []
        elif current and line.strip():
            sections[current].extend(int(w, 16) for w in head)
    return sections


def field_names() -> dict[tuple[int, int, int], str]:
    source = Path("src/hal/h265e_regs.rs").read_text()
    names = {}
    for word, shift, width, name in re.findall(
        r"word:\s*(\d+),\s*shift:\s*(\d+),\s*width:\s*(\d+),\s*name:\s*\"([^\"]+)\"",
        source,
        re.S,
    ):
        names[(int(word), int(shift), int(width))] = name
    return names


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--job=")]
    job = None
    for arg in sys.argv[1:]:
        if arg.startswith("--job="):
            job = int(arg.split("=", 1)[1])
    if len(args) != 1:
        print(__doc__)
        return 2
    traced = traced_sections(Path(args[0]), job)
    if not traced:
        print("no VEPU580 register writes in the trace")
        return 1
    ours = derived_sections()
    names = field_names()
    total = 0
    for offset in sorted(SECTIONS):
        name, first, count = SECTIONS[offset]
        if name not in traced:
            print(f"{name:<8} not written by the traced job")
            continue
        traced_words = traced[name]
        our_words = ours.get(name, [])
        length = min(len(traced_words), len(our_words))
        differences = [
            (first + index, our_words[index], traced_words[index])
            for index in range(length)
            if our_words[index] != traced_words[index]
        ]
        total += len(differences)
        lengths = "" if len(traced_words) == len(our_words) else (
            f" (length: traced {len(traced_words)}, ours {len(our_words)})"
        )
        print(f"{name:<8} {len(differences):3d} differing words of {count}{lengths}")
        for word, before, after in differences:
            hits = sorted(
                field
                for (fword, shift, width), field in names.items()
                if fword == word
                and ((before >> shift) & ((1 << width) - 1))
                != ((after >> shift) & ((1 << width) - 1))
            )
            label = ", ".join(hits) if hits else "(unnamed word)"
            print(
                f"   sw{word:<5} derived={before:#010x} traced={after:#010x}  {label}"
            )
    print(f"job: {'last' if job is None else job}, total differing words: {total}")
    return 0 if total == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
