#!/usr/bin/env python3
"""Diff a traced VEPU580 register job against the image this crate replays.

    python3 tools/vepu580_trace_diff.py <trace.log>

`tools/mpp_ioctl_trace.c` is LD_PRELOAD'ed over the C encoder, the log is the
stderr it writes, and the comparison target is the recorded template in
`src/hal/h264e_vepu580_template.rs`. Differing words are named using the field
map generated from MPP's own structs, so a difference reads as
`BASE.rc_qp.rc_qp_range` rather than as a bare index.
"""
import re
import sys
from pathlib import Path

SECTIONS = {
    0: ("CTL", 24),
    640: ("BASE", 94),
    4096: ("RCKLUT", 57),
    5888: ("PARAM", 180),
    8192: ("RDO", 122),
    8704: ("SCL", 679),
    12288: ("OSD", 288),
}
# Byte offset of each section, in register words.
WORDS = {0: 0, 640: 160, 4096: 1024, 5888: 1472, 8192: 2048, 8704: 2176, 12288: 3072}


def traced_sections(log: Path) -> dict[str, list[int]]:
    lines = log.read_text(errors="replace").splitlines()
    found: dict[str, list[int]] = {}
    for index, line in enumerate(lines):
        if "mpp_trace before cmd=200" not in line or "size=" not in line:
            continue
        fields = dict(
            item.split("=", 1)
            for item in line.split()
            if "=" in item and item.split("=", 1)[0] in {"size", "offset"}
        )
        offset = int(fields["offset"], 0)
        if offset not in SECTIONS:
            continue
        name, _expected = SECTIONS[offset]
        words_line = lines[index + 1]
        if "mpp_trace write words=" not in words_line:
            continue
        words = [int(value, 16) for value in words_line.split()[3:]]
        # A later job of the same session wins; the last encode is the one asked
        # about, and the earlier ones are probe frames.
        found[name] = words
    return found


def template_sections() -> dict[str, list[int]]:
    source = Path("src/hal/h264e_vepu580_template.rs").read_text()
    result = {}
    for name, _ in SECTIONS.values():
        match = re.search(
            r"pub const " + name + r"_WORDS: \[u32; \d+\] = \[(.*?)\];", source, re.S
        )
        result[name] = [int(value, 0) for value in re.findall(r"0x[0-9a-fA-F]+", match.group(1))]
    return result


def field_names() -> dict[tuple[int, int, int], str]:
    """(word, shift, width) -> `section.group.field` from the generated map."""
    source = Path("src/hal/vepu580_regs.rs").read_text()
    names = {}
    for word, shift, width, name in re.findall(
        r"word:\s*(\d+),\s*shift:\s*(\d+),\s*width:\s*(\d+),\s*\"?name:\s*\"([^\"]+)\"",
        source,
        re.S,
    ):
        names[(int(word), int(shift), int(width))] = name
    return names


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    traced = traced_sections(Path(sys.argv[1]))
    if not traced:
        print("no register writes in the trace")
        return 1
    ours = template_sections()
    names = field_names()
    for offset, (name, _count) in sorted(SECTIONS.items()):
        if name not in traced:
            print(f"{name:<7} not written by the traced job")
            continue
        traced_words, our_words = traced[name], ours[name]
        base_word = WORDS[offset]
        differences = [
            (base_word + index, our_words[index], traced_words[index])
            for index in range(min(len(our_words), len(traced_words)))
            if our_words[index] != traced_words[index]
        ]
        # Which of those differences fall inside a named field?
        described = []
        for word, before, after in differences:
            hits = sorted(
                field
                for (fword, shift, width), field in names.items()
                if fword == word
                and ((before >> shift) & ((1 << width) - 1))
                != ((after >> shift) & ((1 << width) - 1))
            )
            described.append((word, before, after, hits))
        print(f"{name:<7} {len(differences)} differing words of {len(our_words)}")
        for word, before, after, hits in described:
            label = ", ".join(hits) if hits else "(unnamed field)"
            print(
                f"   sw{word:<5} replayed={before:#010x} traced={after:#010x}  {label}"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
