# Tasks

## 1. 基线与契约确认

- [x] 1.1 在**任何代码改动之前**记录选封面路径的量化基线：(a) 用 `node -e "const b=Buffer.alloc(5*1024*1024);console.log(JSON.stringify(Array.from(b)).length, Buffer.from(b).toString('base64').length)"` 记录 5 MiB 载荷在 `number[]` 与 base64 两种编码下的 JSON 文本长度；(b) 在真实应用里选一张约 5 MB 的封面，用 `footprint -p <Echo Web Content pid>` 记录渲染进程峰值。验证：两组数字写入 `/tmp/echo-cover-baseline.txt`；若 GUI 无法启动，写明具体命令与失败原因，不填数字。
- [x] 1.2 复核现状链路与 `design.md` 一致：`rg -n 'Array.from|createObjectURL|revokeObjectURL' apps/desktop/src` 应命中 `PlaylistNameDialog.tsx` 的 1 处 `createObjectURL` 与 0 处 `revokeObjectURL`；`rg -n 'bytes: Option<Vec<u8>>' apps/desktop/src-tauri/src/commands.rs` 应命中两个封面命令；`rg -n 'bytes: number\[\] \| null' apps/desktop/src/bridge/index.ts` 应命中两处。验证：三条命令的输出与 design 的现状描述逐项一致；不一致先修正 design 再继续。
- [x] 1.3 确认两个前置事实：(a) `crates/echo-desktop/src/ipc/generate.rs` 不产出 `set_playlist_cover` / `set_artist_cover` 的参数类型——`rg -n 'set_playlist_cover|set_artist_cover' crates/echo-desktop/src/ipc/generate.rs` 只应命中返回类型行；(b) `apps/desktop/src-tauri/src/commands.rs` 当前行数接近 1000 行硬上限（`wc -l`）。验证：两条命令的输出被记录在案；据此确认解码逻辑必须另立模块（design D2）。
- [x] 1.4 确认与进行中 change 的边界：`rg -n 'PM-R13|PM-R14' openspec/changes/align-immersive-player-and-playlist-directory/` 应显示其硬编码引用；本 change **不得**修改 `openspec/specs/playlist-management/spec.md`。验证：记录输出，且 `git diff --name-only` 中不出现 `openspec/specs/playlist-management/spec.md`。

## 2. Rust 侧封面载荷解码

- [x] 2.1 在 `apps/desktop/src-tauri/Cargo.toml` 增加 `base64 = "0.22"` 直接依赖（仅 `echo-app` 使用），运行 `cargo build -p echo-app` 后确认 `Cargo.lock` 中 `base64 0.22.1` 仍在且**没有新增任何传递依赖**（对比改动前后的 `Cargo.lock` diff 只多出 `echo-app → base64` 一条边）。验证：diff 输出符合预期；`cargo tree -p echo-app -i base64` 显示 `base64` 已是既有依赖。
- [x] 2.2 新增 `apps/desktop/src-tauri/src/cover_payload.rs`，实现 `pub(crate) fn decode_cover_bytes(payload: Option<&str>) -> Result<Option<Vec<u8>>, IpcErrorDto>`：按 design D6 先用编码长度上界 `4 * ⌈5 242 880 / 3⌉ = 6 990 508`（`(5 * 1024 * 1024 + 2) / 3 * 4`）预检再解码；`None` 映射为「清除封面」；`Some("")` 与非法 base64 映射为 `Error::validation(ValidationSubject::Other, "cover", …)` 经 `IpcErrorDto::from_mutation` 转出。在 `main.rs` 声明 `mod cover_payload;`。验证：`cargo check -p echo-app --all-features` 通过；`rg -n '6 990 508' apps/desktop/src-tauri/src/cover_payload.rs` 命中，`rg -n '6 999 508' apps/desktop/src-tauri/src` 无输出。
- [x] 2.3 在 `cover_payload.rs` 写单元测试 `cover_payload::tests::`：`decodes_valid_base64_to_original_bytes`、`rejects_malformed_base64_as_a_validation_error`、`rejects_payload_longer_than_the_encoded_limit_before_decoding`（用 `6 990 508` 的边界值上下各一条，证明边界恰好可解码、边界 +1 被拒）、`treats_an_absent_or_null_payload_as_clearing_the_cover`（缺键与显式 `null` 都是 `None` ⇒ 清除，与现状语义一致；这是参数名**不改**的直接结果，见 design D5）、`rejects_an_empty_payload_string`。另加 `both_cover_commands_decode_through_the_same_helper`：用函数指针类型断言两个命令的参数都是 `Option<String>`，并对同一载荷调用 `decode_cover_bytes` 得到相同字节——证明共用同一条解码路径，而不只是签名形状相同。验证：`cargo test -p echo-app --all-features cover_payload::tests` 全部通过。
- [x] 2.4 改造 `apps/desktop/src-tauri/src/commands.rs` 的两个封面命令：参数类型由 `bytes: Option<Vec<u8>>` 收窄为 `bytes: Option<String>`（**参数名保持 `bytes` 不变**，design D5：改名会让旧键缺失被当成 `None` ⇒ 清除封面），并在调用服务前用 2.2 的解码函数转换；服务层签名 `Option<&[u8]>` 不动。验证：`rg -n 'bytes: Option<String>|decode_cover_bytes' apps/desktop/src-tauri/src/commands.rs` 显示两个命令都接入解码；`rg -n 'bytes_base64|bytesBase64' apps/desktop/src` 无输出；`cargo check -p echo-app --all-features` 通过。
- [x] 2.5 在 `crates/echo-desktop/src/runtime/services/tests/playlist_cover.rs` 补两条服务层回归测试 `playlist_cover_rejects_unsupported_mime_without_replacing_the_saved_cover` 与 `playlist_cover_rejects_oversized_payload_without_replacing_the_saved_cover`：先保存一张合法封面，再提交不受支持的 MIME / 超过 5 MiB 的载荷，断言返回校验错误且**已保存封面未被替换**。这是「两层校验保留」的回归基线，Core 本身不改动。前者映射到 `DAS-R22-S05`；后者是防止 Core 校验被削弱的回归护栏，**不单独映射场景**（`DAS-R22-S04` 只覆盖前端选图阶段的上限，后端超限拒绝没有独立规格场景，此处如实记录，不为凑覆盖硬塞证据）。验证：`cargo test -p echo-desktop --all-features playlist_cover` 两条新用例通过。
- [x] 2.6 运行 Rust 静态门禁：`cargo fmt --all -- --check`、`cargo clippy -p echo-app --all-targets --all-features -- -D warnings`、`cargo test -p echo-app --all-features`、`cargo test -p echo-core --all-features`。验证：四条命令均退出 0。

## 3. 前端载荷编码与状态分层

- [x] 3.1 修改 `apps/desktop/src/bridge/index.ts` 的 `BridgeCommandArguments`：`set_playlist_cover` 与 `set_artist_cover` 的 `bytes: number[] | null` 收窄为 `bytes: string | null`（键名不变，`mime` 不变）。验证：`rg -n 'bytes: string \| null' apps/desktop/src/bridge/index.ts` 命中两处；`rg -n 'number\[\]' apps/desktop/src/bridge/index.ts` 无输出。
- [x] 3.2 修改 `apps/desktop/src/features/playlists/PlaylistNameDialog.tsx`：选图改用 `FileReader.readAsDataURL`，按第一个 `,` 切分取 base64 载荷（base64 字母表不含 `,`），mime 仍取 `file.type`；按 design D4 把三态拆成 `useState` 的意图（`untouched` / `replace` / `reset`）与 `useRef` 的 `{ base64, mime }` 载荷；提交时传 `{ id, bytes, mime }`（键名不变，值为 base64 字符串），`reset` 传 `bytes: null`。保留 5 MB 选图上限检查与 `accept` 属性。验证：`rg -n 'Array.from\(bytes\)|new Uint8Array|readAsArrayBuffer' apps/desktop/src/features/playlists/PlaylistNameDialog.tsx` 无输出（**注意**：同文件 `countGraphemes` 的 3 处 `Array.from` 与封面无关，必须保留，不要用裸 `Array.from` 做判据）；`rg -n 'readAsDataURL|useRef' apps/desktop/src/features/playlists/PlaylistNameDialog.tsx` 命中预期位置。
- [x] 3.3 在 `apps/desktop/src/features/playlists/PlaylistNameDialog.test.tsx` 补测试 `sends the picked cover as a base64 payload instead of a byte array`：模拟选图后断言 `bridge.call("set_playlist_cover", …)` 的载荷里 `bytes` 是 base64 字符串、`mime` 为 `image/png`，且 `bytes` 不是数组。验证：`pnpm --dir apps/desktop exec vitest run src/features/playlists/PlaylistNameDialog.test.tsx` 通过。
- [x] 3.4 同文件补测试 `rejects a picked cover larger than the size limit`：选一个超过 5 MB 的 `File`，断言提示出现、没有发出 `set_playlist_cover` 调用、已有封面预览不变。验证：同 3.3 的 vitest 命令通过。
- [x] 3.5 扩展 `PlaylistNameDialog.test.tsx` 的既有用例 `renders a saved cover at detail size and passes blob previews through`：保留原有的「已保存封面走 `?s=detail`」与「选图预览走 blob、不经过 `assetUrl`」断言，补一条「界面状态中不保留逐字节数组」（例如断言提交载荷的 `bytes` 是字符串）。**不要**另建同义用例——既有用例已经覆盖 DAS-R23-S03 的前半段，重复用例会让证据名与场景映射产生歧义。验证：同 3.3 的 vitest 命令通过。
- [x] 3.6 运行前端静态门禁：`pnpm --dir apps/desktop typecheck`、`pnpm --dir apps/desktop lint`、`pnpm --dir apps/desktop test`。验证：三条命令均退出 0。

## 4. 预览临时资源回收与端到端替身

- [x] 4.1 在 `PlaylistNameDialog.tsx` 实现 blob URL 的三个回收时机（design D4）：重新选图时先 revoke 旧 blob；点击「自动封面」回到 `reset` 时 revoke 当前 blob；对话框卸载时在 `useEffect` 的 cleanup 中 revoke。回收句柄放 `useRef`，不与 `coverPreview` 混用。验证：三条回收路径各自有测试覆盖（4.2 的两条 + 既有选图用例的卸载路径）；`rg -n 'revokeObjectURL' apps/desktop/src/features/playlists/PlaylistNameDialog.tsx` 至少命中一处——若抽成 `revokePreviewBlob()` helper，调用点应为 3 处，此时改用 `rg -c 'revokePreviewBlob\('` 断言 3 处调用（含定义行则为 4），**不要**把「字面 `revokeObjectURL` 恰好出现 3 次」当判据。
- [x] 4.2 在 `PlaylistNameDialog.test.tsx` 补测试 `releases the previous preview blob when another cover is picked` 与 `releases the preview blob when the dialog unmounts`：断言旧 blob URL 被回收、卸载后当前 blob URL 被回收。jsdom 25 **没有** `URL.createObjectURL` / `URL.revokeObjectURL`（两者都是 `undefined`，`vi.spyOn` 会抛 `property not found`），因此在 `apps/desktop/src/test/setup.ts` 用 `Object.defineProperty` 统一 stub 这两个方法（现有 `PlaylistNameDialog.test.tsx` 的 `renders a saved cover...` 用例也是这么 stub `createObjectURL` 的），测试里改为读 `vi.mocked(URL.revokeObjectURL)` 的调用参数。setup 级 stub 同时保证既有选图用例在 RTL 卸载触发 cleanup revoke 时不再抛 `TypeError`。验证：`pnpm --dir apps/desktop exec vitest run src/features/playlists/PlaylistNameDialog.test.tsx` 通过，且既有用例 `renders a saved cover at detail size and passes blob previews through` 未回归。
- [x] 4.3 在 `apps/desktop/e2e/mock-bridge-handlers-library.ts` 为 `set_playlist_cover` 补一个 handler（当前命令名已在 `mock-bridge-commands.ts` 的联合类型里但没有 handler，一旦有 e2e 路径打开编辑对话框就会抛「no handler」），并让 handler 记录收到的 `bytes` 载荷以便断言载荷形态（记录的是字符串而不是数组）。验证：`pnpm --dir apps/desktop test:e2e` 退出 0；`rg -n 'set_playlist_cover' apps/desktop/e2e/mock-bridge-handlers-library.ts` 命中。
- [x] 4.4 运行 `pnpm --dir apps/desktop typecheck`、`pnpm --dir apps/desktop lint`、`pnpm --dir apps/desktop test`、`pnpm --dir apps/desktop test:e2e`。验证：四条命令均退出 0。

## 5. 生成契约零漂移与规模门禁

- [x] 5.1 证明 IPC 生成契约**零漂移**（不是「改了生成物」）：运行 `cargo run -p echo-desktop --bin echo-generate-ipc && git diff --exit-code -- apps/desktop/src/ipc/ipc-types.generated.ts` 与 `cargo test -p echo-desktop --all-features ipc::generate::tests::committed_generated_file_is_in_sync_with_the_generator`。验证：两条命令均退出 0，`git diff` 为空；`openspec/changes/fix-cover-bytes-memory-churn/proposal.md` 的「事实校核」第 1 条与该结果一致。
- [x] 5.2 运行规模门禁与依赖门禁：`node scripts/verify/check-scale.mjs`（源文件 ≤1000 行、公开 trait ≤6 方法）与 `pnpm verify:task -- 1.7`（cargo-deny 许可证 + LICENSES.md 存在性）。验证：两条命令均退出 0；`Cargo.lock` 的 crate 集合未变（`base64 0.22.1` 已是既有依赖），因此 **不整体再生成 `LICENSES.md`**，避免把无关 churn 混入本 change。
- [x] 5.3 独占运行治理门 `pnpm verify:governance`（不并行跑 cargo、rg 或 node 扫描；5 秒墙钟断言会被干扰）。验证：整门退出 0；若有失败，记录完整输出并区分本 change 引入的失败与既有 flake。

## 6. 文档与后续 issue

- [x] 6.1 在 `docs/DESIGN.md` 的封面相关小节补一句说明：封面字节经 Tauri command 以 base64 字符串传递、在 command 边界解码、Core 只见 `&[u8]`。验证：`rg -n 'base64' docs/DESIGN.md` 命中该句，且该句不与 §2.4 封面资产协议的档位语义冲突。
- [x] 6.2 为既有缺口「`artwork::validate` 只查体积与 MIME 白名单，非图片内容也能落盘」立 GitHub issue，正文引用本 change 与 `crates/echo-core/src/application/artwork.rs` 的 `validate`。验证：`gh issue view <新 issue 编号> --json title,body` 返回正确内容，并在 PR 中链接。
- [x] 6.3 为「blob 预览仍需解码原图」立 GitHub issue（若 7.3 的实测显示预览解码是新的主导项，则在正文中引用实测数字；若实测显示其可忽略，则在 PR 描述中记录实测结论并把 issue 标记为暂不需要）。验证：`gh issue view <新 issue 编号> --json title,body` 返回正确内容，或 PR 描述中写明实测结论。

## 7. 场景登记、集成验证与交付

- [x] 7.1 把 delta 的两条要求**同步进主规格**（early-sync）：在 `openspec/specs/desktop-app-shell/spec.md` 的 `## Requirements` 末尾追加 `DAS-R22`「封面字节必须以受控载荷传递」与 `DAS-R23`「封面预览的临时资源必须被回收」，内容与 `openspec/changes/fix-cover-bytes-memory-churn/specs/desktop-app-shell/spec.md` **逐字一致**（`openspec archive` 对内容相同的 ADDED 要求按幂等处理）。验证：`node scripts/verify/spec-scenarios.mjs` 输出的 `desktop-app-shell` 末两条 Requirement 为 `DAS-R22`、`DAS-R23`，场景总数为 559。
- [x] 7.2 在 `docs/traceability.md` 的 `## desktop-app-shell` 末尾追加 9 行（`DAS-R22-S01..S06`、`DAS-R23-S01..S03`），「任务」列写 `issue-113`，「测试层」按 design 与下表的映射填写，「实际验收命令」列写 `pnpm verify:scenario -- <ID>`；并把文件头的「当前基线」改为改动后的实际计数（144 个 Requirement / 559 个 Scenario）。验证：`node scripts/verify/spec-scenarios.mjs --check` 输出 `ok: 559 ...`。
- [x] 7.3 在 `scripts/verify/scenario-commands.mjs` 增加 9 条映射并更新顶部场景计数注释，证据指向真实存在的测试：

  | 场景 | 命令 | evidence |
  |---|---|---|
  | `DAS-R22-S01` | `VITEST_ALL` | `vitest("src/features/playlists/PlaylistNameDialog.test.tsx", "sends the picked cover as a base64 payload instead of a byte array")` |
  | `DAS-R22-S02` | `APP_ALL` | `rust("cover_payload::tests::decodes_valid_base64_to_original_bytes")` |
  | `DAS-R22-S03` | `APP_ALL` | `rust("cover_payload::tests::both_cover_commands_decode_through_the_same_helper")` |
  | `DAS-R22-S04` | `VITEST_ALL` | `vitest("src/features/playlists/PlaylistNameDialog.test.tsx", "rejects a picked cover larger than the size limit")` |
  | `DAS-R22-S05` | `DESK_ALL` | `rust("playlist_cover_rejects_unsupported_mime_without_replacing_the_saved_cover")` |
  | `DAS-R22-S06` | `APP_ALL` | `rust("cover_payload::tests::rejects_malformed_base64_as_a_validation_error")` |
  | `DAS-R23-S01` | `VITEST_ALL` | `vitest("src/features/playlists/PlaylistNameDialog.test.tsx", "releases the previous preview blob when another cover is picked")` |
  | `DAS-R23-S02` | `VITEST_ALL` | `vitest("src/features/playlists/PlaylistNameDialog.test.tsx", "releases the preview blob when the dialog unmounts")` |
  | `DAS-R23-S03` | `VITEST_ALL` | `vitest("src/features/playlists/PlaylistNameDialog.test.tsx", "renders a saved cover at detail size and passes blob previews through")` |

  验证：`node scripts/verify/validate-scenario-commands.mjs` 退出 0。
- [x] 7.4 运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `scripts/verify/manifest.json`，再运行不带 `--write` 的漂移门；随后运行 `node scripts/verify/reconcile-scenarios.mjs` 确认 spec / trace / manifest 三方 ID 集合相等。验证：三条命令均退出 0；`manifest.json` 含 559 条且 9 个新 ID 的 command 与 evidence 非空。
- [x] 7.5 更新 `docs/acceptance/PRD-matrix.md`：为对应验收路径追加 9 条 DAS 归属行，并更新摘要行的场景计数。验证：`node scripts/verify/prd-matrix.mjs` 输出与 559 个场景一致，退出 0。
- [x] 7.6 逐条运行 9 个新场景：`node scripts/verify/run-scenario.mjs DAS-R22-S01` … `DAS-R23-S03`（**不要**用 `pnpm verify:scenario -- <id>`，`--` 会被当成 id）。验证：9 条命令均退出 0。
- [x] 7.7 复测选封面路径的内存：重复 1.1 的测量（同一张图、同一资料库），记录 IPC 载荷的 JSON 文本长度与 `Echo Web Content` 的 footprint 峰值，写入 `/tmp/echo-cover-after.txt` 并与基线对比。验证：文件包含前后两组数字与差值；若 GUI 无法运行，写明具体阻塞原因，不编造数字。
- [x] 7.8 运行 `openspec validate fix-cover-bytes-memory-churn --strict`，确认 proposal / specs / design / tasks 全部通过。验证：命令退出 0。
- [x] 7.9 运行 `openspec archive fix-cover-bytes-memory-churn --yes`（delta 已按 7.1 early-sync 进主规格，归档时按幂等处理）；确认 `openspec/changes/archive/` 下有本 change，且 `openspec/specs/desktop-app-shell/spec.md` 里 `DAS-R22`/`DAS-R23` 各只出现一次。验证：`rg -c '### Requirement: 封面字节必须以受控载荷传递' openspec/specs/desktop-app-shell/spec.md` 输出 `1`；`openspec list --changes` 输出符合预期。
- [x] 7.10 归档后复跑 `node scripts/verify/spec-scenarios.mjs --check`、`node scripts/verify/reconcile-scenarios.mjs`、`pnpm verify:governance`（独占运行）。验证：三条命令均退出 0。
- [x] 7.11 提交并推送分支（中文提交信息），用中文 PR 关联 #113：运行 `git add`、`git commit -m "fix(cover): 封面载荷改用 base64 编码并回收预览资源 (#113)"`、`git push -u origin <分支名>`，再用 `gh pr create --base main --head <分支名> --title ... --body-file ...`，正文包含变更摘要、验证证据、`Closes #113`、以及 6.2/6.3 的后续 issue 链接。验证：`gh pr view --json url,title,body` 返回正确内容；CI 运行后记录结果，失败只修本 change 引入的问题并如实报告既有 flake。
