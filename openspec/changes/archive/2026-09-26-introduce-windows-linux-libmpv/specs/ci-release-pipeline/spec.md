## MODIFIED Requirements

### Requirement: 第三方许可证随发布物提供

系统 MUST 在 Release 中附带桌面端打包进应用的三平台第三方组件的许可证说明。至少包含 macOS、Windows、Linux 三个平台打包目录 `apps/desktop/src-tauri/vendor/libmpv/{macos,windows,linux}/NOTICE.md` 所声明的 libmpv 及关联库的许可证文本或来源，随安装包与校验清单一同上传，使终端用户可获取。

#### Scenario: 许可证说明随包上传

- **WHEN** GitHub Release 创建
- **THEN** Release 资产中包含第三方许可证说明，且可下载

#### Scenario: 三平台许可齐全

- **WHEN** GitHub Release 创建且三平台构建均成功
- **THEN** Release 资产包含三个平台各自的第三方许可证说明，任一平台缺失则发布不视为完整