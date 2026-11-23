# RK3588 validation — 2026-11-20

This is the current integrated validation checkpoint after moving work from a
second RK3588 validation system back to the primary RK3588 board.

## 2026-11-23 4K codec matrix

`scripts/board_4k_matrix.sh` passes **24/24** at 3840×2160. The matrix covers
H.264/H.265/VP9 8-bit one- and three-frame decode, H.265 one-frame Main10,
VP9/AV1 Main10 one- and three-frame decode, AV1 8-bit one/three-frame decode,
AV1 8/10-bit film grain, H.264/H.265 4K encode, MJPEG
1080p/1440p/4K encode, and 4K MJPEG decode. Hardware decode is compared byte
for byte with both C MPP and an independent software oracle; H.264/H.265/MJPEG
FFmpeg drop-in encode is byte-identical to vendor.

The matrix found two compatibility defects that do not reproduce below 10-bit
or below 1080p:

- The MPP object path requested NV12 DMA-BUF for every decoder. H.265/VP9/AV1
  Main10 therefore failed before a picture was returned. Ten-bit 4:2:0
  hardware surfaces now use MPP `YUV420SP_10BIT` (FFmpeg `NV15`).
- VP9 used the pixel width as the 10-bit byte stride. A 4K Main10 row needs
  4,800 bytes and MPP's 256-odd alignment yields 4,864; the old 3,840-byte
  stride panicked while reading packed samples.

The same run fixes JPEG object-path output above 720p. FFmpeg only configures
quality, so the compatibility path must not auto-enable the direct
`jpeg-enc` multi-core bands. At 1920×1080, 2560×1440 and 3840×2160 the Rust
and vendor JPEGs are identical and have no DRI marker.

The matrix uses `aomdec --rawvideo` for AV1 because this FFmpeg build has no
usable AV1 software decoder. VP9 Main10 generation requires a `vpxenc` built
with `--enable-vp9-highbitdepth`; pass it through `VPXENC=`. For FFmpeg 4K
decode, use `-fps_mode passthrough`; `-frames:v 1` can otherwise produce an
empty output on both vendor and Rust paths.

The three-frame Main10 additions close the temporal-reference gap that a
single-keyframe test cannot cover. VP9 Main10 produces three frames and the
same C/Rust/software SHA-256
`f485a8d83f3d17ed861f0e86512a4f6c907bda0f2e3ca8cb1af3d25d49814709`;
AV1 Main10 produces
`9584a79e5e6250952eee22a08be28f32086bc1f7d48a5ced736cab3ddb1b1bd8`
across C, Rust and `aomdec`.

`scripts/board_4k_features.sh` extends the baseline to **24/24** feature-gate
groups:

- H.264 Baseline/Main/High at QP26/QP18: 6/6 one-frame streams, five NALs each.
- H.264 CBR/VBR/AVBR, MPI quality band and hierarchical QP: 7/7.
- H.264 fixed-QP and CBR ROI/QP maps: 2/2.
- H.264 4K slice split: 680 NAL for BY_CTU=96 and 403 for BY_BYTE=256.
- H.265 CBR/VBR/AVBR/smart-GOP: 4/4 after the RC re-encode fix below.
- H.265 ROI, slice, auto-tile, GDR and forced-IDR/header mode.
- H.264/H.265 custom RC callbacks, range changes, same-frame re-encode and
  frame-type re-encode.
- 4K `MppEncCfg` NV12/I420/NV21 inputs produce identical H.264 and H.265
  streams.
- `board_4k_soak.sh`: 30-frame H.264/H.265/VP9 three-way decode, 10-frame
  H.265 Main10 three-way decode, and 30-frame H.264/H.265 byte-exact encode.
- H.264/H.265 4K OSD direct and CPU fallback: streams and decoded YUV match.
- MJPEG 4K multi-core (DRI=8160), 90/270-degree rotation, RGB565, ARGB8888 and
  BGRA8888 at qf50/qf90.
- JPEG decoder 4K IDCT scaling at 1/1, 1/2, 1/4 and 1/8 for RGB888 and NV12.
  Geometry and repeated-run hashes are exact; RGB PSNR against the software
  `area` scaler is 60.52/30.82/33.00/27.61 dB. The stock C test CLI has no
  JPEG scale option, so this is a deterministic content/DMA regression gate
  rather than a byte-exact C comparison.
- Independent-process 4K H.264/H.265/VP9 decode at 1, 2, 4, 8 and 16 streams.
  Every codec first matches the single-stream C decoder, then every concurrent
  output matches that golden byte for byte: 15/15 cells, no missing frames.
- H.264 and H.265 forced-IDR/header mode at 4K. H.264 produces 12 NALs,
  H.265 produces 15 NALs, and the C/Rust decoded YUV agrees in both.
- Decode 4K H.264 through both CPU and DMA-BUF frames, scale strided NV12
  directly into the 640x640 RGB model input, and require byte equality at
  geometry 640x360 with top padding 140.
- SDK copied-input and caller DMA-BUF-input encoders are byte-identical at 4K
  for H.264, H.265 and JPEG. The SDK decoder's CPU and DMA-BUF output frames
  are byte-identical at 4K for H.264, H.265, VP9 and AV1, three frames each.
- IEP2 deinterlace at its 1920x1088 maximum is byte-identical to C. A 4K
  request is rejected by the C test harness and now rejected by Rust before
  session open; a direct C probe otherwise returns success with two all-zero
  outputs, confirming that 4K is unsupported rather than a valid hidden mode.

The 4K H.265 rate-control case found a real policy difference. With a 300 kbps
target, C's first IDR exceeds the 86,956-bit allocation and is resubmitted at
QP29; the previous Rust path encoded once at QP26 and moved QP32 to the next
picture. The resulting first IDR was 20,615 bytes instead of C's 12,150. The
model-driven H.265 path now uses the same re-encode policy loop and retains its
DMA-BUF input fd across retries. CBR/VBR/AVBR and smart-GOP are byte-identical
after version-SEI normalization at both 320×240 and 3840×2160.

## 2026-11-20 Public H.264/H.265 syntax API V1.13/V1.14

`scripts/board_sdk_encoder_controls.sh` now creates the encoder through the
new V3 advanced configuration and compares its output with the validated CLI
path. All four cells are byte-identical:

| V3 control | bytes |
|---|---:|
| Main CABAC, `cabac_init_idc=2` | 18,046 |
| constraint-set force/value `0x003f0025` | 17,977 |
| `base_layer_pid=5` with Main defaults retained | 17,977 |
| Main forced to CAVLC and no 8x8 transform | 20,263 |
| High forced to disable 8x8 transform | 17,979 |
| deblock disable with alpha/beta `-2/3` | 18,098 |

The same run retains the existing V1.9 compatibility consumer and all six
V1.10 ROI/GDR/slice cells. `scripts/check_sdk_abi.sh` passes with the V1.14
41-symbol snapshot, and `scripts/board_sdk_package.sh` installs
`.so.1.14.0` and passes pkg-config, CMake shared/static and external consumers.

The V1.14 H.265 descriptor reuses V3's final 32-byte reserve and remains
byte-identical to the CLI in three more cells:

| V3 H.265 control | bytes |
|---|---:|
| SAO luma/chroma disabled | 15,618 |
| deblock beta/tc `3/-2` | 15,878 |
| loop filter across slices, disabled across tiles | 15,829 |
| auto-tile, 4608x64 one frame | 8,876 |

The V3 size remains 552 bytes. ABI/package gates now use V1.14 and 41 symbols,
and the default install artifact is `.so.1.14.0`.

## 2026-11-20 Compatible encoder input submits dma-buf directly

The official `MppFrame` compatibility path previously packed every NV12 input
into `encoder_input_scratch`, then the typed encoder copied it into its HAL
surface. It now borrows the internal dma-heap fd and submits the caller's
existing NV12 allocation directly when all of these are true:

- codec input is NV12;
- the frame buffer has a valid fd;
- no OSD or custom-RC work is pending;
- allocation size and coded Y/UV strides match the typed encoder.

I420/NV21 frames, OSD/custom-RC frames and geometry mismatches keep the CPU
fallback. The new fast path also avoids the interim `DmaBufInput::new()` cost.
Separately, the stable `DmaBufInput<'a>` now borrows the original fd instead
of importing or duplicating it, because hardware submission needs just the fd,
length and strides; the lifetime covers the synchronous call. A real VEPU580
test showed that a duplicated fd completes without `enc_done` (`int_sta=0`).
The stable C ABI
`rk_mpp_encoder_submit_frame_dmabuf()` is synchronous, so it borrows the
caller's fd directly and avoids even that duplicate. A six-frame H.264
`strace -f -e trace=fcntl` run reports zero `F_DUPFD_CLOEXEC` calls;
`board_sdk_encoder_dmabuf.sh` enforces that when `strace` is available.

`scripts/board_mpp_enc_cfg_dma_input.sh` encodes 120 frames of 1280x720
`testsrc2` at 80% OPP through both an internal dma-heap buffer and an internal
CPU buffer. The stream comparison removes only version-SEI framing text:

| codec | identical NALs | direct stream SHA-256 |
| --- | ---: | --- |
| H.264 | 126 | `2fdcd6edd0412e7e666a121ceaae93302e506e50fcfd2cca4a8cc611abcbb2c3` |
| H.265 | 125 | `1865f014eefc877719b1ebc4adf989584d1b8e974b87124b21d4bfd7bf7af84b` |

Nine interleaved H.264 samples used shell `time` with the same probe, fixture
and parameters:

| path | wall samples | wall median | user+sys samples | user+sys median |
| --- | --- | ---: | --- | ---: |
| internal dma-heap | 273–275 ms | **273 ms** | 59–60 ms | **59 ms** |
| CPU fallback | 306–449 ms, bimodal | **311 ms** | 90–232 ms | **95 ms** |

The direct path therefore removes the staging copy and fd-import overhead
without changing a NAL. The 12.2% median wall improvement understates the
lower 306 ms mode; total CPU is consistently about 38% lower.

VEPU580 H.264/H.265 OSD tables are independent of the source surface, so the
same fd path can carry OSD without copying the picture. `board_mpp_osd.sh`
now encodes the vendor library, Rust with
`RKMPP_COMPAT_OSD_DMABUF=1`, and Rust with forced fallback, then compares
streams and decoded YUV. H.264 produces 6 identical NALs with YUV
`1055ca00…73b22a`; H.265 produces 7 identical NALs with YUV
`6f44da15…e5b176`. The comparator now recognises H.265's build-identity
PREFIX_SEI, matching its existing H.264 version-SEI exception.

OSD direct is opt-in rather than default. Seven interleaved 500-frame 720p
samples through `mpi_enc_test`'s cached DRM input were bimodal on the direct
path (1.295-2.114 s, median 1.795 s), while fallback stayed at 1.413-1.576 s,
median 1.428 s. `RKMPP_COMPAT_OSD_DMABUF=1` enables direct submission for
surfaces whose cache policy is known suitable; `RKMPP_COMPAT_CPU_INPUT=1`
forces the fallback.

The Rust dma-buf API exposes the same hardware capability through
`DmaBufInput::with_osd(config)`. The `encoder::tests::dmabuf_osd_hw_matches_copy`
board test runs copied and dma-buf H.264/H.265, both with and without OSD, and
requires all four streams to match byte for byte. VEPU510 and JPEG still reject
OSD explicitly.

### H.265 session CPU input becomes uncached

The VEPU580 H.265 session still copied CPU input into a cached `system`
allocation and issued `DMA_BUF_SYNC` START/END around every picture, even
though the file pipeline already used `system-uncached` surfaces. The session
now matches that pipeline and omits the two redundant sync ioctls.

Seven interleaved 500-frame 720p samples through the compatible encoder
fallback measured:

| change | wall median | user+sys median |
| --- | ---: | ---: |
| `system` + sync | 2.379 s | 1.287 s |
| `system-uncached` + sync | 2.006 s | 1.011 s |
| `system-uncached`, no sync | **1.412 s** | **0.455 s** |

The old and new outputs are byte-identical:
`6fea674832963f18b0b7f2fe0a41421471ea7bfcb6d36d59ebf2720a0da157dc`.
The identical H.264 sync removal was tested and reverted because it produced
no stable gain on the same workload.

The aggregate H.265 feedback buffer was still clearing all 180 STATUS words
every picture, although `Statistics::from_words` only consumes words 0, 1, 2,
3 and 64-75. The session now clears and accumulates only the 14 words that are
actually feedback; words 1/2 are then overwritten by the summed SSE. Seven
interleaved 500-frame 720p samples moved wall median from 1.412 to **1.318 s**
(-6.7%) and user+sys from 0.455 to **0.366 s** (-19.6%). The stream hash stayed
identical.

### H.264 CABAC initialization models

`h264:cabac_idc` now reaches `H264SyntaxConfig`, the VEPU580 slice register and
the software P_SKIP context initializer. The latter uses the same
`skip_init_state[3][2]` rows as C. Two-frame Main-CABAC vendor/Rust object
probes are six-NAL byte-identical for idc 1
(`47e28d89…3862a80`) and idc 2 (`85087619…5c76b6c99`). Model 3 is rejected
before hardware submission.

`h264:constraint_set` now retains C's `(force_mask << 16) | value` semantics
instead of overwriting the whole SPS byte. Forcing set1=0 (`0x020000`) and
set0=1 (`0x010001`) produce five-NAL streams identical across vendor, Rust
object and typed CLI paths (`78e1057c…d2e1584`, `f60db229…4abcdee`).

`h264:base_layer_pid` feeds the type-14 SVC prefix as
`priority_id = base_layer_pid + temporal_id`. The hardware gate forces base 5
and temporal id 3 on a VEPU580 frame and requires the prefix to carry
priority id 8. Values above 56 are rejected before submission.

## 2026-11-17 SDK / C API now use the submission ring

- `SdkSession` enables the ring at depth 3 for H.264 and H.265 by default
  (`RKMPP_SDK_PIPELINE=0` restores blocking). The blocking C-API comparison
  failed at first because a packet carrying `PACKET_FLAG_EOS` set the finished
  flag and flushed the reorder buffer without draining the decoder ring, which
  dropped `depth - 1` frames. Both EOS paths now drain the ring first, and
  `Decoder::flush` does the same for callers that never send EOS.
- `take_h264_access_units` uses the header-only Annex-B split (access-unit
  boundaries need the NAL type and the first payload byte, not a de-escaped
  RBSP), removing the `ebsp_to_rbsp` pass from every submitted chunk.
- Evidence: `board_c_api_h264_mjpeg.sh` PASS with the ring on, including the
  313-byte-chunk and one-packet paths; workspace 59 suites exit 0. On a
  120-frame 720p H.264 stream through the C API: blocking 708-720 ms
  (bimodal; later samples 362-368 ms) versus **235-240 ms** with the ring,
  about 1.6-1.7x. The reference C frontend takes 194 ms on the same stream,
  so the C-API wrapper is still roughly 20 % behind C while the CLI and direct
  decoder paths are 1.2-1.8x ahead. Two further changes close that:
  `rk_mpp_frame_release` recycles the host NV12 buffer into the decoder
  (`nv12_to_packed_into`), and the SDK's chunked input now uses an incremental
  Annex-B NAL scanner instead of re-splitting the pending buffer per submit.
  The scanner is unit-tested against the one-shot splitter (1-byte chunks and
  the real 120-frame stream). Final C-API numbers on 720p NV12, both outputs
  byte-identical to `ffmpeg -c:v h264_rkmpp -pix_fmt nv12`: blocking 559-570 ms
  (bimodal 308 ms), depth 3 **171-207 ms**, against C 159-211 ms — parity
  within noise.

## 2026-11-18 Concurrent-stream ceiling: 40 vs C's 26

On one 1920x800 / 742-frame H.264 stream, independent processes writing planar
YUV to `/dev/null`, the slowest stream is the reported one (three stable
rounds at the ceiling):

| | streams at >=30 FPS | aggregate | CPU at ceiling | RSS at ceiling |
| --- | ---: | ---: | ---: | ---: |
| C `ffmpeg h264_rkmpp` | **26** | 798 FPS | ~100 % | ~3.3 GB |
| Rust `h264dec` | **40** | 1225 FPS | ~18 % | ~2.2 GB |

The limit is the VPU, not the host: at 40 Rust streams each `rkvdec` core
serves 620 jobs/s (1240 total, exactly 40x30); at 26 C streams each serves
442/s because the C frontend has already saturated every CPU core. Throughput
scales with the stream: 640x360 ~5941 FPS (~198 streams), 720p ~1630 (~54),
1920x800 ~1214 (~40), 1080p ~735 (~24). All 40 Rust processes reported
742/742 frames with exit 0. See `docs/NETWORK_DECODE_BENCH.md`.

## 2026-11-17 Encoder throughput against the C library

Five interleaved samples per size, 12 frames each, wall clock around both
commands (`mpi_enc_test` for C, the Rust CLI for this crate):

| encoder | 320x240 | 640x480 | 1280x720 | 1920x1088 |
| --- | ---: | ---: | ---: | ---: |
| H.264 Rust vs C | +3.3 % | +50.0 % | +44.0 % | +36.7 % |
| H.265 Rust vs C | +38.5 % | +2.4 % | +0.0 % | +1.9 % |

JPEG encoding is also ahead on a 1080p 12-frame run (about 71 ms against
115 ms for `ffmpeg -c:v mjpeg_rkmpp`), but the two were driven with different
quality knobs, so only the direction is claimed.

## 2026-11-17 Final decoder throughput against the C baseline

Interleaved samples, DDR/CPU governors pinned, planar output to `/dev/null` on
both sides. "verified" means the two decoders were byte-compared first; the C
baseline is production `ffmpeg -c:v <codec>_rkmpp` except AV1, where the
board's `av1_rkmpp` frontend cannot convert the stream at all and
`mpi_dec_test` is the C decoder.

| codec | stream | C | Rust | Rust advantage |
| --- | --- | ---: | ---: | ---: |
| H.264 | 1920x800 inter, 600f verified | 931 ms | **522 ms** | **+78.4 %** |
| H.264 | 1080p inter, 90f verified | 308 ms | **195 ms** | **+57.9 %** |
| H.264 | 720p, 120f verified | 194 ms | **133 ms** | **+45.9 %** |
| H.264 | 1920x800 all-intra, 240f verified | 446 ms | **325 ms** | **+37.2 %** |
| H.265 | 720p, 120f verified | 204 ms | **164 ms** | **+24.4 %** |
| H.265 | four-IDR 720p, 120f verified | 206 ms | **165 ms** | **+24.8 %** |
| VP9 | 720p, 120f, 15 samples | 207 ms | **159 ms** | **+30.2 %** |
| VP9 | 320x240, 30f | 61 ms | **37 ms** | **+64.9 %** |
| VP8 | 320x240, 120f verified | 107 ms | **90 ms** | **+18.9 %** |
| MPEG-2 | 320x240, 100f verified | 78 ms | **68 ms** | **+14.7 %** |
| MPEG-4 | 320x240, 120f verified | 105 ms | **71 ms** | **+47.9 %** |
| H.263 | CIF, 120f verified | 118 ms | **104 ms** | **+13.5 %** |
| AV1 | 720p, 60f, vs `mpi_dec_test` | 182 ms | **149 ms** | **+22.1 %** |
| AVS+ | 352x288, 700f verified, raw NV12, 5-run median | 808 ms | **606 ms** | **+33.3 %** |
| AVS+ | 352x288, 700f verified, default IEP2 vproc, 5-run median | 809 ms | **490 ms** | **+65.1 %** |

The 120-frame VP8/MPEG-2/MPEG-4/H.263 streams are ten-times concatenations of
the corpus fixtures (VP8 keeps a corrected IVF frame count); each was verified
byte-identical to C before timing. Interlaced MPEG-2 is excluded: its
concatenation hits the documented C input-wrap bug, so its evidence stays the
1x fixture that matches FFmpeg's rounding.

Every row above is ahead of C. VP9 720p is bimodal on this board (144-245 ms
on both sides), so its 15-sample median is quoted.

The AVS+ raw row disables C's IEP2 vproc so both sides write the same decoder
NV12. The vproc row uses C's default path and Rust's decoder-DMA-BUF import
pipeline. Both 700-frame outputs are byte-identical. Individual runs remain
bimodal, so the table quotes five-run medians; paired CPU samples put Rust at
0.13-0.22 s user+sys versus 0.26 s for C's two fast runs.

## 2026-11-17 NEON chroma split and VP9 streamed output

- `nv12_to_planar` now deinterleaves chroma 16 UV pairs per iteration with
  `vuzp1q_u8`/`vuzp2q_u8` on aarch64 (runtime-detected, scalar tail, unit
  tests unchanged). Every decoder that emits planar YUV uses this path.
- The VP9 CLI now writes each packet's pictures as they come out instead of
  collecting the whole stream. Holding ~1.4 MB per 720p picture made the
  allocator fault in and clear fresh pages for every frame: `perf` attributed
  about 20 % to page clear/fault/dcache traffic. The streaming loop keeps
  `decode_stream`'s damage policy exactly (recoverable syntax error drops the
  packet, anything else stops the run) by using the typed packet error, so the
  corrupt-stream verdict table is unchanged.
- Nine interleaved samples, governors pinned, planar to `/dev/null`:
  VP9 720p C 201 ms vs Rust **168 ms (+19.6 %)**, VP9 320x240 C 45 ms vs
  **33 ms (+36.4 %)**. Re-running the other decoders with the same change:
  H.264 1920x800 inter 953 vs 527 ms (+80.8 %), 1080p 286 vs 181 ms
  (+58.0 %), 720p 208 vs 124 ms (+67.7 %), all-intra 435 vs 334 ms
  (+30.2 %); HEVC depth 3 720p 204 vs 157 ms (+29.9 %) and the four-IDR
  stream 215 vs 157 ms (+36.9 %).
- Gates: `board_vp9_matrix.sh` fail=0 named-differences=0 (including the known
  C bug row), `board_vp_corrupt_vs_mpp.sh` exit 0, workspace 59 suites exit 0.

## 2026-11-17 H.265 job ring: now the default and ahead of C

- The same submission-ring work was ported to the HEVC HAL: per-job CABAC,
  SPS/PPS, RPS, scaling-list and RCB tables, a private register window,
  readback and ioctl batch, plus deferred picture output that leases the
  video, colmv and every reference surface the job reads.
- Error propagation now matches the blocking path. A queued job that fails is
  reported after later pictures were already submitted, so the decoder marks
  every still-pending picture it poisoned, writes the failure back to the DPB
  and forces reference errors on the rest of the chain until the next IRAP.
  `hevc_slices_open` (a hardware failure at POC 5 cascading through POC 4/3
  and 9/7/6/8) is the regression that pins this: blocking and queued output
  are byte-identical, 23 clean frames.
- Seven HEVC fixtures are byte-identical between blocking and depth-3 queued
  output: `t720.hevc`, the four-IDR stream, Main10, and the three scaling-list
  variants.
- Seven interleaved samples, governors pinned: 720p 120 frames C 199 ms,
  blocking 412 ms, default depth 3 **164 ms (+21.3 % vs C)**; the four-IDR
  stream C 210 ms, blocking 425 ms, depth 3 **175 ms (+20.0 %)**.
- `RKMPP_HEVC_PIPELINE=0` restores blocking; `3` is the default.
- Gates: `board_regress_hevc.sh` PASS on tiles, Main10, three scaling-list
  variants, open-GOP multi-slice and the four-IDR stream; workspace 59 suites
  exit 0.

## 2026-11-17 H.264 submission ring: queued decode is the default and ahead of C

- The deferred-output bug is closed. The pending list is now
  `VecDeque<Option<PendingFrame>>` with one entry per submitted job, so a top
  field (job, no picture) and an error-concealed picture (picture, no job) no
  longer desynchronise the ring. Previously depth 3 dropped one picture and
  the PAFF / missing-AU streams diverged.
- The slot selector prefers a surface nobody else leases, and the surface
  pools carry `JOB_SLOTS` spare slots. When the pool is still short, the
  oldest job is reaped early and its picture queued, so pool pressure
  throttles the queue instead of failing the decode.
- `rk-mpp-rs h264dec` now keeps three jobs in flight by default
  (`RKMPP_H264_PIPELINE=<depth>`, `0` = blocking). Depth 3 reaches the same
  per-picture poll time as 4 (about 15 us) without the extra surface. The
  library default is unchanged, so SDK/C-API callers see the same semantics.
- Fairness gate first: every timed stream is verified byte-identical between
  Rust, C and software FFmpeg (`test720.264` 120/120, `test_hard.264` 90/90,
  `/tmp/ai.264` 240/240, `/tmp/demo_short.264` 600/600). Closing the last one
  found a real bug: `frame_num` wraps at 16 in that stream, and DPB eviction
  plus P-reference-list ordering compared the raw `frame_num` instead of
  FrameNumWrap, so the older picture landed at `ref_idx` 0 and produced ±1
  drift from frame 18. Frame stores now carry the epoch and both orderings use
  FrameNumWrap; two unit tests pin it (`docs/DIVERGENCES.md`).
- Seven interleaved wall-clock samples, DDR governor pinned, planar output to
  `/dev/null` on both sides, Rust default depth 3 against production
  `ffmpeg -c:v h264_rkmpp`:

  | stream | frames | C | Rust | Rust advantage |
  | --- | ---: | ---: | ---: | ---: |
  | 1920x800 inter (`/tmp/demo_short.264`) | 600 | 923 ms | **533 ms** | **+73.2 %** |
  | 1920x1080 inter (`test_hard.264`) | 90 | 287 ms | **202 ms** | **+42.1 %** |
  | 1280x720 | 120 | 207 ms | **141 ms** | **+46.8 %** |
  | all-intra 1920x800 | 240 | 442 ms | **372 ms** | **+18.8 %** |

  Wall clock includes start-up; FFmpeg's fixed cost is about 20 ms higher on a
  two-frame stream, so the short-row margins drop by roughly 5 points and the
  600-frame row stays near +71 %. Single-job on the 1920x800 inter stream was
  1819 ms against 533 ms at depth 3, i.e. the ring is worth ~3.4x inside the
  decoder. The earlier "Rust trails C by 1.45x" checkpoint is superseded.
- Correctness: `scripts/board_regress.sh` **7/7 PASS** with the default
  depth, including `field_paff.264` (top/bottom fields) and
  `mr1_missing_au20.264` (concealed pictures). Forcing depths 1-4 on all seven
  streams is byte-exact, the packed NV12 path matches the planar reference
  converted by ffmpeg, and DMA-BUF output still returns 120/120 frames.
- Gates: workspace **1319 passed / 0 failed / 45 ignored** over 59 suites,
  release library **1149 passed / 0 failed / 0 ignored**, `cargo fmt --check`
  clean. Numbers and the bug list live in `docs/NETWORK_DECODE_BENCH.md`.

## 2026-11-16 H.264 stream-decode path optimization

- The network/stream path no longer rescans Annex-B start codes per access
  unit. `split_annex_b_headers` records NAL header ranges once, and
  `Decoder::decode_parsed_access_unit` de-escapes only the AU being decoded.
  Two unit tests lock the new path: header split matches the full NAL split on
  type/ref-idc/offset/length, and the parsed AU boundaries equal the old
  `split_access_units` ranges.
- `perf` before the change put `split_annex_b` at 63.8 % of CPU. After it, the
  per-picture cost is about 1.9 ms in hardware submit+poll versus 0.3 ms for
  parse and DPB preparation, so the remaining C gap is queue depth rather than
  a CPU hot loop.
- The non-blocking hardware poll interval now defaults to 20 us instead of
  200 us and is cached with a `OnceLock` (`RKMPP_POLL_US=0` spins). This alone
  was about +2.5 %.
- `RKMPP_DECODE_NV12=1` exposes the native packed output and
  `RKMPP_DECODE_DMABUF=1` keeps the hardware surface, avoiding planarisation
  and the full-frame CPU copy for an in-process consumer.
- Sustained 1920x800 network stream (`/tmp/rfi-network-demo30.264`, 742
  frames, 7 alternating samples): Rust planar improves 333.18 -> 363.73 FPS
  (**+9.15 %**), and the native-surface median is 446.45 FPS versus C
  `h264_rkmpp` -> NV12 at 690.23 FPS. See
  `docs/NETWORK_DECODE_BENCH.md` and `bench/network-decode/2026-11-15/`.
- Gates on this tree: workspace gate **1319 passed / 0 failed / 45 ignored**
  across 59 suites, `cargo test --release --lib --offline` **1149 passed /
  0 failed / 0 ignored**, `cargo fmt --check` and rustdoc `-D warnings` clean.
- Lazy RBSP de-escaping: `decode_nals` takes `NalHeader` and expands only the
  SPS, PPS and the first VCL slice header; every slice payload reaches the
  hardware as raw EBSP. The `00 00 03` probe behind `ebsp_to_rbsp` is now a
  word-at-a-time SWAR scan with a unit test over 3^12 payloads. Seven
  alternating samples give Rust planar 2013 ms (368.60 FPS) and DMA-BUF
  1585 ms (468.14 FPS), against C 1284 ms / 1092 ms.
- Measured with `perf stat` and per-phase timers: the CLI is idle 83 % of the
  run (283 ms of task-clock over 1629 ms wall), submit costs 18 us per
  picture and the hardware poll 1761 us. Both `rkvdec-core` instances get
  equal interrupt counts, so the remaining C/Rust gap is queue depth — see the
  concurrency table in `docs/NETWORK_DECODE_BENCH.md`.
- `scripts/board_regress.sh` had been silently skipping on RK3588 because its
  fixtures live in `~/rk-mpp-oracles/corpus`, not `/tmp`. Run against the real
  corpus it is now **7/7 byte-exact**. The `test720` reference in the corpus
  was a 25-frame clip of a 120-frame stream; it was regenerated with
  `ffmpeg -c:v h264` to `o720_ffmpeg_full.yuv`
  (SHA-256 `f31d2c0321ed82f6f2d4da5fed36bdce53ee355773879877de93138bf1500582`),
  which the Rust output matches exactly.

## 2026-11-12 VDPP2 register bring-up

- RK3588 `vdpp-probe` confirms client 29 is absent: hardware bit 29 is false,
  `INIT_CLIENT_TYPE` returns `EINVAL`, and neither register offsets nor polling
  are available.
- RK3576 VDPP2 parameter generation is now hardware-independent in
  `src/hal/vdpp2.rs`. It carries the vendor common register layout, ZME scale
  factors, all four 17x8 coefficient banks, DCI histogram words, and validated
  geometry limits.
- `scripts/board_vdpp2_regs_vs_c.sh` runs the unchanged C builder under a
  fake `mpp_service` ioctl shim on RK3588. Eleven cells pass: 720x480 to 1920x1080
  YUV444, 1920x1080 to 640x640 YUV420, 640x480 identity, histogram mode, and
  full default enhancement (DMSR/ES/sharp), a custom DMSR/ES profile and a
  custom SHARP profile, YUV420/YUV444 split Y/UV output and a custom ZME
  dering/blend profile, plus a custom 11x17x8 tap8/tap6 coefficient bank that
  replaces both vendor lookup tables. All compared register sections are
  identical.
- `src/service/vdpp2_session.rs` owns source/destination/optional chroma/histogram
  DMA-BUFs, submits the nine VDPP2 register sections plus address offsets, polls
  completion, and combines or exposes split Y/UV output. The packed DCI buffer is
  decoded into the vendor 4096-local-bin / 256-global-bin layout, and the U10
  mean-luma helper follows `get_avg_luma_from_hist()`.
- The targeted RK3588 library gate for VDPP2 is **21 passed / 0 failed / 0 ignored**
  after adding exact 9-byte local-bin packing, little-endian global bins, mean
  luma, truncated-input, legal histogram geometry, hardware-status, default
  DMSR/ES/SHARP golden-trace, ZME configurability, custom tap-bank selection,
  custom parameter-packing and split-output geometry tests. `vdpp-scale` and
  `vdpp-hist` have since passed on an RK3576 boot image that binds client 29.
- The VDPP2 hardware assets also compile on RK3588: the C probe builds against the
  2024-04 C source/library, the RK3576 overlay compiles to a 267-byte DTBO, and
  the independent packed-histogram checker passes synthetic
  `4096 local / 256 global / mean=511` data. The legacy fake-service matrix is
  **10 PASS / 1 documented SKIP**: 2024-04 C halves split-YUV420 chroma twice,
  while the current hardware target uses the newer source's supplied chroma
  dimensions directly. `board_vdpp2_hw_vs_c.sh` now validates both split420 and
  split444 against the newer C source on real hardware.
- RK3576 now boots the exact official SDK 250301 6.1.75 kernel with
  `CONFIG_ROCKCHIP_MPP_VDPP=y`. `/vdpp@27961000` remains `okay`,
  `mpp_vdpp 27961000.vdpp` probes successfully, `probe_hw_support` bit 29 is
  set, and client 29 binds. `CONFIG_OF_OVERLAY` remains unset but is no longer
  needed because the driver is built in.

### RK3576 kernel enablement investigation

- `mpp_vdpp` is a built-in `bool` driver in the Rockchip Kconfig, not a loadable
  module, and it calls non-exported MPP core symbols. A standalone
  `mpp_vdpp.ko` is therefore not a valid delivery path.
- The stock p3 is a 64 MiB externally stored U-Boot FIT with `fdt`, `kernel` and
  `resource` images. Its configuration declares
  `sha256,rsa2048:dev` / PSS signing, while the signature value is absent.
  The replacement builder therefore clones the existing FIT structure instead
  of synthesising a simpler one, and invokes `mkimage -E -p 0x800 -B 0x200`.
- The exact vendor source is in the Luckfox Omni3576 SDK 250301 manifest:
  `rk/kernel-stable`, commit
  `b16c5d1a69947ec66726165407a8aa75368def53`. It reports Linux 6.1.75,
  contains `rockchip,vdpp-rk3576`, and matches the board's compiler string exactly:
  `aarch64-none-linux-gnu-gcc 10.3.1 20210621`.
- `scripts/build_rk3576_vdpp2_kernel.sh` accepts that source and an alternate
  `CROSS_COMPILE`. It enables only `CONFIG_ROCKCHIP_MPP_VDPP`; the extracted
  Image config differs from the stock board config by exactly that one line.
  The original FDT/resource hashes, `rollback-index=0`, load/entry addresses and
  signature declaration are preserved.
- The accepted FIT is **43,024,896 bytes**, SHA-256
  `90649e7d6da5ea889c1c82ed43b135582e1e7d9c59c88e8977bc4938194888df`.
  Its Image is 41,069,056 bytes, SHA-256
  `84efdaed44fcea0f799125e94be1071be7b6418d84b32d39b06e5ff08d2f239a`,
  and vmlinux exports `rockchip_vdpp_driver`, `vdpp_probe` and `vdpp_remove`.
- `scripts/flash_rk3576_vdpp2_boot.sh` defaults to a dry run. Its applied path
  verifies kernel 6.1.75, VDPP DTB status, p3 capacity, the local image hash and
  passwordless sudo; backs up p3; writes with `dd conv=fsync`; reads the exact
  image span back and verifies SHA-256 before an optional reboot. Both dry run
  and applied readback passed for the accepted image.

### 2026-11-13 first RK3576 VDPP boot attempt

- With explicit authorization, the staged FIT was written to `mmcblk2p3`.
  The write completed in 0.39 seconds and readback matched SHA-256
  `afe90fcc8c5dbc4223321158021ee860e233b770ae8d6da5c37bcb86c548f37a`.
- The original p3 was backed up before writing as
  `target/rk3576-vdpp2/rk3576-p3-backup-20261113-014828.img`, 64 MiB, SHA-256
  `11ccf48e1bb04cdf21318d72ce29da57123c6c70d5139d2ca711d2a553317d30`.
- RK3576 rebooted but did not return on `192.0.2.148` within 180 seconds. A full
  `192.0.2.0/24` ICMP sweep found no board address, and no Rockchip USB loader
  device appeared on the host. The EasyTier `198.18.0.31` route accepts TCP
  locally but returns no SSH banner, so it cannot be used as a recovery shell.
- The first 6.1.75 `rk-6.1-rkr3` boot attempt is therefore **not bootable
  enough for hardware validation**. The board needs a physical power cycle
  and/or Loader/MaskROM or serial-console entry before restoring the saved p3.
- Recovery succeeded on 2026-11-13 through Rockchip Loader (`2207:350e`).
  `PartitionList` confirmed p3 at LBA `0x00008000` with `0x00020000` sectors.
  The 64 MiB backup was written with `WriteLBA`, reset with `ResetDevice`, and
  the board returned immediately on the original 6.1.75 kernel. The first
  16 MiB readback after restore matched the backup hash
  `f7f38f9602ed8ee8808d4a6a94c95633794b782c7f832c5cd902e8051453fb03`.
  `/sys/fs/pstore` was empty and the failed kernel did not create a journal
  boot, so no boot-stage fault log is available. The custom VDPP FIT is retained
  as rejected evidence; another write must not be attempted without an exact
  vendor source tree or serial-console capture.

### 2026-11-13 official SDK second boot and hardware acceptance

- The official Luckfox SDK 250301 `.repo` locks the exact vendor kernel source;
  the 1.5 GB expanded tree and SDK GCC 10.3.1 were staged through s2 on the same
  LAN as RK3588/RK3576. The accepted build completed on s2 in about two minutes.
- The 43,024,896-byte FIT was backed up, written and read back at SHA-256
  `90649e7d6da5ea889c1c82ed43b135582e1e7d9c59c88e8977bc4938194888df`,
  then rebooted. RK3576 returned as kernel 6.1.75 with a new boot ID.
- A second ordinary reboot also returned as kernel 6.1.75 with another new boot
  ID; client 29 and the complete 5/5 hardware gate passed again.
- Boot log: `mpp_vdpp 27961000.vdpp: probe device`, `probing finish`; sysfs binds
  the device to `mpp_vdpp`. `vdpp-probe` reports bit 29 present, client init
  binds, register offsets and poll supported, and `hw_id=536969267`.
- `board_vdpp2_hw_vs_c.sh` passes **5/5** against the new C source:
  scale-only and default enhancement are 345,600 bytes each; split YUV420 is
  230,400 luma + 115,200 chroma bytes; split YUV444 is 230,400 luma + 460,800
  chroma bytes; histogram is a 10,240-byte packed buffer parsing to 4,096 local
  and 256 global bins with U10 mean luma 513.
- Two hardware boundary bugs were found and fixed during this acceptance:
  split chroma rows are already compact 16-bit rows, so the output compactor must
  use `2 * dst_c_width_vir` bytes; and dma-heap rounds the histogram allocation,
  so the public packed histogram slice is truncated to the vendor's exact
  10,240-byte layout.

## 2026-11-12 H.264 / JPEG / post-processing closeout

- H.264 encode fixed-QP remains **24/24** over 64x64, 320x240, 640x480 and
  1280x720 x Baseline/Main/High x two QPs. CBR/VBR/AVBR, hierarchical QP and
  SMART_P matrices pass.
- `board_h264_reencode_vs_mpp.sh` passes **13/13** strategy cells: same-frame
  CBR/VBR/AVBR reencode, super-frame reencode, NORMAL drop in all three RC
  modes, CAVLC and CABAC PSKIP in all three RC modes. Each cell also compares
  every submitted job's STATUS window.
- H.264 GDR is **4/4** for row-4, col-4, row-8 and row-1 over 25 frames, 35 NAL
  units per cell. Stock C is nondeterministic here: its ROI table copies
  uninitialised `force_inter`/`mode_mask`/`reserved` stack bits into DMA, and
  three fresh runs produced `d4432259…`, `a49958ff…` and `f77af73e…`.
  `tools/mpp_roi_table_fix.c` zeroes only those undeclared bits at DMA sync;
  Rust emits the same deterministic table and then matches C byte-for-byte.
- H.264 ordinary slice splitting is **4/4** and low-delay packet output is
  **5/5**. The first-packet length differs by the documented version-SEI text
  delta; complete NAL streams and every following slice packet are identical.
- JPEG encoding passes **41/41** cells across restart, 90/270 rotation,
  RGB/YUV input, low-delay slices, four-band multicore and strided planes.
- Post-processing hardware tests pass **4/4**, including client-28 binding,
  two-frame I5O2, direct decoder DMA-BUF import, and a 50x34 H.264 surface with
  horizontal stride 64 and vertical stride 48. The padded import is compared
  against an explicit padded source and both destination slots are
  byte-identical.
- IEP2 full matrices remain **9/9** modes, **6/6** formats and **5/5** PD/DECT
  cells; telecine, progressive content and 322x242/722x482/1366x768 geometry
  all match the C probe's pixels and per-frame state.
- Final RK3588 gate: `board_workspace_gate.sh` reports **1317 passed / 0 failed /
  45 ignored** across 59 suites, `cargo fmt --check` is clean, and workspace
  rustdoc passes with `-D warnings`. `cargo test --release --lib --offline`
  reports **1147 passed / 0 failed / 0 ignored**.

## 2026-11-10 encoder packet metadata

- `scripts/board_mpp_packet_meta.sh` compiles the unchanged vendor C probes and
  runs the same binaries once with vendor MPP and once with
  `librk_mpp_rs.so`, both at the persistent 80 % OPP cap.
- H.264 fixed-QP and CBR are **2/2 packets** exact for both a still pair and a
  moving `testsrc2` pair. Stream bytes are identical with SEI disabled.
- H.265 fixed-QP and CBR are also **2/2 packets** exact. The checked fields are
  `lvl64/32/16/8_inter_num`, `lvl32/16/8/4_intra_num`, `KEY_ENC_SSE`,
  `KEY_OUTPUT_PSKIP`, `KEY_OUTPUT_INTRA`, `KEY_ENC_START_QP`,
  `KEY_ENC_AVERAGE_QP`, and `KEY_MOTION_INFO`.
- The H.265 caller-owned motion-info view is 640 bytes for this fixture and its
  per-frame FNV-1a hashes match vendor: `c41b276389c5a183` for the IDR and
  `106c00c09eaed691` for the moving P picture.
- JPEG is **1/1 packet** exact: vendor reports `start_qp=80`, `avg_qp=80` for
  q-factor 80 and no `KEY_OUTPUT_INTRA`; Rust now emits the same metadata and
  byte-identical JPEG. On RK3576 the VPU720 reference adds `intra=0`; the split
  formatter writes that field only for VPU720.
- The CBR matrix exposed H.265 rate-control metadata drift too. The strategy SEI
  must carry the caller's I band unchanged and `i_quality_delta=2`; the stale
  port emitted `[15:51]:4`. The fixed metadata, rate-controlled
  `cu_qp_delta_enabled=1` PPS and normalized stream now match C across
  320x240, 640x480 and 720p. The gate removes only the build-version SEI from
  each side and passes 13/13 cells.
- The current RK3588 crate `--all-targets` gate is **1279 passed / 0 failed /
  41 ignored**. `scripts/board_workspace_gate.sh`, which also runs the
  `rk-mpp-rs-sys` tests and strict workspace rustdoc, reports **1283 passed /
  0 failed / 41 ignored**. `cargo fmt --all --check` and `git diff --check`
  are clean.
- The RK3576 reduced gate is **1090 library tests passed / 0 failed** plus JPEG
  packet metadata **1/1**. VEPU510 H.264/H.265 fixed-QP moving and still
  matrices are both **2/2** per codec. Built-in CBR/VBR/AVBR are also connected:
  both codecs pass 2-frame metadata and 6-frame stream/YUV/rt_bits matrices.
  The reduced scope avoids LTO-linking 253 independent `--all-targets` test
  binaries one at a time under `CARGO_BUILD_JOBS=1`.
- `scripts/board_sdk_encoder_rc.sh` now passes all six V1.9 H.264/H.265 RC
  combinations on RK3576 against the vendor probe. H.264 CBR/VBR/AVBR are 9,426 /
  10,283 / 8,660 bytes; H.265 are 11,192 / 11,795 / 9,676 bytes, all
  byte-identical. The RK3588 VEPU580 command remains part of the same script.

## 2026-11-23 AV1 global-warped motion

The production AV1 parser was leaving `identity=true` set after reading a
non-identity ROTZOOM model. The hardware global-model buffer therefore erased
the warp and diverged from C/libaom on the second frame of
`av1_inter_global_warped_320x240.ivf`. Clearing the flag fixes the two-frame
output. `scripts/board_av1_global_warped.sh` compares C and Rust and pins
SHA-256 `ce1e081f…f6f5`.

The broader 8-bit AV1 matrix also found a `swreg5` field collision: preskip
segmentation was written to bit 21, which is `sw_delta_lf_present`, instead of
bit 20. Both delta-LF fixtures now match vendor C and libaom:
`55d42329…84d2` and `4fd081ed…da79`.

## 2026-11-11 AV1 software film-grain synthesis

- The independent M11 decoder now applies film grain only to displayed copies.
  `ReferenceFrameStore` keeps post-filter, pre-grain pixels and the frame's grain
  parameter set, so ordinary output and `show_existing_frame` use the historical
  frame's parameters without contaminating inter prediction.
- Stock `aomdec --output-bit-depth=10` is the oracle. The 8-bit stream passes
  **10/10 grain-applied frames**. The 10-bit stream passes **10/10 clean
  pre-grain frames** and **10/10 grain-applied frames**; all thirty frame
  projections are sample-exact after accounting for aomdec's 16-bit container
  alignment.
- The complete AV1 software-reference suite passes **359/359** on RK3588 at the
  persistent 80 % OPP cap.

## 2026-11-11 AV1 multi-size and 4:4:4 inter conformance

- Existing 96x96, 128x128 and 192x192 ten-frame inter fixtures now have locked
  libaom pixel hashes; each stream is **10/10** sample-exact.
- A new 320x240 profile-1 I444 key+P+P fixture matches stock `aomdec` on all
  three displayed YUV444 frames. It combines one-sided compound, inter-intra,
  OBMC, warped motion and nearby NEWMV candidates.
- The I444 stream exposed and fixed two context bugs: inter-intra admitted
  `BLOCK_8X32`/`BLOCK_32X8` when the normative range ends at `BLOCK_32X32`, and
  single-reference MV-stack scanning ignored NEWMV components in matching
  compound neighbours, producing mode context 67 instead of the reference 66.
- The complete AV1 software-reference suite is now **359/359**.

## 2026-11-11 AV1 one-column tile rows

- A new 320x320 key+P+P fixture is encoded with
  `tile_cols_log2=0` and `tile_rows_log2=1`; IntraBC, palette, CDEF, loop
  restoration and the loop filter are disabled to isolate the row-tile path.
- Both full-width row tiles restart their entropy state from the frame CDF,
  while inter-frame neighbour state follows the tile decode order. All three
  visible Y/U/V frames match stock `aomdec` sample-exactly.
- AV1 has no separate WPP flag or HEVC-style entry-point context chain. This
  tile-row shape closes the remaining AV1 item that had been tracked as WPP.

## 2026-11-11 AV1 screen-content IntraBC

- A default 320x320 screen-content key+P+P stream now exercises 20 IntraBC
  blocks and 4-color luma/chroma palettes. The reference decoder uses a
  separate `ndvc` CDF, validates integer DVs against the current tile, decodes
  the inter variable-transform tree and residual payload, then copies from the
  already reconstructed current-frame region.
- All three final Y/U/V frames match stock `aomdec` sample-exactly. The
  complete AV1 software-reference suite is **357/357**.

## 2026-11-11 AV1 B-pyramid

- A 20-frame 320x240 reordered B-pyramid is encoded with
  `--lag-in-frames=19 --auto-alt-ref=1`. It exercises hidden showable frames,
  reference-slot refreshes, order-hint reordering and repeated
  `show_existing_frame` across multiple slots.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly. The
  complete AV1 software-reference suite is **358/358**.

## 2026-11-11 AV1 advanced B-pyramid

- The same 20-frame reordered schedule is encoded with global/warped motion,
  OBMC, inter-intra, masked/distance-weighted compound and reference-frame MVs
  enabled. Multiple coded frames reference at least three non-zero order-hint
  slots.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly. The
  complete AV1 software-reference suite is **359/359**.

## 2026-11-11 AV1 ten-bit B-pyramid

- A 12-frame 320x240 10-bit `testsrc2` B-pyramid is encoded with
  `--lag-in-frames=11 --auto-alt-ref=1`. All 12 coded frames are showable, the
  order-hint history reaches seven non-zero slots while every coded frame uses
  single-reference prediction. The stream combines 10-bit inter prediction,
  residual reconstruction and reference-slot rotation.
- The Rust dump is sample-exact to stock `aomdec` for all 12 displayed Y/U/V
  frames: 2,764,800 bytes with no differing byte. The complete AV1
  software-reference suite is now **360/360**.

## 2026-11-11 AV1 advanced ten-bit B-pyramid

- A 20-frame 320x240 10-bit B-pyramid keeps hidden showable frames,
  `show_existing_frame`, global/warped motion, OBMC, inter-intra,
  masked/distance-weighted compound and reference-frame MVs enabled together.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly:
  4,608,000 bytes with no differing byte. The complete AV1 software-reference
  suite is now **361/361**.

## 2026-11-11 AV1 ten-bit B-pyramid deblock and CDEF

- The same 20-frame advanced 10-bit B schedule enables non-zero deblocking on
  every inter coded frame and enables CDEF for the sequence. Loop restoration
  remains disabled and is not counted as covered by this gate.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly:
  4,608,000 bytes with no differing byte. The complete AV1 software-reference
  suite is now **362/362**.

## 2026-11-11 AV1 ten-bit 4:4:4 advanced B-pyramid

- A 20-frame I444P10 advanced B stream retains hidden showable frames,
  `show_existing_frame`, compound prediction and the advanced inter feature
  set with no chroma subsampling.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly:
  9,216,000 bytes with no differing byte. The complete AV1 software-reference
  suite is now **363/363**.

## 2026-11-11 AV1 ten-bit B-pyramid with inter loop restoration

- A 20-frame advanced 10-bit B stream encoded with `--sb-size=64
  --cpu-used=0 --cq-level=20` contains a real inter frame with `uses_lr=true`
  alongside CDEF and reference reordering.
- The first run failed at coded frame 5 with `ExitSymbol/PaddingNotZero`.
  Comparing libaom and Rust narrowed it to `NEW_NEARMV`: the NEW component must
  use reference index `1 + ref_mv_idx`. The wrong MV had polluted temporal MV
  projection. After the fix, all 20 displayed Y/U/V frames match stock `aomdec`
  sample-exactly: 4,608,000 bytes with no differing byte. The complete AV1
  software-reference suite is now **365/365**.

## 2026-11-11 H.264 CABAC P-slice P_Skip

- A new 48x32 Main-profile stream decodes one CABAC IDR followed by a P picture
  whose six macroblocks are all skip, as confirmed by x264's `mb P ... skip:
  100.0%` report. The P slice uses `cabac_init_idc=0` and
  `first_mb_in_slice=0`; deblocking and weighted prediction are disabled to
  isolate CABAC P syntax.
- The focused integration gate
  `cargo test --release --test h264_reference_i32
  cabac_p_skip_copies_reference_macroblocks_matches_ffmpeg_yuv420p` passes. The
  test reconstructs the IDR first, decodes every `mb_skip_flag` in the P slice,
  copies reference picture 0 and compares both complete 4:2:0 frames. All
  4,608 bytes match FFmpeg.
- The first implementation used `decoded` instead of `decoded && !skip` for the
  skip-neighbour context terms, which moved from ctxIdx 11 to 12/13 and ended
  the slice after two macroblocks. The real fixture reduced that failure to a
  deterministic two-macroblock result; the corrected terms decode all six.
- At the persistent 80% OPP cap, `scripts/board_workspace_gate.sh` reports
  **1267 passed / 0 failed / 41 ignored**, and `cargo test --release --lib
  --offline` reports **1117 passed / 0 failed / 0 ignored**. Fixture manifest,
  rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC zero-motion P16

- A second 48x32 Main-profile stream is locked to a 100% I16 IDR followed by a
  P picture containing six `P_L0_16x16` macroblocks. The P slice has zero MVD,
  CBP 15/0, no deblocking, no weighted prediction and `cabac_init_idc=0`.
- The focused gate
  `cargo test --test h264_reference_i32
  cabac_p16_zero_mv_residuals_match_ffmpeg_yuv420p` passes. It consumes
  P mb_type, MVD, CBP, macroblock QP, every luma 4x4 inter residual, performs
  zero-motion reference copy plus residual reconstruction and compares both
  complete 4:2:0 frames. All 4,608 bytes match FFmpeg.
- A temporary x264 trace exposed the defect: inter CBF neighbours follow the
  I-slice rule in reverse for unavailable macroblocks. Unavailable inter
  neighbours contribute zero, while the intra path contributes a non-zero
  context flag. The first P16 block therefore moved from context 93 to 96 and
  desynchronised the third macroblock. Separate inter CBF context helpers now
  preserve both rules.
- At the persistent 80% OPP cap, `scripts/board_workspace_gate.sh` reports
  **1268 passed / 0 failed / 41 ignored**, and `cargo test --release --lib
  --offline` reports **1117 passed / 0 failed / 0 ignored**. Fixture manifest,
  rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC integer-motion P16

- A random-texture 48x32 source is cropped at x=0 and x=8. A temporary x264
  trace build disables P-skip and all P sub-partitions/intra modes, then forces
  each `P_L0_16x16` cache MV to `(+32, 0)` quarter-pel so the encoder computes
  residuals against that exact vector. The P frame contains six P16
  macroblocks with final MVs `(+32, 0)`: luma moves +8 pixels and 4:2:0 chroma
  moves +4 pixels.
- The focused gate `cabac_p16_integer_motion_matches_ffmpeg_yuv420p` passes. It
  covers non-zero MVD syntax and sign, P16 MVP neighbour selection, integer
  luma/chroma motion compensation, reference-edge extension and residual
  reconstruction. The test supplies FFmpeg's IDR frame as reference picture 0;
  the 2,304-byte P frame matches FFmpeg sample-exactly.
- The first 8-pixel attempt exposed a reversed MVD bypass sign: x264 declared
  `[-4,-8]` while Rust decoded `[+4,+8]`. The zero-MV gate could not observe
  this. After fixing the sign, forced `+32` vectors decode consistently and all
  six prediction offsets are non-zero.
- Fractional luma/chroma vectors, P_16x8/P_8x16/P_8x8, weighted prediction,
  deblocking and B-slice CABAC remain open.
- At the persistent 80% OPP cap, `scripts/board_workspace_gate.sh` reports
  **1269 passed / 0 failed / 41 ignored**, and `cargo test --release --lib
  --offline` reports **1117 passed / 0 failed / 0 ignored**. Fixture manifest,
  rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC fractional-motion P16

- The same 48x32 random-texture crop pair is encoded with normal subpel search.
  The temporary x264 build still forbids P-skip, intra modes and P
  sub-partitions, so the P frame remains six P16 macroblocks while their final
  vectors become `(-6,-9)`, `(8,1)`, `(5,1)`, `(-5,-6)` and `(7,1)`.
- The decoder implements normative quarter-pel luma interpolation for every
  horizontal, vertical and diagonal fractional combination, including the
  clipped six-tap half samples and separate `+512 >> 10` two-dimensional
  rounding. 4:2:0 chroma uses all 64 bilinear weights for its 1/8-sample
  positions.
- The first attempt exposed another H.264 state bug: the previous macroblock's
  MVD neighbour cache must store absolute magnitudes for context selection while
  the signed delta remains available for addition to the MVP. Storing only the
  signed value desynchronised the stream at macroblock 2.
- The 2,304-byte P frame matches FFmpeg sample-exactly. Fractional P16 vectors
  no longer fail; P_16x8/P_8x16/P_8x8, weighted prediction, deblocking and
  B-slice CABAC remain open.
- At the persistent 80% OPP cap, `scripts/board_workspace_gate.sh` reports
  **1270 passed / 0 failed / 41 ignored**, and `cargo test --release --lib
  --offline` reports **1117 passed / 0 failed / 0 ignored**. Fixture manifest,
  rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC partitioned-motion P16x8 / P8x16

- Two new 48x32 Main-profile fixtures force every P macroblock to
  `P_L0_L0_16x8` or `P_L0_L0_8x16`. x264 reports 100% of the selected
  partition and no skip; the temporary trace build exposes no intra
  macroblocks. The 16x8 stream SHA-256 is
  `4645e307b3858f9550b6cfe9d605c535d68462dca03c485295f9295f897b4c25`,
  YUV `ba4c46a50445a9a8f28c49b9cbdacd2cfd01922d7d7fb96cbcc71fc5a99f99e3`;
  the 8x16 stream is
  `0dc9850b597f1ebeb797603f0d30be9d067d69345ddd73573a42ca95ebd0cb58`,
  YUV `805cafd67d7e20345f2af1fe0e676ff24644b149c4ea499c3b2cd966cf79a47f`.
- The decoder consumes both partition MVDs and implements the normative
  A/B/C MVP and MVD context positions from partition scan order. 16x8 vectors
  include `(-32,0)`, `(16,0)`, `(32,0)`, `(-37,0)` and `(6,2)`; 8x16 vectors
  include `(-28,0)`, `(16,0)`, `(-2,17)`, `(-32,0)` and `(28,0)`. Each
  partition gets independent luma/chroma motion compensation, followed by one
  shared macroblock residual reconstruction.
- The pixel gate exposed a missing quarter-pel `(2,2)` branch in the luma
  interpolation dispatch, which fell back to the integer sample. Adding the
  normative two-dimensional half-sample path makes both complete P frames
  match FFmpeg sample-exactly.
- The H.264 CABAC integration suite is **6/6**, the focused CABAC unit suite
  is **7/7**, the crate `--all-targets` gate is **1270 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1274 passed / 0 failed
  / 41 ignored**, and release-library tests are **1119 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- `P_8x8`, weighted prediction, deblocking and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC P_8x8 sub-macroblock partitions

- A new mixed 48x32 Main-profile stream enables P_8x8 and sub-8x8 analysis.
  The P picture contains 16x16, 8x16 and P_8x8 macroblocks; across its P_8x8
  quadrants all four `sub_mb_type` modes are present (`8x8`, `8x4`, `4x8`,
  `4x4`). Stream SHA-256 is
  `5e61cfc82df0f3c4452603bca054bc48d364b32077ff684bd9a9b6bfa296292a`,
  decoded YUV is
  `0d5542e83935c20dfd52efb9075eb4a02cb0d3a2e88a5a3cc1bbb6af973d833f`.
- The decoder consumes the P_8x8 mb_type suffix and each one-/two-/three-bin
  subpartition tree, expands every quadrant into its normative 8x8/8x4/4x8/4x4
  scan order, maintains MVD context and motion vectors in the 4x4 grid, then
  runs independent luma/chroma motion compensation and one shared macroblock
  residual reconstruction. All 4,608 decoded bytes match FFmpeg exactly.
- The H.264 CABAC integration suite is **7/7**, the focused CABAC unit suite is
  **8/8**, the crate `--all-targets` gate is **1272 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1276 passed / 0 failed
  / 41 ignored**, and release-library tests are **1120 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Weighted/deblock P pictures and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC explicit weighted P_L0_16x16

- A new two-frame 48x32 Main-profile stream forces explicit weighted
  prediction with luma scale/offset `3/8` at denominator 2 and U/V `7/4` at
  denominator 3. x264 reports weighted P frames for Y and UV at 100%; P-skip,
  P subpartitions, intra P and transform-8x8 stay disabled. Stream SHA-256 is
  `2f15b4d68909169af744847e567077587abf42c6841211afb08b9b1641404eb9`,
  decoded YUV is
  `346c0689eaa1a4a01fc6cded3b80c5631b206ff24aab4e1d1c8dad9e75ab3502`.
- The slice header ends at bit 71, exposing an existing alignment bug: the
  parser's `+9` CABAC rounding skipped one extra byte and started slice data
  at bit 80. It now rounds to the next byte boundary at bit 72, matching x264
  and FFmpeg.
- Explicit weights are applied after luma/chroma interpolation with the
  normative rounded scale, bit-depth-scaled offset and clipping, before the
  shared residual is added. The fixture exercises all three planes. All 4,608
  decoded bytes match FFmpeg. Weighted P_Skip uses the same sample transform
  but still needs a dedicated fixture.
- The H.264 CABAC integration suite is **8/8**, the focused CABAC unit suite is
  **9/9**, the crate `--all-targets` gate is **1274 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1278 passed / 0 failed
  / 41 ignored**, and release-library tests are **1121 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Weighted P_Skip, P-frame deblocking and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC P-frame deblocking

- A third 48x32 P16 fixture enables loop filtering with slice
  `alpha/beta=2/1`. It combines coded chroma edges with uncoded luma edges
  whose boundary strength is selected from neighboring 4x4 quarter-pel MVs.
  Stream SHA-256 is
  `985990b8b2578ebaa6665d971b11d447259fd5fde41f61cc722655d51dd2fa99`,
  decoded YUV is
  `f26b6364970c12acdac5fd92fe348d11092e4588305fcf1c7b94412eebeeec07`.
- `DeblockMap` now carries P-slice 4x4 MVs. P boundary strength is 3 when
  either side has coefficients, 2 for a component delta of at least four
  quarter-pels, 1 for any smaller non-zero MV delta and 0 for equal motion.
  Luma and chroma traverse vertical before horizontal edges, average QP at
  each boundary and use the existing normative filters.
- The complete deblocked P frame matches FFmpeg sample-exactly. The H.264
  CABAC integration suite is **9/9**, CABAC unit tests are **9/9**, deblock
  unit tests are **8/8**, the crate `--all-targets` gate is **1276 passed / 0
  failed / 41 ignored**, `scripts/board_workspace_gate.sh` is **1280 passed /
  0 failed / 41 ignored**, and release-library tests are **1122 passed /
  0 failed / 0 ignored**. Fixture manifest, rustfmt and strict workspace
  rustdoc pass.
- Weighted P_Skip and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC two-reference P16

- A three-frame 48x32 Main-profile stream activates two L0 references. The
  final P picture deliberately mixes the recent P and older IDR and selects
  per-macroblock references `1,0,0,0,1,0`. Stream SHA-256 is
  `091920b51cc69c454fc4fc0f27a4422281bdddd36f7df91242e7b809f63599af`,
  decoded YUV is
  `06c942f2c21525ac0415f67165dbb61651b4fb85d30154e37f6eb58469d3d53b`.
- The multi-reference decoder accepts an arbitrary active L0 list and carries
  reference indices in the 4x4 motion cache. It consumes CABAC ref_idx unary
  trees with left/top neighbor contexts, applies reference-specific
  luma/chroma motion compensation and selects the matching weighted table.
- Reference-aware MVP reproduces the x264 fallback rule: if B and C are
  unavailable, A supplies the predictor even when it points at a different
  reference. This was the missing rule behind the first 1,847-sample
  divergence; after the fix the complete 4,608-byte P frame is exact.
- The H.264 CABAC integration suite is **10/10**, CABAC unit tests are
  **10/10**, deblock unit tests are **8/8**, the crate `--all-targets` gate is
  **1278 passed / 0 failed / 41 ignored**, `scripts/board_workspace_gate.sh`
  is **1282 passed / 0 failed / 41 ignored**, and release-library tests are
  **1123 passed / 0 failed / 0 ignored**. Fixture manifest, rustfmt and strict
  workspace rustdoc pass.
- Weighted P_Skip and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC B_L0_16x16

- A new IDR+P+B stream is encoded in coding order with direct mode, B-skip, B
  subpartitions, weighted bi-prediction and deblocking disabled. A temporary
  x264 switch forces all six B macroblocks to `B_L0_16x16`; x264 reports B16
  L0 100%. Stream SHA-256 is
  `ab378bde3bdddd8b3f89cc583cc7a37c9757c69e74ef82a9f4b46f830daad822`,
  decoded YUV is
  `09e707f462e533531272be1c9c8a70cbc680644dc406defc28ccc0452c1521f7`.
- The decoder consumes B skip bins in contexts 24-26, the B type predicates in
  contexts 27-30/32, list-0 MVD and the shared inter CBP/residual tree. It
  reconstructs each macroblock from the displayed I reference through
  forward-only motion compensation. The complete displayed 48x32 B frame
  matches FFmpeg sample-exactly.
- The focused CABAC integration suite is **25/25**, CABAC unit tests are
  **10/10**, deblock unit tests are **8/8**, the crate `--all-targets` gate is
  **1279 passed / 0 failed / 41 ignored**, `scripts/board_workspace_gate.sh`
  is **1283 passed / 0 failed / 41 ignored**, and release-library tests are
  **1123 passed / 0 failed / 0 ignored**. Fixture manifest, rustfmt and strict
  workspace rustdoc pass.
- B_Direct, B_L1, B_BI, B subpartitions, weighted B and B deblocking remain
  open.

## 2026-11-12 H.264 CABAC B_L1 and B_BI 16x16

- Two new IDR+P+B streams force all six B macroblocks to `B_L1_16x16` and
  `B_BI_16x16`. The list-1 stream SHA-256 is
  `103ed3ccdf345a4b9c2e43888c44b4c5bc34fe871ff34b1c136c580b525a7ca8`,
  its decoded YUV is
  `b0e3c81cf03b889d05869db67563cba3b5e829e8a576219eaf7d4e9a6aa31cd9`,
  the bi-predicted stream is
  `e94f08bae6653b0af8a86e4d38981b45286e77c439cba2b25134f6aa868e296f`,
  and its decoded YUV is
  `e0927b98b0ae12b2857c5bdb66a8d667f78328ca65274812e03633a1974d0dc9`.
- The decoder retains independent L0/L1 4x4 motion and reference caches,
  consumes list-0 then list-1 MVD syntax when both are present, and blends the
  temporary predictions with `(L0 + L1 + 1) >> 1`. Both complete displayed
  48x32 B frames match FFmpeg sample-exactly.
- The B16 focused gate is **3/3**, the complete CABAC integration suite is
  **27/27**, CABAC unit tests are **10/10**, and deblock unit tests are
  **8/8**. The crate `--all-targets` gate is **1281 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1285 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- B_Direct/B_Skip, B 16x8/8x16/8x8, weighted B and B deblocking remain open.

## 2026-11-12 H.264 CABAC spatial B_Direct and B_Skip

- A new IDR+P+B stream enables spatial direct and mixes one BI macroblock with
  three `B_Direct_16x16` and two `B_Skip` macroblocks. x264 reports direct
  50%, skip 33.3% and BI 16.7%. Stream SHA-256 is
  `e4a0a2177f4b81d41b37af504f3746bcf0ad28e8720f05178f85feeb475d60a0`,
  decoded YUV is
  `5745621d236e55f3fce23d96a1bf8416ad09f887fc7667b1f00bc8b594b10306`.
- Skip and type contexts now use different neighbor predicates: `B_Skip`
  counts as skipped, while direct still counts as non-skip for the skip
  context but is excluded from the type context. Spatial direct derives L0/L1
  motion from the A/B/C neighbors, performs separate list prediction and
  applies the normative BI average. Coded direct consumes the inter residual
  tree; `B_Skip` consumes neither residual nor macroblock-QP syntax.
- The displayed 48x32 B frame matches FFmpeg sample-exactly. Temporal direct,
  B 16x8/8x16/8x8, weighted B and B deblocking remain open.
- The B16 focused gate is **4/4**, the complete CABAC integration suite is
  **28/28**, the crate `--all-targets` gate is **1282 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1286 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC temporal B_Direct and B_Skip

- A second IDR+P+B stream keeps the same BI/direct/skip mix but selects
  temporal direct. The coded picture POCs are 0/4/2. The test decodes the P
  picture first and supplies its 16x16 motion partitions through the reference
  frame metadata.
- Direct motion uses the normative distance scale, here 128, to project the
  colocated P list-0 vector into the B picture's L0 vector; the L1 vector is
  the projected vector minus the colocated vector. Both lists then use the
  existing fractional luma/chroma motion compensation and BI blend.
- Stream SHA-256 is
  `b7bd26b4b60abe4843d72638f4b7809e183289fede92d346352f6df559ab5932`,
  decoded YUV is
  `68c2443dbf2eece268399d7f057f3598c0e417b9786d9a5b0ae6cad256c176ef`,
  and the displayed 48x32 frame matches FFmpeg sample-exactly.
- Col-zero B references, colocated non-16x16 partitions, weighted B and B
  deblocking remain open.
- The B16 focused gate is **5/5**, the complete CABAC integration suite is
  **29/29**, the crate `--all-targets` gate is **1283 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1287 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC B_L0_L0 16x8 and 8x16

- Two additional IDR+P+B streams force every B macroblock to
  `B_L0_L0_16x8` and `B_L0_L0_8x16`; x264 reports B 16x8 100% and B 8x16
  100% respectively. The 16x8 stream SHA-256 is
  `9bea7354638a56f55b0de2ea52afdb907fdf99260e04a5b17bd2006ea2aa7233`
  with YUV
  `9c72387cc041a2379b432d1f2777127506c9d33c40f18fd84af8356149348c0a`.
  The 8x16 stream is
  `0e140e59a0b8b08ecfb42e67718e867fd18234a2d92beb67b1066582e9cd37e8`
  with YUV
  `4f6b51430c4f76392dadc2fdb924c03b7c328609b4da71c16a6a1c23d79033b3`.
- The B mb_type parser now consumes the normative four-bit CABAC prefix and
  maps bits 1/2 to L0 16x8/8x16. Both partitions decode MVDs in scan order,
  update separate 4x4 motion cache regions, run independent luma/chroma motion
  compensation and share one macroblock residual tree. Both displayed frames
  match FFmpeg sample-exactly.
- The B16 focused gate is **7/7**, the complete CABAC integration suite is
  **31/31**, the crate `--all-targets` gate is **1285 passed / 0 failed /
  41 ignored**,
  `scripts/board_workspace_gate.sh` is **1289 passed / 0 failed / 41 ignored**,
  and release-library tests are **1123 passed / 0 failed / 0 ignored**.
  Fixture manifest, rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC B_8x8 L0 subpartitions

- A third IDR+P+B stream forces every B macroblock to `B_8x8` and all four
  sub_mb_types to `B_L0_8x8`; x264 reports B_8x8 100%. Stream SHA-256 is
  `82a8ff29c22e973f21c38a129d6b2b09aa9f03562b080f94607ac696c733e71a`
  and decoded YUV is
  `cd45314da12e3987d3bd3f5ddbb1be420cd8fddd20b4f61f9173e2dc2d925a76`.
- The decoder now accepts B mb_type CABAC bits 15, consumes four
  sub_mb_type decisions in contexts 36-39, decodes four L0 MVDs in 8x8 scan
  order, updates each 4x4 motion-cache quadrant independently and applies one
  shared macroblock residual tree. The complete displayed frame matches
  FFmpeg sample-exactly.
- The B16 focused gate is **8/8**, the complete CABAC integration suite is
  **32/32**, the crate `--all-targets` gate is **1286 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1290 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Direct/L1/BI subpartitions, mixed 8x4/4x8/4x4 shapes, col-zero B references
  and weighted/deblock B remain open.

## 2026-11-12 H.264 CABAC B_8x8 L1 and BI subpartitions

- Two more IDR+P+B streams keep mb_type `B_8x8` but force all four
  sub_mb_types to `B_L1_8x8` and `B_BI_8x8`; x264 reports L1 100% and BI
  100%. The L1 stream SHA-256 is
  `f92fde6bf8c020589376cc2701b245c0d71fd5c4e62c29857704b898f76eeeca`
  with YUV
  `b0fdb0307b0996bf93ebb051ff3a2798445e242f5229a59a975b7ffaa5196611`.
  The BI stream is
  `a6b7a9dbcffbdbe5b3ff5555e872c559800f3e2a800e1f1b24f338c38c3d8b2e`
  with YUV
  `52acffe0637ac105fa8790038230d8e63540f5009ed12cffb3dec3e6ba5d0161`.
- The B_8x8 parser now accepts sub_mb_type paths for L0, L1 and BI 8x8.
  List-0 MVDs are consumed for all four quadrants before list-1, each list
  updates its own 4x4 motion cache, and BI builds separate 8x8 luma/chroma
  predictions before the normative rounded average. Both displayed frames
  match FFmpeg sample-exactly.
- The B16 focused gate is **10/10**, the complete CABAC integration suite is
  **34/34**, the crate `--all-targets` gate is **1288 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1292 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Direct 8x8, 8x4/4x8/4x4, col-zero B references, weighted B and B deblocking
  remain open.

## 2026-11-12 H.264 CABAC mixed L0 B_8x8 geometry

- One IDR+P+B stream keeps `B_8x8` and assigns the four quadrants to
  `B_L0_8x4`, `B_L0_4x8`, `B_L0_4x4` and `B_L0_8x8`. Stream SHA-256 is
  `9d9cc8b58a686d2065c8c183ba0f73bdb7d2ef7a030a300ec2a18683e436225d`
  and decoded YUV is
  `cd45314da12e3987d3bd3f5ddbb1be420cd8fddd20b4f61f9173e2dc2d925a76`.
- The decoder consumes the complete CABAC sub_mb_type tree and the matching
  2/2/4/1 MVD sequence in quadrant and sub-partition scan order. Every leaf
  updates its 4x4 cache cells and receives independent luma/chroma motion
  compensation before the shared residual tree. The displayed frame matches
  FFmpeg sample-exactly.
- The B16 focused gate is **11/11**, the complete CABAC integration suite is
  **35/35**, the crate `--all-targets` gate is **1289 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1293 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Direct 8x8, L1/BI non-8x8 geometry, col-zero B references, weighted B and
  B deblocking remain open.

## 2026-11-08 W5-4 pool closeout

- H.265 lease test passed; the HEVC board suite is **13 PASS / 1 SKIP /
  0 FAIL**. The SKIP remains the named `/tmp/t720.hevc` C-reference absence.
- AV1 lease test passed, 353 AV1 library tests passed, and the 8/10-bit film
  grain hardware output matched libaom.
- VP9 lease test passed, 18 VP9 library tests passed, and all three hardware
  matrices (show-existing, error-resilient segmentation, lossless/tiled/odd
  sizes) passed.

## 2026-11-08 RK3576 / RK3576 H.264 bring-up

- Board: Luckfox Omni3576, RK3576, Linux 6.1.75, C MPP 1.0.11.
- 640×480 High profile, 12 moving frames: Rust VDPU383 and stock C MPP both
  SHA-256
  `615ee129c46e10f7fb4cb2cf8af3cb370523f7605abee7e7315e073a4421b112`.
- VDPU383 `global_cfg` and `rps` for the first two frames match the patched C
  dumps word-for-word.
- RK3588 RK3588 fallback from the same branch: all seven H.264 decoder fixtures
  passed, proving the VDPU34x path remains intact.
- The RK3576 H.264 edge matrix is **5/5** on RK3576: 720p/B 30 frames, 1080p
  hard 90 frames, PAFF 15 frames, LTR 62 frames and a 61-AU missing-reference
  stream producing 22 output frames. The first, second, LTR and missing-
  reference rows are byte-identical to C MPP; PAFF matches FFmpeg, with the C
  harness retaining its known field-order difference.

## 2026-11-08 RK3576 / RK3576 H.265 bring-up

- The VDPU383 hardware matrix is **12/12** on RK3576: 320x240 single IDR, 640x480
  IPPP and B-pyramid, Main10, 2x2 tiles, real WPP at 640x360 and WPP at
  1280x720, JCT-VC DSLICE, explicit SPS/PPS scaling-list streams, a 500-frame
  LTR SPS stream, and a missing-reference stream. Every planar YUV output
  matches stock C MPP byte-for-byte.
- The WPP streams are verified by `trace_headers` with
  `entropy_coding_sync_enabled_flag = 1` and five entry points. The 1280x720
  C/Rust output SHA-256 is
  `5f124bd7c91877106abe010904de7eeeb129d17a09ddde1d0cc06818bbd3b156`.
- JCT-VC DSLICE (50 frames at 1920x1080) hashes to
  `0f28bd89c091b103b3fe3722fa9e8435d96eee414a6c33c29658b4728ec62bc7`.
  Explicit SPS/PPS scaling-list streams hash to
  `ecdaee4973148a40faa39e2dafc322507c5cc2d53e33a02fb08537e7a53668ec`
  and `66baaeb67679a189873f22fb3752caa368d598b72b94d07828f0077a3969e4d0`.
- The 176-byte VDPU383 SPS/PPS/POC packet matches the patched C dump byte-for-
  byte; its offline FNV lock is `0xe97a26406128e428`. RCB offset/length pairs,
  zero CABAC base and `global_len=0xc` are covered by the same path.
- The scaling-list table is 1360 bytes per slot and uses `sps_id * 1360` or
  `(pps_id + 16) * 1360`; the VDPU383 builder stores the six 32x32 matrices in
  the source-defined block-column-major order.
- JCT-VC `LTRPSPS_A_Qualcomm_1.bit` is 416x240 and 500 frames. Its SPS declares
  eight long-term entries and slices alternate `lt_idx_sps=4/7`; C and Rust
  both emit 500 frames with SHA-256
  `e0d7db1516ed31d3046c63c33240ec0866150774eb74b5e88f67bc186ba51bee`.
- The missing-reference row removes the sixth AUD-delimited access unit from a
  12-picture x265 stream. C reports `Could not find ref with POC 5`; C and Rust
  both emit five frames with SHA-256
  `7d67c91340892c5de09e45db43fc160f3991b8f950e30c7d2b0ff5590626a74d`.
- The separate VEPU510 H.265 encode matrix is **15/15**. C's `AUTO_TILE=1`
  produces the same 4710-byte IDR as its ordinary path on RK3576; Rust rejects
  a two-column request before DMA or hardware allocation. Tiles are therefore
  a platform boundary rather than an unimplemented register path.

## 2026-11-08 RK3576 / RK3576 VDPU383 AV1 bring-up

- The official C HAL was built with `scripts/av1_vdpu383_dump.patch`; it dumped
  thirteen 672-byte global headers for the ten displayed frames of
  `av1_tiles_320x240_10.ivf`.
- `src/hal/av1d383.rs` reproduces `vdpu38x_av1d_uncomp_hdr()` including the
  C `mpp_put_bits` little-endian 64-bit packing, 5160-bit payload, 128-bit
  alignment and the 16-byte trailing reservation.
- The first keyframe is byte-identical to C: SHA-256
  `5dc26be2b7f1d2a76d135b64435c737cd62d8b6c2ab720a0fb83c83efddfbf56`
  and FNV `0x8da389d2e180d680`.
- The same keyframe's 212-word serialized `Vdpu383RegSet` is also byte-exact:
  control/statistic words, current/error/global/output/CDF/colmv addresses,
  tile strides and all eleven RCB base/length pairs match the C dump.
- The VDPU383 default CDF image is generated from the official 7400-entry u32
  table: 29,600 bytes, SHA-256
  `43474cb59fa76fa1ed95cbfb2fb46952b061e948a13581c04380e51d099c100b`.
  For 320x240, the planned working CDF is 32,672 bytes, segmentation 3,072,
  colmv 20,480 and RCB 72,448.
- `hal::av1d383::Session` submits a real keyframe through the RkvDec client
  with system DMA, CDF/RCB offsets and the 448-byte C output stride.
- Stock C MPP and Rust both emit 115,200-byte 320x240 YUV420p with SHA-256
  `6f5b544f53584c7de6d3acad5f4611ca2ee3d75ae259ab981ffca5e599b1b9fe`.
  The explicit gate is `scripts/board_rk3576_av1_vdpu383_key.sh`.
- The complete fixture now decodes all ten visible frames through VDPU383.
  The Rust sequence models persistent output, colmv and working-CDF buffers for
  all eight reference slots, including hidden pictures and
  `show_existing_frame`. Its primary-reference coefficient-CDF state is
  inherited exactly like C; the older current-frame `base_q_idx` shortcut
  caused the first state divergence on the fourth coded job.
- C and Rust both emit 1,152,000 bytes of 320x240 YUV420p. Frame-by-frame and
  whole-sequence comparison is byte-identical, with SHA-256
  `9d339ae3c5b270ac9059548da0d3ff5ef6cb0c94a9bcb11f5bfc93783e868138`.
  The explicit full-sequence gate is
  `scripts/board_rk3576_av1_vdpu383_sequence.sh`.
- VDPU383 10-bit output is now closed for the current C-compatible path.
  The 320x240 surface uses MPP's 704-byte packed stride and a 337,920-byte
  allocation; Rust converts it to planar little-endian u16 Y/U/V. All ten
  displayed frames total 2,304,000 bytes with SHA-256
  `f3b69b61a06d3967a89b91f11f56883588df53974d4a1b5efb8fbd9e004d49b8`,
  byte-identical to C's skip-grain output. The explicit gate is
  `scripts/board_rk3576_av1_vdpu383_10bit.sh`.
- VDPU383 FGS is a separate unresolved hardware boundary. C's VDPU383 HAL
  leaves `av1_fgs_en` and `reg134_fgs_base` clear. Enabling either raster or
  tile FGS in Rust makes the job complete without STATUS (`0x00000000`) and
  forces a decoder reset; keep the verified skip-grain path enabled until a
  register/trace oracle exists for this block.

- RK3576 H.265 now includes a 16-cell deterministic corruption matrix:
  SPS/PPS/SEI flips, start/middle/end flips in the IDR and selected P slices,
  and three final-AU truncation lengths. Fourteen cells have identical C/Rust
  output counts and byte-identical planar YUV when frames remain visible.
  A damaged SPS and a flip at the first IDR slice payload make C's
  `mpi_dec_test` wait until timeout; Rust returns promptly with zero output.
  The reproducible gate is
  `scripts/board_rk3576_h265_corrupt_vs_mpp.sh`.

## 2026-11-08 RK3576 / RK3576 VP9 bring-up

- The VDPU383 VP9 matrix is **7/7**: profile 0/2 64x64, 10-bit 64x64,
  lossless 320x240, 10-bit lossless 320x240, error-resilient tiles at
  640x360, and the two adjudicated C-defect rows at 720p and 321x241.
- Regular rows match stock C MPP byte-for-byte. The 720p segmentation and
  321x241 rows match libvpx: C differs on the former and emits one fewer
  frame on the latter.
- VDPU383 uses a separate 168-byte global header, 1312-byte key-frame probe
  FIFO, eleven refined RCB regions and uncompressed `width * height / 2`
  colmv. Tile-width fields are CTU counts. The new
  `scripts/board_rk3576_vp9_vs_mpp.sh` gate passed on RK3576.
- The RK3576 release LTO compiler currently SIGSEGVs in LLVM during the VP9
  build; the functional matrix uses the debug binary, so no throughput claim
  is made from this run.

## 2026-11-08 RK3576 / RK3576 VPU720 JPEG bring-up

- The VPU720 qfactor-50 matrix is **19/19**. NV12 passes at 320x240, 640x480,
  1280x720 and with a 320x240 60-MCU restart; 320x240 NV16, I420, NV21,
  YUV422P, NV61, YUYV, YVYU, UYVY, VYUY, NV24 and YUV444P also match the
  installed C MPP byte-for-byte. YUV400 input and 400/420/422/444 output add
  the last four rows.
- NV12 output sizes are 5,029, 12,028, 28,124 and 5,041 bytes. The 320x240
  4:2:0 variants are 5,029, 4:2:2/packed variants are 5,009, and 4:4:4 variants
  are 4,967 bytes. The Rust path uses client 20, the VPU720 43-word register
  window, a 384-byte transformed Q-table and the hardware status length as a
  byte count.
- `scripts/board_rk3576_jpege_vs_mpp.sh` is the reproducible RK3576 gate. A shared
  layout model derives storage size, U/V offsets, chroma stride and UV-swap.
  Tile input remains. C's 422/444 outputs are only 623 bytes for the NV12
  fixture, and Rust matches those bytes. Low-delay is an explicit reject
  because the C VPU720 HAL has null `part_start`/`part_wait`; the reference
  probe spins on zero-length packets. The HAL also exposes neither an RGB input
  case nor a rotation register, so Rust rejects both explicitly.

## Environment

- RK3588, Debian 11.11, kernel 6.1.57
- `librknnrt` 2.3.2, Rust 1.88.0
- sustained 80% OPP cap enabled: little `performance` @1.416 GHz, big
  `performance` @1.8 GHz (see `docs/rk3588-power.md`)
- Source worktree synced from the local Mac checkout; `.git`, `refs`, `target`,
  model directories and existing board data were preserved
- `/tmp/mpp-old`, `/tmp/mpp-develop` and `/tmp/mpp_trace.so` restored from
  `~/rk-mpp-oracles` with `scripts/setup_board_refs.sh`

## Tests

`cargo test --release --all-targets` currently enumerates **1295 tests**. The
regular suite passes **1254 / 0 failed / 41 ignored** on RK3588. The four ignored
JPEG encoder hardware tests relevant to this slice were rerun separately and
pass **4/4** under the current 80% OPP cap. The two JPEG restart/rotate cases
share a temporary input name and therefore need `--test-threads=1`.

The H.264/MJPEG public SDK gate also passes on RK3588: C and C++ consumers link
against both the shared and static libraries, every exported API symbol is
present, lifecycle/error checks pass, and H.264 (7/127/313-byte chunks and
whole-stream input) plus MJPEG (97/211-byte chunks and four JPEGs in one packet)
match the board C MPP byte for byte in NV12 and I420.

Fixes required by the validation:

- `examples/iep_dump.rs` now supplies the new `DeinterlaceJob.pd_mode` field.
- Fixture manifest ignores macOS AppleDouble `._*` transfer metadata.
- VP8 stream-policy tests now assert the board-verified behavior: a torn final
  packet is zero-padded and decoded; a shortened middle packet is submitted and
  marks that picture with a hardware error instead of being classified as a
  dropped parser packet.
- Pooled JPEG workers now close their `MppService` before any shared dma-buf
  lease can return to the pool. The declaration order makes this hold on every
  early `?` path too. This targets the RK3588 kernel list corruption observed in
  `mpp_dma_find_buffer_fd()`.
- H.264 NV12 and colmv frame storage now uses separate counted pools. IDR and
  geometry changes drop all old reference surfaces before clearing idle cache,
  so the reference-less IDR still receives a fresh mapping. The seven-stream
  H.264 regression, recovery-after-replacement, and a generated 15-frame /
  three-IDR stream all match C MPP byte for byte. A dma-heap test confirms that
  dropping the frame returns `in_use` to zero and the next acquire reuses the
  same fds without another allocation.

The W5-8 recovery gate exposed a stronger RK3588 driver failure on 2026-11-07.
`RESET_SESSION` followed by an H.264 decode still blocked the whole board even
when a replacement fd was opened before closing the old one. Normal recovery
therefore no longer issues reset: it closes the failed fd first and then opens
and health-checks a fresh session. The reset-only diagnostic passes 32
open/reset/query/drop cycles, but it is now an explicitly destructive example
rather than part of the ignored-test suite. The close-before-open decode gate
passes on the current RK3588 boot.

W5-8 gained the missing real-STATUS combination on 2026-11-11. A positive but
unmapped input fd drives an H.265 320x240 auto-tile session through the real
submit path; RK3588 completes the job without `enc_done` (`int_sta=0x00000000`),
so the service closes and reopens the client fd. The encoder then zeroes the
tile line buffers, invalidates the warm-up marker, and makes the next picture
a parameter-set IDR instead of a P frame against an unknown reference epoch.
That recovered IDR is byte-identical to a newly opened encoder. The complete
`scripts/board_framework_recovery.sh` gate is **4/4**: two session replacement
checks, the H.265 tile recovery case, and the real VP8 damaged-packet STATUS
case.

W5-6 cross-session CCU submission is now board-verified as well. The RK3588 device
tree exposes vdec/venc/jpeg `rockchip,ccu` topology, and `BatchServer` emits one
`SET_SESSION_FD` chain for multiple real session fds. The kernel source's
`mpp_collect_msgs()` ends a task at the next delimiter and accepts `LAST_MSG`
only on the final task, so internal tasks are explicitly kept non-final. This
6.1 driver triggers both sessions but does not fill every `mpp_bat_msg.ret`;
the Rust path therefore polls each real session after the shared submit.
`tests/framework_ccu_hw.rs` opens two independent VEPU2 JPEG sessions and
encodes different 640x480 NV12 pictures through one batch. Both JPEG streams
match standalone references byte for byte. `scripts/board_framework_ccu.sh`
passes **3/3** repeated runs. The same script selects RK3576's VEPU510 path; two
independent 320x240 fixed-QP IDR sessions also match their standalone streams
and pass **3/3**.

The previous board disappearance was not thermal: package readings were
38–39°C. The prior boot log ends with CPU6 soft-lockup and RCU stall while
`rk-mpp-rs` is inside the vendor JPEG driver's
`mpp_dma_find_buffer_fd()`/`mpp_dma_import_fd()` path. After fixing the
service-before-lease lifetime, RK3588 completed **500 consecutive async demos**
(3000 pooled JPEG jobs, three workers) without a lockup or MPP reset. The
current 80% CPU cap reduces timing/current stress; it does not replace the
lifetime fix and is not claimed as the root-cause fix by itself.

## Board regression

The current `FULL_QP=0 PERF_GATE=1 scripts/board_regress_all.sh` run passed
every suite with `ALL SUITES OK`. The earlier full-QP run passed the entire
24-cell H.264 QP matrix, but exposed stale `/tmp` output files in the HEVC
tile/scaling/LTR generators and an empty C STATUS trace capture. Those harness
bugs were fixed by cleaning root-owned outputs before generation and checking
that the C trace is non-empty; the complete HEVC suite (13 PASS lines) and
STATUS comparison (C 2 / Rust 2, all fields equal) were then rerun green.

- reference setup
- H.264, H.265, MPEG-2, MPEG-4, H.263 and VP8 decode
- H.264 full 24-cell size/profile/QP matrix
- H.264 CBR/VBR/AVBR, debreath and STATUS
- asynchronous framework, AV1 film grain, IEP2, JPEG decode/encode and VP9
  board tests

The first pass exposed a sync-only false failure: board scripts packaged the
local `.worktrees` and `refs` trees into every transfer, so tar returned
nonzero when those large trees changed during the run. All board sync commands
now exclude both; reference trees remain managed by
`scripts/setup_board_refs.sh`. After restoring `/tmp` corpora from
`~/rk-mpp-oracles/corpus`, the H.264 and HEVC decode suites were rerun and every
case passed with no decoder SKIP.

## Official `mpi_dec_test` compatibility

`scripts/board_mpp_official_dec_test.sh` runs the vendor `mpi_dec_test`
executable without modifying or relinking the test source. `LD_PRELOAD` routes
`mpp_create/init/destroy`, packet/frame/buffer objects and
`decode_put_packet/decode_get_frame` to rk-mpp-rs, while the native library
continues to provide its test-only log, cfg and OSAL helpers.

| case | RK3588 | RK3576 |
|---|---|---|
| H.264 640x480, 6 B-frame pictures | PASS, byte-identical | PASS, byte-identical |
| H.265 320x240, 1 picture | PASS, byte-identical | PASS, byte-identical |
| MJPEG 320x240, 1 picture | PASS, byte-identical | PASS, byte-identical |

The compatibility layer now implements the local `MppMeta` surface used by
the official object API: s32/s64/ptr, frame/packet/buffer, default getters,
duplicate/ref/lifetime and deep copies for `KEY_USER_DATA`/
`KEY_USER_DATAS`. Advanced decode consumes packet `KEY_OUTPUT_FRAME` and
publishes `KEY_INPUT_PACKET` on the returned frame, so the preallocated frame
identity remains unchanged. `scripts/board_mpp_compat_meta.sh` passes on RK3588
and RK3576; on RK3588 the unchanged native `mpp_meta_test` also passes with two
threads and ten loops. A returned frame-identity mismatch is a hard test
failure. This is a real official decoder-tool gate. The VEPU580
H.264/H.265 encoder packet kernel metadata was subsequently closed on
2026-11-10 for VEPU580 and VEPU510. VEPU510 built-in CBR/VBR/AVBR and
user-defined streaming RC callbacks both use the same per-frame session.

`scripts/board_mpp_official_enc_test.sh` now exercises the same bridge for the
vendor encoder tool. It reads `MppEncCfg` from the vendor helper object, submits
the test's NV12 `MppFrame`, and drains owned `MppPacket` objects:

The 2024 RK3588 harness is run with `ENC_GOP_SPEC=60` to preserve its captured
reference schedule. The 2025 RK3576 harness uses the script default `0:60`, which
sets an ordinary 60-picture GOP without the absent smart-GOP ref config.

- H.265 320x240 one picture: byte-identical, SHA
  `1f00679ad21815e45c08f1fedd2873d4604cc654ccb131a6ce44ce6fd352e84a`.
- H.265 320x240 six pictures: byte-identical, SHA
  `e763e3577eff87ec4154438de2a761e35affeba3d92e897ae2742f884af03462`.
- MJPEG 320x240 one picture: byte-identical, SHA
  `08eb7c6f097f70e9e5424111a83dddef4d7cc46fc01d2772b42dc565b5388623`.
- H.264 320x240 one picture: both streams decode to identical YUV, SHA
  `9d8c6f546089d1564f8d5c0401d652dfa4c0e9b24a634dbb7aa33722324ac6fc`.
- H.264 320x240 six pictures: all SPS/PPS, rc-SEI and VCL NAL units match C
  MPP; all six decoded YUV pictures are byte-identical. Only the version-SEI
  build text differs, which the stream comparator checks for framing only.

On RK3576 the unchanged official encoder tool now also passes all exposed codecs:
H.264 High one picture and a real six-picture stream compare identical NAL
sequences and decoded reconstruction (version-SEI framing only); H.265 one and
six pictures are byte-identical (`8c693d15…1efa97a7`,
`cf6b5f6a…eba2e5fd`); MJPEG is byte-identical (`93d7ac00…eab945e`). The RK3576
object path uses its own CABAC/8x8 and H.265 register templates plus a MEIW
buffer; it does not reuse the direct CLI's 4710-byte H.265 prefix format.
With `CHECK_MOTION_INFO=1`, the script also compares the caller-owned
`KEY_MOTION_INFO` buffer: H.264's 1200 bytes are byte-identical
(`655a3ef0…5955273`) and H.265's 1280 bytes are byte-identical
(`bfe492ba…09f462`).

The compatibility C consumer now also covers external buffer ownership:
caller-pointer `mpp_buffer_import`, external-group `commit/get`, fd-only import
through a memfd-backed mapping, `MppBufferInfo` round-trip, index propagation,
and pointer/read/write behavior. Internal buffers now carry a real dma-heap fd
and are recycled from the group's unused list; usage and count limits are
updated on actual allocations. These checks run as part of
`board_mpp_compat_data.sh` on RK3576.

H.264/H.265/VP9/AV1/MJPEG compatibility decoders now request DMA-BUF SDK
frames. Advanced `KEY_OUTPUT_FRAME` fills the caller frame directly from the
decoder's mapped planes instead of allocating an intermediate frame/buffer and
copying twice. The unchanged `mpi_dec_test` suite still matches native MPP
byte-for-byte for H.264, H.265 and MJPEG on both boards.

H.264/H.265/VP9 and MJPEG also accept the caller frame's dma-buf as the next
hardware output surface. On RK3576, the focused C advanced test verifies frame and
buffer identity plus byte-identical H.265/MJPEG output; H.264 640x480 six-frame
advanced output is byte-identical to the native C golden
(`338214ea…374b815`). The VP9 consumer now splits IVF packets one frame per
submit, keeps output surfaces alive across the reference DPB, and reserves the
caller frame only for `show_frame`. Its direct DMA-BUF path preserves the
256-odd hardware stride and 9/5 frame allocation. On both RK3588 and RK3576, all 12
frames of `vp9_p0_lossless_320x240_12.ivf` match C MPP byte for byte
(`c5381bbf…3949a`). AV1 uses the same packet-level output contract: VDPU34x
imports the caller buffer into its display pool, while VDPU383 replaces the
frame's output lease without disturbing colmv/CDF state. Coded `show_frame`
pictures write that surface directly; `show_existing` copies from the historical
display surface because its backing reference was decoded in an earlier packet.
All 10 frames of `av1_320x240_10.ivf` match C MPP on both boards
(`88f34ea5…c5f8e`). The shared buffer pool now reuses Empty slots after
`clear()`, so repeated injection no longer hits a false `Full` at the pool's
historical slot count.

Performance gate:

- H.264 720p: C MPP 121.5 FPS, Rust 283.8 FPS (**+133.7%**)
- 1920×1080→640 RGB scale: 0.5060 ms (limit 0.65)
- 1280×720→640 RGB scale: 1.1100 ms (limit 1.15)
- 640×480→640 RGB scale: 0.4687 ms (limit 0.50)
- 1930×1080→640 RGB scale: 1.4702 ms (limit 1.65)

Sustained-motion and concurrent decode evidence uses a generated
`testsrc2` 1280x720/30 fps stream with actual inter-frame motion. H.264,
H.265 and VP9 are 120 frames; AV1 is 60 frames. Single-stream figures are the
median of nine interleaved C/Rust samples at the 80% OPP cap:

| codec | C MPP | Rust | Rust delta |
|---|---:|---:|---:|
| H.264 | 209.8 FPS | 378.0 FPS | +80.2% |
| H.265 | 228.1 FPS | 292.8 FPS | +28.4% |
| VP9 | 207.6 FPS | 244.2 FPS | +17.6% |
| AV1 | 201.3 FPS | 310.7 FPS | +54.3% |

The first concurrent matrix exposed the real regression: H.264 with planar
I420 conversion reached 413.8 FPS at one process, fell to 262.0 FPS/process at
four and 210.5 at six, making aggregate Rust throughput 10–11% slower than C.
The DMA-BUF frame path removes the per-frame CPU NV12 copy, and the H.264
decoder now skips even the redundant mapping and `DMA_BUF_SYNC` when it will
not read pixels. `board_multistream_bench.sh` then measured:

| streams | C aggregate | Rust DMA aggregate | Rust per stream | delta |
|---:|---:|---:|---:|---:|
| 1 | 186.0 FPS | 381.0 FPS | 381.0 FPS | +104.8% |
| 2 | 582.5 FPS | 854.1 FPS | 427.0 FPS | +46.6% |
| 4 | 1151.1 FPS | 1240.3 FPS | 310.1 FPS | +7.8% |
| 6 | 1541.8 FPS | 1525.4 FPS | 254.2 FPS | -1.1% |
| 8 | 1550.9 FPS | 1540.9 FPS | 192.6 FPS | -0.6% |
| 12 | 1621.6 FPS | 1629.0 FPS | 135.7 FPS | +0.5% |
| 16 | 1679.8 FPS | 1606.7 FPS | 100.4 FPS | -4.4% |
| 24 | 1696.1 FPS | 1577.2 FPS | 65.7 FPS | -7.0% |

Thus 24 independent 720p playback processes were measured above real-time
simultaneously. Extrapolating the measured 1.58k aggregate ceiling gives about
52 streams at 30 fps, but that is a throughput estimate rather than a
process-count claim. At 6+ streams both implementations are dominated by the
shared rkvdec service ceiling; the Rust-specific disadvantage is gone once
pixel conversion is removed.

Standalone drop-in decode is also closed. `scripts/board_mpp_standalone.sh`
creates an overlay directory whose only `librockchip_mpp.so.1` is a symlink to
`librk_mpp_rs.so`, verifies the unchanged vendor `mpi_dec_test` resolves that
symlink through `ldd`, and compares its output against a second run using the
real vendor library. RK3588 now passes 16/16 byte-exact cases: H.264
640x480/6 frames (`338214ea…f815`), H.265 (`fcedb435…05e0`), VP9 12 frames
(`c5381bbf…3949a`), AV1 10 frames (`88f34ea5…c5f8e`), MJPEG
(`98c5f9cc…a689f8`), VP8 IP/altref, MPEG-2, MPEG-4 IP/IPBB, H.263 CIF/QCIF,
two AVS+ vproc cycles (`6ef29ca6…e169`, `35ac81e6…f3d1`) and an
interlaced-frame cycle (`1fa2e8ed…c2b0`). RK3576 passes
its supported H.264/H.265/VP9/AV1/MJPEG subset. The local shim covers log/data symbols, environment and
time services, OSAL allocation, SoC name/type, spinlocks, `MppDecCfg`,
singleton/trie metadata and the kmpp/cfg startup objects. Logging currently
uses Rust naked ABI trampolines to preserve the variadic registers before
tail-calling a local C `vfprintf` helper. The board regression requires the
unchanged decoder log to contain the actual `320x240` and coding type rather
than unexpanded `%d` placeholders.

`scripts/board_mpp_standalone_encode.sh` exercises the same replacement on
the encoder side. The unchanged `mpi_enc_test` resolves `librockchip_mpp.so.1`
to `librk_mpp_rs.so`, creates a local key/value `MppEncCfg`, and runs
`MPP_ENC_SET_CFG` through the existing encoder bridge. On RK3588, H.264 and H.265
standalone streams decode byte-identically to the vendor streams
(`9d8c6f54…24ac6fc` and `c7c74e0c…eb03643`), while MJPEG is stream-identical
(`08eb7c6f…b5388623`). This closes the one-frame object path; reference-cfg
objects also survive create/reset/LT/ST add/check and the H.264 case uses
`-g 1:60`.
`board_mpp_ref_cpb.sh` now also exercises all `MppCfgStrFmt` string formats
against the current vendor. LOG and JSON produced by either library are
accepted by the other with matching CPB fields. TOML extraction is
byte-identical between vendor and Rust, and Rust accepts the vendor file, but
the vendor rejects its own extracted TOML in `mpp_cfg_from_toml` with return
`-40`; this is registered in `docs/DIVERGENCES.md`. Reproduce the format
matrix with `BOARD=my-rk3588-host MPP_REF=/tmp/mpp-develop scripts/board_mpp_ref_cpb.sh`.
`scripts/board_mpp_enc_cfg_strings.sh` is the corresponding `MppEncCfg`
scalar/array test. The vendor and Rust libraries each dump LOG, JSON and TOML;
the other library applies that file and reads back mode, target bitrate, width
and codec type. LOG, JSON and TOML all pass 1/1. The Rust parser also accepts
the vendor's full nested config, including the four-element hierarchical-QP
arrays.
`scripts/board_mpp_hier_qp.sh` goes one step further and encodes 33 frames with
CBR plus `hier 1:2:3:4:1:1:1:1`. The probe sets the hierarchical-QP scalar and
both arrays through `MppEncCfg`; after `MPP_ENC_SET_CFG`, Rust must reproduce
the VEPU580 stream. Against the stable `/tmp/mpp-old` encoder oracle, both
streams are byte-identical after removing only the build-identity SEI. A second
cell supplies the 16-entry `hw:aq_step_i` table `0..15`; Rust replaces the
I-picture `RCKLUT.aq_step` while preserving the distinct P-picture default, and
the stream again matches the old oracle.
`scripts/board_mpp_rc_fields.sh` uses the unmodified `rc_probe` to cover
advanced scalar fields from `MppEncCfg`. A 12-frame CBR debreath run and a
six-frame super-frame reencode run both match `/tmp/mpp-old` after removing the
build-identity SEI. A third low-bitrate six-frame `DROP_MODE=2` cell exercises
the software PSKIP replacement and is byte-identical as well.
`scripts/board_mpp_enc_cfg_test.sh` runs the vendor's unmodified
`mpp_enc_cfg_test` with `librk_mpp_rs.so` as `librockchip_mpp.so.1`. All nine
cases pass: scalar/array access, LOG extraction, scalar/array apply,
extract/apply roundtrip, JSON apply and the `KmppVencStCfg` kobj roundtrip.
The same script compiles a focused C consumer that uses the ST_CFG handle
directly, exercises scalar plus hierarchical-QP/AQ arrays through LOG, JSON
and TOML, and verifies that the vendor-compatible `mpp_enc_cfg_deinit()` path
releases it. All three formats pass.
The corresponding LOG/JSON/TOML bidirectional `MppEncCfg` matrix remains
**3/3 PASS**. The same tree also passes the host workspace gate with
**1364 passed / 0 failed / 46 ignored**, and the RK3588 workspace gate with
**1366 passed / 0 failed / 46 ignored** plus strict rustdoc.
`scripts/board_mpp_enc_cfg_gdr.sh` also maps the vendor probes'
`rc:refresh_en/mode/num` keys through the Rust standalone library. H.264 GDR
is NAL-identical under the version-SEI rule (`09b7bc26…4367ca8`) and H.265 is
byte-identical (`bc22eb33…454f035`) to the typed CLI path.
`board_h264_deblock_vs_mpp.sh` now includes Rust-object cells for
`h264:dblk_alpha` and `h264:dblk_disable`; both match the typed CLI's five
NALs under the version-SEI rule (`2730e887…10c697d4`).
`board_mpp_enc_cfg_h264_syntax.sh` matches vendor/Rust object streams for
Main-CAVLC without 8x8 (`5c83b5e8…85e8c1ff`) and High-CABAC without 8x8
(`349ac658…d4d2a5`).
`board_mpp_enc_cfg_h265_syntax.sh` maps H.265 deblock/SAO/loop-filter keys
through the standalone object path; both object cells are byte-identical to
the typed CLI (`1263cb1f…f647b4` and `1cef6ee0…b8171d`).
`board_mpp_enc_cfg_i420.sh` encodes the same picture through NV12, I420 and
NV21 compatible objects for both H.264 and H.265; all streams are
byte-identical (`073c6215…f00b435e` and `df2395b1…be2f4b62`).
The legacy utility does not call `MPP_ENC_SET_REF_CFG` directly on the
multi-frame path: it stores the object at `"rc:ref_cfg"` inside `MppEncCfg`.
The bridge now reads that pointer before constructing the encoder and maps the
three TSVC2 short-term entries into the VEPU580 schedule. A six-frame
`-g 1:60` run produces the vendor alternation of disposable and reference P
pictures (`nal_ref_idc 0/2/0/2/0`), repeats the type-14 prefix exactly, shares
the reference picture's frame_num as the C DPB does, advances POC independently,
and changes the non-reference slot cycle as required. All 16 NAL units match
the vendor stream except the intentionally different version-SEI text; both
six-frame outputs decode to the same 320x240 YUV, SHA-256
`fec69971fb789cf8cf3e2798f85ccbdd01b7e22d532621114e036e672864f0a4`
(`691200` bytes). Extending the same fixture to 12 frames exercises a second
slot cycle; all 28 NAL units still match under the version-SEI rule and the
1,382,400-byte YUV output has SHA-256
`d9a3ed5528cc4b0dc5895741e12d7092eb37d47a024aaab3417dce7aecad7e63`.
TSVC3 uses four repeating short-term steps with temporal ids `2, 1, 2, 0`.
The temporal-zero step walks back to the previous temporal-zero reference,
which produces a short-term list reorder at the following reference picture.
Over the same 12-frame fixture its 28 NAL units match and the YUV SHA-256 is
`7794eb885e695c780f9e98c979677f74bac09a45f80b1deefc7f733f7488f8e2`.
TSVC4 adds a one-frame long-term reference. The opening IDR sets
`long_term_reference_flag`, frame 8 replaces LT0 with MMCO6, and later pictures
reorder the P list to `long_term_pic_num=0`. Its 12-frame stream again has 28
matching NAL units and decoded YUV SHA-256
`33923dd26ff83f4d2d59d380b372614875bd2e7f58d281f5bce062eb8db1186a`.
A 64-frame run crosses `gop=60`: the second IDR re-emits SPS/PPS and both SEIs,
the reference schedule clears, and all 136 NAL units match. Its 7,372,800-byte
YUV output has SHA-256
`a3ad1bc65ac3ee66c2014ef3e2075722c5dc7aec77b29ec6568155f0f35a0593`.
`MPP_ENC_SET_IDR_FRAME` is now executable rather than a no-op. With forced IDRs
before input frames 2 and 4, `HEADER_MODE_EACH_IDR` enabled and SEI disabled,
the official-path probe emits three IDR sections, four pictures each
(`12` NALs total). RK3588 and RK3576 both match the C stream and decode to the same
691,200-byte YUV (`05901761…f71159` on RK3588). The H.265 VEPU580 probe runs the
same schedule and matches 15 NALs; its YUV SHA-256 is
`b2765277b355c52eae77bae10997ec4284c03b20d76f2b1cadfeacf0f3c56607`.
POC restarts at 1 after each forced IDR. Other encoder controls remain open.

On RK3576, VEPU510 now executes the same controls. The MPP object path matches
the vendor's complete 15-NAL stream and 691,200-byte decoded YUV
(`7f3d4664…841231`). `MPP_ENC_GET_HDR_SYNC` now selects the direct
`h265e_probe` template or the `mpi_enc_test` template exactly as the caller
expects. The board gate also covers both caller motion-info capacities: 640
bytes for the probe and 1280 bytes for unchanged `mpi_enc_test`, and the
corrected `kmpp_objdef_get/register` ABI lets the preload path initialize
without a segfault.

The RC API control-plane probe matches vendor output on RK3588 and RK3576. Both list
the same six built-in strategies (`default` for AVC/HEVC/MJPEG/VP8 and `smart`
for AVC/HEVC), filter AVC to two entries, report the initial
`default,type=0` selection, append a registered `test_h264e_rc`, select and
restore it by `GET/SET_RC_API_CURRENT`. A user-defined strategy can now drive
H.264/H.265 frames through the complete callback lifecycle.
The direct `rc_api_add`, `rc_brief_get_all` and `rc_brief_get_by_type` exports
are compared in a separate process and match vendor output as well.

`board_mpp_custom_rc.sh` registers a C-defined strategy and runs the complete
callback lifecycle through both encoders. The matrix covers H.264/H.265 x
fixed-QP/CBR/VBR/AVBR on RK3588 VEPU580 and RK3576 VEPU510, 8/8 on each board,
plus same-frame-type and frame-type-changing reencode, 12/12 total.
On RK3588, H.264 fixed is YUV `6d1e7dc9…b90cd3`; H.265 fixed is
`53bf281b…ad9789e`. Their CBR/VBR/AVBR outputs are respectively
`3f34100a…f7023e` and `cad3894e…a2384e`. On RK3576, the corresponding hashes
are H.264 `955dc593…f49a71` / `54d0d202…bda0` and H.265
`c27657d4…e67e` / `39c1fc34…fe8b`. Streams, decoded YUV, init configuration,
full callback field lines and hardware `bit_real/quality_real` all match the
ABI-compatible vendor library. The same-frame-type reencode case requests a
second attempt at QP30 from `check_reenc`: RK3588 outputs H.264 YUV `60fe9815…8ad0` and H.265
`f618041a…1246f`; RK3576 outputs H.264 `771a0735…fee70` and H.265
`9fd432ba…8171`. The first hardware packet is discarded, while the same
input/reference surfaces are rerun before `frame_end`.
The frame-type-changing case starts from a P picture and requests a non-IDR
intra/IDR update. H.265 matches the vendor's hybrid P-syntax/INTRA-config/IDR-NAL
template, but that stream is a C-BUG: FFmpeg emits the first two frames
(RK3588 YUV `fffd339a…009d`, RK3576 `ba4b21e9…5343`) and then stops with
`Could not find ref with POC 1`; the full three-frame stream remains
byte-identical to vendor. H.264 preserves the vendor's actual P slice while
the callback state reports the requested type and decodes all three frames:
RK3588 YUV `d2b11845…d4ac`, RK3576 `0191b6b5…dc56`. Combining reencode with drop/PSKIP
remains outside this verified matrix.

`board_mpp_osd.sh` uses unchanged `mpi_enc_test` with `osd_enable=1`, a custom
256-color palette and eight index regions. H.264 matches the vendor stream and
decoded YUV byte-for-byte (`1055ca00…3b22a`). H.265 now does the same:
stream SHA `625d464e…efc838` and YUV `6f44da15…3cbd310` match exactly. The
root cause was not palette or buffer content, but H.265 replacing its offset
list after OSD setup and dropping `reg3093..3099`; appending preserves all
seven OSD offsets. A 6,144-byte buffer dump still confirms the eight expected
`0..7` index regions.

These are the current 80% OPP medians from 11 interleaved samples, so they are
directly comparable to the current functional and stress results. The older
uncapped-profile peaks are retained only as historical observations.

### H.264 file-input DMA pipeline

The supplied RK3588 `mpi_enc_test` result used MPP's built-in moving
`fill_image()` input. Reproducing that class of work at 1280x720, High profile,
QP26, GOP60 and 500 pictures under the persistent 80% OPP cap gives the
following 11-sample medians:

| path | median | throughput | vs C whole path |
|---|---:|---:|---:|
| C MPP built-in fill, core 6 | 1222 ms | 408.97 FPS | baseline |
| Rust mmap file input, core 6 | 1306.909 ms | 382.58 FPS | -6.4% |
| Rust two-surface DMA pipeline, cores 6+7 | 999.532 ms | 500.23 FPS | +22.3% |

The single-core Rust path is therefore still slightly behind C's in-memory
fill loop, but the complete file-input path is not: the old CLI path measured
about 354 FPS on the same 500-picture fixture. Two changes removed most of the
remaining software gap:

1. a regular input file is mapped and copied directly into a
   `system-uncached` DMA surface, avoiding the kernel's `read` /
   `copy_to_user` path;
2. on a multi-core affinity, a reader thread fills the next preallocated DMA
   surface while the encoder core drives VEPU580, so input movement overlaps
   the hardware job instead of extending it.

The second number deliberately uses two cores. It is a whole-pipeline result,
not a claim that VEPU580 itself became 22% faster. Both Rust modes produced
the same 2,995,229-byte bitstream with SHA-256
`0a039e02910169e9900349a16e676f580c3da9e265b154d15ebcbde3a2bc6a2e`.
The 691,200,000-byte input fixture is the byte-exact C `fill_image()` output
with SHA-256
`db3d065df8ea78f9068aa397dfc6ed3bcd404d27df85b772c307d58aff556546`.
`scripts/board_rk3588_h264_encode_pipeline.sh` regenerates the fixture from
`tools/mpp_nv12_fill.c`, checks both hashes and repeats the interleaved
benchmark.

### H.265 file-input DMA pipeline

The `h265-gop` file path now uses the same two-surface reader/encoder overlap
as H.264. The previous path allocated a plain frame `Vec`, read each picture
into it, and then copied it into the encoder's DMA input surface. The new path
fills the next `system-uncached` surface on the reader thread, submits the
current surface fd, and recycles the surface only after the hardware job
finishes.

At the persistent 80% OPP cap, the same 120-frame 720p fixture produced the
same H.265 stream before and after the change:
`a2865a07a64d266073072d75eea21433e9e5ef09a76acd4034a146f38188d20a`.
`perf stat` measured 415 to 302 ms wall time, 182.4 to 99.8 ms task-clock and
137.7 to 58.5 ms system time. A seven-sample interleaved comparison selected
the retained `system-uncached` heap at a 275 ms median over `system` at 285 ms.

The current five-sample, 60-frame release benchmark passes all four H.265
rows. Rust versus C throughput is +39.1% at 320x240, +77.3% at 640x480,
+27.1% at 720p and +35.7% at 1080p. On a 500-frame 720p run, Rust completed
in 1.047 s against C's 1.842 s; H.264 completed in 1.022 s against C's
2.503 s.

The CLI now writes each H.265 access unit through a 1 MiB `BufWriter` instead
of accumulating the full stream. The 120-frame 720p output kept the same
SHA-256 `a2865a07…88d20a`, and a subsequent 60-frame four-size benchmark
remained 4/4 PASS.

The rate-control CLI paths now use the same DMA input and streamed-output
pipeline. At 80% OPP, five interleaved 120-frame 720p samples measured
`h265-gop-rc` at 279 ms versus 387 ms forced-single-core and `h265-rate` CBR
at 273 ms versus 384 ms. Their outputs were byte-identical between the two
paths: `7c4e93db…c11f65` and `bb99af68…d7a6ad03`.

VEPU580 H.264 now reuses its STATUS vector and Batch per session. At 80% OPP,
seven interleaved 500-frame 720p samples moved from 1140 to 1055 ms while the
7,031,305-byte stream remained identical
(`68715bd4…e2d5617c05`).

VEPU580 H.265 reuses aggregate STATUS, per-tile STATUS and Batch storage. Seven
interleaved 500-frame 720p samples moved from 1111 to 1069 ms with an identical
stream (`ae613590…c93b469783`).

Because `SET_REG_READ` overwrites the complete STATUS range, the 16 KiB clear
was removed. H.264 moved from 1098 to 1052 ms and H.265 from 1129 to 1075 ms
over seven interleaved 500-frame samples, with unchanged streams.

The encoder output packet buffers now live in `system-uncached` DMA heaps. CPU
reads therefore need no per-packet cache invalidate START/END ioctls. Seven
interleaved 500-frame 720p A/B samples at 80% OPP measured H.264 at 993.284 ms
versus **968.188 ms** (-2.53%) and H.265 at 1043.576 ms versus **1018.445 ms**
(-2.41%). The outputs remained byte-identical:
`0a039e02910169e9900349a16e676f580c3da9e265b154d15ebcbde3a2bc6a2e` and
`ae613590ee5b546f89d76dad1ecaa1528da292abfbd4e92303cc84c93b469783`.
H.264 low-delay output remained 5/5 exact. The H.265 low-delay comparison
is also 4/4 exact. Its board script now detects whether the selected vendor
headers have `EncRcTaskInfo.bit_target_fix`, passes the matching
`CUSTOM_RC_HAS_BIT_TARGET_FIX` value to the C probe, and compares streams after
removing only the build-identity SEI text. The cells cover BY_CTU, BY_BYTE,
the 40-packet slice-FIFO case and a six-picture short-term GOP.

The H.264 and HEVC `/tmp` corpora were restored from
`~/rk-mpp-oracles/corpus` after reboot. The H.264 decode suite passed 7/7 and
the HEVC suite passed every available large-profile, tile, slice, open-GOP,
Main10, scaling-list and long-term-reference case before the final performance
gate.

The async framework gate also reports per-job queue wait, execution and
turnaround time plus cumulative submitted/done/failed/cancelled counters.
On RK3588, six JPEG decode jobs across three workers were byte-identical to the
sync path with `submitted=6 done=6 failed=0 cancelled=0`.

## H.264 streaming output

On a 1080p30 High/B=2 stream:

- VUI reorder depth resolves to **2**
- Rust planar output is **byte-identical** to C MPP after NV12→planar
  conversion (SHA-256 `aa049b832da5…dc36c6`)
- Rust packed NV12 is **byte-identical** to C MPP NV12
  (SHA-256 `1c2475b2cb0f…a06408`)
- single-thread streaming decode median is about 0.29–0.44 s for 90 frames,
  versus about 0.88–0.90 s for the C test program

Five-process-boundary sweep, 1080p30:

| path | sustained ≥30 FPS |
| --- | ---: |
| C MPP decode | **28 streams** |
| Rust streaming decode | **26 streams** |

## AV1 distance-weighted compound

The software AV1 reference decoder now covers distance-weighted compound
prediction. The order-hint lookup selects libaom's `fwd_offset`/`bck_offset`;
the blend uses the normative truncating four-bit weighting before the shared
compound round offset and final rounding.

The checked-in fixture
`tests/data/av1_inter_distance_weighted_320x240.ivf` contains one 32x16 and one
8x8 distance block with weights `[11,5]` and `[13,3]`. On RK3588, all five visible
Y/U/V frames are byte-identical between this decoder and the unmodified libaom
decoder. The targeted unit test, all 12 inter-frame tests, and the complete
fixture manifest pass locally and on RK3588.

Inter-intra is also complete. Two isolated five-frame fixtures cover 15 smooth
inter-intra blocks and nine wedge inter-intra blocks; together they exercise
all four DC/V/H/Smooth modes plus multiple wedge indices. Every visible Y/U/V
frame is byte-exact against libaom.

OBMC now follows the normative above-first/left-second neighbour scan, reads
`use_obmc_cdf`, keeps switchable interpolation syntax for OBMC blocks, and
blends neighbour predictions with the 1–64 sample masks. The five-frame fixture
contains 32 OBMC blocks and 232 OBMC mipixels; every visible Y/U/V frame is
byte-exact against libaom.

Local warped motion now gathers same-reference neighbour samples, derives the
least-squares affine matrix and shear parameters, and runs libaom's 193-phase
eight-tap warp filter. Its isolated fixture has 10 `WARPED_CAUSAL` blocks,
including zero-MV blocks driven by projected neighbours; all five visible Y/U/V
frames are byte-exact against libaom.

Single-reference global warped motion now derives `GLOBALMV` at each block centre from
`ROTZOOM/AFFINE` matrices and reuses the same warp predictor. A two-frame
isolated ROTZOOM fixture exercises 756 global-warp blocks; both visible Y/U/V
frames are byte-exact against libaom.

Compound global warp keeps independent warp state for both references and
supports distance-weighted and wedge blends with mixed warped/identity inputs.
A four-frame isolated fixture covers three `GLOBAL_GLOBALMV` blocks; all four
visible Y/U/V frames are byte-exact against libaom.

Temporal MV prediction now stores an 8x8 motion field for each reference
picture, projects it through order hints, and merges the projected candidates
between the nearest and outer spatial scans. A three-frame isolated fixture
covers 128 inter blocks; all visible Y/U/V frames are byte-exact against
libaom.

Inter segmentation now carries primary-reference segment maps, spatial and
temporal segment-id prediction, forced reference/skip/global modes, and
per-segment ALT_Q/ALT_LF/lossless state. A three-frame isolated fixture covers
1050 nonzero segment blocks and is byte-exact against libaom on Y/U/V.

Selected tx-size context now uses an inter neighbour's block extent even when
the neighbour is not skipped, while transform-partition context keeps the
separate skip-only override. The previously failing `av1_lr_320x240.ivf`
key+P stream now decodes all 458 footprint blocks (356 inter), and both visible
Y/U/V frames are byte-exact against libaom.

The remaining AV1 software-reference gap is a wider conformance/performance
matrix.

The final `delta_lf_multi` syntax is now represented with libaom's four-row
default CDF. Color frames maintain four independent loop-filter levels;
monochrome frames consume only the two active rows. The generated CDF state is
97 tables / 19,697 values and clones all four rows through SavedCdf. A real
one-frame keyframe, produced by a temporary libaom copy with only
`DEFAULT_DELTA_LF_MULTI=1`, is accepted by the unmodified `aomdec`; the Rust
pre/deblock/CDEF/LR Y/U/V planes are byte-identical to its final YUV. The
patched encoder's second inter frame is invalid, so the fixture deliberately
contains one frame and makes no encoder-stability claim. The complete host
`--all-targets` suite passes. A monochrome counterpart with the same patched
encoder exercises `mono_chrome=1`; its Y plane is byte-identical to `aomdec`.
That fixture exposed and now covers the missing mono UV-syntax gate and the
single-plane deblock grid.

## YOLO video chain

`yolov8s.rknn`, 768×432 H.264 people video, 40 timed frames after 5 warmups,
three interleaved rounds:

The model file was not present under the RK3588 home after this reboot, so the
end-to-end table below was not regenerated in this validation pass. Both CLIs
were rebuilt successfully; the measurement is the last RK3588 result from the same
source tree.

| path | median latency | median throughput | detections |
| --- | ---: | ---: | ---: |
| C | 33.608 ms | 29.75 FPS | 3 / 40 frames |
| Rust | **32.823 ms** | **30.47 FPS** | 3 / 40 frames |

Rust is 2.34% faster and the detection statistics match in every round.

## Playback

- C: `mppvideodec → xvimagesink`
- Rust: `h264_stream NV12 → rawvideoparse → xvimagesink`
- A 12-second 1080p30 steady playback completes in 12.15 s for 8 C streams,
  12.25 s for 12 C streams and 12.33 s for 16 C streams.
- Rust completes 8 streams in 12.36 s; 12 streams take 12.90 s and 16 streams
  take 17.27 s.

Thus the current RK3588 playback guidance is: C up to about 16 near-real-time
streams, Rust up to 8 stable streams with 12 streams borderline. The remaining
Rust playback cost is the user-space NV12 pipe; direct DMABUF scanout is still
the next performance step.

## Post-commit re-verification (2026-11-07)

The batch that had accumulated in the worktree since the 80 % cap checkpoint is
now landed as ten commits (`6f5d644` .. `4691f29`): the H.264 VUI reorder depth
and packed-NV12 helper that `684d9c2` needed in order to build, the
close-before-open session recovery on every submit/poll path, VP8 DPB surfaces
on the shared pool, the concurrent VEPU580 session gate, AV1 reference slices
90-101, the unsafe-contract annotations and C-API overflow guard, the new
bench/probe tools, the sync-tar excludes, and the plan and checklist updates
(top level is now 94 completed / 6 open, 132 / 11 with sub-items).

Re-run against the committed tree:

| gate | result |
|---|---|
| host `cargo test --all-targets --offline` | 1149 enumerated: 1118 passed, 0 failed, 31 ignored |
| `cargo fmt --all --check`, `git diff --check` | clean |
| RK3588 `cargo test --release --all-targets -- --ignored --test-threads=1` | all 31 board-only tests executed, 0 failed |
| `scripts/board_regress.sh` | H.264 7/7 byte-exact |
| `scripts/board_regress_hevc.sh` | 13 PASS, 1 SKIP (`/tmp/t720.hevc` C reference output absent on this boot) |
| `scripts/board_framework_recovery.sh` | 4/4 (two close-before-open replacements, H.265 tile recovery after real `int_sta=0`, one VDPU2 damaged-packet gate) |
| `scripts/board_framework_async_vs_sync.sh` | ALL PASS: 6/6 jobs identical, `submitted=6 done=6 failed=0 cancelled=0`, avg wait 2.134 ms, avg run 4.937 ms, 9 pool allocations under a cap of 10 |
| `scripts/board_iep_modes_vs_probe.sh` | 9 PASS, 0 FAIL |
| `scripts/board_mpp_packet_meta.sh` | RK3588 VEPU580 H.264/H.265 fixed-QP/CBR still+moving 8/8; RK3576 VEPU510 fixed-QP/CBR/VBR/AVBR 2-frame matrix plus 6-frame RC stream/YUV; JPEG 2/2; H.265 motion-info hashes exact |

Environment for this run: kernel 6.1.57 with the persistent cap service active
(`policy0` 1,416,000 kHz, `policy4`/`policy6` 1,800,000 kHz). Two
environment-gated debug traces still present in the worktree (palette map
origin in `src/reference/av1/palette.rs`, frame-header dump in
`examples/av1_reference_dump.rs`) were removed before committing, so neither
entered history.

## Multi-tile inter re-verification (2026-11-08)

The 2x2-tile inter fixture now has a non-ignored ten-picture Y/U/V gate.  On
RK3588 under the persistent 80 % OPP cap, the targeted release test passed
(`1 passed`, `0 failed`) and the complete release library suite passed
(`978 passed`, `0 failed`, `0 ignored`).  The host all-target suite enumerated
1151 tests and finished with `1120 passed`, `0 failed`, `31 ignored`.

## FFmpeg RKMPP drop-in (2026-11-21)

The installed Rockchip FFmpeg frontend is `c5e745e3c1` with
`--enable-rkmpp`. `scripts/board_ffmpeg_rkmpp.sh` overrides only
`librockchip_mpp.so.1` with `librk_mpp_rs.so`; the FFmpeg binary and its
`*_rkmpp` wrappers are unchanged.

| path | result |
|---|---|
| H.264 decode, 12×1280×720 NV12 | byte-identical, `eed99af9…ae50ae7c` |
| H.265 decode, 1×320×240 NV12 | byte-identical, `fcedb435…1d805e0` |
| VP9 decode, 12×320×240 NV12 | byte-identical, `c5381bbf…e3949a` |
| H.264 encode, 12×320×240 CQP26 | valid 12-frame bitstream; differs from vendor wrapper output |
| H.265 encode, 12×320×240 CQP26 | valid 12-frame bitstream; differs from vendor wrapper output |
| MJPEG encode, 12×320×240 | byte-identical, `5a0ccd0a…92c7873` |

Final gate: `SUMMARY passed=6 skipped=0 failed=0`.

The FFmpeg wrapper uses its newer info-change/external-buffer/control sequence,
not only the official `mpi_dec_test` path. The compatibility layer now returns
an info-change frame before the first picture, accepts the external group and
parser controls, returns the held first picture after `INFO_CHANGE_READY`,
attaches `KEY_INPUT_FRAME` to every encoded packet, and emits an EOS packet.
CPU NV12 copy-out is verified. The deinterlace command is accepted for this
progressive matrix, but real deinterlacing is not implemented.

The compatibility SDK path was also tested directly. A staged install produced
`librockchip_mpp.so`, `librockchip_mpp.so.1`, vendor `rockchip/*.h`, and
`rockchip_mpp.pc` version `1.14.0`. A C consumer linked through
`pkg-config rockchip_mpp` and ran with `librk_mpp_rs.so.1`. FFmpeg source
`c5e745e3c1` then configured successfully with:

```bash
PKG_CONFIG_PATH=/tmp/rk-mpp-ffmpeg-prefix/lib/pkgconfig \
./configure --enable-version3 --enable-rkmpp --enable-libdrm \
    --disable-everything --disable-doc --disable-programs --disable-network \
    --enable-decoder=h264_rkmpp --enable-encoder=h264_rkmpp
```

The minimal source build completed with `make -j8` exit 0 and produced
`libavcodec.a`.
