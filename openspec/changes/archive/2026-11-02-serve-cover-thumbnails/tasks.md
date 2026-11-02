# Tasks

## 1. 基线与契约确认

- [x] 1.1 在**任何代码改动之前**，从当前 HEAD 运行 `pnpm echo dev` 构建并启动应用，准备一个至少 24 张不同封面（最好 500+ 首歌）的资料库；记录 macOS `Echo Web Content` 进程的 `footprint -p <pid>` 基线、首访与滚动观察，保存到 `/tmp/echo-cover-baseline.txt` 供最终报告引用。验证：文件中同时包含 `footprint` 原始输出与观察结论；若 GUI 无法启动，记录具体命令和失败原因，不填数字。
- [x] 1.2 用 `rg -n 'impl CoverCache for' -g '*.rs'` 与 `rg -n --glob '!**/*.test.*' --glob '!**/e2e/**' 'assetUrl\(' apps/desktop/src` 复核只有 `DiskCoverCache` / `MemoryCoverCache` 两个实现者、9 个生产调用点，与 `design.md` 的归位表一致。验证：命令输出与设计表逐项一致；若不一致，先更新设计再继续。

## 2. Core 封面缓存

- [x] 2.1 在 `crates/echo-core/src/application/ports/media.rs` 的 `CoverCache` 上新增 `thumbnail(&self, asset_key: &str, detail: bool) -> Result<Option<Vec<u8>>, Error>`，写清 `detail=false → 128px`、`true → 512px`、未知 key → `Ok(None)`、畸形 key → 校验错误、损坏原图 → `corrupt_media`。`DiskCoverCache` 保留现有固有方法并委托私有实现，trait 实现也委托同一实现；`MemoryCoverCache` 返回已存字节。验证：`cargo check -p echo-core --all-features` 通过，且 `rg -n 'fn thumbnail'` 同时覆盖 trait、两个实现与私有实现。
- [x] 2.2 把 `DiskCoverCache::thumbnail` 的未知 key 行为从 `Err(unavailable)` 改为 `Ok(None)`，更新 `cover_cache_thumbnail_missing_and_corrupt_originals`；损坏原图仍断言 `corrupt_media`。验证：`cargo test -p echo-core --all-features infrastructure::metadata::cover::tests::cover_cache_thumbnail_missing_and_corrupt_originals` 通过。
- [x] 2.3 给 `DiskCoverCache` 加 `inflight: Arc<Mutex<HashMap<(String, bool), Arc<Mutex<()>>>>>` 与 `#[cfg(test)]` 解码计数；`thumbnail` 先读盘，未命中再取 `(hash, detail)` 闸门，拿锁后重查磁盘，成功生成后原子写，释放并移除闸门。验证：`cargo test -p echo-core --all-features infrastructure::metadata::cover::tests` 全部通过；`rg -n 'inflight|write_atomically' crates/echo-core/src/infrastructure/metadata/cover.rs` 显示闸门与原子写都在 `thumbnail` 路径上。
- [x] 2.4 新增测试：`cover_cache_concurrent_thumbnail_generation_decodes_once`（`Barrier` 启动 8 个线程请求同一 key/档位，解码计数必须为 1，所有返回字节一致）、`cover_cache_thumbnail_is_generated_on_demand_and_reused`（首次生成 `thumb-128.bin`，第二次不再解码）、`cover_cache_thumbnail_tiers_are_independent`（128 与 512 各自存在且尺寸正确）。验证：逐条 `cargo test -p echo-core --all-features <测试名>` 通过。
- [x] 2.5 新增测试：`cover_cache_thumbnail_write_failure_leaves_no_partial_file`（用不可写/临时目录或注入失败，断言没有可读的截断缩略图）、`cover_cache_thumbnail_drops_alpha_and_keeps_jpeg`（透明 PNG 生成 JPEG，解码后 `ColorType::Rgb8`，记录既有 alpha 丢弃行为）。验证：两条测试通过。
- [x] 2.6 新增 `small_fakes.rs` 的 `thumbnail` 测试：未知 key 返回 `Ok(None)`，已存 key 两档都返回已存字节。验证：`cargo test -p echo-core --all-features application::testkit::small_fakes::tests` 通过。
- [x] 2.7 运行 Core 静态门禁：`cargo fmt --all -- --check`、`cargo clippy -p echo-core --all-targets --all-features -- -D warnings`、`cargo test -p echo-core --all-features`。验证：三条命令均退出 0。

## 3. 协议解析与安全边界

- [x] 3.1 在 `crates/echo-desktop/src/platform/security.rs` 增加 `CoverSize { List, Detail, Original }` 与 `CoverRequest { asset_key, size }`；把 `parse` 改为返回 `Result<CoverRequest, CoverError>`：先剥离 scheme，截 fragment、切 query 并解析 `s`，再用剩余 path 走既有 key 校验。缺省/未知/空/重复 `s` 取第一个或回落 `List`；`s` 不进入 `is_valid_key`、长度或路径。验证：`cargo test -p echo-desktop --all-features platform::security::tests` 通过。
- [x] 3.2 更新并新增解析测试：`cover_uri_parses_size_selector_before_stripping_query`（`list`/`detail`/`original`、缺省、未知、空值、重复、fragment）、`cover_uri_defaults_to_list_for_unknown_or_missing_size`、`cover_uri_size_selector_cannot_bypass_key_validation`（`?s=../../etc/passwd` 仍接受 key 但回落 list；`cover://asset/../../etc/passwd?s=detail` 仍 `InvalidKey`；超长 key 仍 `InvalidKey`；多段 key 仍拒绝）。验证：逐条 `cargo test -p echo-desktop --all-features <测试名>` 通过。
- [x] 3.3 更新 `CoverProtocol` 与模块文档：删除“size enum 由前端负责、永不进入文件系统”的旧句子，改为“协议解析并 allowlist 档位；档位只选择缓存读取路径，不参与 key 校验或路径构造”。验证：`rg -n 'size enum is the frontend|尺寸枚举.*前端' crates/echo-desktop/src/platform/security.rs` 无输出，且 `rg -n 'CoverSize|CoverRequest'` 显示新类型与文档。
- [x] 3.4 更新 `scripts/verify/checks/task-12.7.mjs` 的 `DESKTOP_FILTERS`，加入新的档位安全测试；确认既有 `cover_uri_rejects_path_traversal_and_absolute_paths`、CSP/capability 漂移测试仍在列表。验证：`node scripts/verify/checks/task-12.7.mjs` 退出 0，输出包含新增测试的 `ok:` 行。
- [x] 3.5 运行 `cargo fmt --all -- --check`、`cargo clippy -p echo-desktop --all-targets --all-features -- -D warnings`、`cargo test -p echo-desktop --all-features`。验证：三条命令均退出 0。

## 4. 桌面壳协议处理器

- [x] 4.1 在 `apps/desktop/src-tauri/src/cover_protocol.rs` 抽出纯函数：`cover_bytes(&dyn CoverCache, &CoverRequest) -> Result<Option<Vec<u8>>, Error>`（`Original → get`，`List → thumbnail(false)`，`Detail → thumbnail(true)`）与 `cover_response(bytes, origin) -> HttpResponse<Vec<u8>>`（Content-Type、Vary、Cache-Control、ACAO 白名单）。验证：`cargo test -p echo-app cover_protocol::tests` 通过。
- [x] 4.2 新增测试：`cover_bytes_dispatches_original_and_thumbnail_tiers`（记录型 fake 断言调用 `get`/`thumbnail(false)`/`thumbnail(true)`）、`cover_responses_are_immutable_and_keep_cors_vary`（断言 `Cache-Control: private, max-age=31536000, immutable`、`Vary: Origin`，允许的 Origin 有 ACAO、拒绝的没有）。验证：两条测试通过。
- [x] 4.3 把 `bootstrap.rs` 的 `register_uri_scheme_protocol` 改为 `register_asynchronous_uri_scheme_protocol`；处理器解析请求、取 `Arc<dyn CoverCache>` 与 Origin 后，用 `tauri::async_runtime::spawn_blocking` 调用 `cover_bytes` 并 `responder.respond`；解析失败/状态缺失仍在同步前缀立即 404。验证：`cargo check -p echo-app --all-features` 通过；`rg -n 'register_asynchronous_uri_scheme_protocol|spawn_blocking' apps/desktop/src-tauri/src` 显示两处都在 cover 路径上。
- [x] 4.4 为 `cover_protocol_handler` 保留 404/400 语义并添加可测的辅助函数测试：未知 key / `thumbnail` 返回 `Ok(None)` / `Err` 都映射 404；畸形 key 不进入 `spawn_blocking`。验证：`cargo test -p echo-app --all-features cover_protocol::tests` 通过。
- [x] 4.5 运行 `cargo fmt --all -- --check`、`cargo clippy -p echo-app --all-targets --all-features -- -D warnings`、`cargo test -p echo-app --all-features`。验证：三条命令均退出 0。

## 5. 前端档位与调用点

- [x] 5.1 修改 `apps/desktop/src/bridge/index.ts`：导出 `CoverSize = "list" | "detail" | "original"`，`assetUrl(key, size = "list")` 在 `convertFileSrc` 之后追加 `?s=`；仅对 `cover://`、`http://cover.localhost/`、`https://cover.localhost/` 追加，`data:`/`blob:` 原样返回。验证：新建 `apps/desktop/src/bridge/assetUrl.test.ts` 覆盖默认 list、显式 detail/original、Windows 基址、data/blob 跳过，以及 mock `convertFileSrc` 模拟 `encodeURIComponent` 后 query 未被编码；`pnpm --dir apps/desktop exec vitest run src/bridge/assetUrl.test.ts` 通过。
- [x] 5.2 按 `design.md` D8 表逐一归位 9 个生产调用点：`ImmersivePlayer.tsx:193` → `original`；`PlaylistNameDialog.tsx:274` 的已保存封面分支 → `detail`；其余 7 处 → `list`。验证：`rg -n --glob '!**/*.test.*' --glob '!**/e2e/**' 'assetUrl\(' apps/desktop/src` 输出 9 处且每处都带档位参数。
- [x] 5.3 更新前端模块 mock 与断言：`ImmersivePlayer.test.tsx`、`QueuePanel.test.tsx`、`PlaylistsView.test.tsx` 的 `assetUrl` mock 接收 `size`；`ImmersivePlayer.test.tsx:206` 断言 `?s=original`；`PlayerBar.test.tsx:272/291`、`QueuePanel.test.tsx:114`、`SongRow.test.tsx:190` 断言 `?s=list`；为 `PlaylistsView.test.tsx` 的歌单卡、`CollectionDirectory.test.tsx` 的目录卡、`SongMenu.test.tsx` 的菜单缩略图补 `?s=list` 断言。验证：受影响测试文件全部通过。
- [x] 5.4 为 `PlaylistNameDialog.test.tsx` 增加“已保存封面走 detail 档”用例（mock `bridge.assetUrl` 并传 `coverPreview="cv1-..."`），断言 `?s=detail`；确认 blob 分支不调用 `assetUrl`。验证：`pnpm --dir apps/desktop exec vitest run src/features/playlists/PlaylistNameDialog.test.tsx` 通过。
- [x] 5.5 确认 `ImmersivePlayer` 的 `artwork` URL 继续同时传给 `<img>` 与 `useArtworkTint`，`artworkPalette` 的 `crossOrigin="anonymous"` 不变；增加测试断言主色提取收到的 URL 与 `?s=original` 的封面 URL 相同。验证：`pnpm --dir apps/desktop exec vitest run src/features/player/ImmersivePlayer.test.tsx src/features/player/useArtworkTint.test.tsx` 通过。
- [x] 5.6 更新 `apps/desktop/src/test/setup.ts` 与 `apps/desktop/e2e/mock-bridge.ts` 的注释/行为说明：测试基址返回 `cover://<key>` 时由 `assetUrl` 追加 query；e2e 的 data URI 不追加。验证：`pnpm --dir apps/desktop test` 全量通过，`pnpm --dir apps/desktop test:e2e` 中封面相关用例仍渲染。
- [x] 5.7 运行 `pnpm --dir apps/desktop typecheck`、`pnpm --dir apps/desktop lint`、`pnpm --dir apps/desktop test`、`pnpm --dir apps/desktop build`。验证：四条命令均退出 0；构建产物中的前端与提交的 `dist` 一致（`pnpm verify:frontend-fresh` 通过）。

## 6. 文档、追踪与场景注册

- [x] 6.1 在 `docs/DESIGN.md` 第 2 节新增 `### 2.4 封面资产协议`：写明 `s=list|detail|original`、默认 `list`、档位不参与 key 校验/长度/路径、按需生成与不可变缓存头；不修改归档 change 的 §16 原文。验证：`rg -n '2.4 封面资产协议|s=list\\|detail\\|original|默认 list' docs/DESIGN.md` 命中且内容完整。
- [x] 6.2 更新 `crates/echo-desktop/src/platform/security.rs` 模块文档、`apps/desktop/src/bridge/index.ts:227` 注释、`scripts/verify/checks/task-12.7.mjs:5` 的 `design §16` 引用，指向新的 DAS 要求与 `docs/DESIGN.md` 2.4；确认不再有“尺寸枚举只归前端”的旧表述。验证：`rg -n 'size enum is the frontend|尺寸枚举.*前端|design §16' crates/echo-desktop/src/platform/security.rs apps/desktop/src/bridge/index.ts scripts/verify/checks/task-12.7.mjs` 无旧语义输出。
- [x] 6.3 在 `docs/traceability.md` 的 `## desktop-app-shell` 末尾追加 11 行：`DAS-R20-S01..S08`（8 条档位/缓存场景）与 `DAS-R21-S01..S03`（3 条非阻塞/单飞场景），任务列写 `issue-111`，并为每条填可执行命令。同步把文件头“当前基线”改为改动后的实际 tracked 计数（当前 tracked 136 Requirement / 523 Scenario，改动后 138 / 534；若计数再次漂移，以 `spec-scenarios.mjs` 的输出为准）。验证：`node scripts/verify/spec-scenarios.mjs --check` 输出 `ok: 534 ...`。
- [x] 6.4 在 `scripts/verify/scenario-commands.mjs` 增加 11 条映射并更新顶部“523 scenarios”注释：R20-S01 用 `VITEST_ALL`（SongRow/PlayerBar/QueuePanel/PlaylistsView/AddToPlaylistDialog/CollectionDirectory/SongMenu 的测试文件），S02 用 `VITEST_ALL`（PlaylistNameDialog），S03 用 `VITEST_ALL`（ImmersivePlayer），S04/S05 用 `DESK_ALL`（协议测试），S06 用 `APP_ALL`（响应头测试），S07/S08 用 `CORE_ALL`（缓存测试），R21-S01 用 `ATTEST("DAS-R21-S01")`，S02/S03 用 `CORE_ALL`（并发/失败测试）。每条 `evidence` 必须指向实际存在的测试名或文件。验证：`node scripts/verify/validate-scenario-commands.mjs` 退出 0。
- [x] 6.5 运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `manifest.json`，再运行不带 `--write` 的 drift gate。验证：两次命令均退出 0；`manifest.json` 含 534 条且 11 个新 ID 的 command/evidence 非空。
- [x] 6.6 更新 `docs/acceptance/PRD-matrix.md`：为 A1/A12/A13/A14 各追加 11 条 DAS 归属行（共 44 行），并把四个摘要行的“证明场景数”各加 11。验证：`node scripts/verify/prd-matrix.mjs` 输出“14 条验收路径与 534 个场景一致”，退出 0。
- [x] 6.7 运行 `node scripts/verify/reconcile-scenarios.mjs`，确认 specs、traceability、manifest 三方 ID 集合相等。验证：命令退出 0。
- [x] 6.8 在 `docs/native-attestation-playbook.md` 的 §5/§6 登记 `DAS-R21-S01` 的人工测量步骤与记录字段；创建 `artifacts/native-attestations/DAS-R21-S01.log`，填写 `os`、`versions`、`desktop-env`、`operator`、`result`、`evidence-path`、`date` 与首访/滚动测量数据。验证：`node scripts/verify/checks/check-native-attestation.mjs DAS-R21-S01` 退出 0；若 GUI 无法运行，停止并报告原因，不写通过记录。
- [x] 6.9 为 `gc()` 容量语义另立 GitHub issue（标题说明缩略图增大 `stored_bytes`、当前只记日志），在 issue 正文引用本 change 与 `cover.rs` 的 pass 2；在 PR 中链接。验证：`gh issue view <新 issue 编号> --json title,body` 返回正确内容。

## 7. 集成验证与交付

- [x] 7.1 逐条运行 11 个新场景：`node scripts/verify/run-scenario.mjs DAS-R20-S01` … `DAS-R21-S03`（不要用 `pnpm verify:scenario -- <id>`，`--` 会被当成 id）。验证：11 条命令均退出 0；`DAS-R21-S01` 的 attestation 记录完整。
- [x] 7.2 独占运行 `pnpm verify:governance`（不并行跑 cargo、rg 或 node 扫描；5 秒墙钟断言会被干扰）。验证：整门退出 0；若有失败，记录完整输出并区分本 change 引入的失败与既有 flake。
- [x] 7.3 在真实应用缓存目录验证：`find <cache>/ -name 'thumb-*.bin' | wc -l` > 0；对 `thumb-128.bin` / `thumb-512.bin` 执行 `file`，确认 JPEG 最长边 ≤128 / ≤512；打开歌单编辑对话框确认预览为 512 档。验证：把命令与输出保存到 `/tmp/echo-cover-evidence.txt`；计数器为 0 或尺寸超限即失败。
- [x] 7.4 打开沉浸页，确认封面请求 URL 带 `s=original`、黑胶封面与主色与改动前截图一致；用同一歌曲的改动前/后截图对比（或像素探针）记录结果。验证：截图与 URL 证据保存到 `/tmp/echo-cover-immersive/`；主色提取仍成功（无回退到主题色日志）。
- [x] 7.5 在真实应用上重复 1.1 的资料库场景，记录 `Echo Web Content` 的 `footprint -p <pid>`；与基线对比并写入 `/tmp/echo-cover-after.txt`。验证：前后两次命令、PID、输出与差值都在文件中；若无法测量，写明具体阻塞原因，不编造数字。
- [x] 7.6 在 500+ 首歌、24 行可见列表上首访并滚动，记录是否掉帧、协议日志中的缩略图生成耗时与主线程表现；把结论与原始数据写入 attestation 记录和最终报告。验证：报告包含“可感知卡顿：有/无”的结论及对应证据；不接受“应该/大概”。
- [ ] 7.7 对照 issue #111「三、验收」逐项勾选：缩略图存在、列表/卡片/队列 ≤128、歌单预览 512、沉浸页原图且截图一致、macOS 内存对比、治理门与受影响测试、首访/滚动性能；每项写明验证命令与结果。验证：最终报告中的清单每项都有命令/输出链接或明确的环境阻塞说明。
- [x] 7.8 运行 `openspec validate serve-cover-thumbnails --strict`，确认 proposal/specs/design/tasks 全部通过。验证：命令退出 0。
- [x] 7.9 运行 `openspec archive serve-cover-thumbnails --yes`（该命令把 delta 同步进主 specs 并归档）；确认 `openspec/specs/desktop-app-shell/spec.md` 末尾出现 R20/R21 与 11 个场景，`openspec/changes/archive/` 下有本 change。验证：`rg -n 'DAS-R20|DAS-R21|封面资产协议' openspec/specs/desktop-app-shell/spec.md` 与 `openspec list --changes` 输出符合预期。
- [ ] 7.10 提交并推送分支（中文提交信息），用中文 PR 关联 #111：运行 `git add`、`git commit -m "feat(cover): 按显示档位提供封面缩略图 (#111)"`、`git push -u origin codex/issue-111-cover-thumbnails`，再用 `gh pr create --base main --head codex/issue-111-cover-thumbnails --title ... --body-file ...`，正文包含变更摘要、验证证据、`Closes #111`、后续 `gc` issue 链接。验证：`gh pr view --json url,title,body` 返回正确内容；CI 运行后记录结果，失败只修本 change 引入的问题并如实报告既有 flake。
