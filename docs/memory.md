# t6 memory ledger

Generated from `scripts/board_memory_profile.sh` on NanoPC-T6-LTS, kernel
6.1.57. Each row is the maximum of three runs.

| codec | fixture | peak process RSS | peak new dma-buf |
|---|---|---:|---:|
| H.264 decode | `test720.264` | 43,180 KiB | 164,906 KiB |
| H.265 decode | `t720.hevc` | 14,900 KiB | 193,029 KiB |
| VP9 decode | `vp90_720.ivf` | 19,992 KiB | 148,738 KiB |
| AV1 decode | `av1_320x240_10.ivf` | 3,288 KiB | not observed |
| MPEG-2 decode | `m2v_320x240_12.m2v` | 2,680 KiB | not observed |
| MPEG-4 decode | `mp4_320x240_12_ip.m4v` | 2,612 KiB | not observed |
| H.263 decode | `h263_cif_12_ip.263` | 2,748 KiB | not observed |
| VP8 decode | `vp8_320x240_12_ip.ivf` | 2,672 KiB | not observed |
| H.264 encode | `1280x720.nv12` | 3,572 KiB | not observed |
| H.265 encode | `1280x720.nv12` | 2,820 KiB | not observed |

Method:

- `VmHWM` in `/proc/<pid>/status` is sampled at 1 ms while the release binary
  is running.
- New dma-buf bytes are sampled from
  `/sys/kernel/debug/dma_buf/bufinfo`. This measures objects created while the
  process is alive relative to the system-wide baseline.
- CPU RSS and dma-buf are intentionally separate: dma-buf pages are not part
  of the process RSS and must never be added as if they were resident twice.

Caveats:

- Tiny 320x240 decoders and the one-frame encoder cases create/free their
  buffers faster than the debugfs sampling loop can reliably observe, so their
  dma-buf column is `not observed`, not proven zero.
- The dma-buf delta is system-wide, not charged by inode to the PID. Re-run on
  an otherwise idle board for publication-quality comparisons.
- The H.264/H.265 encoder allocation budget is fixed at session `open`; no
  per-frame alloc/mmap occurs after that, as recorded by W8-3.
