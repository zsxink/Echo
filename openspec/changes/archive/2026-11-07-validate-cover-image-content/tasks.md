# Tasks

> 每个任务组自带对应测试/文档与验证；任务可追溯到 `LD-R12` 的 8 个场景。所有命令在仓库根目录执行。

## 1. 基线与现状复核（不写代码）

- [x] 1.1 按 `AGENT.md` 从 `main` 新建独立分支（建议 `fix/issue-132-validate-cover-content`），不得直接向 `main` 推送。验证：`git branch --show-current` 输出该分支名，`git status --short` 只包含本 change 的规划文件。
- [x] 1.2 重跑 `design.md` D3 的六条现状核对命令（`rg` 两处 `artwork.rs` 调用点、`cargo tree -p echo-core -e features -i image`、`rg -n -A2 '^name = "image"' Cargo.lock`、`rg -n 'fn cover_media_type' apps/desktop/src-tauri/src/cover_protocol.rs`、`node scripts/verify/spec-scenarios.mjs --check`），把输出写入 `/tmp/echo-issue-132-baseline.txt`。验证：文件含六条命令输出；事实校核与 `proposal.md` 一致（146 Requirement / 564 Scenario、`image 0.25.6` 六项特性、`validate` 两条判断）；不一致先修正 proposal/design/spec 再继续。
- [x] 1.3 记录失败基线：建立一个临时最小复现（只用 `cargo test` 的测试代码，不改生产代码）证明「纯文本字节 + `image/png` 声明」当前能通过 `artwork::validate`。验证：`cargo test -p echo-core --all-features artwork::tests` 中该断言在修复前失败，失败输出追加到基线文件；不要伪造已通过的结果。
- [x] 1.4 复核进行中 change 的边界：`rg -n 'PM-R13|PM-R14|library-durability|artwork' openspec/changes/align-immersive-player-and-playlist-directory/` 与 `git diff --name-only` 记录输出。验证：确认本 change 不修改 `openspec/specs/playlist-management/spec.md`、`openspec/specs/desktop-app-shell/spec.md` 与 `PlaylistNameDialog.tsx`，且不与进行中 change 争用同一批文件。

## 2. Core 失败用例先落地（红，`LD-R12-S01..S03`、`S06`、`S07`）

- [x] 2.1 在 `crates/echo-core/src/application/artwork.rs` 新增 `#[cfg(test)] mod tests`，用 `image` 编码器在测试内生成极小 JPEG / PNG / WebP 字节（不新增 fixture 文件、不依赖 `ffmpeg`/`cwebp`），并准备固定伪造样本：纯文本、ZIP 头、`\x89PNG` 签名 + 截断头、格式错配、仅含合法 IHDR 的 20000×20000 PNG 头。验证：`rg -n 'mod tests|ImageFormat::|IHDR' crates/echo-core/src/application/artwork.rs` 命中；`cargo test -p echo-core --all-features artwork::tests` 能编译。
- [x] 2.2 写 5 个失败用例：`validate_rejects_non_image_bytes_declared_as_png`、`validate_rejects_declared_mime_that_differs_from_detected_format`、`validate_rejects_truncated_or_malformed_container_header`、`validate_reads_only_the_container_header_without_pixel_decoding`、`existing_size_and_mime_whitelist_rejections_are_unchanged`。前 3 个断言 `Error::validation` 且 reason 可区分；第 4 个断言「仅含合法大尺寸 IHDR 的 PNG 头」通过校验（证明无像素解码、无 512 MiB 默认上限误拒）；第 5 个断言空字节 / 5 MiB+1 / `image/gif` 仍按既有原因拒绝。验证：`cargo test -p echo-core --all-features artwork::tests` 中前 3 个失败、后 2 个通过或按预期失败，输出追加到基线文件；不得先改生产代码让它们通过。
- [x] 2.3 写 `validate_accepts_jpeg_png_and_webp_with_matching_mime` 与 `save_and_resolve_round_trip_all_three_formats`（用 `application::testing` 既有 `MemoryControlPlane` / `MemoryCoverCache`，断言目录原件的字节、长度、BLAKE3 与 MIME 一致且 `cache.put` 收到相同字节）。验证：`cargo test -p echo-core --all-features artwork::tests` 中合法格式路径通过；这两种用例作为 `LD-R12-S04` 的正常路径证据保留。

## 3. Core 校验实现（绿，`LD-R12-S01..S07`）

- [x] 3.1 按 design D1/D2 在 `validate` 的「体积 → 白名单」之后追加内容校验：`ImageReader::new(Cursor::new(bytes)).with_guessed_format()` 取实际格式，`ImageFormat::from_mime_type(mime)` 取声明格式，要求 `detected == expected && detected.reading_enabled()`；`reader.no_limits()` 后 `into_dimensions()` 要求非零尺寸。三类失败分别映射为可区分的 `Error::validation(Subject::Other, "cover", ...)`，不得 `unwrap` / `expect` / panic，注释说明「为何不 decode、为何 no_limits」。验证：`cargo test -p echo-core --all-features artwork::tests` 全绿；`rg -n 'with_guessed_format|no_limits|into_dimensions|from_mime_type|reading_enabled|decode\\(' crates/echo-core/src/application/artwork.rs` 显示前五者命中、`decode(` 无输出。
- [x] 3.2 加约束测试 `every_whitelisted_mime_has_an_enabled_decoder`：枚举三种白名单 MIME，断言 `ImageFormat::from_mime_type(...).is_some_and(|f| f.reading_enabled())`；临时从 `Cargo.toml` 移除 `webp` 特性时该测试必须失败，随后恢复特性并把失败输出记入基线。验证：`cargo test -p echo-core --all-features artwork::tests` 通过；`git diff --exit-code -- Cargo.toml Cargo.lock` 输出为空（零依赖变化）。
- [x] 3.3 补 `playlist_and_artist_cover_writes_share_the_same_validation`：用既有 testkit fakes 经 `set_playlist_cover` 与 `set_artist_cover` 提交同一伪造字节，断言两者都返回 `Validation`，且 `MemoryControlPlane` 未记录任何 `write_asset`、未产生 `PortableAssetReference`，已保存封面不变。验证：`cargo test -p echo-core --all-features artwork::tests` 通过；`rg -n 'artwork::save' crates/echo-core/src/application/{playlist/cover,artist_cover}.rs` 仍各命中一次（未新增绕行入口），映射 `LD-R12-S05`。
- [x] 3.4 补 `resolving_an_existing_invalid_original_reports_a_validation_error` 与 `legacy_cache_only_cover_is_unaffected`：前者伪造一个引用指向非图片字节的 `PortableAssetReference`，断言 `resolve` 返回 `Validation`、未调用 `cache.put`、原件字节与记录原样保留；后者断言 `resolve_legacy` 的 cache-only 语义未变。验证：`cargo test -p echo-core --all-features artwork::tests` 通过；`rg -n 'fn resolve_legacy' crates/echo-core/src/application/artwork.rs` 的函数体与 HEAD 一致，映射 `LD-R12-S08`。
- [x] 3.5 运行受影响 crate 的静态门禁：`cargo fmt --all -- --check`、`cargo clippy -p echo-core --all-targets --all-features -- -D warnings`、`cargo test -p echo-core --all-features`。验证：三条命令均退出 0；`crates/echo-core/src/application/artwork.rs` 仍远低于 500 行（规模门禁无新增白名单需求）。

## 4. 服务层回归、架构与兼容边界

- [x] 4.1 在 `crates/echo-desktop/src/runtime/services/tests/playlist_cover.rs` 补 `playlist_cover_rejects_fake_image_without_replacing_the_saved_cover`：先保存一张合法 PNG 封面，再提交「文本字节 + `image/png`」，断言返回 `Validation` 且已保存封面未被替换、名称输入不变。该用例是防止 Core 校验被削弱的服务层护栏，按 #113 的先例**不单独映射场景**。验证：`cargo test -p echo-desktop --all-features playlist_cover` 全部通过。
- [x] 4.2 更新 `crates/echo-desktop/src/runtime/services/playlists.rs` 中 `set_playlist_cover` 的 `# Errors` 文档，把「内容不是声明类型 / 容器头不可解析」补进校验失败原因，不改函数签名。验证：`rg -n 'Validation|容器头|声明' crates/echo-desktop/src/runtime/services/playlists.rs` 命中；`cargo clippy -p echo-desktop --all-targets --all-features -- -D warnings` 退出 0。
- [x] 4.3 运行 `cargo test -p echo-core --all-features --test arch` 与 `cargo check --workspace --all-features`。验证：两条命令退出 0，架构门未出现 `application → crate::infrastructure` 违例，且 `Cargo.lock` 无意外变化。
- [x] 4.4 证明跨边界契约零漂移：`cargo run -p echo-desktop --bin echo-generate-ipc` 后运行 `git diff --exit-code -- apps/desktop/src/ipc/ipc-types.generated.ts`，并运行 `cargo test -p echo-desktop --all-features ipc::generate::tests::committed_generated_file_is_in_sync_with_the_generator`。验证：两条命令退出 0、`git diff` 为空；`rg -n 'set_playlist_cover|set_artist_cover' apps/desktop/src/bridge/index.ts` 与 HEAD 一致。
- [x] 4.5 回归确认内嵌封面链路未被误改：`cargo test -p echo-core --all-features infrastructure::metadata::cover` 与 `cargo test -p echo-core --all-features infrastructure::metadata::tags::tests` 全通过，尤其保留 `DiskCoverCache` 中「非图片字节写入缓存并按原样读回」的既有测试。验证：两条命令退出 0；`git diff --name-only` 中不含 `crates/echo-core/src/infrastructure/metadata/cover.rs` 与 `tags/mod.rs`。

## 5. 项目文档同步

- [x] 5.1 在 `docs/DESIGN.md` §2.4 封面写入条目补一句：Core 在写入资料库原件前校验声明 MIME 与实际容器格式一致且容器头可解析，拒绝发生在 durable 写入之前；同时说明平台壳 `cover_media_type` 只负责读取侧 `Content-Type`、不参与写入校验。验证：`rg -n '容器头|写入校验|cover_media_type' docs/DESIGN.md` 命中该句，且不与既有 base64 载荷、档位语义冲突。
- [x] 5.2 复核 `docs/PRODUCT.md` / `docs/ROADMAP.md` 是否需要同步：本 change 不改变产品范围、阶段或界面术语，记录结论即可，不改根级指引。验证：`git diff --name-only` 中不含这两个文件与 `AGENT.md`；若结论相反，先修正规格再改文档。

## 6. 规格同步与 8 个场景登记（验证通过后才做）

- [x] 6.1 把 delta 的 `LD-12 自定义封面原件写入前必须确认内容真实性` 整段（含 8 个场景）**逐字**追加到 `openspec/specs/library-durability/spec.md` 的 `## Requirements` 末尾，作为 early-sync（归档时按幂等处理）。验证：`rg -c '^### Requirement: LD-12' openspec/specs/library-durability/spec.md` 输出 `1`；`node scripts/verify/spec-scenarios.mjs` 输出 147 个 Requirement / 572 个 Scenario，末 8 个 ID 为 `LD-R12-S01..S08`。
- [x] 6.2 在 `docs/traceability.md` 的 `## library-durability` 段末尾追加 8 行（`LD-R12-S01..S08`）：任务列写 `issue-132`，测试层写 `Core`，实际验收命令写 `pnpm verify:scenario -- <ID>`；文件头「当前基线」改为 147 个 Requirement / 572 个 Scenario。验证：`node scripts/verify/spec-scenarios.mjs --check` 输出 `ok: 572 ...`。
- [x] 6.3 在 `scripts/verify/scenario-commands.mjs` 增加 8 条映射，全部使用 `CORE_ALL`，并把顶部 `564 scenarios` 注释更新为 572：

  | 场景 | evidence（真实测试名） |
  |---|---|
  | `LD-R12-S01` | `validate_rejects_non_image_bytes_declared_as_png` |
  | `LD-R12-S02` | `validate_rejects_declared_mime_that_differs_from_detected_format` |
  | `LD-R12-S03` | `validate_rejects_truncated_or_malformed_container_header` |
  | `LD-R12-S04` | `validate_accepts_jpeg_png_and_webp_with_matching_mime` + `save_and_resolve_round_trip_all_three_formats` |
  | `LD-R12-S05` | `playlist_and_artist_cover_writes_share_the_same_validation` |
  | `LD-R12-S06` | `validate_reads_only_the_container_header_without_pixel_decoding` |
  | `LD-R12-S07` | `existing_size_and_mime_whitelist_rejections_are_unchanged` |
  | `LD-R12-S08` | `resolving_an_existing_invalid_original_reports_a_validation_error` + `legacy_cache_only_cover_is_unaffected` |

  验证：`node scripts/verify/validate-scenario-commands.mjs` 与 `node scripts/verify/check-scenario-evidence.mjs` 均退出 0；测试名与 2.2/2.3/3.3/3.4 实际落地的名称逐字一致，不得为了凑证据改名或新增同义用例。
- [x] 6.4 运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `scripts/verify/manifest.json`，再运行不带 `--write` 的漂移门与 `node scripts/verify/reconcile-scenarios.mjs`。验证：三条命令退出 0；manifest 含 572 条且 8 个新 ID 的 command / evidence 非空。
- [x] 6.5 复核 `docs/acceptance/PRD-matrix.md` 是否需要登记。**实施复核修正原任务前提**：`prd-matrix.mjs` 的 `A_FAMILIES` 不包含 `LD` 前缀，既有 `LD-R01..R11` 均不归属 A1–A14，因此本 change 不得追加 `LD` 归属行，摘要计数也不变；强行追加会被门禁判为「规格中不存在的场景行」。验证：`node scripts/verify/prd-matrix.mjs` 输出 `14 条验收路径与 572 个场景一致` 并退出 0；`rg -n '\bLD-R' docs/acceptance/PRD-matrix.md` 无输出；`git diff --name-only` 中不含该文件。
- [x] 6.6 运行 8 个新场景：`node scripts/verify/run-scenario.mjs LD-R12-S01 … LD-R12-S08`（不要用 `pnpm verify:scenario -- <id>`，`--` 会被当成 id）。**验证方式说明**：8 个场景共用同一 `CORE_ALL` 命令，runner 按命令去重只执行一次完整 Core 套件并逐场景核对 evidence，因此一次传入 8 个 ID 与逐条执行等价且避免 8 次重复全量测试；另单独执行 `LD-R12-S01` 验证单 ID 形式。验证：两条命令退出 0，8 个 `ok: scenario LD-R12-SNN` 均出现且各自 evidence 被观测到。

## 7. 交付门禁、归档与清理

- [x] 7.1 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-targets --all-features`。验证：三条命令均退出 0；无新增 `unsafe`、无新增源文件超 1000 行、无新增公开 trait 超 6 方法。
- [x] 7.2 **独占**运行 `pnpm verify:governance`（不并行跑 cargo、rg 或 node 扫描；注入套件有墙钟断言）。验证：整门退出 0；若失败，记录完整输出并区分本 change 引入的失败与既有 flake，不擅自放宽门禁。
- [x] 7.3 运行 `openspec validate validate-cover-image-content --strict`。验证：命令退出 0，proposal / specs / design / tasks 全部通过。
- [x] 7.4 运行 `openspec archive validate-cover-image-content --yes`（delta 已在 6.1 early-sync 进主规格，归档按幂等处理）。验证：`openspec list --changes` 不再列出本 change，`openspec/changes/archive/` 下存在本 change，且 `rg -c '^### Requirement: LD-12' openspec/specs/library-durability/spec.md` 仍输出 `1`。
- [x] 7.5 归档后复跑 `node scripts/verify/spec-scenarios.mjs --check`、`node scripts/verify/reconcile-scenarios.mjs`、`node scripts/verify/validate-scenario-commands.mjs`，并按 §9 用 `rg -n 'validate_for_save|CoverFormatProbe|cover_media_type\\(' crates/echo-core/src` 做残留引用检查。验证：三条命令退出 0；引用检查显示没有新增被废弃的替代实现、导出或测试替身，既有 `cover_media_type` 仅留在平台壳。

## 8. 交付

- [x] 8.1 在独立分支以中文提交并推送（例如 `fix(cover): 封面原文件写入前校验内容真实性 (#132)`），用中文 PR 关联 #132：运行 `git add`、`git commit`、`git push -u origin <分支名>`，再用 `gh pr create --base main --head <分支名> --title ... --body-file ...`。验证：`gh pr view --json url,title,body` 返回正确内容，PR 正文含变更摘要、失败复现与修复验证、门禁证据、`Closes #132`，并说明「容器头合法但像素数据损坏仍属展示阶段」这一残留边界；CI 运行后记录结果，失败只修本 change 引入的问题并如实报告既有 flake。
