# Echo

本地优先的跨平台音乐播放器：扫描并管理你自己的音乐资料库，展示封面与歌词，维护播放列表和歌单。不依赖账号或流媒体曲库；音乐始终属于你，远端只承担个人设备间的可替换同步。

## 产品定位

- **本地优先**：选择本地音乐资料库根目录后，Echo 扫描其中的音乐文件；播放、管理、搜索都不需要网络。
- **无账号体系**：没有账号、没有推荐流；资料库是你的数据中心，同步（二期）只在一份资料库与一个远端（S3 / WebDAV）之间进行。
- **完整日常体验**：导入新歌曲、搜索与浏览、排序、收藏、歌单、稳定的播放队列，以及沉浸式黑胶播放和跟随播放进度的歌词。
- **四平台演进**：一期面向 macOS、Windows、Linux 桌面；二期接入资料库同步，三期、四期扩展 Android 与 iOS。

## 核心特性（一期）

- **曲库管理**：选择并扫描资料库根目录，解析标签、封面与歌词；持续监听目录变化并提供手动重新扫描。导入新歌曲时复制入资料库 `media/`，按 `歌手/歌手 - 歌曲名` 组织；内容相同不重复导入，重名不同内容自动追加编号，绝不覆盖已有文件。
- **系统集成**：macOS 提供菜单栏状态行与控制中心“正在播放”（Now Playing）集成，Windows/Linux 提供系统托盘；主窗口关闭后仍可查看播放状态、控制播放。支持设为系统默认播放器，文件管理器双击音频即可临时播放。
- **播放体验**：稳定的队列、进度、音量和快捷键；沉浸式黑胶播放器；歌词支持内嵌解析/同名 `.lrc`/用户覆盖层，随进度定位，无歌词时显示空状态。
- **个性化**：三套主题（珊瑚玫红 / 深钴蓝 / 松石绿）并记住选择；“关闭主窗口时”可退出或驻留后台。
- **覆盖层优先**：封面、歌词与元数据覆盖层不改写原始文件；展示与播放时覆盖层优先于文件内嵌数据。

## 产品原则

1. 资料库优先于账号与推荐流。
2. 播放控制始终可达，沉浸模式不牺牲基本操作。
3. 同步状态必须清楚且不打断聆听。
4. 封面与歌曲信息服务于辨识，不替代高效检索。

## 现状与路线

当前处于一期（完整桌面版播放器），已以 macOS 单平台验证为基线封版，Windows/Linux 的安装包、原生 E2E 与故障恢复矩阵等正在扩展。资料库同步、批量操作与移动端在后续阶段立项。

| 阶段 | 主题 | 主要交付 | 完成标志 |
|---|---|---|---|
| 一期 | 完整桌面版播放器 | macOS、Windows、Linux 上可日常使用的本地音乐播放器 | 可稳定导入、管理、搜索和播放本地曲库 |
| 二期 | 资料库同步 | S3 兼容的多设备资料库同步能力 | 两台桌面设备可可靠同步同一资料库 |
| 三期 | Android | Android 原生体验与既有资料库同步 | Android 可完整管理、播放并同步资料库 |
| 四期 | iOS | iOS 原生体验与既有资料库同步 | iOS 可完整管理、播放并同步资料库 |

> 详细规划见 [`docs/ROADMAP.md`](docs/ROADMAP.md)，产品定位见 [`docs/PRODUCT.md`](docs/PRODUCT.md)，设计约束见 [`docs/DESIGN.md`](docs/DESIGN.md)，界面交互见 [`docs/interface-terminology.md`](docs/interface-terminology.md)。

## 界面原型

桌面原型与品牌视觉见 [`docs/prototype/echo-desktop-player.html`](docs/prototype/echo-desktop-player.html) 及同目录图标素材。

## 技术概览

Rust Core（资料库、导入、标签解析、同步与冲突裁决）保持与 UI 和播放器无关；桌面 UI 采用 Tauri 2 + React + TypeScript，播放使用 mpv/libmpv，播放能力按平台适配（移动端后续为 Flutter + media_kit）。

## 开发与验证

### 仓库结构

```
crates/
  echo-core/        领域模型、用例、SQLite 基础设施（无 UI 依赖，可独立测试）
  echo-desktop/     桌面运行时：播放 actor/coordinator、IPC DTO、平台适配（含 macOS 菜单栏/系统集成）
apps/
  desktop/
    src/            React 壳（bridge / features / player / styles）
    src-tauri/      Tauri 壳（命令层、对话框、安全 capability、macOS Now Playing/菜单栏行）
    e2e/            native E2E（真实壳 + Gate 注入）
openspec/           规格与变更管理（add-macos-now-playing-menu 为当前变更）
tests/
  scenarios/        场景级可执行 manifest（218 个）
  native/           多平台人工场景 manifest
scripts/
  verify/           统一验证执行器（manifest.json 登记全部任务/场景命令）
fixtures/           音频/封面/字幕 fixtures
```

### 快速开始

**环境要求**

| 依赖 | 版本 | 说明 |
|---|---|---|
| Node.js | ≥ 20（见 `.nvmrc`） | 前端与脚本 |
| pnpm | 11（`packageManager` 固定为 `pnpm@11.18.0`） | 可 `corepack enable` 自动启用 |
| Rust | 1.98.1（`rust-toolchain.toml` 自动安装） | 含 `rustfmt`/`clippy`，首次编译较慢 |
| libmpv | 已随仓库 vendored（`apps/desktop/src-tauri/vendor/libmpv/macos`） | macOS 开箱可用；Windows/Linux 需按平台提供 |

**启动**

```sh
git clone <repo> && cd Echo
pnpm install            # 安装前端依赖
pnpm echo dev           # 完整启动: 生成 IPC 类型 → 构建前端 → tauri dev
```

`pnpm echo dev` 依次执行 `generate:ipc` → `build` → `tauri dev`，前者必须先跑：
本项目的 `tauri dev` 直接加载 `apps/desktop/dist` 的已构建前端（`tauri.conf.json` 未配置 `beforeDevCommand`/`devUrl`），所以每次启动都要先构建前端。

其他入口：

```sh
pnpm echo release       # 构建发布包: 生成 IPC → 构建前端 → tauri build
pnpm echo build         # 只构建、不启动
pnpm echo help          # 查看全部命令
```

- 首次 `cargo` 编译会拉取并构建全部 Rust 依赖，耗时较长属正常。
- 发布：推送形如 `v0.1.0` 的 git tag 会触发 GitHub Actions（`.github/workflows/release.yml`）构建 macOS、Windows、Linux 安装包并发布为 GitHub Release，含 `SHA256SUMS` 与第三方许可证说明。产物当前**未签名/未公证**（macOS 首次启动需在「隐私与安全性」中放行；签名与 notarization 为后续项）。
- 数据目录：`$APP_DATA/com.zsxink.echo`（SQLite 单文件库，cover 缓存在 `covers/`）
- Gate 测试模式：`ECHO_GATE_DATA_DIR=<tmp>` 覆盖数据目录；`ECHO_GATE_ROOT`/`ECHO_GATE_IMPORT` 替换系统选择器（native E2E 使用）

### 验证

所有任务/场景命令统一登记在 `scripts/verify/manifest.json`：

```sh
node scripts/verify/run-task.mjs -- <task-id>…      # 按任务运行（如 8.10 13.8）
pnpm verify:scenario -- <SCENARIO-ID>               # 按场景运行
pnpm verify:scenario -- --all                       # 全量场景（218）
node scripts/verify/reconcile-scenarios.mjs         # specs↔traceability↔manifest 三方对账
```

质量门：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace --all-features`、`pnpm --dir apps/desktop typecheck && pnpm --dir apps/desktop test`。

### 规格与变更流程（OpenSpec）

- 规格：`openspec/specs/<area>/spec.md`（11 个领域）+ 变更 delta（`openspec/changes/<change>/specs/`）
- 场景 ID：由 `scripts/verify/spec-scenarios.mjs` 按标题顺序确定性派生（`<AREA>-R<NN>-S<NN>`），`docs/traceability.md` 为命名权威
- 变更验证：任务勾选必须以登记并通过的 check 脚本为证据；`openspec validate` 校验变更结构

### 一期边界（重要）

离线、无账号、无网络：UI 不渲染同步入口，bridge 无 sync/upload/download 命令，Cargo workspace 无网络客户端依赖，CSP 拒绝远程连接。同步仅以"基础数据形状"存在（`0005_sync_foundation.sql` 的 revision/outbox/tombstone），二期才接入引擎（见 `sync-foundation` 规格）。