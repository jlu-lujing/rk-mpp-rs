# rk-mpp-rs 交接文档

写给下一个接手实现的模型/人。目标是 `refs/mpp` → `rk-mpp-rs` 的完整 Rust 重写：
功能对齐、性能不劣于 C、每次验证通过就提交。

**当前阶段口径（用户 2026-11-02 决定）**：只做像素/字节级模仿，以及不改变输出的性能
提升；结果与 C 不一致即视为缺陷，除非另外证明 C 是 bug。功能前沿（tiles/WPP/SAO、B 帧、
refresh/GDR 分配等）冻结，先把已实现路径上的已知差异关掉。

**该口径之后的实际推进（截至 2026-11-07）**：deblock/SAO 细调、H.264 two-pass deflicker、
>4096 宽 P 帧与 **H.265 水平 tiles** 已按同一逐字节标准落地（tiles 见 ROADMAP 的
`H.265 (rkvenc): uniform horizontal tiles`）；AV1 软件参考解码器已完成 fractional
single-reference inter、inter residual、recursive transform tree、frame 内混合
intra/inter、inter deblock/CDEF、inter LR、average/distance-weighted/difference-weighted/wedge
compound、inter-intra、OBMC、local warp、单参考/compound global warped motion 与 temporal
MV fields，以及 inter segmentation 的 segment map、temporal prediction 与 segment
features；`delta_lf_multi` 的四路 CDF 路径已由真实彩色 keyframe 三平面与单色 Y 平面像素门
闭合，单色门同时修正 UV 语法门控和 deblock 单平面网格；混合 intra/inter selected tx
context 也已闭合完整 `av1_lr` key+P 流。
仍未闭合的前沿是 WPP、B 帧与多参考调度。
口径本身（不一致即缺陷）没有放松：tiles 的残留状态问题就是靠登记
`docs/DIVERGENCES.md` 而不是靠放宽判据处理的。

**当前板（用户 2026-11-02 再次决定）：t6。** t62 只保留为历史证据；后续同步、
构建和板级验证全部在 t6（NanoPC-T6-LTS，内核 6.1.57）执行。t6 重启后
`/tmp/mpp-old`、`/tmp/mpp-develop`、`/tmp/mpp_trace.so` 会被清理，权威副本仍在
`~/rk-mpp-oracles`，先运行 `scripts/setup_board_refs.sh` 回填。所有板级脚本默认
`BOARD="${BOARD:-t6}"`。

t6 当前持久化使用硬件最高频率的 80% OPP：little 1.416 GHz、big 1.8 GHz，
governor 固定为 `performance`。pooled JPEG 必须保证 `MppService` 在任何共享
dma-buf lease 归还前关闭；旧顺序曾让驱动在 `mpp_dma_find_buffer_fd()` 自旋锁死。
当前顺序已通过 500 轮 async demo / 3000 jobs 压测。

本文以仓库现状为准；接手时先跑 `git rev-parse --short HEAD` 和
`git status --short`，不要相信本文记录的历史提交号。`README.md` 是最新状态速览，
`ROADMAP.md` 是逐项证据台账（权威），本文是"从哪继续、怎么不被坑"的操作手册。

---

## 0. 五分钟了解现状
一页能力矩阵（做了什么、怎么证明、哪些在 RK3588 不可达）：[CAPABILITY.md](CAPABILITY.md)。


> **2026-11-02 事件：** 第一次尝试 `scripts/board_corpus_sweep.sh`（corpus 全量
> 解码对照）时，t6 在跑完不到 10 条流后从网络消失，连续约 3 分钟 ssh 超时，疑似
> 内核/驱动被某条流挂住，需要物理冷启动。脚本已加固（`TIMEOUT_S` 默认 30 秒、
> `MAX_BYTES` 默认 256 MiB、逐条打印 verdict），但**尚未完成任何一次全量运行**；
> 重启后先用 `LIMIT=10` 逐段放量，并记录卡住的是哪一条流。

> **2026-11-02 t62 事件（第二次）：** `tests/data/av1_tiles_320x240_10.ivf`
> 会触发 `rk_vcodec: mpp_task_timeout_work ... processing time out!` 与
> `mpp_av1dec ... resetting`，并可导致整板重启（内核 6.1.141）。当时进一步做
> **只含第一个 2,351-byte coded frame 的 IVF 前缀**后，C 仍能输出一帧；Rust
> 报告一个输出但 YUV 为 0 字节，内核再次 reset/reboot。这不是 t6 全量结论的
> 自动延伸：t6 是 6.1.57，t62 是 6.1.141；同一 SoC 不等于同一驱动执行行为。
> 根因是 Rust 少了 C 的 CDEF/loop-restoration filter DMA base：
> `reg[85]`/`reg[91]` 为 0，STATUS `0x2109` 的 `sw_dec_bus_int` 正是总线错误。
> 同时对齐 C 的 uncached DMA32 表/帧、DMA32 stream 和 4:2:2 大小的 display
> allocation。修复后单帧 STATUS `0x1100`，完整 fixture 10 帧/1,152,000 bytes
> 与当前 C 逐字节一致，8/10-bit film-grain 硬件回归也通过。
>
> 修复后 tile fixture 已从 corpus quarantine 移回常规扫描。安全措施保留：
> 1. `POLL_HW_FINISH` 改为 5 秒上限（非阻塞轮询 + EINVAL 回退），避免进程永久挂住；
> 2. 修复合入前曾把该夹具在任何解码之前 `QUARANTINE`；stub 验证过带/不带
>    `td_` 前缀均不会启动解码器。根因修复后该 quarantine 已移除，fixture
>    重新作为 corpus sweep 的 AV1 tile 覆盖项。
> 复现时连带检查 `dmesg` 是否有 `mpp_task_timeout_work`；不要盲目重复跑。
>
> **安全诊断入口（2026-11-02）：** `examples/av1_dump_regs.rs` 现在可以在
> **不打开 `/dev/mpp_service`** 的情况下，为 tile fixture 的 11 个作业导出
> PicParams、输入 CDF、global model、tile-info、stream、film-grain 和 512-word
> 寄存器/地址偏移镜像。配套的 `tools/av1/compare_input_dumps.py` 与 C
> `DUMP_AV1_DATAS` 逐字节及逐字段比较，并归一化两边不同的 DMA fd：
>
> ```bash
> cargo run --example av1_dump_regs -- \
>   tests/data/av1_tiles_320x240_10.ivf 11 /tmp/av1-rust
> python3 tools/av1/compare_input_dumps.py /data/video /tmp/av1-rust
> ```
>
> 离线证据：导出器自行重放 Rust 的 DPB 槽位分配，不从 C snapshot 抄 current/
> reference slot；11/11 PicParams 仍与 C golden 相同，88 个镜像文件尺寸正确。
> 比较器在模拟一致输入上 11/11 PASS，并能在单字破坏时定位到具体寄存器。
> t62 实际 C dump 已证明：作业 0-10 的 PicParams、global model、tile-info、
> split stream、film-grain、512-word 寄存器和地址偏移全部相同。作业 0-2 的输入
> CDF 也相同；作业 3+ 的 CDF 依赖上一硬件作业写回，普通 CPU 导出无法独立复现，
> 比较器将其记作 `DEFERRED`，不会掩盖其它镜像或寄存器差异。
>
> t62 恢复后先运行安全预检，不要直接跑 `av1dec`：
>
> ```bash
> BOARD=t62 ./scripts/board_av1_presubmit_compare.sh
> ```
>
> 脚本会从当前提交临时打包源码，在独立的
> `~/rk-mpp-oracles/mpp-av1-dump-v2` 构建 C dump oracle，运行已知正常的 C 解码，
> 再运行 CPU-only Rust 导出器并比较全部输入镜像。它不会向 Rust AV1 提交硬件
> 作业；只有打印 `PRE-SUBMIT DETERMINISTIC INPUTS MATCH` 后才恢复板级 AV1 调试。

## 第二块板：t62（2026-11-02 起）

`ssh t62`（用户 `eddysun`）是另一台 NanoPC-T6-LTS：内核 6.1.141、`/dev/mpp_service`
可用、`hw_support mask = 0x1009321a`，与 t6 相同。已在这台机器上复验：VP8 profile 1/3
与缩放关键帧、VP9 六格矩阵、H.265 三尺寸 13 格码控，结果与 t6 一致。

搭建要点（新板照做即可）：

1. **oracle 必须用 git checkout 构建，不能用源码 tarball。** 版本 SEI 由
   `CMakeLists.txt` 在 `${PROJECT_SOURCE_DIR}/.git` 存在时用
   `git log -1 --pretty=format:"%h author: %<|(30)%an %cd %s"` 生成；没有 `.git`
   时会写 `unknown mpp version for missing VCS info`，整条 H.265 码流就会在第 ~90
   字节差一个 SEI 文本。用 `git bundle` 传仓库再 `git checkout 80957d266` 即可。
2. **cargo 在这台板是 offline + Debian registry**（`~/.cargo/config.toml`），
   registry 里只有 `libc-0.2.169`。复制过来的 `Cargo.lock` 若锁定别的版本会解析
   失败；删掉锁文件让 cargo 就地解析即可。
3. 新脚本统一使用 `BOARD="${BOARD:-t62}"`，默认不再写死 t6。驱动脚本时从
   **另一个目录**（如 `~/rk-mpp-run`）运行，因为有些脚本会打包当前目录并在
   同一目标解压；在目标目录里运行会自己吞自己。
4. `ffmpeg`/`ffprobe` 需要 `apt-get install`，矩阵脚本的 NV12→planar 依赖它们。板上的 ffmpeg **没有 AV1 编码器**（无 libaom-av1），AV1 大尺寸 fixture 要在宿主生成后 scp，小尺寸已固化在 `tests/data/av1_{96x96,128x128,192x192}_10.ivf`。
  AV1 默认 CDF 表由 `tools/gen_av1_cdf.py` 从规范 HTML 生成：`curl -sL https://aomediacodec.github.io/av1-spec/ -o /tmp/av1spec.html && python3 tools/gen_av1_cdf.py /tmp/av1spec.html > src/reference/av1/cdf_defaults.rs`。
5. t62 上已有重新生成的覆盖语料 `~/rk-mpp-oracles/corpus-gen`（59 个 payload；
   从当前提交重跑后会再写一个 `corpus-manifest.json`）。原 t6 的第三方 corpus
   无法原样复制；扫描时使用 `CORPUS=.../corpus-gen`，并确保从当前提交重新运行
   `scripts/gen_corpus.sh`，不要使用过期的 MPEG-4 容器副本。manifest 会在任何
   解码前校验文件集合、大小、SHA-256、IVF FourCC 和 raw elementary-stream magic；
   `.m4v` 若实际是 MP4 容器会直接失败。验证：`tests/data` 的 142 个文件写/读
   清单均通过，单字节修改和伪 `.m4v` 均被拒绝。
   当前 `corpus-gen4` 有 66 个 payload，AV1 扩展到 640x480 long-GOP、
   1280x720、1920x1080、Main10、多 sequence header 和 320↔640 动态分辨率；
   全量结果为 `pass=60 fail=0 named=6 skipped=0`。AV1 tile 已是普通 PASS。64×64 AV1
   在 C 上也会逐帧自恢复 reset，C/Rust 均输出十帧零图，不能单独归因于 Rust。
6. **H.264 QP 矩阵需要参考 harness 的 level 补丁。** `mpi_enc_test.c` 把
   `h264:level` 写死为 40，而 crate 的 Constrained Baseline 是 30；`board_h264_qp_vs_mpp.sh`
   现在会连同 profile/cabac/trans8x8 一起把 level sed 成 30，所以新板不需要预置打过补丁
   的 `/tmp/mpi_enc_test.c.orig`。在这之前，t62 上 30 格全部因为 SPS level 不同而失败。

```bash
cd ~/workspace/rknn-test/rk-mpp-rs
cargo test --all-targets      # host/t6 均 1118 passed；31 个硬件 ignored 另跑通过
cargo fmt --check
python3 scripts/check_corpus_manifest.py --write tests/data \
  --manifest /tmp/tests-data-manifest.json
python3 scripts/check_corpus_manifest.py --check tests/data \
  --manifest /tmp/tests-data-manifest.json
git log --oneline -10
grep -c '^- \[x\]' ROADMAP.md  # 已完成条目数
grep -c '^- \[ \]' ROADMAP.md  # 未完成条目数
```

板上验证（`BOARD` 默认 t6，详见第 4 节）：

```bash
ONLY=gop ./scripts/board_h264_qp_vs_mpp.sh   # 固定 QP：6 格全 PASS 约 3 分钟
./scripts/board_h264_cbr_vs_mpp.sh           # CBR + VBR + AVBR 各 10 NAL 全同
./scripts/board_h264_level_vs_mpp.sh         # SPS level 矩阵 13 格
./scripts/board_h264_deblock_vs_mpp.sh       # deblock 3 格 + auto-QP 2 格
./scripts/board_h264_roi_vs_mpp.sh           # ROI/QP map 2 格
./scripts/board_h264_scene_vs_mpp.sh         # 硬切场景检测
./scripts/board_h264_slice_vs_mpp.sh         # 普通整 AU slice split 4/4
./scripts/board_h264_lowdelay_vs_mpp.sh      # 逐 slice packet 5/5
./scripts/board_jpege_restart_vs_mpp.sh      # JPEG restart（-t 8 + split env）
./scripts/board_jpege_lowdelay_vs_mpp.sh     # JPEG 逐 band packet 2/2
./scripts/board_jpege_multicore_vs_mpp.sh    # JPEG 默认多核 + 旋转 5/5
./scripts/board_jpege_stride_vs_mpp.sh       # JPEG stride 2/2
./scripts/board_h265_gdr_vs_mpp.sh           # H.265 GDR row/column 4/4
./scripts/board_h265_slice_vs_mpp.sh         # H.265 普通整 AU slice 3/3
./scripts/board_h265_lowdelay_vs_mpp.sh      # H.265 逐 slice packet 4/4
./scripts/board_h265_roi_vs_mpp.sh           # H.265 ROI/QP map 4/4
./scripts/board_h265_tile_vs_mpp.sh          # H.265 水平 tiles 2/2
./scripts/board_iep_modes_vs_probe.sh        # IEP2 全模式 + 原始 dst slots 9/9
./scripts/board_iep_formats_vs_probe.sh      # IEP2 420/422 SP/Planar 6/6
./scripts/board_iep_dect_vs_probe.sh         # IEP2 PD/DECT 逆 telecine + 几何边界 5/5
./scripts/board_vp_corrupt_vs_mpp.sh         # 损坏流 vs C 判决表
```

ROADMAP 顶层清单当前为 78 项完成、6 项未完成；未完成项包含硬件不存在、无
conformance 流和冻结的功能前沿，不能直接用勾选比例估算代码工作量。已完成路径里
H.264/H.265 编码、JPEG、IEP2 和主要解码矩阵都有板上逐字节证据。
AV1 tile 的 t62 超时已作为历史事件记录并完成根因修复；当前工作板回到 t6。
H.264/H.265 普通 slice split 与低延迟逐 slice packet 都已在 t6 上通过。其它已知差异和
下一步证据见 `docs/DIVERGENCES.md` 与 ROADMAP。

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
| `src/reference/` | 自研软件参考解码器（M11：CABAC、HEVC 反变换、H.264 CAVLC/4×4+8×8 扫描/反量化/残差/DC；AV1 key/intra 滤波链、SavedCdf、单参考 integer/fractional inter、average/distance-weighted/difference-weighted/wedge compound、inter-intra、OBMC、local warp、单参考/compound global warp 与 temporal MV fields、inter residual 与 frame 内混合 intra/inter、多 tile tile-group：逐 tile 从帧初始 CDF 重启、帧末 CDF 取 context_update_tile_id，帧级 mi/segment 状态跨 tile 携带而邻块上下文、MV 单点候选、OBMC/projection 与 CDEF 单元按 tile 裁剪。2×2 tile 的 10 个 inter coded frame 已全部与 `aomdec` Y/U/V 逐字节一致；temporal motion field 会按 half-res cell 清除 intra 覆盖、在 base-invalid 时继续 extension probes、应用 order-hint sign bias，difference-weighted chroma mask 使用 luma 域重算） |
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

- 当前板：`BOARD="${BOARD:-t6}"`（NanoPC-T6-LTS，RK3588，内核 6.1.57）。
  `/dev/mpp_service` 需要 `sudo`。
- 参考 C MPP：
  - `/tmp/mpp-old`（commit `80957d266`，2024-04-24）＝**编码器字节级预言机**；
    构建在 `/tmp/mpp-old/build`。SONAME 为 `librockchip_mpp.so.1`。
  - `/tmp/mpp-develop`（master）＝解码器基准。
  - `/tmp/mpp_trace.so`＝ioctl tracer（由 `tools/mpp_ioctl_trace.c` 构建）。
  - ⚠️ 它们是 `/tmp` 里的**工作副本**；权威副本在 `~/rk-mpp-oracles`
    （`mpp-old`、`mpp-develop`、`mpp_trace.so`、`corpus-gen/`）。t62 的 `/tmp`
    是 tmpfs；t6 即使不是 tmpfs 也会被 tmpfiles 清理，两者重启后都应先运行
    `scripts/setup_board_refs.sh` 检查并从 oracle home 回填。
- 同步/构建（脚本里都在用）：
  ```bash
  BOARD="${BOARD:-t6}"
  COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
    ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
  ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
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
- rkvenc 同进程多会话**可用**：1/2/3 线程在 64x64/320x240/640x480 产出一致，且与 C 参考逐 NAL 相同。
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

### 5.3 H.264 编码当前边界

- 已闭合的路径包括固定 QP / CBR / VBR / AVBR、ROI/QP map、GDR、deblock、
  `cal_first_i_start_qp`、场景检测、普通 slice split、逐 slice low-delay packet、
  宽度 >4096 的 CIME 扩展行以及 P 帧。证据入口分别是
  `board_h264_slice_vs_mpp.sh`（4/4）与 `board_h264_lowdelay_vs_mpp.sh`（5/5）。
- 尚未开始的新功能：hier QP / SMART_P、重编码 / 多 pass、super frame 和相应
  drop/pskip 策略。它们仍属于冻结的功能前沿，不与当前等价性回归混提。
- JPEG 编码的 low-delay packet 输出已闭合：单 job 保持原字节流，`EncodedJpeg`
  在 RST 标记处暴露 packet ranges；320x240/5 packet 与 640x480/10 packet 均与
  C 逐字节、逐 packet 相同。low-delay part 的旋转和 >720p 组合仍明确拒绝；
  JPEG 多核分带已单独关闭下面一行。
- JPEG 默认四 band 多核路径已闭合：GOP 无关的 streaming `MULTI_MSG` 提交、
  `SET_SESSION_FD` delimiter、scratch band 和 reg53 拼接均按 C 实现；
  1920x1080 qf50/qf90 + 1288x722 + rot90/rot270 为 5/5，
  322x242/1366x768 stride 为 2/2。显式 split 产生超出画面的 band 时 C 会输出无 EOI 的无效
  JPEG，记录为 C-BUG，Rust 保留有效流。

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
- CBR 通路也已逐字节：`h265-gop-rc` 喂 C 的逐帧决策（IDR 26/86955、P 25/582）后，
  含 CBR PPS、`cbr` SEI 行与两个 slice NAL 的 1169 字节流与 C 完全一致
  （`scripts/board_h265_cbr_vs_mpp.sh`）。注意 GOP 的 LT 寄存器状态对 IDR 也要置位。
  STATUS 测量已移植（`measured_from_status`：`quality = qp_sum/(mb64<<6)`、
  `madi/num_b16`、`madp/num_ctu`），CBR 模型环也接上了：首帧决策与 C 一致，
  前缀与 IDR NAL 逐字节。第二帧仍不同（我们 9320 bits/QP27，C 582/QP25）；
  测量 bits 与 C 只差 24，所以问题在模型的 VBV/`calc_ratio` 状态折叠，不在测量。
- 三种码控都已逐字节（纯 GOP、模型自选 QP）：CBR/VBR 1163 字节、AVBR 1164；
  显式决策格 1169 字节。`-rc` 编号以 SEI 实测为准：0:vbr 1:cbr 2:fixqp 3:avbr。
- H.265 GDR 已闭合：C probe 通过显式 short-term ref cfg 真正触发 `is_i_refresh`；
  Rust 按 IGOP 边界调度 refresh、交替双 DPB、在边界重发 VPS/SPS/PPS +
  recovery-point SEI。320x240/640x480 × row/column 为 **4/4 整条逐字节**。
- slice split 已闭合：普通整 AU 的 `board_h265_slice_vs_mpp.sh` 为 **3/3**
  （CTU 8、BY_BYTE 256、640x480 的 40-slice 格）。
  `SliceSplit` 覆盖 `MPP_ENC_SPLIT_BY_BYTE/BY_CTU`，
  `EncodedFrame.packets` 把 VPS/SPS/PPS+SEI 放在首 packet，并按
  `POLL_HW_IRQ` 的 slice length 切分。`board_h265_lowdelay_vs_mpp.sh` 为
  **4/4**：320x240 CTU 8（3 packet）、BY_BYTE 256（9 packet）、640x480 CTU 2
  （40 packet，跨 32 项 FIFO）和 6 帧 short-term IDR+P（18 packet）。完整码流
  和每个 packet 长度均与 C probe 相同；默认 IDR/GOP/GDR/三模式码控回归均通过。
- ROI/QP map 已闭合：`codec::h265::roi` 复刻参考 helper 的 16×16 区域填充、
  HEVC CTU 标记、CU64/32/16/8 展开、force-split 和 base/QP 两张 DMA 表。
  `board_h265_roi_vs_mpp.sh` 为 **4/4**：相对 QP、绝对 QP+force-intra、多 region、
  640x480；六帧 short-term GOP 整条码流逐字节一致。
- 水平 tiles 已闭合：`build_pps` 写 `tiles_enabled_flag`/`num_tile_columns_minus1`，
  每列一个 job，`reg0252_tile_cfg`/`reg0253_tile_pos`/`reg212_rc_cfg.rc_ctu_num`/
  tile 相对的 CIME line buffer 窗口/`reg0193_dual_core` 握手/`reg0176-177` 交替
  line buffer 全部照 `hal_h265e_v580_set_uniform_tile`。RK3588 只到两列
  （`auto_tile` 或宽度 >4096），四列需要 >8192 宽度、硬件不接受。
  `board_h265_tile_vs_mpp.sh` 为 **2/2 逐字节**；`tools/h265e_trace_diff.py --job=0/1`
  显示除 10 个 dma-buf fd 字与 `mei_stor` 外无一个非地址字不同。
  tile + low-delay 逐 slice 输出**显式拒绝**（C 跨 tile 的 FIFO 行为无证据）。
- `-g 60` 是 **harness 误用**：`sscanf` 只填 `gop_mode=60`，`gop_len=vi_len=0`，MPP 据此生成
  `lt_gap=0` + 两个 `REF_TO_PREV_INTRA` 的病态调度；我们的 API 拒绝 `gop=0`。新 cell 用 `-g 0:60:0`。
- WPP、B 帧与多参考在 C 参考里不存在（见 ROADMAP），不是待写代码。

### 5.5 M9 后处理（IEP）

- VDPP（client 29）在 RK3588 **不存在**，不要照 MPP 的 vdpp2 实现。
- IEP2（client 28）可绑定，但 `iep2_params` 只有去隔行，没有 YUV scale/crop 字段。
  通用 scale 属于 legacy `/dev/iep`；t6 没有该节点，driver 也不绑定。
- `tools/iep2_probe.c` 可按数值 mode 驱动 client 28，并原样输出 dst0/dst1；
  **PD/DECT 需要单独探针**：`iep2_test` 写死 I5O2，永远走不到 `iep2_check_pd`/`iep2_check_ffo`，
  用 `tools/iep2_dect_probe.c`（同一测试改 DECT + 每帧打印 `iep2_api_info`），
  `board_iep_dect_vs_probe.sh` 5/5 逐字节且逐帧状态相同（telecine、逐行、322x242、722x482、1366x768）。
  `board_iep_modes_vs_probe.sh` 为 **9/9**：I5O2（TFF/BFF）、I5O1T/B、I2O2、
  I1O1T/B、DISABLE、BYPASS。T 模式只写 dst0，B 模式只写 dst1，未用槽为零。
- 格式矩阵已闭合：420/422 × SP/Planar × UV/VU 为 **6/6**，输入 fixture、
  C probe 和 Rust 两侧同一格式，两个 raw dst slot 逐字节一致。
- 现有 `src/scale.rs` 已覆盖推理链路需要的 RGB 缩放/letterbox（比 RGA 快 2.9–3.8×），
  不需要为了 YOLO 去移植 IEP；做完整性再做。

### 5.6 M10 framework（29k，最大缺口）

已闭合：同步 submit+poll；异步任务队列、完成回调/wait/bounded event；
每任务排队/执行/端到端耗时，以及 submitted/done/failed/cancelled 和队列高水位
统计；引用计数 dma-heap 池、import/dup_fd、池复用和并发压测。
`scripts/board_framework_async_vs_sync.sh` 与 `framework_async_hw`/
`framework_pool_hw` 已在 t6 通过。

仍缺：内核协议没有 dma-fence；多核 CCU、secure/TA 无可用接口或设备。
VP8 与 H.264/HEVC/AV1/VP9 的池化帧已经真正持有 DPB lease；H.265
保留 DPB 引用，AV1 只复用非参考帧，VP9 保护 hardware previous/last
colmv。下一步优先做真实 STATUS 异常恢复，而不是重复已完成的队列基础设施。

### 5.6.1 W12 RK3576 bring-up（新板，不是 RK3588 的别名）

- lf1 是 Luckfox Omni3576 / RK3576，使用 VDPU383、VEPU510、VPU720 和
  VDPP2；不能复用 RK3588 的 VDPU34x/VEPU580 寄存器写法。
- `service::soc` 已按 device-tree compatible 选择 HAL，RK3588-only 入口在
  RK3576 上显式拒绝；`probe` 会打印 SoC、模型和预期代际。
- 已生成 276-word VDPU383 寄存器表，并完成首个 H.264 硬件路径。lf1 的
  640×480、12 帧 High profile 动画面与官方 C MPP 逐字节，SHA-256 均为
  `615ee129c46e10f7fb4cb2cf8af3cb370523f7605abee7e7315e073a4421b112`。
- 前两帧 VDPU383 `global_cfg` 和 `rps` 也与 C 调试 dump 逐 word 相同。
  关键板级差异是 704 stride、未压缩 colmv，以及 DPB flags/MVC layer ids
  共用一个 32-bit word。
- H.264 边界 5/5 已在 lf1 通过：720p/B 30 帧、1080p hard 90 帧、PAFF
  15 帧、LTR 62 帧和缺失参考 61 AU/22 输出帧。前两条、LTR 与缺失参考
  与 C MPP 逐字节；PAFF 与 FFmpeg
  逐字节，C harness 的场序输出是既知差异。入口为
  `scripts/board_rk3576_h264_edges_vs_mpp.sh`。
- H.265 初始 10/10 矩阵已在 lf1 通过：320×240 单 IDR、640×480 IPPP 与
  B-pyramid、Main10、2×2 tiles、真实 WPP（640×360，5 个 entry point）、
  1280×720 WPP、JCT-VC DSLICE，以及显式 SPS/PPS scaling list；Rust/YUV 与
  C MPP 逐字节相同。176-byte VDPU383 global packet 与 C dump 相同，单测固定
  FNV `0xe97a26406128e428`。
- H.265 的 LTR/错误注入和更宽 conformance 矩阵仍需 lf1 实跑；
  `scripts/board_rk3576_h265_vs_mpp.sh` 是当前入口。
- VDPU383 VP9 初始 7/7 已在 lf1 通过：profile 0/2、8/10-bit、lossless、
  errres tiles 和 720p；普通行与 C MPP 逐字节，720p errres 与 321x241
  继续按既有 libvpx 判据处理 C 侧 bug。VDPU383 的 global header、1312-byte
  keyframe probe、11 个 RCB region 和 `width * height / 2` colmv 已接入。
  `scripts/board_rk3576_vp9_vs_mpp.sh` 是当前入口；lf1 release LTO 当前会
  在 LLVM SIGSEGV，因此功能门使用 debug。
- VPU720 JPEG 已在 lf1 通过 19/19：NV12 三尺寸+60-MCU restart，320x240
  NV16/I420/NV21/YUV422P/NV61/YUYV/YVYU/UYVY/VYUY/NV24/YUV444P/YUV400，
  以及 400/420/422/444 输出均与 C MPP 逐字节；实现使用 client 20、43-word
  base register window 和 384-byte 变换 Q-table。`EncoderConfig::input_layout()`
  统一给出平面存储大小、U/V offset、chroma stride 与 UV-swap，VPU720 再映射
  到 C 的 source/output format。C 的 422/444 输出对 NV12 fixture 只有 623
  bytes，Rust 只承诺复现该行为。`scripts/board_rk3576_jpege_vs_mpp.sh` 是
  当前入口；tile input 仍待做。C 的 VPU720 HAL 没有 RGB 输入、rotation 或
  low-delay `part_start`/`part_wait`，Rust 对它们显式拒绝。
- t6 回退门仍使用 VDPU34x：既有 7/7 H.264 回归通过，证明分派没有破坏
  RK3588。
- 下一步：把同一 VDPU383 方法推进到 AV1/AVS2，再做 VEPU510、VPU720
  tile input 与 VDPP2。RK3576 未被硬件暴露的
  VP8/MPEG-2/MPEG-4/H.263 要由能力矩阵明确拒绝。

### 5.7 解码器边缘

- AVS+/AVS2（client 12 可绑定，但无 conformance 流；AVS2 该板无）
- 损坏/截断码流的一致性（目前只测了"返回非零退出码"）
- VP8 profile 1/3、AV1 更宽 conformance、VP9 边界矩阵
- H.264/SCC 高 profile? 4:2:2/4:4:4 被 t6 rkvdec 拒收，属硬件限制（记录在 ROADMAP）

### 5.8 性能

- 解码吞吐脚本：`scripts/board_bench_vs_mpp.sh`（`BENCH_FILTER=` 选 pass）
- 编码吞吐：`scripts/board_bench_encode_vs_mpp.sh`（`SAMPLES=5`）
- 当前证据只支持“解码/编码不比 C 慢”。同一二进制在 t62 上 720p H.264 可从
  72.9 ms 漂到 120.1 ms；未做交错采样的百分比不进入结论。
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

   **H.265 tile 首帧没有跨会话稳定的 C oracle**（2026-11-04）：同一个 C probe、同一份输入、同一组参数，
   紧跟我们会话时输出 `9724d029..`、紧跟它自己会话时输出 `b27d68dc..`，而且两者**解码出的像素也不同**
   （`7dc42fe5..` 对 `c9fcd4f7..`）——VEPU580 的双核状态跨进程保留，tile 首帧会读到它；中间归还
   512 MiB 归零页也不变，说明状态在硬件块内而非内存，而 `MPP_CMD_RESET_SESSION` 只清队列。
   因此两侧各跑一趟丢弃作业：我们据此把自己钉在 `9724d029..`，`board_h265_tile_vs_mpp.sh` 也在 C
   编码之前先跑一次我方编码，把 C 送进同一状态。**这个预热不是冗余代码**：实测去掉它之后，只要
   前一个脚本的最后一次编码是 C 侧，`auto320` 就 FAIL。逐字节结论的边界只在"该状态内"，写文档不要越过。
8. **`ssh "$BOARD" '...'` 单引号内 `$VAR` 不展开**；需要本地展开时用双引号或分支。
9. 板上 `/tmp` 文件由 root 创建时，`scp` 前要 `sudo chmod 644`；删除要 `sudo rm -f`。
10. **不要用 `cat a b` 造 fixture**（会读到自己）；`cp` 后 `cat >>`。
11. 复杂嵌套 heredoc 反复出问题：写成 `/tmp/x.py` 再 `python3 /tmp/x.py`。
12. 改 Rust 文件若用脚本切片，**先用 git diff 对 HEAD 校验测试模块完整**。
13. **不要把金样过期误判成并发干扰**（2026-11-04 撤回旧结论）：`h264_encoder_hw` 的两个用例
    曾以“两个 rkvenc 会话互相干扰”为由要求 `--test-threads=1`。实测串行同样失败——真正原因是
    金样早于两个 SEI（version + rc_cfg）加入码流；而 `setup_ext_line_bufs` 在 C 里只分配本上下文
    缓冲，且这些尺寸根本不到 >4096 的 ext 路径。重取 C 金样后，1/2/3 线程都通过，并新增了
    并发会话用例。判据：**串行也失败 ⇒ 不是并发问题**。
14. **残留的编码进程会占住硬件**：板上曾挂着一个 7.5 小时前的 `mpi_enc_test`，
    现象是脚本"卡住不报错"而不是失败。跑编码套件（或编排脚本）前先
    `ssh "$BOARD" 'ps -eo pid,etime,comm | grep mpi_'` 看一眼，有残留 `sudo kill -9`；
    杀掉本地 ssh 会话并不会终止远端仍在跑的测试进程。
15. 编排脚本里调 `ssh` 要给子任务 `< /dev/null`：`ssh` 不带 `-n` 会继承 stdin，
    当外层标准输入是管道/文件时会永久阻塞（`board_regress_all.sh` 就这么卡过一次）。
    另外 macOS 自带 bash 3.2 + `set -u`：`SKIP` 这类可选变量必须显式 `SKIP="${SKIP:-}"`。
16. 固定 QP 的 24 格矩阵与 6 格 GOP 是**常回归门**，任何碰编码器的改动都要重跑。

---

## 7. 命令速查

```bash
BOARD="${BOARD:-t6}"

# 本地
cargo test --all-targets && cargo fmt --check

# 板级
ONLY=gop  ./scripts/board_h264_qp_vs_mpp.sh          # 固定 QP GOP：6 格
ONLY=sizes ./scripts/board_h264_qp_vs_mpp.sh         # 固定 QP 24 格（约 10 分钟）
./scripts/board_h264_cbr_vs_mpp.sh                   # CBR + VBR
./scripts/board_h264_rc_capture.sh                   # 重采 RC fixture（REFRESH=1 覆盖）
./scripts/board_h264_status_vs_mpp.sh                # STATUS 字段级对比

# C 侧状态取证（AVBR 会用到）
scp tools/rc_probe.c "$BOARD":/tmp/
ssh "$BOARD" 'cc -O2 -w -I/tmp/mpp-old/inc /tmp/rc_probe.c \
  /tmp/mpp-old/build/mpp/librockchip_mpp.so.1 -o /tmp/rc_probe'
ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-old/build/mpp rc_debug=0xf0 mpp_syslog_perror=1 \
  /tmp/rc_probe /tmp/rk-mpp-rs-move-320x240.nv12 /tmp/out.264 320 240 6 \
  300000 250000 350000 2'            # 最后的 2 = AVBR（1=CBR，0=VBR）
```

---

## 8. 开工检查单

1. `cargo test --all-targets` 必须 1118 通过（31 个板级 ignore）；不通过先修，别加功能。
2. `git status` 干净（除本文/补丁）；确认在 `e5e9a7f` 或其后。
3. 跑 `scripts/setup_board_refs.sh`（秒级，幂等）确认三个 oracle 齐；缺了会自动从
   `~/rk-mpp-oracles` 回填，两边都没有才会触发 clone+cmake 重建（这一步最贵）。
   之后跑 `ONLY=gop ./scripts/board_h264_qp_vs_mpp.sh` 确认真实判据仍可用。
4. 再跑 `./scripts/board_h264_cbr_vs_mpp.sh`，确认 CBR/VBR 基线仍全过。
5. 之后按第 5 节顺序推进；每完成一个小项：更新 `ROADMAP.md` → `cargo fmt` →
   全量测试 → 板级证据 → 提交（提交信息写清证据与已知边界）。
