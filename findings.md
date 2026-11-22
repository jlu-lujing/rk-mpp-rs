# Findings & Decisions

## Requirements

- Priority codecs: H.264 decode and MJPEG decode.
- Public API must support live packet input, persistent sessions, frame output,
  output metadata, EOS/flush/reset, and errors.
- Keep the current `rk_jpeg_open/frame/close` ABI for `rknn-fastinfer`.
- Add a versioned C header and verify actual C and C++ consumers on RK boards.
- Extend the public contract to all implemented codecs/encoders after the
  priority H.264/MJPEG path is stable.
- Turn V1.8 into a product-ready SDK: Linux SONAME, install layout,
  pkg-config, CMake, ABI snapshot and standalone examples.

## Packaging Findings

- Both validation boards provide `pkg-config`, CMake and `readelf`.
- RK3576 has pkg-config 1.8.1 / CMake 3.25.1 / binutils 2.40.
- RK3588 has pkg-config 0.29.2 / CMake 3.18.4 / binutils 2.35.2.
- The repository currently has no LICENSE, NOTICE, CHANGELOG or build script.
- Cargo already exports `cdylib`, `staticlib` and `rlib`, but the shared object
  has no embedded SONAME and there is no install/package metadata.
- `cargo:rustc-cdylib-link-arg` successfully applies a Linux-only SONAME
  without affecting rlib/staticlib builds.
- pkg-config must use `${pcfiledir}/../..` rather than a substituted absolute
  prefix to remain relocatable. CMake imports can derive the prefix from
  `CMAKE_CURRENT_LIST_DIR`.
- The current C encoder config is 68 bytes. Future fields must be appended and
  `struct_size` must be treated as a prefix length; rejecting old sizes would
  break binary compatibility within ABI major 1.
- Rate control should not enlarge `rk_mpp_encoder_config_t`. A separate
  `rk_mpp_encoder_advanced_config_t` carrying an independently versioned
  `rk_mpp_encoder_rate_control_t` preserves every V1.8 offset and lets future
  ROI/GDR/slice fields append without changing existing storage requirements.
- The VEPU580 H.264 and H.265 sessions already accept CBR/VBR/AVBR targets,
  bitrate bounds and QP bounds. The stable ABI work is a mapping and validation
  layer, not a new hardware implementation.
- The RK3576 VEPU510 streaming wrappers currently expose only their captured
  fixed-QP paths. Non-fixed-QP configuration must be rejected explicitly until
  the per-picture VEPU510 RC session is wired and board-validated.
- The V1.9 advanced ABI passed byte-for-byte against the validated CLI for RK3588
  VEPU580 H.264/H.265 in all three CBR/VBR/AVBR modes. Sizes were H.264
  CBR/VBR/AVBR `10848/12191/10476` bytes and H.265
  `11049/11335/9249` bytes.
- A minimal V1.8 header snapshot compiles an encoder consumer directly against
  the V1.9 shared library. This checks the real old-layout call path instead of
  relying only on static size assertions.
- ROI/GDR/slice need a new advanced-config v2 rather than enlarging the V1.9
  advanced struct: the existing `rk_mpp_encoder_advanced_config_init()` writes
  104 bytes, so an old caller would overflow if that initializer started writing
  appended fields.
- V1.10 C layouts are 40 bytes per ROI, 16 bytes for GDR, 24 bytes for slice
  split and 472 bytes for the total V2 advanced config. ROI storage is embedded
  for up to eight regions, so no caller pointer has to outlive create.
- RK3588 control byte sizes matched the CLI exactly: H.264
  slice/ROI/GDR `21148/26266/23445`; H.265 `16391/16022/17104`.
- The sys crate will use checked-in hand-written declarations rather than
  bindgen: the public header is already fixed-width C, CI is offline, and exact
  enum/function coverage can be asserted without a libclang dependency.
- `rk-mpp-rs-sys` declarations exactly match the current ABI snapshot (35
  symbols in V1.11). The final workspace gate is 1199 tests: 1195 from the root
  SDK and four sys-crate layout/native tests. Static release linking requires
  `LTO=false` with the current Rust staticlib archive.
- The stable decoder `Frame` copies every hardware surface into a `Vec<u8>` by
  default, including MJPEG. DMA-BUF output is therefore opt-in: MJPEG NV12 can
  retain its existing DmaBuf, while every default/CPU-backed path keeps its
  previous behavior. Cloning a DMA-backed frame deliberately falls back to a
  CPU byte copy rather than silently sharing a mutable surface.
- With `RK_MPP_DECODER_FLAG_DMABUF_OUTPUT`, V1.11 MJPEG NV12 keeps the JPEG
  output DmaBuf in `Frame`; Y and UV expose the same duplicated fd with distinct
  offsets and the hardware row stride. The exported fd is caller-owned and
  closing it does not shorten the frame's lifetime.
- H.264's DPB surface is already a `BufferLease<DmaBuf>`. DMA output therefore
  clones that lease into each reordered `Picture`; the extra reference prevents
  the V4L2 surface from returning to the pool before the SDK emits it.
- H.265 uses the same pooled `video` lease. Its reordered `Picture` now owns a
  clone, while the decoder may release its slot or reset the DPB without
  invalidating an already-decoded display frame. The raw vdpu34x/vdpu383
  layout is NV12 only for 8-bit 4:2:0; Main10 and 422/444 stay on the existing
  CPU planar conversion path.
- VP9 also exposes a pooled video lease. Both ordinary displayed frames and
  `show_existing_frame` clone it, so reference-map updates can release the
  decoder's frame slot without invalidating an SDK frame that has not yet been
  consumed. VP9 10-bit remains on CPU planar output.
- AV1 has two different output lifetimes. VDPU34x already produces the final
  post-processed `display` as a pooled lease, while VDPU383 originally owned a
  raw `DmaBuf` per frame. VDPU383 output now comes from the same buffer-pool
  abstraction; `output_refs() > 1` excludes a delayed SDK frame from slot
  reuse, and DMA mode skips the normal planar CPU readback.
- Encoder fd input reuses the hardware address fields that already point at the
  encoder's staging surface. H.264 VEPU580 passes the fd into its job builder;
  H.265 and both VEPU510 paths use a one-shot override consumed by the next
  picture. JPEG already had fd-capable VEPU2/VPU720 implementations, so the SDK
  only needed stride validation and enum dispatch.
- Scaling H.264/H.265 decoder NV12 through a materialised RGB frame is the
  dominant avoidable copy on large inputs. Sampling the strided Y/UV planes
  directly in the output-pixel loop avoids both full-frame copies. On the
  1080p→640 path this reduced RK3588/RK3576 work from 20.32/24.36 ms to 4.93/5.95 ms,
  while the real 720p CPU/DMA decode output remained byte-identical after
  scaling.
- The official `MppApi` table is 184 bytes on aarch64: two 32-bit header words,
  fourteen function pointers and sixteen reserved 32-bit words. Lifecycle can
  be implemented independently; unsupported encode/ISP/task entries remain
  explicit failures while object-backed decode entries are wired.
- The basic vendor decode loop uses opaque packet/frame objects rather than
  exposing the SDK's Rust frame type. Packing decoded NV12 into
  `hor_stride = align16(width)` and `ver_stride = align16(height)` preserves
  byte identity with `dump_mpp_frame_to_file`; H.264, H.265 and MJPEG all pass
  the same official-header C consumer on RK3588 and RK3576.
- Replacing all of `librockchip_mpp` directly is not the first useful W7-3
  gate: the vendor test utilities also call internal log, cfg, meta and OSAL
  symbols. `LD_PRELOAD` cleanly interposes the public MPP entry points while
  retaining those helpers, and proves the unchanged `mpi_dec_test` control
  loop itself. The advanced MJPEG path passes the preallocated output frame via
  `KEY_OUTPUT_FRAME`, so the returned `MppFrame` identity must be preserved.
- Encoder compatibility can reuse the stable `encoder::Encoder` backend without
  exposing its Rust types. The vendor test's `MppEncCfg` is opaque, so the
  minimal bridge reads keys through dynamically resolved
  `mpp_enc_cfg_get_s32`; frames are packed from `MppBuffer`, and output packets
  own their copied stream bytes. The old encoder starts with `idr_pic_id=0`;
  the synchronous header query must not consume an extra IDR id.
- The default vendor encoder CPB does not simply reference the previous P
  picture: after the opening IDR it keeps referencing slot zero and emits
  `long_term_reference_flag=1` plus `ref_pic_list_modification` entries for
  later P pictures. VEPU580's register file can use the same reference address,
  but those header bits are applied by MPP's software stream amendment layer.
  Replaying that amendment yields exact six-frame VCL output and reconstruction.

## Research Findings

- The Rust `EncoderConfig` already exposes `H264DeblockConfig` and
  `H264SyntaxConfig`, including CABAC initialization, transform-8x8,
  constraint-set force/value and base-layer priority ID. The stable C SDK only
  exposes rate control, ROI, GDR and slice split, so C consumers cannot reach
  controls whose hardware paths are already byte-validated by the CLI tests.
- H.264 defaults are profile-derived: VEPU580 starts CABAC at Main and
  transform-8x8 at High. A C syntax descriptor therefore needs field-selection
  flags rather than blindly overriding both booleans; otherwise setting only
  `base_layer_pid` would silently change entropy coding or transform defaults.
- The V1.13 ABI adds four public symbols, not two:
  `rk_mpp_encoder_h264_deblock_init`, `rk_mpp_encoder_h264_syntax_init`,
  `rk_mpp_encoder_advanced_config_v3_init` and
  `rk_mpp_encoder_create_advanced_v3`. The frozen V1 snapshot therefore grows
  from 36 to 40 exports.
- The internal C variadic logger previously leaked three `rk_mpp_log_*`
  accessors into the public symbol table even though they are not in the public
  header. They are now named with the non-ABI `rk_mpprs_internal_*` prefix.
- RK3588 V1.13 C V3 output is byte-identical to the CLI for CABAC idc 2
  (18,046 bytes), constraint `0x003f0025` (17,977), base PID 5 (17,977) and
  deblock `1:-2:3` (18,098).
- V1.13's V3 advanced config ends with 32 reserved bytes. Replacing that reserve
  with a same-size H.265 syntax descriptor keeps the total 552-byte layout and
  every earlier field offset unchanged. H.265 booleans can be encoded as
  selected-field flags plus a value bitmask, so defaults such as
  `loop_filter_across_tiles=true` remain intact when a caller changes only one
  unrelated control.
- V1.14 RK3588 C V3 output matches the CLI for H.265 SAO disable (15,618
  bytes), deblock beta/tc 3/-2 (15,878) and loop-filter across slices with
  tiles disabled (15,829). The V3 size remains 552 and the ABI grows from 40
  to 41 exports.
- The V1.14 `auto_tile` bit is mapped into `H265SyntaxOptions`, but a 320x240
  stream cannot distinguish the VEPU580 auto-tile branch. The existing
  validated wide case is 4608x64, where the CLI switches to two horizontal
  tiles, so the C V3 gate needs that shape rather than another small fixture.
- V1.14 RK3588 auto-tile now passes on the 4608x64 one-frame shape: C V3 and
  CLI both emit 8,876 bytes and two packets.
- `h264_stream` still used `std::fs::read()` and `split_access_units()` even
  though the SDK already has an incremental Annex-B/AU scanner. On the 50 MiB
  network stream the current process peaks at about 55.8 MiB RSS and takes
  about 1.57 s for 742 frames. Reusing the chunk scanner can remove the
  file-sized allocation without changing decoder/reorder semantics.
- The bounded scanner initially regressed 50 MiB wall time until the chunk
  size was raised to 4 MiB. The final three interleaved warm pairs are
  1.576 s / 55.9 MiB (whole file) versus 1.595 s / 17.0 MiB (bounded). The
  first bounded run is cold at 2.226 s, so the warm comparison is recorded
  separately.
- The main `h264dec` path has the same bounded-input result while retaining
  its depth-3 ring: three interleaved pairs are 0.665 s / 59.5 MiB (whole
  file) versus 0.665 s / 20.4 MiB (bounded), with 742/742 output and unchanged
  720p planar/NV12 hashes.

- `src/lib.rs` currently exports only `rk_jpeg_*`; it is path-based, single-image,
  RGB-letterbox oriented, and does not expose H.264.
- `codec::h264::Decoder` already provides `new`, `decode_access_unit`,
  `set_output_nv12`, `recover_session`, and a bounded `DisplayReorder`.
- `codec::jpeg` parses in-memory bytes, and `hal::jpegd::Job` builds hardware
  decode jobs for RGB888 or NV12. The missing layer is session/stream framing
  and a stable public API.
- `Cargo.toml` currently builds `rlib` and `staticlib`, not `cdylib`.
- No versioned public C header exists outside the reference tree.
- H.264 stream framing originally started a new AU at the next VCL NAL. That
  dropped SPS/PPS inserted after the previous picture. The splitter now starts
  the next AU at the first non-VCL NAL after the prior picture.
- C and C++ consumers pass against both `librk_mpp_rs.so` and
  `librk_mpp_rs.a`; all `rk_mpp_*` and legacy `rk_jpeg_*` symbols are exported.
- A whole H.264 stream or several concatenated JPEGs may be submitted in one
  packet, while byte-at-a-time fragmentation also remains supported.
- `codec::hevc::Decoder` accepts one complete access unit at a time and already
  returns POC, display epoch, chroma format and frame errors. Its CLI currently
  buffers the whole file and sorts afterward, so the SDK needs a bounded
  streaming reorder queue.
- HEVC `split_access_units` has the same parameter-set carryover issue as the
  old H.264 splitter: VPS/SPS/PPS after one picture must start the next AU.
- HEVC SPS currently discards `sps_max_num_reorder_pics`; the SDK needs that
  value to bound live playback latency instead of waiting until EOS.
- H.265 10-bit output is already planar little-endian 16-bit. Add an explicit
  `I420P10` pixel format rather than silently treating it as 8-bit I420.
- The H.265/HEVC C consumer passes on RK3576 and RK3588 for 8-bit NV12/I420 and
  Main10 I420P10. The Main10 fixture has six frames with B-pyramid reordering,
  so SPS-bounded display ordering is exercised rather than only IDR output.
- VP9 already emits displayed pictures directly from `decode_packet`, including
  `show_existing_frame` and hidden-alt-ref superframes, so the SDK does not add
  another reorder queue. VP9 byte-stream packet boundaries are normative; IVF
  demux remains a caller/container responsibility.
- The VP9 C gate passes on RK3576 and RK3588: profile 0 12-frame NV12/I420 and profile
  2 10-bit 12-frame I420P10 outputs are byte-identical to C MPP. The test feeds
  each IVF frame as one packet and exercises decoder reset/reuse.
- `codec::av1::Decoder::decode_packet` already accepts one temporal unit and
  returns zero to many display-ready pictures. It carries sequence state,
  backward CDFs, eight reference slots and `show_existing_frame` internally.
- AV1 IVF uses the `AV01` FourCC and MPP decoder type `16777224`. The 320x240
  10-bit fixture contains 10 IVF packets; the 8-bit inter-box fixture contains
  two.
- RK3576 AV1 requires a separate VDPU383 hardware path. `decoder383.rs` shares
  the AV1 packet parser while maintaining its own Session, texture slots,
  reference CDF state and planar readback.
- Verified AV1 8-bit key+P SHA-256 is `7f4a0c9e…4020f`. On RK3576 VDPU383
  10-bit output matches C at `f3b69b61…49b8`; on RK3588 grain-applied output is
  `b75e6172…904d`, matching the Rust/libaom result while C retains its known
  missing-film-grain bug.
- `hal::h264e::Vepu580H264::encode_frame` is already synchronous: it copies the
  NV12 input, submits/polls the hardware and returns complete `EncodedFrame`
  values with packet boundaries and sequence metadata. A public encoder can
  therefore queue fully-owned output without inventing asynchronous ownership.
- RK3576's captured VEPU510 H.264 path is fixed at 320x240, Baseline QP26,
  one IDR plus P pictures and 30 fps. The public API exposes only that validated
  path instead of pretending the VEPU580 configuration surface transfers.
- The public H.264 encoder's 6-frame output matches the VEPU580 CLI on RK3588
  (SHA-256 `aa6276ba…173d1`) and `h264e510-gop6` on RK3576
  (`0e65b51a…81f33`). NV12 and converted I420 inputs match each other on each
  board; the two SoCs use different hardware bitstreams by design.
- VEPU580 H.265 already exposes a frame-level fixed-QP session, so the public
  encoder can queue one parsed packet per picture. RK3576's captured VEPU510
  path remains limited to 320x240 QP26 GOP6 but now emits one picture per
  submit through a small streaming wrapper.
- Verified H.265 hashes are `6c285115…979e9` on RK3588 VEPU580 and
  `8088e821…7fe397` on RK3576 VEPU510; NV12 and I420 match on each platform.
- The existing `jpege720::JpegEncoder` already selects VEPU2 or VPU720 and
  returns a packet-range view, so JPEG needed no new hardware wrapper. The
  public layer pads visible NV12/I420 input to the JPEG stride rules and queues
  the returned packet ranges.
- Verified JPEG hashes are `4f571054…7db1e` on RK3588 VEPU2 and
  `64153672…7ac69` on RK3576 VPU720; NV12 and I420 match on each platform.
- VP8's decoder already returns display-ready `Picture` values per IVF packet,
  including hidden-alt-ref handling. Unlike VP9/AV1 it has no register status
  accessor, so the shared decoder status call must return unsupported.
- VP8 API hashes on RK3588 are `1f07acf5…4c9c2` (NV12) and
  `0a6bbff3…326096` (I420). RK3576 C MPP reports "unable to create dec vp8
  ... unsupported"; the Rust handle also rejects create, so RK3576 is a hardware
  SKIP rather than a masked failure.
- The legacy MPEG-2/MPEG-4/H.263 front ends only expose whole-stream decode, so
  the shared API buffers arbitrary chunks and emits all display frames at
  EOS/flush. This is an explicit latency boundary, not packet-level decode.
- Verified legacy NV12 hashes on RK3588 are MPEG-2 `2dc97045…6371e`, MPEG-4
  `33cde882…64580`, H.263 `4a2a9312…c2f45e`; I420 equivalents also match.
- `mpi_dec_test` reads non-IVF elementary streams in 4 KiB packets and expects
  pictures before EOS. The legacy Rust SDK retains whole streams, which made
  the official tool rewind forever with no output. The object compatibility
  path now keeps parser state and consumes complete picture groups/VOPs
  incrementally; the public whole-stream SDK behavior is unchanged.
- MPEG-4 needs VOL, time-base and DivX custom-version state across packets;
  H.263 needs its `found_i_vop` gate across packets. Replaying the whole prefix
  was correct but O(n²). Incremental parser state preserves the same frame
  bytes without re-decoding earlier pictures.
- A 100-frame MPEG-2 stream takes 0.14–0.15 s with incremental parsing versus
  2.62–2.73 s with replay. MPEG-4 (96 frames) improves from 2.66–2.71 s to
  0.12–0.14 s, and H.263 (96 frames) from 8.45–8.46 s to 0.15–0.16 s. Vendor C
  is 0.10–0.14 s under the same DMC-performance/CPU-4–7 relative setup.
- The remaining object-layer copy was larger than the parser. `Frame::from_legacy`
  converted I420 to a new NV12 `Vec`, then `frame_from_sdk` copied that into a
  fresh DMA-heap buffer. Keeping the legacy frame planar and writing directly
  into a normal CPU `MppBuffer` with one UV interleave pass removes both the
  intermediate allocation and per-frame mmap/madvise/munmap work.
- After that change, `perf stat` for 100-frame MPEG-2 is 50.1 ms task-clock and
  75.4M instructions versus vendor C 66.5 ms and 80.0M. A new perf record has
  no Rust user-space hotspot; the top samples are libc file writes and process
  startup. The 0.02–0.04 s wall difference is therefore I/O/startup/sync noise,
  not remaining Rust decode arithmetic.
- `mpi_enc_test` has two option-trie ABIs in the local oracles. The 2024-04
  binary calls four-argument `mpp_opt_setup(..., 0x47, info_count)` and
  two-argument `mpp_trie_add_info(info)`; `mpp_opt_parse` expects the returned
  pointer to be the original `MppOptInfo *`. The installed `0x20` binary calls
  three-argument add with a name string and context, then loads `ctx` from
  offset 8 of a 24-byte `MppTrieInfo v0`
  `{name, ctx, index, str_len}`. Encoding the tag explicitly fixes both
  without pointer heuristics.
- `/usr/local/bin/mpi_enc_test` has no `kmpp_mode` string or option. The actual
  KMPP oracle is `mpp-develop/build/test/mpi_enc_test`. Its userspace object
  path now runs on the Rust library: dual-ABI objdef lookup/register, trie
  VLA/leaf storage, object allocation, field get/set, flags and callbacks are
  implemented. `-kmpp_mode 0` H.264 is byte-identical to the new vendor stream
  (`e18fe707…14bdbd`, 3 NAL). H.265 is byte-identical over four NALs after
  carrying the new full-range/VUI/CU-QP-delta and AQ/ROI/MD/QP-bias defaults
  (`119bbf79…caa76d5`); the register trace differs only in dma-buf addresses.
  A 60-frame GOP is also exact: H.264 62 NAL (`6a8cb407…3e3544`) and H.265
  63 NAL (`7eed6267…138b42`). OSD, ROI and `user_data_enable` use the existing
  MppMeta path; two-frame user-data output is H.264 10 NAL and H.265 11 NAL.
  The userspace KMPP buffer object now allocates, sets up, exports a
  `KmppShmPtr`, and releases its memory. Kernel buffer-group ioctl remains
  unavailable on the current platform.
  The RK3588 kernel exposes neither
  `/dev/vcodec` nor `/proc/kmpp` ctrl-cfg support, so the tool rejects KMPP
  modes 1/2 before encoding.
- The RK3588 standalone decoder matrix is now 15/15 with exact output-size
  checks: H.264, H.265, VP8 IP/altref, VP9, MPEG-2, MPEG-4 IP/IPBB,
  H.263 CIF/QCIF, AV1, MJPEG and two AVS+ vproc cycles all match vendor C
  byte-for-byte; the extra MPEG-2 case omits `-n` and exercises the EOS path.
  AVS uses coding type `16777221`; its two cycles are 9 frames
  (`6ef29ca6…e169`) and 7 frames (`35ac81e6…f3d1`).
- JPEG restart interval 20 matches the CLI output at RK3576
  `0ccf41ea…82997` and RK3588 `25254033…df748a`.
- Verified VP9 SHA-256 values are
  `c5381bbf…3949a` (profile 0 NV12),
  `53bc3dd4…0abea69` (profile 0 I420), and
  `1c4c63f9…9a09` (profile 2 I420P10).

## FFmpeg and Encoder Scheduling

- RK3588 FFmpeg `c5e745e3c1` now runs unchanged with `librk_mpp_rs.so` as
  `librockchip_mpp.so.1`. H.264/H.265/VP9 NV12 decode is byte-identical, and
  H.264/H.265/MJPEG encode completes.
- Before direct-buffer work, FFmpeg+rkmpp-rs decode used 1,512 `mmap`, 1,296
  `munmap` and 211,580 page faults for 600 H.264 frames. The vendor path used
  260/57/7,993.
- `frame_from_sdk()` was packing every decoded frame into a new `Vec`,
  allocating a dma-heap buffer, mapping it and copying again. Transferring the
  canonical decoded DMA-BUF instead reduced H.264 decode from 0.901 s to
  0.513 s and H.265 from 0.927 s to 0.517 s.
- FFmpeg encode imports DRM input through `mpp_buffer_import()`. The old
  `frame_input_view()` only inspected `buffer.dma`, so imported buffers looked
  CPU-only and were staged through a full NV12 copy. Using `buffer.fd` reduced
  H.264 encode 0.806 -> 0.711 s and H.265 0.838 -> 0.738 s.
- The direct paths preserve MJPEG CPU fallback. `accepts_dmabuf_frame()` now
  restricts direct encode input to VEPU580/VEPU510 H.264/H.265; allowing MJPEG
  changed its bitstream.
- Perf after the copy fixes shows FFmpeg+rkmpp-rs encode retired instructions
  near vendor C (111M versus 108M), but CPU utilization is only about 50-60%.
  The remaining wall-time gap is waiting, not memory traffic.
- Imported MPP DMA-BUF objects were eagerly mmap'ed in
  `new_external_buffer`, even though the FFmpeg hardware path needs only the
  fd. This cost one mapping per input frame. Mapping is now lazy: pointer,
  info, read/write and CPU fallback establish it on demand. H.265 `mmap` calls
  dropped from 544 to 244 over 300 frames. Rust retired instructions fell from
  390.0M to 327.4M, effectively equal to C's 327.3M; task-clock fell from
  428.0 to 362.9 ms. The C data-path object checks still return a non-null
  pointer and byte-exact read/write results.
- FFmpeg's RKMPP hwcontext allocates its internal DRM frames from an MPP group
  with `MPP_BUFFER_FLAGS_DMA32` and no `MPP_BUFFER_FLAGS_CACHABLE`. The
  compatibility allocator was still using the cached `system` dma-heap. The
  CPU-visible second frame matched the source, but VEPU580 read stale cache
  lines, so FFmpeg+rkmpp-rs produced a correct I-frame and a divergent P-frame.
  Selecting `system-uncached` for non-cacheable MPP groups restores exact
  input visibility.
- With uncached MPP DRM buffers, H.264 and H.265 are byte-identical to vendor C
  for the 2/12-frame gates and 60-frame single-GOP sequences at both 320x240 and
  1280x720.
- FFmpeg sends `MPP_ENC_SET_CFG` once during init and again on the first frame
  to add DRM-plane strides. Reconstructing `Encoder` on the second command
  discarded the session's previously selected SEI and header modes. Preserving
  those controls across config replacement removes the extra SEI NALs and makes
  the H.264 output match vendor C exactly.
- H.265's object path needs the configured level in VPS/SPS, slice temporal MVP,
  `colFromL0Flag`, `col_ref_idx`, and colmv load/store registers. It also must
  begin a new IDR when the configured GOP boundary is reached; the short
  pre-boundary tests did not expose this.
- FFmpeg does not attach `KEY_MOTION_INFO`. Vendor C therefore leaves H.264
  `enc_pic.mei_stor` and `meiw_addr` clear. Matching that and delaying H.264
  scene-tune feedback by one picture reproduces the async encoder's state at
  frame 105, where a 120-frame fixture otherwise changes lambda/ATF rows early.
- A second GOP makes C FFmpeg an unstable stream oracle on this RK3588.
  Immediately after a cold boot, eight consecutive H.264 120-frame/GOP60 runs
  produced four hashes: `0c9e7106…`, `d7aba365…`, `cc460bf5…`, and
  `38c89798…`. Three separately decoded samples produced three different YUVs.
  Eight 60-frame H.264 runs all produced `77ec189d…`; eight 60-frame H.265 runs
  all produced `6e634163…`. Warmups do not make the 120-frame case
  deterministic. Rust is deterministic and matches the valid 60-frame C mode,
  so exactness is defined over one complete GOP.
- VEPU580 H.264 depth-2 needs three reconstruction surfaces in the vendor
  `0→1→2→0` rotation. The prior synchronous two-slot alternation stayed
  byte-exact only because each job had completed before the next was prepared;
  with two pending jobs it overwrote a surface still used as a reference.
- A blocking `POLL_HW_FINISH` is required when more than one encoder job is
  queued. The non-blocking poll can return after one task while later queued
  STATUS destinations have not yet been filled, which changed the final
  packets even though all named registers matched.
- The depth-2 H.264 FFmpeg path reaches 898 FPS on 300-frame 720p versus C
  FFmpeg at 860 FPS, while 60-frame single-GOP output remains byte-identical.
  Depth 1 improves the synchronous 351 FPS path to 477 FPS but is still below
  C; depth 2 is therefore the default compatibility path.
- H.265 FFmpeg uses a short-term GOP, unlike the standalone long-term IDR
  variant. Its P jobs rotate through three reconstruction surfaces and refer to
  the immediately preceding surface. Depth 1 is byte-exact and improves the
  synchronous FFmpeg frontend from roughly 399 to 490 FPS.
- The H.265 depth-2 divergence was the missing frame-parallel half of VEPU580
  setup, not a reconstruction-slot mistake. C's `task_cnt == 2` path programs
  `reg0193_dual_core` from its persistent `curr_idx/prev_idx` counters: an
  intra picture restarts both at zero with `rxe=0`; later jobs use
  `txid=curr`, `rxid=prev`, `txe=1`, `rxe=1`, offset 9, then advance
  `prev=curr` and `curr=(curr+1)&3`.
- The first five C trace words are `00090010`, `00090031`, `00090036`,
  `0009003b`, and `0009003c`. Rust now emits the same sequence when depth 2 is
  selected. Both 320x240 and 1280x720 60-frame single-GOP streams match C, including
  the former divergence around frame 41.
- With the handshake present, 11 interleaved 300-frame 720p samples give Rust
  depth 2 **0.444 s / 676 FPS** and C **0.444 s / 676 FPS**. The earlier
  912 FPS result was the unsafe unsynchronised path. Rust CPU user+sys is
  0.372 s versus C 0.393 s, so there is no CPU-cost regression beyond the C
  baseline. H.265 depth 2 is now the compatibility default.
- The native file CLI now feeds the deferred session queue directly. H.265
  fixed-QP defaults to depth 2 when refresh/tiles/low-delay split are absent;
  300-frame 720p rises from 439 to 698 FPS and remains byte-identical to sync
  at 320x240 and 1280x720.
- H.264 native depth 1 is byte-exact but does not overlap enough work to beat
  the existing input-copy pipeline. Depth 2 reaches 906 FPS versus C's 829,
  but scene-tune observes STATUS one picture later, so it is opt-in rather than
  the default. The explicit high-throughput stream decodes all 300 frames but
  is not asserted byte-identical to sync across multiple GOPs.
- Attempting to catch scene tune up by reading the younger pending task's
  STATUS after the blocking poll did not change the stream: the younger
  `enc_done`/STATUS fields are still unavailable when the oldest task unblocks.
  Matching sync would therefore require waiting for the younger task before
  preparing the next picture, which removes the depth-2 overlap this mode
  exists to provide.
- Multi-process encoder tests show that host scheduling is no longer the limit:
  H.264 saturates near 1050 FPS and H.265 near 1010 FPS around four concurrent
  720p streams. Adding six or eight streams does not materially increase
  aggregate output even when each process is pinned independently.
- Vendor MPP uses `mpp_enc_async_thread`: the caller queues frame tasks and a
  worker thread advances submit, HAL start, HAL wait and packet publication.
  The current Rust VEPU580 path still runs `submit -> poll(enc_done)` inline
  for every picture.
- A pooled-surface compatibility experiment avoiding `dup/mmap` regressed
  decode because every access crossed `BufferPool` guards. It was fully
  reverted; the owned DMA-BUF wrapper remains faster.
- A first high-level async encoder worker moved the existing synchronous
  `Encoder` into a Rust thread while FFmpeg queued frames. It compiled and
  reduced direct blocking, but H.264/H.265 intermittently lost delayed packets
  and passed stale MPP frames back through FFmpeg's four-frame queue. The
  implementation was rejected. The proper design must split submit and finish
  below the encoder session so packet generation, input ownership and
  reconstruction lifetime are explicit per job.
- Async probing found two real MPP API bugs worth keeping: `encode_put_frame`
  must accept a NULL frame as EOS, and `encode_get_packet` must return
  `MPP_NOK` when no packet is ready. Returning `MPP_OK` with a null packet made
  the FFmpeg wrapper report `ENOMEM`.
- The unchanged `mpi_enc_test` and FFmpeg share `MPP_ENC_SET_CFG`, but not the
  same H.265 defaults. FFmpeg always writes `h265:level`; value zero is then
  raised by picture size (2.0 at 320x240) and its SPS enables temporal MVP.
  The stock tool leaves the key absent, so `h265e_init` retains level 4.0 and
  TMVP off. Preserving key presence reproduces both callers; VEPU510 uses 30
  rather than 120 when the key is absent.
- H.264 has a separate explicit-zero rule: FFmpeg's `h264:level=0` starts at
  1.0 and lets the SPS builder raise it to the minimum level that fits.
  Treating that zero as an unset reference level breaks FFmpeg at config time.
- H.265 scene-tune shares H.264's transition rules but not its mean-MADP gate.
  Vendor H.265 uses 896/640/384; reusing 192/128/64 keeps the detector in the
  wrong row even though single-GOP streams remain exact.
- The aggregate STATUS path must retain words 77–84. Words 64–75 include
  `madp`/`madi`/block counts, but the motion detector also needs the four
  `md_sad_b16num` and four `madi_b16num` histograms.
- The complete row-3 overlay is not just WGT 1479–1482. It also changes WGT
  1496/1498/1499/1501/1503 and 18 RDO words. The earlier captured values mixed
  row 2 and row 3, so the C job 106 -> 107 differential is the authoritative
  source.
- With those fixes, a 320x240 moving 300-frame/five-GOP stream matches C at
  315 NALs and SHA-256 `d577adc9…32edea`. Across 300 ioctl jobs only the 13
  DMA-BUF address words differ; every non-address register word is identical.
- The current upstream object path is already faster than C without spending
  more CPU. Five interleaved 300-frame samples at the RK3588 80% OPP cap give
  320x240 Rust/C wall 149/162 ms and user+sys 55/70 ms; 720p wall 806/864 ms
  and user+sys 236/294 ms. The benchmark gate treats both wall and CPU as
  hard limits rather than optimizing only elapsed time.
- `tune:scene_mode` has three vendor values, but only IPC (`1`) changes the
  VEPU580 tune branch. IPC-PTZ (`2`) still compares unequal to IPC in the HAL
  and therefore follows the default path.
- IPC row 0 is not just a different pre-intra table. It also changes WGT words
  1793–1828 and `RC_KLUT[1056]`; row 1 returns the KLUT offset to zero but keeps
  its own WGT/RDO tables. A 160-job C/Rust trace has exact WGT/RDO/OSD sections,
  with row 0 at job 0 and row 1 at job 5.
- H.264 shares the compatible scene-mode key but needs its own probe entry
  because the object probe's baseline framing differs from the native CLI.
  `H264_SCENE_MODE=1` produces 64 NALs over 60 frames and matches the C probe;
  the SHA difference is only the version SEI text.
- The current H.264 object path is not slower than C in any material cell.
  Five 300-frame samples at 80% OPP measure 320x240 Rust/C wall 157/152 ms and
  equal 67 ms user+sys; 720p is 769/822 ms with user+sys 232/278 ms. The shared
  benchmark treats CPU and wall as separate hard gates.
- `hw:qbias_*` is not just an SEI or config-store field. H.264 writes
  `RDO_QUANT.quant_f_bias_I/P`; H.265 packs both values into WGT word 1484.
  The vendor fallback differs by codec (683/341 vs 171/85), and disabling the
  override must preserve the active scene row instead of forcing either pair.
- `tune:vmaf_opt` is not equivalent to selecting the full scene row 3. It
  changes four WGT pre-intra words and 16 RDO words, while leaving row3's
  2167/2169 thresholds untouched. Treating it as a full row patch would change
  unrelated tuning state.
- The compatible tune controls also need native CLI exposure; otherwise users
  must route through the MPP object API to reach already-supported hardware
  behavior. `H265_SCENE_MODE`, `H265_QBIAS_*` and `H265_VMAF_OPT` now match C
  in the same trace gate.
- Short 320x240 encodes are dominated by process startup. At 1000 frames,
  H.264 Rust is 9.1% faster with 31.7% less user+sys, and H.265 is 5.6% faster
  with 24.3% less CPU. The earlier 300-frame H.264 3.3% deficit is not a
  steady-state regression.
- The top-level AV1 software tile entropy checkbox was stale. The current
  reference suite has 365 passing tests, including real multi-tile and advanced
  B-pyramid fixtures; the hardware/speculative rows are now the only remaining
  top-level items.
- AVS+ is not hardware-absent on RK3588. Client 12 binds, vendor C decodes a
  public 352x288 stream to exact NV12, and the Rust sequence/I-picture parser
  and 60-word HAL now match that raw frame. AVS2 is genuinely absent from this
  SoC and must not be conflated with AVS+.
- C's default AVS+ harness runs IEP2 vproc after client 12, so its single-frame
  oracle is post-processed. With `enable_deinterlace=0`, the raw Rust and C
  decoder outputs are byte-identical (`3acac415…2e11c2`); the 60-word register
  files match exactly apart from dma-buf fds.
- The public AVS sample's complete 7-frame display cycle is byte-identical after
  implementing the C parser's persistent `picture_reference_flag`, ref2/ref3
  field-offset rule, video-edit `vec_flag` reset, two-anchor rotation and
  displayed-slot-aware EOS flush (`47d05886…5fdb20` I420).
- The same AVS cycle passes through the public SDK and C ABI codec value 10, not
  just the direct CLI path; `examples/sdk_decode avs` and `avsdec` are identical.
- A second public fixture, `stream03.avs`, produces a 6-frame cycle that also
  matches C byte-for-byte (`e6069fc1…78f29`), so the result is not tied to the
  first sample's GOP shape.
- A 100x concatenated `stream01.avs` produces 700 raw-NV12 frames byte-identical
  to C. Three wall-clock samples had a large spread; the median was Rust 694 ms
  versus C 825 ms with vproc disabled. The CLI now offers a reusable-buffer NV12
  sink and a direct DMA-mapping callback path.
- C's default AVS path is now reproduced through a stateful Rust IEP2 pipeline:
  I1O1T for the first frame, one anchor-only frame, then alternating TFF/BFF
  I5O2 windows. Porting `iep2_update_gmv()` closed the last 164-word register
  mismatch; both public fixtures match C byte-for-byte in `vproc` mode. Their
  full post-processed cycles are 9 and 7 frames (`6ef29ca6…e169` and
  `35ac81e6…f3d1`). The decoder's three source frames are passed as borrowed
  dma-buf descriptors, avoiding the previous CPU copies.
- A 100x concatenated public stream emits 700 default-vproc frames byte-identical
  to C (`64b3df4c…be54a`). Repeated wall-clock samples are bimodal on both sides,
  so the DMA-BUF path is recorded as functionally exact without claiming a
  stable performance winner.
- The vproc CLI now writes the compact IEP result directly instead of copying it
  through a second reusable buffer. Three 700-frame samples were 525/523/790 ms;
  the output hash remains unchanged and timing remains bimodal.
- AVS vproc no longer remaps decoder frames for every IEP job. The three source
  pictures now borrow their descriptors for one run, while the two retained
  anchors hold only duplicated fds. Packed IEP destinations are emitted directly
  instead of allocating two `Vec<u8>` copies per I5O2 window. The same 700-frame
  stream remains byte-identical; five-run medians moved from Rust 556 ms before
  the change to 490 ms after it, versus C 809 ms. Timing is still bimodal.
  Paired `time -p` samples used 0.13-0.22 s of Rust user+sys CPU versus 0.26 s
  for C's two fast runs, so the wall-time reduction does not trade more CPU for
  speed.
- AVS vproc is now selectable through the stable API rather than only the CLI:
  Rust uses `DecoderConfig::with_vproc_output(true)` and C sets
  `RK_MPP_DECODER_FLAG_VPROC_OUTPUT`. The complete 9/7-frame outputs from both
  public fixtures match C through the Rust SDK and a compiled C consumer.
- The AVS parser has a deterministic 20,000-iteration mutation test covering
  truncation, byte flips, insertion and arbitrary inputs; all parser entry points
  return errors or partial parses without panicking.
- `gen_avs_plus_profile.py` derives a syntax-equivalent profile 0x48 stream by
  preserving slice data and adding the AVS+ header bits with weighting/AEC
  disabled. Its first four raw frames match C exactly (`9eb0602a…5cfa4`).
- Enabling base1 weighted quant with chroma deltas and model2 also decodes
  byte-exactly through C and Rust (`669ae431…4ff9f`). AEC cannot be validated by
  header derivation because it changes slice entropy coding.
- The AVS+ corruption matrix separates valid behavior from harness behavior:
  Rust suppresses hardware-error frames and bounds malformed inputs, while C's
  `mpi_dec_test` can report success with no frame, hang on a 100-byte truncation,
  or accept a mutated profile byte.
- The official H.265 encoder's version SEI contains the producing binary's
  build identity, so whole-file `cmp` is not a valid correctness gate. On
  RK3588 the parameter sets, rc-SEI, VCL NALs and decoded reconstruction match;
  only that text differs. The board gate therefore uses `compare_streams.py`
  (version SEI framing only) and an explicit YUV comparison.

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| `rk_mpp_decoder_*` V1 ABI | Clear codec-agnostic naming and future extension |
| `struct_size` on public structs | ABI evolution without silent layout breakage |
| Decoder-owned frame + explicit release | Stable pointer lifetime and no caller allocator coupling |
| Plane pointer/stride array | Covers NV12, planar YUV and future formats without reinterpretation |
| H.264 framing at VCL/AUD boundaries | Reuses the proven access-unit semantics |
| MJPEG SOI/EOI scanner | Decouples JPEG decode from files and supports concatenated streams |
| Sticky EOS plus idempotent flush | Gives callers one unambiguous end condition and allows reset/reuse |
| Packet metadata inherited by all pictures in that packet | Avoids inventing timestamps when one packet contains multiple AUs/frames |
| New V1 YUV-only output | Deterministic and cheap; legacy JPEG remains the RGB compatibility path |
| Extend ABI 1.1 with H.265 and `I420P10` | Adds codecs/output without changing existing struct layouts or symbols |
| Reorder H.265 only until the SPS is available | Parameter sets usually share the first AU; using the SPS value avoids a fixed conservative delay |
| Keep VP9 packet input container-neutral | Payloads are already frame-delimited; superframes are handled by the decoder |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| Existing root `Decoder` name is JPEG-specific | Keep it and use `sdk`/`c_api` naming for new generic surface |

## Resources

- `src/lib.rs`
- `src/codec/h264/decoder.rs`
- `src/codec/hevc/decoder.rs`
- `src/codec/vp9/decoder.rs`
- `src/codec/av1/decoder.rs`
- `src/codec/vp8/decoder.rs`
- `src/hal/h264e.rs`
- `src/codec/jpeg/parser.rs`
- `src/hal/jpegd.rs`
- `src/bin/h264_stream.rs`
