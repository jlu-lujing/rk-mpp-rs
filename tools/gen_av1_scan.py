#!/usr/bin/env python3
"""Generate the AV1 coefficient scan-order tables from the specification.

    curl -sL https://aomediacodec.github.io/av1-spec/ -o /tmp/av1spec.html
    python3 tools/gen_av1_scan.py /tmp/av1spec.html > src/reference/av1/scan_defaults.rs

The specification lists every Default/Mcol/Mrow scan as
`Name [ N ] = { ... }`; each is emitted as a flat u16 slice in table order.
AV1 has no >32x32 scan, so 64x64 and the 64-wide rectangles are absent.
"""
import re
import sys

def load(path):
    html = open(path, errors="replace").read()
    text = re.sub(r"<[^>]+>", " ", html)
    for entity, char in (("&lt;", "<"), ("&gt;", ">"), ("&amp;", "&")):
        text = text.replace(entity, char)
    return re.sub(r"\s+", " ", text)

def tables(text):
    pattern = re.compile(
        r"((?:Default|Mcol|Mrow)_Scan_\d+x\d+)\s*\[\s*(\d+)\s*\]\s*=\s*\{([^}]*)\}"
    )
    out = []
    for match in pattern.finditer(text):
        values = [int(v) for v in re.findall(r"\d+", match.group(3))]
        if len(values) != int(match.group(2)):
            raise SystemExit(f"{match.group(1)}: expected {match.group(2)} values")
        out.append((match.group(1), values))
    return out

def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    parsed = tables(load(sys.argv[1]))
    fnv = 0xcbf29ce484222325
    total = 0
    for _, values in parsed:
        for value in values:
            fnv ^= value
            fnv = (fnv * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
        total += len(values)

    print("//! AV1 coefficient scan orders, generated from the published specification.")
    print("//!")
    print("//! `tools/gen_av1_scan.py` parses the appendix tables; each scan is a flat")
    print("//! `u16` array in scan order (position index, not coefficient value).")
    print("//!")
    print(f"//! {len(parsed)} tables, {total} entries, FNV-1a 0x{fnv:016x}.")
    print()
    print("/// One scan order.")
    print("pub struct ScanTable {")
    print("    pub name: &'static str,")
    print("    pub order: &'static [u16],")
    print("}")
    print()
    for name, values in parsed:
        print(f"pub static {name.upper()}: ScanTable = ScanTable {{")
        print(f'    name: "{name}",')
        print("    order: &[")
        for start in range(0, len(values), 16):
            chunk = ", ".join(str(v) for v in values[start:start + 16])
            print(f"        {chunk},")
        print("    ],")
        print("};")
        print()
    print("/// Every generated scan, in specification order.")
    print(f"pub static TABLES: [&ScanTable; {len(parsed)}] = [")
    for name, _ in parsed:
        print(f"    &{name.upper()},")
    print("];")
    print()
    print("#[cfg(test)]")
    print("mod tests {")
    print("    use super::*;")
    print()
    print("    #[test]")
    print("    fn the_generated_set_matches_the_specification_extract() {")
    print(f"        assert_eq!(TABLES.len(), {len(parsed)});")
    print("        let total: usize = TABLES.iter().map(|table| table.order.len()).sum();")
    print(f"        assert_eq!(total, {total});")
    print("        let mut hash = 0xcbf29ce484222325u64;")
    print("        for table in TABLES {")
    print("            for value in table.order {")
    print("                hash ^= u64::from(*value);")
    print("                hash = hash.wrapping_mul(0x100000001b3);")
    print("            }")
    print("        }")
    print(f"        assert_eq!(hash, 0x{fnv:016x});")
    print("    }")
    print()
    print("    #[test]")
    print("    fn four_by_four_scans_are_the_specification_rows() {")
    print("        assert_eq!(&DEFAULT_SCAN_4X4.order[..8], &[0, 1, 4, 8, 5, 2, 3, 6]);")
    print("        assert_eq!(&MCOL_SCAN_4X4.order[..8], &[0, 4, 8, 12, 1, 5, 9, 13]);")
    print("        assert_eq!(&MROW_SCAN_4X4.order[..8], &[0, 1, 2, 3, 4, 5, 6, 7]);")
    print("    }")
    print("}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
