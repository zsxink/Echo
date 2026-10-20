## 1. 现状复核与提案基线

- [x] 1.1 基于实施时当前 HEAD 复核 Core、SQLite、`echo-sync`、desktop adapter、Tauri runtime/provisioning/composition 与 React feature 的真实模块路径、消费者、公共 re-export、事务边界及测试 owner；更新 design 中已失效的路径/结论。验证：每项职责与调用者可由代码引用检查、模块测试或可重复命令证明。证据：`evidence/current-head-baseline.md`；当前 HEAD `db9ff141da4ce153ebd0acffdf06ca5324ec734b`，strict OpenSpec validation 通过。
- [x] 1.2 对照活动 `introduce-lan-library-sync` 的 proposal/design/spec/tasks，标注共享文件、未完成行为合同和可并行边界；确认本 change 不复制/改变同步行为。验证：形成一份无冲突的目标文件/任务映射并通过 change review。证据：`evidence/active-change-overlap.md`；已逐项映射当前 Tauri、SQLite 与架构文档改动路径到活动 change 的行为 owner，列出 SQLite 旧路径到 `sync/` 新路径的迁移关系，标注未修改的 `echo-sync`/React 路径，并修正 task 7.6 的剩余验收描述。

- [x] 1.3 复核是否需要 `engineering-governance` delta spec；只有发现现有规格缺少可长期验证的治理要求时才新增最小 delta。验证：给出“无 delta”的依据，或在 proposal/design 中说明新 requirement、scenario 及其与现有治理条款的区别。无 delta：design 明确对照 `engineering-governance` §2–3、§5、§8、§11–13，change 无新的长期治理要求；`.openspec.yaml` 保持 `skip_specs: true`。

## 2. 协作契约收窄

- [x] 2.1 以真实消费者和测试替身为依据，为同步 session lifecycle/provisioning 与 host/peer cycle execution 定义明确 owner 和调用方向；决定是否拆分 `SyncCyclePort` / `SyncProvisioner` 能力，并处理 runtime 对 `LanClient` 的具体类型依赖。验证：每个 trait 方法至少有明确消费者；接口实现、mock/fake 与 runtime 测试覆盖启动、停止、取消、失败和恢复路径；没有新增 transport 反向依赖。已拆分为两类能力，composition 共享同一 provisioner 状态；特定客户端在 adapter 从 secure store 装配，runtime 不持有 transport 类型。
- [x] 2.2 为 `NativeSyncProvisioner` 建立状态生命周期与责任映射，仅提取具有独立生命周期、消费者和测试边界的 collaborator；composition root 显式装配依赖。验证：状态创建/释放路径可追踪，原有同步周期行为不变，避免无消费者抽象。字段审计见 `evidence/provisioner-state-ownership.md`。
- [x] 2.3 保持 Core ports、`echo-sync` protocol/client/server、desktop security/platform adapter、Tauri command/event 与 React typed adapter 的依赖方向和 DTO 所有权。验证：各 crate/app 架构守卫通过，wire/IPC 契约快照或生成物无非预期变化。Core/echo-sync/Tauri architecture integration tests 与已提交 IPC 生成物一致性测试通过；`echo-sync` 协议 fixtures 通过。

## 3. SQLite sync adapter 聚合

- [x] 3.1 在 `SqliteDatabase` 连接、schema 和事务 owner 不变的前提下，将同步持久化实现按复核后的能力收纳到清晰的 SQLite sync 模块入口；明确 queue、ledger/publication、snapshot、projection/remote apply、conflict/recovery、resource persistence 的实际归属。验证：Core 编译与 SQLite sync 测试通过；无重复 SQL/实现或无消费者导出。`sync/mod.rs` 是唯一聚合入口，旧根目录路径已删除；`cargo check -p echo-core` 与 79 项 sync 测试通过。
- [x] 3.2 保持跨表原子性、幂等与恢复不变量；不改 schema、migration、记录格式或事务语义。验证：同步持久化相关测试通过，迁移清单与序列化/golden fixtures 无非预期差异，并执行适用的故障/恢复用例。迁移仅移动模块归属；sync 专项测试含事务回滚、恢复与重试场景均通过，wire/golden fixtures 在 `echo-sync` 全量测试中通过。
- [x] 3.3 如模块移动影响测试路径或架构门禁，更新测试组织和声明，删除旧路径残留但保留有实际消费者的稳定 re-export。验证：对应 `arch.rs` 与 repository/module path 检查通过；测试数和关键场景覆盖不减少。根模块 facade 仅保留确有消费者的 context/manifests/projection/song-alias exports；内部 helper 的 crate 可见性 lint 由 `sync` 边界局部 allow 处理，避免错误收窄权限。

## 4. Tauri composition 与端到端边界

- [x] 4.1 按已批准的责任矩阵整理 `sync_runtime`、`sync_provisioner`、`sync_commands` 和 composition root；保持运行状态、生命周期、取消、进度、资源发布与错误映射的外部行为。验证：src-tauri 同步 runtime/provisioning 单测与集成测试通过。19 runtime tests、27 provisioner tests 通过（各有 0 failed；另 1 ignored）。

- [x] 4.2 验证 React sync feature 仍只使用类型化 IPC；只在跨功能 app-shell 协调确有职责时保留其现有位置，不将目录搬动当作边界改善。验证：桌面 typecheck/lint/test 与相关 E2E 通过；IPC DTO、command/event 名称及参数无变化。desktop format/typecheck/lint/test/build 与 IPC generator drift test 通过；React app-shell 无移动，native device E2E 未运行，见 `evidence/implementation-verification.md` 的限制说明。

- [x] 4.3 清理被替代模块、旧入口、失效测试替身及重复逻辑；如需保留兼容 facade/re-export，记录其消费者和移除条件。验证：全仓引用检查、clippy/dead-code 检查、架构守卫和残留清单复核通过。全 workspace clippy、crate architecture tests 通过；SQLite sync source 只由聚合模块编译一次，保留的根 facade 由当前 sibling/tests 消费。

## 5. 文档与集成验收

- [x] 5.1 更新 `docs/DESIGN.md` 的 workspace 结构图，将当前 `echo-sync` 边界与依赖方向纳入唯一架构事实来源；检查其他架构说明仅引用存在的模块/入口。验证：文档引用检查与人工一致性复核通过。workspace tree 和边界描述已更新，并通过 Prettier 检查。

- [x] 5.2 运行 Rust format/lint/test、前端 typecheck/lint/test、src-tauri 测试、可执行的 sync 回归测试、架构守卫、场景/traceability 对账及 `pnpm exec openspec validate clarify-sync-module-ownership --strict`；将基于当前 HEAD 确认后的实际命令与结果记录在 change。验证：本 change 范围内规定检查退出码为 0；未运行项及外部验收阻塞有明确处置，不以未运行代替通过。Rust、桌面、Tauri、架构、IPC、场景/traceability 和 strict OpenSpec 检查通过，命令与数量见 `evidence/implementation-verification.md`；LAN 硬件 E2E 与活动 change 的设备/安装包验收由其 owner 完成，不在本 change 内伪报通过。

- [ ] 5.3 归档前确认行为兼容性、delta spec 同步（如有）、任务状态和全部残留处置完成；运行 `pnpm exec openspec validate --archived`。验证：change strict validation 与归档校验通过；未归档前不创建 PR。当前 change 的 strict validation 通过且行为兼容性复核完成；归档校验因既有独立 change `2026-10-13-introduce-audio-effects-equalizer` 有 20 个未完成任务而失败。按范围约束不修改该 change，因此 5.3 保持未完成，当前 change 不归档、不创建 PR。
