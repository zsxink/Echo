## Why

`.m4a` 与 `.wav` 已随 0.1.1 进入支持矩阵（扫描、导入、播放、文件关联均已就位），但**元数据解析环节缺少测试担保与端到端验收**：`tone-short.m4a` fixture 带标签却从未被标签解析测试断言，`tone-short.wav` 无标签的兜底路径没有验证，导入对话框过滤也宽于 core 支持矩阵（多列 `aac`、`aiff`）。这些问题不解决，m4a/wav 的「能导入、能解析歌曲元数据」只是偶然成立，没有回归保护。

## What Changes

- 为 m4a 标签解析补齐 fixture 驱动的测试担保：`LoftyMetadataReader` 对 `tone-short.m4a` 的 `read`/`read_bytes` 与其它格式一致（title/artist/album 断言、超限与缺失字段行为一致）。
- 为 wav 无标签路径补齐端到端验收：wav 经扫描/导入后套用既有「未知艺人 / 未命名歌曲」兜底，歌曲记录 `format=Wav`、可正常展示与播放。
- 将导入对话框扩展名过滤与 core 支持矩阵 `SUPPORTED_EXTENSIONS` 对齐，移除越宽的 `aac`、`aiff`（选中后必被导入阶段拒绝的无意义项）。
- 保持 m4a/wav 的播放支持现状不变（libmpv 已覆盖），不引入解码 feature，不触碰 `audio_format` 兼容面。

## Capabilities

### New Capabilities

（无——行为规格均已存在）

### Modified Capabilities

- `safe-file-ingestion`: 导入对话框过滤与 core 支持矩阵严格一致；m4a/wav 导入端到端验收可验证
- `local-library`: m4a/wav 元数据解析场景的验收强化；wav 无标签时使用既有兜底规则

## Impact

- **代码**：`crates/echo-core/src/infrastructure/metadata/tags.rs`（新增 m4a fixture 测试）、`apps/desktop/src-tauri/src/dialogs.rs`（对话框过滤收敛）、`crates/echo-core/src/application/import/`（wav 兜底端到端测试，若现有测试已覆盖则仅补断言）、`crates/echo-core/src/application/scan.rs`（不改 `SUPPORTED_EXTENSIONS`，仅作为对齐基准）
- **测试**：`tags.rs` 新增 m4a 用例；导入/扫描测试补 wav 无标签用例；必要时补对话框过滤断言
- **不改**：`AudioFormat` 枚举、`SUPPORTED_EXTENSIONS` 矩阵、SQLite schema、播放层、文件关联契约
- **兼容性**：无破坏性变更；对话框过滤收窄是移除导入阶段必然失败的无意义选项