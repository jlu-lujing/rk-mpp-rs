# Changelog

All notable changes to the public SDK are recorded here. The project version and
the C ABI version are independent: the crate is currently `0.1.x`, while
`RK_MPP_API_VERSION_MAJOR` defines shared-library ABI compatibility.

## [1.10] - 2026-11-09

- Added `rk_mpp_encoder_advanced_config_v2_t`, keeping the V1.9 104-byte
  initializer and create function unchanged.
- Exposed up to eight ROI regions, GDR/intra-refresh and slice splitting with
  low-delay packet output through typed Rust and C configuration.
- Verified H.264 and H.265 VEPU580 ROI, GDR and slice controls against their
  validated CLI paths on t6.
- Added the raw `rk-mpp-rs-sys` workspace crate with all 34 V1.10 exports,
  exact C layouts, explicit-path/pkg-config discovery and dynamic/static tests.

## [1.9] - 2026-11-09

- Added `rk_mpp_encoder_advanced_config_t` and
  `rk_mpp_encoder_rate_control_t` without changing the 68-byte V1.8 encoder
  configuration.
- Exposed Fixed-QP, CBR, VBR and AVBR rate control, bitrate bounds and QP bounds
  through Rust and C for VEPU580 H.264 and H.265 sessions.
- Preserved `rk_mpp_encoder_create()` and every V1.8 struct offset and symbol.

## [1.8] - 2026-11-09

### Added

- Stable Rust decoder and encoder handles for every implemented codec.
- Versioned C ABI in `include/rk_mpp_rs.h`.
- Streaming H.264/H.265 input, packet-oriented VP8/VP9/AV1 input, fragmented
  MJPEG input, and buffered MPEG-2/MPEG-4/H.263 input.
- NV12, I420 and I420P10 frame output.
- H.264, H.265 and JPEG packet output with explicit release.
- `cdylib`, `staticlib` and `rlib` build products.
- Linux SONAME, configurable install layout, pkg-config and CMake packages.
- ABI symbol/layout snapshots and standalone Rust/C/C++ consumers.

### Compatibility

- API major `1` is the shared-library ABI boundary.
- Existing `rk_jpeg_open`, `rk_jpeg_frame` and `rk_jpeg_close` symbols remain
  available for `rknn-fastinfer`.

## Future

- Advanced encoder control through the stable ABI.
- DMA-BUF frame import/export, fences and asynchronous completion.
- Official Rockchip MPP ABI compatibility.
