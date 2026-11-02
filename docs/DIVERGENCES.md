# Known divergences from the C reference

Scope rule (2026-11-02): pixel/byte-exact imitation plus output-preserving
performance. A difference is a defect **unless the C behaviour has been proven
wrong**. This file is the register of every difference that is currently known,
with its disposition and the evidence behind it. New work should close an entry
here before adding capability.

Dispositions:

- **FIX** - ours is wrong or unproven; the C result is the target.
- **C-BUG** - the C result is proven wrong by an independent decoder/spec check;
  ours is kept and the evidence is recorded.
- **NO-ORACLE** - the C vintage cannot produce the comparison at all; not claimed.
- **BY-DESIGN** - a deliberate, documented difference in *behaviour* (exit code,
  recovery) that does not change decoded pixels for valid streams.

## Decoders

| Case | Disposition | Evidence |
|---|---|---|
| VP9 `vp9_p0_720p_errres_seg`, frames 4/5 | **C-BUG** | Rust == libvpx (SHA-256 `0f0ff0da…deb51`) on all 6 frames; C differs by 106+107 V-plane bytes, max delta 3 (`5b737017…cac1`). Registers match word for word. |
| VP9 `vp9_p0_lossless_321x241_2`, frame count | **C-BUG** | libvpx and Rust produce 2 frames; C emits 1. Rust hash is the libvpx hash `8ac48f11…beca`. |
| VP9 damaged streams (`half-last`, `mid-flip`) | **BY-DESIGN** | C aborts the run (rc 255, 0 frames); Rust emits the pictures that did decode. Matching C would discard valid output. |
| VP8 key-frame scale bits | **C-BUG** (candidate) | C parses the bits into fields no register reads and decodes at full size; Rust refuses the stream. Needs a libvpx check on a stream that actually uses non-unit scale before the label is final. |
| AV1 wider conformance / VP9 profile edge matrix | **NO-ORACLE** | Fixtures not in the corpus; only the listed streams are proven. |
| H.263 field-coded PAFF | **NO-ORACLE** | No stream in the corpus. |
| AVS+ / AVS2 | **NO-ORACLE** | No conformance stream and no hardware on t6. |

## Encoders

| Case | Disposition | Evidence |
|---|---|---|
| H.264 slice split, output packets (`MPP_ENC_SPLIT_OUT_LOWDELAY`) | NOT IMPLEMENTED | Slice batching is byte-identical with `split_out=0`; no per-slice packet API yet. Frozen with the feature frontier. |
| H.264 P pictures above 4096 width | **NO-ORACLE** so far | Only IDR cells captured at those widths; needs a P capture before claiming. |
| H.265 GDR / intra refresh | **NO-ORACLE** | The 2024-04 vintage cannot be made to emit `is_i_refresh` through the public config (no ref cfg in the probe path); the crate's table path is opt-in and unverified. |
| H.265 tiles/WPP/SAO, B frames, multi-reference | NOT IMPLEMENTED | Frozen with the feature frontier. |

## Post-processing

| Case | Disposition | Evidence |
|---|---|---|
| IEP2 deinterlace, 720x480 I5O2 | **FIX** | Both sides produce two frames; 1,035,360 of 1,036,800 bytes differ. C pairs fields within the same input frame via motion history on this fixture; the crate pairs prev/cur. This is the next pixel-exactness target. |
| VDPP (client 29) | **NO-ORACLE** | The engine does not exist on RK3588; M9's original engine cannot be imitated on this board. |

## Framework

| Case | Disposition | Evidence |
|---|---|---|
| fences, multi-core CCU, secure mode | NOT IMPLEMENTED | Surveyed in `docs/framework-m10.md`; blocked on this kernel/board or on a TA. |
| rkvenc multi-instance | NOT IMPLEMENTED | The reference itself supports one live session per process here. |
