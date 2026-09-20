# normalize-os-file-open-paths — 施工收尾记录

施工日期：2026-09-20（HEAD `e48ae6e`，与 design 基准一致，E1–E7/E13/E14/E16–E21 全部复验相符）

## 交付内容

| 层 | 改动 |
|---|---|
| 壳（`apps/desktop/src-tauri/src/main.rs`） | 新增纯函数 `open_targets(&[tauri::Url]) -> Vec<PathBuf>`（`to_file_path()` 归一化 + 非 file scheme 丢弃告警 + 3 个单测）；`RunEvent::Opened` 分支改走 `open_targets`；`deliver_file_open` / `record_gate_open` 收窄为 `PathBuf`（URL 原文当路径下发无法编译） |
| 运行时（`crates/echo-desktop/src/runtime/mod.rs`） | `receive_file_open` / `on_ready` / `pending_opens` 收窄为 `PathBuf`；3 个 runtime 用例同步 |
| actor（`crates/echo-desktop/src/player/actor.rs`） | 抽出 `begin_load()`（generation+1、清 `pending_seek`/`position`/`duration`）；三个加载入口（库内、暂停库内、临时项）统一先调它；新增 5 个行为测试；`TestBackend` 增 `media_replay_on_observe`（建模范 mpv observe 回放 duration）。`is_local_media_path` / `protocol-whitelist` 零改动（3.3 已核） |
| 前端 | `App.tsx` 注释澄清载荷为已解码路径；`App.test.tsx` 新增「字面 `%20` 不做二次解码」用例 |
| 门禁 | `task-9.1.mjs` 扩第 4 段验收（open_targets 行为级断言 + 禁止 `url.to_string()`）；新增 `task-16.1.mjs`（归一化契约）、`task-16.2.mjs`（进度事实）、`check-scenario-command-proof.mjs` + `scenario-command-proof-declarations.json`（治理，manifest 16.3，已接入 `ci-governance.mjs`）；injection-suite 新增 `file-open-normalization/raw-url-payload` 与 `scenario-command-proof/undeclared-hybrid` 两个注入条目 |
| 场景纠正 | `SFI-R06-S01/S02/S03` 从无关的 `task-12.7.mjs` 迁到 `task-9.1.mjs && cargo test … runtime::services::tests::open_path`；新增 3 个分派谓词测试；`gen-scenario-manifests --write` 重生成（258 场景，spec=trace=manifest 对账通过） |
| 文档 | `docs/traceability.md` 三行覆盖描述更正（12.7 → 9.1）；`docs/native-attestation-playbook.md` 补第 0 条修正记录；`openspec validate --strict` 通过 |

## 关键验证输出

- `cargo test -p echo-app open_targets`：3 passed；`cargo test -p echo-desktop --all-features player::actor`：46 passed（含新增 5 个）；`runtime::services::tests::open_path`：3 passed。
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets --all-features -- -D warnings`：退出 0；`cargo check -p echo-app` 后 `gen/schemas/` 与 `ipc-types.generated.ts` 无改动（D2 成立）。
- 前端：`pnpm typecheck && pnpm lint && pnpm test && pnpm build` 全部通过（188 tests）。
- 失败证明（手工，5.1）：把 `open_targets(&urls)` 改回 `url.to_string()` 形态后 `task-9.1.mjs` 退出 1，恢复后复绿。
- 注入证明（5.2/5.3）：`injection-suite --only file-open` 与 `--only scenario-command-proof` 均 1/1 proven。
- 场景实跑：`DAS-R06-S02`、`SFI-R07-S01`、`SFI-R06-S01/S02/S03` 全部 ok（非 0 测试空跑）。
- e2e：`apps/desktop` 下 `node e2e/run-e2e.mjs` 全绿（13.1 覆盖面不变）。

## 与 design/tasks 的偏差（2 处，均为有意的落地决策）

1. **16.3 采用「证明 / 申报」双轨棘轮**：tasks 5.3 原文要求所有混合形态检查的内部断言必须在 ENTRIES 有条目。存量有 9 个混合形态场景后端检查（task-3.10/3.14/12.4/12.6/12.7/13.8/9.2/9.3/9.6）各需一个可证伪的 mutate 条目，为本 change 范围外且注入套件运行时间会翻倍。落地为：未证明的混合检查必须显式登记进 `scenario-command-proof-declarations.json`（写明原因、机器冻结、只许减不许增），新增未申报的混合检查依旧直接失败。这把 G4/G5 的「按文件二分类整体豁免」换成了「具名申报」，缺口可见、棘轮冻结；9 条申报作为下一个 change 的候选议题。
2. **`open_targets` 的告警用 `eprintln!` 而非 `tracing::warn!`**：echo-app 无 tracing 依赖，proposal 明确不新增依赖；echo-app 现有约定即 `eprintln!`（cover 协议同款）。design 片段里的 `tracing::warn!` 按实际依赖面调整。

## 观察项（未修，下一个 change 候选 —— tasks 6.4）

- **(a) `task-1.9.mjs` 对 file:// 形态天然不敏感**：走 argv 支路 + basename 子串断言 + fixture 无特殊字符。将来若要覆盖文件关联，需改为 `open -a Echo.app <file>` 触发 `RunEvent::Opened`、断言整行等于绝对路径、fixture 含空格/中文。
- **(b) `COMMANDS[id] || ATTEST(id)` 兜底 + 50 个未填写 native 模板**：仓库级既有状态，需独立 change 评估。
- **(c) 16.3 申报表中的 9 个混合形态检查**：逐个补 mutation-backed 注入条目（见偏差 1）。

## 阻塞 / 待操作者

- **6.1 macOS 真机复核**：需要 Finder 双击 GUI 交互，本会话无法执行。操作者步骤：`ECHO_GATE_OPEN_LOG=/tmp/echo-opens.log <启动 Echo>` → Finder 双击文件名含空格与中文的音频 → `cat /tmp/echo-opens.log` 必须为以 `/` 开头、无 `file://`、百分号已解码的绝对路径，且出声、标题显示解码名。⚠️ 注意区分 app 是裸二进制（embed dist）还是 `tauri dev`。

## 已解除的阻塞（2026-09-20 追记）

- **`task-1.9.mjs`（打包 Gate）环境阻塞已修复**：根因是 rust-lang/rust#157750 —— rustc `-C strip=debuginfo`（release 且 `debug=0` 时 cargo 自动启用）在 Xcode 27 linker 下产出 `__LINKEDIT` 字符串池未 8 字节对齐的 dylib，macOS 27 dyld 拒绝加载。修复为 LLVM llvm/llvm-project#203680（rust PR #158410，2026-06-26 合入，milestone 1.98.0）。处置：toolchain pin `1.96.0 → 1.98.1`（`rust-toolchain.toml` + `.github/workflows/ci.yml`×3 + `release.yml`×1 + `README.md` + `injection-suite.mjs` toolchain 条目锚点同步）；1.98.1 新 clippy lint 带来 5 处机械修复（4×`manual_assert_eq`、1×`missing_const_for_fn`）。修复后 `task-1.9.mjs` ok、workspace clippy/fmt/test 全绿、`injection-suite --only toolchain` 1/1 proven。
