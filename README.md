# rk-mpp-rs

用纯 Rust 重写 Rockchip **MPP**：直接驱动 `/dev/mpp_service`，不链接
`librockchip_mpp`、不依赖 `librga`。目标是覆盖 `refs/mpp` 的完整表面——
所有解码器、编码器、后处理与 framework——并保证**与 C MPP 逐字节相同**、
性能不劣于 C。

起源是 `rknn-fastinfer` 需要一条无 C 依赖的 JPEG 解码 + 缩放链路；现在范围已经是
整个库。当前进度与逐项证据见 [ROADMAP.md](ROADMAP.md)，接手指南见
[docs/HANDOFF.md](docs/HANDOFF.md)。

## 现状

| 能力 | 状态 |
|---|---|
| `mpp_service` 客户端（全部 `MPP_CMD_*`、MULTI_MSG、RCB、IOVA、poll、probe） | 完成 |
| JPEG 解码（jpegd，RGB888/NV12，1/2–1/8 缩放） | 完成，比 C 快约 30% |
| H.264 解码（rkvdec） | 完成：frame/MBAFF/PAFF/长参考/错误路径**逐字节** |
| H.265 解码（rkvdec） | 完成：Main/Main10/tiles/WPP/DSLICE/LTR/scaling list；4:2:2/4:4:4 被硬件拒收 |
| VP9 解码（rkvdec） | profile 0/2、lossless、tiles、error-resilient 逐字节；边界矩阵未完 |
| AV1 解码（av1d） | Main 8/10-bit、film grain 逐字节；更宽 conformance 未完 |
| MPEG-2 / MPEG-4 / H.263 / VP8 解码（vdpu2） | 全部逐字节；AVS+/AVS2 未做 |
| H.264 编码（VEPU580） | 固定 QP、CBR、VBR 与 C **逐 NAL 相同**；AVBR/ROI/GDR/slice split/>4096 未完 |
| JPEG 编码（vepu） | baseline 4:2:0（NV12 / YUYV / UYVY）完成 |
| H.265 编码 | 未开始 |
| 后处理 | 自研 RGB 缩放/letterbox（比 RGA 快 2.9–3.8×）；IEP 未移植，VDPP 该板不存在 |
| framework | 仅同步 submit+poll；异步队列/fence/多核/secure 未做 |

整体约完成一半（按 ROADMAP 各里程碑的工作量估加权）。ROADMAP 上 47 项已勾、18 项未勾。

## 怎么验证的

所有"完成"都对应板上（NanoPC-T6，RK3588）源码编译的 C MPP：

- 解码器：与 `mpi_dec_test` 的输出**逐字节**相同（NV12→planar 换算后）。
- 编码器：与 C 编码器**逐 NAL**相同。固定 QP 覆盖 4 尺寸 × 3 profile × 2 QP = 24 格，
  加 6 格 GOP（1 IDR + 1/3/5 P，静止/运动）；CBR 与 VBR 各 6 帧 10/10 NAL 相同。
- 码率控制：寄存器半边对 C 作业**逐字**相同；位分配与量化控制器对 C **自己打印的内部
  状态**逐帧相同（`rc_debug=0xf0`）。
- 性能：同一流、两侧外部计时、交错采样取中位数。

复跑：

```bash
cargo test --all-targets                     # 441 通过，11 个板级用例 ignore
ONLY=gop  ./scripts/board_h264_qp_vs_mpp.sh  # 固定 QP 6 格
./scripts/board_h264_cbr_vs_mpp.sh           # CBR + VBR
BENCH_FILTER=vp8 ./scripts/board_bench_vs_mpp.sh
```

## 性能

| 项目 | 结果 |
|---|---|
| 解码（21 条流，`scripts/board_bench_vs_mpp.sh`） | 全部快于 C：+55% … +738% |
| 编码（`scripts/board_bench_encode_vs_mpp.sh`） | 320p/480p/720p 快 8–30%；1088 行**平手**（不作胜利声明） |
| RGB letterbox（替换 RGA） | 1080p→640: 0.84 ms vs RGA 3.21 ms；1:1: 0.53 vs 1.50 |

## 用法

硬件 probe（无参数）：`rk-mpp-rs`

| 子命令 | 说明 |
|---|---|
| `decode <jpg> <out.rgb> [1\|2\|4\|8]` | JPEG 解码到 RGB（下采样） |
| `jpeg-info <jpg>` / `jpeg-enc <in.yuv> <out.jpg> <w> <h> [quant] [fmt] [nv12\|yuyv\|uyvy]` | JPEG 解析 / 编码 |
| `h264 <file>` / `h264dec <in.264> <out.yuv>` | H.264 语法打印 / 解码 |
| `hevc` / `hevcdec <in.265> <out.yuv>` / `hevcpps` / `hevcdpb` | H.265 语法、PPS、DPB 打印与解码 |
| `av1dec <in.ivf> <out.yuv>` / `vp9dec <in.ivf> <out.yuv>` / `vp8dec <in.ivf> <out.yuv>` / `vp8` | AV1、VP9、VP8 |
| `m2v` / `m2vdec <in.m2v> <out.yuv>` / `mp4dec <in.m4v> <out.yuv>` / `h263dec <in.263> <out.yuv>` | MPEG-2 / MPEG-4 / H.263 |
| `h264-enc <in.nv12> <out.264> <w> <h> [qp] [profile] [frames] [gop] [cbr:\|vbr:bps:min:max]` | H.264 编码；给码率三元组即进入码率控制 |
| `spspps` / `h264dpb` | H.264 参数集与 DPB 打印 |
| `probe-frame` / `bench-scale` / `bench-decode` / `bench-dma` / `pipeline` | 夹具与基准 |

## 构建

```bash
cargo build --release                     # 宿主仅做类型检查
# 板上（aarch64）
COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
```

除 `libc` 外无第三方依赖。

## 布局

```
src/service/      mpp_service ioctl 客户端、dma-heap、batch、poll、probe
src/codec/        各格式语法解析（h264 hevc vp9 av1 m2v mpg4 h263 vp8 jpeg）
src/codec/rc/     码率控制移植（MppDataV2 + rc_model_v2 的 CBR/VBR）
src/hal/          各硬件块寄存器生成（h264d hevc av1d vp9d vp8d jpegd jpege h264e）
src/scale.rs      RGB 缩放 / letterbox（替代 RGA）
src/reference/    自研软件参考解码器（M11）
scripts/          板上回归、吞吐、取证脚本
tools/            寄存器字段生成器、ioctl tracer、C 侧探针、diff 工具
tests/data/       C 侧真实输出构成的 golden / fixture
```

## 文档

- [ROADMAP.md](ROADMAP.md) —— 逐 capability 的进度台账与证据（权威）
- [docs/PLAN.md](docs/PLAN.md) —— 完整未来计划：全部剩余工作、阶段出口、依赖与风险
- [docs/HANDOFF.md](docs/HANDOFF.md) —— 交接：下一步、板子工具链、易错清单
- [docs/h264d_hw.md](docs/h264d_hw.md) —— rkvdec H.264 硬件契约（从 C 提取）
- [docs/superpowers/](docs/superpowers/) —— 历史计划与设计稿（带状态说明）

## 已知限制

- 板上参考实现与 tracer 放在 `/tmp`（`/tmp/mpp-old`、`/tmp/mpp-develop`、
  `/tmp/mpp_trace.so`），重启即失，重建步骤见 HANDOFF。
- rkvenc 每进程只支持一个活会话，多实例需先解决（framework 部分）。
- AVS+/AVS2 无 conformance 流/硬件支持，未实现；损坏码流一致性只测了退出码。
