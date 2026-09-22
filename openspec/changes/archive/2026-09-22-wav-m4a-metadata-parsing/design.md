## Context

现状（0.1.1）已核实：`.m4a`、`.wav` 位于 Core 支持矩阵 `SUPPORTED_EXTENSIONS`（`crates/echo-core/src/application/scan.rs:40`），probe 已把 AAC/ALAC→`Mp4`、PCM→`Wav`（`probe.rs:261-277`）并有 fixture 测试；播放走 libmpv 已覆盖；桌面文件关联 drift 测试已钉死保证格式。缺口集中在**元数据解析无测试担保**与**对话框过滤宽于矩阵**。动机见 proposal.md — Why。

约束：行为规格（`safe-file-ingestion`、`local-library`）已声明 m4a/wav 支持与「未知艺人/未命名歌曲」兜底（`crates/echo-core/src/domain/text.rs:291-323`），本 change 只做验收强化与一处真实行为收敛，不新造规则。

## Goals / Non-Goals

**Goals:**
- 让 m4a 标签解析获得与 mp3/flac/ape 同等的 fixture 测试担保。
- 让 wav 无标签路径验证既有兜底规则的端到端成立。
- 对话框过滤器与 `SUPPORTED_EXTENSIONS` 严格一致。

**Non-Goals:**
- 不修改 `AudioFormat`、`SUPPORTED_EXTENSIONS`、SQLite schema 或文件关联契约。
- 不引入 symphonia `aac`/`alac` 解码 feature（播放已由 libmpv 覆盖；core 侧解码是未来独立关注点）。
- 不改变无标签歌曲的兜底规则本身（规则已存在且已被测试覆盖）。
- 不在 UI 层新增「导入无标签 wav」向导或提示（非所需）。

## Decisions

### 1. m4a 标签解析：复用既有 `tags.rs` fixture 测试骨架

`gen-fixtures.mjs` 已生成带标签的 `tone-short.m4a`（`M4A Tone`），但 `tags.rs` 测试从未引用它。方案：在 `tags.rs` 测试模块仿照 `tone-short.flac` 用例新增 `.m4a` 断言——`title="M4A Tone"`、artist/album 与 fixture 一致、`parameters.sample_rate_hz` 等参数来自容器属性、`duration` 保持 probe-owned（`None`）、无警告。同时补 `read_bytes` 与 `read_from_file` 一致性断言，复用现有 `read(fixture)` helper（`tags.rs:349`）。

为什么用 fixture 而非内存构造：与现有格式的测试姿势完全一致（highfixture-driven），且能真实验证 lofty 对 MP4/AAC 容器的 tag 读取路径，而非构造出的假标签。

### 2. wav 无标签兜底：复用既有领域规则，加端到端测试

目标命名兜底已由 `target_artist_component`/`target_file_stem`（`domain/text.rs:309-323`）与 `plan_named_target`（`import/report.rs:41`）实现并被「标签缺失」测试覆盖。wav 无标签时**无需新代码**——只需在导入/扫描测试中补 `.wav` 用例，断言兜底命名与 `format=Wav` 成立。

扫描侧：在 `scan.rs` 或 `import/tests.rs` 配一个真实（或无标签种子）wav，断言歌曲记录可建立、可检索、可用兜底标题展示。这验证「扫描 wav」路径与「导入 wav」路径都成立。

### 3. 对话框过滤器收敛：从字母表改为引用 Core 矩阵

`apps/desktop/src-tauri/src/dialogs.rs:57-62` 现在手写 `["mp3","flac","m4a","aac","ogg","opus","wav","aiff","ape"]`。方案：改为持有与 Core `SUPPORTED_EXTENSIONS` 一致的集合（desktop 侧同一份 8 项矩阵），移除 `aac`、`aiff`。

为什么收敛而不是保留：`aac`/`aiff` 不在 Core 矩阵内，用户选中后必然在导入阶段被拒绝——这是「可选但必失败」的误导性入口。收敛到矩阵让文件选择与实际可导入能力一致。备选方案「在导入阶段静默跳过非矩阵扩展名」被拒：会让用户误以为导入成功却无结果，违反「结果清晰」原则（`safe-file-ingestion` Purpose）。

实现注意：`SUPPORTED_EXTENSIONS` 是 `pub(crate)`，desktop crate 无法直接引用。选项（a）desktop 测断言对话框列表与它声明的支持集一致（用 `AudioFormat` 派生，与 `security.rs:433` 的 drift 测试同构）；（b）把矩阵提升为公开导出。倾向 (a)：先在 desktop 侧派生列表，再让 `dialogs.rs` 转为从该派生列表构建，配 drift 测试锁定，避免为一次过滤合入跨 crate 公共面扩张。

## Risks / Trade-offs

- [对话框过滤列表可能再次漂移] → 用 `AudioFormat` 全集派生 + 断言 tauri.conf `fileAssociations` 相等（复用 `security.rs:433` 已有模式），单点锁定。
- [`.wav` 若某实现意外带标签（ID3v2-in-WAV），兜底断言会误伤] → 测试断言设在与现有无标签 fixture 一致的预期（`tone-short.wav` 明确无标签）；若 fixture 变更需同步。
- [m4a fixture 的标签字段是生成脚本写的，改动需重生成] → 复用 `fixtures/audio/checksums.sha256` 校验；如测试失败先查 fixture 未被污染。

## Migration Plan

无迁移：不改 schema、不改领域规则、不发布接口。对话框过滤收窄随本次 change 一起落地；若个别用户依赖 `aac`/`aiff` 入口，影响为「无法在文件选择器中看见」，仍可拖放或以其他方式导入（拖放入口不经该过滤器）。

## Open Questions

无。