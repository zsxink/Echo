# Proposal

## Why

同步实现横跨 `echo-core`、`echo-sync`、`echo-desktop`、Tauri shell 与 React。跨层分布符合项目分层，但同一层内的模块归属和协作边界不够清晰：SQLite 同步实现散布在多个顶层文件中；桌面 runtime 的周期执行与 provisioning/lifecycle 契约相互耦合；组合装配对象同时持有多类持久状态与适配器。近期已有按文件拆分的工作，单纯继续拆文件不能解决这些职责与依赖问题。

本 change 将这些发现整理为后续可执行的、行为保持型架构重组提案。目标是让每项同步能力有明确 owner 和窄协作契约，同时保留现有 crate 分层及对外行为。

## What Changes

- 在 `engineering-governance` 既有约束下，明确同步实现的模块所有权、协作契约、依赖方向和事务/生命周期责任。
- 规划 SQLite sync adapter 按能力聚合的边界，同时保留 `SqliteDatabase` 对连接、schema 和事务协调的所有权；目录整理不得隐式拆分或改变事务边界。
- 规划桌面同步 runtime、provisioning/lifecycle 与 composition root 的职责边界，重点复核 cycle execution 和 provisioning 是否应由独立窄接口表达，以及通用 runtime 是否需要接触具体 `LanClient` 类型。
- 明确 `echo-sync` transport、desktop platform/security adapter、Tauri IPC 和 React sync feature 之间的依赖及 DTO 所有权。
- 按分层和可独立验证的协作契约规划实施任务；模块移动以职责、消费者和边界为依据，不以文件行数或目录对称为目标。
- 不修改同步行为、协议、数据库 schema/migrations、Tauri command/event 或 IPC DTO。若当前实现复核表明这些兼容性约束无法满足，须暂停并调整提案范围，不得在实现中顺带改变。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- 本 change 不新增或修改 capability spec。现有 `engineering-governance` 已规定架构边界、依赖方向、抽象边界、实现前现状复核、兼容性、验证及重构残留清理；活动 `introduce-lan-library-sync` 已覆盖同步行为与持久化合同。本提案将既有要求应用到同步模块组织，不另造重复 requirement。Change 元数据声明 `skip_specs: true`。

## Impact

- 规划涉及 `crates/echo-core/src/infrastructure/sqlite/` 的同步持久化实现、Core application ports、`crates/echo-sync/src/`、`crates/echo-desktop/` 平台适配、`apps/desktop/src-tauri/src/` 的同步 runtime/provisioning/commands/composition，以及 `apps/desktop/src/features/sync/`。
- 需与活动中的 `introduce-lan-library-sync` 协调：该 change 已规定同步用户行为、持久化行为和桌面生命周期合同。本 change 只澄清实现组织，不复制或重新定义这些行为；实施前需基于当时 HEAD 复核活动 change 的状态与重叠文件。
- 依据 `docs/DESIGN.md`、`openspec/CODE_STANDARDS.md` §2.1、§3、§5、§8、§9 及 `openspec/specs/engineering-governance/spec.md`。`docs/DESIGN.md` 当前结构示意遗漏 workspace 中的 `echo-sync`，可在后续实施阶段随架构文档复核一并修正；本提案阶段不改项目级文档。
- 本阶段仅新增本 change 的 OpenSpec 提案文件；不改生产代码、主规格或活动 change。产品行为和跨设备数据兼容性保持不变。
