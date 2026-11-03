# rk-mpp-rs

用纯 Rust 重写 Rockchip **MPP**：直接驱动 `/dev/mpp_service`，不链接
`librockchip_mpp`、不依赖 `librga`。目标是覆盖 `refs/mpp` 的完整表面——
所有解码器、编码器、后处理与 framework——并保证**与 C MPP 逐字节相同**、
性能不劣于 C。

起源是 `rknn-fastinfer` 需要一条无 C 依赖的 JPEG 解码 + 缩放链路；现在范围已经是
整个库。当前进度与逐项证据见 [ROADMAP.md](ROADMAP.md)，接手指南见
[docs/HANDOFF.md](docs/HANDOFF.md)。

## 验证范围（重要）

**本项目只对 RK3588（NanoPC-T6-LTS；当前板为 t6）做验证与保证。**

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
| AV1 解码（av1d） | Main 8/10-bit、film grain 逐字节；t62/6.1.141 的 2x2 tile fixture 已修复 filter-base 缺失，10 帧输出与 C 逐字节一致；更宽 conformance 未完 |
| MPEG-2 / MPEG-4 / H.263 / VP8 解码（vdpu2） | 渐进 MPEG-2、MPEG-4、H.263、VP8 逐字节；隔行 MPEG-2 使用 FFmpeg 为 oracle（C 会错误回绕输入）；VP8 profile 1/3（板验，语料仅 p0 故派生流）；AVS+/AVS2 未做 |
| H.264 编码（VEPU580） | 固定 QP 与 CBR / VBR / AVBR 三种码率控制都与 C **逐 NAL 相同**；level（1.0–5.2 含 bump 规则）、deblock 细调、ROI/QP map（逐块 DMA 表）、auto 起始 QP、场景检测均**板上逐字节**；GDR **板上 4/4 逐字节**（含 `refresh_num == 1` 时 band 起点经 RK_U16 截断成 0xFFF0 的角落），slice split **板上 4/4 逐字节**（BY_CTU 96/8/150 与 BY_BYTE 256；8 块那格 38 个 slice 越过 32 项 slice FIFO，同时钉住 `slc_done_en`）。逐 slice 输出也已接通：`MPP_ENC_SPLIT_OUT_LOWDELAY` 的 BY_CTU 96/150/8、BY_BYTE 256 和 6 帧 IDR+P **5/5** 通过，32 项 FIFO 之外仍是 38 个数据 packet，所有实际 slice 边界逐项相同。>4096 的 CIME 扩展行与 P 帧 **板上逐字节**（4160x64、4608x256）。P 帧另有 11 个尺寸的扫描（`scripts/board_h264_p_size_probe.sh`，含 FBC 阈值两侧）。 |
| JPEG 编码（vepu） | baseline 4:2:0（NV12 / YUYV / UYVY）+ restart 间隔、90°/270° 旋转、RGB565/555/444 与 8888 系输入全部板上逐字节；low-delay slices 的 5-packet 与 10-packet 矩阵**逐 packet 一致**。RK3588 默认四 band 多核路径已实现，1920x1080/1288x722、旋转 90/270 以及 322x242/1366x768 stride 矩阵**逐字节** |
| H.265 编码 | 固定 QP 的 **IDR + P GOP**（最多 5 帧、含动画面）、**CBR/VBR/AVBR**、**GDR/intra-refresh** 和 **ROI/QP map** 均**板上整条逐字节**；GDR 覆盖 320x240/640x480 × row/column 4/4，ROI 覆盖相对/绝对 QP、force-intra、多 region 与 640x480 4/4。BY_CTU/BY_BYTE slice split 普通整 AU **3/3**；`MPP_ENC_SPLIT_OUT_LOWDELAY` 为 **4/4 逐字节、逐 packet 一致**，覆盖 320x240、640x480 和 6 帧 IDR+P，其中 40-slice 格跨越 32 项 FIFO。tiles/WPP/SAO、B 帧和多参考仍未做 |
| 后处理 | 自研 RGB 缩放/letterbox（比 RGA 快 2.9–3.8×）；IEP2 I5O2/I5O1T/I5O1B/I2O2/I1O1T/I1O1B 及 DISABLE/BYPASS 共 **9/9 原始 dst slot 逐字节**，含 TFF/BFF 与 1280×720。420/422 × SP/Planar × UV/VU 格式矩阵 **6/6**。client 28 参数块只有 deinterlace，legacy `/dev/iep` scale 节点在 t6 不存在，VDPP 也不存在 |
| framework | 同步 submit+poll 之外新增：异步任务队列（回调/等待/事件）、引用计数 dma-heap 缓冲池；fence/多核/secure 未做（调研见 docs/framework-m10.md） |

整体约完成 72%（按 ROADMAP 各里程碑的工作量估加权；M11 软件参考解码器约 83%）。
ROADMAP 顶层清单当前为
71 项完成、10 项未完成；未完成项里有硬件不存在、无 conformance 流和冻结前沿，
不宜直接把勾选比例当作代码工作量。

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
cargo test --all-targets                     # 732 通过，26 个板级用例 ignore
ONLY=gop  ./scripts/board_h264_qp_vs_mpp.sh  # 固定 QP 6 格
./scripts/board_h264_cbr_vs_mpp.sh           # CBR + VBR
./scripts/board_h264_slice_vs_mpp.sh         # 普通整 AU：4/4
./scripts/board_h264_lowdelay_vs_mpp.sh      # 逐 slice packet：5/5
./scripts/board_jpege_lowdelay_vs_mpp.sh      # JPEG 逐 band packet：2/2
./scripts/board_jpege_multicore_vs_mpp.sh     # JPEG 多核：5/5
./scripts/board_jpege_stride_vs_mpp.sh        # JPEG stride：2/2
./scripts/board_h265_gdr_vs_mpp.sh           # H.265 GDR：4/4
./scripts/board_h265_slice_vs_mpp.sh         # H.265 普通整 AU slice：3/3
./scripts/board_h265_lowdelay_vs_mpp.sh      # H.265 逐 slice packet：4/4
./scripts/board_h265_roi_vs_mpp.sh           # H.265 ROI/QP map：4/4
./scripts/board_iep_modes_vs_probe.sh        # IEP2 modes + raw slots：9/9
./scripts/board_iep_formats_vs_probe.sh      # IEP2 420/422 SP/Planar：6/6
BENCH_FILTER=vp8 ./scripts/board_bench_vs_mpp.sh
```

## 性能

| 项目 | 结果 |
|---|---|
| 解码 / 编码 | 当前证据只支持“**不比 C 慢**”。同一二进制在 t62 的 720p H.264 结果可从 72.9 ms 漂到 120.1 ms；旧的高速百分比是未做交错采样时的高噪声样本，不再作为结论 |
| RGB letterbox（替换 RGA） | 1080p→640: 0.84 ms vs RGA 3.21 ms；1:1: 0.53 vs 1.50 |

## 用法

硬件 probe（无参数）：`rk-mpp-rs`

| 子命令 | 说明 |
|---|---|
| `decode <jpg> <out.rgb> [1\|2\|4\|8]` | JPEG 解码到 RGB（下采样） |
| `jpeg-info <jpg>` / `jpeg-enc <in.yuv> <out.jpg> <w> <h> [quant] [fmt] [input] [rst<mcus>] [splitout:lowdelay]` | JPEG 解析 / 编码与逐 restart band packet |
| `h264 <file>` / `h264dec <in.264> <out.yuv>` | H.264 语法打印 / 解码 |
| `hevc` / `hevcdec <in.265> <out.yuv>` / `hevcpps` / `hevcdpb` | H.265 语法、PPS、DPB 打印与解码 |
| `av1dec <in.ivf> <out.yuv>` / `vp9dec <in.ivf> <out.yuv>` / `vp8dec <in.ivf> <out.yuv>` / `vp8` | AV1、VP9、VP8 |
| `m2v` / `m2vdec <in.m2v> <out.yuv>` / `mp4dec <in.m4v> <out.yuv>` / `h263dec <in.263> <out.yuv>` | MPEG-2 / MPEG-4 / H.263 |
| `h264-enc <in.nv12> <out.264> <w> <h> [qp] [profile] [frames] [gop] [cbr:\|vbr:bps:min:max] ... [splitout:lowdelay]` | H.264 编码；给码率三元组即进入码率控制，低延迟模式逐 slice 输出 packet |
| `h265-idr ... [roi:x:y:w:h:intra:abs:qp]... [split:ctu:\|split:byte:arg] [splitout:lowdelay]` / `h265-gop ... [shortterm] ...` | H.265 固定 QP IDR/GOP、GDR、ROI/QP map 与逐 slice packet 输出 |
| `iep-dei <in.yuv> <out.yuv> <w> <h> [TFF\|BFF] [I5O2\|I5O1T\|I5O1B\|I2O2\|I1O1T\|I1O1B]` | IEP2 去隔行；原始输出保留两个 dst slot |
| `spspps` / `h264dpb` | H.264 参数集与 DPB 打印 |
| `probe-frame` / `bench-scale` / `bench-decode` / `bench-dma` / `pipeline` | 夹具与基准 |
| `async-demo <jpg> [jobs] [workers] [out.rgb]` | 异步任务队列 + 缓冲池跑 JPEG 解码，逐帧与同步路径比字节 |

## 构建

```bash
cargo build --release                     # 宿主仅做类型检查
# 板上（aarch64）
BOARD=${BOARD:-t6}
COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
```

除 `libc` 外无第三方依赖。

## 布局

```
src/service/      mpp_service ioctl 客户端、dma-heap、batch、poll、probe
src/codec/        各格式语法解析（h264 hevc vp9 av1 m2v mpg4 h263 vp8 jpeg）
src/codec/rc/     码率控制移植（MppDataV2 + rc_model_v2 的 CBR/VBR）
src/hal/          各硬件块寄存器生成（h264d hevc av1d vp9d vp8d jpegd jpege h264e）
src/scale.rs      RGB 缩放 / letterbox（替代 RGA）
src/reference/    自研软件参考解码器（M11；CABAC、HEVC 变换、H.264 CAVLC/扫描/反量化/残差/DC）
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
- **H.265 编码**的 IDR、P GOP、CBR/VBR/AVBR 与 GDR 都已有板上整条码流逐字节证据。
- IEP2 去隔行**已逐字节**：720x480 I5O2 TFF 与 C `iep2_test` 的 1,036,800 字节完全相同。
  之前"像素不同"的根因不是场配对，而是提交批次最后一个请求多带了
  `MPP_FLAGS_REG_OFFSET_ALONE`，驱动没有真正执行作业（`iep2_output` 全 0）；IEP 走
  `submit_batch_plain`（只带 MULTI_MSG|LAST_MSG）后硬件开始工作，输出立即与 C 一致。
