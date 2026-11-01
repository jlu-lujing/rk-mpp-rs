# Vendored MPP header (2024-04 vintage)

`rkenc/h265e/hal_h265e_vepu580_reg.h` is a verbatim copy of the H.265 VEPU580
register header from Rockchip MPP commit `80957d266` (2024-04-24) - the same
commit the board's byte-level encoder oracle (`~/rk-mpp-oracles/mpp-old` on t6,
see `scripts/setup_board_refs.sh`) is built from.

* md5 `c3ea6482ea1801226a61eb00c2278ae6`, byte-identical to the oracle's copy
  (`md5sum ~/rk-mpp-oracles/mpp-old/mpp/hal/rkenc/h265e/hal_h265e_vepu580_reg.h`).
* Apache-2.0, Rockchip Electronics Co. LTD; the copyright header is intact.
* Consumed by `tools/gen_vepu580_regs.py --hevc` to generate
  `src/hal/h265e_regs.rs`.

Why this copy instead of `refs/mpp` (master): the two copies of this header
differs by roughly two thousand lines, and every byte comparison on the board is
against the vintage. Naming the vintage here keeps the generated field map tied to
the source that actually produced the goldens.

The generator still reads the *shared* CTL, BASE and RC-KLUT structs from master,
so `python3 tools/check_vepu580_vintage_drift.py` proves the two vintages agree
word for word on those windows. Run it after touching either header or the
generator.
