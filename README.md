# <img src="docs/prototype/echo-icon.svg" width="28" height="28" alt="" /> Echo

[English](README.en.md)

本地优先的跨平台音乐播放器：扫描并管理自己的音乐资料库，展示封面与歌词，维护播放列表和歌单。不依赖账号或流媒体曲库；目前可在桌面设备间通过局域网同步资料库。

> **最新发布**：[v0.3.0](https://github.com/zsxink/Echo/releases/tag/v0.3.0) — macOS · Windows · Linux 安装包（DMG / EXE / deb / AppImage）

## 产品定位

- **本地优先**：选择本地音乐资料库根目录后，Echo 扫描其中的音乐文件；播放、管理、搜索都不需要网络。
- **无账号体系**：没有账号或推荐流；资料库保存在本地，当前同步通过局域网主机在设备间传输变更与媒体文件。
- **完整日常体验**：导入新歌曲、搜索与浏览、排序、收藏、歌单、稳定的播放队列，以及沉浸式黑胶播放和跟随播放进度的歌词。
- **跨平台路线**：桌面版支持 macOS、Windows、Linux；局域网同步已实现。Android、MinIO / S3 远端存储和 iOS 仍属于后续规划。

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

## 相关工具

[MusicTag](https://github.com/zsxink/MusicTag) 可逐首编辑本地音乐文件的标签、封面与歌词，并将修改写回文件；Echo 用于扫描、管理和播放本地曲库。两者定位互补，可搭配整理与聆听音乐。可在 [MusicTag 仓库](https://github.com/zsxink/MusicTag) 了解项目，或前往 [发行版页面](https://github.com/zsxink/MusicTag/releases) 下载。

## 现状与路线

当前最新版本为 v0.3.0。GitHub Actions 提供 macOS、Windows、Linux 安装包与 `SHA256SUMS`（见 [Releases](https://github.com/zsxink/Echo/releases)）。macOS 单平台的运行、原生 E2E 与故障恢复矩阵已验证；Windows/Linux 的原生 E2E 与恢复矩阵仍在扩展。

局域网同步功能已实现并归档；设备矩阵、真机传播时延和并发规模验收仍未完成。0.2.0 音效与均衡器 P0 已有实现，三平台原生音频发布验收仍是门槛。详见 [`docs/ROADMAP.md`](docs/ROADMAP.md)。

| 阶段 | 主题 | 状态 |
|---|---|---|
| 一期 | 完整桌面版播放器 | v0.3.0 已发布；部分跨平台验收继续扩展 |
| 二期 | 局域网同步 | 功能已实现；真机与规模验收待完成 |
| 三期 | Android（局域网连接） | 规划中 |
| 四期 | MinIO / S3 远端存储同步 | 规划中 |
| 五期 | iOS | 规划中 |
| 0.2.0 | 音效与均衡器 P0 | 实现已交付；三平台原生发布验收待完成 |

> 详细规划见 [`docs/ROADMAP.md`](docs/ROADMAP.md)，产品定位见 [`docs/PRODUCT.md`](docs/PRODUCT.md)，设计约束见 [`docs/DESIGN.md`](docs/DESIGN.md)，界面交互见 [`docs/interface-terminology.md`](docs/interface-terminology.md)。

### 系统要求

| 平台 | 最低版本 | 实测基线 | 产物 |
|---|---|---|---|
| Windows | Windows 10 | Windows 10 22H2 / 11 x64 | x64 NSIS 安装包 |
| macOS | macOS 12 | macOS 12+（Apple Silicon） | aarch64 DMG |
| Linux | Ubuntu 22.04 | Ubuntu 22.04+ x64 | deb / AppImage |

Windows 产物仅提供 x64。WebView2 运行时缺失时，安装器会用内嵌的 bootstrapper 自动引导安装。安装包当前**未签名**：Windows SmartScreen 可能提示「已保护你的电脑」，选择「更多信息 → 仍要运行」即可；macOS 首次启动需在「隐私与安全性」中放行。

## 界面原型

桌面原型与品牌视觉见 [`docs/prototype/echo-desktop-player.html`](docs/prototype/echo-desktop-player.html) 及同目录图标素材。

## 技术概览

Rust Core（资料库、导入、标签解析与同步规则）保持与 UI 和播放器无关；桌面 UI 采用 Tauri 2 + React + TypeScript，播放使用 mpv/libmpv，播放能力按平台适配（移动端规划使用 Flutter + media_kit）。

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
openspec/           规格与变更管理（当前活动变更见 openspec/changes/）
tests/
  scenarios/        场景级可执行 manifest
  native/           多平台人工场景 manifest
scripts/
  verify/           统一验证执行器（manifest.json 登记全部任务/场景命令）
fixtures/           音频/封面/字幕 fixtures
```

### 快速开始

**环境要求**

| 依赖 | 版本 | 说明 |
|---|---|---|
| Node.js | ≥ 22（见 `.nvmrc`） | 前端与脚本 |
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
pnpm verify:scenario -- --all                       # 全量场景
node scripts/verify/reconcile-scenarios.mjs         # specs↔traceability↔manifest 三方对账
```

质量门：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace --all-features`、`pnpm --dir apps/desktop typecheck && pnpm --dir apps/desktop test`。

### 规格与变更流程（OpenSpec）

- 规格：`openspec/specs/<area>/spec.md`（11 个领域）+ 变更 delta（`openspec/changes/<change>/specs/`）
- 场景 ID：由 `scripts/verify/spec-scenarios.mjs` 按标题顺序确定性派生（`<AREA>-R<NN>-S<NN>`），`docs/traceability.md` 为命名权威
- 变更验证：任务勾选必须以登记并通过的 check 脚本为证据；`openspec validate` 校验变更结构

### 一期边界与当前同步状态

以上“不包含同步入口”是一期的历史范围，不代表当前产品限制。桌面局域网同步已在二期实现：同步主机汇聚设备变更，客户端保留自己的 SQLite 投影；数据库文件本身不会通过网络传输。同步功能与验收状态见 [`docs/ROADMAP.md`](docs/ROADMAP.md)。

## 用户媒体权利与隐私

导入与局域网同步会复制你的音频与歌词文件：你须自行确保拥有在自己设备间复制、同步这些内容的权利；Echo 不提供、不下载任何音频内容，也未取得任何音乐、歌词或封面授权。隐私方面，Echo 无账号、无遥测或崩溃上报、无云端中继、无联网歌词/封面抓取，诊断日志仅保留在本地设备。详见 [`docs/media-rights-and-privacy.md`](docs/media-rights-and-privacy.md)。

### 许可证

Echo 采用**「Echo 个人学习使用许可」（Echo Personal Study License）**——一份独立定制的源码可用许可：仅限个人学习与研究用途，禁止任何商业用途、销售/转卖与再分发；Change Date（2099-12-31）后按 GPL-2.0-or-later 提供。该许可借鉴了 Business Source License 1.1 的 Change Date 结构，但**不是** BUSL-1.1。详见根目录 [`LICENSE`](LICENSE)。

第三方依赖的许可清单见 [`LICENSES.md`](LICENSES.md)（由 `cargo deny` 与前端 prod 依赖审计维护）；各平台打包的 libmpv 及关联库的许可、来源、对应源码与随包许可全文见各平台 `vendor/libmpv/*/NOTICE.md` 与 `vendor/libmpv/THIRD-PARTY-LICENSES.txt`（随安装包分发）。
