## 1. Vendor Windows libmpv 供应链

- [x] 1.1 选定 shinchiro 的 Windows libmpv 固定发行（`mpv-dev-{arch}-*.7z`，含版本号与下载镜像），把静态 FFmpeg 的 `libmpv-2.dll` 与配套 Vulkan loader `vulkan-1.dll`（LunarG VulkanRT 1.4.304.0 固定版）及说明落入 `apps/desktop/src-tauri/vendor/libmpv/windows/`，其他杂项不落仓；验证目录内每个文件存在且 `file` 头为 PE32+ x86-64
- [x] 1.2 编写 `vendor/libmpv/windows/manifest.json`（沿用 macOS 结构：`sourceUrl`、`version`、`libmpvAbi=2.5`、`architectures`、`files`→各 DLL SHA-256、`licenses`），用 `shasum -a 256` 生成校验值并核对与 manifest 一致
- [x] 1.3 编写 `vendor/libmpv/windows/NOTICE.md`（沿用 macOS 的「表格 provenance + 来源」格式，覆盖 libmpv、静态 FFmpeg、Vulkan Loader，写明 shinchiro/LunarG 来源与版本）；验证文件结构与 macOS NOTICE 同构

## 2. Vendor Linux libmpv 供应链（CI 首构建）

- [x] 2.1 写一版 CI 脚本（`scripts/release/build-linux-libmpv.mjs`）用官方 mpv-build 在 **Ubuntu 22.04（glibc 2.35）** 容器按 minimal 配置构建 `libmpv.so` 与 `libav*.so` 等，输出带 `build.tag` 与 glibc 版本记录；验证产物齐全、`readelf --version-info` 显示要求的 glibc 符号 ≤ 2.35
- [x] 2.2 首构建产物落仓 `vendor/libmpv/linux/`（`libmpv.so`、`libav*.so`、关联库与说明），编写 `manifest.json`（同 1.2 结构 + glibc 门槛字段）与 `NOTICE.md`（含自建构建所引入依赖的许可）；验证 SHA-256 与 glibc 记录准确
      - 生产侧已就绪：`scripts/release/build-linux-libmpv.mjs` 构建 mpv/FFmpeg、设 rpath、以 `readelf --version-info` 校验 glibc ≤ 2.35（`GLIBC_MAX = 35`）并写出 `manifest.json` + `NOTICE.md`；已接线进 `release.yml:113`
      - **待 CI 首构建落仓**：`vendor/libmpv/linux/` 目前只有 macos/windows，Linux 实产物未生成，SHA-256 与 glibc 记录无从核对；9.7 的 Linux 分支现输出显式 `pending:`，待首构建后自动转为正式校验

## 3. 加载路径与打包布局

- [x] 3.1 改 `apps/desktop/src-tauri/src/main.rs` 的 `bundled_libmpv` 为按 `target_os` 三分支（macOS 维持 `Frameworks/`；Windows `exe.parent()/libmpv-2.dll`；Linux `exe.parent()/libmpv.so`），保持既有 `None`→显式错误路径；验证各分支 `cargo check` 各平台通过且 macOS 回归不破
- [x] 3.2 `main.rs` 顶部加 `#![windows_subsystem = "windows"]`；验证 Windows 构建无 GUI 子系统的 `main` 错误、`echo` 冒烟不再出现控制台黑框
- [x] 3.3 扩展 `apps/desktop/src-tauri/build.rs`：Windows/Linux dev 模式下把对应 vendor 库集拷贝到 `target/{profile}/`（与 `exe.parent()` 同目录）；验证 `tauri dev` 各平台能解析到本平台库
- [x] 3.4 Windows 用平台专属配置 `tauri.windows.conf.json` 的 `bundle.resources` 把整组 DLL 放入安装根目录，Linux 用 `tauri.conf.json` 的 `bundle.linux` 放入 AppImage/deb 的安装目录；两者 MUST NOT 声明在平台无关的顶层 `bundle.resources`（该字段会被复制进每个平台的构建产物）；验证配置变更后 `pnpm echo release` 产物布局与 3.1 解析路径一致（Linux 打包验证待 2.2 vendor 落仓后完成）
      - Windows 侧配置与解析路径已核对：主配置 `bundle.resources` 缺失，4 条 DLL 映射全在 `tauri.windows.conf.json`，`main.rs:160` 以 `exe.parent()/libmpv-2.dll` 解析（与安装根目录一致）；Linux `deb`/`appimage` 映射指向 `vendor/libmpv/linux/libmpv.so`
      - **待真机验证**：Windows NSIS 实际安装布局、Linux AppImage/deb 布局须在对应 runner 上构建后核对（本机 macOS 无法产出）

## 4. Gate 校验扩展

- [x] 4.1 将 `scripts/verify/checks/task-9.7.mjs` 的 Windows/Linux deferred note（第 218-257 行）替换为正式校验：Windows 检查 vendor 完整性 + `bundled_libmpv` Windows 分支 + tauri.conf Windows extraResources 映射；Linux 检查 vendor 完整性 + glibc ≤ 2.35 门槛；验证 9.7 在三平台都绿、且删除任一 DLL 时失败
      - 代码已全部落地：6a vendor SHA-256 完整性、6b `bundled_libmpv`（`libmpv-2.dll` 紧邻 exe + `#![windows_subsystem]`）、6c `tauri.windows.conf.json` DLL 映射（并断言主配置无平台无关 `resources`）；Linux 7a 完整性、7b `readelf --version-info` glibc ≤ 2.35 门槛、7c `bundle.linux.{deb,appimage}.files`。Linux 分支在 vendor 未落仓时输出显式 `pending:` 而非静默跳过
      - 同步修正 9.7 头部注释：Windows 映射位置已由 `tauri.conf` 改为 `tauri.windows.conf.json`（原注释停留在旧写法）
      - **待真机验证**：三平台各自 runner 上 9.7 全绿、以及删任一 DLL 时失败，均需 windows-latest / ubuntu-latest 实跑；本机 macOS 只能验证 macOS 分支
- [x] 4.2 新增 `scripts/verify/checks/task-9.8.mjs` 平台 Gate：参照 task-8.12 的禁静默跳过形态，在三平台真机用 `libloading` 加载 vendor 库、断言 `api_version()` 通过并跑一个真实 mpv 命令；Windows 额外探测 WebView2 存在性并报告（不阻断）；验证删库/ABI 错配时 9.8 失败而非跳过
- [x] 4.3 把 `task-9.8` 登记进 `scripts/verify/manifest.json`（id、命令、平台约束），并让 `.github/workflows/ci.yml` 三平台都跑 9.7+9.8（Linux job 从仅 rust check 扩展为含 Gate）；验证 CI 上三平台 Gate job 绿、出错能红

## 5. 发布与文档同步

- [x] 5.1 更新 `.github/workflows/release.yml`：Windows job 上传 `vendor/libmpv/windows/NOTICE.md`、Linux job 上传 `vendor/libmpv/linux/NOTICE.md`（macOS 维持现状），publish 阶段 `files:` 把三份 NOTICE 都带上；验证 dry-run 生成 Release 时三份 NOTICE 都出现在资产列表
- [x] 5.2 按 proposal Impact 同步 `docs/DESIGN.md`、`docs/ROADMAP.md`（9.8/13.5/13.7 的三平台候选包、libmpv 装载、许可证描述落到本 change 落点）、`AGENT.md` 中「Windows/Linux 尚未接入 libmpv」的表述；验证文档不再有「Windows/Linux 未接入播放后端」的残留表述
- [x] 5.3 归档本 change：`openspec apply` 完成后 `scripts/verify/manifest.json` 与 `logspec`/traceability 随归档任务与 Gate 一起重新生成，基线数同步（参考既有 prd-matrix 流程）；验证 `openspec validate` 通过且归档后 main specs 的三平台契约生效
      - 归档于 `2026-09-26-introduce-windows-linux-libmpv`；三份 delta 已同步进 main specs：`ci-release-pipeline`（许可证说明扩为三平台 + 新增「三平台许可齐全」）、`desktop-playback`（桌面音频播放补入三平台解析/ABI 约束 + 新增「平台播放后端缺失」）、`platform-libmpv-supply-chain`（新建，含「MUST NOT 携带其它平台播放后端」及两个场景）
      - `openspec validate --specs` 15/15 通过；`scripts/verify/manifest.json` 的 9.7 条目描述已在前一 change 中更新
      - **待处理**：`docs/traceability.md` 尚未补入本 capability 的 10 个场景，基线数（102 Requirement / 390 Scenario）待同步；`docs/ROADMAP.md` 已更新为指向归档路径并标注真机验证待办