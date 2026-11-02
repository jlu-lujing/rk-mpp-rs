# rk-mpp-rs

用纯 Rust 重写 Rockchip **MPP**：直接驱动 `/dev/mpp_service`，不链接
`librockchip_mpp`、不依赖 `librga`。目标是覆盖 `refs/mpp` 的完整表面——
所有解码器、编码器、后处理与 framework——并保证**与 C MPP 逐字节相同**、
性能不劣于 C。

起源是 `rknn-fastinfer` 需要一条无 C 依赖的 JPEG 解码 + 缩放链路；现在范围已经是
整个库。当前进度与逐项证据见 [ROADMAP.md](ROADMAP.md)，接手指南见
[docs/HANDOFF.md](docs/HANDOFF.md)。

## 验证范围（重要）

**本项目只对 RK3588（NanoPC-T6-LTS）做验证与保证。**

- 所有"完成 / 逐字节一致 / 性能"结论都只在这块板子上成立，且只针对它暴露的硬件块：
  client 1（vdpu2）、4（av1d）、9（rkvdec）、13（jpegd）、16（rkv-enc）、
  19（vepu JPEG）、28（IEP）。client 12（AVS+）能绑定但尚无 conformance 流，
  因此**未实现、不宣称**；client 3（vdpu2-pp）会绑定，但本源码里没有任何解码器
  驱动它（见 ROADMAP 的能力表）。
- **其他 Rockchip SoC 未验证。** 代码不把板型写死（有 `probe` 能力探测，HAL 按硬件块分层），
  因此可能在别的板子上编译甚至运行，但那不属于承诺范围：不同代的 VEPU/VDEC
  寄存器布局、STATUS 语义和固件行为都不同，需要各自取证。
- 本板不存在的硬件按事实记录，不做假装支持：VDPP（client 29）在该 SoC 上不可用；
  rkvdec 拒收 H.265 4:2:2 / 4:4:4。
- 未来扩展到其他 SoC 的工作与成本见 [docs/PLAN.md](docs/PLAN.md) 的 W12 与决策 D2；
  当前决定是 **先只保 RK3588**。

## 现状

| 能力 | 状态 |
|---|---|
| `mpp_service` 客户端（全部 `MPP_CMD_*`、MULTI_MSG、RCB、IOVA、poll、probe） | 完成 |
| JPEG 解码（jpegd，RGB888/NV12，1/2–1/8 缩放） | 完成，比 C 快约 30% |
| H.264 解码（rkvdec） | 完成：frame/MBAFF/PAFF/长参考/错误路径**逐字节** |
| H.265 解码（rkvdec） | 完成：Main/Main10/tiles/WPP/DSLICE/LTR/scaling list；4:2:2/4:4:4 被硬件拒收 |
| VP9 解码（rkvdec） | profile 0/2、lossless、tiles、error-resilient 逐字节；720p 分割流经 libvpx 三方判定为 **C 侧 bug**（Rust == libvpx，C 差 213 字节），321x241 亦为 C 丢帧；仅损坏流中止属刻意保留 |
| AV1 解码（av1d） | Main 8/10-bit、film grain 逐字节；更宽 conformance 未完 |
| MPEG-2 / MPEG-4 / H.263 / VP8 解码（vdpu2） | 全部逐字节；VP8 profile 1/3（板验，语料仅 p0 故派生流）；损坏/截断流与 C 的行为表 4/6 SAME（scripts/board_vp_corrupt_vs_mpp.sh）；AVS+/AVS2 未做 |
| H.264 编码（VEPU580） | 固定 QP 与 CBR / VBR / AVBR 三种码率控制都与 C **逐 NAL 相同**；level（1.0–5.2 含 bump 规则）、deblock 细调、ROI/QP map（逐块 DMA 表）、auto 起始 QP、场景检测均**板上逐字节**；GDR **板上 4/4 逐字节**（含 `refresh_num == 1` 时 band 起点经 RK_U16 截断成 0xFFF0 的角落），slice split **板上 4/4 逐字节**（BY_CTU 96/8/150 与 BY_BYTE 256；8 块那格 38 个 slice 越过 32 项 slice FIFO，同时钉住 `slc_done_en`），>4096 的 CIME 扩展行 **板上 2/2 逐字节**（4160x64、4608x256）。P 帧另有 11 个尺寸的逐字节扫描（`scripts/board_h264_p_size_probe.sh`，含 FBC 阈值两侧）。编码侧剩下的是**逐 slice 输出路径**（`slen_fifo` / low-delay slice 包）与 >4096 宽度下的 P 格 |
| JPEG 编码（vepu） | baseline 4:2:0（NV12 / YUYV / UYVY）+ restart 间隔、90°/270° 旋转、RGB565/555/444 与 8888 系输入全部板上逐字节；low-delay slices / 多核分带见 docs/jpege-slices-multicore.md |
| H.265 编码 | 固定 QP 的 **IDR + P GOP**（最多 5 帧、含动画面）与 **CBR/VBR/AVBR**（模型自选 QP）均**板上整条逐字节**；VI/refresh 调度、tiles/WPP/SAO、B 帧未做 |
| 后处理 | 自研 RGB 缩放/letterbox（比 RGA 快 2.9–3.8×）；IEP 未移植，VDPP 该板不存在 |
| framework | 同步 submit+poll 之外新增：异步任务队列（回调/等待/事件）、引用计数 dma-heap 缓冲池；fence/多核/secure 未做（调研见 docs/framework-m10.md） |

整体约完成一半（按 ROADMAP 各里程碑的工作量估加权）。ROADMAP 上 75 项已勾、20 项未勾。

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
| `async-demo <jpg> [jobs] [workers] [out.rgb]` | 异步任务队列 + 缓冲池跑 JPEG 解码，逐帧与同步路径比字节 |

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
- [docs/DIVERGENCES.md](docs/DIVERGENCES.md) —— 已知与 C 的差异登记：必须修 / 已证 C bug / 无 oracle
- [docs/HANDOFF.md](docs/HANDOFF.md) —— 交接：下一步、板子工具链、易错清单
- [docs/h264d_hw.md](docs/h264d_hw.md) —— rkvdec H.264 硬件契约（从 C 提取）
- [docs/superpowers/](docs/superpowers/) —— 历史计划与设计稿（带状态说明）

## 已知限制

- 板上的 C 参考实现与 tracer 是工作副本（`/tmp/mpp-old`、`/tmp/mpp-develop`、
  `/tmp/mpp_trace.so`）。t6 的 `/tmp` 实测**不是 tmpfs**（根是 overlay，
  `upperdir=/data/root`），但它归 `systemd-tmpfiles` 管（`D /tmp 1777 root root -`
  且清理 timer 在跑），属于会被清的位置。权威副本已放在板上的
  `~/rk-mpp-oracles`（含编解码 oracle、tracer 与 438 个流/参考输出），
  `scripts/setup_board_refs.sh` 幂等检测并回填（已用"把 tracer 藏起来再跑一次"验证过恢复路径）。
- rkvenc 每进程只支持一个活会话，多实例需先解决（framework 部分）。
- AVS+/AVS2 无 conformance 流/硬件支持，未实现；损坏码流一致性只测了退出码。
- 只验证 RK3588；其他 SoC、其他内核/固件版本的行为未测（见上文"验证范围"）。
- 验证本身的坑（已踩过一次）：**"每格只编一张 IDR"的尺寸矩阵看不见任何参考路径上的 bug**。
  曾经有一段时间 24 格矩阵在四个尺寸（含 1280x720）全绿，而 P 帧在 720p 每帧多花约 38% 位 ——
  根因是 FBC 体偏移写死 8192，而 `align(w·h/64, 8K)` 在编码面积超过 524288 像素后是 16384，
  参考面因此被从错误位置读取；IDR 不读参考，所以完全无感。现在 P 的证明在
  `scripts/board_h264_p_size_probe.sh`（11 个尺寸，阈值两侧各有格子），它应当并进矩阵，而不是与矩阵平行存在。
- 证据分级：**H.265 编码**目前只有宿主级字段/单测证据——板级脚本
  已就位，但结果没有写进台账，所以不按"已逐字节验证"记账。一条特性从"代码写完"到
  "已验证"之间隔着的正是这一次板级运行。
- IEP2 去隔行**已跑通硬件**：720x480 一个 I5O2 作业，两边都出 2 帧、尺寸相同（session、
  寄存器读写、`POLL_HW_FINISH` 全部工作）。但像素与 C `iep2_test` 不同 —— 1,036,800 字节里
  差 1,035,360。分歧形状是：C 每帧所有行来自同一输入帧（首帧甚至不含输入 0），我们按
  prev/cur 混场 ⇒ C 的 I5O2 依 motion-history 在此夹具上选了同帧配对。要逐字节就得把那个
  配对决策做出来，不是改寄存器。
