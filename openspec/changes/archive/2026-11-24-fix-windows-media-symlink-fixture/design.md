## Context

`has_media_dir` 的目录判断在保护检查前运行。Windows 目录和文件符号链接有独立创建类型；默认文件型夹具不是目录测试所需场景。

## Goals / Non-Goals

- 让 Windows、macOS、Linux 的既有文件和目录符号链接测试以等价的输入覆盖产品行为。
- 不修改生产扫描逻辑、产品规格、CI 矩阵和现有错误断言。

## Decisions

读取既有测试目标的元数据，确认它是普通文件或目录。Windows 按类型使用 `std::os::windows::fs::symlink_dir` 或 `symlink_file`，Unix 使用 `std::os::unix::fs::symlink`。首轮 Windows 验证中 `mklink /D` 修正了根目录场景，但另外三个嵌套路径被 cmd 解析成无效开关，原生 API 避免该歧义。创建成功后，先用不跟随链接的元数据验证链接本身，再用跟随链接的元数据验证文件或目录种类。只在权限不足（含 Windows 1314）时允许本机测试跳过；意外错误或成功后形状不匹配必须失败。临时 Windows 验证禁止任何夹具跳过。没有新增依赖、公开接口、跨层引用或失效实现。
