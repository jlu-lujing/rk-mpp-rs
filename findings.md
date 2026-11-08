# Findings & Decisions

## Requirements

- Priority codecs: H.264 decode and MJPEG decode.
- Public API must support live packet input, persistent sessions, frame output,
  output metadata, EOS/flush/reset, and errors.
- Keep the current `rk_jpeg_open/frame/close` ABI for `rknn-fastinfer`.
- Add a versioned C header and verify actual C and C++ consumers on RK boards.
- Extend the public contract to all implemented codecs/encoders after the
  priority H.264/MJPEG path is stable.

## Research Findings

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
- The H.265/HEVC C consumer passes on lf1 and t6 for 8-bit NV12/I420 and
  Main10 I420P10. The Main10 fixture has six frames with B-pyramid reordering,
  so SPS-bounded display ordering is exercised rather than only IDR output.
- VP9 already emits displayed pictures directly from `decode_packet`, including
  `show_existing_frame` and hidden-alt-ref superframes, so the SDK does not add
  another reorder queue. VP9 byte-stream packet boundaries are normative; IVF
  demux remains a caller/container responsibility.
- The VP9 C gate passes on lf1 and t6: profile 0 12-frame NV12/I420 and profile
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
- Verified AV1 8-bit key+P SHA-256 is `7f4a0c9e…4020f`. On lf1 VDPU383
  10-bit output matches C at `f3b69b61…49b8`; on t6 grain-applied output is
  `b75e6172…904d`, matching the Rust/libaom result while C retains its known
  missing-film-grain bug.
- `hal::h264e::Vepu580H264::encode_frame` is already synchronous: it copies the
  NV12 input, submits/polls the hardware and returns complete `EncodedFrame`
  values with packet boundaries and sequence metadata. A public encoder can
  therefore queue fully-owned output without inventing asynchronous ownership.
- RK3576's captured VEPU510 H.264 path is fixed at 320x240, Baseline QP26,
  one IDR plus P pictures and 30 fps. The public API exposes only that validated
  path instead of pretending the VEPU580 configuration surface transfers.
- The public H.264 encoder's 6-frame output matches the VEPU580 CLI on t6
  (SHA-256 `aa6276ba…173d1`) and `h264e510-gop6` on lf1
  (`0e65b51a…81f33`). NV12 and converted I420 inputs match each other on each
  board; the two SoCs use different hardware bitstreams by design.
- VEPU580 H.265 already exposes a frame-level fixed-QP session, so the public
  encoder can queue one parsed packet per picture. RK3576's captured VEPU510
  path remains limited to 320x240 QP26 GOP6 but now emits one picture per
  submit through a small streaming wrapper.
- Verified H.265 hashes are `6c285115…979e9` on t6 VEPU580 and
  `8088e821…7fe397` on lf1 VEPU510; NV12 and I420 match on each platform.
- The existing `jpege720::JpegEncoder` already selects VEPU2 or VPU720 and
  returns a packet-range view, so JPEG needed no new hardware wrapper. The
  public layer pads visible NV12/I420 input to the JPEG stride rules and queues
  the returned packet ranges.
- Verified JPEG hashes are `4f571054…7db1e` on t6 VEPU2 and
  `64153672…7ac69` on lf1 VPU720; NV12 and I420 match on each platform.
- VP8's decoder already returns display-ready `Picture` values per IVF packet,
  including hidden-alt-ref handling. Unlike VP9/AV1 it has no register status
  accessor, so the shared decoder status call must return unsupported.
- VP8 API hashes on t6 are `1f07acf5…4c9c2` (NV12) and
  `0a6bbff3…326096` (I420). RK3576 C MPP reports "unable to create dec vp8
  ... unsupported"; the Rust handle also rejects create, so lf1 is a hardware
  SKIP rather than a masked failure.
- The legacy MPEG-2/MPEG-4/H.263 front ends only expose whole-stream decode, so
  the shared API buffers arbitrary chunks and emits all display frames at
  EOS/flush. This is an explicit latency boundary, not packet-level decode.
- Verified legacy NV12 hashes on t6 are MPEG-2 `2dc97045…6371e`, MPEG-4
  `33cde882…64580`, H.263 `4a2a9312…c2f45e`; I420 equivalents also match.
- JPEG restart interval 20 matches the CLI output at lf1
  `0ccf41ea…82997` and t6 `25254033…df748a`.
- Verified VP9 SHA-256 values are
  `c5381bbf…3949a` (profile 0 NV12),
  `53bc3dd4…0abea69` (profile 0 I420), and
  `1c4c63f9…9a09` (profile 2 I420P10).

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
