# Echo

本地优先的桌面音乐播放器：Rust Core + Tauri + React。一期目标是 macOS、Windows、Linux 上可日常使用的本地曲库管理、搜索与播放；二期在同一 Rust Core 与同步基础数据形状之上做资料库同步。

> 详细规划见 [`docs/ROADMAP.md`](docs/ROADMAP.md)，产品定位见 [`docs/PRODUCT.md`](docs/PRODUCT.md)，设计约束见 [`docs/DESIGN.md`](docs/DESIGN.md)。

## 仓库结构

```
crates/
  echo-core/        领域模型、用例、SQLite 基础设施（无 UI 依赖，可独立测试）
  echo-desktop/     桌面运行时：播放 actor/coordinator、IPC DTO、平台适配
apps/
  desktop/
    src/            React 壳（bridge / features / player / styles）
    src-tauri/      Tauri 壳（命令层、对话框、安全 capability）
    e2e/            native E2E（真实壳 + Gate 注入）
openspec/           规格与变更管理（release-0-1-0-desktop-player 为当前变更）
tests/
  scenarios/        场景级可执行 manifest（166 个）
  native/           多平台人工场景 manifest
scripts/
  verify/           统一验证执行器（manifest.json 登记全部任务/场景命令）
fixtures/           音频/封面/字幕 fixtures
```

## 快速开始

```sh
pnpm install
./dev.sh                # tauri dev（需要 vendored libmpv，见 apps/desktop/src-tauri/vendor/）
```

- 数据目录：`$APP_DATA/echo`（SQLite 单文件库，cover 缓存在 `covers/`）
- Gate 测试模式：`ECHO_GATE_DATA_DIR=<tmp>` 覆盖数据目录；`ECHO_GATE_ROOT`/`ECHO_GATE_IMPORT` 替换系统选择器（native E2E 使用）

## 验证

所有任务/场景命令统一登记在 `scripts/verify/manifest.json`：

```sh
node scripts/verify/run-task.mjs -- <task-id>…      # 按任务运行（如 8.10 13.8）
pnpm verify:scenario -- <SCENARIO-ID>               # 按场景运行
pnpm verify:scenario -- --all                       # 全量场景（166）
node scripts/verify/reconcile-scenarios.mjs         # specs↔traceability↔manifest 三方对账
```

质量门：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace --all-features`、`pnpm --dir apps/desktop typecheck && pnpm --dir apps/desktop test`。

## 规格与变更流程（OpenSpec）

- 规格：`openspec/specs/<area>/spec.md`（7 个领域）+ 变更 delta（`openspec/changes/<change>/specs/`）
- 场景 ID：由 `scripts/verify/spec-scenarios.mjs` 按标题顺序确定性派生（`<AREA>-R<NN>-S<NN>`），`traceability.md` 为命名权威
- 变更验证：任务勾选必须以登记并通过的 check 脚本为证据；`openspec validate` 校验变更结构

## 一期边界（重要）

离线、无账号、无网络：UI 不渲染同步入口，bridge 无 sync/upload/download 命令，Cargo workspace 无网络客户端依赖，CSP 拒绝远程连接。同步仅以"基础数据形状"存在（`0005_sync_foundation.sql` 的 revision/outbox/tombstone），二期才接入引擎（见 `sync-foundation` 规格）。
