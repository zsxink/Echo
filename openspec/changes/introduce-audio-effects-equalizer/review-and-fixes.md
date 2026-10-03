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
