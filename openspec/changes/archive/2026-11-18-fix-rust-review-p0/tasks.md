# Tasks

> 每个任务组自带对应测试与验证；任务可追溯到 `issue #162` 与本 change 的 delta 场景。所有命令在仓库根目录执行。
>
> **场景登记范围**：`local-library`、`library-durability`、`desktop-playback` 在 `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX` 内，共新增 9 个场景，基线 621 → 630。`lan-library-sync`、`macos-now-playing-menu`、`macos-notch-player`、`engineering-governance` **不在**该映射内（见 design D14），其 delta 场景只由 crate 级测试命令证明，不进入 `traceability.md` / `manifest.json`。

## 1. 基线与失败证据（不写生产代码）

- [x] 1.1 按 `AGENT.md` 从 `main` 新建独立分支（建议 `fix/issue-162-rust-review-p0`），不得直接向 `main` 推送。验证：`git branch --show-current` 输出该分支名，`git status --short` 只包含本 change 的规划文件。
- [x] 1.2 复核工具基线仍成立并把输出写入 `/tmp/echo-issue-162-baseline.txt`：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。验证：文件含三条命令的退出码；若 clippy 或测试在基线阶段已失败，先修正 `proposal.md` 的「工具基线全部通过」表述再继续，不得带着红灯开工。
- [x] 1.3 复核 design.md「Context」表中的 20 条事实行号与当前 HEAD 一致，把不一致项先修回 `design.md`。验证：`rg -n 'identity_key\(\)|unwrap_or_default|danger_accept_invalid_certs|changed_at_ms = mutation' crates/echo-core/src crates/echo-sync/src` 的输出与表中位置吻合；`rg -n 'PROTOCOL_VERSION' crates/echo-sync/src/protocol.rs` 仍为 `= 1`。
- [x] 1.4 记录场景基线：`node scripts/verify/spec-scenarios.mjs --check` 输出 `ok: 621`，并记录三个目标区域当前的末位场景 ID（`LL-R12-S03`、`LD-R12-S08`、`DP-R22-S03`）。验证：输出追加到基线文件；后续新增场景必须只追加在所属 Requirement 末尾（场景 ID 由位置派生）。

## 2. Core 领域层：错误类型与不变量

- [x] 2.1 为 `crates/echo-core/src/error.rs` 补 `pub fn invariant(why: impl Into<String>) -> Self` 构造器（`InvariantViolation` 变体已存在但无 builder）。验证：`rg -n 'pub fn invariant' crates/echo-core/src/error.rs` 命中；`cargo test -p echo-core --all-features` 通过。
- [x] 2.2 给 `TombstoneDecisionError`、`ConfirmationError`、`DeleteIntentError`（`domain/sync.rs`）与 `AliasError`（`domain/sync_members.rs`）加 `thiserror::Error` 派生与逐变体 `#[error("...")]`，保留既有 `Clone/Copy/Debug/Eq/PartialEq`。验证：`rg -n 'thiserror::Error' crates/echo-core/src/domain/sync.rs crates/echo-core/src/domain/sync_members.rs` 四处命中；`rg -n 'Copy' crates/echo-core/src/domain/sync.rs` 显示四个枚举仍派生 `Copy`。
- [x] 2.3 加测试 `domain_errors_implement_std_error`：对四个错误各取一个变体，断言 `format!("{}", error)` 非空且不等于类型名，并断言 `fn assert_error<E: std::error::Error + Send + Sync + 'static>()` 能接受它们。验证：`cargo test -p echo-core --all-features domain_errors_implement_std_error` 通过；`cargo clippy -p echo-core --all-targets --all-features -- -D warnings` 退出 0。

## 3. Core 应用层：恢复、根切换、删除

- [x] 3.1 写失败用例 `restore_propagates_a_projection_read_failure`：用既有 testkit 让 `all_in_root` 返回错误，断言 `RestoreLibrary::run` 返回 `Err` 且错误类别不是「全部记录被拒绝」的报告结果（映射 `LD-R06-S03`）。验证：`cargo test -p echo-core --all-features restore_propagates_a_projection_read_failure` 在修复前失败，输出追加到基线文件。
- [x] 3.2 把 `application/restore.rs:103` 的 `unwrap_or_default()` 改为 `?`（design D5）。验证：`rg -n 'unwrap_or_default' crates/echo-core/src/application/restore.rs` 无输出；3.1 的用例转为通过。
- [x] 3.3 写失败用例 `root_switch_rejects_an_unparseable_epoch` 与 `root_switch_does_not_overflow_the_epoch`：前者让持久化 epoch 值不可解析、断言切换失败且原活动根不变；后者把 epoch 置为 `u64::MAX`、断言不 panic 且不以回绕后的值提交（映射 `LL-R01-S06`）。验证：两条用例在修复前失败（后者应表现为溢出 panic 或代际复用），输出追加到基线文件。
- [x] 3.4 修正 `application/root_switch.rs:366-385`：解析失败返回 `Error::invariant`（不得 `unwrap_or(0)`），自增用 `checked_add(1)`，溢出时以明确失败结束切换（design 见 `proposal.md` 的 LL-R01 增补段）。验证：3.3 两条用例转为通过；`rg -n 'unwrap_or\(0\)|current_epoch \+ 1' crates/echo-core/src/application/root_switch.rs` 无输出。
- [x] 3.5 写失败用例 `derive_lrc_path_keeps_an_extension_less_name`：构造无扩展名的 `RelativeMediaPath`，断言推导结果是「原路径 + `.lrc`」而非截断一个字节；再构造大写扩展名的路径，断言不 panic（映射 `LL-R07-S05`）。验证：修复前用例失败或 panic，输出追加到基线文件。
- [x] 3.6 按 design D12 重写 `application/delete.rs:545-551` 的 `derive_lrc_path`：改用 `extension()` + `strip_suffix`，失败路径返回 `Error::invariant("lrc sibling path")`，删除 `expect`。验证：3.5 用例通过；`rg -n 'expect\(' crates/echo-core/src/application/delete.rs` 无输出。

## 4. Core 基础设施：文件系统

- [x] 4.1 写失败用例 `coalescer_samples_the_real_case_of_a_created_file`：直接构造 `Coalescer`（或经 `#[cfg(test)]` 的可见性入口），在 pending 中放入 `media/Song.MP3` 的 Created 事件，在 base 下真实创建该大小写文件，驱动两次 flush，断言最终 emit 的 `FileEvent.path.normalized()` 仍是 `media/Song.MP3` 且事件未被丢弃（映射 `LL-R02-S08`）。该用例**不得依赖宿主文件系统是否区分大小写**——断言的是采样路径而非「能否找到文件」。验证：修复前用例失败（事件被当作 vanished 丢弃），输出追加到基线文件。
- [x] 4.2 按 design D1 重构 `infrastructure/filesystem/watcher.rs`：`pending` 改为 `HashMap<RelativeMediaPath, PendingEvent>`，把 `RESCAN_SENTINEL` 拆成独立的 `rescan_at: Option<Instant>` 字段，`flush_ready` 直接用键的 `normalized()` 采样，删除 `RelativeMediaPath::new(&key)` 的反解。验证：4.1 用例通过；`rg -n 'identity_key\(\)' crates/echo-core/src/infrastructure/filesystem/watcher.rs` 无输出；`cargo test -p echo-core --all-features infrastructure::filesystem::watcher` 全绿。
- [x] 4.3 补 watcher 回归：`rescan_sentinel_still_emits_one_rescan_event`（单侧 rename 仍产生一次 `RescanNeeded`）与 `case_variants_of_one_file_still_coalesce`（同一文件两种大小写拼写仍合并为一条 pending）。验证：两条用例通过，证明 D1 没有破坏折叠语义。
- [x] 4.4 写失败用例 `enumerate_files_rejects_a_symlinked_media_dir`：在临时根目录下创建指向库外目录的 `media` 符号链接（`#[cfg(unix)]` 用 `std::os::unix::fs::symlink`，Windows 用 `symlink_dir` 并允许跳过），断言 `enumerate_files` 返回 `Err` 且未枚举链接目标中的任何文件（映射 `LL-R08-S04`）。验证：修复前用例失败（返回 `Ok` 并读到库外文件），输出追加到基线文件。
- [x] 4.5 按 design D2 在 `infrastructure/filesystem/walker.rs` 的 `canonical_root` 之前加 `symlink_metadata` 检查，`is_symlink()` 时返回 `Error::unavailable("library", "library media/ must not be a symlink")`。验证：4.4 用例通过；`rg -n 'symlink_metadata' crates/echo-core/src/infrastructure/filesystem/walker.rs` 命中。

## 5. Core 基础设施：不可信容器与算术

- [x] 5.1 写失败用例 `deeply_nested_udta_does_not_overflow_the_stack`：构造约 1 MB、嵌套层数远超正常值的 `udta` atom 字节，调用 `read_quicktime_udta_tags`，断言返回且不崩溃、标签解析结果为空或部分（映射 `LL-R03-S06`）。验证：修复前用例以栈溢出崩溃（该用例在修复前**不要**提交进 CI 主路径，只在本地确认一次后连同修复一起提交）。
- [x] 5.2 按 design D3 给 `walk_quicktime_udta` 加 `depth: u8` 参数与 `MAX_UDTA_DEPTH = 8`，超限直接返回。验证：5.1 用例通过；`rg -n 'MAX_UDTA_DEPTH' crates/echo-core/src/infrastructure/metadata/tags/mod.rs` 命中。
- [x] 5.3 写失败用例 `lrc_offset_overflow_drops_the_line_instead_of_panicking` 与 `probe_duration_overflow_yields_no_duration`：前者构造时间戳接近 `i64` 边界且 `offset` 同向极值，断言该行被计入 `out_of_range` 且不 panic；后者构造 `seconds` 极大且 `frac` 非零的时间基换算，断言 `duration` 为 `None` 且探测仍返回音频轨（映射 `LL-R03-S07`）。验证：两条用例在 debug 下以算术溢出 panic，输出追加到基线文件。
- [x] 5.4 按 design D4 修正 `metadata/lrc.rs:69` 与 `metadata/probe.rs:115`：改用 `checked_add` / `checked_mul` + `checked_add`，溢出分别走「丢弃该行」与「不提供时长」。验证：5.3 两条用例通过；`cargo test -p echo-core --all-features infrastructure::metadata` 全绿。

## 6. Core 基础设施：SQLite 事务

- [x] 6.1 写失败用例 `catalog_delete_rolls_back_every_statement` 与 `catalog_remove_member_rolls_back_every_statement`：在 `delete` / `remove_member` 的最后一条语句前注入失败（用既有的 SQLite 测试替身或一个会在提交前报错的 hook），断言 outbox、墓碑、行删除、快照、水位五者全部回到操作前状态（映射 `LL-R05-S03`）。验证：修复前用例失败（出现「已排队但行仍在」或「行已删但墓碑缺失」），输出追加到基线文件。
- [x] 6.2 按 design D6 把 `infrastructure/sqlite/repos_catalog.rs` 的 `delete`（262-300）与 `remove_member`（359-421）内所有语句改在 `connection.transaction()` 句柄上执行并 `commit()`。验证：6.1 两条用例通过；`rg -n 'connection\.' crates/echo-core/src/infrastructure/sqlite/repos_catalog.rs` 在 262-300 与 359-421 两个函数体内无残留（避免同连接二次借用）。
- [x] 6.3 运行 Core 静态门禁与全量测试：`cargo fmt --all -- --check`、`cargo clippy -p echo-core --all-targets --all-features -- -D warnings`、`cargo test -p echo-core --all-features`。验证：三条命令退出 0；`node scripts/verify/check-scale.mjs` 通过（`repos_catalog.rs` 与 `watcher.rs` 仍低于 500 行）。

## 7. 同步层：配对 PAKE 与协议版本（**BREAKING**）

- [x] 7.1 在 `crates/echo-sync/Cargo.toml` 加入 `pakery-crypto`（`default-features = false`，`features = ["std", "spake2", "p256"]`）、`rand_core = "0.10"`、`getrandom = { version = "0.4", features = ["sys_rng"] }`，并把它们放进 workspace 依赖表以保持版本集中。验证：`cargo tree -p echo-sync -e normal -i pakery-crypto` 命中；`cargo deny check licenses` 与 `cargo deny check advisories` 均退出 0；`rg -n 'sha2' Cargo.lock` 显示 0.10 与 0.11 并存（`multiple-versions` 为 `warn`，记录即可）。
- [x] 7.2 新增 `crates/echo-sync/src/pairing_pake.rs`：封装口令标量派生（`blake3(code || device_id)` → `P256Group::scalar_from_wide_bytes`，见 design D7 的实现修正）、`PartyA`/`PartyB` 两轮消息、`verify_peer_confirmation` 常量时间校验、会话密钥提取，以及 design D7 的「`getrandom::fill` 预检 + `UnwrapErr(SysRng)` 适配」随机源。验证：`rg -n 'Spake2P256|scalar_from_wide_bytes|verify_peer_confirmation' crates/echo-sync/src/pairing_pake.rs` 三者命中；`cargo test -p echo-sync --features client,server pairing_pake` 能编译。
- [x] 7.3 写失败用例 `pairing_code_is_bound_to_the_tls_channel`：模拟一个不持有配对码的中继（用自己的密钥参与两轮），断言设备侧 `verify_peer_confirmation` 失败、不返回可固定的证书；再写 `correct_code_completes_and_pins_the_authenticated_key`，断言成功路径固定的 SPKI 正是握手认证的那一个（映射 `lan-library-sync` R02 的两个新场景；该区域不在 `AREA_PREFIX` 内，见 design D14）。验证：第一条在修复前失败（现有实现只做回显比对，中继可成功），输出追加到基线文件。
- [x] 7.4 改造 `pairing.rs` 与 `client_pairing.rs`：`PairingCodeRequest` 携带首轮 PAKE 消息，`PairingBootstrapResponse` 携带主机侧消息与确认 MAC，新增设备侧确认 MAC 的回传字段；`validate_pairing_bootstrap` 在 PAKE 确认通过前不得放行；`danger_accept_invalid_certs(true)` 的注释写明只覆盖本次 bootstrap。验证：7.3 两条用例通过；`rg -n 'danger_accept_invalid_certs' crates/echo-sync/src/client_pairing.rs` 命中且上方有解释注释。
- [x] 7.5 把 `PROTOCOL_VERSION` 升到 `2`，并在 `server.rs` 的配对路由与既有版本校验点（`protocol.rs:223`、`:410`）确认旧版本被明确拒绝。验证：`rg -n 'PROTOCOL_VERSION: u16 = 2' crates/echo-sync/src/protocol.rs` 命中；写用例 `protocol_v1_pairing_is_rejected_with_a_distinguishable_error` 并断言错误映射到既有的「协议不兼容」类别而非「验证码错误」。
- [x] 7.6 补配对失败分类回归：`expired_or_used_code_still_reports_code_failure` 与 `pake_mismatch_reports_protocol_failure_not_code_failure`，确保新增的密码学失败不会被误报成「验证码错误」。验证：`cargo test -p echo-sync --features client,server pairing` 全绿；`cargo clippy -p echo-sync --all-targets --all-features -- -D warnings` 退出 0。
- [x] 7.7 更新 `apps/desktop/src-tauri/src/sync_commands.rs` 的错误映射，把 PAKE 失败与协议版本不匹配分别映射到既有的可诊断类别。验证：`rg -n 'PairingProtocolFailed|PairingFailed' apps/desktop/src-tauri/src/sync_commands.rs` 显示两条路径都存在；`cargo check -p echo-app --all-features` 退出 0。

## 8. 同步层：冲突时钟上界

- [x] 8.1 写失败用例 `host_clamps_a_far_future_changed_at_ms`：以注入时钟让本机 `now` 固定，提交一个 `u64::MAX` 的 `changed_at_ms`，断言裁决使用的是 `now + MAX_CLOCK_SKEW_MS` 而非原值；再断言本机随后对同一对象的编辑不会被永久判负（映射 `lan-library-sync` R05 的新场景）。验证：修复前用例失败，输出追加到基线文件。
- [x] 8.2 按 design D8 给 `HostApiState` 加可注入的 `now_ms`（默认 `SystemTime::now`），在 `server.rs:607` 钳制 `mutation.changed_at_ms`，公开 `MAX_CLOCK_SKEW_MS = 300_000` 常量。验证：8.1 用例通过；`rg -n 'MAX_CLOCK_SKEW_MS' crates/echo-sync/src` 命中；`rg -n 'let changed_at_ms = mutation.changed_at_ms;' crates/echo-sync/src/server.rs` 无输出。
- [x] 8.3 回归确认钳制不改写持久化正文：`clamping_does_not_rewrite_the_stored_mutation`，断言提交摘要与已存 mutation 的 `changed_at_ms` 保持原值。验证：用例通过；`cargo test -p echo-sync --features server` 全绿。

## 9. 桌面与平台层

- [x] 9.1 修正 `apps/desktop/src-tauri/src/macos_now_playing.rs:328-330`：`addTarget:action:` 绑定为 `let _: () = unsafe { msg_send![...] };`，并补一行注释指向 `MPRemoteCommand.h` 的 `void` 声明（design D9）。验证：`rg -n 'let _: \(\) = unsafe' apps/desktop/src-tauri/src/macos_now_playing.rs` 命中；`cargo check -p echo-app --all-features` 退出 0；在 macOS 上 `cargo run -p echo-app` 启动后控制中心四个命令可用（手工确认，记入 PR）。
- [x] 9.2 写失败用例 `reinstalling_the_screen_observer_removes_the_previous_registration`（`#[cfg(target_os = "macos")]`）：连续两次 `install_screen_observer`，随后 `unregister_screen_observer`，断言通知中心不再向任何观察者发消息且进程不崩溃。若无法在测试中观测通知中心状态，改为断言 `OBSERVER` 槽位在两次安装后只持有一个值且第一次的 `Retained` 已被 `removeObserver`（通过一个 `#[cfg(test)]` 计数钩子）。验证：修复前用例失败或无法观测到注销，输出追加到基线文件。
- [x] 9.3 按 design D10 重构 `apps/desktop/src-tauri/src/notch/window.rs`：抽出私有 `fn unregister(observer)`，`install_screen_observer` 在登记新观察者前先 `take()` + 注销旧的，`unregister_screen_observer` 复用同一函数。验证：9.2 用例通过；`rg -n 'removeObserver' apps/desktop/src-tauri/src/notch/window.rs` 只命中一处（唯一 `unsafe` 调用点）。
- [x] 9.4 修正 `crates/echo-desktop/src/player/actor/mod.rs:324-326`：`checked_sub(..).unwrap()` 改为 `unwrap_or_else(Instant::now)`（design D11）。验证：`rg -n 'checked_sub' crates/echo-desktop/src/player/actor/mod.rs` 命中且同处无 `.unwrap()`；补用例 `actor_initializes_when_the_host_uptime_is_under_one_second`（通过注入的计时基准，若无法注入则以「构造不 panic」为断言）映射 `DP-R01-S04`。
- [x] 9.5 修正 `crates/echo-desktop/src/player/coordinator.rs:187-226`：`play_context` / `play_context_paused` 在 `ctx.songs` 为空时返回明确的冲突错误而非索引，`current_id()` 的 `expect` 改为返回错误；同步更新 `# Errors` 文档并移除 `# Panics`。验证：`rg -n 'expect\("view context always yields' crates/echo-desktop/src/player/coordinator.rs` 无输出；`cargo test -p echo-desktop --all-features player` 全绿；空上下文用例断言返回冲突类别。
- [x] 9.6 运行桌面层门禁：`cargo clippy -p echo-desktop -p echo-app --all-targets --all-features -- -D warnings`、`cargo test -p echo-desktop --all-features`、`cargo check -p echo-app --all-features`。验证：三条命令退出 0。

## 10. 规格同步与 9 个场景登记（实现验证通过后才做）

- [x] 10.1 把 delta 中 9 个新场景**逐字**追加到对应主规格的所属 Requirement 末尾（位置决定 ID，不得插在中间）：`openspec/specs/local-library/spec.md` 的 R01/R02/R03/R05/R07/R08、`openspec/specs/library-durability/spec.md` 的 LD-06、`openspec/specs/desktop-playback/spec.md` 的 R01；其余四个 capability 的 delta 按既有同步流程同步但不进入场景登记。验证：`rg -c '^### Requirement:'` 各文件计数不变；`node scripts/verify/spec-scenarios.mjs` 输出 630 个 Scenario，新 ID 为 `LL-R01-S06`、`LL-R02-S08`、`LL-R03-S06`、`LL-R03-S07`、`LL-R05-S03`、`LL-R07-S05`、`LL-R08-S04`、`LD-R06-S03`、`DP-R01-S04`。
- [x] 10.2 在 `docs/traceability.md` 对应区域末尾追加这 9 行（任务列写 `issue-162`，测试层按实际 crate 写，验收命令写 `node scripts/verify/run-scenario.mjs <ID>`），并把文件头「当前基线」改为 630。验证：`node scripts/verify/spec-scenarios.mjs --check` 输出 `ok: 630`。
- [x] 10.3 在 `scripts/verify/scenario-commands.mjs` 为 9 个场景补映射，测试名必须与第 3–9 组实际落地的用例名逐字一致：

  | 场景 | 组 | evidence |
  |---|---|---|
  | `LL-R01-S06` | 3 | `root_switch_rejects_an_unparseable_epoch` + `root_switch_does_not_overflow_the_epoch` |
  | `LL-R02-S08` | 4 | `coalescer_samples_the_real_case_of_a_created_file` |
  | `LL-R03-S06` | 5 | `deeply_nested_udta_does_not_overflow_the_stack` |
  | `LL-R03-S07` | 5 | `lrc_offset_overflow_drops_the_line_instead_of_panicking` + `probe_duration_overflow_yields_no_duration` |
  | `LL-R05-S03` | 6 | `catalog_delete_rolls_back_every_statement` + `catalog_remove_member_rolls_back_every_statement` |
  | `LL-R07-S05` | 3 | `derive_lrc_path_keeps_an_extension_less_name` |
  | `LL-R08-S04` | 4 | `enumerate_files_rejects_a_symlinked_media_dir` |
  | `LD-R06-S03` | 3 | `restore_propagates_a_projection_read_failure` |
  | `DP-R01-S04` | 9 | `actor_initializes_when_the_host_uptime_is_under_one_second` |

  验证：`node scripts/verify/validate-scenario-commands.mjs` 与 `node scripts/verify/check-scenario-evidence.mjs` 均退出 0；不得为凑证据改名或新增同义用例。
- [x] 10.4 运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `scripts/verify/manifest.json`，再运行不带 `--write` 的漂移门与 `node scripts/verify/reconcile-scenarios.mjs`。验证：三条命令退出 0；manifest 含 630 条且 9 个新 ID 的 command / evidence 非空。
- [x] 10.5 复核 `docs/acceptance/PRD-matrix.md` 是否需要登记。按既有先例，`prd-matrix.mjs` 的 `A_FAMILIES` 不含 `LL`/`LD`/`DP` 前缀时不追加归属行。验证：`node scripts/verify/prd-matrix.mjs` 退出 0；`git diff --name-only` 中不含该文件；若确实需要登记，先修正 `proposal.md` 的基线数字再改。
- [x] 10.6 运行 9 个新场景：`node scripts/verify/run-scenario.mjs LL-R01-S06 LL-R02-S08 LL-R03-S06 LL-R03-S07 LL-R05-S03 LL-R07-S05 LL-R08-S04 LD-R06-S03 DP-R01-S04`（**不要**用 `pnpm verify:scenario -- <id>`，`--` 会被当成 id）。验证：命令退出 0，9 条 `ok: scenario ...` 均出现；另单独执行一条单 ID 形式确认。**注意**：runner 按命令去重，本 change 的 9 个场景分属多个命令，须确认每组命令各自被执行到。

## 11. 文档与最终门禁

- [x] 11.1 更新 `docs/DESIGN.md`：在同步章节写明配对码通过 SPAKE2 绑定到 TLS 通道、协议版本 2；在文件系统章节写明 `media/` 不得为符号链接、watcher 以真实路径采样。验证：`rg -n 'SPAKE2|符号链接|真实路径' docs/DESIGN.md` 命中三处，且不与既有 SPKI pin 描述冲突。
- [x] 11.2 复核 `docs/PRODUCT.md` / `docs/ROADMAP.md`：本 change 不改变产品范围、阶段或界面术语，记录结论即可。验证：`git diff --name-only` 中不含这两个文件与 `AGENT.md`；若结论相反，先修正规格再改文档。
- [x] 11.3 运行完整门禁：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo deny check`、`node scripts/verify/reconcile-scenarios.mjs`、`pnpm verify:governance`、`pnpm exec openspec validate fix-rust-review-p0 --strict`。验证：七条命令退出 0；`pnpm verify:governance` 须**独占运行**（并行 cargo 或其它扫描会让墙钟断言误报）。
- [x] 11.4 按 `engineering-governance` 的要求，在创建 PR 前把本 change 的 delta 同步到主规格并归档，然后才进入 PR。验证：`pnpm exec openspec list` 中不再列出 `fix-rust-review-p0` 为进行中；`git status --short` 干净。
- [ ] 11.5 在 PR 描述中显式写明三项发布影响：协议版本 1 → 2 导致已配对设备需重新配对；`echo-sync` 新增 PAKE 依赖与其许可证核对结论；`AREA_PREFIX` 外的四个 capability 只由 crate 级测试证明（design D14）。验证：PR 正文含这三条，且与 `design.md` 的 Migration Plan 一致。
