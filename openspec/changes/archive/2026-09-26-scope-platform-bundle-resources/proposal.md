## Why

0.1.4 的 macOS 安装包把整套 Windows 播放后端打进了 dmg：`Echo.app/Contents/Resources/` 下实测存在 `libmpv-2.dll`（114.2 MB）、`vulkan-1.dll`（1.5 MB）、`NOTICE-libmpv-win.md` 与 `VulkanRT-LICENSE.txt`，而 `Echo.app` 总体积 146 MB 中属于 macOS 的仅 `Contents/Frameworks` 的 16 MB。根因是 `tauri.conf.json` 把 Windows 的 DLL 映射写进了平台无关的顶层 `bundle.resources` —— Tauri 在 macOS 打包时同样照抄该字段，而 macOS 走的是 `bundle.macOS.files`、Linux 走 `bundle.linux.*.files`，只有 Windows 依赖 `resources`，于是只有 macOS 与 Linux 被误伤。用户为用不上的 Windows 播放后端付出约 115 MB 的下载与磁盘代价，也使 macOS 产物的实际内容与「只携带本平台库」的供应链契约不符。

## What Changes

- **BREAKING**（仅限构建产物内容，不涉及运行时行为与公共接口）：把 Windows 的 `bundle.resources` 映射从 `tauri.conf.json` 顶层移入新增的平台专属配置 `tauri.windows.conf.json`。Tauri 按 RFC 7396 JSON Merge Patch 只把平台文件合并进对应目标的构建，macOS/Linux 打包时不再看到这些条目。
- 同步 `scripts/verify/checks/task-9.7.mjs` 的 Windows 分支：其第 265 行读取的是主 `tauri.conf.json` 的 `bundle.resources`，配置迁移后该断言必须改为读 `tauri.windows.conf.json`，否则 Windows 平台 Gate 会失败。
- 在 Gate 中新增一条 macOS 侧断言：正式构建产物的 `Contents/Resources/` 不得包含任何非本平台的播放后端二进制（跨平台 `.dll` 与 Linux `.so`），使这类误打包在 CI 内可复现地失败，而不是只靠人工 review 配置发现。
- 范围外（非目标）：不改动 macOS 现有 `bundle.macOS.files` 的 `Frameworks/` 布局，不改动 Linux `bundle.linux.*.files`，不改动 `bundled_libmpv` 的运行时解析逻辑，不重新构建或重新 vendor 任何 libmpv 二进制，也不调整版本号。

## Capabilities

### New Capabilities

（无 —— 本次不引入新的行为能力。）

### Modified Capabilities

（无 —— 本 change 在 `.openspec.yaml` 中声明 `skip_specs: true`，不产出 spec delta。）

行为契约层面的修正落在**未归档**的 `introduce-windows-linux-libmpv` change 之内：其「三平台打包与加载契约」requirement 由本次直接补入「每个平台的安装包 MUST NOT 携带其它平台的播放后端二进制」及对应场景（见 tasks 3.1），使该能力**在首次进入 `openspec/specs/` 时即带有该约束**，而不是先落库再打补丁。这样也回避了一个归档冲突：对该能力写 `MODIFIED` delta 会因目标 spec 尚不存在而被 `openspec validate` 判为 archive 时拒绝（已实测该 INFO）。

本 change 因此只修正实现与校验，不改变任何已成立的 requirement。

## Impact

- `apps/desktop/src-tauri/tauri.conf.json`（移除顶层 `bundle.resources`）与新增 `apps/desktop/src-tauri/tauri.windows.conf.json`。
- `scripts/verify/checks/task-9.7.mjs`（Windows 分支的资源映射读取源；新增 macOS 产物纯净性断言）。该检查在 `scripts/verify/manifest.json` 登记并由 `.github/workflows/ci.yml` 的 macOS 与 Windows 平台 Gate 触发。
- macOS 正式构建产物体积（`Contents/Resources/` 减少约 115 MB，dmg 相应缩小）。
- 未归档 change `introduce-windows-linux-libmpv`：其 task 3.4 描述了本次要修正的实现（顶层 `bundle.resources`），其 `platform-libmpv-supply-chain` capability 的「三平台打包与加载契约」requirement 需由本次补入跨平台产物纯净性约束。两者的归档顺序约束见 design.md 决策 3。
