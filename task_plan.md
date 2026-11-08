# Task Plan: Public Codec SDK

## Goal
Provide stable, versioned Rust and C library interfaces for hardware codec use,
prioritizing persistent H.264 and MJPEG decode sessions, then extending the
same contract to the other implemented codecs and encoders.

## Current Phase
Phases 1-8 are complete. The SDK is now installable and ABI-checked. Phase 9
extends the stable encoder configuration surface without breaking old V1
callers.

## Phases

### Phase 1: API Contract and Discovery
- [x] Define ownership, packet/frame lifetime, error and flush semantics.
- [x] Audit H.264 and JPEG internal APIs for reusable primitives.
- [x] Add findings and API compatibility decisions.
- **Status:** complete

### Phase 2: Versioned Header and Shared Runtime
- [x] Add a C-compatible header with fixed-width structs and API version.
- [x] Add common packet/frame queues, errors and output-format metadata.
- [x] Export `cdylib` while preserving `rlib` and `staticlib`.
- **Status:** complete

### Phase 3: H.264 Decode Backend
- [x] Implement arbitrary-chunk Annex-B packet input and bounded reordering.
- [x] Expose NV12 and planar YUV output without file-path coupling.
- [x] Implement flush, reset, EOS, status and recovery.
- **Status:** complete

### Phase 4: MJPEG Decode Backend
- [x] Implement packet-stable JPEG framing and a persistent public session.
- [x] Expose deterministic NV12 and planar YUV output.
- [x] Cover concatenated packet input, EOS and malformed/state errors.
- **Status:** complete with fresh DMA/service resources per image to avoid the
  hardware postprocessor writeback race.

### Phase 5: C/C++ Integration Gates
- [x] Compile and run a C harness against the shared library/header.
- [x] Compile and run linked C++ smoke tests for shared and static libraries.
- [x] Verify H.264 and MJPEG outputs against existing board goldens.
- **Status:** complete on lf1 and t6.

### Phase 6: Other Codec Interfaces
- [ ] Extend the decoder contract to HEVC, VP9, AV1, VP8, MPEG-2, MPEG-4, H.263.
- [ ] Add the same contract to JPEG single-image decode and legacy `rk_jpeg_*`.
- [ ] Add encoder interfaces for the implemented H.264, H.265 and JPEG paths.
- **Status:** in_progress

### Phase 6.1: H.265/HEVC Decode Backend
- [x] Add H.265 codec IDs and 10-bit planar output to the V1 header.
- [x] Implement arbitrary-chunk Annex-B framing with parameter-set carryover.
- [x] Track SPS reorder depth and emit display order across epochs.
- [x] Verify 8-bit NV12/I420 and Main10 I420P10 output against C MPP.
- **Status:** complete on lf1 and t6.

### Phase 6.2: VP9 Decode Backend
- [x] Add the VP9 codec ID and packet-oriented input rules.
- [x] Expose 8-bit NV12/I420 and 10-bit I420P10 output.
- [x] Verify profile 0/2, inter and hidden-alt-ref packets against C MPP.
- **Status:** complete on lf1 and t6.

### Phase 6.3: AV1 Decode Backend
- [x] Add the AV1 codec ID and temporal-unit packet input rules.
- [x] Expose planar 8-bit NV12/I420 and 10-bit I420P10 output.
- [x] Verify RK3588 and RK3576 VDPU383 key/inter/show-existing packets.
- **Status:** complete on lf1 and t6.

### Phase 6.4: Encoder API Contract
- [x] Define frame input ownership, packet output and EOS/flush rules.
- [x] Add H.264, H.265 and JPEG encoder handles to Rust and C.
- [x] Verify output against the existing C-validated CLI goldens.
- **Status:** complete.

### Phase 6.4.1: H.264 Encoder API
- [x] Expose synchronous submit/receive with copied NV12/I420 input.
- [x] Add fixed-QP profile, level, GOP, QP and frame-rate configuration.
- [x] Verify multi-frame GOP byte equality on lf1 and t6.
- **Status:** complete for VEPU580 and the currently captured VEPU510 path.

### Phase 6.4.2: H.265 Encoder API
- [x] Select VEPU580 or VEPU510 from the configured SoC.
- [x] Expose copied NV12/I420 input and owned Annex-B packet output.
- [x] Verify fixed-QP IDR/GOP output on lf1 and t6.
- **Status:** complete for VEPU580 and the captured VEPU510 path.

### Phase 6.4.3: JPEG Encoder API
- [x] Add deterministic NV12/I420 input and JPEG packet output.
- [x] Expose quality and restart configuration for 4:2:0.
- [x] Verify byte equality on lf1 and t6.
- **Status:** complete.

### Phase 6.5: Legacy Decoder Interfaces
- [x] Add VP8 packet/frame output to the shared decoder contract.
- [x] Add MPEG-2, MPEG-4 and H.263 stream/packet backends.
- [x] Cover Annex-B, elementary-stream and codec packet framing across SoCs.
- **Status:** complete where hardware exists.

### Phase 6.5.1: VP8 Decode Backend
- [x] Add the VP8 codec ID and IVF packet rules.
- [x] Expose NV12/I420 output with the shared decoder lifecycle.
- [x] Verify key/inter/alt-ref packets against C MPP on t6; lf1 is a hardware
      platform SKIP.
- **Status:** complete on t6; lf1 is a hardware platform SKIP.

### Phase 6.5.2: MPEG-2 / MPEG-4 / H.263 Decoder Backends
- [x] Add the three codec IDs and elementary-stream framing rules.
- [x] Expose NV12/I420 output with reset/release lifecycle.
- [x] Verify one representative stream per codec on t6; lf1 is a VDPU2
      hardware SKIP.
- **Status:** complete.

### Phase 7: Full Verification and Handoff
- [x] Run host-independent gates on lf1 and t6.
- [x] Run H.264/MJPEG C/C++ consumer tests on lf1.
- [x] Run the complete suite on lf1 and the 80% OPP-capped t6 gate.
- [x] Re-run the H.264/MJPEG C/C++ consumer test on t6.
- [x] Update and commit the H.264/MJPEG SDK slice.
- [x] Run lf1/t6 full gates and both C/C++ boards for H.265.
- [x] Update and commit the H.265/API 1.1 slice.
- [x] Run lf1/t6 C/C++ gates for VP9 profile 0 and profile 2.
- [x] Run final VP9 full suite and commit the API 1.2 slice.
- [x] Run lf1/t6 C/C++ gates for AV1 8-bit and 10-bit.
- [x] Run final AV1 full suite and commit the API 1.3 slice.
- [x] Run lf1/t6 H.264 encoder API gates and full suites.
- [x] Commit the H.264 encoder/API 1.4 slice.
- [x] Run lf1/t6 H.265 encoder API gates.
- [x] Run final H.265 encoder full suite and commit the API 1.5 slice.
- [x] Run lf1/t6 JPEG encoder API gates including restart MCU.
- [x] Run final JPEG encoder full suite and commit the API 1.6 slice.
- [x] Run t6 VP8 C API gate and lf1 platform-SKIP audit.
- [x] Run final VP8 full suite and commit the API 1.7 slice.
- [x] Run t6 MPEG-2/MPEG-4/H.263 C API gates and lf1 platform audit.
- [x] Run final legacy-decoder full suite and commit the API 1.8 slice.
- **Status:** complete

### Phase 8: Product-Ready SDK Packaging
- [x] Add Linux SONAME and a versioned shared-library install layout.
- [x] Add relocatable pkg-config and CMake package files.
- [x] Add ABI symbol/layout snapshots and a clean consumer check.
- [x] Add minimal Rust and C/C++ consumer examples, CHANGELOG and license files.
- **Status:** complete on lf1 and t6.

### Phase 8.1: Build and Install
- [x] Embed `librk_mpp_rs.so.1` as the Linux SONAME.
- [x] Install header, static/shared libraries and symlinks under a configurable prefix.
- [x] Support `DESTDIR` and verify from t6 and lf1.
- **Status:** complete.

### Phase 8.2: Package Metadata
- [x] Install `rk-mpp-rs.pc`.
- [x] Install a relocatable `rk-mpp-rsConfig.cmake`.
- [x] Compile pkg-config and CMake consumers against a temporary prefix.
- **Status:** complete.

### Phase 8.3: ABI Freeze
- [x] Snapshot exported `rk_mpp_*` and legacy `rk_jpeg_*` symbols.
- [x] Add C11 static assertions for enum values, struct sizes and field offsets.
- [x] Verify SONAME, symbols and layout on lf1 and t6.
- **Status:** complete.

### Phase 8.4: Distribution
- [x] Add Rust decode/encode examples and C/C++ consumer examples.
- [x] Add CHANGELOG, NOTICE and dual-license texts.
- [x] Verify packaged consumers after installation.
- **Status:** complete.

### Phase 9: Backward-Compatible Advanced Encoder ABI
- [x] Accept older shorter encoder-config prefixes and append new fields.
- [x] Expose rate-control modes and bitrate targets through Rust/C.
- [x] Add ROI/GDR/slice controls without changing existing offsets.
- [x] Verify old V1.8 and new V1.9 consumers against the same library.
- **Status:** complete on VEPU580; newer-only streaming controls are explicit
  platform boundaries on RK3576.

### Phase 9.1: Stable Rate-Control Configuration
- [x] Add a separately sized advanced encoder config and rate-control config.
- [x] Keep the 68-byte V1 encoder config and existing create path unchanged.
- [x] Map Fixed-QP/CBR/VBR/AVBR, bitrate bounds and QP bounds into Rust.
- [x] Verify VEPU580 H.264/H.265 ABI output against the validated CLI paths.
- [x] Re-run old V1.8 C consumers against the V1.9 library.
- **Status:** complete on t6 for VEPU580; RK3576 streaming RC is an explicit
  unsupported boundary.

### Phase 9.2: ROI, GDR and Slice Controls
- [x] Add a separately versioned advanced-config v2 ABI.
- [x] Add typed Rust ROI, GDR and slice configuration.
- [x] Wire H.264/H.265 VEPU580 controls to the existing byte-exact paths.
- [x] Verify every control against its validated CLI path on t6.
- [x] Re-run V1.9 advanced and V1.8 base consumers against V1.10.
- **Status:** complete on t6/VEPU580; RK3576 streaming controls are an explicit
  platform SKIP.

### Phase 10: rk-mpp-rs-sys Raw Bindings
- [x] Add a workspace member crate with complete V1.10 raw declarations.
- [x] Discover explicit, pkg-config and in-tree native libraries.
- [x] Verify every public C struct size/offset and function link.
- [x] Run version/init smoke consumers on t6 and lf1.
- [x] Document shared/static linking for downstream Rust projects.
- **Status:** complete on t6 and lf1.

### Phase 11: DMA-BUF Frame Export
- [x] Add imported dma-buf ownership and safe fd duplication.
- [x] Preserve MJPEG hardware output as frame-owned DMA-BUF.
- [x] Expose Rust and C plane fd/offset/size/stride metadata.
- [x] Keep CPU frames and frame lifetime semantics backward compatible.
- [x] Verify fd export and byte-identical visible planes on t6/lf1.
- **Status:** complete on t6 and lf1 for MJPEG NV12; CPU-backed codecs remain an
  explicit unsupported boundary.

### Phase 12: H.264 DMA-BUF Frame Output
- [x] Retain a cloned DPB buffer lease through display reordering.
- [x] Export strided H.264 NV12 Y/UV planes without CPU repacking.
- [x] Preserve default CPU/POC-reordered output exactly.
- [x] Verify H.264 visible bytes and fd metadata against the C oracle.
- [x] Re-run package, workspace and DMA-BUF gates on t6/lf1.
- **Status:** complete on t6 and lf1.

## Decisions Made

| Decision | Rationale |
|----------|-----------|
| Preserve existing `rk_jpeg_*` symbols | `rknn-fastinfer` already depends on the legacy surface |
| Add `rk_mpp_*` V1 decoder API beside legacy symbols | Avoid ABI breakage while replacing path-bound JPEG calls |
| Use packet/chunk input rather than file paths | Required for cameras, containers and live streams |
| Return decoder-owned frame storage until explicit release | Avoid per-frame allocation APIs and hidden pointer invalidation |
| Keep new V1 output to NV12/I420/I420P10 | Hardware JPEG RGB writeback is not deterministic; the legacy `rk_jpeg_*` RGB path remains intact |
| Recreate JPEG DMA/service resources per image | Reuse produced an eight-byte stale writeback even after service reopen |
| Copy input before returning from submit | Camera and socket buffers may be reused immediately by the caller |

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| H.264 SPS/PPS between pictures were dropped at an AU boundary | 1 | Move the complete non-VCL suffix into the next access unit |
