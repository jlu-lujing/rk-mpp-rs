# rk-mpp-rs-sys

Raw Rust FFI declarations for the `rk-mpp-rs` V1.10 C ABI. This crate exposes
the exact C layouts, constants and functions; callers must uphold the lifetime
and threading rules from `rk_mpp_rs.h`.

Build the native workspace library first:

```bash
cargo build --release -p rk-mpp-rs --lib
```

Use the in-tree `target/release` directory:

```bash
RK_MPP_RS_SYS_LIB_DIR="$PWD/target/release" \
LD_LIBRARY_PATH="$PWD/target/release" \
cargo test --release -p rk-mpp-rs-sys
```

Use an installed pkg-config package:

```bash
RK_MPP_RS_SYS_USE_PKG_CONFIG=1 \
PKG_CONFIG_PATH=/usr/local/lib/pkgconfig \
LD_LIBRARY_PATH=/usr/local/lib \
cargo test --release -p rk-mpp-rs-sys
```

Set `RK_MPP_RS_SYS_STATIC=1` to link `librk_mpp_rs.a`. For release static
builds, also set `CARGO_PROFILE_RELEASE_LTO=false`; Rust LTO otherwise tries to
read bitcode from the native static archive.

The build script also falls back to the workspace `target/<profile>` directory
when no explicit environment variable is provided. A shared-library consumer
must still make `librk_mpp_rs.so.1` available through the dynamic loader.
