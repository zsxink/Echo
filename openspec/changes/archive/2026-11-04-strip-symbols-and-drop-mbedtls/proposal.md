# Proposal

## Why

Issue #100 的第一项（fat 二进制收敛为 arm64-only）已随 #139 落地，实测节省 8.37 MB。继续实量发现两项**无需重编译**的确定性收益：根 `[profile.release]` 未配置 `strip`，`Echo` 主程序约 1.9 万个本地符号使 `__LINKEDIT` 占 4.8 MB（strip 后 25.82 MB → 21.99 MB），17 个仍带本地符号的 vendor dylib 另有约 1.18 MB 收益；同时 `libmbedcrypto` / `libmbedtls` / `libmbedx509` 三个库不在 `libmpv.dylib` 的依赖闭包内，主程序与所有 dylib 的 `otool -L`、`strings` 均无引用（当前 FFmpeg 构建未链接 TLS 后端），属多打包的 Homebrew 依赖，合计 1.45 MB。合计约 6,466,960 字节（≈6.17 MiB），主程序 + Frameworks 从 54.8 MB 降到 48.3 MB。收益确定且不涉及播放后端重建，值得在 #100 之后立即落地。

## What Changes

- **Rust 侧全局 strip**：在根 `Cargo.toml` 的 `[profile.release]` 增加 `strip = "symbols"`（现有 `lto = "thin"` / `codegen-units = 1` 不变）。该配置只在工作区根 manifest 生效，且对 macOS / Windows / Linux 三平台 release 产物同时生效；崩溃回溯将丢失本地符号名（**BREAKING**：可观测性降级，不影响运行）。
- **vendor dylib strip**：在 `scripts/release/thin-macos-libmpv.mjs` 的 `--write` 路径中，于 ad-hoc 重签之前对每个 Mach-O 库执行 `strip -x`（仅剥离本地符号），复用脚本已有的 SHA-256 与 `fileArchitectures` 重算、`ensureSignature` 重签和原子替换机制；Gate 增加「无成规模本地符号」的断言，防止未来重建回退。
- **删除 mbedTLS 三个库**：从 vendor 磁盘、`manifest.json`、`tauri.conf.json` 的 `bundle.macOS.files`、`NOTICE.md`、`THIRD-PARTY-LICENSES.txt` 五处同步移除 `libmbedcrypto.dylib` / `libmbedtls.dylib` / `libmbedx509.dylib`；manifest 内部的 `files`、`fileArchitectures`、`licenses` 三处一并清理；`THIRD-PARTY-LICENSES.txt` 中与 Windows Vulkan Loader、macOS libgraphite2 共用的 Apache-2.0 段只删 mbed TLS 措辞，保留其余组件。
- **验收口径修正**：issue #137 的「`rg -n mbed` 为空」按字面不可达（`-i mbed` 命中大量 `embedded`，归档 change 的 `evidence/*.json` 亦为历史快照），改为精确匹配活跃配置/许可文件（`mbedtls|mbedcrypto|mbedx509|mbed TLS|Mbed-TLS`）为空，归档证据不在清理范围。
- 记录 strip 前后 `Echo.app` 与 DMG 的实际体积对比，作为 issue #137 的交付证据。

明确非目标：

- 不做 FFmpeg/mpv 重编译裁剪（去视频 codec/filter、禁用 libass 等，潜在 13–15 MB），需先补可复现的 macOS 构建脚本，另行评估（issue #100 第二项）。
- 不评估系统原生解码器复用（桌面端不成立，见 #100 第三项）。
- 不引入 macOS-only 的 target 级 strip 配置（已确认采用全局 strip，见 design 决策 1）。
- 不改动 Windows / Linux 的 libmpv 供应链文件（`windows/manifest.json`、`tauri.windows.conf.json` 等）。
- 不清理 git 历史或归档 change 的历史快照。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `platform-libmpv-supply-chain`：新增「macOS 播放后端符号与依赖最小集」要求——进入发布产物的播放后端二进制 MUST 不含成规模的本地符号，manifest / 打包映射 / 许可说明 MUST 只覆盖实际分发且被依赖闭包引用的组件，删除组件后供应链记录与二进制保持一致。这是对既有「库完整性校验与 ABI 契约」「三平台打包与加载契约」「第三方许可证说明」的可验证补充，不改变其原有行为。

## Impact

- **产品/路线图基准**：`docs/DESIGN.md` 中「桌面播放由桌面适配层管理、不进入共享 Core」的分层不变；`docs/ROADMAP.md` 的 0.1.0 三平台 libmpv 基座与发布验收不受影响。本变更只作用于发布产物的字节构成与供应链记录，不改变播放能力、格式覆盖与用户可见功能。
- **构建/打包**：根 `Cargo.toml` 的 profile 变更影响三平台 release 产物；`tauri.conf.json` 的 `bundle.macOS.files` 从 22 条减到 19 条；`codesign` 与 `bundle.macOS.signingIdentity: "-"` / `hardenedRuntime: false` 流程不变。DMG 压缩后实际收益小于未压缩数值。
- **供应链**：`apps/desktop/src-tauri/vendor/libmpv/macos/` 下 3 个二进制删除、17 个 dylib 字节改变，`manifest.json` 的 `files` SHA-256 全量重算、`fileArchitectures` 不变（仍全为 arm64）、`licenses` 去掉 mbed TLS 条目；`NOTICE.md` 与 `THIRD-PARTY-LICENSES.txt` 同步。这些文件是 `licensing-privacy-notices` 与 `ci-release-pipeline` 所依赖的事实来源，须与二进制一致。
- **验证 Gate**：`scripts/verify/checks/task-1.9.mjs`、`task-1.10.mjs`、`task-9.7.mjs` 均按 `Object.keys(manifest.files)` 动态遍历，删库不需改其清单；需在 `task-9.7.mjs` 增加符号纯度与依赖闭包的拒绝断言，并在 `scripts/verify/manifest.json` 的 9.7 组注册新增单测。`ci.yml` 的 `pnpm verify:task -- 1.9 1.10` 字符串被 `task-1.10.mjs` / `task-9.7.mjs` 硬断言，不得改动。
- **代码规范**：`openspec/CODE_STANDARDS.md` 的分层、单文件 ≤500/1000 行、纯函数可测、不在业务路径静默失败等约束适用；`engineering-governance` 要求规格、追溯记录与场景清单三方一致，新增 Gate 断言须有注入式自证用例，不得静默通过。
- **交付流程**：关联 GitHub issue #137，在独立分支实施，通过关联该 issue 的 PR 合并到 `main`；实现与验证完成后先同步 delta spec 并归档 change，再创建 PR。
