## Why

延续 [#165](https://github.com/zsxink/Echo/issues/165) 和 Draft #166。Linux CI 从 runner 预装 Rust 1.99.1 更新到 1.100.0 时，即使 Echo 使用的 1.98.1 与依赖均未改变，缓存键仍改变并触发冷编译。

## What Changes

- Linux 的 workspace 和 governance 两个 Rust 缓存组，使用一致的独立 Rustup home，仅安装项目钉住的工具链。
- 保留缓存 action 对编译器、环境和依赖的校验，以及 main-only 的写权限。
- 增加结构门禁与反向证明。原生测试、发布、平台打包、benchmark 和缓存外的产品代码不变。

## Capabilities

### Modified Capabilities

- `engineering-governance`：Linux Rust 缓存环境及其隔离。

## Impact

只影响 CI 基础设施，不改变产品定位、架构层次、路线图或接口。首次迁移到新键预计冷启动；之后由正常的 main CI 写入并供 PR 验证。
