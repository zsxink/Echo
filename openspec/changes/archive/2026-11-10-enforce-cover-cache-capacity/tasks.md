# Tasks

## 1. 基线、分支与事实校核（不修改生产代码）

- [x] 1.1 确认 issue #126 仍为 OPEN（`gh issue view 126 --repo zsxink/Echo --json number,title,state`），并确认已在**任何生产代码改动之前**从 `main` 创建独立分支 `fix/issue-126-cover-cache-capacity`；禁止直接向 `main` 提交或推送。验证：`git branch --show-current` 输出该分支名，`git status --short` 只有本 change 的规划产物。
- [x] 1.2 按 `design.md` D7 重跑事实校核：`rg -n 'fn gc|stored_bytes|max_capacity|thumb-' crates/echo-core/src/infrastructure/metadata/cover.rs`、`rg -n 'cover_cache.gc' crates/echo-desktop/src/runtime/app.rs`、`rg -n 'trait CoverCache' crates/echo-core/src/application/ports/media.rs`。验证：pass 2 仍只写 debug 日志、生产 GC 只在启动装配调用一次、trait 仍为 4 个方法；与 proposal「事实校核」不符时先修正规格再继续。
- [x] 1.3 记录治理基线：`wc -l crates/echo-core/src/infrastructure/metadata/cover.rs`、`node scripts/verify/check-scale.mjs`、`node scripts/verify/spec-scenarios.mjs --check`、`node scripts/verify/reconcile-scenarios.mjs`、`node scripts/verify/prd-matrix.mjs`。验证：五条命令退出 0，确认新能力 `cover-cache-capacity` 不在 `AREA_PREFIX`、不影响 603 条 0.1.0 场景集合；此时 `git diff --name-only` 不含生产代码。

## 2. 复现测试先行（红）

- [x] 2.1 在 `cover.rs` 既有 `#[cfg(test)] mod tests` 内补充可复现 issue #126 的测试（后续按 design D6 迁入同目录 `cover_capacity_tests.rs`，测试名与断言不变）：用 `rgba_png` 生成两档缩略图，把 `with_capacity` 设为「原图 + mime.txt 之和 + 少量余量」，断言生成缩略图后 `stored_bytes() > cap`、执行 `gc(&[key])` 后 `stored_bytes() <= cap`，且 `original()` 仍返回原始字节。验证：`cargo test -p echo-core --all-features cover_cache_capacity -- --nocapture` **以非零退出**并把失败输出记录为 #126 的复现证据（当前 pass 2 只记日志）。
- [x] 2.2 同组补一条「原图自身已超上限」的复现测试，断言缩略图仍保留且 `gc` 成功返回；保留既有 `cover_cache_gc_over_capacity_retains_referenced_assets` 作为引用完整性基线并确认其当前通过。验证：`cargo test -p echo-core --all-features cover_cache_gc_over_capacity_retains_referenced_assets -- --nocapture` 退出 0；2.1 的红测试仍可单独复现。

## 3. 容量回收实现、场景测试与文档同步

- [x] 3.1 在 `cover.rs` 实现 `gc()` pass 2 的候选收集与不可达标判定（design D1 / D3）：仅收集被引用条目下 `is_file()` 且名为 `thumb-512.bin` / `thumb-128.bin` 的候选，`total - reclaimable > max_capacity` 时不删除任何文件、写结构化 `tracing::warn!` 并返回 `Ok(())`；`original.bin` / `mime.txt` / `*.tmp` / 未知文件名一律不进候选。验证：2.1 / 2.2 两条测试转绿，`cargo test -p echo-core --all-features cover_cache_capacity` 退出 0。
- [x] 3.2 实现确定性淘汰与错误语义（design D2 / D4）：按 `(512 档优先, 文件占用降序, hash 升序)` 排序，逐个 `remove_file` 并累减 `total`，达标即停；`NotFound` 视为已删除，其它 I/O 错误写结构化 `warn!` 并继续下一个候选；成功回收后写 `debug!`（`total` / `capacity` / `evicted` / `reclaimed`，不含用户路径）。增加 `#[cfg(test)]` 的单次删除失败注入开关（与既有 `fail_thumbnail_write` 同形，生产零开销）。验证：`cargo test -p echo-core --all-features cover_cache_capacity` 退出 0，`rg -n 'remove_dir_all|remove_file' crates/echo-core/src/infrastructure/metadata/cover.rs` 显示 pass 1 未改动。
- [x] 3.3 按 design D6 把 2.1 / 2.2 的复现测试迁入同目录 `cover_capacity_tests.rs`（`#[path]` 兄弟模块），并在该模块内补齐 delta spec 的全部场景测试（共 12 条）：详情档优先于列表档、达标即停、临时文件与原图不被误删、单个缩略图删除失败仍继续、未引用条目整目录清理、畸形 key 不改变结果、连续两次 GC 不反复清空、淘汰后按需重建并复用、并发重建只解码一次、重建失败不留可读半成品。验证：`cargo test -p echo-core --all-features cover_cache` 全部通过，且 `cover_cache_gc_over_capacity_retains_referenced_assets`、`cover_cache_thumbnail_write_failure_leaves_no_partial_file`、`cover_cache_concurrent_thumbnail_generation_decodes_once` 三条既有回归用例未被改写或删除。
- [x] 3.4 同步失效表述与项目文档：修正 `cover.rs` 模块头注释（当前写「above the capacity limit, only ever touches *unreferenced* entries」）与 pass 2 注释，明确「被引用条目内的可重建缩略图参与容量回收、原图永不删除」；在 `docs/DESIGN.md` §2.4 补一句容量回收职责，不改写既有条目。验证：`rg -n '容量|capacity|缩略图' docs/DESIGN.md | head -20` 命中新增一句且 §2.4 既有条目不变；`rg -n 'only ever touches' crates/echo-core/src/infrastructure/metadata/cover.rs` 无输出。
- [x] 3.5 本组静态门禁：`cargo fmt --all -- --check`、`cargo clippy -p echo-core --all-targets --all-features -- -D warnings`、`cargo test -p echo-core --all-features`、`node scripts/verify/check-scale.mjs`（确认 `cover.rs` 与 `cover_capacity_tests.rs` 均 < 1000 行）。验证：四条命令均退出 0。

## 4. 规格同步与集成验证

- [x] 4.1 early-sync：把 `specs/cover-cache-capacity/spec.md` 的 4 个 Requirement 与全部 Scenario **逐字**同步为 `openspec/specs/cover-cache-capacity/spec.md`（含 `## Purpose`），不改动任何既有 capability spec。验证：`diff <(tail -n +1 openspec/changes/enforce-cover-cache-capacity/specs/cover-cache-capacity/spec.md) <(tail -n +1 openspec/specs/cover-cache-capacity/spec.md)` 无输出（忽略首行 `# Spec Delta` 与 `# cover-cache-capacity Specification` 的差异时逐条核对），`rg -c '^### Requirement:' openspec/specs/cover-cache-capacity/spec.md` 输出 `4`。
- [x] 4.2 三方对账与规格门禁：`node scripts/verify/spec-scenarios.mjs --check`、`node scripts/verify/reconcile-scenarios.mjs`、`node scripts/verify/prd-matrix.mjs`、`node scripts/verify/validate-scenario-commands.mjs`、`openspec validate enforce-cover-cache-capacity --strict`。验证：五条命令均退出 0，且 `spec-scenarios.mjs --check` 仍输出 603（新能力不进入 0.1.0 场景清单）。
- [x] 4.3 全量静态与回归门禁：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-targets --all-features`、`node scripts/verify/check-scale.mjs`。验证：四条命令均退出 0；`git status --short` 中不含生成物、锁文件或其它无关文件。
- [x] 4.4 **独占**运行 `pnpm verify:governance`（不与 cargo / node / rg 扫描并行，避免 5 秒墙钟断言被干扰）。验证：整门退出 0、注入自证全部通过；失败只修本 change 引入的问题并如实报告既有 flake。

## 5. 归档与交付

- [x] 5.1 确认 1–4 全部勾选、`openspec status --change enforce-cover-cache-capacity` 显示任务完成、delta 已 early-sync。验证：`openspec validate enforce-cover-cache-capacity --strict` 退出 0。
- [x] 5.2 运行 `openspec archive enforce-cover-cache-capacity --yes`，确认归档目录出现且主规格保留 `cover-cache-capacity` 的 4 个 Requirement。验证：`openspec list --changes` 输出符合预期、`rg -c '^### Requirement:' openspec/specs/cover-cache-capacity/spec.md` 输出 `4`；归档后复跑 `node scripts/verify/spec-scenarios.mjs --check`、`node scripts/verify/reconcile-scenarios.mjs` 与 `pnpm verify:governance`（独占）均退出 0。
- [x] 5.3 在独立分支上以中文提交并推送（例如 `fix(cover): 封面缓存超限时回收可重建缩略图 (#126)`）。验证：`git status --short` 干净、`git branch --show-current` 为 `fix/issue-126-cover-cache-capacity`、`git log -1 --oneline` 符合预期，且未直接向 `main` 推送。
- [x] 5.4 用中文 PR 关联 #126：`gh pr create --base main --head fix/issue-126-cover-cache-capacity --title ... --body-file ...`，正文包含变更摘要、复现测试的红/绿证据、验证命令与结果、兼容性说明、`Closes #126`。验证：`gh pr view --json url,title,body,state` 返回正确内容，PR 未合并到 `main`（由维护者审阅合并）。
