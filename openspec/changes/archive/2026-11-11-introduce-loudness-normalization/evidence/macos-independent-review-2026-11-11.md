# 响度归一化 macOS 证据独立复核（2026-11-11）

复核方式：不采用实现者的结论，重新从磁盘读取报告、素材、运行库与代码，重算每个 dB 数、每个哈希、每条规格同步状态；并用**全新生成的 fixture**、**全新进程**把 macOS 探针完整重跑一次，与首次报告逐项比对。复核者没有拿到实现者的推理过程，只拿到 `openspec/changes/introduce-loudness-normalization/`、仓库路径与两份 JSON 报告。

- 首次报告：`evidence/macos-normalization-2026-11-11.json`
- 独立重跑：`evidence/macos-normalization-independent-rerun-2026-11-11.json`
- P0 参数路径基线对照：`evidence/macos-audio-effects-runtime-baseline-2026-11-11.json`
- 运行库：`apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib`，SHA-256 `536cc29610c2cb10…`
- 两次运行的运行库哈希、分发清单哈希、fixture 清单哈希完全一致；报告都保持 `gate_passed=false`。

## A. 证据数值复核（43 项，全部 CONFIRMED）

| 结论 | 复核项 | 实测 |
| --- | --- | --- |
| CONFIRMED | libmpv / manifest / 两个输入素材的 SHA-256 与磁盘字节一致 | `536cc296…` / `54485a92…` / `88085245…` / `1ffe0782…` |
| CONFIRMED | 理论 `10^(±6/20)` | `1.995262314969` / `0.501187233627` |
| CONFIRMED | 最小链结构（两次运行） | 3 节点、无 equalizer/extrastereo、limiter 末位 |
| CONFIRMED | 静态 +6 dB 参考（harness 自检，两次） | `5.996256 dB`，误差 `-0.003744 dB`，`harness_valid=true` |
| CONFIRMED | 运行时 `0 → -6 dB`（两次） | `-6.038842` / `-6.044049 dB`，误差 `-0.038842` / `-0.044049 dB` |
| CONFIRMED | 运行时 `0 → +6 dB`（两次） | `+5.994225` / `+5.989077 dB`，误差 `-0.005775` / `-0.010923 dB` |
| CONFIRMED | 关闭回落 `+6 → 0 dB`（两次） | `-5.982563` / `-5.989066 dB`，误差 `+0.017437` / `+0.010934 dB` |
| CONFIRMED | 三次运行时命令均在 EOF 前发出、rc ≥ 0、误差 ≤ 0.25 dB | `eof_before_command=false`，`runtime_confirmed=true` |
| CONFIRMED | 压力素材（0 dB 与 +6 dB 增益，两次） | 峰值 `0.891265869` = `-0.999854 dBFS`，≤ 0 dBFS |
| CONFIRMED | 所有 `error_db` / `peak_dbfs` 由原始测量值独立重算 | 差 < 1e-9 |
| CONFIRMED | 两次运行逐项差异 | dB 差 ≤ 0.006 dB；峰值完全相同 |
| CONFIRMED | 报告不宣称发布 Gate 通过、不宣称其他平台已验证、列明 5 项未测内容 | `gate_passed=false` / `deferred` / `not_measured` |

## B. 规格与实现一致性复核（21 项，全部 CONFIRMED）

| 结论 | 复核项 | 实测 |
| --- | --- | --- |
| CONFIRMED | `0031` 迁移恰好 5 个可空列 | `loudness_source` / `*_gain_mdb` ×2 / `*_peak_ppm` ×2 |
| CONFIRMED | `SONG_SELECT` 列尾顺序与迁移一致；`conversion.rs` 读 21..25 | 索引与列顺序逐项对应 |
| CONFIRMED | `song_full_snapshot_payload` 不含响度列 | 同步载荷形状未变 |
| CONFIRMED | R128 对齐 `+5 dB`（`-18 - (-23)`） | `TARGET_LUFS=-18` |
| CONFIRMED | 增益边界 `-24..+6 dB`；preamp `-12..+12 dB` / 0.5 dB 步进；合并 `-60..+6 dB` | 常量逐项核对 |
| CONFIRMED | 关闭时合并前置增益等于 P0 `effective_preamp_db` | 中性分支 |
| CONFIRMED | limiter 在 `install()` 末位；仅增益模式跳过 EQ/空间节点 | `guard` 分支存在 |
| CONFIRMED | `Configuration` 派生 `PartialEq`，preamp 纳入斜坡目标比较 | 切歌增益变化不会被缓存成无变化 |
| CONFIRMED | 默认偏好为关闭 / Track / 0 dB | `NormalizationPreference::default()` |
| CONFIRMED | UI 文案不含「提升音质」「动态响度归一化」「true-peak 合规」，只写 sample-peak 保护 | 逐串核对 |
| CONFIRMED | 新主规格无 delta 操作头，7 条 Requirement 全部落地 | `openspec/specs/loudness-normalization/spec.md` |
| CONFIRMED | `audio-effects` 两条 MODIFIED Requirement 已同步到主规格 | 标题逐项比对 |

## D. DSP 公式独立重算（12 项，全部 CONFIRMED）

用独立的 Python 实现（不调用 Rust 代码）重算 `compute()` 的每一种分支，与设计 D4 公式及 Rust 单测期望值比对：

| 结论 | 复核项 | 独立重算 |
| --- | --- | --- |
| CONFIRMED | RG2 track −6 dB / album −8 dB | `gain=-6.000000` / `-8.000000 dB` |
| CONFIRMED | R128 −6 dB 对齐到 −18 LUFS（+5 dB） | `gain=-1.000000 dB` |
| CONFIRMED | 用户 preamp +3 dB | `gain=-3.000000 dB` |
| CONFIRMED | peak=0.89 请求 +9 dB 的钳制 | `gain=0.012200 dB`（`-20log10(0.89)-1`） |
| CONFIRMED | 无 peak 请求 +9 dB 的上限 | `gain=+6.000000 dB` |
| CONFIRMED | EQ 保护 −3.5 dB + 归一化 +2 dB | `combined=-1.500000 dB`（相加而非覆盖） |
| CONFIRMED | 关闭时保持 P0 前置增益 | `combined=-3.500000 dB` |
| CONFIRMED | R128 Q7.8 `-1408 → -5.5 dB` | `-1408/256=-5.5` |
| CONFIRMED | limiter −1 dBFS 线性阈值 | `0.891250938` |
| CONFIRMED | +6 dB 提升不被 EQ headroom 抵消 | `headroom 6.5 dB ≥ 6 dB` |

## C. 复核结论与边界

- **CONFIRMED（合计 76 项：43 数值/哈希 + 21 规格与实现一致性 + 12 DSP 公式重算，0 REFUTED / 0 CHANGED）**：macOS 打包运行库上，归一化使用的 `volume` 节点运行时增益命令确实改变 PCM；最小链无 EQ/空间节点；末端 limiter 在 +6 dB 提升与近满幅素材下把整数 PCM 峰值保持在 `-0.999854 dBFS`。
- **REFUTED**：无。
- **CHANGED**：无（本次复核没有改动任何实现或规格数值）。
- **仍不成立**：30 ms 切歌斜坡平滑度、实时延迟、underrun、设备重建、中间浮点链非有限值、true-peak、BS.1770 响度，以及 Windows / Linux 分发运行库。PCM AO 非实时且输出整数 PCM16，不能证明这些项；报告已逐项列在 `not_measured`，且 `gate_passed=false`。
