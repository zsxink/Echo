## Why
关联 Issue：[Echo #176](https://github.com/zsxink/Echo/issues/176)。

Windows 的 walker 测试夹具使用默认文件型的 `mklink` 命令创建指向既有目录的链接。链接创建成功并不保证目录链接语义，导致 `media/` 保护场景没有得到规格要求的输入，可能被当作没有媒体树而返回空结果。

## What Changes

- 测试夹具改用 Rust 标准库的原生符号链接 API：Windows 根据既有目标类型创建目录或文件链接，Unix 使用原生符号链接调用，不再通过 shell 解释路径。
- 成功创建后校验链接本身及跟随链接后的目标类型；保留扫描行为和拒绝/不遍历的断言。
- 既有 `local-library` 规格已定义该行为，不改变产品用例、生产代码、默认 CI 范围或依赖。

## Capabilities

- 不新增或修改产品能力；覆盖 `local-library` 的跨平台路径、隐私与性能约束（LL-R08-S04）。

## Impact

仅改动 Core Infrastructure walker 模块的邻近单元测试。按 `docs/DESIGN.md` 保持 Core 跨平台及原有依赖方向，无接口、存储、用户界面或路线图变更。
