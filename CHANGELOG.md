# Changelog

All notable changes to the public SDK are recorded here. The project version and
the C ABI version are independent: the crate is currently `0.1.x`, while
`RK_MPP_API_VERSION_MAJOR` defines shared-library ABI compatibility.

## [Unreleased]

- The official MPP compatibility surface now follows the newer FFmpeg
  `rkmpp` lifecycle: info-change frames, external-buffer controls, encoder
  packet `KEY_INPUT_FRAME`, NULL-frame EOS and `MPP_NOK` for an empty packet
  queue.
- Imported encoder buffers now submit their DMA-BUF fd directly instead of
  falling back to NV12 CPU staging. Canonical decoded NV12 surfaces are also
  transferred into compatibility frames without packing and reallocating.
  RK3588 FFmpeg+rkmpp-rs H.264/H.265 decode improved from about 0.90 s to
  0.51 s over 600 720p frames and is slightly faster than vendor FFmpeg.
- `h264_stream` now reads input in bounded 4 MiB chunks through a reusable
  incremental access-unit scanner instead of loading the complete Annex-B file.
  On a 50 MiB / 742-frame RK3588 stream, warm wall time remains effectively
  unchanged while peak RSS falls from about 55.9 MiB to 17 MiB.
- The main `h264dec` CLI uses the same bounded scanner while retaining its
  depth-3 submission ring. On the same 50 MiB stream, wall median is
  `0.665 -> 0.665 s` and peak RSS falls from `59.5 MiB` to `20.4 MiB`.
- The stable SDK refreshes H.264 display reorder depth as SPS values arrive and
  exposes `Decoder::max_num_reorder_frames()`. Previously the initial depth
  could remain 16 even when the SPS declared 2.
- Added `Frame::convert_format()` for NV12/I420/I420P10 conversion while
  preserving PTS, DTS, POC and error metadata.
- `IepDeinterlace::import_src_frame()` can now consume decoder-owned NV12
  DMA-BUF frames directly, and its source/destination allocations, Y/UV plane
  offsets, and horizontal/vertical strides account for padded decoder surfaces.
- H.264 GDR now writes a deterministic ROI force-intra table. The stock C
  table builder leaks uninitialised bitfield stack data into DMA; the board
  oracle normalises only those undeclared bits before comparison.
- Added the RK3576 VDPP2 client-29 common/ZME scale and DCI histogram register
  builders, DMA-BUF session plumbing, and `vdpp-probe`/`vdpp-scale` entry
  points. The register images match the vendor C builder through a fake-service
  trace; RK3576 hardware validation remains pending.

## [1.14] - 2026-11-20

- Extended the V3 advanced encoder configuration's trailing 32 bytes with a
  field-selective H.265 syntax descriptor without changing the 552-byte layout
  or any V1.13 field offset.
- Exposed SAO luma/chroma disable, deblock disable and beta/tc offsets,
  loop-filter across slices/tiles and auto-tile through C and
  `rk-mpp-rs-sys`, bringing the ABI snapshot to 41 exports.
- RK3588 C V3 output is byte-identical to the CLI for SAO disable
  (15,618 bytes), deblock beta/tc (15,878) and loop-filter across
  slices/tiles (15,829). The 4608x64 auto-tile path also matches at
  8,876 bytes.

## [1.13] - 2026-11-20

- Added an independently sized V3 advanced encoder configuration and preserved
  all V1/V1.9/V1.10 layouts and create entry points.
- Exposed H.264 deblocking and entropy/transform syntax through typed C
  descriptors: CABAC, transform-8x8, `cabac_init_idc`, constraint-set
  force/value and base-layer priority ID.
- H.264 syntax flags apply fields independently, so changing
  `base_layer_pid` does not silently alter profile-derived CABAC or 8x8
  transform defaults.
- The H.264 CLI now accepts `cabac:0|1` and `trans8x8:0|1`, allowing
  profile defaults to be overridden explicitly and tested against the C V3
  path.
- Added C layout, Rust FFI and sys-crate coverage for the two new initializers
  and the V3 create path, bringing the ABI snapshot to 40 exports.

## [1.12] - 2026-11-09

- Added `DmaBufInput` and `Encoder::submit_dmabuf_frame()` for NV12 encoder
  input without copying to the encoder's internal staging surface.
- Added `rk_mpp_encoder_submit_frame_dmabuf()` with fd, allocation size and
  coded Y/UV stride metadata.
- Copied-input APIs remain unchanged. H.264, H.265 and JPEG DMA input produce
  byte-identical packets on RK3588/VEPU580 and RK3576/VEPU510+VPU720.
- `Frame::letterbox_nv12()` and the scaler can read strided NV12 CPU/DMA frames
  directly into RGB model input without a whole-frame intermediate copy.
- Added the first official-MPP compatibility symbols:
  `mpp_create`, `mpp_init`, `mpp_destroy`, `mpp_check_support_format`,
  `mpp_show_support_format`, `mpp_show_color_format`, and a layout-compatible
  `MppApi` function table. `MppPacket`, `MppFrame`, `MppBuffer`, and the
  H.264/H.265/MJPEG `decode_put_packet` / `decode_get_frame` CPU data path now
  run through an official-header C consumer. The unchanged vendor
  `mpi_dec_test` binary also passes H.264/H.265/MJPEG when `LD_PRELOAD` routes
  the public MPP entry points to this crate while vendor helper symbols remain
  available. The same object bridge now accepts vendor `MppEncCfg`, submits
  `MppFrame` pictures and returns encoded `MppPacket`s; RK3588 H.265/MJPEG streams
  are byte-identical and H.264 six-frame VCL output/reconstruction matches C
  MPP, with only the build-identity version SEI text differing. Imported
  DMA-BUF objects and the RK3576 H.264/H.265 matrix remain reserved.

## [1.11] - 2026-11-09

- H.264, 8-bit 4:2:0 H.265/VP9/AV1 and MJPEG NV12 decoding can retain their
  hardware dma-buf instead of copying into a CPU `Vec` when
  `RK_MPP_DECODER_FLAG_DMABUF_OUTPUT` is requested.
- Added Rust `DmaBufPlane` and owned fd duplication for decoded frames.
- Added `rk_mpp_decoder_export_frame_dmabuf()` for isolated plane
  offset/size/stride export. CPU-backed codecs return `UNSUPPORTED`.
- Added dma-buf import/duplication primitives and board tests on RK3588 and RK3576.
- Default H.264/H.265/VP9/AV1/MJPEG frames remain contiguous CPU storage for V1
  compatibility.

## [1.10] - 2026-11-09

- Added `rk_mpp_encoder_advanced_config_v2_t`, keeping the V1.9 104-byte
  initializer and create function unchanged.
- Exposed up to eight ROI regions, GDR/intra-refresh and slice splitting with
  low-delay packet output through typed Rust and C configuration.
- Verified H.264 and H.265 VEPU580 ROI, GDR and slice controls against their
  validated CLI paths on RK3588.
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
