# rk-mpp-rs 完整未来计划

> 本文是**从现在到"MPP 表面全部覆盖"的完整计划**，覆盖每一个已知的未完成项、
> 每个工作流的验收标准、彼此依赖、阶段出口与风险。
> 进度证据仍以 `ROADMAP.md` 为权威台账；本文只回答"接下来做什么、按什么顺序、
> 什么算做完"。
>
> 基线：`6029ebe`（2026-11-01）。441 测试通过（353 单元 + 88 集成，11 个板级 ignore），
> ROADMAP 75 项已勾 / 20 项未勾。码率控制固定 QP / CBR / VBR / AVBR 全部逐 NAL 相同，
> P 帧现在也在 11 个尺寸上逐字节相同（W1-19 已闭合：根因是 FBC 体偏移写死 8192，而它在编码面积
> 超过 524288 像素后应为 16384；IDR 不读参考，所以只编 IDR 的矩阵一直全绿）。
> 板上 oracle 与语料已固化到 `~/rk-mpp-oracles`（`scripts/setup_board_refs.sh`）。

---

## 0. 完成定义（Definition of Done）

任何一项能力只有同时满足下列 **五道门** 才在 ROADMAP 勾选，缺一不可：

| 门 | 要求 | 证据形态 |
|---|---|---|
| G1 语法/结构 | 解析或生成与 C 语义一致 | 单测 + 真实流/真实寄存器 fixture |
| G2 硬件路径 | 提交、poll、错误路径正确 | 板上跑通，无 HW 错误位 |
| G3 逐字节 | 与板上源码编译的 C MPP **输出逐字节相同** | 脚本 + golden/hash |
| G4 性能 | 同流、两侧外部计时、交错采样，不慢于 C | bench 脚本输出 |
| G5 记录 | ROADMAP 有证据段落，README/本文状态同步 | 提交信息 + 文档 diff |

例外必须**写在 ROADMAP 里**并说明原因（例：version SEI 内容不复刻、
VDPP 在 RK3588 不存在、4:2:2/4:4:4 被硬件拒收、C 侧自身有 bug 的流）。

**"全部完成"的最终定义**（项目终态）：

1. `refs/mpp` 中 RK3588 上实际存在的每个硬件块（client 1/4/9/12/13/16/19/28）
   都有 Rust 实现，且 G1–G4 全过；
2. 公开表面（见 §19 决策 D1）——包含 C API 兼容层时，能替换 `librockchip_mpp` 链接；
3. framework 具备异步、fence、缓冲池、错误恢复与多实例能力；
4. 解码/编码吞吐不低于 C MPP；推理链路的 JPEG+scale 链路保持现有优势；
5. 文档（README / ROADMAP / PLAN / HANDOFF / rustdoc / CHANGELOG）与实际一致。

---

## 1. 基线快照（2026-11-01）

**已收口**：`mpp_service` 客户端；JPEG 解码；H.264 / H.265 / VP9 / AV1 /
MPEG-2 / MPEG-4 / H.263 / VP8 解码（逐字节，见 ROADMAP）；H.264 编码的固定 QP、
CBR、VBR（逐 NAL）；JPEG 编码 baseline；自研 RGB 缩放。

**未收口**（全部 18 项 + 已知隐含项）：

- H.264 编码：场景检测、ROI/QP map、GDR、deblock 细调、slice split、
  >4096 宽度、level 参数化、CIME/RIME 生成、ME/CIME 参数化、重编码/多 pass、
  hier QP/SMART_P、`cal_first_i_start_qp`、与 `mpi_enc_test` 的配置对齐
- H.265 编码：零
- JPEG 编码：RGB 输入、90/270 旋转、restart markers、low-delay slices、多核切分
- 后处理：IEP；8/10-bit 互转；HDR；VDPP（本板不可用）；RGA3（可选）
- framework：异步队列、fence、buffer import/export 与引用计数池、多核 CCU、
  secure/TA、rkvenc 单会话限制
- 解码器：AVS+/AVS2、损坏/截断流一致性、VP9 全矩阵、AV1 更宽 conformance、
  HEVC SCC、H.264 field-coded PAFF 复核、非 16 对齐尺寸的字节级路径
- 工程：C API 兼容层（若需要）、CI、fuzz、rustdoc/CHANGELOG、板上 oracle 持久化

---

## 2. 目标分层与"谁负责什么"

```
┌───────────────────────────────────────────────┐
│ L5 公共表面：Rust API +（可选）C ABI 兼容层     │  W7
├───────────────────────────────────────────────┤
│ L4 framework：任务队列/异步/fence/池/多核/安全  │  W5
├───────────────────────────────────────────────┤
│ L3 codec：语法 ↔ 寄存器无关的语义对象           │  W1 W2 W3 W6
├───────────────────────────────────────────────┤
│ L2 HAL：各硬件块寄存器生成、STATUS 解析          │  W1 W2 W4 W6
├───────────────────────────────────────────────┤
│ L1 service：mpp_service ioctl、dma-heap、poll   │  已完成
└───────────────────────────────────────────────┘
```

约束（贯穿所有工作流）：

- 只允许 `libc` 一个外部依赖（如确需新增须先记录理由并获准）；
- `src/hal/*` 的 `build()` 类函数保持纯函数（不 ioctl），便于离线测试；
- 所有硬件访问集中在 `src/service/`；
- 公开类型尽量 `Debug + Clone + Send + Sync`；
- 不修改 `refs/mpp`；板上的 C 构建只用于取证据。

---

## 3. 工作流总览

| 代号 | 工作流 | 规模 | 依赖 | 主线？ |
|---|---|---|---|---|
| W0 | 验证基建与 oracle 持久化 | M | — | 是（最先） |
| W1 | H.264 编码收尾 | L | W0 | 是 |
| W2 | H.265 编码 | XL | W0, W1（复用 RC/工具） | 是 |
| W3 | JPEG 编码余项 | S–M | W0 | 否 |
| W4 | 后处理（IEP/格式/HDR） | L | W0 | 否 |
| W5 | framework（异步/fence/池/多核/安全） | XL | W0 | 是 |
| W6 | 解码器边缘与 conformance | L | W0 | 否 |
| W7 | C API 兼容层（待决策 D1） | L | W1, W2, W5 | 视决策 |
| W8 | 性能工程 | M–L | 各功能完成 | 是 |
| W9 | 可靠性/安全（fuzz、unsafe、错误注入） | M | W5 | 是 |
| W10 | 测试资产与 conformance 语料 | M | W0 | 是 |
| W11 | 文档与发布 | S（持续） | 全部 | 是 |
| W12 | 其他 SoC（待决策 D2） | XL | 全部 | 视决策 |

规模口径：S ≤ 2 天，M ≈ 3–5 天，L ≈ 6–10 天，XL > 10 天（单人等效）。

---

## 4. W0 验证基建与 oracle 持久化（最先做，约 3–5 天）

**为什么先做**：当前编码器的字节级 oracle 全在板上 `/tmp`（`/tmp/mpp-old`、
`/tmp/mpp-develop`、`/tmp/mpp_trace.so`），重启即失；重建成本最高的是 C 侧构建，
一旦丢失，后续所有编码工作都没有判据。

任务：

- [x] W0-1 `/tmp/mpp-old`（`80957d266`）与 `/tmp/mpp-develop` 已固化到
      `~/rk-mpp-oracles`（66M / 54M），`scripts/setup_board_refs.sh` 先检查后回填，
      两边都缺才 clone + cmake 重建。实测更正：t6 的 `/tmp` **不是 tmpfs**
      （根为 overlay，`upperdir=/data/root`），但 `D /tmp 1777 root root -`
      与 `systemd-tmpfiles-clean.timer` 说明它是会被清的位置——原文"重启即失"的
      机理判断是错的，风险结论不变。
- [x] W0-2 `mpp_trace.so` 同样存进 oracle home，缺失时由脚本从
      `tools/mpp_ioctl_trace.c` 重建。恢复路径已验证：把 tracer 改名后跑脚本，
      它报 `miss` 并回填。
- [x] W0-3 `scripts/board_regress_all.sh`：按依赖顺序跑 oracle 检查 + 六套解码回归 +
      固定 QP（默认 GOP 6 格，`FULL_QP=1` 跑 24 格）+ CBR/VBR + STATUS，
      统一 PASS/FAIL 汇总与退出码；`SKIP=` 可跳单套。
- [~] W0-4 fixture 管理：**板上语料与参考输出**已入 `corpus/`（438 个文件、1.3G 含 oracle）；
      `tests/data/` 的来源头与 `refresh_fixtures.sh` 仍待做。
- [x] W0-5 抖动检查：`scripts/board_jitter_check.sh` 对 H.264/H.265 C decode
      和 H.264 C encode 各跑两次，同时比较输出字节与 SHA-256。t6 首轮得到
      H.264 decode `e72eeb14…b5d11`、H.265 decode
      `87375cba…119f74`、H.264 encode `8ef4f37b…9c86d`，两次均一致。
      `JITTER_CHECK=1 scripts/board_regress_all.sh` 已接入。
- [x] W0-6 板子信息采集脚本：`scripts/board_info.sh` 采集 OS/kernel、设备树、
      CPU/内存/thermal/governor、mpp/dma-heap 节点、platform driver、hw_support
      mask 和逐 client 绑定结果，生成 [`docs/board.md`](board.md)。脚本已在 t6
      实跑，并用 release binary 避免编译 warning 污染报告。

验收：断网/重启后照脚本可在 15 分钟内恢复全部 oracle 并跑通 `board_regress_all.sh`。

---

## 5. W1 H.264 编码收尾（约 10–15 天）

### 5.1 AVBR —— **已完成**（`12626d2`；三模式 10/10 NAL 相同）

- [x] W1-1 `docs/rc-avbr-wip.patch` 已应用（提交见 git log），补 `src/hal/h264e.rs`
      三处 match 与 `rc_model_config` 的模式映射（原先 `_ => Cbr` 会把 AVBR 静默当成
      CBR，属于必须修的坑），并把 STATUS 反馈改为 `end_with`（`quality_real`、
      `madi`、`madp` 按 HAL 的除数）。补丁已可删除。
- [x] W1-2 用 `tools/rc_probe.c … 2`（AVBR）+ `rc_debug=0xf0` 抓取 C 侧状态，
      存为 `tests/data/rc_model_v2_c_avbr_state.txt`（含逐步算式注释）。
- [x] W1-3 单测 `avbr_scales_the_gop_by_a_still_moving_level` 逐步比对
      （222 / 607031 / 9276 / QP 27）；`board_h264_cbr_vs_mpp.sh` 已加 `avbr:2`，
      **CBR / VBR / AVBR 三模式各 10/10 NAL 相同**（6 帧，实测通过）。
- 注意：`pre_i_scale` 在该版本从未赋值（恒 0）；`bps_change` 在有符号调整前算；
      `qratio = -6` 的静止条件；`min_still_percent = bps_min*100/bps_max`。

### 5.2 场景检测器（2–4 天）

- [x] W1-4 用 `hal_h264e` 的调试开关（需先确认 env 名）或寄存器捕获，构造硬切序列
      观察 `vepu580_h264e_tune_stat_update` 触发条件（连续三帧大运动）。
- [x] W1-5 移植 detector：读 `madi/madp/md_sad_b16num*/madi_b16num*`，切换
      quant bias 与 ATR 权重行；`scene_motion_flag` 进入状态机。
- [x] W1-6 硬切 fixture 上逐字节：`scripts/board_h264_scene_vs_mpp.sh`（testsrc→testsrc2
      八帧固定 QP）整流逐字节相同。见 ROADMAP 场景检测器条目。

### 5.3 编码器其余参数化（5–7 天）

- [x] W1-7 level 参数化：`level_idc` 生成 SPS/PPS/`rect_size/mv_limit`，并逐级验证
      （至少 3.1/4.0/5.1）。
- [x] W1-8 `calc_cime_parameter` + `cime_multi`/`rime_multi` 生成。搜索范围/
      cache 由 level、编码尺寸和 GMV=0 推导；tune patch 的四行 CIME/RIME 与
      RDO 阈值/权重现在每帧从表重建，不再把 quiet 行留在录制模板里。
- [x] W1-9 `setup_vepu580_me` 全量参数化：`pmv_mdst`、`mv_limit`、CIME/RIME
      搜索范围、`mei_stor` 和真实 motion-info DMA 地址均已生成；相关字段由
      level、slice type、几何和分配后的 fd 驱动。
- [x] W1-10 宽度 > 4096 —— 偏移与 RCB 已推导并有宿主单测；
      `FRAMES=2 board_h264_wide_vs_mpp.sh` 已跑，4160x64 与 4608x256 均含
      IDR+P，6 NAL 逐字节相同，CIME extension offset/RCB 路径板级 2/2。
- [x] W1-11 `cal_first_i_start_qp` —— 板级 5/5 格逐字节（含 QP 轨迹）。

### 5.4 编码特性（6–10 天）

- [x] W1-12 QP map / ROI —— 走 `Vepu580RoiH264BsCfg`/`RoiQpCfg` 块记录，不是 `roi_qthd*`
      那八行；板级 2/2 格逐字节。
- [x] W1-13 GDR / intra refresh —— 板级 4/4 逐字节（row-4 / col-4 / row-8 / row-1）。
      stock C 的 ROI 表构造只赋值命名 bit，未初始化的
      `force_inter`/`mode_mask`/`reserved` 会进入 DMA，连续三次 C 运行得到
      `d4432259…`、`a49958ff…`、`f77af73e…`。`tools/mpp_roi_table_fix.c`
      在 sync 前仅清零这些未定义位，Rust 与修正后的确定性 C 表 35 NAL 全一致。
      两个只有上板才能定的行为：sweep 处参考重发 SPS/PPS + recovery-point SEI；
      `refresh_num == 1` 时 band 起点经 `RK_U16` 截断为 0xFFF0 → 该帧不强制 intra。
- [x] W1-14 deblock 细调 —— 含「参考不写 beta」这个 quirk；板级三格逐字节。
- [x] W1-15 slice split 与逐 slice 输出 —— 普通整 AU 在
      `board_h264_slice_vs_mpp.sh` 为 4/4；低延迟 packet 在
      `board_h264_lowdelay_vs_mpp.sh` 为 5/5（BY_CTU 96/150/8、BY_BYTE 256、
      6 帧 IDR+P）。38-slice 格跨 32 项硬件 FIFO，所有实际 slice 边界与拼接码流一致。
- [x] W1-16 hier QP / SMART_P（`hier_qp_cfg`、`vgop`、`vi_bit` 窗口）
      - [x] hier QP：四层计数器与 `hier_qp_delta` 已接入三种 RC，IDR 清零；
        `board_h264_cbr_vs_mpp.sh` 的 `hier-cbr/vbr/avbr` 三格均逐 NAL 相同。
      - [x] SMART_P：`i_scale=320`/`vi_scale=32`、`vi_bit` 窗口、
        `INTER_VI_FRAME` 分配与 2-QP VI delta 已接入；IDR 保留为长期参考，
        VI 在 slice header 重排 L0。`board_h264_smartp_vs_mpp.sh` 的
        CBR/VBR/AVBR 三格均为 33 帧、37 NAL 逐字节相同。
- [x] W1-17 重编码与多 pass：`check_re_enc`、`reenc_calc_*_ratio`、super frame、
      `-n` 超发时的 drop/pskip 策略；补"同帧重编"路径
      - [x] 同帧重编：CBR/VBR/AVBR 的 `check_re_enc` 与 reencode ratio 已接入；
        低码率强制过码时 CBR 为 7 个硬件 job、VBR/AVBR 为 8 个，
        三种最终码流和逐 job STATUS 均与 C 相同。
      - [x] super-frame re-encode：阈值强制重编时三种 RC 均与 C 相同；
        `super-reenc` 格为 12 个逐 STATUS 相同的硬件 job。
      - [x] super-frame NORMAL drop：丢弃帧回滚参考状态、提交 0 bit，并按
        参考路径结束 RC 两次；三种 RC 的最终流和 7 个逐 job STATUS 均相同。
      - [x] PSKIP（CAVLC/CABAC）：software all-skip NAL 在 CBR/VBR/AVBR 下均与 C
        相同，包含 POC/frame_num 推进、RC 的双结束语义，以及 C 回灌的
        CAVLC 13-bit / CABAC 15-bit 口径。CABAC 的 skip context、terminal
        和 flush 与 C 逐字节相同；三种 RC 均为 7/8/9 个硬件 job。
      - [x] two-pass deflicker：首个 IDR 之后，每个 IDR 先以 non-reference P
        写入 64×16 对齐的线性 NV12 pass1 buffer，第二个 job 再从这里取源；
        `calc_debreath_qp`、RC 状态恢复、重编抑制、逐 IDR version/RC SEI 与
        SPS/PPS 只发一次均已对齐。CBR/VBR/AVBR 的 17 NAL 和 12 job STATUS
        全同，CABAC 格 17 NAL 和 11 job STATUS 全同。
- [x] W1-18 与 `mpi_enc_test` 的配置对齐：差异来自 harness 将
      `rc:qp_init=-1` 并把 general/intra QP 范围从库默认 `8..48` 放宽到
      `10..51`，不是 RC 模型差异。Rust 增加可配置 RC QP 范围；C probe 和
      Rust 使用同一配置后，六帧 CBR 的 10 个 NAL 逐字节一致。

- [x] **W1-19（P1 阻断，已闭合）P 帧在 1280x720 与参考分岔** —— 根因：FBC 体偏移
      （reg 164 / 166）写死成常量 8192，而 `pixel_buf_fbc_hdr_size = align(aligned_w*aligned_h/64, 8K)`
      在编码面积超过 524288 像素后是 16384 —— 于是硬件被告知"体从 8192 开始"，而缓冲区的体
      其实从 16384 开始，参考面被从错误位置读取。IDR 不读参考，所以"每格只编一张 IDR"的
      24 格矩阵在四个尺寸（含 1280x720）一直全绿。
      修法：`fbc_header_size()` 成为分配与两个偏移的唯一来源，单测走阈值（320x240 / 640x480 /
      800x600 得 8192，1280x720 得 16384）并断言 1280x720 作业里的偏移值。
      铁据：`board_h264_p_size_probe.sh` 扩到 11 个尺寸（阈值两侧各有格子）全部逐字节；
      原始失败样例（720p 六帧 testsrc）逐帧相同（1371 / 1075 / 1341 / 1097 / 1336）；四帧完全相同
      与逐帧亮度阶跃的场景差值全为 0；既有门全部复跑通过（GOP 6/6、cbr/vbr/avbr、GDR 4/4、
      24 格矩阵）。
      教训已写进 README：**只编 IDR 的尺寸看不见参考路径的问题**；P 帧的尺寸扫描应并进矩阵。

- [ ] W2-1 用 2024-04 C 编 H.265 固定 QP 流（多尺寸多 QP 多 profile），抓
      `hal_h265e_vepu580.c` 的 ioctl job；用 `tools/gen_vepu580_regs.py` 扩展字段映射
      （H265 专用结构），产出 `src/hal/h265e_regs.rs`。
- [~] W2-2 SPS/PPS/VPS 生成 —— 仅固定 QP IDR 的三个参数集，用独立 HEVC 解析器回读
      （13 条宿主单测）；tiles/SAO 已完成，WPP 在 C 参考里不存在（标志恒 0、无 entry point 写入），
      RPS、profile/level 变化未做。
- [ ] W2-3 CTU/CU 编码参数、MVP、SAO 表：照 C 生成，逐字段
- [ ] W2-4 参考帧与 DPB：与 H.264 共用 `FrameState` 思路，H.265 有 RPS
- [x] W2-5 固定 QP 的 I + P 帧逐 NAL 相同（320x240 多格，640x480/720p 已补）
- [x] W2-6 复用 `codec::rc::model`（C/H.265 用同一 RC 模型）：CBR/VBR/AVBR 与
      smart GOP 均逐字节
- [~] W2-7 slice 切分与逐 slice 输出已闭合（BY_CTU/BY_BYTE、32 项 FIFO 跨越、
      6 帧 IDR+P；普通整 AU 3/3，low-delay 4/4）；WPP 未做（水平 tiles 见 W2-11）
- [x] W2-8 10-bit 编码探针 —— t6 的 VEPU580 接受 MPP 的 10-bit prep/frame
      配置，但硬件 job 返回 `int_sta=0x100` 且不产生 packet；Rust 配置层据此
      显式拒绝非 8-bit，`board_h265_10bit_probe.sh` 固化这条拒绝证据。
- [x] W2-9 GDR / intra refresh —— C ref-cfg oracle 已建立；
      320x240/640x480 × row/column 为 4/4 整条逐字节。
- [x] W2-9b ROI/QP map 已闭合（相对/绝对 QP、force-intra、多 region、
      640x480，`board_h265_roi_vs_mpp.sh` 4/4）；deblock beta/tc、deblock
      disable、SAO luma/chroma disable 与 loop-filter 选项已接入 PPS/HAL，
      `board_h265_syntax_tune_vs_mpp.sh` 4/4，均为 7 NAL 逐字节相同。
- [x] W2-10 吞吐测量 —— `board_bench_h265_encode_vs_mpp.sh` 用官方
      `mpi_enc_test` 与 Rust 交错采样 5 轮：320x240 / 640x480 / 720p Rust
      分别快 15.8% / 3.6% / 13.8%；1080p 为 C 147 ms、Rust 149 ms，
      Rust 慢 1.3%，在允许平手的 5% 噪声门内，4/4 通过。
- [x] W2-11 水平 tiles —— PPS 写 `tiles_enabled_flag`/`num_tile_columns_minus1`，
      每列一个 job（`reg0252_tile_cfg`、`reg0253_tile_pos`、`reg212_rc_cfg.rc_ctu_num`、
      tile 相对的 CIME line buffer 窗口、`reg0193_dual_core` 握手、`reg0176/177`
      交替 line buffer）。RK3588 只到两列：`auto_tile` 在 4K 以下要两列，宽度 >4096
      强制两列，四列需要 >8192 宽度而硬件不接受。`board_h265_tile_vs_mpp.sh` **2/2 逐字节**
      （auto_tile 320x240 覆盖 IDR+P、4608x64 强制两列，后者 P 帧两侧都以 `int_sta=0x100`
      被拒，故该格为 IDR）。C tile job 的 trace 与派生镜像比对（`tools/h265e_trace_diff.py
      --job=0/1`）除 10 个 dma-buf fd 字与 `mei_stor` 外**没有一个非地址字不同**。
      tile + low-delay 逐 slice 输出显式拒绝；会话首帧的跨进程双核残留状态登记在
      `docs/DIVERGENCES.md`（C 自身在两种字节模式间摇摆，我们用一趟丢弃作业把自己钉定）。

验收：至少 4 尺寸 × 2 QP 的固定 QP 矩阵、6 帧 GOP、CBR/VBR 各一条逐 NAL；
场景检测与 ROI 按 H.264 的同一套证据标准。

---

## 7. W3 JPEG 编码余项（S–M，约 3–5 天）

- [x] W3-1 RGB / RGBX 输入（VEPU2 的输入格式寄存器路径）
- [x] W3-2 90/270 旋转
- [x] W3-3 restart markers（间隔、DRI）
- [x] W3-4 low-delay slices —— `board_jpege_lowdelay_vs_mpp.sh` 2/2；
      320x240 5 packet、640x480 10 packet 均与 C 的完整码流和逐 packet 长度一致。
      旋转和 >720p 因 C 走其它路径而明确拒绝。
- [x] W3-5 多核切分 —— RK3588 默认四 band 路径已用 streaming `MULTI_MSG`
      批处理实现；1920x1080 qf50/qf90、1288x722 不均匀 band、rot90/rot270
      为 5/5，322x242/1366x768 stride 为 2/2。
- [x] W3-6 4:2:2/4:4:4/gray：记录为硬件不支持（已验），不实现伪支持

验收：每种新输入/特性出可独立解码的流，与 C 逐字节；不支持的组合返回明确错误。

---

## 8. W4 后处理（L，约 8–12 天）

- [x] W4-1 IEP2（client 28）：I5O2、I5O1T/B、I2O2、I1O1T/B、DISABLE、BYPASS
      已板上 9/9 逐字节，原始 dst0/dst1 均比较；420/422 × SP/Planar × UV/VU
      为 6/6；PD/DECT 逆 telecine 已闭合（场序 + pulldown 状态机，
      `board_iep_dect_vs_probe.sh` 5/5 逐字节且逐帧状态相同，含 322x242/722x482/
      1366x768 等非 16 对齐尺寸）。
      client 28 的 `iep2_params` 只有 deinterlace，没有 YUV scale/crop 字段；
      legacy `/dev/iep` scale 后端在 t6 无节点、driver 不绑定，因此板上无法实现或验证。
- [x] W4-2 IEP 与解码器联动（NV12 直通、零拷贝）：
      `IepDeinterlace::import_src_frame()` 直接 dup 解码 Frame 的 dma-buf，
      校验 NV12 单 fd、Y/UV plane offset、horizontal/vertical stride 后作为
      三张 IEP 源图；源/目标分配和 `iep2_params` 地址都使用实际的
      `hor_stride * ver_stride`。板上既有的
      `iep_consumes_decoder_dmabuf_frames_without_copying_through_cpu` 通过，
      新增 50×34 解码流（H=64/V=48）还将零拷贝导入与显式 padded source
      对比，两张 destination slot 均逐字节一致。
- [x] W4-3 8/10-bit 互转（打包/planar、位深扩展）：
      `Frame::convert_format()` 支持 NV12/I420 ↔ I420P10，8→10 用位复制，
      10→8 用右移；转换保留 PTS/DTS/POC/error info。宿主单测覆盖
      I420 可逆转和 NV12 解交织/回打包。
- [x] W4-4 HDR tone mapping 判定为平台不可用：t6 没有 VDPP，当前解码帧
      也没有可验证的 HDR metadata/oracle；不提供伪装成硬件等价的软件 HDR。
- [x] W4-5 RGA3 为可选路径：现有 NEON 缩放覆盖推理链路且更快，不引入
      第二个不可控硬件依赖。
- [~] W4-6 VDPP：RK3576 client 29 的 NV12 scale、YUV444/420、DCI histogram
      和默认 DMSR/ES/sharp enhancement bank 已移植，全部 DMSR/ES/SHARP 输入
      参数也已建模，ZME bypass/dering/blend、可替换的 11×17×8 tap8/tap6 bank
      和 YUV420/YUV444 split Y/UV 输出也已接通；`board_vdpp2_regs_vs_c.sh`
      通过 fake-service C trace 十一格逐字比较；
      packed histogram 的 4096 local /
      256 global bin 与 U10 平均亮度解析已完成，`vdpp-hist` 可供 lf1 做原始
      packed/local/global 对比。可撤销的 configfs DTB overlay 和
      `board_vdpp2_hw_vs_c.sh` 已准备好；t6 没有 client 29，lf1 的 DT status
      虽为 `okay`，但内核 `CONFIG_ROCKCHIP_MPP_VDPP` 未开启且无 configfs
      overlay，平台驱动没有绑定，client-29 init 返回 `EINVAL`；真实硬件像素和
      histogram 仍需重编包含 `mpp_vdpp` 的内核。

验收：IEP 路径与 C 输出逐字节（或 PSNR ∞）；性能不低于 C，缩放不低于现有 `scale.rs`
（若更慢，保留 `scale.rs` 作为推理链路默认）。

---

## 9. W5 framework（XL，约 25–35 天；最大缺口）

按依赖顺序：

- [x] W5-1 任务队列与状态机：`QUEUE/START/POLL/FLUSH/RESET`，与 C 语义一致
- [x] W5-2 异步提交 + 完成回调（18 条宿主单测；`SessionGate` 可作为显式共享
      单会话时的可选串行器，不是每进程单会话的正确性要求）
- [ ] W5-3 dma-fence in/out：`MPP_CMD_SET_FENCE`、fence 数组、poll 集成
- [x] W5-4 缓冲池：引用计数、in-flight 状态机与 RAII `BufferLease` 已落地
      （10 宿主 + 7 板级 ignore，含 lease fd→IOVA/dup 门）；VP8 DPB 已改用
      通用缓冲池并在 slot 消亡时 RAII 归还；H.264/HEVC/AV1/VP9 的
      DPB surface 均持有 lease。H.264 在 IDR/分辨率变化时释放旧 frame；
      HEVC 只保留 DPB 引用；AV1 复用前释放非参考帧；VP9 显式保护硬件
      `previous/last` colmv fd。pooled JPEG 现在先关闭
      `MppService` 再归还共享
      dma-buf，修复旧 drop 顺序把 fd 过早暴露给其他 worker 的内核 import 竞态；
      t6 连续 3000 jobs 压测无 lockup/reset。
- [x] W5-5 多实例：H.264 的 CIME extension 与 H.265 tile line buffer 都是
      session-owned；t6 上 1/2/3 线程在 64x64/320x240/640x480 产出一致且
      与 C 参考相同，旧的“每进程单会话”结论已撤回
- [x] W5-6 多核 CCU：设备树探测 vdec/venc/jpeg，`BatchServer` 用
      `SET_SESSION_FD` 把最多 8 个 session 合成一个提交批次；t6 两个独立
      VEPU2 JPEG session 和 lf1 两个独立 VEPU510 H.264 session 的批量结果都
      与各自单会话逐字节一致，两块板各 3/3。6.1 内核的批量 wait 不填
      `mpp_bat_msg.ret`，因此提交后回退到每个真实 session 的 poll；无 CCU
      或 `SET_SESSION_FD` 不支持时自动走单会话路径。
- [~] W5-7 secure/TA：无 TA 可验；服务层现已在任何 secure batch 下发前
      返回 `Unsupported`，但仍需真实 TA/kernel 提供能力探测接口后才能实现安全路径
- [x] W5-8 错误恢复：HW timeout 进入 close/reopen recovery，legacy VDPU2
      IRQ 错误可恢复，session replacement/fd 生命周期、真实 VP8 STATUS 错误和
      H.265 tile recovery 板级 4/4；invalid input fd 在 t6 触发真实
      `int_sta=0`，重开后清空 tile line buffer、重新执行丢弃预跑，并把下一帧
      强制成带参数集的 IDR，输出与全新会话逐字节一致。RESET_SESSION 仅保留
      显式破坏性诊断。
- [x] W5-9 统计与追踪：`TaskTiming` 记录排队、执行和端到端耗时；
      `TaskMetrics` 累计 submitted/done/failed/cancelled、总耗时和
      max queued/running；现有 bounded event channel 同步携带每任务 timing。
      host 单测与 t6 async-demo 同路径验证。

验收：并发多任务/多实例压测无泄漏（valgrind/asan 或 fd 计数）、fence 顺序正确、
与我方同步路径输出一致；与 C MPP 的同场景行为对照（不要求逐字节，要求语义等价）。

---

## 10. W6 解码器边缘与 conformance（L，约 10–15 天）

- [ ] W6-1 AVS+ 解码（client 12；需要 conformance 流，见 W10）
- [ ] W6-2 AVS2：本板 rkvdec 声明支持，但无流；有流再做
- [ ] W6-3 损坏/截断/随机 bit-flip 流：与 C 的"输出帧数 + 错误码"一致（逐字节要求
      降级为可解释的差异）
- [ ] W6-4 VP9 全矩阵：profile 0/1/2/3、10-bit、分辨率/分块边界
- [ ] W6-5 AV1 更宽 conformance：分辨率、10-bit 打包、film grain 组合；
      average、difference-weighted 与 wedge compound 已由四/五帧 libaom 像素门闭合，
      distance-weighted、smooth/wedge inter-intra、OBMC、local warped motion、
      单参考/compound global warped motion 与 temporal MV fields 已由隔离
      fixture 闭合；inter segmentation 的 map、temporal prediction 与 features
      已由三帧隔离 fixture 闭合；`delta_lf_multi` 的 luma/U/V 四路
      与单色两路 CDF 状态已实现；真实彩色 keyframe 锁定三平面，真实单色
      keyframe 锁定 Y 平面，并覆盖 UV 语法门控与单平面 deblock；
      真实 2×2 tile keyframe 已闭合逐 tile 熵独立（每 tile 从帧初始 CDF 重启、帧末 CDF
      取 `context_update_tile_id`）与 tile 边界裁剪的 intra 邻块/CDEF 传输，三平面逐字节；
      2×2 tile 的 keyframe 与后续 inter coded frame 已闭合：帧级 mi/segment 状态跨 tile
      携带、tile-local context 裁剪、temporal motion-field cell/sign-bias 规则和
      difference-weighted chroma mask 均已修正，10 帧 Y/U/V 与 libaom 逐字节一致；
      余下仅是更宽的 AV1 conformance/性能矩阵
- [ ] W6-6 HEVC SCC（调色板/屏幕内容）：若 C 无路径则记录为 C 缺失
- [ ] W6-7 H.264 field-coded PAFF 复核（当前 MBAFF/PAFF 有覆盖，field-coded 单独补）
- [ ] W6-8 非 16 对齐尺寸的字节级路径（JPEG 已有；编码器受 harness 限制，需自建 C 驱动）
- [ ] W6-9 输出格式矩阵：NV12/PLANAR/RGB888/10-bit、downscale 因子组合
- [ ] W6-10 flush/seek/重放：同一 session 解码多段流（DPB 重置、IDR 边界）
- [ ] W6-11 换 SoC 复核硬拒项：H.265 4:2:2/4:4:4 在 RK3588 被 rkvdec 拒绝，
      换到接受的板子上再验（依赖 D2；在那之前保持"明确拒绝 + 文档标注"）
- [ ] W6-12 JPEG 解码边界：progressive / arithmetic / CMYK / 多 scan / 16-bit DQT
      ——先确认 C 侧 jpegd 支持到哪，支持的对齐，不支持的明确拒绝

验收：每条流有 C 侧 golden；损坏流给"与 C 相同的行为"并记录差异原因。

---

## 11. W7 公共 API 与 C ABI（决策 D1；若需要，L 约 10–20 天）

- [ ] W7-1 Rust 公开 API 稳定化：`Decoder`/`Encoder` 句柄、配置、帧/包类型。
      **全部已实现 decoder 与 H.264/H.265/JPEG encoder 已完成**
- [ ] W7-2 C ABI：`mpp_create/mpp_init/mpp_destroy`、`MppApi` 函数表子集、
      `mpp_frame/mpp_packet/mpp_buffer` 最小实现
      **Foundation landed:** official-header lifecycle and support-query
      symbols plus the complete 184-byte `MppApi` table layout pass on t6/lf1.
      **Decode slice landed:** packet/frame/buffer ownership and CPU NV12
      `decode_put_packet/decode_get_frame` pass H.264, H.265 and MJPEG against
      C MPP on both boards. Local `MppMeta` now covers scalar/pointer/object
      values, user-data deep copies, duplicate/ref/lifetime and frame/packet
      lazy ownership. Imported DMABUF buffers, kernel meta and complete
      standalone helper symbols remain.
- [ ] W7-3 `mpi_dec_test`/`mpi_enc_test` 兼容性验证（用官方测试工具链接我们的库）
      **Decode partial landed:** the unchanged vendor `mpi_dec_test` runs
      H.264/H.265/MJPEG under `LD_PRELOAD` with byte-identical output on
      t6/lf1. Vendor log/cfg/OSAL helpers remain linked from the native MPP;
      standalone linking and `mpi_enc_test` remain.
      **Encoder partial landed:** vendor `MppEncCfg`, `MppFrame` input and
      `MppPacket` output are bridged into `encoder::Encoder`. t6/lf1 H.265 and
      MJPEG one/six-frame streams are byte-identical; H.264 one/six-frame
      VCL/reconstruction matches C MPP with only version-SEI framing differing.
      RK3576 High CABAC/8x8, MEIW and its object-specific H.265 header/register
      templates are covered. Standalone helper replacement remains.
- [x] W7-4 头文件与符号导出策略（前缀、版本、`-sys` crate）。
      **V1.11 头文件、`rk_mpp_` 前缀、SONAME、pkg-config/CMake、符号快照和
      版本化 advanced encoder config、完整 `rk-mpp-rs-sys` 已完成**
- [ ] W7-5 ABI 测试：C 程序调用我们的库跑通 3 个解码 + 2 个编码用例。
      **H.264/H.265/VP9/AV1/VP8/MPEG-2/MPEG-4/H.263/MJPEG 解码及三种编码 C/C++ 用例已完成**

验收：官方 `mpi_*_test` 在不改源码的情况下能链接并跑通（允许功能子集）。

---

## 12. W8 性能工程（M–L，约 8–12 天）

- [x] W8-1 NEON 缩放内核：一般比例 bilinear 行现在预计算每四个输出像素的
      gather index/weight，NEON 横向插值后直接纵向合并，尾部回退标量。
      t6 `60` 次交错采样中，非整数 `1930x1080→640x640` 从 `2.804` ms
      降到 `1.321` ms（约 `2.12x`），全部 General/int ratio 样例与标量输出
      逐字节相同。
- [x] W8-2 零拷贝链路：解码→缩放→推理输入，减少 DMA_BUF sync 次数。
      `Frame::letterbox_nv12()` 直接读取 CPU 或 DMA-BUF frame 的 strided
      Y/UV planes，在缩放时完成 BT.601 RGB 转换，不生成整帧 NV12/RGB 中间副本。
      H.264 1280x720→640x360 在 t6/lf1 的 CPU/DMA 路径 RGB 输出逐字节一致；
      1080p→640x640 在 t6 为 4.93 ms、lf1 为 5.95 ms，相比
      “整帧 NV12→RGB + RGB scale”的 20.32/24.36 ms 分别减少约 76%/76%。
- [x] W8-3 编码器缓冲复用：`Vepu580H264::open` 一次性分配 input、output、
      两个 recon 的 FBC pixel/thumb、motion-info、ROI、GDR 和 CIME extension
      表；后续 `encode_frame` 只做 sync、copy、寄存器提交和 STATUS 读取，
      路径内没有 `DmaBuf::alloc`/`mmap`。H.265 session 同样在 `open_inner`
      完成 input/output/two surfaces/motion-info 的固定分配。
- [ ] W8-4 1088 行编码的抖动定位（当前与 C 平手、双峰 ~110/~140ms）
- [x] W8-5 基准门槛化：`scripts/board_perf_gate.sh` 运行 t6 的交错
      `test720` decode 与四条 NEON scale 基准，读取
      `scripts/perf_baseline.tsv` 的相对吞吐/毫秒上限；任一关键行缺失或超限
      即失败。`PERF_GATE=1 scripts/board_regress_all.sh` 已接入，默认关闭以
      保持普通正确性回归的速度。
- [x] W8-6 内存占用报告：`scripts/board_memory_profile.sh` 在 t6 采样
      `/proc/*/status` 的 `VmHWM` 和 debugfs dma-buf 总量，生成
      [`docs/memory.md`](memory.md)。八种 decode 和六帧 H.264/H.265 encode
      已有 t6 基线；CPU RSS 与 dma-buf 分列，避免重复计算。编码器每帧
      alloc/mmap 已由 W8-3 消除。极短 320x240 decoder 的 dma-buf 事件快于
      采样窗口，报告明确标记为未观测而不是伪报为零。

验收：每个已交付路径"不慢于 C"；推理链路（JPEG+scale）保持现有 30% 与 2.9–3.8× 优势。

---

## 13. W9 可靠性与安全（M，约 5–8 天）

- [~] W9-1 `unsafe` 审计：`service` 的 ioctl/mmap/dma-buf/dup 边界与 C ABI
      入口已补 SAFETY 不变量和长度溢出检查；NEON 调度与 AV1 C-ABI
      `zeroed`/字节视图也已补不变量。codec/reference 其余 `unsafe` 块仍需逐项复核
- [ ] W9-2 cargo-fuzz：语法解析器（H.264/HEVC/AV1/VP9/JPEG/MPEG）
- [ ] W9-3 错误注入：假 STATUS、超时、EINVAL、ENOMEM
- [ ] W9-4 资源泄漏：fd/mmap 计数测试，长时间循环（1e6 次提交）
- [~] W9-5 clippy 全绿（`-D warnings` 视情况）：普通
      `cargo check --all-targets` 已零 warning；`clippy -D warnings` 仍有大量
      既有风格/复杂度 findings，尚未作为硬门启用。

验收：fuzz 24h 无崩溃；错误路径全部返回 `Result` 且不泄漏。

---

## 14. W10 测试资产与 conformance 语料（M，约 3–5 天）

- [x] W10-1 流语料清单与来源：[`tests/data/SOURCES.md`](../tests/data/SOURCES.md)
      按 x264/x265/libvpx/libaom/FFmpeg 生成流、C MPP captures 和本项目
      pixel gates 分类；没有把第三方大 conformance corpus 收入仓库。
- [ ] W10-2 AVS+/AVS2 conformance 流获取（或明确"无法获得"）
- [x] W10-3 hash 清单：`scripts/update_fixture_hashes.sh` 生成
      `tests/data/SHA256SUMS`，`tests/fixture_manifest.rs` 同时检查文件集合和
      每个文件的 SHA-256。158 个 fixture，总计约 2.7 MiB。
- [x] W10-4 golden 大小控制：仓库内只保留小型生成流、C-MPP capture 和 pixel
      gate golden，总计约 2.7 MiB；来源/生成入口在 `tests/data/SOURCES.md`
      固定，字节由 `SHA256SUMS` 锁定。更大的外部 corpus 不进 git。

---

## 15. W11 文档与发布（S，持续）

- [ ] W11-1 README/ROADMAP/PLAN/HANDOFF 每次提交同步（G5）
- [ ] W11-2 rustdoc：公开 API 全部有例子；`cargo doc` 无警告
- [ ] W11-3 CHANGELOG（按能力分类，不按提交罗列）
- [ ] W11-4 许可与归属：移植自 Apache-2.0 的 MPP，保留版权头与 NOTICE
- [ ] W11-5 示例：解码、编码、缩放、异步四个最小例子

---

## 16. W12 平台扩展（决策 D2）

- [~] W12-1 SoC 能力探测抽象（client mask + 绑定结果表）
      已按 device-tree compatible 选择 RK3588/RK3576 的预期
      VDEC/VEPU/JPEG/VDPP 代际；RK3588-only HAL 在 RK3576 上显式拒绝。
- [~] W12-2 RK3568/RK3576 等目标板适配（各自 VEPU/VDEC 版本）
      RK3576 已确认 VDPU383/VEPU510/VPU720/VDPP2，并生成 VDPU383
      276-word 寄存器表；VDPU383 H.264 640x480 12 帧已与 C MPP 逐字节，
      720p/B、1080p hard、PAFF、LTR 与缺失参考边界 5/5 通过（PAFF 用 FFmpeg 判定
      C 侧场序差异）；H.265 12/12 矩阵
      （Main/Main10、B、tiles、真实 WPP、720p、DSLICE、SPS/PPS scaling list）
      也已逐字节，500 帧 LTR SPS 流与删 AU 的缺失参考流同样逐字节；
      176-byte global packet 与 C dump 相同；VP9 初始 7/7
      （profile 0/2、8/10-bit、lossless、tiles，两个 C bug 由 libvpx 判定）；
      VPU720 JPEG 19/19（3 sizes + 60-MCU restart + 420/422/444 planar/
      semi-planar/packed/YUV400 输入与 400/420/422/444 输出）逐字节。
      VDPU383 AV1 已生成与 C 首帧逐字节的 672-byte global header
      （FNV `0x8da389d2e180d680`）和 212-word register struct image；
      default CDF、segmentation/colmv/RCB sizing 已对齐，首 keyframe 已真实
      提交并输出与 C 逐字节；multi-frame/inter/10-bit/film grain 仍待实现。
      VEPU510 H.264 已从 C struct 生成 645-field/6-window 寄存器表，并与 lf1
      IDR trace 的 section/STATUS 尺寸对齐；首个 320x240 QP26 Baseline IDR、
      GOP2、GOP6、CBR6、VBR6 和 AVBR6 已与 C 全流逐字节
      （6479 / 8671 / 18225 / 14187 / 15616 / 12313 bytes）；ROI 两帧格
      也已逐字节（10845 bytes），BY_CTU/BY_BYTE 与 LOWDELAY packet 两档同样
      通过（4/4，BY_BYTE 低延迟为 40 packets）；row=2 与 col=4 两档 GDR
      也逐字节（2/2）；VEPU510 H.265 首个固定 QP Main IDR 已与 C 全流
      逐字节（4710 bytes），六帧固定 GOP 也逐字节（16994 bytes），CBR6
      为 11192 bytes，VBR6/AVBR6 为 11795/9676 bytes；BY_CTU/BY_BYTE
      与 LOWDELAY packet 两档也已逐字节，row/column GDR 与 ROI 同样通过；
      10-bit 与 C 一样在构图前明确拒绝；tiles 与 C 一样不启用，Rust 对
      多于 1 列在 DMA 前显式拒绝，功能/边界矩阵为 15/15。
      AV1/AVS2、H.265 随机错误注入、完整 H.264/H.265/VEPU510 编码矩阵、
      VPU720 tile input 与 VDPP2 仍需完成。
- [ ] W12-3 每个 SoC 独立验收矩阵（同一能力在多个块上允许不同 HAL）

---

## 17. 阶段划分与出口

| 阶段 | 内容 | 出口标准（可演示） | 规模 |
|---|---|---|---|
| P0 | W0 + AVBR | 重启后一键恢复 oracle；AVBR 6 帧 10/10 NAL | 5–7 天 |
| P1 | W1 其余 | H.264 编码特性齐全，矩阵全绿 | 10–15 天 |
| P2 | W2 | H.265 编码固定 QP/CBR/VBR 逐字节 | 20–30 天 |
| P3 | W5 + W9 | 异步/fence/池/多实例可用，长稳无泄漏 | 25–35 天 |
| P4 | W4 + W6 + W3 | 后处理与解码边缘收口，conformance 通过 | 15–20 天 |
| P5 | W7 + W8 + W11 + W12 | C ABI（若需要）、性能门槛、跨 SoC、发布 | 15–30 天 |

关键路径：**W0 → W1 → W2 →（W5 ∥ W6/W4）→ W7/W8 → 发布**。
W5 可以与 W1/W2 并行（不同人/不同上下文），但 W7 必须等 W5。

---

## 18. 风险登记

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| R1 | 板上 `/tmp` oracle 丢失 | 全部编码证据失效 | **已处置（W0-1/2）**：权威副本在 `~/rk-mpp-oracles`，`setup_board_refs.sh` 检查并回填；实测 `/tmp` 非 tmpfs 但受 tmpfiles 清理 |
| R2 | 硬件/固件行为漂移（换板/升级） | 金样失效 | 记录内核/固件版本；能力探测脚本；金样带 hash 与来源 |
| R3 | H.265 编码规模被低估 | 主线延期 | 先做固定 QP 的 I 帧最小闭环，再逐特性；用寄存器 diff 驱动 |
| R4 | framework 无 C 侧语义文档 | 行为偏差 | 以 `mpi_*_test` 行为为 oracle；写对照测试 |
| R5 | secure/TA 无设备支持 | 无法验证 | 能力探测 + 明确拒绝；文档标注"未验证" |
| R6 | 多实例共享行缓冲 | 并发不可用 | **已处置（W5-5）**：行缓冲为 session-owned，t6 多线程编码验证通过；锁/`SessionGate` 仅保留为显式共享单会话时的可选策略 |
| R7 | conformance 流不可得（AVS） | 无法证明 | 明确记录"无流不可验"，不伪造证据 |
| R8 | 性能回归悄悄发生 | 违反目标 | W8-5 把吞吐门槛纳入回归脚本 |

---

## 19. 待决策（需用户确认）

- **D1 C API 兼容层**：是否需要能直接替换 `librockchip_mpp` 的 `.so`（决定 W7 是否在主线）？
- **D2 目标 SoC —— 已决定（2026-11-01）：先只保 RK3588 保真。**
  其他 SoC 不在承诺范围，代码保持"探测 + 分块 HAL"的可扩展形态；W12 暂不排入主线，
  等 3588 收口后再评估。
- **D3 secure/TA**：是否有可验证的 TA 环境；没有则按"探测即拒绝"实现并记录。
- **D4 多 pass/两遍编码 —— 已决定**：H.264 two-pass deflicker 已实现；
  `mpi_enc_mt` 的并发/多实例语义归 W5，不再阻塞 W1-17。
- **D5 逐字节范围**：CBR/VBR 是否要在所有分辨率/内容上都逐字节，还是仅代表性矩阵
  （逐字节覆盖成本随内容增长很快）。
- **D6 依赖策略**：是否接受为 NEON/并发引入 crate（当前仅 `libc`）。

除 D2 已定（仅 RK3588）外，其余按"不做 C ABI / 不做 secure / 仅代表性逐字节矩阵"
的默认推进，等有明确需求再改变。

---

## 20. 每轮工作的操作规范

1. 开工先跑 `scripts/board_regress_all.sh`（W0-3 落地后）或现有三条脚本，确认基线绿。
2. 一次只做一条可验证的能力；证据（fixture/脚本输出）先于代码结论。
3. 每完成一项：ROADMAP 加/改条目（含证据与已知边界）→ `cargo fmt` →
   `cargo test --all-targets` → 板级脚本 → 提交（信息写清证据与例外）。
4. 出现与应用意图不符的发现（如"其实不是 bug"），写撤回而不是删改旧结论。
5. 文档同步属于 G5，视为完成的一部分，不做事后补。

---

## 21. 验收矩阵（工作流 × 五道门）

§4–§16 的 `W*-*` 复选框是唯一任务清单；下表说明每一列在哪条工作流上以什么证据收口，
避免"代码写完就算完"。

| 工作流 | G1 语法/结构 | G2 硬件路径 | G3 逐字节 | G4 性能 | G5 记录 |
|---|---|---|---|---|---|
| W0 基建 | 脚本幂等 | 板上跑通 | hash 稳定 | 恢复耗时 < 15 min | `docs/board.md` |
| W1 H.264 编码 | 参数/语法单测 | HW 错误位为 0 | 24 格 + GOP + CBR/VBR/AVBR/特性流 | ≤720p 快于 C，1088 持平 | ROADMAP 条目 + 脚本 |
| W2 H.265 编码 | SPS/PPS/VPS/RPS 单测 | 同左 | 4 尺寸 × 2 QP + GOP + RC | 同左 | 同左 |
| W3 JPEG 编码 | 头/表单测 | 同左 | 独立解码 + 与 C 逐字节 | 不慢于 C | 同左 |
| W4 后处理 | 几何/格式单测 | IEP 绑定 | PSNR ∞ 或逐字节 | 不低于 C / 不低于 `scale.rs` | 保留回退路径 |
| W5 framework | 状态机单测 | 多任务/多实例 | 与同步路径输出一致 | 无退化 | 泄漏/并发证据 |
| W6 解码边缘 | 解析器单测 | 错误位为 0 | C golden | 不慢于 C | 差异原因记录 |
| W7 C ABI | 头文件/符号 | `mpi_*_test` 跑通 | 同 Rust 路径 | 同左 | ABI 兼容测试 |
| W8 性能 | — | — | 不影响 | 门槛化回归 | 前后对比表 |
| W9 可靠性 | fuzz 无崩溃 | 错误注入 | — | — | 审计记录 |
| W10 语料 | 来源/许可清单 | — | 生成可复现 | — | hash 清单 |
| W11 文档 | — | — | — | — | 本文 + ROADMAP 同步 |

**每阶段出口 = 该阶段涉及的工作流在上述每一列都有证据**；缺列即为未完成，
在 ROADMAP 上保持未勾并写明缺口。
