# t6 memory ledger

Generated from `scripts/board_memory_profile.sh` on NanoPC-T6-LTS, kernel
6.1.57. Each row is the maximum of three runs.

| codec | fixture | peak process RSS | peak new dma-buf |
|---|---|---:|---:|
| H.264 decode | `test720.264` | 42,436 KiB | 164,906 KiB |
| H.265 decode | `t720.hevc` | 4,908 KiB | 254,200 KiB |
| VP9 decode | `vp90_720.ivf` | 19,780 KiB | 148,738 KiB |
| AV1 decode | `av1_320x240_10.ivf` | 3,276 KiB | not observed |
| MPEG-2 decode | `m2v_320x240_12.m2v` | 2,588 KiB | not observed |
| MPEG-4 decode | `mp4_320x240_12_ip.m4v` | 2,672 KiB | not observed |
| H.263 decode | `h263_cif_12_ip.263` | 2,468 KiB | not observed |
| VP8 decode | `vp8_320x240_12_ip.ivf` | 2,388 KiB | not observed |
| H.264 encode | `1280x720x6.nv12` | 3,712 KiB | 82,176 KiB |
| H.265 encode | `1280x720x6.nv12` | 10,436 KiB | 86,570 KiB |

Method:

- `VmHWM` in `/proc/<pid>/status` is sampled at 1 ms while the release binary
  is running.
- New dma-buf bytes are sampled from
  `/sys/kernel/debug/dma_buf/bufinfo`. This measures objects created while the
  process is alive relative to the system-wide baseline.
- CPU RSS and dma-buf are intentionally separate: dma-buf pages are not part
  of the process RSS and must never be added as if they were resident twice.

Caveats:

- Tiny 320x240 decoders create/free their buffers faster than the debugfs
  sampling loop can reliably observe, so their dma-buf column is `not
  observed`, not proven zero. Encoders use six repeated input frames to widen
  the observation window.
- The dma-buf delta is system-wide, not charged by inode to the PID. Re-run on
  an otherwise idle board for publication-quality comparisons.
- The H.264/H.265 encoder allocation budget is fixed at session `open`; no
  per-frame alloc/mmap occurs after that, as recorded by W8-3.
