# Task Plan: Public Codec SDK

## Goal
Provide stable, versioned Rust and C library interfaces for hardware codec use,
prioritizing persistent H.264 and MJPEG decode sessions, then extending the
same contract to the other implemented codecs and encoders.

## Current Phase
Phase 26 is the active workstream: remove the remaining FFmpeg/MPP
compatibility bottlenecks and make VEPU580 encode scheduling asynchronous.

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
- **Status:** complete on RK3576 and RK3588.

### Phase 6: Other Codec Interfaces
- [x] Extend the decoder contract to HEVC, VP9, AV1, VP8, MPEG-2, MPEG-4, H.263.
- [x] Add the same contract to JPEG single-image decode and legacy `rk_jpeg_*`.
- [x] Add encoder interfaces for the implemented H.264, H.265 and JPEG paths.
- **Status:** complete where the corresponding hardware exists.

### Phase 6.1: H.265/HEVC Decode Backend
- [x] Add H.265 codec IDs and 10-bit planar output to the V1 header.
- [x] Implement arbitrary-chunk Annex-B framing with parameter-set carryover.
- [x] Track SPS reorder depth and emit display order across epochs.
- [x] Verify 8-bit NV12/I420 and Main10 I420P10 output against C MPP.
- **Status:** complete on RK3576 and RK3588.

### Phase 6.2: VP9 Decode Backend
- [x] Add the VP9 codec ID and packet-oriented input rules.
- [x] Expose 8-bit NV12/I420 and 10-bit I420P10 output.
- [x] Verify profile 0/2, inter and hidden-alt-ref packets against C MPP.
- **Status:** complete on RK3576 and RK3588.

### Phase 6.3: AV1 Decode Backend
- [x] Add the AV1 codec ID and temporal-unit packet input rules.
- [x] Expose planar 8-bit NV12/I420 and 10-bit I420P10 output.
- [x] Verify RK3588 and RK3576 VDPU383 key/inter/show-existing packets.
- **Status:** complete on RK3576 and RK3588.

### Phase 6.4: Encoder API Contract
- [x] Define frame input ownership, packet output and EOS/flush rules.
- [x] Add H.264, H.265 and JPEG encoder handles to Rust and C.
- [x] Verify output against the existing C-validated CLI goldens.
- **Status:** complete.

### Phase 6.4.1: H.264 Encoder API
- [x] Expose synchronous submit/receive with copied NV12/I420 input.
- [x] Add fixed-QP profile, level, GOP, QP and frame-rate configuration.
- [x] Verify multi-frame GOP byte equality on RK3576 and RK3588.
- **Status:** complete for VEPU580 and the currently captured VEPU510 path.

### Phase 6.4.2: H.265 Encoder API
- [x] Select VEPU580 or VEPU510 from the configured SoC.
- [x] Expose copied NV12/I420 input and owned Annex-B packet output.
- [x] Verify fixed-QP IDR/GOP output on RK3576 and RK3588.
- **Status:** complete for VEPU580 and the captured VEPU510 path.

### Phase 6.4.3: JPEG Encoder API
- [x] Add deterministic NV12/I420 input and JPEG packet output.
- [x] Expose quality and restart configuration for 4:2:0.
- [x] Verify byte equality on RK3576 and RK3588.
- **Status:** complete.

### Phase 6.5: Legacy Decoder Interfaces
- [x] Add VP8 packet/frame output to the shared decoder contract.
- [x] Add MPEG-2, MPEG-4 and H.263 stream/packet backends.
- [x] Cover Annex-B, elementary-stream and codec packet framing across SoCs.
- **Status:** complete where hardware exists.

### Phase 6.5.1: VP8 Decode Backend
- [x] Add the VP8 codec ID and IVF packet rules.
- [x] Expose NV12/I420 output with the shared decoder lifecycle.
- [x] Verify key/inter/alt-ref packets against C MPP on RK3588; RK3576 is a hardware
      platform SKIP.
- **Status:** complete on RK3588; RK3576 is a hardware platform SKIP.

### Phase 6.5.2: MPEG-2 / MPEG-4 / H.263 Decoder Backends
- [x] Add the three codec IDs and elementary-stream framing rules.
- [x] Expose NV12/I420 output with reset/release lifecycle.
- [x] Verify one representative stream per codec on RK3588; RK3576 is a VDPU2
      hardware SKIP.
- **Status:** complete.

### Phase 7: Full Verification and Handoff
- [x] Run host-independent gates on RK3576 and RK3588.
- [x] Run H.264/MJPEG C/C++ consumer tests on RK3576.
- [x] Run the complete suite on RK3576 and the 80% OPP-capped RK3588 gate.
- [x] Re-run the H.264/MJPEG C/C++ consumer test on RK3588.
- [x] Update and commit the H.264/MJPEG SDK slice.
- [x] Run RK3576/RK3588 full gates and both C/C++ boards for H.265.
- [x] Update and commit the H.265/API 1.1 slice.
- [x] Run RK3576/RK3588 C/C++ gates for VP9 profile 0 and profile 2.
- [x] Run final VP9 full suite and commit the API 1.2 slice.
- [x] Run RK3576/RK3588 C/C++ gates for AV1 8-bit and 10-bit.
- [x] Run final AV1 full suite and commit the API 1.3 slice.
- [x] Run RK3576/RK3588 H.264 encoder API gates and full suites.
- [x] Commit the H.264 encoder/API 1.4 slice.
- [x] Run RK3576/RK3588 H.265 encoder API gates.
- [x] Run final H.265 encoder full suite and commit the API 1.5 slice.
- [x] Run RK3576/RK3588 JPEG encoder API gates including restart MCU.
- [x] Run final JPEG encoder full suite and commit the API 1.6 slice.
- [x] Run RK3588 VP8 C API gate and RK3576 platform-SKIP audit.
- [x] Run final VP8 full suite and commit the API 1.7 slice.
- [x] Run RK3588 MPEG-2/MPEG-4/H.263 C API gates and RK3576 platform audit.
- [x] Run final legacy-decoder full suite and commit the API 1.8 slice.
- **Status:** complete

### Phase 8: Product-Ready SDK Packaging
- [x] Add Linux SONAME and a versioned shared-library install layout.
- [x] Add relocatable pkg-config and CMake package files.
- [x] Add ABI symbol/layout snapshots and a clean consumer check.
- [x] Add minimal Rust and C/C++ consumer examples, CHANGELOG and license files.
- **Status:** complete on RK3576 and RK3588.

### Phase 8.1: Build and Install
- [x] Embed `librk_mpp_rs.so.1` as the Linux SONAME.
- [x] Install header, static/shared libraries and symlinks under a configurable prefix.
- [x] Support `DESTDIR` and verify from RK3588 and RK3576.
- **Status:** complete.

### Phase 8.2: Package Metadata
- [x] Install `rk-mpp-rs.pc`.
- [x] Install a relocatable `rk-mpp-rsConfig.cmake`.
- [x] Compile pkg-config and CMake consumers against a temporary prefix.
- **Status:** complete.

### Phase 8.3: ABI Freeze
- [x] Snapshot exported `rk_mpp_*` and legacy `rk_jpeg_*` symbols.
- [x] Add C11 static assertions for enum values, struct sizes and field offsets.
- [x] Verify SONAME, symbols and layout on RK3576 and RK3588.
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
- **Status:** complete on RK3588 for VEPU580; RK3576 streaming RC is an explicit
  unsupported boundary.

### Phase 9.2: ROI, GDR and Slice Controls
- [x] Add a separately versioned advanced-config v2 ABI.
- [x] Add typed Rust ROI, GDR and slice configuration.
- [x] Wire H.264/H.265 VEPU580 controls to the existing byte-exact paths.
- [x] Verify every control against its validated CLI path on RK3588.
- [x] Re-run V1.9 advanced and V1.8 base consumers against V1.10.
- **Status:** complete on RK3588/VEPU580; RK3576 streaming controls are an explicit
  platform SKIP.

### Phase 10: rk-mpp-rs-sys Raw Bindings
- [x] Add a workspace member crate with complete V1.10 raw declarations.
- [x] Discover explicit, pkg-config and in-tree native libraries.
- [x] Verify every public C struct size/offset and function link.
- [x] Run version/init smoke consumers on RK3588 and RK3576.
- [x] Document shared/static linking for downstream Rust projects.
- **Status:** complete on RK3588 and RK3576.

### Phase 11: DMA-BUF Frame Export
- [x] Add imported dma-buf ownership and safe fd duplication.
- [x] Preserve MJPEG hardware output as frame-owned DMA-BUF.
- [x] Expose Rust and C plane fd/offset/size/stride metadata.
- [x] Keep CPU frames and frame lifetime semantics backward compatible.
- [x] Verify fd export and byte-identical visible planes on RK3588/RK3576.
- **Status:** complete on RK3588 and RK3576 for MJPEG NV12; CPU-backed codecs remain an
  explicit unsupported boundary.

### Phase 12: H.264 DMA-BUF Frame Output
- [x] Retain a cloned DPB buffer lease through display reordering.
- [x] Export strided H.264 NV12 Y/UV planes without CPU repacking.
- [x] Preserve default CPU/POC-reordered output exactly.
- [x] Verify H.264 visible bytes and fd metadata against the C oracle.
- [x] Re-run package, workspace and DMA-BUF gates on RK3588/RK3576.
- **Status:** complete on RK3588 and RK3576.

### Phase 13: H.265 DMA-BUF Frame Output
- [x] Share the pooled decoded-surface type between H.264 and H.265.
- [x] Retain the H.265 video lease through picture reordering.
- [x] Export 8-bit 4:2:0 H.265 as strided NV12 Y/UV planes.
- [x] Preserve default CPU/planar and H.265 Main10 behavior.
- [x] Verify RK3588 against the C oracle and RK3576 VDPU383 DMA against CPU output.
- [x] Re-run package, workspace and DMA-BUF gates on RK3588/RK3576.
- **Status:** complete on RK3588 and RK3576 for 8-bit 4:2:0 NV12.

### Phase 14: VP9 DMA-BUF Frame Output
- [x] Retain the VP9 video lease for ordinary and show-existing pictures.
- [x] Export 8-bit 4:2:0 VP9 as strided NV12 Y/UV planes.
- [x] Preserve default CPU/planar and VP9 10-bit behavior.
- [x] Write only visible rows in the IVF consumer instead of strided padding.
- [x] Verify RK3588 against the C oracle and RK3576 DMA against CPU output.
- [x] Re-run package, workspace and DMA-BUF gates on RK3588/RK3576.
- **Status:** complete on RK3588 and RK3576 for 8-bit NV12.

### Phase 15: AV1 DMA-BUF Frame Output
- [x] Export the VDPU34x post-processed display lease as AV1 NV12.
- [x] Move VDPU383 AV1 output to a reference-counted buffer pool.
- [x] Prevent VDPU383 slot reuse while an SDK frame still owns the surface.
- [x] Skip CPU planar readback when VDPU383 DMA output is requested.
- [x] Preserve default CPU output and AV1 10-bit behavior.
- [x] Verify RK3588 against the C oracle and RK3576 VDPU383 DMA against CPU output.
- [x] Re-run the RK3576 full 8-bit and 10-bit VDPU383 reference sequences.
- [x] Re-run package, workspace and DMA-BUF gates on RK3588/RK3576.
- **Status:** complete on RK3588 and RK3576 for 8-bit NV12.

### Phase 16: Encoder DMA-BUF Input API V1.12
- [x] Add Rust `DmaBufInput` with fd import and NV12 stride validation.
- [x] Add `rk_mpp_encoder_submit_frame_dmabuf()` and raw FFI declaration.
- [x] Wire VEPU580 and VEPU510 H.264/H.265 jobs to an external input fd.
- [x] Wire VEPU2 and VPU720 JPEG jobs to an external input fd.
- [x] Preserve all copied-input APIs and outputs.
- [x] Verify copied and DMA input are byte-identical on RK3588/RK3576.
- [x] Re-run package, workspace and ABI gates on RK3588/RK3576.
- **Status:** complete on RK3588 and RK3576 for NV12 H.264/H.265/JPEG.

### Phase 17: W8-2 Decode-to-Model-Input Direct Scaling
- [x] Add strided NV12 bilinear resize and BT.601 RGB conversion to `Scaler`.
- [x] Add `Frame::letterbox_nv12()` for CPU and DMA-BUF SDK frames.
- [x] Avoid full-frame NV12 or RGB intermediate copies.
- [x] Verify H.264 CPU and DMA frame output is byte-identical on RK3588/RK3576.
- [x] Benchmark direct scaling against the full-frame conversion path.
- [x] Re-run workspace gates after adding NV12 scale tests.
- **Status:** complete on RK3588 and RK3576; 1201/0/38 workspace gate.

### Phase 18: Official MPP Drop-in Lifecycle Foundation
- [x] Export `mpp_create`, `mpp_init`, `mpp_destroy`, support and show-format.
- [x] Match the official 184-byte `MppApi` function-table layout.
- [x] Compile a C consumer against the vendor `rk_mpi.h` and run on RK3588/RK3576.
- [x] Add deterministic null/value/init/reset/control lifecycle checks.
- [x] Implement MppFrame/MppPacket/MppBuffer ownership and H.264/H.265/MJPEG
      CPU NV12 decode put/get data flow.
- [x] Link and run official `mpi_dec_test` unchanged.
- **Status:** complete; the broader object/meta surface continues in later
      compatibility phases.

### Phase 19: Official `mpi_dec_test` Compatibility
- [x] Interpose public MPP symbols into unchanged vendor `mpi_dec_test`.
- [x] Preserve advanced MJPEG `KEY_OUTPUT_FRAME` through packet `MppMeta`.
- [x] Validate H.264/H.265/MJPEG against native outputs on RK3588 and RK3576.
- [x] Run the unchanged RK3588 `mpi_dec_test` over VP8 IP/altref, MPEG-2,
      MPEG-4 IP/B and H.263 CIF/QCIF, with exact output-size and byte checks.
- [x] Replace whole-prefix legacy replay with incremental parser state and
      benchmark MPEG-2/MPEG-4/H.263 on 100/96-frame streams.
- [x] Remove the legacy object-path I420→NV12 intermediate allocation and
      write directly into a normal CPU buffer with one UV interleave pass.
- [x] Remove the remaining dependency on vendor log/cfg/OSAL helper symbols.
- [x] Pass unchanged `mpi_enc_test`.
- **Status:** complete with the standalone library on RK3588.

### Phase 20: Official `mpi_enc_test` Object Bridge
- [x] Parse vendor `MppEncCfg` into the stable encoder configuration.
- [x] Bridge opaque `MppFrame` input and owned `MppPacket` output.
- [x] Validate RK3588 H.265/MJPEG byte identity and H.264 one-frame reconstruction.
- [x] Implement the default IDR-only CPB reference schedule and slice amendment
      for exact RK3588 H.264 six-frame VCL/reconstruction.
- [x] Close RK3576 H.264/H.265 parameter gaps.
- [x] Remove vendor helper dependency and pass standalone `mpi_enc_test`.
- **Status:** RK3588 was revalidated on 2026-11-21; the earlier RK3576 matrix
      remains recorded in CAPABILITY, while current RK3576 SSH was unavailable.

### Phase 21: Public H.264 Syntax and Deblock ABI
- Goal: expose the already board-validated typed H.264 syntax/deblock controls
  through the stable C SDK without changing V1, V1.9 or V1.10 layouts.
- [x] Add H.264 deblock and syntax descriptors plus V1.13 advanced config V3.
- [x] Map V3 descriptors into `EncoderConfig` with codec/range validation.
- [x] Add C, Rust FFI, sys-crate and ABI snapshot coverage.
- [x] Verify the C V3 output against the validated CLI path on RK3588.
- [x] Update README, CAPABILITY, HANDOFF, CHANGELOG and progress evidence.
- **Status:** complete on RK3588; full host workspace gate green.

### Phase 22: Public H.265 Syntax ABI
- Goal: expose the existing typed H.265 SAO/deblock/loop-filter/tile controls
  through V3 without changing its size or any V1.13 field offset.
- [x] Replace the V3 trailing reserve with a 32-byte H.265 syntax descriptor.
- [x] Map field-selective H.265 syntax values into `EncoderConfig`.
- [x] Add C/system ABI coverage and the 41st ABI export.
- [x] Verify SAO, deblock and loop-filter cells against the CLI on RK3588.
- [x] Update API version, packaging default and all evidence docs.
- **Status:** complete on RK3588; final host workspace and SDK package gates run.

### Phase 23: H.265 Auto-Tile Hardware Closure
- Goal: prove the V1.14 auto-tile bit reaches the real VEPU580 tile path and
  remains byte-identical to the CLI, rather than relying on descriptor mapping
  tests alone.
- [x] Generate a one-frame 4608x64 NV12 wide fixture.
- [x] Add C harness parsing for `H265_AUTO_TILE`.
- [x] Compare V3 auto-tile against `h265-gop ... tile:auto` on RK3588.
- [x] Record packet/byte evidence and update docs.
- **Status:** complete on RK3588; 8,876 bytes / 2 packets byte-identical.

### Phase 24: Bounded-Memory H.264 Stream CLI
- Goal: remove the whole-file read from `h264_stream` while preserving its
  bounded display reorder and output bytes.
- [x] Reuse the SDK's incremental H.264 access-unit scanner for file chunks.
- [x] Add a public SDK reorder-depth accessor for status output.
- [x] Verify planar and NV12 hashes against the current whole-file CLI.
- [x] Measure wall time and peak RSS on the 50 MiB / 742-frame network stream.
- [x] Update docs and commit only if output remains byte-identical.
- **Status:** complete; RSS 55.9 -> 17.0 MiB with flat warm wall time.

### Phase 25: Bounded-Memory Main `h264dec`
- Goal: remove the whole-file read from the primary H.264 CLI while retaining
  its depth-3 submission ring, output recycling and byte identity.
- [x] Drive `h264dec` from 4 MiB chunks and the shared access-unit scanner.
- [x] Preserve the current display reorder and output hashes.
- [x] Measure the 50 MiB / 742-frame wall time, RSS and output count.
- [x] Update docs and commit.
- **Status:** complete; 742/742, flat wall time, RSS 59.5 -> 20.4 MiB.

### Phase 26: FFmpeg Compatibility and Asynchronous Encoding
- Goal: make the Rust MPP paths competitive with vendor FFmpeg and remove the
  synchronous submit/poll gap that remains in VEPU580 encoding.
- [x] Add FFmpeg decode/encode info-change, buffer-group, packet-meta and EOS
  compatibility.
- [x] Add `--mpp-compat` SDK packaging for rebuilding FFmpeg against Rust MPP.
- [x] Benchmark vendor FFmpeg, FFmpeg+rkmpp-rs and native rkmpp-rs.
- [x] Remove imported-input CPU staging and decoded-frame packing copies.
- [x] Honor uncached MPP DRM buffer groups and preserve session controls across
  FFmpeg's second encoder config update.
- [x] Require H.264 FFmpeg output to match vendor C byte-for-byte and direct
  DMA-BUF encode output to match the CPU fallback.
- [x] Match H.265 VPS/SPS/PPS, temporal MVP, GOP-boundary IDR and long-GOP output
  to vendor C for 60-frame 320x240/1280x720 single-GOP sequences. A second GOP
  exposes nondeterministic C output and is recorded as a C-oracle limitation.
- [x] Match H.264's FFmpeg motion-info and deferred scene-tune state so long
  moving sequences do not switch lambda/ATF rows early.
- [x] Re-run byte-exact standalone, FFmpeg and workspace gates.
- [x] Split VEPU580 H.264 submit and completion into a deferred job.
- [x] Extend the same queue contract to VEPU580 H.265 and model C's
  frame-parallel `reg0193_dual_core` handshake. Depth 2 is byte-exact at
  320x240/1280x720 and is now the default.
- [x] Feed deferred encoder completions through the FFmpeg C compatibility API.
- [x] Benchmark depth 1/2 and retain only byte-identical outputs. Depth 2 is
  exact and now the default FFmpeg fixed-QP H.264 path.
- [x] Defer imported fd-only MPP buffer mapping until pointer, info, read/write
  or CPU fallback actually needs it. This removes one `mmap/munmap` pair per
  FFmpeg DMA-BUF frame and brings Rust instructions to C parity.
- [x] Feed native file encoders through the VEPU580 deferred queue. H.265
  depth 2 is exact and default; H.264 depth 2 is exposed as a high-throughput
  opt-in because async scene-tune feedback differs from the blocking stream.
- [x] Measure 1/2/4/6/8-way encoder scaling. Aggregate H.264/H.265 throughput
  plateaus around four streams, identifying VEPU580 as the next hard limit.
- [x] Preserve the reference object-path level/TMVP defaults: FFmpeg's explicit
  `h265:level=0` selects the size-derived level and temporal MVP, while an
  unchanged `mpi_enc_test` keeps level 4.0 and TMVP off. Revalidate the
  standalone encoder NAL/reconstruction and FFmpeg exact gates.
- [x] Support `0x47` direct-context and `0x20` 24-byte `MppTrieInfo v0`
      `mpp_opt_setup` / `mpp_trie_add_info` ABIs while keeping the unchanged
      encoder object gate green.
- [x] Export the upstream `kmpp_obj_*` surface and implement the userspace
      object core needed by `mpp-develop/build/test/mpi_enc_test` to load,
      register definitions and parse encoder arguments.
- [x] Match the new upstream H.264 default stream byte-for-byte.
- [x] Match the new upstream H.265 fixed-QP stream and register trace.
- [x] Match new upstream H.264/H.265 60-frame single-GOP streams.
- [x] Carry the H.265 motion histograms through the 23-word aggregate STATUS.
- [x] Use the H.265 896/640/384 CTU gate and complete the row-3 WGT/RDO overlay.
- [x] Match a 320x240 moving 300-frame/five-GOP stream and add the board gate.
- [x] Add an interleaved object-path benchmark with independent wall and CPU
  limits; Rust is faster with lower user+sys at 320x240 and 720p.
- [x] Map compatible `tune:scene_mode` and implement H.265 IPC row 0/1.
- [x] Add the IPC trace gate; 160 jobs have exact WGT/RDO/OSD sections.
- [x] Validate the shared H.264 IPC scene mode at 60 frames / 64 NALs.
- [x] Bridge new upstream OSD, ROI and user-data metadata.
- [x] Implement the userspace KMPP buffer object.
- [ ] Finish kernel KMPP buffer-group/ioctl compatibility.
- [x] Correct NULL-frame EOS and empty-packet `MPP_NOK` semantics needed by the
  FFmpeg encoder loop.
- [x] Reject the high-level cross-thread compatibility worker; it lost delayed
  H.264/H.265 packets non-deterministically.
- [x] Implement the deferred queue below the encoder session so packet
  generation and input ownership are explicit per job.
- **Status:** in_progress

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
