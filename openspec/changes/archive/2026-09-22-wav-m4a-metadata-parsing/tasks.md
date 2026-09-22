## 1. m4a 标签解析测试担保

- [x] 1.1 在 `crates/echo-core/src/infrastructure/metadata/tags.rs` 测试模块新增 `m4a_tags_parse_from_fixture`：用 `read("tone-short.m4a")` 断言 `title == "M4A Tone"`、artist/album 与 `gen-fixtures.mjs` 的 `meta("M4A Tone")` 一致、`parameters.sample_rate_hz` 来自容器属性、`duration` 保持 `None`（probe-owned）、`warnings` 为空
- [x] 1.2 复用既有 `read_bytes` 一致性姿势，新增断言：`tone-short.m4a` 的 `read_bytes(读文件字节)` 与 `read_from_file` 的 title/artist 一致（对齐 `tags_parse_from_in_memory_import_source_bytes` 的测试模式）
- [x] 1.3 验证：`cd crates/echo-core && cargo test metadata::tags -- --nocapture` 通过，且 `cargo fmt --check` 无改动
- [x] 1.4 验证：`cargo test --workspace` 全绿（确保 m4a 用例不破坏既有测试）

## 2. wav 无标签兜底端到端验证

- [x] 2.1 在 `crates/echo-core/src/application/import/tests.rs`（或既有 wav 导入用例处）新增用例：导入无标签 `tone-short.wav`，目标命名按兜底 `未知艺人/未知艺人 - 未命名歌曲.wav`（对齐 `plan_named_target` 的既有断言语义），歌曲记录可建立且 `format == Wav`
- [x] 2.2 在扫描侧（`scan.rs` 测试或 `application/testing/scan_fixture.rs` 支持的姿势）验证 WAV 无标签扫描：`start_scan(&fixture).run(...)` 后歌曲可检索、`format()==Some(Wav)`、标题兜底展示；确认 `.wav` 在 `SUPPORTED_EXTENSIONS` 路径不被跳过
- [x] 2.3 验证：`cd crates/echo-core && cargo test import scan -- --nocapture` 通过，`cargo fmt --check` 干净
- [x] 2.4 验证：`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过
- [x] 2.5 补真实 `LoftyMetadataReader` 读取 `tone-short.wav` 的断言（`tagless_wav_reads_as_ok_with_empty_fields`）：lofty 对无标签 WAV 必须 `Ok` 空字段而非 Err，容器参数仍解析——否则真实导入/扫描路径的兜底规则会失效

## 3. 导入对话框过滤器对齐支持矩阵

- [x] 3.1 在 `desktop` 侧新增一处「支持格式列表」单一来源：从 `AudioFormat` 派生（对齐 `security.rs` `tauri_conf_file_associations_cover_the_guaranteed_formats` 的已有 pattern），供对话框与 drift 测试共用
- [x] 3.2 修改 `apps/desktop/src-tauri/src/dialogs.rs` 过滤器为从该单一来源构建，移除 `aac`、`aiff`；确认 `.m4a`/`.wav` 仍在
- [x] 3.3 新增 drift 测试：对话框过滤器含 `.wav`/`.m4a` 且不含 `aac`/`aiff`，与 Core `SUPPORTED_EXTENSIONS` 一致（用 `AudioFormat` 全集推导，方法同 `security.rs:433`）
- [x] 3.4 验证：`cd crates/echo-desktop && cargo test platform::security dialogs -- --nocapture` 通过（含既有文件关联 drift 测试未破坏）
- [x] 3.5 验证：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过

## 4. 全量回归与规格对齐

- [x] 4.1 验证：`cargo test --workspace` 全绿（含 `echo-desktop` 测试）
- [x] 4.2 验证：`openspec validate wav-m4a-metadata-parsing --strict --no-interactive` 通过（delta spec、requirement 与 scenario 格式合法）
- [x] 4.3 同步 `docs/DESIGN.md` 或能力 spec（如对话框过滤行为需在 `safe-file-ingestion` 或相关文档体现）——若验证命令无相应检查，确认无需文档变更并记录
- [x] 4.4 复核：归档前 `openspec archive` 预览确认 `specs/` 合并进主 spec 无冲突，`.ape` 同款 prior art 对齐（`46a4f37` 的走法）