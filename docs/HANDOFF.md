# rk-mpp-rs 交接文档

写给下一个接手实现的模型/人。目标是 `refs/mpp` → `rk-mpp-rs` 的完整 Rust 重写：
功能对齐、性能不劣于 C、每次验证通过就提交。

本文以仓库现状为准（`d0e6947`；工作树干净）。`README.md` 是最新状态速览，`ROADMAP.md` 是逐项证据台账（权威），
本文是"从哪继续、怎么不被坑"的操作手册。

---

## 0. 五分钟了解现状

```bash
cd ~/workspace/rknn-test/rk-mpp-rs
cargo test --all-targets      # 593 passed, 23 board-only ignored
cargo fmt --check
git log --oneline -10
grep -c '^- \[x\]' ROADMAP.md  # 已完成条目数
grep -c '^- \[ \]' ROADMAP.md  # 未完成条目数
```

板上验证（需要 `ssh t6`，详见第 4 节）：

```bash
ONLY=gop ./scripts/board_h264_qp_vs_mpp.sh   # 固定 QP：6 格全 PASS 约 3 分钟
./scripts/board_h264_cbr_vs_mpp.sh           # CBR + VBR + AVBR 各 10 NAL 全同
./scripts/board_h264_level_vs_mpp.sh         # SPS level 矩阵 13 格
./scripts/board_h264_deblock_vs_mpp.sh       # deblock 3 格 + auto-QP 2 格
./scripts/board_h264_roi_vs_mpp.sh           # ROI/QP map 2 格
./scripts/board_h264_scene_vs_mpp.sh         # 硬切场景检测
./scripts/board_jpege_restart_vs_mpp.sh      # JPEG restart（-t 8 + split env）
./scripts/board_vp_corrupt_vs_mpp.sh         # 损坏流 vs C 判决表
```

整体完成度约六成。解码侧基本收口；H.264 编码的 level、deblock、ROI/QP map、
auto-QP 起始 QP、场景检测、GDR、slice split、>4096 扩展行与 P 帧全部板上逐字节；
JPEG 编码有 restart/90°/270°/RGB；framework 有异步任务队列 + 引用计数缓冲池。
**当前主攻 H.265 编码**：固定 QP 的 VPS/SPS/PPS 已 3/3 板上逐字节，寄存器作业已抓取，
推导图与 C 的六段作业只差 `enc_pic.mei_stor` 一位（见 ROADMAP 的 H.265 条目）。
下一步是让 H.265 有自己拥有缓冲的提交路径；H.264 那条线剩下的是逐 slice 输出路径
（`slen_fifo`/low-delay 包）与 >4096 宽度下的 P 格。

---

## 1. 目录与关键文件

| 路径 | 作用 |
|---|---|
| `src/service/` | `mpp_service` ioctl 客户端、dma-heap、batch、poll |
| `src/codec/<codec>/` | 各格式语法解析（h264/hevc/vp9/av1/m2v/mpg4/h263/vp8/jpeg） |
| `src/hal/` | 各硬件块寄存器生成（h264d, hevc, av1d, vp9d, vp8d, jpegd/jpege, h264e） |
| `src/hal/h264e.rs` | VEPU580 H.264 编码器：缓冲、寄存器、提交、STATUS 读取 |
| `src/codec/rc/` | 码率控制移植（`data.rs` = MppDataV2，`model.rs` = rc_model_v2 的 CBR/VBR） |
| `src/scale.rs` | 自研 RGB 缩放/letterbox（替代 RGA，比 RGA 快 2.9–3.8×） |
| `src/reference/` | 自研软件参考解码器（M11，CABAC/反变换起步） |
| `scripts/board_*.sh` | 板上验证脚本（回归、吞吐、寄存器捕获） |
| `tools/` | 取证工具：ioctl tracer、C 探针、寄存器 diff、字段生成器 |
| `tests/data/` | 所有 golden/fixture（C 侧真实输出） |
| `ROADMAP.md` | 逐 capability 的证据台账，**提交前必须更新** |

生成器：`tools/gen_h264_regs.py`、`gen_vdpu34x_regs.py`、`gen_vepu580_regs.py`
（从 C 结构体定义生成寄存器字段名/位置，改 HAL 前先看它们能否直接给出字段）。

`gen_vepu580_regs.py` 的 H.265 路线读两个 vintage：共享的 CTL/BASE/RC_KLUT 取自
`refs/mpp`（master），H.265 专属的 WGT/RDO/OSD 取自 `rk-mpp-rs/refs/mpp-2024-04`
（与板上 oracle 同源的 2024-04 副本，见该目录 README）。这种混用只在两边布局一致时成立，
所以改完任一头部文件或生成器后要跑 `python3 tools/check_vepu580_vintage_drift.py`。
两份寄存器映射（`vepu580_regs.rs`、`h265e_regs.rs`）都必须能从生成器 + `rustfmt`
逐字节重现，H.265 那份额外加了 `windows_match_the_generated_section_table` 把
`RegisterFile` 的数组长度钉在分区表上（H.265 是六段、无 SCL，与 H.264 不同）。

---

## 2. 验证文化（不可降级）

1. **每个能力都要有 C 侧预言机级别的证据**：优先"与板上源码编译的 C MPP 逐字节相同"，
   其次"与参考实现自己打印的内部状态逐帧相同"，最后才是 PSNR/可解码。
2. **寄存器级差异要用命名字段解释**，不能只说"差 N 个字"。
   工具：`tools/mpp_ioctl_trace.c`（LD_PRELOAD 抓 ioctl）+
   `tools/vepu580_trace_diff.py` / `tools/rc_trace_report.py`（把字翻译成字段）。
3. **发现结论错了，写进 ROADMAP 并撤回**，不要悄悄改。仓库里已有两次这样的记录
   （STATUS 窗口错位一个字；把 tracer 双打印误当成 12 次作业），都保留了原文与更正。
4. **测试掉了一个也要能被发现**：加测试时若用脚本切片改文件，先 diff 模块对 HEAD
   （曾把 `mod tests` 切掉，`cargo test` 仍绿）。
5. 提交信息写"做了什么 + 为什么 + 证据"，像 `git log` 里现有的那些。
   一次提交一个可验证的结论，别把未验证功能混进已验证提交。

---

## 3. 已完成（编码器码率控制这条线）

H.264/VEPU580 编码已做到：

- **固定 QP**：4 尺寸 × 3 profile × 2 QP = 24 格 IDR，加 6 格 GOP（1 IDR + 1/3/5 P，
  静止/运动），**逐 NAL 与 C 编码器相同**，含两条 user-data SEI。脚本：
  `scripts/board_h264_qp_vs_mpp.sh`。
- **码率控制三件套**（全部有 fixture 或板上判据）：
  1. 寄存器半边 `RegisterFile::apply_rate_control`（`setup_vepu580_rc_base` 非固定 QP 分支），
     对 C 的 CBR 作业 BASE/RCKLUT/RDO **逐字相同**（仅 dma 地址不同）。
     fixture：`tests/data/vepu580_rc_c_move6_cbr_regs.txt`。
  2. 位分配 `codec::rc::model`（`bits_model_*`、`calc_next_i_ratio`），
     与 C 自己打印的 `i_scale` 两条跑法逐帧相同。
     fixture：`tests/data/rc_model_v2_c_state.txt`。
  3. 量化控制器 `calc_cbr_ratio` / `calc_vbr_ratio` + `cur_scale_qp` 记账，
     与 C 打印的六帧 × 六个数全同。fixture：`tests/data/rc_model_v2_c_cbr_qp.txt`。
- **CBR 与 VBR 已接线**：`EncoderConfig::rc_mode`，CLI `[cbr:|vbr:]target:min:max`；
  `scripts/board_h264_cbr_vs_mpp.sh` 两模式各 **10/10 NAL 相同**。

关键语义（重新实现时别弄错）：

- `bit_real = bs_lgth_l32 * 8`，`bs_lgth_l32` 就是 STATUS 第一字（硬件写的字节数，
  不含软件前缀）。`hal_h264e_vepu580.c:2389` / `:2440`。
- `quality_real = qp_sum / mbs`，`mbs = mb_w * mb_h`（16×16 块数）；
  `madi / num_b16`，`madp / num_ctu`（分母为 0 时该项取 0）。
- CBR 预算用 `bps_target`；**VBR/AVBR 预算用 `bps_max`**，但控制器朝 `bps_target` 调。
- CBR 的 `ins_ratio` 不满足"正在逼近目标"的两个条件时**归零**；
  VBR 只在高/降两种条件下计数；VBR 没有水位项。
- 首帧 intra 的 `cur_scale_qp = (quality_init + i_quality_delta) << 6`；
  `i_quality_delta` 不是逐帧偏移。
- 固定 QP 与 RC 的 `uuid_rc_cfg` SEI 文本不同（`fixqp` vs `cbr/vbr/avbr` 行），
  模式名经 `sei::RcCfgLine::with_mode`。

---

## 4. 板子与工具链（必读）

- 板子：`ssh t6`（NanoPC-T6，RK3588，内核 6.1.57）。动物 `/dev/mpp_service` 需要 `sudo`。
- 参考 C MPP：
  - `/tmp/mpp-old`（commit `80957d266`，2024-04-24）＝**编码器字节级预言机**；
    构建在 `/tmp/mpp-old/build`。SONAME 为 `librockchip_mpp.so.1`。
  - `/tmp/mpp-develop`（master）＝解码器基准。
  - `/tmp/mpp_trace.so`＝ioctl tracer（由 `tools/mpp_ioctl_trace.c` 构建）。
  - ⚠️ 它们是 `/tmp` 里的**工作副本**；权威副本在 `~/rk-mpp-oracles`
    （`mpp-old` 66M、`mpp-develop` 54M、`mpp_trace.so`、`corpus/` 438 个流与参考输出）。
    实测 t6 的 `/tmp` 不是 tmpfs（根是 overlay，`upperdir=/data/root`），
    但 `systemd-tmpfiles-clean.timer` 在跑且策略含 `D /tmp 1777 root root -`，
    所以它是会被清的位置。`scripts/setup_board_refs.sh` 先检查后回填，
    三样都缺时会 clone `80957d266` / master 并用 cmake 重建。
- 同步/构建（脚本里都在用）：
  ```bash
  COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
    ssh t6 'tar -xf - -C ~/rk-mpp-rs'
  ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
  ```
- **MPP 的调试开关是裸小写 env 名**（`mpp_env_get_u32` → `getenv(name)`，无 `MPP_` 前缀）：
  - `rc_debug=0x40`（RC 状态）、`0x80`（QP 项）、`0x20`（bps 行）、`0xf0` 全开；
  - `mpp_syslog_perror=1` 让日志同时进 stderr——否则只进 **syslog**
    （`/var/log/syslog`），这就是"`mpi_enc_test` 什么都不打印"的答案；
  - HAL 里 `hal_h264e_dbg_*` 的开关名尚未确认（`hal_h264e_debug` 未被读到），需要时读
    `mpp_env_get_u32` 的调用点。
- 参考探针：`tools/rc_probe.c`（直接驱动 MPI，能带模式/码率/gop 参数，方便配 rc_debug）；
  `tools/h264_enc_probe.c`（固定 QP 单帧 golden）；`tools/rc_data_probe.c`（MppDataV2 取证）。
- `mpi_enc_test` 与库默认不完全一致：同样参数的 CBR，`mpi_enc_test` 首帧 QP=23，
  库默认路径是 26。**与库对比用 `tools/rc_probe.c`；与 harness 对比要先把它的配置列全**。
- rkvenc 每进程只允许一个活会话（`setup_ext_line_bufs` 写共享行缓冲）。多实例前必须解。
- fixture 生成坑：`cat src dst` 会读到刚写的文件；用 `cp` 再 `cat >>`。
  `mpi_enc_test` 对非 16 倍数尺寸按 width×height 取帧，会切错（本仓库只用 16 倍数做逐字节）。

---

## 5. 下一步（按优先级）

### 5.1 AVBR —— 已完成（`12626d2`，三模式 10/10 NAL 相同）

以下为当时的实施记录，保留备查。原计划：

**源码**：`mpp/codec/rc/rc_model_v2.c`

- `calc_avbr_ratio` 961–1060
- `moving_ratio_calc` 899–960
- `moving_judge_update` 272–290
- `bit_statics_update` 291–303
- 初始化：`bits_model_init` 1062–1160（`target_bps = bps_max`、
  `min_still_percent = bps_min*100/bps_max` 或 25，`max_still_qp = 35`，
  `motion_sensitivity = 90`），`bits_model_param_init` 里新增窗口：
  `pre_i_bit`(2)、`pre_i_mean_qp`(2，初值 -1)、`pre_p_bit`(8)、`madi`(8)、`madp`(8)、
  `gop_bits`(igop)。

**现成进度**：`docs/rc-avbr-wip.patch`（本仓库，勿丢）已经包含
`model.rs` 的 AVBR 状态字段、两张表（`MEAN_QP2SCALE`、`BIT2PERCENT`）、
`alloc_with`、`moving_judge_update`、`bit_statics_update`、`moving_ratio_calc`、
`calc_avbr_ratio`、`Measured`/`end_with`，以及 `encoder.rs` 的 `RcMode::Avbr`、
`rc_probe.c` 的 mode=2。**未完成且未验证**：`src/hal/h264e.rs` 里
`RcMode::Avbr` 的四处 match（model 构造、SEI 模式名、`rc:` 匹配、`end_with` 反馈），
以及编译/测试/板上对比。

**要点（写错就会差几 QP）**：

- `bps_change = moving_ratio * bps_target >> 8`，在水位/有符号调整**之前**算；
- `gop_bits = moving_ratio * gop_total_bits >> 8`，用它调 `bits_model_alloc`；
- `bit_diff_ratio` 超支分支乘 `moving_percent = 100 * moving_ratio >> 8`；
- `i_ratio = clip(pre_i_scale >> 4, 10, 200)`，而 **`pre_i_scale` 在该版本从未被赋值**
  （恒 0 ⇒ i_ratio=10，`pre_i_scale > 640` 分支永不进）；
- `ins_ratio` 用 `tab_lnx[idx2] - tab_lnx[idx1]`（**注意顺序与 VBR 相反**），
  之后 `>>2` 再 clip；
- `rgop_dratio` / `agop_dratio` 各 clip 到 ±1；
- 若 `bps_max >= ins_bps && final_qratio > 0 && scale_qp >= max_still_qp<<6`：
  `final_qratio` 去掉 `bit_diff_ratio`，且 `qratio = -6`；
- 反馈额外需要 `quality_real`、`madi`、`madp`（编码器侧在 `encode_frame` 里已有
  `Statistics`，`tests/data` 里有 C 的 STATUS fixture 可核对）。

**验收标准（缺一不可）**：

1. `cargo test --all-targets` 全绿；
2. 用 `tools/rc_probe.c … 2`（AVBR）+ `rc_debug=0xf0 mpp_syslog_perror=1` 抓 C 侧逐帧
   `moving_ratio / bit_target / next_ratio / quality_target`，写成 `tests/data/` fixture，
   加单测逐帧比对；
3. 扩展 `scripts/board_h264_cbr_vs_mpp.sh` 加 `avbr:2` 一格，要求 10/10 NAL 相同；
4. 固定 QP 的 `ONLY=gop` 回归仍 6/6。

### 5.2 场景检测器（`vepu580_h264e_tune_stat_update`）

- 读 STATUS 的 `madi`、`madp`、`md_sad_b16num*`、`madi_b16num*`，连续三帧大运动后把
  ATR 权重与 quant bias 切到另一行。当前 `scene_motion_flag` 固定在初值。
- 现在有了 `rc_debug` 这条通路，先做实验：构造硬切序列，用 `hal_h264e_debug`/寄存器
  捕获观察 ATR 字段是否切换；确认触发条件后再移植。
- 验收：硬切序列上与 C 逐字节相同（目前所有 fixture 都没触发它）。

### 5.3 H.264 编码其余

- ROI / QP map（`roi_qthd*` 目前填的是整幅的 `qp_min/qp_max`）
- GDR / intra refresh（`INTRA_RFH_FRAME` 分支、`i_refresh_bit` 窗口）
- deblock 细调、slice split（多 slice 一行一条）
- 宽度 > 4096（CIME extension lines，当前 `validate_reference_config` 拒绝）
- `cal_first_i_start_qp`：仅在调用方不设初始 quality 时使用；照抄并补单测即可

### 5.4 H.265 编码（M8，12k）

已走到"寄存器图对上了，还没有把作业发下去"这一步：

- 参数集：`codec::h265::encoder` 的 VPS/SPS/PPS 对 320x240、640x480、1280x720
  **板上逐字节**（`scripts/board_h265_qp_vs_mpp.sh` 3/3），C 的流存在
  `tests/data/h265e_c_fixqp_qp26_*.265`，宿主测试 `parameter_sets_match_the_recorded_c_streams`
  常驻复验。两个坑别再踩：`-t` 的 HEVC 值是十进制 `16777220`（`-t 9` 是 VP8），
  NAL 第二字节是 `nuh_layer_id` 不是类型位，载荷要插防竞争字节。
- 寄存器图：`hal::h265e` 从 `h265e_vepu580_template.rs`（2024-04 C 的 320x240
  FIXQP26 单 IDR trace）出发，再按几何/QP/语法推导。`tools/h265e_trace_diff.py`
  逐字段比对，现在只剩 `enc_pic.mei_stor` 一位不同——C 的 harness 传了 motion-info
  缓冲，我们没申请。别把模板里的地址字当真：它们属于 oracle 的缓冲区。
- 提交路径已落地：`h265-idr`（`src/hal/h265e_session.rs`）自己申请输入/输出/两块 FBC
  重建面（含显示与 MV thumb）/motion-info，填 160..175 的地址字，带上 C 的
  multi-offset 表（161/162 色度、164/166 FBC 体、172 输出容量、175 packet 长度），
  提交六段寄存器并 poll，前缀由 `codec::h265::encoder::build_prefix` 拼出。
  `scripts/board_h265_idr_vs_mpp.sh`：**320x240 / 640x480 / 1280x720 整条码流与 C
  逐字节相同**（860 / 1618 / 3585 字节）。
- P 帧已通：GOP 的长期参考是**首帧 IDR**（SPS 里一条 `poc_lsb 0` 的 LT 条目，
  P 的 `synt_refm` 标 `used_by_lt_flg0`），P 都重建到另一个槽；第三张起
  `ref_pic_lst_mdf_l0`/`lst_entry_l0`/`num_negative_pics` 置 1。P 图还要把
  `lamd_modb_qp` 写进 `iprd_wgt_qp_hevc_0_51`，IDR 保留 intra 表。
  `scripts/board_h265_gop_vs_mpp.sh`：2/3/5 帧在 320x240，以及 2 帧的 640x480、
  1280x720 **整条码流与 C 逐字节相同**；横移 8 像素的动画面（1032 字节）也逐字节。
- 下一步：tiles/WPP/SAO、码率控制（CBR/VBR/AVBR）、B 帧/多参考、逐 slice 输出；
  另一条已知差异：纯推导图刻意不开 `enc_pic.mei_stor`，会话里因为自己拥有 MD 缓冲才置 1。

### 5.5 M9 后处理（IEP）

- VDPP（client 29）在 RK3588 **不存在**，不要照 MPP 的 vdpp2 实现。
- IEP（client 28）可绑定：去隔行、YUV 缩放、裁剪。入口 `mpp/vproc/iep/iep.c`、`iep2/iep2.c`。
- 现有 `src/scale.rs` 已覆盖推理链路需要的 RGB 缩放/letterbox（比 RGA 快 2.9–3.8×），
  不需要为了 YOLO 去移植 IEP；做完整性再做。

### 5.6 M10 framework（29k，最大缺口）

未动：异步任务队列 + 完成回调、dma-fence in/out、buffer import/export、
多核 CCU 分发、secure/TA、帧缓冲池引用计数（DPB 池已部分存在）。
现有只有同步 submit+poll（`src/service/`）。建议顺序：任务队列 → fence →
buffer 池引用计数 → 多核 → secure（secure 需设备支持，可能无法验证）。

### 5.7 解码器边缘

- AVS+/AVS2（client 12 可绑定，但无 conformance 流；AVS2 该板无）
- 损坏/截断码流的一致性（目前只测了"返回非零退出码"）
- VP8 profile 1/3、AV1 更宽 conformance、VP9 边界矩阵
- H.264/SCC 高 profile? 4:2:2/4:4:4 被 t6 rkvdec 拒收，属硬件限制（记录在 ROADMAP）

### 5.8 性能

- 解码吞吐脚本：`scripts/board_bench_vs_mpp.sh`（`BENCH_FILTER=` 选 pass）
- 编码吞吐：`scripts/board_bench_encode_vs_mpp.sh`（`SAMPLES=5`）
- 已测：解码全部快于 C（+55%…+738%）；编码 ≤720p 快 8–30%，1088 行**平手**（不要吹成胜利）
- 规则：两侧都**外部计时**、交错采样、给中位数；少于十几帧的流不能单独成结论。

---

## 6. 易错清单（血泪版）

1. **tracer 每段寄存器打印两次**（`before cmd=200` 一次 + 单独一次）。按 `write words=`
   行数计数会把作业数翻倍——这曾导致"每帧重编码"的错误结论。
2. **`words=` 后面第一个 token 是数量不是数据**。当数据读会把整个窗口错位一个字
   （STATUS fixture 曾经全错）。
3. **MPP env 无前缀**：`rc_debug`、`mpp_syslog_perror` 是小写裸名；
   `MPP_RC_DEBUG` 无效。日志默认进 syslog。
4. **`mpi_enc_test` 不打印**不是因为它不打印，是因为进 syslog；加
   `mpp_syslog_perror=1` 就有。
5. **`qp_sum` 按 16×16 块累加**，`num_ctu` 数 32×32 块；除以 `4*num_ctu` 才得到 QP。
   固定 QP 26 @320x240 → `qp_sum=7800`，7800/300=26。
6. **`bit_real` 是硬件字节数×8**，不含软件写的前缀（SPS/PPS/SEI）。
7. **tracer 输出很大**：永远 `grep`，不要 `cat`（曾一次烧掉 4 万 token）。
8. **`ssh t6 '...'` 单引号内 `$VAR` 不展开**；需要本地展开时用双引号或分支。
9. 板上 `/tmp` 文件由 root 创建时，`scp` 前要 `sudo chmod 644`；删除要 `sudo rm -f`。
10. **不要用 `cat a b` 造 fixture**（会读到自己）；`cp` 后 `cat >>`。
11. 复杂嵌套 heredoc 反复出问题：写成 `/tmp/x.py` 再 `python3 /tmp/x.py`。
12. 改 Rust 文件若用脚本切片，**先用 git diff 对 HEAD 校验测试模块完整**。
13. 编码器每进程单会话；两个 rkvenc 测试并行会互相干扰（GOP 金样会错）。
14. **残留的编码进程会占住硬件**：板上曾挂着一个 7.5 小时前的 `mpi_enc_test`，
    现象是脚本"卡住不报错"而不是失败。跑编码套件（或编排脚本）前先
    `ssh t6 'ps -eo pid,etime,comm | grep mpi_'` 看一眼，有残留 `sudo kill -9`；
    杀掉本地 ssh 会话并不会终止远端仍在跑的测试进程。
15. 编排脚本里调 `ssh` 要给子任务 `< /dev/null`：`ssh` 不带 `-n` 会继承 stdin，
    当外层标准输入是管道/文件时会永久阻塞（`board_regress_all.sh` 就这么卡过一次）。
    另外 macOS 自带 bash 3.2 + `set -u`：`SKIP` 这类可选变量必须显式 `SKIP="${SKIP:-}"`。
14. 固定 QP 的 24 格矩阵与 6 格 GOP 是**常回归门**，任何碰编码器的改动都要重跑。

---

## 7. 命令速查

```bash
# 本地
cargo test --all-targets && cargo fmt --check

# 板级
ONLY=gop  ./scripts/board_h264_qp_vs_mpp.sh          # 固定 QP GOP：6 格
ONLY=sizes ./scripts/board_h264_qp_vs_mpp.sh         # 固定 QP 24 格（约 10 分钟）
./scripts/board_h264_cbr_vs_mpp.sh                   # CBR + VBR
./scripts/board_h264_rc_capture.sh                   # 重采 RC fixture（REFRESH=1 覆盖）
./scripts/board_h264_status_vs_mpp.sh                # STATUS 字段级对比

# C 侧状态取证（AVBR 会用到）
scp tools/rc_probe.c t6:/tmp/
ssh t6 'cc -O2 -w -I/tmp/mpp-old/inc /tmp/rc_probe.c \
  /tmp/mpp-old/build/mpp/librockchip_mpp.so.1 -o /tmp/rc_probe'
ssh t6 'sudo env LD_LIBRARY_PATH=/tmp/mpp-old/build/mpp rc_debug=0xf0 mpp_syslog_perror=1 \
  /tmp/rc_probe /tmp/rk-mpp-rs-move-320x240.nv12 /tmp/out.264 320 240 6 \
  300000 250000 350000 2'            # 最后的 2 = AVBR（1=CBR，0=VBR）
```

---

## 8. 开工检查单

1. `cargo test --all-targets` 必须 586 通过（23 个板级 ignore）；不通过先修，别加功能。
2. `git status` 干净（除本文/补丁）；确认在 `e5e9a7f` 或其后。
3. 跑 `scripts/setup_board_refs.sh`（秒级，幂等）确认三个 oracle 齐；缺了会自动从
   `~/rk-mpp-oracles` 回填，两边都没有才会触发 clone+cmake 重建（这一步最贵）。
   之后跑 `ONLY=gop ./scripts/board_h264_qp_vs_mpp.sh` 确认真实判据仍可用。
4. 再跑 `./scripts/board_h264_cbr_vs_mpp.sh`，确认 CBR/VBR 基线仍全过。
5. 之后按第 5 节顺序推进；每完成一个小项：更新 `ROADMAP.md` → `cargo fmt` →
   全量测试 → 板级证据 → 提交（提交信息写清证据与已知边界）。
