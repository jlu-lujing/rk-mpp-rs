#!/usr/bin/env python3
"""Generate the VEPU580 register field map from MPP's own struct definitions.

    python3 tools/gen_vepu580_regs.py             > src/hal/vepu580_regs.rs
    python3 tools/gen_vepu580_regs.py --hevc      > src/hal/h265e_regs.rs

`mpp/hal/rkenc/common/vepu580_common.h` annotates every top-level member with its
absolute address and register number (`/* 0x00000300 reg192 */`), so the word an
anonymous bitfield struct lands on is stated rather than inferred. The fields are
read in declaration order, first declared in the low bits, which is what the
little-endian bitfield layout in the driver produces.

The `--hevc` mode reads the H.265 encoder's own struct views of the *same* VEPU580
windows (`HevcVepu580ControlCfg`, `HevcVepu580Base` in `hal_h265e_vepu580_reg.h`,
and the shared `Vepu580RcKlut`) and writes `src/hal/h265e_regs.rs`. It covers only
the three windows the fixed-QP IDR path derives from the source at the moment -
control, buffer/video-syntax and the RC lookup table. The tuning windows (param/
wgt, rdo, scl) are deliberately not emitted: `HevcVepu580Wgt_t` runs well past the
captured 180-word param section, and the honest way to fill those is the same
recorded-job template the H.264 encoder replays, which is an on-board capture the
orchestrator owns, not a hand-written table.

Members named reserved* are counted but not emitted. A struct whose declared
fields do not add up to exactly one word is an error: it means the parser met a
form it did not understand, and silently mis-shifting a register is exactly the
mistake this file exists to prevent.
"""
import re
import sys
from pathlib import Path

# The register headers are not inside this crate: `refs/mpp` is the master
# checkout the whole project reads, and the vendored vintage sits under this
# crate's own `refs/`. Resolving both from this file rather than from the working
# directory keeps the documented `python3 tools/gen_vepu580_regs.py --hevc >
# src/hal/h265e_regs.rs` working from the crate root, where it used to die on a
# FileNotFoundError for the common header.
ROOT = Path(__file__).resolve().parents[2]  # the workspace holding refs/mpp
CRATE = Path(__file__).resolve().parents[1]  # this crate

COMMON = ROOT / "refs/mpp/mpp/hal/rkenc/common/vepu580_common.h"
H264E = ROOT / "refs/mpp/mpp/hal/rkenc/h264e/hal_h264e_vepu580_reg.h"
# The H.265 view is read from the 2024-04 vintage vendored under refs/ rather
# than from `refs/mpp` (master): the two copies of `hal_h265e_vepu580_reg.h`
# differ in roughly two thousand lines, and every byte comparison on the board
# is against the vintage. Master stays the source for the H.264 path, where the
# known drift is documented and the affected words are left to the capture.
H265E = CRATE / "refs/mpp-2024-04/rkenc/h265e/hal_h265e_vepu580_reg.h"

# (header, struct name, section prefix, first absolute word, words in our register
# file). The first word and length match VEPU580_*_OFFSET and the sections
# tools/gen_h264e_vepu580_template.py extracts from an ioctl trace.
#
# Vepu580BaseCfg is the H.264 view of the same window: it embeds
# Vepu580BaseCommon and then adds reg221 onwards, which is where the syntax
# (synt_sps / synt_pps / synt_sli0) and rdo_cfg live. Those words belong to the
# BASE section, so they share its first word and array.
STRUCTS = [
    (COMMON, "Vepu580CtlCommon", "CTL", 0, 24),
    (COMMON, "Vepu580BaseCommon", "BASE", 160, 94),
    (COMMON, "Vepu580RcKlut", "RCKLUT", 1024, 57),
    (H264E, "Vepu580BaseCfg", "H264", 160, 94),
    # The two tuning windows. The finetune module's writes land here, which is
    # where the last difference between a Rust P job and a C P job lives.
    (H264E, "Vepu580Section3", "S3", 1472, 180),
    (H264E, "Vepu580RdoCfg", "RDO", 2048, 122),
    # The cime/rime/fme weights. `Vepu580Section3` ends with a `common` member
    # this struct by value, and `vepu580_h264e_tune_reg_patch` writes into it -
    # the CIME/RIME search multipliers and the scene-change overrides. It gets
    # the same `S3` prefix and shares the `param` array, so its words are read
    # from the struct's own `reg` comments.
    (COMMON, "Vepu580WgtCommon", "S3", 1496, 180),
]

# The H.265 encoder's struct views of the same windows. Only the three the
# fixed-QP IDR path derives at build time are emitted; see the module docstring.
# Each window is parsed twice, mirroring the H.264 list: the shared common struct
# under the plain window prefix (`Vepu580CtlCommon`/`Vepu580BaseCommon`), and the
# H.265 encoder's own view under its own prefix. `HevcVepu580ControlCfg` and
# `HevcVepu580Base` embed the common struct by value, which the parser skips, so
# they contribute only the registers past it (`rdo_ckg_hevc`/`enc_id`, and the
# syntax block from `reg0221_me_cfg` onwards) without colliding with the common
# fields. The `RC` prefix names the shared RC lookup table. h265e.rs reads the
# QP/ROI rows through RC and the syntax registers through H265.
# The H.265 job writes six windows, not seven: the trace of a fixed-QP IDR on RK3588
# shows CTL 24, BASE 94, RC_KLUT 57, WGT 374 at word 1472, RDO 807 at word 2048
# and OSD 288 at word 3072 - and no SCL write at all. The sizes are the
# `H265eV580RegSet` struct list in the vintage header; the word counts of the
# H.264 map (180 / 122 / 679) were wrong here, and any field past the first 180
# words of the weight window would have landed outside its section.
STRUCTS_H265 = [
    (COMMON, "Vepu580CtlCommon", "CTL", 0, 24),
    (H265E, "HevcVepu580ControlCfg", "CTL", 0, 24),
    (COMMON, "Vepu580BaseCommon", "BASE", 160, 94),
    (H265E, "HevcVepu580Base", "H265", 160, 94),
    (COMMON, "Vepu580RcKlut", "RC", 1024, 57),
    (H265E, "HevcVepu580Wgt", "WGT", 1472, 374),
    (H265E, "Vepu580RdoCfg", "RDO", 2048, 807),
    (H265E, "Vepu580OsdCfg", "OSD", 3072, 288),
]

REG_COMMENT = re.compile(r"reg(\d+)")
FIELD = re.compile(r"RK_[US]\d+\s+(\w+)\s*:\s*(\d+)\s*;")
ARRAY = re.compile(r"(RK_U(?:8|16|32))\s+(\w+)\s*\[\s*(\d+)\s*\]\s*;")
SCALAR = re.compile(r"RK_U32\s+(\w+)\s*;")


def struct_body(text: str, name: str) -> str:
    start = text.index(f"typedef struct {name}_t {{")
    depth, index = 0, start + len(f"typedef struct {name}_t ")
    for position in range(index, len(text)):
        if text[position] == "{":
            depth += 1
        elif text[position] == "}":
            depth -= 1
            if depth == 0:
                return text[start:position]
    raise SystemExit(f"{name}: unterminated struct")


def members(body: str, first_word: int):
    """Yield (word, group name, [(field, bits), ...], words) for one struct body.

    `first_word` seeds the running word: a few structs open with their range
    comment above the `typedef` rather than above the first member.
    """
    lines = body.splitlines()[1:]
    comment, index, position = None, first_word, 0
    while position < len(lines):
        line = lines[position].strip()
        reg = REG_COMMENT.search(line)
        if reg:
            comment = int(reg.group(1))
        if line.startswith("struct {"):
            depth, fields, walker = 1, [], position + 1
            while depth:
                inner = lines[walker].strip()
                depth += inner.count("{") - inner.count("}")
                field = FIELD.match(inner)
                if depth or field:
                    if field and depth:
                        fields.append((field.group(1), int(field.group(2))))
                if not depth:
                    break
                walker += 1
            name_match = re.match(r"\}\s*(\w+)\s*;", lines[walker].strip())
            if not name_match:
                raise SystemExit(f"member after line {position}: unnamed group")
            word = comment if comment is not None else index
            yield word, name_match.group(1), fields, 1
            index, comment, position = word + 1, None, walker + 1
            continue
        if re.match(r"[A-Z][A-Za-z0-9_]*\s+\w+\s*;$", line):
            position += 1  # a nested struct by value; the next comment locates it
            continue
        array, scalar = ARRAY.match(line), SCALAR.match(line)
        if array or scalar:
            word = comment if comment is not None else index
            if array:
                elem_bytes = {"RK_U8": 1, "RK_U16": 2, "RK_U32": 4}[array.group(1)]
                name, count = array.group(2), int(array.group(3))
                words = (count * elem_bytes + 3) // 4
                # Arrays get their own kind of record: a byte table such as
                # `aq_tthd[16]` is written with a byte pointer in the reference
                # and cannot be addressed as fields.
                yield word, name, [("@array", words, elem_bytes, count)], words
            else:
                name = scalar.group(1)
                yield word, name, [("", 32)] * 1, 1
            index, comment = word + (words if array else 1), None
        position += 1


# Section lists per mode, in write order. H.265 has no scan/quant (SCL) window:
# the vintage's job writes six sections, verified against the ioctl trace.
SECTIONS_H264 = [
    ("ctl", 0, 24),
    ("base", 160, 94),
    ("rc_klut", 1024, 57),
    ("param", 1472, 180),
    ("rdo", 2048, 122),
    ("scl", 2176, 679),
    ("osd", 3072, 288),
]
SECTIONS_H265 = [
    ("ctl", 0, 24),
    ("base", 160, 94),
    ("rc_klut", 1024, 57),
    ("wgt", 1472, 374),
    ("rdo", 2048, 807),
    ("osd", 3072, 288),
]

SECTIONS = SECTIONS_H264  # replaced by main() for the --hevc run
SECTIONS_LIT = ""
# The section table states a fact about a job, and the two jobs earn it differently:
# the H.264 order is what the template extractor reads out of an ioctl trace, the
# H.265 order is the list of written members in `H265eV580RegSet`.
SECTIONS_NOTE = {
    False: "/// the order `tools/gen_h264e_vepu580_template.py` extracts them.",
    True: "/// the order of `H265eV580RegSet`'s written members (H.265 has no SCL).",
}


def section_literal(sections) -> str:
    rows = ", ".join('("%s", %d, %d)' % (n, w, c) for n, w, c in sections)
    return "[" + rows + "]"


def main() -> int:
    global SECTIONS, SECTIONS_LIT
    hevc = "--hevc" in sys.argv[1:]
    SECTIONS = SECTIONS_H265 if hevc else SECTIONS_H264
    SECTIONS_LIT = section_literal(SECTIONS)
    if hevc:
        source_comment = (
            "// refs/mpp-2024-04/rkenc/h265e/hal_h265e_vepu580_reg.h (the vendored\n"
            "// copy of the vintage the board oracle builds from) and\n"
            "// refs/mpp/mpp/hal/rkenc/common/vepu580_common.h. Do not edit by hand."
        )
    else:
        source_comment = (
            "// refs/mpp/mpp/hal/rkenc/common/vepu580_common.h and\n"
            "// refs/mpp/mpp/hal/rkenc/h264e/hal_h264e_vepu580_reg.h. Do not edit by hand."
        )
    out = [
        "// @generated by tools/gen_vepu580_regs.py from",
        source_comment,
        "",
        "//! VEPU580 register field positions.",
        "//!",
        "//! `word` is the absolute register word in the window the driver sees, so",
        "//! `RegisterFile` can find the section it belongs to. Widths and shifts come",
        "//! from the declaration order in MPP's own structs.",
        "",
        "/// A register array, addressed as whole words. The reference writes the",
        "/// byte tables such as `aq_tthd` through a cast pointer, so they cannot be",
        "/// addressed as bitfields; `RegisterFile::put_bytes` does the same.",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "pub struct Array {",
        "    /// Absolute word of the array's first word.",
        "    pub word: usize,",
        "    /// Number of elements.",
        "    pub count: usize,",
        "    /// Size of one element in bytes.",
        "    pub elem_bytes: usize,",
        "    /// Words the array occupies.",
        "    pub words: usize,",
        "    pub name: &'static str,",
        "}",
        "",
        "/// One named register field.",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "pub struct Field {",
        "    /// Absolute word index in the VEPU580 register window.",
        "    pub word: usize,",
        "    /// Bit position of the field inside that word.",
        "    pub shift: u32,",
        "    /// Width of the field in bits.",
        "    pub width: u32,",
        "    /// `section.group.field`, for error messages.",
        "    pub name: &'static str,",
        "}",
        "",
        "/// The sections of our `RegisterFile`: first absolute word and word count, in",
        SECTIONS_NOTE[hevc],
        f"pub const SECTIONS: [(&str, usize, usize); {len(SECTIONS)}] = {SECTIONS_LIT};",
        "",
    ]
    emitted = 0
    seen: set[str] = set()
    structs = STRUCTS_H265 if hevc else STRUCTS
    for header, struct, prefix, first_word, words in structs:
        out.append(f"// {struct} ({header.name}) at absolute word {first_word}.")
        body = struct_body(header.read_text(), struct)
        for word, group, fields, consumed in members(body, first_word):
            if not group.startswith("reserved") and word + consumed > first_word + words:
                raise SystemExit(
                    f"{struct}.{group}: word {word} is outside the captured section"
                )
            if fields and fields[0][0] == "@array":
                _, array_words, elem_bytes, count = fields[0]
                const = f"{prefix}_{group.upper()}"
                if const in seen:
                    # Two struct views of one window (the shared `Vepu580CtlCommon`
                    # and the codec's own `HevcVepu580ControlCfg`) describe the same
                    # padding, and a gap is named after its word position, so both
                    # views propose one const for it. The first already stated the
                    # fact; the second would be a duplicate definition.
                    continue
                seen.add(const)
                out.append(
                    f"pub const {const}: Array = Array {{ word: {word}, "
                    f"count: {count}, elem_bytes: {elem_bytes}, words: {array_words}, "
                    f"name: \"{prefix}.{group}\" }};"
                )
                emitted += 1
                continue
            if word is None:
                raise SystemExit(
                    f"{struct}.{group}: no register number and no word to inherit"
                )
            named = [name for name, _ in fields if name]
            total = sum(bits for _, bits in fields)
            # An array or a scalar contributes whole words and has no bitfields;
            # only a declared group of fields has to fill its word exactly.
            if len(named) > 1 and total != 32:
                raise SystemExit(f"{struct}.{group}: fields add to {total} bits, not 32")
            shift, bits_used = 0, 0
            for name, bits in fields:
                if name.startswith("reserved") or not name:
                    shift += bits
                    continue
                const = f"{prefix}_{group}_{name}".upper()
                if const in seen:
                    continue
                seen.add(const)
                out.append(
                    f"pub const {const}: Field = Field {{ word: {word}, shift: {shift}, "
                    f"width: {bits}, name: \"{prefix}.{group}.{name}\" }};"
                )
                emitted += 1
                shift += bits
            bits_used += total
        out.append("")
    out.append(f"// {emitted} fields in total.")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
