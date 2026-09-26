## Context

当前 `apps/desktop/src-tauri/src/main.rs` 的 `bundled_libmpv`（main.rs:133）仅在 `target_os = "macos"` 分支解析 `Frameworks/libmpv.dylib`，非 macOS 恒返回 `None`；`wire_composition`同步要求 libmpv 存在（main.rs:343），一旦缺失 `Builder::build()` 走 `exit(1)` 失败分支 —— 这就是 0.1.3 Windows 安装包「黑框 → 白框 → 闪退」的根因，也是 Linux 部署后会重现的同类故障。当前供应链只有 macOS（`vendor/libmpv/macos/` + `manifest.json` + `NOTICE.md`，publisher media-kit/libmpv-darwin-build v0.7.2，ABI 2.0.0），`scripts/verify/checks/task-9.7.mjs` 对 Windows/Linux 只输出 deferred note（第 218-257 行），`release.yml` 却在三平台打包并发布正式安装包。动机见 proposal.md Why，行为契约见三个 spec delta（`platform-libmpv-supply-chain`、`desktop-playback`、`ci-release-pipeline`）。

既有可复用资产：`MpvSys::load`（`crates/echo-desktop/src/player/ffi.rs:166`）已用 `libloading` 按符号集加载并暴露 `api_version()`（ffi.rs:371），ABI 校验只做符号存在性 + 客户端 API 版本；`task-8.12.mjs` 是「真 libmpv 冒烟 + 禁静默跳过」的既有 Gate 形态，可作为三平台冒烟模板。设计只补齐路径解析、打包布局、供应链管理、Gate/CI 四个维度，不动 `echo-desktop` 播放器核心。

## Goals / Non-Goals

**Goals:**
- Windows/Linux 的 libmpv 与关联 FFmpeg 库拥有可审计供应链：固定来源与版本、统一 manifest（SHA-256 / ABI / 许可）、统一 NOTICE。
- `bundled_libmpv` 按平台解析打包库，各平台启动不再闪退；Windows 同时消除控制台黑框。
- 打包布局把三平台库随安装包分发；Gate 与 CI 三平台实机冒烟，把「能不能启动播放」放进发布门槛。
- 替换 task-9.7 中 Windows/Linux 的 deferred note 为正式校验。

**Non-Goals:**
- 不做 macOS universal 架构（维持 arm64 基线）、不做任何代码签名 / notarization / 公证、不做 Linux 发行版签名（沿用 proposal 范围外项）。
- 不把 FFmpeg 编译、mpv 源码全量编译放进本 change（Linux 首个发行由 CI 一次性构建落仓 vendor，见 Decisions D1）。
- 不改动 `crates/echo-desktop` 的播放器语义；`MpvSys` 符号集与 ABI 版本契约维持现状。
- 不解决 WebView2 运行时缺失等 Windows 环境自身问题（仅在冒烟里探测并报告）。

## Decisions

### D1. Windows 用 shinchiro 固定发行，Linux 首个发行由 CI `--minimal` 自建，二者都落仓 vendor

- **Windows**：取 shinchiro `mpv-dev-{arch}-*.7z` 的一个**固定**发行（GitHub Actions 用的 `*.7z` 镜像，解出 `mpv-1.dll` + `libav*.dll` + `libmbed*.dll` + `build.tag`）。固定版本号与 SourceForge URL 写进 vendor `manifest.json` `sourceUrl`/`version`，不追最新；选版时给出具体 tag/URL 便于审计。
- **Linux**：首个发行由 CI 用官方 mpv 源码按 `--minimal` 构建（避免系统包版本漂移、保证 glibc ≥ 2.35 可控），产出 `libmpv.so` + `libav*.so` 等，落仓 `vendor/libmpv/linux/`。构建用官方脚本依赖能力匹配的容器（host Ubuntu 24.04 即可满足 ≥ 2.35）或用 `mpv-build` 平替；具体构建脚本与版本选点作为 task 落点，但产物必须带 `build.tag` + glibc 探测记录。

**Why**：macOS 已有 manifest+checksum+ABI 的完整可信供应链（media-kit 固定发行）；Windows/Linux 沿用同一「vendor 目录 + manifest + NOTICE」结构，使三平台契约一致（spec `平台/libmpv`）。用户已选定 Linux 自建、Windows 固定发行。
**备选**：Windows 也用自建跨编译器产物 —— 不可行（音频后端需要完整 FFmpeg 工具链，shinchiro 已是事实主流且带 `.dll` 依赖集）；Linux 直用发行版 libmpv5 —— 版本漂移、多发行 glibc 差异会破坏「打包内即可加载」契约。

### D2. `bundled_libmpv` 按目标平台解析，统一经 `libloading` 加载，Windows 补 `windows_subsystem`

`bundled_libmpv`（main.rs:133）改为按 `target_os` 三分支，各自基于**运行中可执行文件自身目录**定位（沿用 macOS 的 `std::env::current_exe()` 思路，不用 `dirs::executable_dir`）：

- macOS：维持 `parent()?.parent()?.join("Frameworks/libmpv.dylib")`（dev `target/Frameworks` 与 .app 两种布局都命中）。
- Windows：`exe.parent()?.join("libmpv-1.dll")`，安装目录即 DLL 所在目录；`build.rs` 在 dev 模式下把 vendor 的 DLL 集拷到 `target/{profile}/`，供 `tauri dev` 与集成测试命中同一布局。
- Linux：`exe.parent()?.join("libmpv.so")`；deb 的 `/usr/bin`、AppImage 的内挂 `usr/bin` 均为安装目录，dev 模式拷贝到 `target/{profile}/` 同策略。

判定候选存在的分支返回 `PathBuf`，否则 `None` → 既有错误路径把「缺失库」显式化（spec `desktop-playback` 的「平台播放后端缺失」场景）。

`main.rs` 顶部加 `#![windows_subsystem = "windows"]`：exe 不再挂 console 子系统，黑框消失。该属性只对 Windows 有意义，其它平台无影响。

**Why**：`MpvSys::load` 加载即解析符号缺失、`HandleError` 已把 ABI 漂移显式化，无需新增加载路径存根；`current_exe` 相对目录在三种打包布局下都稳定可达，避免 `executable_dir` 在 macOS 的 `None` 坑。`windows_subsystem` 是 Windows 崩溃问题的直接成因之一，一并修掉。
**备选**：用 Tauri `resource_dir` / bundle resources —— 会引入框架版本差异（`extraResources` vs `resources`），且 dev 模式下还要为它单独做 staging 路径，`current_exe` 方案在各平台天然对 dev/release 统一。

### D3. 打包布局：Windows 用 NSIS `extraResources`（或等价的目录扩容），Linux 用 `bundle.linux`

- Windows：NSIS 安装器配置把整组 DLL（mpv + ffmpeg deps）放进安装根目录，保证与 `exe.parent()` 同目录诉求吻合。
- Linux：`bundle.linux` 配置把 `libmpv.so` 与 `libav*.so` 打进 AppImage 与 deb（AppImage 内 `/usr` 布局、deb 的 `/usr/bin`），与 exe 同目录解析一致。

**Why**：与 D2 的「exe 同目录」解析契约形成闭环；沿用 Tauri 已支持的 bundle 机制，不引入自定义安装脚本。
**备选**：Windows 用第二 `resources` 目录 —— 需要改解析逻辑到额外目录，且 NSIS 对多目录的资源管理更繁琐。

### D4. 供应链管理：三平台共享同一 `manifest.json` 结构，`NOTICE.md` 各平台一份（内容并入统一许可页）

`vendor/libmpv/{macos,windows,linux}/` 各自：`manifest.json`（`sourceUrl`、`version`、`libmpvAbi`、`architectures`、`files`→SHA-256、`licenses`）+ `NOTICE.md`。Windows/Linux 的 NOTICE 沿用 macOS 既有的「分节许可证文本 + 来源链接」格式；FFmpeg/Mbed TLS 等共同依赖的许可文本在构建时合并进统一 `NOTICE.md`（artifact 页），让终端用户一份文件看全三平台（spec `platform-libmpv-supply-chain` 的「新增来源纳入统一 NOTICE」）。

**Why**：task-9.7 已按 macOS manifest 结构做校验，三平台统一结构让校验逻辑可复用而非复制一份 mjs；许可来源合并到统一页符合「随发布物提供第三方许可」的既有 ci-release-pipeline requirement（其文本本次扩展为三平台）。
**备选**：每个平台独立许可文档 → 结构不一致，Gate 与发布资产都要分别枚举。

### D5. Gate 扩展：`task-9.7` Windows/Linux 分支落地 + 新增 9.8 平台 Gate 含真机冒烟

- `task-9.7`：把现在的 Windows/Linux deferred note（第 218-257 行）替换为正式校验 — Windows 检查 vendor 完整性 + `bundled_libmpv` 的 Windows 分支与 tauri.conf Windows extraResources 映射；Linux 检查 vendor 完整性 + glibc 版本门槛。
- 新增 `task-9.8`（平台 Gate）：在 Windows 与 Linux runner 上跑「安装包内真实 DLL/SO 冒烟」——参照 task-8.12 的禁静默跳过形态，用 `libloading` 加载 vendor 库、断言 `api_version` 通过、调用一个真实 mpv 命令；Windows 额外探测 WebView2 存在性并报告（不阻断）。CI 把三平台都纳入 Gate job。

**Why**：既有 task-8.12 已验证「macOS 上冒烟 + 防静默跳过」的形态有效；9.8 把它复制到 Windows/Linux 真机，堵住「打包了但打不开」的最后一环。spec `platform-libmpv-supply-chain` 的 ABI 契约、`ci-release-pipeline` 的三平台许可要求都由此落地。
**备选**：只在 release.yml 加启动探测 → 探测失败仍产出 Release 资产，用户拿到坏包；Gate 放在 CI 的 PR 门槛上才能提前止损。

## Risks / Trade-offs

- **[Linux 自建产物的 glibc 门限]** 构建环境 glibc 高于目标发行版，AppImage/deb 跑在旧发行上报 `GLIBC_2.35 not found` → 构建容器固定 **Ubuntu 22.04（glibc 2.35）**，使产物的 glibc 符号版本天然 ≤ 2.35，同时兼容 22.04 与 24.04；把最低 glibc 记录进 manifest；9.8 用 `readelf --version-info` 断言库文件本身不要求 > 2.35，发布门槛挡住超标的候选。
- **[Windows 安装器 DLL 布局漂移]** NSIS 若把 DLL 放进 `bin/` 而非根目录，`exe.parent()` 解析就会失配 → 9.7 校验 tauri.conf 的 Windows extraResources 目标路径对应根目录；9.8 冒烟直接在安装目录解析，布局错了会当场失败。
- **[shinchiro 源站下载不稳定]** SourceForge 镜像慢或变动 URL → 在 CI 加镜像重试与固定校验；manifest 记录 URL + SHA-256，任一变化都触发校验失败而非静默采用。
- **[Windows WebView2 缺失（环境自身问题）]** 冒烟能启动应用但 WebView 建不起来 → 9.8 把它作为可报告项而非冒烟失败条件，避免 Gate 把环境问题误判为供应链问题。
- **[`windows_subsystem` 改变控制台行为]** 消除黑框是正确的，但意味着 `println!`/`eprintln!` 不再有控制台可见 → 既有 panic-hook 已把日志落到 `app_data_dir/logs`，错误信息仍可追溯，spec「明确可读错误」由日志承担。

## Migration Plan

- 无用户数据迁移；这是纯打包/加载路径变更，可随下一个发布版本一起出。
- 回滚策略：保留 macOS 路径代码不变；Windows/Linux 分支若冒烟失败，可在 `release.yml` 临时只发布 macOS、并在 `bundled_libmpv` 的 Windows/Linux 分支关掉（回到 `None`），即恢复「不闪退但不可播放」旧状 —— 但这正是要修的 bug，故回滚仅作 CI 故障期的临时止血，不持久。
- 供应链侧：Linux vendor 产物第一次由 CI 落仓后进仓库；后续换版本走 proposal 的「重新生成 manifest → 重新校验」流程。

## Open Questions

- 无 —— 会影响 spec、方案或任务拆解的未知项已在上文决策中定死；剩下的（shinchiro 具体版本号、Linux 构建脚本的精确容器镜像）是任务级细节，留到 apply 时按 manifest 实际落点定。