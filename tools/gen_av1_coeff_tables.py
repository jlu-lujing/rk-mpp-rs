#!/usr/bin/env python3
"""Generate the AV1 coefficient-context tables from the specification.

    curl -sL https://aomediacodec.github.io/av1-spec/ -o /tmp/av1spec.html
    python3 tools/gen_av1_coeff_tables.py /tmp/av1spec.html > src/reference/av1/coeff_tables.rs

Emits `Adjusted_Tx_Size`, `Coeff_Base_Ctx_Offset`, `Coeff_Base_Pos_Ctx_Offset`
and `Sig_Ref_Diff_Offset`. The position-offset array is written with the
specification's SIG_COEF_CONTEXTS_2D (26) folded in: 26, 31, 36.
"""
import re
import sys

TX_NAMES = [
    "TX_4X4", "TX_8X8", "TX_16X16", "TX_32X32", "TX_64X64",
    "TX_4X8", "TX_8X4", "TX_8X16", "TX_16X8", "TX_16X32", "TX_32X16",
    "TX_32X64", "TX_64X32", "TX_4X16", "TX_16X4", "TX_8X32", "TX_32X8",
    "TX_16X64", "TX_64X16",
]

TX_RUST = {
    "TX_4X4": "Tx4x4", "TX_8X8": "Tx8x8", "TX_16X16": "Tx16x16",
    "TX_32X32": "Tx32x32", "TX_64X64": "Tx64x64", "TX_4X8": "Tx4x8",
    "TX_8X4": "Tx8x4", "TX_8X16": "Tx8x16", "TX_16X8": "Tx16x8",
    "TX_16X32": "Tx16x32", "TX_32X16": "Tx32x16", "TX_32X64": "Tx32x64",
    "TX_64X32": "Tx64x32", "TX_4X16": "Tx4x16", "TX_16X4": "Tx16x4",
    "TX_8X32": "Tx8x32", "TX_32X8": "Tx32x8", "TX_16X64": "Tx16x64",
    "TX_64X16": "Tx64x16",
}

def load(path):
    html = open(path, errors="replace").read()
    text = re.sub(r"<[^>]+>", " ", html)
    for entity, char in (("&lt;", "<"), ("&gt;", ">"), ("&amp;", "&")):
        text = text.replace(entity, char)
    return re.sub(r"\s+", " ", text)

def body(text, name):
    match = re.search(re.escape(name) + r"\s*(?:\[[^\]]*\]\s*)+=\s*\{", text)
    if not match:
        raise SystemExit(f"{name}: not found")
    start = match.end() - 1
    depth = 0
    end = start
    while end < len(text):
        if text[end] == "{":
            depth += 1
        elif text[end] == "}":
            depth -= 1
            if depth == 0:
                break
        end += 1
    return text[start:end + 1]

def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    text = load(sys.argv[1])

    adj = re.findall(r"TX_\d+X\d+", body(text, "Adjusted_Tx_Size"))
    assert len(adj) == 19, len(adj)
    offsets = [int(v) for v in re.findall(r"\d+", body(text, "Coeff_Base_Ctx_Offset"))]
    assert len(offsets) == 19 * 5 * 5, len(offsets)
    pos = body(text, "Coeff_Base_Pos_Ctx_Offset")
    pos_values = [26, 31, 36] if "SIG_COEF_CONTEXTS_2D" in pos else []
    if not pos_values:
        raise SystemExit("position offsets not understood")
    refs = [int(v) for v in re.findall(r"-?\d+", body(text, "Sig_Ref_Diff_Offset"))]
    assert len(refs) == 3 * 5 * 2, len(refs)

    fnv = 0xcbf29ce484222325
    for value in offsets + pos_values + [r & 0xFF for r in refs]:
        fnv ^= value
        fnv = (fnv * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF

    print("//! AV1 coefficient-context tables, generated from the published specification.")
    print("//!")
    print("//! `tools/gen_av1_coeff_tables.py`; `Coeff_Base_Pos_Ctx_Offset` has the")
    print("//! specification's SIG_COEF_CONTEXTS_2D (26) folded in as 26/31/36.")
    print(f"//!")
    print(f"//! FNV-1a 0x{fnv:016x} over the offsets, positions and reference deltas.")
    print()
    print("use crate::reference::av1::tx::TxSize;")
    print()
    print("/// `Adjusted_Tx_Size[ TX_SIZES_ALL ]`.")
    print("pub static ADJUSTED_TX_SIZE: [TxSize; 19] = [")
    for name in adj:
        print(f"    TxSize::{TX_RUST[name]},")
    print("];")
    print()
    print("/// `Coeff_Base_Ctx_Offset[ TX_SIZES_ALL ][ 5 ][ 5 ]` (flat).")
    print(f"pub static COEFF_BASE_CTX_OFFSET: [u8; {len(offsets)}] = [")
    for start in range(0, len(offsets), 25):
        chunk = ", ".join(str(v) for v in offsets[start:start + 25])
        print(f"    {chunk},")
    print("];")
    print()
    print("/// `Coeff_Base_Pos_Ctx_Offset[ 3 ]`.")
    print(f"pub static COEFF_BASE_POS_CTX_OFFSET: [u8; 3] = {pos_values};")
    print()
    print("/// `Sig_Ref_Diff_Offset[ 3 ][ SIG_REF_DIFF_OFFSET_NUM ][ 2 ]` (flat).")
    print(f"pub static SIG_REF_DIFF_OFFSET: [i8; {len(refs)}] = [")
    for start in range(0, len(refs), 10):
        chunk = ", ".join(str(v) for v in refs[start:start + 10])
        print(f"    {chunk},")
    print("];")
    print()
    print("#[cfg(test)]")
    print("mod tests {")
    print("    use super::*;")
    print()
    print("    #[test]")
    print("    fn the_extract_has_the_specification_shapes() {")
    print("        assert_eq!(ADJUSTED_TX_SIZE.len(), 19);")
    print("        assert_eq!(COEFF_BASE_CTX_OFFSET.len(), 19 * 5 * 5);")
    print("        assert_eq!(COEFF_BASE_POS_CTX_OFFSET, [26, 31, 36]);")
    print("        assert_eq!(SIG_REF_DIFF_OFFSET.len(), 3 * 5 * 2);")
    print("        assert_eq!(COEFF_BASE_CTX_OFFSET[..5], [0, 1, 6, 6, 0]);")
    print("        assert_eq!(ADJUSTED_TX_SIZE[4], TxSize::Tx32x32);")
    print("        assert_eq!(ADJUSTED_TX_SIZE[18], TxSize::Tx32x16);")
    print("    }")
    print("}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
