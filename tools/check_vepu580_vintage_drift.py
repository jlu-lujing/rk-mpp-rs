"""Check that the two MPP vintages still agree on the VEPU580 windows this crate
reads from both of them.

`tools/gen_vepu580_regs.py --hevc` builds the H.265 field map from *two* sources:
the shared CTL, BASE and RC_KLUT windows come from `refs/mpp` (master) and the
H.265-only windows come from the vendored 2024-04 vintage under `refs/`, because
the board oracle - `~/rk-mpp-oracles/mpp-old` on RK3588, commit 80957d266 - is built
from the vintage and every byte comparison is against it.

That mix is only sound while the two vintages lay the shared windows out
identically. Nothing enforces it: a silent drift would put master's field shifts
onto the vintage's hardware, which is a wrong register write rather than a
compile error. This script walks both declarations with the generator's own
parser and compares every field name that appears in both.

    python3 tools/check_vepu580_vintage_drift.py

Exit 0 = every shared field agrees on word, shift and width. Non-zero lists the
disagreements; the fix is to read that window from the vintage too, the way the
H.265 control window's own fields already are.
"""
import importlib.util
import re
import sys
from pathlib import Path

GEN = Path(__file__).resolve().parent / "gen_vepu580_regs.py"
spec = importlib.util.spec_from_file_location("gen_vepu580_regs", GEN)
gen = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gen)

# (what is compared, master header + struct, vintage header + struct, first word)
WINDOWS = [
    ("CTL", gen.COMMON, "Vepu580CtlCommon", gen.H265E, "HevcVepu580ControlCfg", 0),
    ("BASE", gen.COMMON, "Vepu580BaseCommon", gen.H265E, "HevcVepu580Base", 160),
    ("RC_KLUT", gen.COMMON, "Vepu580RcKlut", gen.H265E, "HevcVepu580RcKlut", 1024),
]


def fields(header: Path, struct: str, first_word: int):
    """`field name -> list of (word, shift, width)`, matching how the generator
    walks a struct: reserved bits advance the shift but are not named."""
    body = gen.struct_body(header.read_text(), struct)
    out = {}
    for word, group, members, _consumed in gen.members(body, first_word):
        if word is None or (members and members[0][0] == "@array"):
            continue
        shift = 0
        for name, bits in members:
            if name.startswith("reserved") or not name:
                shift += bits
                continue
            out.setdefault(name, []).append((word, shift, bits))
            shift += bits
    return out


def main() -> int:
    failed = 0
    for label, h_master, s_master, h_vintage, s_vintage, first in WINDOWS:
        if not h_vintage.exists():
            print(f"FAIL {label}: the vendored vintage header {h_vintage} is missing")
            failed += 1
            continue
        master = fields(h_master, s_master, first)
        vintage = fields(h_vintage, s_vintage, first)
        shared = sorted(set(master) & set(vintage))
        bad = [
            n
            for n in shared
            if {(w, s, b) for w, s, b in master[n]} != {(w, s, b) for w, s, b in vintage[n]}
        ]
        print(
            f"{label}: master {len(master)} fields, vintage {len(vintage)} fields, "
            f"{len(shared)} shared names, {len(bad)} disagreements"
        )
        for name in bad:
            print(f"    {name}: master {master[name]} vs vintage {vintage[name]}")
        failed += len(bad)
        if not shared:
            print(f"    (no shared field names - the structs were renamed, re-check)")
            failed += 1
    if failed:
        print("VEPU580 vintage drift check FAILED")
        return 1
    print("VEPU580 shared windows agree between master and the 2024-04 vintage")
    return 0


if __name__ == "__main__":
    sys.exit(main())
