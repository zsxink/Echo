# 2026-10-23 收尾检查点

用户要求串行 sub-agent，随后因额度不足要求收尾。目录协议 agent 已完成交接；投影 agent 触及额度上限后由主 Agent 接手，修复未完成的 trait 实现、迭代器可变性与界面回归。此检查点未完成整个 change，不归档、不创建 PR，也不宣称 issue #72 已全面修复。

## 本次落盘

- 目录协议：权威 commit、独立 seq/前驱、无锁读取、跨进程写锁、覆盖快照、按 ID 原正文收据和活跃链压缩，详见 `directory-protocol.md`。
- issue #72 的可证明/孤立两类合成 fixture、完整 typed 状态比较器，详见 `fixture-and-ipc.md`。
- SQLite 先验证完整 replay，再在同一事务投影；悬空歌曲/歌单关系保留原 envelope 与 typed reason，补齐父对象后重试关联；投影不会制造上传事件。
- 新增 projection status / retained evidence 读取端口；专用同步私有状态只保留为 `RequiresStateRecovery`，不会自动恢复联网信任。
- Core/Desktop/IPC 提供已保存待恢复、待投影和原操作 ID；批量导入可恢复原操作，不重复导入。设置页显示待更新、未关联资料和历史完整性尚未验证，可刷新既有保存前缀。
- 修复既有导入 crash fake 的新能力转发、零版本记录、同步投影回滚反馈，以及未知未来格式的只读打开回归。

## 实际验证

| 检查 | 结果 |
| --- | --- |
| `cargo test -p echo-core --lib` | 703 通过、0 失败、1 忽略（32.26 秒；随后新增投影测试单独运行） |
| `cargo test -p echo-core library_durability_projection --lib` | 新增 2 条通过：完整 retained after-images 对比、缺父补齐无回声；坏后缀在任何 DB 写之前拒绝 |
| `cargo test -p echo-desktop --lib` | 422 通过、0 失败 |
| `pnpm --filter @echo/desktop test` | 42 文件、385 条通过；现有 React act 警告仍有输出 |
| `pnpm --filter @echo/desktop typecheck` | 通过 |
| `pnpm --filter @echo/desktop format:check` | 通过 |
| `pnpm --filter @echo/desktop lint` | 全量 0 error；新组件出现的 1 条 effect-ref warning 已修复并单文件复查 |
| `pnpm --filter @echo/desktop build` | 通过 |
| IPC generator | 已实际重新生成，Desktop 全量测试包含生成物漂移检查 |
| scale / whitespace / OpenSpec strict | 通过，未增加规模白名单 |

`cargo check -p echo-core`、`cargo check -p echo-desktop`、前端构建后串行执行的 `cargo check -p echo-app`、`cargo fmt --all --check` 均通过。新增 change 场景尚未同步到主规格，所以现有 440 场景的一致性不代表本 change 的新场景登记已完成。

**`pnpm verify:governance` 未通过（退出 101）。** 场景对齐（主规格/trace/manifest 均 440）、命令校验及 build-purity 已通过；其 workspace 测试阶段在 `echo-app` 得到 86 通过、20 失败、1 忽略。多数失败发生于原生 LAN fixture 的裸数据库没有配置新的 directory adapter（`library durability: directory commit adapter is not configured`）；另有 host-publication fixture 的 manifest `{}` 无 `format_version`，以及 host-runtime `LibraryUnavailable`。不能将这 20 个失败当成已修复，也不能放宽生产持久化要求让测试假通过。复现：`cargo test -p echo-app --bin Echo` 或重跑 governance。此次未运行完整 workspace Clippy/all-features/e2e 与三平台验收。

## 后续必须继续，未勾完整任务

1. Application 统一顺序提交协调器：目前部分 per-operation 闭包仍直接写业务投影；真实冻结 pending/前驱捕获仍不完整，不能保证所有本地和 LAN 路径已消除 DB-first 窗口。
2. 私有历史状态的有界归档和完整引用图 GC：活跃链有界，但覆盖快照仍累积历史 playback/pending/receipt/ledger；资产保守保留，3.5 不完成。
3. 完整 v1/v2→v3 升级围栏、SQLite 一致救援、typed 逻辑导出、身份映射和独立临时 SQLite 完整性比对/替换。当前原件备份与基本导出不是完整迁移验收。
4. 生产播放 recorder、删除/撤销、写回/移动、封面原件，以及 LAN 快照/ACK/主确认/新 binding 意图接纳的全部入口收口。
5. 喜欢/歌单全部乐观状态和播放统计可见反馈，真实完整回填进度；新设置报告不等于迁移已经完成。
6. 新场景 registry/trace/manifest、主加两从断网重建、10k IO/收敛测量、全 workspace Clippy/all-features/e2e 与 macOS/Windows/Linux 原生证据；随后规格同步/归档，再创建关联 #72 PR。

当前继续工作的第一项应是修复上述 20 项原生 LAN 集成失败并做独立审查，再接统一提交协调器和真实候选捕获。遵守用户要求继续逐个调度，避免同时启动多个写入 agent。
