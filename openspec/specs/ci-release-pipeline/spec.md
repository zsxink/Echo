# ci-release-pipeline Specification

## Purpose

定义 Echo 的三平台桌面发布流水线的外部可观察行为:通过 git tag 触发构建,产出 macOS、Windows、Linux 的安装包,校验版本一致性与产物完整性,并发布为 GitHub Release。本能力覆盖 13.7 的非签名基线(notarization、证书、universal 架构留待后续 change)。

## Requirements

### Requirement: API tag 触发发布

系统 MUST 在推送形如 `v<major>.<minor>.<patch>` 的 git tag 时触发三平台发布流水线;也 MUST 支持人工通过 `workflow_dispatch` 触发同一流水线。触发时系统 MUST 将 tag 的版本号(去除前导 `v`)与 `apps/desktop/src-tauri/tauri.conf.json` 的 `version` 字段比对,不一致 MUST 使构建失败并给出明确错误,防止发布错误版本号的应用。

#### Scenario: 语义化 tag 触发构建

- **WHEN** 仓库推送 tag `v0.1.0`
- **THEN** 三平台发布流水线在 macOS、Windows、Linux 各自启动构建

#### Scenario: 版本不匹配时拒绝发布

- **WHEN** 推送 tag `v1.2.3` 而 `tauri.conf.json` 的 `version` 为 `0.1.0`
- **THEN** 发布流水线在任一平台构建前失败,错误信息同时包含 tag 版本与配置版本

#### Scenario: 手动触发

- **WHEN** 维护者人工运行 `workflow_dispatch` 触发发布流水线
- **THEN** 流水线与 tag 触发走相同的构建与发布路径

### Requirement: 三平台产物矩阵

系统 MUST 在各自平台上产出符合平台约定的安装包:macOS 产出 `.app` 与 `.dmg`,Windows 产出 NSIS 安装器(`.exe`),Linux 产出 `.deb` 与 `.AppImage`。产物命名 MUST 包含应用名与版本号。macOS 产物架构在本阶段为 arm64;通用(universal)构建、代码签名与无害化(notarization)不在本能力范围。

#### Scenario: macOS 产物

- **WHEN** macOS 构建成功
- **THEN** 同时产出 `Echo.app` 与 `.dmg`,且 `.dmg` 文件名包含 `Echo` 与版本号

#### Scenario: Windows 产物

- **WHEN** Windows 构建成功
- **THEN** 产出 NSIS 安装器 `.exe`,文件名包含 `Echo` 与版本号

#### Scenario: Linux 产物

- **WHEN** Linux 构建成功
- **THEN** 同时产出 `.deb` 与 `.AppImage`,文件名包含 `Echo` 与版本号

### Requirement: 产物完整性校验

系统 MUST 在三平台构建完成后,为每个平台的安装包生成 `SHA256SUMS` 校验清单,并将其作为发布物之一上传。发布流水线 MUST 在任一平台未产出预期的任何安装包文件时失败,而不是静默继续。

#### Scenario: 校验清单随发布物上传

- **WHEN** 三平台安装包被上传到 GitHub Release
- **THEN** 每个平台对应的 `SHA256SUMS` 也同时存在且可下载

#### Scenario: 缺失产物导致失败

- **WHEN** 某平台的预期安装包文件未生成(构建被跳过或产物路径错误)
- **THEN** 该平台 job 失败,发布流水线不继续进行

### Requirement: 第三方许可证随发布物提供

系统 MUST 在 Release 中附带桌面端打包进应用的第三方组件的许可证说明。至少包含 macOS 打包目录 `apps/desktop/src-tauri/vendor/libmpv/macos/NOTICE.md` 所声明的 libmpv 及关联库的许可证文本或来源,随安装包与校验清单一同上传,使终端用户可获取。

#### Scenario: 许可证说明随包上传

- **WHEN** GitHub Release 创建
- **THEN** Release 资产中包含第三方许可证说明,且可下载

### Requirement: 发布为 GitHub Release

系统 MUST 将三平台的安装包、`SHA256SUMS` 与许可证说明汇总上传到与触发 tag 对应的 GitHub Release 资产中;release 流水线的成功标准是这三个平台的资产齐全。重复触发同一 tag 的后续运行 MUST 不会创建重复的 Release(幂等或明确判定已存在)。

#### Scenario: 资产汇总到 tag 对应 Release

- **WHEN** 三平台构建均成功且 tag 为 `v0.1.0`
- **THEN** 名为 `v0.1.0` 的 GitHub Release 存在,且包含三平台的安装包、三份 `SHA256SUMS` 与许可证说明

#### Scenario: 重复触发不产生重复 Release

- **WHEN** 同一 tag 触发两次发布
- **THEN** 不产生第二个同名 Release;资产更新到既有 Release