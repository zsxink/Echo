## Why

0.1.3 的 Windows 安装包在 Windows 上启动即闪退：`wire_composition` 在非 macOS 平台要求一个 `bundled_libmpv`，但该函数对 Windows/Linux 恒返回 `None`，启动进入 `Builder::build()` 失败分支后 `exit(1)`。macOS 有完整的 pinned libmpv 供应链（`vendor/libmpv/macos/` + manifest + checksum + Gate），Windows/Linux 从未实现，却被 `release.yml` 打包、发布为正式安装包。`docs/PRODUCT.md` 与 `desktop-playback` spec 都声明桌面需三平台支持，这次 change 补齐 Windows/Linux 的播放后端，让两个平台真正可启动、可播放。

## What Changes

- 建立 **Windows libmpv 供应链**：`vendor/libmpv/windows/` 放置从 shinchiro 的 `mpv-dev-*.7z` 中选定的固定版本（非追最新）的 `mpv-1.dll` 与配套依赖 DLL，配 `manifest.json`（SHA-256 / 源 / ABI / 许可）与 `NOTICE.md`，并用 NSIS `extraResources`（或同等机制）打进安装包。
- 建立 **Linux libmpv 供应链**：`vendor/libmpv/linux/` 放置首个发行（由 CI 用官方 mpv 源码按 `--minimal` 构建、glibc ≥ 2.35）产出的 `libmpv.so` 与配套 `libav*.so`，配同一 `manifest.json` 结构；`tauri.conf.json` `bundle.linux` 将这些文件打进 AppImage/deb。
- **扩展 `bundled_libmpv` 加载**：`apps/desktop/src-tauri/src/main.rs` 的 `bundled_libmpv` 从"仅 macOS"改为按 `target_os` 解析对应平台目录中打包的库（macOS `Frameworks/`、Windows 安装目录、Linux 安装目录），并用 `libloading` 加载时保持既有 ABI 校验。
- 消除 Windows 启动黑框：`main.rs` 顶部加 `#![windows_subsystem = "windows"]`，让 exe 不再挂在 console 子系统。
- **扩展 Gate 校验**：`task-9.7` 的 Windows/Linux 延迟分支落地为正式校验，新增 9.8 platform Gate 对 Windows DLL 目录搜索与 Linux glibc 运行依赖做含真机启动的冒烟；CI 三平台均跑 Gate。
- **范围外（非目标）**：macOS arm64→universal 打包、Windows/macOS 代码签名与 notarization、Linux 发行版打包的签名。

## Capabilities

### New Capabilities

- `platform-libmpv-supply-chain`: 定义 Windows/Linux 的 libmpv 与 FFmpeg 依赖的 vendor 结构、固定来源、checksum 校验、许可证说明，以及三平台加载/打包契约。

### Modified Capabilities

- `ci-release-pipeline`: 许可证要求从"至少 macOS"扩展为三平台许可证清单均随安装包上传（requirement #4 措辞调整）。
- `desktop-playback`: 明确 mpv 播放适配器在三平台均须能解析打包的 libmpv；当前 requirement 已隐含三平台，若现有措辞不足以表达 Windows/Linux 加载契约则补充。

## Impact

- `apps/desktop/src-tauri/` 的 `main.rs`（`bundled_libmpv`、`#![windows_subsystem]`）、`build.rs`（平台化 rpath/链接）、`tauri.conf.json`（bundle 配置）、`Cargo.toml`。
- `scripts/verify/checks/task-9.7.mjs`（Win/Linux 分支落地）与新增 9.8 校验脚本；`.github/workflows/ci.yml` 与 `release.yml` 增加三平台 Gate / 冒烟步骤。
- `vendor/libmpv/` 下新增 `windows/`、`linux/` 目录（二进制 + manifest + NOTICE）。
- Windows 安装器（NSIS `.exe`）与 Linux（`.deb`/`.AppImage`）的 bundle 布局。
- 同步 `docs/DESIGN.md`、`docs/ROADMAP.md`、`AGENT.md` 中"Windows/Linux 尚未接入 libmpv"的表述。