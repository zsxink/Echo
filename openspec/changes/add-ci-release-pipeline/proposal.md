## Why

0.1.0 已以 macOS 单平台验证封版,任务 13.7(CI 三平台打包)明确留待后续 change 交付:macOS notarized DMG、Windows 安装包、Linux AppImage/deb,以及全新/覆盖安装、卸载、libmpv 装载、checksum 和许可证。当前仓库只有 `.github/workflows/ci.yml`(push/PR 的验证流水线),没有任何 tag 触发的发布流水线;`tauri.conf.json` 的 `bundle.targets` 固定为 `["app"]`,在 Windows 与 Linux 上不产出任何安装包。用户要从零把"三端桌面版构建 + tag `v0.1.0` 触发 + 发布为 GitHub Release"补齐。

## What Changes

- 新增 `.github/workflows/release.yml`:以 `push` tag `v*` 触发,也支持 `workflow_dispatch` 手动触发;按 macOS/Windows/Linux 三平台矩阵构建发布包,上传为一个 GitHub Release,并附 checksum 与第三方许可证说明。
- 调整 `apps/desktop/src-tauri/tauri.conf.json` 的 `bundle.targets`:由 `["app"]` 改为按官方标准对齐 13.7 —— macOS 产出 `.app` + `.dmg`,Windows 产出 NSIS `.exe`,Linux 产出 `.deb` + `.AppImage`;Tauri 按平台自动过滤不可用 target,故用一个联合数组即可。
- macOS 产物架构定为 **arm64**(Apple Silicon);universal/Intel 以及代码签名、公证留待后续 change 接证书后处理。本 change **跳过签名与 notarization**。
- 发布 workflow 对 tag 版本号与 `tauri.conf.json` 的 `version` 字段做一致性校验,不一致即失败。
- 产物装配:每个平台生成 `SHA256SUMS`,并在 Release 中附带 libmpv 第三方许可证说明(`NOTICE.md`)。

## Capabilities

### New Capabilities

- `ci-release-pipeline`:定义 Echo 的三平台发布流水线的外部可观察行为 —— 触发方式、三平台产物矩阵、tag↔版本一致性校验、checksum 与许可证随包、发布为 GitHub Release。

### Modified Capabilities

<!-- 无现有 capability 的需求级行为变化。13.7 尚未写入任何 spec;本 change 新建能力承载。 -->

## Impact

- **CI/交付**:新增 `.github/workflows/release.yml`(独立于现有 `ci.yml`);本地发布命令 `pnpm echo release` 继续作为单平台发布入口,workflow 复用它,不重复实现构建编排。既有 `ci.yml` 的 `macos-platform-gate`(task-9.7)检查 `target/aarch64-apple-darwin/.../Echo.app` 的 libmpv checksum —— 本 change 不改变 app target 的产出路径,**9.7 不受影响**;但 `bundle.targets` 变化会使后续 `tauri build` 在三平台额外产出 dmg/nsis/deb/AppImage,需确认既有 gate 不因此失败。
- **构建配置**:`apps/desktop/src-tauri/tauri.conf.json`(bundle targets);`apps/desktop/package.json` 与根 `package.json` 版本语义保持一致(tag `v0.1.0` ↔ `version 0.1.0`)。
- **工程治理**:新 capability 需登记进规格/追溯/场景清单(`docs/traceability.md` + `scripts/verify/manifest.json`),使"发布流水线领域"不被门禁静默排除。
- 不涉及 product 行为、不涉及 Core/domain 逻辑、不引入新的运行时依赖。