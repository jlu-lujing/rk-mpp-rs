#!/usr/bin/env python3
"""Generate the AV1 default CDF tables from the published specification.

    curl -sL https://aomediacodec.github.io/av1-spec/ -o /tmp/av1spec.html
    python3 tools/gen_av1_cdf.py /tmp/av1spec.html > src/reference/av1/cdf_defaults.rs
    cargo fmt -- src/reference/av1/cdf_defaults.rs

The generated file is not rustfmt-shaped; run `cargo fmt` after regenerating.

Only the non-coefficient tables are emitted: the specification lists 74 arrays
between the default tables and the coefficient section, and every value is a
15-bit probability (or the trailing adaptation counter) so they fit a u16.
The declared dimension expressions are kept verbatim as `shape` because they
reference specification constants; the flat order is the specification's own
row-major order.
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
    pattern = re.compile(r"([A-Z][A-Za-z0-9_]+_Cdf)\s*((?:\[[^\]]*\]\s*)+)=\s*\{")
    out = []
    for match in pattern.finditer(text):
        name = match.group(1)
        shape = [part.strip() for part in re.findall(r"\[([^\]]*)\]", match.group(2))]
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
        body = text[start:end + 1]
        values = []
        safe = re.compile(r"^[0-9+*() \t-]+$")
        flat = body.replace("{", " ").replace("}", " ")
        for token in flat.split(","):
            token = token.strip()
            if not token:
                continue
            if token.isdigit() or (token.startswith("-") and token[1:].isdigit()):
                values.append(int(token))
            elif safe.match(token) and any(op in token for op in "*+"):
                # The specification writes products such as `128 * 125`.
                value = eval(token, {"__builtins__": {}}, {})
                values.append(int(value))
            else:
                raise SystemExit(f"{name}: unexpected value {token!r}")
        out.append((name, shape, values))
    return out

def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    parsed = tables(load(sys.argv[1]))
    fnv = 0xcbf29ce484222325
    total = 0
    for _, _, values in parsed:
        for value in values:
            fnv ^= value & 0xFFFF
            fnv = (fnv * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
        total += len(values)

    print("//! AV1 default CDF tables, generated from the published specification.")
    print("//!")
    print("//! `tools/gen_av1_cdf.py` parses the tables section of")
    print("//! <https://aomediacodec.github.io/av1-spec/> and emits every non-coefficient")
    print("//! `Default_*_Cdf` array in the specification's row-major order. `shape` keeps the")
    print("//! declared dimension expressions so a reader can trace a flat index back.")
    print("//!")
    print(f"//! {len(parsed)} tables, {total} values, FNV-1a 0x{fnv:016x}.")
    print()
    print("/// One specification table: the declared shape and its flat row-major values.")
    print("pub struct CdfTable {")
    print("    pub name: &'static str,")
    print("    pub shape: &'static [&'static str],")
    print("    pub values: &'static [u16],")
    print("}")
    print()
    for index, (name, shape, values) in enumerate(parsed):
        ident = name.upper()
        shape_rs = ", ".join(f'"{part}"' for part in shape)
        print(f"pub static {ident}: CdfTable = CdfTable {{")
        print(f'    name: "{name}",')
        print(f"    shape: &[{shape_rs}],")
        print("    values: &[")
        for start in range(0, len(values), 12):
            chunk = ", ".join(str(v) for v in values[start:start + 12])
            print(f"        {chunk},")
        print("    ],")
        print("};")
        print()
    print("/// Every generated table, in specification order.")
    print(f"pub static TABLES: [&CdfTable; {len(parsed)}] = [")
    for name, _, _ in parsed:
        print(f"    &{name.upper()},")
    print("];")
    print()
    print("#[cfg(test)]")
    print("mod tests {")
    print("    use super::*;")
    print()
    print("    fn fnv(values: &[u16]) -> u64 {")
    print("        values.iter().fold(0xcbf29ce484222325u64, |mut hash, value| {")
    print("            hash ^= u64::from(*value);")
    print("            hash.wrapping_mul(0x100000001b3)")
    print("        })")
    print("    }")
    print()
    print("    #[test]")
    print("    fn the_generated_set_matches_the_specification_extract() {")
    print(f"        assert_eq!(TABLES.len(), {len(parsed)});")
    print(f"        let total: usize = TABLES.iter().map(|table| table.values.len()).sum();")
    print(f"        assert_eq!(total, {total});")
    print("        let mut hash = 0xcbf29ce484222325u64;")
    print("        for table in TABLES {")
    print("            for value in table.values {")
    print("                hash ^= u64::from(*value);")
    print("                hash = hash.wrapping_mul(0x100000001b3);")
    print("            }")
    print("        }")
    print(f"        assert_eq!(hash, 0x{fnv:016x});")
    print("    }")
    print()
    print("    #[test]")
    print("    fn partition_w8_starts_with_the_specification_row() {")
    print("        assert_eq!(DEFAULT_PARTITION_W8_CDF.shape, &[\"PARTITION_CONTEXTS\", \"5\"]);")
    print("        assert_eq!(&DEFAULT_PARTITION_W8_CDF.values[..5], &[19132, 25510, 30392, 32768, 0]);")
    print("    }")
    print()
    print("}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
