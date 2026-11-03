# Tasks

## 1. 失败复现与基线

- [x] 1.1 记录当前解码能力与失败基线：运行 `cargo tree -p echo-core -e features -i image`，确认 `image` 只启用 `png`/`jpeg`；运行 `rg -n 'image = \\{ version = "0.25"' Cargo.toml` 与 `rg -n 'load_from_memory' crates/echo-core/src/infrastructure/metadata/cover.rs`，确认根因链路与 issue #134 一致。验证：输出保存到 `/tmp/echo-issue-134-baseline.txt`，包含特性列表与两处代码位置。
- [x] 1.2 生成真实 lossy WebP fixture：用 `ffmpeg -f lavfi -i testsrc=size=600x400:rate=1 -frames:v 1 /tmp/echo-cover-src.png` 生成源图，再用 `cwebp -q 80 /tmp/echo-cover-src.png -o crates/echo-core/src/infrastructure/metadata/fixtures/cover.webp` 写入 fixture；运行 `file crates/echo-core/src/infrastructure/metadata/fixtures/cover.webp` 与 `ffprobe -v error -show_entries stream=codec_name,width,height -of default=nw=1` 确认是 VP8 WebP（或等效 lossy WebP）且为 600×400。验证：两条命令输出保存到基线文件；fixture 同时加入 `crates/echo-core/src/infrastructure/metadata/mod.rs` 所在目录结构。
- [x] 1.3 先写失败测试 `cover_cache_thumbnail_decodes_webp_original`（`include_bytes!("fixtures/cover.webp")`，经 `DiskCoverCache::put` / `thumbnail` 请求 `list` 与 `detail`），运行 `cargo test -p echo-core --all-features cover_cache_thumbnail_decodes_webp_original`。验证：修复前测试失败，错误包含 `WebP is not supported`；把输出追加到基线文件，确认失败原因与 issue 日志一致。

## 2. 启用解码器

- [x] 2.1 修改 workspace `Cargo.toml` 的 `image` 依赖：保持 `default-features = false`，特性改为 `["png", "jpeg", "gif", "bmp", "tiff", "webp"]`。验证：`cargo tree -p echo-core -e features -i image` 显示六项特性；`git diff --stat Cargo.lock` 只新增 `image-webp`/`gif`/`tiff` 及其轻量传递依赖，没有 `avif`/`ravif`/`dav1d`。
- [x] 2.2 重新运行 `cargo test -p echo-core --all-features cover_cache_thumbnail_decodes_webp_original`，确认测试在未改动 `scale_to` 的情况下通过。验证：测试退出 0，`list` 与 `detail` 输出均为 JPEG magic（`FF D8 FF`）、最长边分别 ≤128/≤512，且 `thumb-128.bin`/`thumb-512.bin` 存在。

## 3. MIME 声明策略

- [x] 3.1 在 `crates/echo-core/src/infrastructure/metadata/tags/mod.rs` 实现 `mime_of` 规范化：已知 lofty 变体映射为规范 MIME；未知声明 `trim` + 小写后仅放行可解码集合（`image/png`、`image/jpeg`、`image/gif`、`image/bmp`、`image/tiff`、`image/webp`），其余返回 `application/octet-stream`。用常量集中声明可解码集合并写注释说明与 workspace `image` 特性的对应关系。验证：`rg -n "application/octet-stream|DECODABLE" crates/echo-core/src/infrastructure/metadata/tags/mod.rs` 显示策略实现。
- [x] 3.2 更新 `crates/echo-core/src/domain/media.rs` 中 `EmbeddedCover::mime` 的文档：说明它是“声明且可解码的封面 MIME；无法解码的声明降级为 `application/octet-stream`”。验证：`rg -n "声明且可解码|application/octet-stream" crates/echo-core/src/domain/media.rs` 命中。
- [x] 3.3 更新/新增 `tags/tests.rs`：
  - `mime_of_maps_every_variant_with_a_default_fallback` 断言 WebP（含大小写/空白变体）保留、`image/avif` 与任意字符串降级为 `application/octet-stream`；
  - `mime_of_outputs_stay_within_the_enabled_decoders` 枚举所有可能声明的图片 MIME，跳过 `application/octet-stream`，断言 `image::ImageFormat::from_mime_type` 存在且 `reading_enabled()` 为真。
  验证：`cargo test -p echo-core --all-features infrastructure::metadata::tags::tests` 全部通过；临时把 `webp` 从 `Cargo.toml` 移除时约束测试必须失败（验证后恢复），把失败输出记录到基线文件。

## 4. Rust 质量门

- [x] 4.1 运行 `cargo fmt --all -- --check`、`cargo clippy -p echo-core --all-targets --all-features -- -D warnings`、`cargo test -p echo-core --all-features`。验证：三条命令均退出 0；WebP、MIME 与既有封面/标签测试全部通过。
- [x] 4.2 运行 `cargo check --workspace --all-features`，确认依赖特性变化没有影响其它 crate。验证：命令退出 0，`Cargo.lock` 无意外新增。

## 5. 规格同步与场景登记

- [x] 5.1 把两个 delta 的要求按顺序同步进主规格末尾：`openspec/specs/desktop-app-shell/spec.md` 追加 `DAS-R24`「封面缩略图必须解码已声明的图片格式」，`openspec/specs/local-library/spec.md` 追加 `LL-R12`「内嵌封面 MIME 声明必须与解码能力一致」，内容与 change 的 delta **逐字一致**。验证：`rg -n 'DAS-R24|LL-R12' openspec/specs` 各命中一次，且 `node scripts/verify/spec-scenarios.mjs` 输出 146 个 Requirement / 564 个 Scenario。
- [x] 5.2 在 `docs/traceability.md` 追加 5 行：`DAS-R24-S01`、`DAS-R24-S02`、`LL-R12-S01..S03`，任务列写 `issue-134`，测试层写 `Core`，验收命令写对应 `pnpm verify:scenario -- <ID>`；并把文件头「当前基线」更新为 146 个 Requirement / 564 个 Scenario。验证：`node scripts/verify/spec-scenarios.mjs --check` 输出 `ok: 564 ...`。
- [x] 5.3 在 `scripts/verify/scenario-commands.mjs` 增加 5 条映射并更新顶部 `559 scenarios` 注释为 564：DAS 两条与 LL 三条均使用 `CORE_ALL`，evidence 分别指向 `cover_cache_thumbnail_decodes_webp_original`、`mime_of_uses_webp_declaration`、`mime_of_downgrades_unsupported_declaration`、`mime_of_outputs_stay_within_the_enabled_decoders`（按实际测试名落位，测试名必须与 3.3/1.3 一致）。验证：`node scripts/verify/validate-scenario-commands.mjs` 退出 0。
- [x] 5.4 运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `scripts/verify/manifest.json`，再运行不带 `--write` 的漂移门与 `node scripts/verify/reconcile-scenarios.mjs`。验证：三条命令均退出 0；manifest 含 564 条且 5 个新 ID 的 command/evidence 非空。
- [x] 5.5 更新 `docs/acceptance/PRD-matrix.md`：为对应验收路径追加 5 条归属行并更新摘要计数。验证：`node scripts/verify/prd-matrix.mjs` 与 564 个场景一致并退出 0。

## 6. 集成验证与归档

- [x] 6.1 逐条运行 5 个新场景：`node scripts/verify/run-scenario.mjs DAS-R24-S01`、`DAS-R24-S02`、`LL-R12-S01`、`LL-R12-S02`、`LL-R12-S03`（不要用 `pnpm verify:scenario -- <id>`）。验证：5 条命令均退出 0 且 evidence 被观测到。
- [x] 6.2 独占运行 `pnpm verify:governance`（不并行跑 cargo、rg 或 node 扫描）。验证：整门退出 0；失败时区分本 change 引入与既有 flake 并如实记录。
- [x] 6.3 运行 `openspec validate fix-webp-cover-thumbnails --strict`。验证：命令退出 0，proposal/specs/design/tasks 全部通过。
- [x] 6.4 运行 `openspec archive fix-webp-cover-thumbnails --yes`；确认 `openspec/changes/archive/` 下有本 change，且主规格中 `DAS-R24`/`LL-R12` 各只出现一次。验证：`rg -c '### Requirement: 封面缩略图必须解码已声明的图片格式' openspec/specs/desktop-app-shell/spec.md` 与对应 local-library 命令均输出 `1`。
- [x] 6.5 归档后复跑 `node scripts/verify/spec-scenarios.mjs --check`、`node scripts/verify/reconcile-scenarios.mjs`。验证：两条命令均退出 0。

## 7. 交付

- [ ] 7.1 在独立分支提交并推送（中文提交信息），用中文 PR 关联 #134：`git commit -m "fix(cover): 启用 WebP 等内嵌封面解码并锁定 MIME 策略 (#134)"`、`git push -u origin <分支名>`，再用 `gh pr create --base main --head <分支名> --title ... --body-file ...`，正文包含变更摘要、失败复现与修复验证、`Closes #134`。验证：`gh pr view --json url,title,body` 返回正确内容；CI 运行后记录结果，失败只修本 change 引入的问题并如实报告既有 flake。
