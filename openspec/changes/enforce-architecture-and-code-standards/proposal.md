## Why

Echo 的规范体系本身是完整的——`openspec/CODE_STANDARDS.md` 已经规定了分层规则（§3.1）、抽象规模（§3.2）、文件规模（§2.1）、`unsafe` 隔离（§4.1）、类型化 IPC（§6）与交付门槛（§8）。缺的不是规范，而是**把规范变成"会自动红的机制"**：违反规范不会被任何门禁拦住，只会在下一次人工审计时被发现。

因此在只读审计中同时观察到规范与实现的双向漂移：`echo-desktop` 整包丢失 workspace lint 继承（其 9,958 行生产代码从未接受 `clippy::pedantic/nursery` 检查）；应为纯领域规则的播放上下文解析、歌曲绝对路径重建与播放会话恢复裁决留在平台层；按总行数口径有 15 个 Rust 源文件超过 §2.1 的 1000 行硬限；`BridgeCommandMap` 的 39 个命令条目中有 38 个返回 `unknown`；而**仓库自己的验证门禁也存在静默通过路径**——场景执行只查退出码、零测试防护脚本依赖不存在的输入而无法运行、两个规格领域未登记因而完全脱离追溯。

规范只写在文档里等于没有规范。本变更把"代码规范 + 架构规范"提升为 **OpenSpec 中可验证的需求与场景**，并补齐使其自动失败的守卫，让偏离在提交与发布前显式暴露。

## What Changes

- 新增 `engineering-governance` 规格领域，把代码规范与架构规范写成可验证的需求与场景：分层边界、架构守卫对称性、静态检查继承、门禁有效性、规格追溯一致性、代码规模上限、跨边界契约、构建纯净、归档与文档一致性，以及代码变更的规格—实现复核与残留清理。
- 修复验证门禁的静默通过路径：场景执行必须证明至少运行了一个测试；零测试防护脚本必须能在干净检出上运行；规格领域注册必须完整，且三方对账必须真正比较数量并阻断（当前实测 `spec 209 ≠ trace 179 ≠ manifest 181`，96 处失配）。
- 让每个 crate 恢复 workspace lint 继承：`echo-desktop` 的 crate 级 `unsafe_code = "allow"` 收窄到真正需要 FFI 的模块级，其余模块重新接受 workspace 的 `clippy::all/pedantic/nursery`。
- 让架构守卫对称：新增平台层守卫，使"领域规则不得在平台层重复实现"与既有的"Core 不得依赖平台"一样可被自动拒绝；守卫必须覆盖 workspace 全部 crate 并保留"能拒绝违规"的自证测试。
- 把已漂移出 Core 的领域规则归位：播放上下文解析（含"最近"过滤、分页遍历、排序、选中项校验、歌单顺序）、歌曲绝对路径解析、播放会话恢复裁决下沉为 Core 的单一实现；桌面端只做适配与 I/O。**行为不变**。
- 收敛超出规模上限的模块与端口：按职责拆分超过 1000 行的生产文件与超过 6 个方法的 `pub trait`；白名单必须显式登记且只减不增。
- 让跨边界命令契约由生成器产出并被漂移门禁守护，消除返回值 `unknown`；调用点必须使用具体返回类型。
- 收敛前端架构：禁止静默丢弃跨边界调用的失败（生产路径实测 19 处裸丢弃）、统一为一个共享状态原语（现为 5 个自研 store、3 种写法、无共享原语）、为每个功能模块建立公开入口（现无任何 `index.ts`，跨模块直引内部文件约 18 处）、拆开文件名与职责不符的模块，并解开测试基建对功能模块内部的反向依赖。
- 拆分单一巨型错误枚举 `echo-core/src/error.rs`（823 行）为按层的子模块；跨 IPC 边界的错误映射已正确，不做改动。
- 把测试替身目录（`application/testing/`，约 3,931 行）提升为 `testkit` 并按 port 分组；以 `#[cfg(test)]`/feature 门控测试替身，避免其进入默认生产构建。
- 把核心门禁接入 CI，并对齐工具链（CI 当前用浮动 stable，而仓库钉住 1.96.0），使发布门禁不再依赖仓库外的临时手工产物。
- 启用已存在但从未被执行的归档一致性校验（`openspec validate --archived` 当前在 4 个归档变更上失败，共 12 个未完成任务）。
- 为门禁自身的有效性建立证明：本轮只审计了 79 个检查中的少数，其余为未知有效性；新增检查必须随附"注入违规即失败"的可复现证明。
- 处置从未被任何脚本解析的登记资产（`tests/` 下 136 个 yaml）与场景命令的重复执行（209 条场景命令仅对应 115 条不同命令）。
- 修正指向不存在模块的架构说明，并消除架构事实来源的重复。

## Capabilities

### New Capabilities

- `engineering-governance`: 定义 Echo 仓库的工程治理契约——分层边界与依赖方向、代码规模上限、静态检查继承、验证门禁有效性、规格追溯完整性、跨边界契约生成、构建纯净与归档一致性，全部以可被自动门禁验证的需求与场景表达。

### Modified Capabilities

- `desktop-playback`: 新增"播放上下文解析由共享 Core 单一提供"的需求，明确资料库视图与歌单到有序播放上下文的解析归属与顺序语义，使桌面端与后续移动端不得自行复制该规则。

## Impact

- **架构与分层**：`crates/echo-core/src/application/`（新增播放上下文用例；`ports.rs` 拆分）、`crates/echo-core/src/domain/`（路径解析、会话裁决规则归位）、`crates/echo-desktop/src/runtime/services.rs`（1,591 行，交出解析与裁决后收窄为命令编排）、`crates/echo-desktop/src/player/`（会话类型与 `fake.rs` 门控）。
- **静态检查与构建**：`crates/echo-desktop/Cargo.toml` 的 `[lints]` 段、`crates/echo-desktop/src/player/ffi.rs`（承接模块级 `unsafe` 放宽）。首次恢复继承会暴露一批既存告警，属被掩盖的存量债务，按模块分批收敛。
- **门禁与 CI**：`scripts/verify/run-scenario.mjs`、`scripts/verify/validate-scenario-commands.mjs`、`scripts/verify/reconcile-scenarios.mjs`、`scripts/verify/spec-scenarios.mjs`、`scripts/verify/checks/task-13.9.mjs`、`scripts/verify/gen-scenario-manifests.mjs`、新的规模与端口守卫检查、`.github/workflows/ci.yml`。
- **跨边界契约**：`crates/echo-desktop/src/ipc/generate.rs`（新增命令 → 返回 DTO 映射）、`apps/desktop/src/ipc/ipc-types.generated.ts`、`apps/desktop/src/bridge/index.ts` 与全部 `bridge.call(` 调用点。
- **前端架构**：`apps/desktop/src/bridge/index.ts`（区分需要结果与显式吞错的调用）、5 个自研 store（`player/playerStore.ts`、`app/toast.ts`、`app/coverArt.ts`、`features/library/songUpdates.ts`、`features/library/coverPalette.ts`）、新增的 `features/*/index.ts` 公开入口、`features/player/ImmersivePlayer.tsx`（671 行）、`app/App.tsx`、`test/setup.ts`。
- **Core 内聚**：`crates/echo-core/src/error.rs`（823 行，仅拆分为按层子模块，**不改动已正确的跨边界错误映射**）、`crates/echo-core/src/application/testing/`（约 3,931 行，提升为 `testkit`）。
- **构建与工具链**：`rust-toolchain.toml`（钉 1.96.0）与 `.github/workflows/ci.yml`（第 27、94 行使用浮动 stable）需对齐；`crates/echo-desktop/src/player/fake.rs` 需加条件编译门控。
- **规格与文档**：`openspec/CODE_STANDARDS.md`（唯一真相声明与门禁映射）、`docs/DESIGN.md`（补 `src-tauri` 层级）、`openspec/specs/`（新增领域注册）。
- **不受影响**：产品可见行为、Tauri command/event 名称与参数、IPC 错误的 DTO 形状与 code 稳定性、SQLite schema 与迁移、远端同步协议、`PlayerPort`（3 方法，保持不动）。本变更中的归位与拆分均为行为保持型重构。
- 受 `docs/PRODUCT.md` 的本地优先定位、`docs/DESIGN.md` 的平台播放器与 Core 边界、`openspec/CODE_STANDARDS.md` §2.1/§3.1/§3.2/§4.1/§6/§8 约束。
