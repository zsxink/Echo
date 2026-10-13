# 实现复核与修复（2026-10-03）

本轮复核现有实现的正确性、故障路径和快照排序。原生音频捕获、响度匹配盲听与三平台发布 Gate 继续以 `native-gate.md` 为准；软件回归不替代这些证据。

## 已确认问题

| 编号 | 问题 | 修复与回归 |
|---|---|---|
| R7 | 不支持的环境中，旁路返回 `Ok(false)` 表示过渡未结束，却被设置为 completed，后续不再推进；播放安全状态也沿用旧值 | 区分待完成、确认成功和失败；多轮旁路继续推进，成功清除旧实际增益/频段，失败持续阻止放行；覆盖 mono → stereo 恢复 |
| R8 | FileLoaded、Play、Toggle 等路径中，音效首样本屏障实际暂停后端，却发布 Playing，导致进度及统计误判 | 屏障期间发布非播放状态，确认释放后才发布 Playing；晚到重配置不得复活 Ended/Failed/Stopped，Loading 等待 FileLoaded；暂停写入失败不假报 Paused，并在后续轮询中重试 |
| R9 | 面板查询快照失败后的重试调用了提交音效及修复偏好的命令 | 区分读取与操作错误；读取重试只调用 GET；连续失败后成功的回归断言没有变更命令 |
| R10 | 保存、重命名、删除未选项不改变音频 revision，操作前生成但晚到的同 revision/epoch 快照可覆盖新列表和名称 | 服务锁内为对外快照分配独立单调序号，前端拒绝旧序号；不改变音频 revision，不增加后端处理命令 |
| R11 | 默认播放回执覆盖恢复失败理由，界面将损坏/未来数据误报为普通未保存草稿 | 独立恢复失败投影保留到显式修复成功，界面说明恢复失败及原数据保留；不直接显示可能包含路径的底层错误 |

## 验证与限制

修复后前端验证：36 个测试文件、327 项测试通过；typecheck、lint、format、build 及真实 Chromium 的 mock-bridge E2E 通过，涵盖预设、开关、键盘 EQ、窄屏和浮层关闭。lint 仍有 9 条现有 warning。没有原生 DSP 输出测量在本轮取得。

Rust 验证：`cargo test --workspace` 全部通过，其中桌面单元测试 387 项；`cargo fmt --all --check` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。旁路/actor 新增 11 项回归；服务新增 3 项、DTO 新增 1 项回归。独立复核确认快照排序、恢复保护与终态音频投影逻辑正确。IPC 已通过生成器更新，生成漂移测试随全仓测试通过。

本 change 严格校验、规模检查及 `git diff --check` 通过，未新增规模豁免。

修复前基线：`cargo test --workspace`、全功能 Clippy、Rust 格式检查、前端测试（321 项）、typecheck、lint、format、build、浏览器 E2E 和本 change 的 OpenSpec 严格校验通过。前端 lint 有 9 条现有 warning，前端全量测试有既有 React act warning。

`pnpm verify:governance` 基线失败：主规格中的 `IL-R10/11` 与 `PM-R13/14` 共 15 个场景未登记到 traceability/manifest，合计 45 项对账错误。检查在第一步停止，不能称为治理通过。本轮不将其他 capability 的登记补全计为音效修复。

修复后重跑该命令，仍为相同的 45 项登记对账错误；原任务 2.4/6.2 继续保持未完成。

任务 1.3–1.5、2.5、6.1 及 Windows/Linux 原生矩阵等仍未完成；不归档、不创建发布 PR，不改变未取得证据的任务状态。

## 2026-10-08 专业音频复核修复

本次修复 AFX-10.1–10.4，并以持续原生 PCM 实验完成 AFX-9.5。仅修复本轮五项问题，不把整个 change 或三平台 Gate 标为完成。

- **运行时调参**：根因是旧短 PCM 输入在命令前已解码到 EOF。修复探针后，主 Agent 独立测得 EQ +5.990866 dB、preamp −6.042937 dB、反相空间 m=1.25→1 变化 −1.947385 dB；同相约 0、单侧第一声道约 −1.026806 dB，与理论均差≤0.25 dB。静态 +6 dB 对照 +5.995869 dB。证据及样本位置见 `evidence/macos-runtime-parameter-recheck-20261008.json`。
- **EQ/preamp 过渡**：实际中间曲线和 headroom 共同插值，采用有效滤镜合成峰值预算，移除全部正增益之和造成的过度衰减。先降 gain、再收紧 preamp、再升 gain、最后放宽 preamp；manual 不引入 Auto 补偿，连续编辑从当前值继续。
- **低采样率**：停用 16 kHz 等段在中间系数中为 identity，不影响保护预算，保存参数继续保留。
- **空间旁路**：初装 m=1/preamp=0，实际发送内部 width 过渡到用户固定目标 1.25；关闭达到 m=1/preamp=0 后移除链。用户没有 width 编辑入口。
- **重配置与失败**：启用和关闭均按负 gain→preamp→非负 gain 重断言，防止原生节点恢复安装值而 Rust 缓存造成保护漏发；关闭过程中命令失败也清理全部拥有滤镜。
- **说明更正**：最大净增益 +0.5 dBFS 相对 limiter −1 dBFS 阈值，需要约 1.5 dB 稳态衰减；增加前置增益不意味着减少限幅或固定提高节目响度。

回归：21 项 native-effects 命令/数学回归；`cargo test --workspace` 共 933 项通过、1 项既有忽略；workspace 全目标/全功能严格 Clippy、Rust 格式、14 项 Python 工具测试、OpenSpec strict、`pnpm verify:task -- AFX-1` 与完整 `pnpm verify:governance` 通过（场景对账 423/423/423、覆盖率门禁和 28/28 注入证明均通过）。证据门禁增加完成 AFX-9.5 必须保留三个参数的 pre-EOF PCM 对照；将 EQ 证据改为 EOF 后命令时，门禁正确失败，恢复有效证据后通过。

计算成本的辅助检查：优化构建独立计算 100 次交错 ±12 dB 中间曲线，48 kHz 平均约 1.255 ms/p95 1.301 ms，96 kHz 平均约 1.258 ms/p95 1.310 ms；每频点共享 sin/cos，同增益值复用峰值、中性直接返回 0。这是算法微基准，不是 5 分钟真实播放 CPU Gate。

限制：30 ms 为 actor 分轮发命令的目标轨迹，本次没有逐样本连续性、无点击或无新增 underrun 的设备捕获证据；类型切换/链移除、完整扫频、CoreAudio 时延、CPU、匹配响度试听及 Windows/Linux 原生 Gate 仍按既有未完成任务验收。

## 2026-10-09 DSP/首样本/UI 复核修订与验证状态

按只读复核记录修正了 OpenSpec 文档与实验说明中的 DSP 计算、limiter 语义、采样率判据和历史结论标注。更新包括级联峰值、真实密栅格采样范围、Auto 数值舍入、脉冲线性模型、trim/pad 的输入零样本说明、dBTP 诊断边界，以及 EQ 环境有效段和请求/实际 preamp 展示契约。

AFX-12.1 与 AFX-4.6 的 actor/configuration receipt 修复已由 native worker 和独立 reviewer 审查；workspace 测试通过（1 项既有 ignored），严格全目标/全功能 Clippy 通过。覆盖 GET_META 成功/缺失/坏 graph、暂停失败后的重试与屏障放行条件。该结果不替代设备输出 PCM Gate。AFX-12.3 数学门禁与 mutation/extrema 自检覆盖 48 组、每组 1,048,577 个频点，最大观测高估 0.00000547 dB；Rust `effects::math` 10 项数值测试通过。

AFX-11.1/AFX-9.6 的 macOS 静态低电平响应复核已完成：`sine_sweep.py` 生成 52 份报告、7,774 个频点比较，最大原生与 RBJ 差 0.000006809483 dB、保守舍入界 0.000134970 dB；主 Agent 独立复核报告与哈希，23 项 Python 工具测试通过。AFX-12.4 的逐频捕获部分因此完成，但该证据不含 limiter 高压力/浮点链诊断。AFX-12.2 的 retry/edit 修复已完成：重新提交保留的本地曲线，恢复保护先修复再重提；最终 37 个前端测试文件、338 项测试及 typecheck/build/lint（0 errors）/format 通过，独立审查未发现剩余阻断项。AFX-12.4 limiter 高压力诊断及设备/跨平台 Gate 仍未完成，原生 Gate 仍未通过。

上述验证由主 Agent/工作者执行；本次文档修订没有运行测试或 OpenSpec 校验。AFX-12.1–12.3 已完成，AFX-12.4 和三平台发布 Gate 仍保持未完成。

## 2026-10-10 最终回归

主 Agent 最终验证通过：workspace Rust 测试 947 项通过、1 项既有 ignored；全目标/全功能严格 Clippy、Rust format 通过。前端 37 个测试文件、338 项测试通过，typecheck、build、format 通过，lint 无错误（9 条既有 warnings）。AFX-12.1/12.2 任务门禁通过；AFX-1/2/12.3 数学及证据门禁、Python 23 项测试通过。

完整治理检查通过：423/423/423 场景对账、Core 覆盖率 ≥90%、测试替身构建隔离、嵌入前端匹配和 31/31 故障注入。新增注入验证 normalization 错误启用、无定义带宽误返回 0、失败命令被标记为确认，均正确失败且恢复文件。

独立审查追加修复暂停失败标记阻塞关闭音效后的新曲加载，以及 EQ 编辑失败重试旧请求。恢复保护先修复数据再重提本地曲线，读取失败仍只重读；相关最终回归通过。AFX-12.1–12.3 完成，AFX-12.4 的高压力 limiter/浮点 PCM/dBTP、实际图率、设备连续性/时延/CPU、Windows/Linux 与响度匹配试听仍待验收。
