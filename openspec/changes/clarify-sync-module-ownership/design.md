# Design

## Context

本 change 聚焦同步实现的内聚与协作边界，不重新设计 LAN 同步。当前事实来源包括 `docs/DESIGN.md`、`openspec/CODE_STANDARDS.md`、已归档的 `engineering-governance` 规格，以及活动中的 `introduce-lan-library-sync`。活动 change 已定义同步拓扑、确认与恢复语义、持久化一致性、传输生命周期及安全边界；本设计只定义实现如何承接这些合同。

本 change 不新增 governance delta：`engineering-governance` 已覆盖分层/架构守卫（§2–3）、验证门禁（§5）、模块与接口约束（§8）、公开入口/封装（§11）、文档一致性与归档完成（§12），以及当前 HEAD 复核和重构残留清理（§13）。本 change 的具体同步 owner、事务/生命周期边界和兼容约束是这些长期治理要求在 LAN sync 上的应用，不构成新的长期规则。故 `.openspec.yaml` 保持 `skip_specs: true`。

当前实现审查基线（实施前须在当前 HEAD 复验）：

- Core 的领域同步类型按概念分布在 `domain/sync.rs`、`sync_binding.rs`、`sync_resources.rs`、`sync_members.rs` 等模块；application ports 已部分按能力拆分，并保留聚合入口。
- SQLite 同步实现已从 adapter 根聚合至 `infrastructure/sqlite/sync/`，入口为 `sync/mod.rs`，内部按 enqueue、apply、configuration、ledger、queue、snapshot、projection、recovery、resource 等能力组织；`SqliteDatabase` 仍是连接、schema 和事务的稳定 owner。
- `echo-sync` 已承载 LAN protocol、client/server、pairing、discovery、replica 与 resource transfer；不应把网络协议类型上移到 Core。
- Tauri 已有 `sync_runtime/`、`sync_provisioner/` 子模块，但 runtime cycle port 继承 provisioning port，且 provisioned peer 暴露 `LanClient`；`NativeSyncProvisioner` 同时持有数据库和多类资源/周期状态。
- React 已有 `features/sync/` adapter 与 UI。由 `useAppShell` 完成的同步后 catalog refresh 属于应用壳跨功能协调，不因触发源是同步而自动迁入 sync feature。

上述事实不是冻结的文件清单；任务必须先复查实际消费者、测试边界和活动 change 的落地情况。

## Goals

- 为每项同步实现明确 owner、稳定入口及跨模块协作接口。
- 让模块聚合反映业务能力和事务/生命周期所有权，而不是仅按名称或文件规模归档。
- 让桌面运行时依赖所需的同步能力，不继承或暴露不必要的 provisioning/transport 细节。
- 保持现有同步产品行为、协议与持久化兼容性。

## Non-Goals

- 不改变同步拓扑、冲突/删除裁决、配对安全、离线行为、状态转换、重试或资源传输语义。
- 不修改 wire protocol、SQLite schema/migrations、同步记录格式、Tauri command/event 名称或参数、IPC DTO。
- 不把全部 sync 代码搬入单一 crate/目录；不为目录对称而新建无职责的 facade、manager 或 trait。
- 不以文件行数为独立拆分依据，不做无消费者的公共 API 改动。
- 不重做 React 同步 UI，也不迁移 app-shell 的跨功能 catalog 协调。

## Ownership and Collaboration

| 层/模块                    | Owner                                                                                  | 协作边界                                                                                                                                                                                                              |
| -------------------------- | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Core domain                | 同步身份、顺序、mutation、冲突和资源规则                                               | 纯规则与领域类型；不依赖 SQLite、HTTP、Tauri、UI 或具体凭据                                                                                                                                                           |
| Core application           | 同步用例及按调用方能力定义的 ports                                                     | 协调用例与事务需求；不暴露具体 transport 或 SQLite 类型                                                                                                                                                               |
| Core SQLite infrastructure | sync persistence adapters                                                              | 按 queue、ledger/publication、snapshot、projection/remote apply、conflict/recovery、resource persistence 等经复核后的能力聚合；事务仍由正确的 application/use-case adapter 协调，数据库连接与 schema ownership 不分裂 |
| `echo-sync`                | LAN wire protocol、HTTP client/server、discovery、pairing、replica transport、资源传输 | 依赖 Core ports；将协议 DTO 转为 Core 契约；不直连 UI 或 SQLite 实现                                                                                                                                                  |
| `echo-desktop`             | 桌面安全存储、平台能力与必要的 filesystem adapters                                     | 实现 platform ports；不持有同步裁决规则                                                                                                                                                                               |
| Tauri runtime              | 桌面同步会话、状态、取消、进度与生命周期策略                                           | 消费窄化后的 cycle/lifecycle collaborators；不得依赖 SQLite 内部表或具体 HTTP client 表示                                                                                                                             |
| Tauri composition/commands | 依赖装配、IPC 命令和事件转换                                                           | composition root 负责组装；commands 只做 DTO 转换、调用和错误/事件映射                                                                                                                                                |
| React sync feature         | 同步设置、状态与冲突 UI、typed bridge adapter                                          | 仅依赖稳定 IPC/生成 DTO；跨 feature 的 catalog refresh 继续由 app-shell 协调                                                                                                                                          |

## Decisions

### D1. 以 capability 与协作合同组织 SQLite sync adapter

将现有同步存储实现收纳到可识别的 sync 子模块，并由模块根提供必要的内部组合入口。候选分组为 queue、ledger/publication、snapshots、projection/remote apply、conflicts/recovery、resources；具体分组须以调用图、共享事务与测试边界复核为准。

**理由**：当前顶层文件各自可辨认，但整体入口和同层能力聚合不明显。子模块能改善 owner 的可发现性，同时保留共同数据库边界。

**约束**：移动模块不等于拆事务。若一个 use case 必须原子更新事件、对象投影、游标或恢复状态，事务仍须覆盖原有不变量；不得把事务拆成跨模块独立提交。`SqliteDatabase`、迁移定义和连接 actor 不移入 transport 或 Tauri。

### D2. Runtime execution 与 provisioning/lifecycle 通过按消费者收窄的协作者组合

实施阶段评估并优先采用两个独立协作面：一个表达 session provisioning/teardown 等生命周期能力，另一个表达 host/peer cycle 执行能力。runtime 只持有所需 collaborator，不以 trait 继承强制 cycle driver 同时实现全部 provisioning API。

**理由**：当前 `SyncCyclePort: SyncProvisioner` 将不同调用方能力合并，增加实现和测试替身耦合。按调用方能力拆分符合 `CODE_STANDARDS.md` §3.2；是否保留部分组合由实际消费者与替身验证决定，不为了抽象数量而拆接口。

具体 `LanClient` 应留在 `echo-sync` transport adapter 内部。runtime-facing 结果使用表达运行时所需能力的类型/操作；如果当前行为确实需要保留具体类型传递，design review 必须说明消费者及无法通过窄 adapter 表达的原因。

### D3. Provisioner 状态按生命周期与资源所有权拆分，组合根负责装配

先为 `NativeSyncProvisioner` 字段与方法建立 ownership map（谁创建、谁使用、何时释放、需否跨 cycle 保留）。只有当状态具有独立生命周期、窄消费者和独立测试边界时才提取 collaborator；共享同一锁/事务不因目录重组而拆开。数据库、secure store、resource store、transfer telemetry 与 scan service 的构造应在 composition root 可追踪，策略留在真正 owner。

### D4. 保持 Core 与 transport/platform 边界

Core domain/application 不接受 wire DTO、`LanClient`、Tauri state/command 或平台凭据类型。`echo-sync` 实现远端协议适配；`echo-desktop` 实现平台与安全能力；Tauri 负责桌面装配/IPC；React 通过 typed IPC 使用可观察状态。既有领域规则不得在 Tauri 或 React 重复实现。

### D5. 规格与活动 change 协调

实施前将 `introduce-lan-library-sync` 当前 proposal/design/spec/task 状态与目标文件逐项比对。不得改写其中已批准的行为要求；若结构重组需要修改任何外部行为、持久化兼容合同或 IPC/protocol 合同，暂停本 change 并先通过 OpenSpec update 明确扩围或另立 change。

## Compatibility and Migration

预期是纯内部重构：LAN wire bytes/JSON 与版本策略、SQLite schema/migrations、同步对象/事件身份、事务原子性、Tauri command/event 和参数、IPC DTO、前端用户可见行为均保持不变。保留既有稳定公共 re-export；只在全仓引用与生成产物检查证明无消费者后删除旧入口。

按层分阶段迁移：先建立可验证的责任/消费者清单；再收窄接口并更新所有实现与测试替身；随后按能力移动 SQLite 实现；再调整 Tauri composition/runtime；最后清理旧路径和无消费者导出。每阶段独立编译和测试，避免同时改变结构与行为。

## Verification Strategy

实际命令在 apply 阶段按当前 workspace scripts、CI 与 scenario manifest 复核后写入 tasks。至少覆盖：

- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- `cargo test -p echo-core`、`cargo test -p echo-sync --all-features`、`cargo test -p echo-desktop`、`cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml`；涉及的 feature/平台约束按 workspace 实际配置调整。
- SQLite sync queue/ledger/publication/snapshot/projection/remote-apply/conflict/recovery/resource 的事务、幂等与恢复回归；对照重构前后 schema/migration 清单和序列化 golden tests。
- Tauri runtime lifecycle、取消、错误、session ordering、provisioning 与 test doubles 的单元/集成回归；证明 cycle driver 不再因接口继承而承担未使用能力，或记录保留理由。
- `pnpm --dir apps/desktop typecheck`、`lint`、`test` 与相关 sync E2E；确认 IPC 生成物及 command/event 契约无变化。
- 架构守卫、`pnpm exec openspec validate clarify-sync-module-ownership --strict`、场景/traceability 对账（若实施涉及登记场景）。
- 残留清理：全仓引用检查、dead-code/clippy 检查、旧模块路径与重复实现检查；任何保留的兼容入口均记录有效消费者。

## Risks

- SQLite 多模块共享事务和 schema：只移动文件容易误以为可拆分一致性边界；须以真实事务测试和原子性不变量验收。
- `introduce-lan-library-sync` 活跃实施可能持续改变同一文件；实施前须按当前 HEAD 复核，必要时调整顺序或等待对应任务收敛。
- trait/模块移动会影响大量测试替身和内部 import；优先保留 facade/re-export 作为过渡，再在消费者归零后删除。
- `NativeSyncProvisioner` 多类状态可能共享同步周期或锁；只有能证明独立生命周期和消费者时才抽出，不按字段类型机械拆组件。
- 架构图当前遗漏 `echo-sync`；文档同步应作为本 change 实施收尾的一部分并检查其他引用，避免形成第二份架构事实来源。
