# rk-mpp-rs

用纯 Rust 重写 Rockchip **MPP**：直接驱动 `/dev/mpp_service`，不链接
`librockchip_mpp`、不依赖 `librga`。目标是覆盖 `refs/mpp` 的完整表面——
所有解码器、编码器、后处理与 framework——并保证**与 C MPP 逐字节相同**、
性能不劣于 C。

起源是 `rknn-fastinfer` 需要一条无 C 依赖的 JPEG 解码 + 缩放链路；现在范围已经是
整个库。当前进度与逐项证据见 [ROADMAP.md](ROADMAP.md)，接手指南见
[docs/HANDOFF.md](docs/HANDOFF.md)。

## 验证范围（重要）
完整能力矩阵（每条结论的证据与边界）：[docs/CAPABILITY.md](docs/CAPABILITY.md)。


**本项目只对 RK3588（NanoPC-T6-LTS；当前板为 t6）做验证与保证。**

- 所有"完成 / 逐字节一致 / 性能"结论都只在这块板子上成立，且只针对它暴露的硬件块：
  client 1（vdpu2）、4（av1d）、9（rkvdec）、13（jpegd）、16（rkv-enc）、
  19（vepu JPEG）、28（IEP）。client 12（AVS+）能绑定但尚无 conformance 流，
  因此**未实现、不宣称**；client 3（vdpu2-pp）会绑定，但本源码里没有任何解码器
  驱动它（见 ROADMAP 的能力表）。
- **RK3576 正在适配，尚不属于完整保证范围。** 已确认
  VDPU383/VEPU510/VPU720/VDPP2，且 RK3576 H.264 640x480 12 帧已与 C MPP
  逐字节，720p/B、1080p、PAFF、LTR 和缺失参考边界矩阵 5/5 通过；VDPU383 H.265 初始
  12/12 矩阵（Main/Main10、B、tiles、真实 WPP、
  1280x720、JCT-VC DSLICE、SPS/PPS scaling list、500 帧 8-entry LTR
  SPS 流，以及删 AU 后 11 个输入 access unit 产出 5 帧的缺失参考路径）
  也已逐字节；VDPU383 VP9
  初始 7/7 矩阵（profile 0/2、8/10-bit、lossless、tiles）通过，两个既有 C
  bug 路径由 libvpx 判定；VPU720 JPEG 19/19（NV12 三尺寸+restart，320x240
  NV16/I420/NV21/YUV422P/NV61/YUYV/YVYU/UYVY/VYUY/NV24/YUV444P/YUV400，
  以及 400/420/422/444 四种输出）也与 C MPP 逐字节。H.265 随机错误注入、AV1、
  VPU720 tile input、VEPU510 编码、VDPP2 和 H.264 完整边界矩阵仍在推进。
  VEPU510 H.264 已生成 6 个写窗口、645 个字段的 Rust 寄存器表，并完成首个
  320x240 QP26 Baseline IDR、GOP2、6 帧固定 GOP、CBR6、VBR6 与 AVBR6
  硬件编码，分别与 C MPP **6479 / 8671 / 18225 / 14187 / 15616 / 12313
  bytes 逐字节一致**；ROI 强制 intra +绝对 QP 的两帧格，以及 BY_CTU=96 /
  BY_BYTE=256 的整 AU 和 LOWDELAY packet 两档也逐字节。BY_BYTE 低延迟格
  产生 40 个 packet，跨越 VEPU510 的 8 项 slice FIFO，码流与逐 packet
  长度均和 C 相同。GDR row=2/GOP4 与 col=4/GOP6 也逐字节；刷新帧抑制
  IGOP IDR、钳住 CIME 搜索轴，并在 recovery 帧重发 SPS/PPS。
  VEPU510 H.265 的首个 320x240 QP26 Main IDR 也已与 lf1 C MPP 逐字节，
  **4710 bytes**；IDR+P GOP2 与 GOP6 同样逐字节，分别为 **6810 / 16994
  bytes**；CBR6 / VBR6 / AVBR6 也逐字节，分别为 **11192 / 11795 / 9676
  bytes**；BY_CTU/BY_BYTE、LOWDELAY packet、row/column GDR 与 ROI 也都
  逐字节；10-bit 输入与 lf1 C MPP 一致，在构图前显式拒绝。RK3576 的 C
  `AUTO_TILE=1` 与普通 IDR 产生同一个 4710-byte 码流，确认 tiles 是该
  VEPU510 HAL 的平台边界；Rust 对多于 1 列的 tile 请求在 DMA/硬件分配前
  显式拒绝。VEPU510 H.265 功能/边界矩阵为 **15/15**。
  VDPU383 AV1 已落下第一层 C ABI：`src/hal/av1d383.rs` 生成的 672-byte
  global header 与 lf1 C dump 首帧逐字节一致（FNV
  `0x8da389d2e180d680`），首 keyframe 的 212-word C register struct image
  也与 C dump 完全一致；官方 7400×u32 的 29,600-byte default CDF 已生成并
  固定，首帧 RCB/CDF/segmentation/colmv buffer plan 已对齐。真实 VDPU383
  首 keyframe 已通过 Rust 提交，320×240 YUV420p 与 C 完全一致
  (`6f5b544f…b9fe`)。多帧/inter/10-bit 仍未接通。
  其他 Rockchip SoC 未验证：不同代的 VEPU/VDEC 寄存器布局、STATUS 语义和
  固件行为都不同，需要各自取证。详见 [docs/rk3576.md](docs/rk3576.md)。
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
| H.265 解码（rkvdec） | 完成：Main/Main10/tiles/WPP/DSLICE/LTR/scaling list；RK3576 另通过 16 格确定性损坏矩阵（SPS/PPS/SEI、IDR/P slice 起中尾、三种尾部截断）。14 格 C/Rust 输出计数与 YUV 逐字节一致；损坏 SPS 和首个 IDR slice 起点时 C harness 会无限等待，Rust 会立即结束并输出 0 帧。4:2:2/4:4:4 被硬件拒收 |
| VP9 解码（rkvdec） | profile 0/2、lossless、tiles、error-resilient 逐字节；720p 分割流经 libvpx 三方判定为 **C 侧 bug**（Rust == libvpx，C 差 213 字节），321x241 亦为 C 丢帧；仅损坏流中止属刻意保留 |
| AV1 解码（av1d） | Main 8/10-bit、film grain 逐字节；96x96 至 1920x1080（含 60 帧长 GOP、多 sequence、320↔640 分辨率切换）三方对齐 C 与 libaom 逐字节，64x64 为硬件不完成（C 输出全黑，已登记 C-bug）；t62/6.1.141 的 2x2 tile fixture 已修复 filter-base 缺失，10 帧输出与 C 逐字节一致。RK3576 VDPU383 已完成 8-bit 10 帧引用序列和 10-bit skip-grain 序列：`show_existing_frame`、八槽 CDF/colmv 生命周期、inter 主参考 coefficient-CDF 继承、704-byte 10-bit stride 与 packed-NV12 → planar u16 均已接通。10-bit 的 10 个显示帧共 2,304,000 bytes，与 C 逐字节，SHA-256 `f3b69b61a06d3967a89b91f11f56883588df53974d4a1b5efb8fbd9e004d49b8`；VDPU383 film grain 的 FGS enable/base 实验会以 STATUS 0 超时并复位，C HAL 也未启用该路径 |
| MPEG-2 / MPEG-4 / H.263 / VP8 解码（vdpu2） | 渐进 MPEG-2、MPEG-4、H.263、VP8 逐字节；隔行 MPEG-2 使用 FFmpeg 为 oracle（C 会错误回绕输入）；VP8 profile 1/3（板验，语料仅 p0 故派生流）；AVS+/AVS2 未做 |
| H.264 编码（VEPU580） | 固定 QP 与 CBR / VBR / AVBR 三种码率控制都与 C **逐 NAL 相同**；hier QP 的四层计数器和 1–4 QP 偏移在 CBR/VBR/AVBR 三格均**逐字节**；SMART_P 的 VI 分配、长期 IDR、参考重排和 2-QP delta 在 33 帧 × 三种 RC 下均为 37 NAL **逐字节**；同帧重编、super-frame re-encode、NORMAL drop、CAVLC/CABAC software PSKIP 和 two-pass deflicker 在强制过码格中达到最终码流和全部逐 job STATUS **一致**；level（1.0–5.2 含 bump 规则）、deblock 细调、ROI/QP map（逐块 DMA 表）、auto 起始 QP、场景检测均**板上逐字节**；GDR **板上 4/4 逐字节**（含 `refresh_num == 1` 时 band 起点经 RK_U16 截断成 0xFFF0 的角落），slice split **板上 4/4 逐字节**（BY_CTU 96/8/150 与 BY_BYTE 256；8 块那格 38 个 slice 越过 32 项 slice FIFO，同时钉住 `slc_done_en`）。逐 slice 输出也已接通：`MPP_ENC_SPLIT_OUT_LOWDELAY` 的 BY_CTU 96/150/8、BY_BYTE 256 和 6 帧 IDR+P **5/5** 通过，32 项 FIFO 之外仍是 38 个数据 packet，所有实际 slice 边界逐项相同。>4096 的 CIME 扩展行与 P 帧 **板上逐字节**（4160x64、4608x256）。P 帧另有 11 个尺寸的扫描（`scripts/board_h264_p_size_probe.sh`，含 FBC 阈值两侧）。 |
| JPEG 编码（vepu） | baseline 4:2:0（NV12 / YUYV / UYVY）+ restart 间隔、90°/270° 旋转、RGB565/555/444 与 8888 系输入全部板上逐字节；low-delay slices 的 5-packet 与 10-packet 矩阵**逐 packet 一致**。RK3588 默认四 band 多核路径已实现，1920x1080/1288x722、旋转 90/270 以及 322x242/1366x768 stride 矩阵**逐字节** |
| H.265 编码 | 固定 QP 的 **IDR + P GOP**（最多 5 帧、含动画面）、**CBR/VBR/AVBR**、**GDR/intra-refresh**、**ROI/QP map** 与 **deblock/SAO 细调** 均**板上整条逐字节**；GDR 覆盖 320x240/640x480 × row/column 4/4，ROI 覆盖相对/绝对 QP、force-intra、多 region 与 640x480 4/4，deblock beta/tc、disable、SAO luma/chroma 与 loop-filter 为 4/4。**水平 tiles** 为 **2/2 逐字节**（`auto_tile` 320x240 覆盖 IDR+P 共 9 NAL、4608x64 强制两列共 7 NAL），tile 作业的寄存器镜像与 C trace 除 dma-buf fd 外**无一个非地址字不同**；C 对 tile 首帧没有跨会话稳定结果（同参数连跑两次连解码像素都不同），两侧各先跑一趟丢弃作业后才可比，见 DIVERGENCES。BY_CTU/BY_BYTE slice split 普通整 AU **3/3**；`MPP_ENC_SPLIT_OUT_LOWDELAY` 为 **4/4 逐字节、逐 packet 一致**，覆盖 320x240、640x480 和 6 帧 IDR+P，其中 40-slice 格跨越 32 项 FIFO。吞吐 4/4 不慢于 C：320x240/640x480/720p Rust 快 15.8%/3.6%/13.8%，1080p 为 149 ms vs C 147 ms（慢 1.3%，平手门内）。10-bit 经 C 探针确认 t6 VEPU580 以 `int_sta=0x100` 拒绝、无 packet，Rust 显式拒绝。**WPP、B 帧与多参考在 C 参考里就不存在**（`m_entropyCodingSyncEnabledFlag` 恒为 0、编码层无 `B_FRAME`、`num_ref = 1`），没有可重写的目标；tiles 与逐 slice low-delay 输出的组合被显式拒绝（C 侧行为未证）；harness 的 `-g 60`（解析成 `gop_len=0` 的病态 smart-GOP）按 BY-DESIGN 拒绝，有效 GOP 形式逐字节 |
| 后处理 | 自研 RGB 缩放/letterbox（比 RGA 快 2.9–3.8×）；新增 strided NV12 直缩，直接从 CPU/DMA Frame 读取 Y/UV 并在缩放中转换 RGB，不再生成整帧中间副本。1080p→640 在 t6/lf1 为 4.93/5.95 ms，对应旧“NV12→RGB + scale”为 20.32/24.36 ms。IEP2 的 **PD/DECT 逆 telecine** 两遍路径（反向下拉 + 场序状态机）已接入：3:2 telecine 与逐行素材 **2/2 与 C probe 逐字节一致**，逐帧 `pd_flag/pd_types/dil_order/frm_mode/ratio` 也逐行相同，322x242/722x482/1366x768 等非 16 对齐尺寸同样逐字节；IEP2 I5O2/I5O1T/I5O1B/I2O2/I1O1T/I1O1B 及 DISABLE/BYPASS 共 **9/9 原始 dst slot 逐字节**，含 TFF/BFF 与 1280×720。420/422 × SP/Planar × UV/VU 格式矩阵 **6/6**。client 28 参数块只有 deinterlace，legacy `/dev/iep` scale 节点在 t6 不存在，VDPP 也不存在 |
| framework | 同步 submit+poll 之外新增：异步任务队列（回调/等待/事件、排队/执行/端到端耗时与成功/失败/取消累计统计）、引用计数 dma-heap 缓冲池；pooled JPEG 保证先关闭 MPP service 再归还共享 dma-buf，t6 连续 3000 jobs 压测通过；VP8 与 H.264/HEVC/AV1/VP9 的 DPB surface 已持有 RAII lease，四类解码边界回归在 t6 通过；fence/多核/secure 未做（调研见 docs/framework-m10.md） |

整体约完成 89%（按 ROADMAP 各里程碑的工作量估加权；M11 软件参考解码器约 90%）。
ROADMAP 顶层清单当前为
105 项完成、6 项未完成（含缩进子项为 143 / 10）；未完成项里有硬件不存在、无 conformance
流和冻结前沿，不宜直接把勾选比例当作代码工作量。M11 自研软件参考解码器已推进到 103 个
AV1 slice。第 103 slice 闭合 2×2 多 tile inter：10 帧全部走完四个独立 tile，
帧级 mi/segment 状态跨 tile 携带而邻块上下文按 tile 裁剪；同时修正 temporal motion-field
的 intra-cell 清除、base-invalid 时仍执行 extension probes、order-hint sign bias、
overlapping half-res block 清理和 4:2:0 difference-weighted mask 的 luma 域取样。
全部显示帧 Y/U/V 与 `aomdec` **逐字节一致**。第 102 slice 把它推进到真正的多 tile：一次 tile group 里每个 tile 都从帧初始
CDF 重新开始（对应 libaom 的 `tile_data->tctx = *cm->fc`），帧末 CDF 取
`context_update_tile_id` 那个 tile，而 intra 邻块可用性按 **tile 边界**裁剪
（`up_available = mi_row > tile->mi_row_start`）。真实 2×2 tile keyframe 的四个 tile 各自
走完自己的码流（逐 tile `exit_symbol` 校验），三平面与 `aomdec` **逐字节一致**。
第 101 slice 用真实单色 libaom keyframe 闭合 mono multi-LF：Rust 不再错误
读取 UV mode/残差，deblock 只建立单平面网格，最终 Y 与 `aomdec` **逐字节一致**。
第 100 slice 用真实单帧 libaom keyframe 闭合彩色 `delta_lf_multi`：帧头明确为
`delta_q=1 / delta_lf=1 / multi=1`，Rust 的 pre/deblock/CDEF/LR 三平面与未修改的
`aomdec` 最终 YUV **逐字节一致**。第 99 slice 将 luma/U/V 四路切换为 libaom 的四行
`default_delta_lf_multi_cdf`，单色流只消费两路主动 CDF。第 98 slice 修正混合
intra/inter frame 的 selected tx-size context：相邻 inter
block 的宽高必须覆盖 transform-context，不能在非 skip 时退回邻居 tx extent。原先卡在
tile 尾部的 `av1_lr_320x240.ivf` 现已完成 key+P 全熵解码，458 个 footprint block
中的 356 个 inter block 及两帧 Y/U/V 均与 libaom 逐字节一致。第 97 slice 完成 inter segmentation：key/inter frame 均支持 segment-id
空间预测、temporal segment-map prediction、segment 强制 reference/skip/global MV，以及
`ALT_Q` block qindex、`ALT_LF` 和 lossless 传播。三帧隔离流的 key 与 P 帧覆盖 1050 个
非零 segment block，Y/U/V 与 libaom **逐字节一致**。第 96 slice 完成 temporal MV fields：参考帧保存 8×8 motion field，按 order
hint 投影到当前帧，并在 spatial nearest/outer scans 之间合并 temporal candidates；
global-mode context 同步接入。三帧隔离 fixture 覆盖 128 个 inter block，Y/U/V 与 libaom
**逐字节一致**。第 95 slice 完成 compound global warp：两个 reference 各自保留矩阵/warp
状态，distance-weighted 与 wedge compound 可分别 warp 或平移，并复用 compound round-1
中间精度。四帧隔离流含 3 个 `GLOBAL_GLOBALMV` block，Y/U/V 与 libaom **逐字节一致**。
第 94 slice 完成单参考 global warped motion：按块中心从 `ROTZOOM/AFFINE` 矩阵求
`GLOBALMV`、复用 affine/shear 与 193-phase warp filter，并处理 AOM 对负 phase 的
非 signed rounding。两帧 `ROTZOOM` 隔离 fixture 含 756 个 global-warp block，Y/U/V 与
libaom **逐字节一致**。第 93 slice 完成
local warped motion：从同参考的单参考邻块中心 MV 收集
least-squares 样本，按当前 MV 过滤异常点，推导 affine 矩阵与 shear 参数，并用 libaom
的 193-phase 8-tap warp filter 生成 Y/UV 预测。五帧隔离 fixture 含 10 个
`WARPED_CAUSAL` block，Y/U/V 与 libaom **逐字节一致**。第 92 slice 完成 OBMC：按 above-first/left-second
扫描可重叠邻块，读取 `use_obmc_cdf`，用邻块单参考 MV/滤波生成当前块边缘预测，再按
normative 1–64 长度 mask 做 above/left 顺序混合；五帧 fixture 覆盖 32 个 OBMC block、
232 个 OBMC mipixel，全部 Y/U/V 与 libaom 逐字节一致。第 91 slice 完成 inter-intra：在单参考 MV 后读取 inter-intra
标志、DC/V/H/Smooth 模式、可选 wedge 与 index，并用当前帧邻域 intra 预测和 normative
smooth/wedge mask 与 inter 预测混合；smooth 与 wedge 两组 fixture 合计覆盖 24 个
inter-intra block，Y/U/V 全部逐字节一致。
第 90 slice 完成 distance-weighted compound：按 frame order hint 选择
`quant_dist_weight`/`quant_dist_lookup_table`，复刻 libaom 的 `fwd_offset`/`bck_offset`
分配与 4-bit 加权、round offset 和最终 4-bit 舍入。新增五帧 320×240 隔离 fixture，
覆盖 32×16 与 8×8 距离块、权重 `[11,5]` 和 `[13,3]`；全部 Y/U/V 与 libaom **逐字节一致**。
warp/OBMC、temporal MV 和 inter segmentation 仍未完成。第 89 slice
完成 wedge compound：六张 64×64 master mask、9 种 block codebook、
sign flip、luma/chroma 采样和 masked 16-bit blend 均接入；同时补齐
regular/smooth/sharp/bilinear 四种 switchable interpolation filter。五帧 wedge fixture
的全部 Y/U/V 与 libaom **逐字节一致**。第 88 slice 接入 compound mode group、masked compound type 和
difference-weighted prediction：`comp_group_idx` 邻域上下文、`compound_type_cdf`、
mask type、luma difference mask 及 16-bit masked blend 均按 libaom 实现；可见帧按
`show_frame`/show-existing 顺序输出。五帧 fixture 的全部 Y/U/V 与 libaom
**逐字节一致**。第 87 slice
恢复双参考帧解析，覆盖全部 8 种 compound mode 符号、compound DRL、
双 MV 赋值和 normative average prediction。新增四帧 320×240 fixture，最终帧同时覆盖
uni/bidirectional `NearestNearest`、`NearNear` 和 `NewNew`；四帧 Y/U/V 均与 libaom
**逐字节一致**（frame 3 MD5 `b217ef76fa2e65327d3b4ecfa7ee1f36`）。masked compound、
warp/OBMC、temporal MV 和 inter segmentation 仍未完成。第 86 slice
修正 coefficient CDF 必须按帧头 `base_q_idx` 选择，而不是按块级 delta-q；补齐
4-high inter chroma skip context，并把 MV CDF 纳入 SavedCdf。新增三帧 delta-q fixture，
其 Y/U/V 均与 libaom **逐字节一致**。第 85 slice
把 LR 参数网格接入 inter frame，并修正
`UseWienerCdf` /
`UseSgrprojCdf` / `RestorationTypeCdf` 从 SavedCdf 的继承；restoration-enabled key+P
fixture 的 P frame（242 个分区叶块）三平面与 libaom **逐字节一致**
（MD5 `aabd1f611b39c59fda7d5c7c2877d7ec`）。第 84 slice 接入了
`TX_MODE_SELECT` inter transform tree、switchable interpolation filter、inter deblock 和
CDEF；LF+CDEF key+P fixture 的 P frame（255 个 inter block）三平面同样**逐字节一致**。
单参考 inter 还覆盖 fractional MV 的 16-phase 8-tap 与 4-tap 双阶段插值、inter 残差和
transform type、frame 内 intra/inter 混合块、skip 的 above/left 系数上下文清零，以及
sub-8×8 chroma 按四个 luma MV 分象限预测。high-QP 320×240 fixture 的 327 个 P-frame
block 与 libaom trace 一致（P-frame MD5 `a6cb3249ce923e683934ce85fa5ba0e9`）；更早的
整数像素 inter fixture 的 20 个 inter block 参考帧/模式/MV 也与 libaom trace 一致。
此前的 §7.16 normative
superres：213×240 coded frame 按 denominator 12
上采样到 320×240 后进入 LR，重建前、去块后、CDEF 后、LR 后四个阶段的 Y/U/V 三平面
都已与 libaom **逐像素一致**；更早为 palette plane 坐标、左右 upsampled directional phase、
CDEF、方向预测、反量化和按 plane 去块网格、4×4 `BlockDecoded` 可用性网格、
inverse-identity 尺寸缩放、帧级 loop restoration、CDEF、去块、Y/U/V 帧级重建、
palette/filter-intra 单 plane 像素预测、
`Quant[]` 到逆变换布局的桥接、
真实 320x240 keyframe 的整 tile 熵解码闭合：palette 颜色/索引图、10-bit
颜色、CFL alpha、边缘分区、完整系数上下文以及 `exit_symbol()` 尾位校验均通过；
此前为 intra tile block sink、skip 语义修正、filter-intra、CDEF index、
stateful coefficient syntax、
partition/mode/skip/tx_depth、
可变 CDF 状态、residual transform 布局、
`ReferenceStream`、块级 transform 选择、
统一帧内预测入口、帧头参考状态、tile-group 拆包、
§5.9 完整帧头、§8.1 位流描述符、§5.2 OBU/IVF 解复用与 §5.5 序列头），
逐项证据见 `docs/CAPABILITY.md` 的 M11 表与 `ROADMAP.md` 尾部；上文的 89% 是按里程碑
工作量重算的加权估算。

## 对外 API

当前已有可链接的 **RK MPP Rust V1.12 编解码 SDK**：

- C 头文件：[include/rk_mpp_rs.h](include/rk_mpp_rs.h)
- Rust 接口：`rk_mpp_rs::sdk`
- Raw FFI：workspace crate `rk-mpp-rs-sys`
- 构建产物：`librk_mpp_rs.so`、`librk_mpp_rs.a` 和 `rlib`
- 优先编解码器：H.264、H.265/HEVC、VP9、AV1、VP8、MPEG-2、MPEG-4、
  H.263 和 MJPEG/JPEG decode；
  H.264/H.265 VEPU580/VEPU510 及 JPEG VEPU2/VPU720 encode

接口接受内存中的任意分块码流，不绑定文件路径。一个 decoder handle 可持续
提交 packet、接收 frame、显式释放 frame，并支持 `flush`、`reset`、
`recover`、H.264/H.265/VP9/AV1 `status` 和粘性 EOS。H.264 按 POC、H.265 按
display epoch + POC 有界重排；MJPEG 支持单个 JPEG 被拆成多包，也支持一个
packet 里包含多个 JPEG。H.264/MJPEG 输出 NV12 或 I420；H.265 8-bit 输出
NV12/I420，Main10 输出小端 16-bit 的 I420P10；VP9/AV1 8-bit 输出 NV12/I420，
10-bit 输出 I420P10；VP8 输出 NV12/I420。VP9/VP8 要求每个 submit 是一个
原始 coded packet（VP9 可为 superframe），AV1 要求一个 temporal unit；
IVF/WebM 解复用属于调用方。VP8 只在 t6 的 vdpu2 提供，lf1/RK3576 的
C MPP 和 Rust 都不存在该 decoder。新的
V1 接口暂不宣称 RGB888，历史
`rk_jpeg_open/frame/close` RGB 接口继续保留给 `rknn-fastinfer`。

MPEG-2/MPEG-4/H.263 使用同一 handle 接收任意分块 elementary stream；现有
内部 decoder 以完整 stream 为状态边界，因此这三类格式在 EOS/flush 时统一
产出所有待显示帧。

板级 C/C++ 门：

```bash
BOARD=luck2 ./scripts/board_c_api_h264_mjpeg.sh
BOARD=t6 ./scripts/board_c_api_h264_mjpeg.sh
```

lf1 和 t6 已验证共享库和静态库链接、全部公开符号、生命周期错误路径，以及 H.264
7/127/313-byte 分块和整条单包输入；H.265 8-bit 37-byte 分块、整条单包和
Main10 53-byte 分块输入；VP9 profile 0/2 各 12 个 IVF packet；AV1 8-bit
2 个 packet、10-bit 10 个 temporal unit；MJPEG 97/211-byte 分块和四帧单包
输入。除已登记的 AV1 film-grain C bug 外，输出均与板上 oracle 逐字节一致。

### 安装 SDK

```bash
./scripts/install_sdk.sh --prefix /usr/local

# 打包/打包机安装时
./scripts/install_sdk.sh --prefix /usr/local --destdir /tmp/rk-mpp-stage
```

安装布局：

- `include/rk_mpp_rs.h`
- `lib/librk_mpp_rs.so.1.12.0`
- `lib/librk_mpp_rs.so.1 -> librk_mpp_rs.so.1.12.0`
- `lib/librk_mpp_rs.so -> librk_mpp_rs.so.1`
- `lib/librk_mpp_rs.a`
- `lib/pkgconfig/rk-mpp-rs.pc`
- `lib/cmake/rk-mpp-rs/`

`SONAME` 固定为 ABI major：`librk_mpp_rs.so.1`。

```bash
gcc app.c $(pkg-config --cflags --libs rk-mpp-rs)

add_executable(app app.cpp)
target_link_libraries(app PRIVATE rk_mpp_rs::rk_mpp_rs)
# 或 rk_mpp_rs::rk_mpp_rs_static
```

Rust 最小示例：

```bash
cargo run --example sdk_decode -- h264 input.264 output.yuv
cargo run --release --example sdk_encode -- h264 input.nv12 output.264 320 240 26 6
```

`rk-mpp-rs-sys` 暴露 V1.12 的全部 36 个 C 导出、enum、struct 和不透明句柄。

另有进行中的官方 MPP drop-in ABI 基础层：`mpp_create`、`mpp_init`、
`mpp_destroy`、格式查询和官方 `MppApi` 184-byte 函数表。当前只保证生命周期
与函数表布局，并提供 `MppFrame`/`MppPacket`/`MppBuffer`、packet/frame
访问器和 H.264/H.265/MJPEG 的 CPU NV12 `decode_put_packet` /
`decode_get_frame` 数据面。完整 `MppMeta`、advanced buffer import、encoder
对象流和 standalone 所需的完整内部辅助符号仍未接入。已可在保留 vendor
log/cfg/OSAL 辅助库的条件下，通过 `LD_PRELOAD` 让**未修改的**
`mpi_dec_test` 解码 H.264/H.265/MJPEG；这还不是完全脱离 vendor 库的替换。
同一桥接层也已接通 `MppEncCfg`、`encode_put_frame` 和
`encode_get_packet`：t6 的 H.265/MJPEG 单帧 stream 与 C MPP 逐字节一致，
H.264 单帧解码重建一致。H.264 多帧和 RK3576 H.264/H.265 官方参数矩阵仍在
收敛，不能把当前 encoder 路径标成完整兼容。
链接发现支持显式 `RK_MPP_RS_SYS_LIB_DIR`、已安装 pkg-config，以及 workspace
`target/<profile>`：

```bash
RK_MPP_RS_SYS_LIB_DIR="$PWD/target/release" \
LD_LIBRARY_PATH="$PWD/target/release" \
cargo test --release -p rk-mpp-rs-sys

./scripts/board_sys_crate.sh   # dynamic + static + pkg-config 板级门
```

ABI 和完整安装 consumer 门：

```bash
PROFILE=release ./scripts/check_sdk_abi.sh
BOARD=t6 ./scripts/board_sdk_package.sh
BOARD=luck2 ./scripts/board_sdk_package.sh
```

H.264 编码接口接受紧凑的 visible-size NV12/I420，内部复制并补齐 coded
geometry，再以同步 packet queue 返回 Annex-B。t6 VEPU580 支持
baseline/main/high、level、GOP、QP 和帧率；RK3576 VEPU510 当前只开放已板验的
320×240 Baseline QP26 GOP6 @30fps 路径。两端都把 6 帧 GOP 与现有 CLI 输出
逐字节比较，并验证 reset 复用和 packet release。

H.265 编码共享同一套原有 encoder ABI。t6 VEPU580 开放 width/height、QP
和 GOP；RK3576 VEPU510 当前只开放板验过的 320×240 Main QP26 GOP6 @30fps
路径。H.264/H.265 都接受 NV12 或 I420，packet 带 keyframe/frame_num/POC
元数据。

V1.9 增加独立的 `rk_mpp_encoder_advanced_config_t` 和
`rk_mpp_encoder_create_advanced()`，因此原有 68-byte
`rk_mpp_encoder_config_t` 与 `rk_mpp_encoder_create()` 完全不变。Rust 使用
`RateControlConfig`/`RateControlMode`；C 使用
`rk_mpp_encoder_rate_control_t`。t6 VEPU580 H.264/H.265 支持 CBR、VBR、
AVBR、目标/最小/最大码率和初始/最小/最大 QP；C API 六组输出与已验证 CLI
逐字节相同。RK3576 VEPU510 的逐帧 streaming RC 尚未开放，当前明确返回
unsupported。

```bash
./scripts/board_sdk_encoder_rc.sh
```

V1.10 通过独立的 `rk_mpp_encoder_advanced_config_v2_t` 增加最多 8 个 ROI、
GDR/intra-refresh、slice split 和 low-delay packet 输出；V1.9 的 104-byte
config、初始化和 create 函数保持原样。Rust 对应 `RoiConfig`、`GdrConfig`、
`SliceSplitConfig`。t6 上 H.264/H.265 的 ROI、GDR、slice-lowdelay 六组 C API
输出均与既有 CLI 逐字节相同。

```bash
./scripts/board_sdk_encoder_controls.sh
```

V1.11 可选择让 H.264、8-bit 4:2:0 H.265/VP9/AV1 与 MJPEG NV12 解码帧保留硬件
dma-buf。默认行为仍是连续 CPU frame，以保持 V1 兼容；设置
`RK_MPP_DECODER_FLAG_DMABUF_OUTPUT` 后不再复制到 CPU `Vec`。调用方可在
`receive_frame()` 后按 plane 导出私有 fd、offset、size 和 stride，用于
RKNN/RGA 等 fd consumer：

```c
config.flags |= RK_MPP_DECODER_FLAG_DMABUF_OUTPUT;
/* rk_mpp_decoder_create(...) */
int fd = -1;
uint64_t offset, size;
uint32_t stride;
rk_mpp_decoder_export_frame_dmabuf(
    decoder, 0, &fd, &offset, &size, &stride);
/* use fd */
close(fd);
```

Rust 对应 `DecoderConfig::with_dmabuf_output(true)`、
`Frame::dmabuf_plane()` 和 `Frame::duplicate_dmabuf()`。H.264/H.265/VP9/AV1/MJPEG
已支持该 flag；H.265 Main10/422/444、VP9/AV1 10-bit 以及其他仍未接入 DMA frame 的 codec
调用导出会返回 `RK_MPP_ERR_UNSUPPORTED`。

```bash
./scripts/board_sdk_dmabuf.sh
```

V1.12 保留原有字节数组输入，同时允许 H.264/H.265/JPEG 直接编码 NV12
dma-buf。调用方传入 fd、完整 allocation 长度和 coded Y/UV stride；H.264/H.265
要求 stride 为 16 对齐，JPEG 要求 8×2 对齐。提交是同步的，函数返回后调用方
即可复用输入 surface：

```c
rk_mpp_encoder_submit_frame_dmabuf(
    encoder, fd, allocation_size, hor_stride, ver_stride, pts, dts);
```

Rust 对应 `DmaBufInput::new(...)` 和 `Encoder::submit_dmabuf_frame()`。
lf1/t6 上 H.264、H.265、JPEG 的 fd 输入输出与 copied-input 输出逐字节一致：

```bash
./scripts/board_sdk_encoder_dmabuf.sh
```

JPEG 编码沿用同一 encoder handle，支持 NV12/I420、0–10 内置量化档和可选
restart MCU 间隔。lf1/t6 单帧 JPEG 与现有 `jpeg-enc` CLI 输出逐字节一致。

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
cargo test --release --all-targets           # t6：1225 枚举；1188 常规通过，37 个硬件 ignored
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
./scripts/board_h265_tile_vs_mpp.sh          # H.265 水平 tiles：2/2
BOARD=luck2 ./scripts/board_rk3576_h265_vs_mpp.sh # RK3576 VDPU383：12/12
BOARD=luck2 ./scripts/board_rk3576_vp9_vs_mpp.sh  # RK3576 VDPU383 VP9：7/7
BOARD=luck2 ./scripts/board_rk3576_h264_edges_vs_mpp.sh # H.264 边界：5/5
BOARD=luck2 ./scripts/board_rk3576_h265_corrupt_vs_mpp.sh # H.265 损坏矩阵：16 格
BOARD=luck2 ./scripts/board_rk3576_jpege_vs_mpp.sh # VPU720 JPEG：19/19
BOARD=luck2 ./scripts/board_rk3576_h264e510_vs_mpp.sh # VEPU510 H.264 固定/CBR/VBR/AVBR/ROI/split/GDR：13/13
BOARD=luck2 ./scripts/board_rk3576_h265e510_vs_mpp.sh # VEPU510 H.265 功能/边界矩阵：15/15
BOARD=luck2 ./scripts/board_rk3576_av1_vdpu383_key.sh # VDPU383 AV1 首 keyframe：C == Rust
BOARD=luck2 ./scripts/board_rk3576_av1_vdpu383_sequence.sh # VDPU383 AV1 10 帧序列：C == Rust
BOARD=luck2 ./scripts/board_rk3576_av1_vdpu383_10bit.sh # VDPU383 AV1 10-bit skip-grain：C == Rust
./scripts/board_iep_modes_vs_probe.sh        # IEP2 modes + raw slots：9/9
./scripts/board_iep_formats_vs_probe.sh      # IEP2 420/422 SP/Planar：6/6
BOARD=luck2 ./scripts/board_c_api_h264_mjpeg.sh # V1 C/C++ H.264 + MJPEG SDK
BENCH_FILTER=vp8 ./scripts/board_bench_vs_mpp.sh
```

## 性能

| 项目 | 结果 |
|---|---|
| 解码 / 编码 | 当前证据只支持“**不比 C 慢**”（VP9 实测：720p 进程 wall-clock C 72 ms / Rust 69 ms，短流约 2×；内部 loop 计时差异属口径不同，不作为百分比结论）。同一二进制在 t62 的 720p H.264 结果可从 72.9 ms 漂到 120.1 ms；旧的高速百分比是未做交错采样时的高噪声样本，不再作为结论 |
| RGB letterbox（替换 RGA） | 1080p→640: 0.84 ms vs RGA 3.21 ms；1:1: 0.53 vs 1.50 |

### H.264 流式播放路径

`h264_stream` 使用 VUI 的 `max_num_reorder_frames` 做有界 POC 重排，不再把整段
视频收进 `Vec<Picture>`。1080p30 High/B=2 的 90 帧样片上，旧 `h264dec`
需要约 279 MB RSS，流式路径约 17 MB；输出 planar YUV 与旧全文件排序结果
逐字节一致。同一测试中解码循环由约 955 ms 降到 545 ms。

```bash
# 丢弃输出，测流式解码
./target/release/h264_stream in.264

# 输出 planar YUV，或向播放器管道输出
./target/release/h264_stream in.264 out.yuv
./target/release/h264_stream in.264 - |
  gst-launch-1.0 -q fdsrc fd=0 ! \
    rawvideoparse format=i420 width=1920 height=1080 framerate=30/1 ! \
    waylandsink
```

这使 1080p30 的 Rust 纯解码上限从 24 路提高到 26 路（C MPP 约 28 路）。
真实多窗口播放目前支持约 8 路；12 路会被 stdout planar-YUV 管道和 Wayland
拷贝压垮。下一步应把 decoded frame/DMA-BUF 直接交给 DRM/Wayland sink，避免
每帧 3.1 MB 的用户态传输。

## 用法

硬件 probe（无参数）：`rk-mpp-rs`

| 子命令 | 说明 |
|---|---|
| `decode <jpg> <out.rgb> [1\|2\|4\|8]` | JPEG 解码到 RGB（下采样） |
| `jpeg-info <jpg>` / `jpeg-enc <in.yuv> <out.jpg> <w> <h> [quant] [fmt] [input] [rst<mcus>] [splitout:lowdelay]` | JPEG 解析 / 编码与逐 restart band packet |
| `h264 <file>` / `h264dec <in.264> <out.yuv>` | H.264 语法打印 / 解码 |
| `h264_stream <in.264> [out.yuv|-]` | 有界 POC 重排的流式 H.264 解码与输出 |
| `hevc` / `hevcdec <in.265> <out.yuv>` / `hevcpps` / `hevcdpb` | H.265 语法、PPS、DPB 打印与解码 |
| `av1dec <in.ivf> <out.yuv>` / `vp9dec <in.ivf> <out.yuv>` / `vp8dec <in.ivf> <out.yuv>` / `vp8` | AV1、VP9、VP8 |
| `m2v` / `m2vdec <in.m2v> <out.yuv>` / `mp4dec <in.m4v> <out.yuv>` / `h263dec <in.263> <out.yuv>` | MPEG-2 / MPEG-4 / H.263 |
| `h264-enc <in.nv12> <out.264> <w> <h> [qp] [profile] [frames] [gop] [cbr:\|vbr:bps:min:max] ... [splitout:lowdelay]` | H.264 编码；给码率三元组即进入码率控制，低延迟模式逐 slice 输出 packet |
| `h265-idr ... [roi:x:y:w:h:intra:abs:qp]... [split:ctu:\|split:byte:arg] [splitout:lowdelay]` / `h265-gop ... [shortterm] ...` | H.265 固定 QP IDR/GOP、GDR、ROI/QP map 与逐 slice packet 输出 |
| `h264e510-split2 <in.2x320x240.nv12> <out.264> <mb:N\|byte:N> [lowdelay]` | RK3576 VEPU510 BY_CTU/BY_BYTE 两帧编码与逐 slice packet 输出 |
| `h264e510-gdr <in.Nx320x240.nv12> <out.264> <row\|col:num:gop:frames>` | RK3576 VEPU510 固定 QP GDR / intra refresh |
| `h265e510 <in.320x240.nv12> <out.265>` | RK3576 VEPU510 固定 QP Main IDR |
| `h265e510-gop2 <in.2x320x240.nv12> <out.265>` | RK3576 VEPU510 固定 QP Main IDR+P |
| `h265e510-gop6 <in.6x320x240.nv12> <out.265>` | RK3576 VEPU510 固定 QP Main 六帧 GOP |
| `h265e510-cbr6 <in.6x320x240.nv12> <out.265>` | RK3576 VEPU510 H.265 CBR 六帧 |
| `h265e510-vbr6 <in.6x320x240.nv12> <out.265>` | RK3576 VEPU510 H.265 VBR 六帧 |
| `h265e510-avbr6 <in.6x320x240.nv12> <out.265>` | RK3576 VEPU510 H.265 AVBR 六帧 |
| `h265e510-split2 <in.2x320x240.nv12> <out.265> <ctu:N\|byte:N> [lowdelay]` | RK3576 VEPU510 H.265 slice split 与逐 packet 输出 |
| `h265e510-gdr <in.Nx320x240.nv12> <out.265> <row\|col:num:gop:frames>` | RK3576 VEPU510 H.265 GDR |
| `h265e510-roi2 <in.2x320x240.nv12> <out.265>` | RK3576 VEPU510 H.265 ROI / QP 区 |
| `h265e510-tiles <unused.nv12> <unused.265> <columns>` | RK3576 VEPU510 H.265 tile 请求边界；多于 1 列必须显式拒绝 |
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
- [docs/T6_VALIDATION.md](docs/T6_VALIDATION.md) —— 2026-11-06 的 t6 全量测试、板级回归、性能与播放结果
- [docs/DIVERGENCES.md](docs/DIVERGENCES.md) —— 已知与 C 的差异登记：必须修 / 已证 C bug / 无 oracle
- [docs/HANDOFF.md](docs/HANDOFF.md) —— 交接：下一步、板子工具链、易错清单
- [docs/rk3576.md](docs/rk3576.md) —— RK3576 硬件代际、C MPP 基线与 Rust 适配进度
- [docs/h264d_hw.md](docs/h264d_hw.md) —— rkvdec H.264 硬件契约（从 C 提取）
- [docs/superpowers/](docs/superpowers/) —— 历史计划与设计稿（带状态说明）

## 已知限制

- 板上的 C 参考实现与 tracer 是工作副本（`/tmp/mpp-old`、`/tmp/mpp-develop`、
  `/tmp/mpp_trace.so`）。t6 的 `/tmp` 实测**不是 tmpfs**（根是 overlay，
  `upperdir=/data/root`），但它归 `systemd-tmpfiles` 管（`D /tmp 1777 root root -`
  且清理 timer 在跑），属于会被清的位置。权威副本已放在板上的
  `~/rk-mpp-oracles`（含编解码 oracle、tracer 与 438 个流/参考输出），
  `scripts/setup_board_refs.sh` 幂等检测并回填（已用"把 tracer 藏起来再跑一次"验证过恢复路径）。
- rkvenc 同进程多会话已验证：64x64/320x240/640x480 下 1/2/3 线程各产出一条一致码流，且与 C 参考除版本 SEI 文本外逐 NAL 相同。原先“每进程只允许一个活会话”的说法来自过期金样（串行也失败），已撤回。
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
