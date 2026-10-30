# platform-libmpv-supply-chain 增量规格

## MODIFIED Requirements

### Requirement: 第三方许可证说明

系统 MUST 为各平台 vendor 的 libmpv 及其关联库（含 FFmpeg 与其它依赖）提供统一格式的许可证说明文件，随对应平台安装包带入应用；Windows/Linux 引入的库来源新增的许可信息 MUST 合并进与既有 macOS 结构一致的 NOTICE 说明，单一 manifest 与 NOTICE 覆盖三平台。

manifest 与 NOTICE 中记录的每个组件许可 MUST 与该平台实际构建配置一致：构建启用了 GPL 选项或 GPL 组件（如 FFmpeg `--enable-gpl`、链接 x264/x265）时，对应组件 MUST 记录为 GPL-2.0-or-later，MUST NOT 记录为 LGPL。

各平台安装包 MUST 携带打包二进制所涉许可的完整文本（合并许可文本文件），且覆盖 manifest `licenses` 清单中出现的全部许可类型；每个平台的 NOTICE MUST 说明对应源码的获取方式（来源仓库与固定版本/提交）。

#### Scenario: 三平台均随包携带许可说明

- **WHEN** 任一平台的安装包被安装
- **THEN** 应用内包含该平台所 vendor 库的许可证说明，且结构与既有 macOS NOTICE 一致

#### Scenario: 新增来源纳入统一 NOTICE

- **WHEN** 新增某平台库来源或版本（如 Linux 自建产物引入的依赖）
- **THEN** 该来源的许可证信息并入统一 NOTICE，安装包随之更新

#### Scenario: 许可记录与构建配置一致

- **WHEN** 某平台构建配置使组件按 GPL 授权（如保留 `--enable-gpl` 或链接 GPL 库）
- **THEN** 该组件在 manifest 与 NOTICE 中记录为 GPL-2.0-or-later，与二进制的实际许可义务一致

#### Scenario: 许可全文随包可得

- **WHEN** 用户安装任一平台安装包并查找打包库的许可条款
- **THEN** 安装包内的合并许可文本包含该平台 manifest 所列全部许可类型的完整文本

#### Scenario: 对应源码可追溯

- **WHEN** 用户或审计者需要获取打包二进制的对应源码
- **THEN** 该平台 NOTICE 指明来源仓库与固定版本/提交，据此可重建或下载对应源码
