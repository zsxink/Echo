# Spec Delta

## ADDED Requirements

### Requirement: CI status checks use stable platform names

持续集成与发布构建矩阵 MUST 为每个受支持平台提供明确的平台语义标识，并 MUST 使用该标识构造面向维护者的 job 显示名。显示名 MUST NOT 包含 runner label；runner label 仅用于选择执行环境。更新 runner label 时，相同 job 的平台语义显示名 MUST 保持不变。矩阵校验 MUST 验证平台标识与其 runner 配置的对应关系，并能在该映射缺失或错误时失败。

#### Scenario: CI 检查名不泄露 runner label

- **WHEN** Rust 与 Frontend 的 macOS、Linux、Windows CI 矩阵运行
- **THEN** 检查名分别按 job 类型与平台显示（例如 `Rust (macOS)`、`Frontend (Linux)`），且不显示 `xcode-27`、`ubuntu-latest` 或 `windows-latest`

#### Scenario: 发布构建检查名不泄露 runner label

- **WHEN** macOS、Windows、Linux 发布构建矩阵运行
- **THEN** 检查名分别显示 `Build (macOS)`、`Build (Windows)`、`Build (Linux)`，而 runner label 只用于执行环境选择

#### Scenario: Runner label 更新不改变平台检查名

- **WHEN** 某平台的 runner label 被替换，但该矩阵的平台映射保持正确
- **THEN** 该平台面向维护者的检查名保持不变

#### Scenario: 缺失或错误的平台映射被门禁拒绝

- **WHEN** CI 或发布矩阵缺少某受支持平台条目，或平台标识映射到错误的 runner
- **THEN** 对应 workflow 结构门禁以非零状态失败并指出不满足的平台映射
