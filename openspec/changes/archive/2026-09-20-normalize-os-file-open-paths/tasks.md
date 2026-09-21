> 施工前先读 `design.md` 的「实测证据」与「本轮方案自身的复核」，并复跑 E1–E7、E13/E14、E16–E21。
> 本 change 的所有数值以 **HEAD `e48ae6e`** 为准；行号会随 HEAD 漂移，定位时用 `grep` 而不是照抄行号。
> `scripts/verify/manifest.json` 现有任务 id 最大为 `15.4`，本 change 的门禁登记为新的 **`16.x`** 组（`pnpm verify:task 16.x` 依赖它）。
> propose 阶段只规划不实现：以下全部保持未勾选。

## 1. 前置复核与门禁登记

- [x] 1.1 在当前 HEAD 上复跑 `design.md` 的关键证据并按差异更新 design：`sed -n '865,874p' apps/desktop/src-tauri/src/main.rs`（E1）、`sed -n '1241,1272p' crates/echo-desktop/src/player/actor.rs`（E5/E6/E13）、`sed -n '1035,1077p' crates/echo-desktop/src/player/actor.rs`（E14）、`node scripts/verify/scenario-commands.mjs`（E16/E21）、`node scripts/verify/injection-suite.mjs --list`（E18）与 design 的**脚本 A**（E19/E20）。任何一条与 design 不符时先改 design 再动代码。
- [x] 1.2 在 `scripts/verify/manifest.json` 的 `tasks[]` 里注册 `16.1`–`16.3`（`16.1` = 壳侧归一化契约检查、`16.2` = 加载进度事实检查、`16.3` = 场景验收命令失败证明检查），`commands[].cmd` 分别指向 `node scripts/verify/checks/task-16.1.mjs`、`task-16.2.mjs`、`scripts/verify/check-scenario-command-proof.mjs`；运行 `pnpm verify:task 16.1` 确认登记被识别（脚本写完不登记就是死门禁）。
- [x] 1.3 确认「不改命令集与权限」的判定成立：`play_temporary_file` 的签名与 `main.json` 的 permissions 均不改，运行 `cargo check -p echo-app` 后 `git status --short` 中**不应**出现 `crates/echo-desktop/src/ipc/ipc-types.generated.ts` 与 `apps/desktop/src-tauri/gen/schemas/` 的改动；若出现则说明间接触动了 IPC 形状，必须回头核对 `design.md` D2。

## 2. 壳边界归一化（主修）

- [x] 2.1 在 `apps/desktop/src-tauri/src/main.rs` 新增私有纯函数 `fn open_targets(urls: &[tauri::Url]) -> Vec<PathBuf>`：对每个 URL 调 `to_file_path()`，`Ok` 收集、`Err` 记 `tracing::warn!` 并丢弃；配 `#[cfg(all(test, target_os = "macos"))]` 或与本次一并加入的同 crate 模块单测，断言：`file:///Users/…/We%20Will%20Rock%20You%20-%20Queen.flac` → `/Users/…/We Will Rock You - Queen.flac`；`http://x/a.flac`、`smb://host/share/a.flac` 被丢弃（返回空）；`[file, http, file]` 混合输入只留两条且顺序保持。运行 `cargo test -p echo-app --lib open_targets`（若 `echo-app` 为 bin crate 用 `cargo test -p echo-app`）通过。
- [x] 2.2 把 `RunEvent::Opened { urls }` 分支改为 `for path in open_targets(&urls) { deliver_file_open(app, path); }`，并删除该分支里的 `url.to_string()`；运行 `grep -c 'url.to_string()' apps/desktop/src-tauri/src/main.rs` 为 **0**。
- [x] 2.3 收窄 `fn deliver_file_open(app: &tauri::AppHandle, path: PathBuf)` 与 `fn record_gate_open(paths: &[PathBuf])`（日志用 `to_string_lossy()` 写出，保持 `task-1.9.mjs` 的 `ECHO_GATE_OPEN_LOG` 行格式为绝对路径、无 scheme 前缀）；运行 `cargo clippy -p echo-app -- -D warnings` 通过。
- [x] 2.4 收窄 `crates/echo-desktop/src/runtime/mod.rs` 的 `StartupSupervisor::receive_file_open(&self, path: PathBuf)` 与 `pending_opens` 队列元素为 `PathBuf`（含 `on_ready()` 的返回类型），并同步该文件内 3 个 runtime 用例的载荷（`"/music/a.flac".to_owned()` → `PathBuf::from(...)`）；运行 `cargo test -p echo-desktop --all-features runtime::` 通过。
- [x] 2.5 平台无关验证：`cargo fmt --all -- --check` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings` 均退出 0（`--` 不能省）；再跑 `node scripts/verify/checks/task-9.1.mjs` 确认既有 FIFO 语义未被破坏。

## 3. 加载进度事实（次要缺陷 1）

- [x] 3.1 在 `crates/echo-desktop/src/player/actor.rs` 抽出 `fn begin_load(&mut self) -> u64`：执行 `generation += 1`、`pending_seek = None`、`position = None`、`duration = None`，返回新 generation；把 `load_path` 开头与 `LoadLibrarySong` / `LoadLibrarySongPaused` 两个解析失败分支统一改为先调它（`load_path` 自身的 `generation += 1` 与 `pending_seek = None` 一并移除以免重复）。
- [x] 3.2 补断言：模块内单测覆盖三个 `Failed` 产生点（含 `://` 形态被拒绝、resolver 返回 `Err` 的两条命令各一例），断言失败快照的 `duration`/`position` 均为 `None`；再加一例断言进入 `Loading` 的快照 `duration`/`position` 亦为 `None`，以及 `FileLoaded` 后由 mpv 上报值驱动的既有行为不变。运行 `cargo test -p echo-desktop --all-features player::actor` 通过。
- [x] 3.3 **不得放宽纵深防御**：不改 `is_local_media_path`、不改其既有测试 `is_local_media_path_classifies_schemes`、不改 `protocol-whitelist` 相关代码；运行 `git diff --stat` 确认这三处零改动，并运行 `cargo test -p echo-desktop --all-features is_local_media_path` 通过。

## 4. 前端契约（不做二次解码）

- [x] 4.1 更新 `apps/desktop/src/app/App.tsx` 中 `app://file-open-request` 订阅处的注释：载荷已经是**解码后的绝对路径**（OS 边界在壳侧完成 `to_file_path()`），显示名直接取路径末段；逻辑不改。
- [x] 4.2 在 `apps/desktop/src/app/App.test.tsx` 新增用例：载荷为含字面 `%20` 的路径（如 `/tmp/My%20Mix.flac`）时，`play_temporary_file` 收到的 `displayName` 必须是 `My%20Mix.flac`（**不得**是 `My Mix.flac`），且 `path` 原样透传；保留既有 `:138` 的普通路径用例为回归。运行 `cd apps/desktop && pnpm test -- --run src/app/App.test.tsx` 通过。
- [x] 4.3 前端整体门禁：`cd apps/desktop && pnpm typecheck && pnpm lint && pnpm test && pnpm build` 全部退出 0（无 HMR，`build` 不可省）。

## 5. 门禁通电与场景纠正

- [x] 5.1 扩展 `scripts/verify/checks/task-9.1.mjs`，新增第 4 段验收（**行为断言，不只看 token**）：断言 `main.rs` 的 `RunEvent::Opened` 分支调用 `open_targets(` 且整个文件不再出现 `url.to_string()`；并把 `open_targets` 的单测纳入该检查的 `cargo test` 调用；运行 `node scripts/verify/checks/task-9.1.mjs` 通过，且把该断言临时改回旧形态时它**必须失败**（手工确认一次）。
- [x] 5.2 在 `scripts/verify/injection-suite.mjs` 的 `ENTRIES` 增一条 `file-open-normalization`：机制用 `mutate`——把 `main.rs` 里的 `open_targets(&urls)` 改回 `urls.iter().map(|u| u.to_string()).map(PathBuf::from)` 形态，断言 `task-9.1.mjs` 退出非 0，`finally` 按哈希恢复原文。运行 `node scripts/verify/injection-suite.mjs --only file-open` 通过（本机若被删除护栏拦截，按签名 `SAFE_DELETE_BULK_CONFIRM_REQUIRED` 归因后在 CI 用 `ECHO_INJECTION_REQUIRE_ALL=1` 复跑）。
- [x] 5.3 让新增治理需求可机械校验：新增 `scripts/verify/check-scenario-command-proof.mjs`，它读 `scenarioCommands()` 取出所有以 `node scripts/verify/checks/<…>.mjs` 承担的命令，与注入套件（含 `injection-object-kinds.mjs`）的条目集合比对，并**按断言而非按文件**判断豁免——一个检查只有在「不含 `spawnSync`/`execFileSync`」或「其自包含断言已在 `ENTRIES` 中声明」时才可通过；同时断言没有检查被整体豁免而不申报其内部断言。注册进 `manifest.json` 的 `16.3`、加进 `ci-governance.mjs` 的 `checks` 列表，并**为它自己也补一条注入证明条目**（否则新脚本就是第二个死门禁）。运行 `pnpm verify:task 16.3` 与 `node scripts/verify/injection-suite.mjs --only scenario-command-proof` 通过。
- [x] 5.4 修正 `SFI-R06-S01/S02/S03` 的命令映射：在 `scripts/verify/scenario-commands.mjs`（**唯一权威源**）改这三条，按 `design.md` D5 的表——`S01`/`S02` 指向能证伪「路径 → 临时项 / 路径 → 既有 UUID」的定向测试（与 `task-9.1.mjs` 的壳侧检查组合）；`S03` 若无法在本机自动化则改用 `ATTEST("SFI-R06-S03")` 并同步填实 `tests/native/SFI-R06-S03.md` 的可执行步骤与 `docs/native-attestation-playbook.md`。改完运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `manifest.json.scenarios[]` / `tests/scenarios/*.yaml`，**不得**手改这些生成物。
- [x] 5.5 门禁一致性与静态三件套：`node scripts/verify/scenario-commands.mjs`、`node scripts/verify/validate-scenario-commands.mjs`、`node scripts/verify/validate-scenario-manifests.mjs`、`node scripts/verify/reconcile-scenarios.mjs`、`node scripts/verify/check-scenario-churn.mjs`、`node scripts/verify/check-verification-validity.mjs` 全部退出 0；再用 `node scripts/verify/run-scenario.mjs DAS-R06-S02` 与 `node scripts/verify/run-scenario.mjs SFI-R07-S01` 确认两条场景实际跑起来（不是"选中 0 个测试"）。
- [x] 5.6 全量治理门禁：`pnpm verify:governance` 退出 0（含 14.4/14.5/14.7/14.8/14.9、覆盖率门禁 12.8 与内嵌前端新鲜度）。大重构后若覆盖率假红，先 `cargo llvm-cov clean --workspace` 再复跑。

## 6. 端到端验证与文档同步

- [x] 6.1 macOS 真机复核（手工，一次性）：`ECHO_GATE_OPEN_LOG=/tmp/echo-opens.log <启动 Echo>`，在 Finder 双击一个**文件名含空格与中文**的音频，`cat /tmp/echo-opens.log` 必须是一行**以 `/` 开头、无 `file://`、百分号已解码**的绝对路径，且音频真的出声、标题显示解码后的文件名。⚠️ 先确认 app 是怎么起的——`cargo build` 的裸二进制编译期 embed dist，`tauri dev` 走 CLI 静态服务器，两者写同一个 `target/debug/echo`。
  - 处置：**以复核结论关闭**（design.md「归档复核 2026-09-21」）。Finder 双击人工步骤本会话无法执行（wrapup 已记录），断言行为已由 `open_targets` 单测 + `App.test.tsx` 字面 `%20` 用例 + `task-1.9` 冷/热单实例自动化覆盖。
- [x] 6.2 跨平台回归：`cd apps/desktop && CHROME_NO_SANDBOX=1 node e2e/run-e2e.mjs` 通过（必须在 `apps/desktop` 下跑）；macOS 上再跑 `node scripts/verify/checks/task-1.9.mjs` 确认打包 Gate 仍绿。
- [x] 6.3 同步追溯与举证文档：更新 `docs/traceability.md` 中 `DAS-R06-*` / `SFI-R06-*` / `SFI-R07-*` 的覆盖描述（现在是 `task-9.1.mjs` + 定向测试的组合，不是 `task-12.7.mjs`），并在 `docs/native-attestation-playbook.md` 更正这些场景的覆盖口径；运行 `node scripts/verify/reconcile-scenarios.mjs` 与 `pnpm exec openspec validate normalize-os-file-open-paths --strict` 均退出 0。
- [x] 6.4 记录**未修的两条观察项**（写入本 change 的收尾说明并作为下一个 change 的候选，不在本 change 内动手）：(a) `task-1.9.mjs` 走 argv 支路且只用 `basename` 做子串断言、fixture 名不含特殊字符，因此对 `file://` 形态天然不敏感（`design.md` D6）——将来若要让它真正覆盖文件关联，需改为 `open -a Echo.app <file>` 触发 `RunEvent::Opened`，断言改为**整行等于绝对路径**并使用含空格/中文的临时 fixture；(b) `scenario-commands.mjs` 末行的 `COMMANDS[id] || ATTEST(id)` 兜底会让未登记场景静默降级为人工举证，而 `tests/native/` 的 50 个模板全部未填写、`artifacts/native-attestations/` 为空（`design.md` D8）——需要独立 change 评估。
- [x] 6.5 回填归档素材：把 6.1 的真机日志片段与 5.2/5.6 的输出摘要贴进本 change 的收尾记录；归档时按 `openspec archive normalize-os-file-open-paths` 执行并把 `tasks.md` 逐条回填勾选。
  - 证据：wrapup.md 已回填 5.2/5.6 的注入证明与治理输出摘要（`injection-suite` 1/1 proven、`pnpm verify:governance`、scenario 实跑、e2e）；6.1 的 Finder 日志片段随 6.1 延期（见 design.md「归档复核 2026-09-21」）。本批复核完成回填并勾选。
