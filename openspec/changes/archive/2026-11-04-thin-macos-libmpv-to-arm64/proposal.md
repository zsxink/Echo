# Proposal

## Why

Issue #100 实测发现，macOS 端 vendored 的 libmpv/ffmpeg 播放后端中有 10 个 dylib 是 x86_64 + arm64 的 fat 二进制，而当前发布目标只有 arm64（`manifest.json` 的 `architectures` 与 `ci-release-pipeline` 均已如此声明）。这 10 个库合计 28.30 MB，只保留 arm64 slice 后为 20.32 MB，可无损节省约 **8.0 MB**；而现有 Gate 只断言「包含 arm64」，fat 产物可以静默通过，导致体积税持续存在。

本次变更先落地 issue #100 的第一项（零重编译、收益确定）：把 macOS vendored 播放后端收敛为纯 arm64，并让 Gate 真正拒绝 fat 产物，避免后续重建再次混入 x86_64。

## What Changes

- 用 `lipo -thin arm64` 将 macOS vendored 目录中 10 个 fat dylib 收敛为 arm64-only：
  `libmpv`、`libavcodec`、`libavfilter`、`libavformat`、`libavutil`、`libswresample`、`libswscale`、`libmbedcrypto`、`libmbedtls`、`libmbedx509`。
  其余 10 个依赖本来就是 arm64-only，保持不变。
- 同步 `vendor/libmpv/macos/manifest.json`：把上述文件的 `fileArchitectures` 改为 `["arm64"]`，重新计算并更新 `files` 中的 SHA-256；在构建说明中记录 macOS 侧必须 `ARCHS=arm64` / `ONLY_ACTIVE_ARCH=YES`。
- 更新 `NOTICE.md`：删除「x86_64 slice 保留旧运行时」的说明，改为声明播放后端全部为 arm64-only，与发布目标一致。
- 收紧 Gate：`task-9.7` 的 vendor 与 app bundle 校验、`task-1.10` 的 bundle 校验从「包含声明架构」改为「架构集合与 `fileArchitectures` 精确相等」，出现多余架构即失败。
- 提供可复现的维护脚本（用于本次瘦身及未来重建后的复核）：校验并列出每个 dylib 的实际架构，拒绝意外架构；脚本行为有单测覆盖。
- **BREAKING**：Intel Mac 不再能直接运行/开发这套 vendored 播放后端；发布目标本来就不包含 Intel，故不影响正式产物。

明确非目标：

- 不做 ffmpeg codec/demuxer 裁剪（issue #100 第二项，需重编译）。
- 不评估系统原生解码器或移动端 media_kit 方案（issue #100 第三项）。
- 不改变 Windows/Linux 的 libmpv 供应链，也不引入 universal 构建。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `platform-libmpv-supply-chain`：新增「macOS 播放后端架构纯度」要求——vendored 二进制与 manifest 声明的架构集合必须精确匹配，Gate 必须拒绝 fat/多余架构，manifest、NOTICE 与实际二进制保持一致。

## Impact

- **二进制与供应链**：`apps/desktop/src-tauri/vendor/libmpv/macos/` 下 10 个 dylib 被替换为 arm64-only 版本；`manifest.json` 的哈希与架构记录同步更新；`NOTICE.md` 更新。
- **构建/打包**：`tauri.conf.json` 的 `bundle.macOS.files` 映射不变；app bundle 体积预计减少约 8 MB。dylib 为 ad-hoc 签名，`lipo -thin` 后签名仍有效，打包期重新签名流程不变。
- **验证 Gate**：`scripts/verify/checks/task-1.10.mjs`、`scripts/verify/checks/task-9.7.mjs` 增加架构精确匹配断言；新增维护脚本及其单测（`scripts/release/` 下），并接入 `scripts/verify/manifest.json`。
- **平台边界**：仅影响 macOS vendor 目录与 macOS Gate；Windows/Linux 与 Core、UI 无关。Intel/Rosetta 开发场景不再受支持。
- **许可与合规**：`NOTICE.md` 与 manifest 是 `licensing-privacy-notices` 所依赖的事实来源，二者需与本变更后的二进制一致；许可清单本身不变。
