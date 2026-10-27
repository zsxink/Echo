# 历史兼容代码删除技术方案

> 状态：**规划输入**，尚未进入 OpenSpec change，也尚未改动任何代码。
> 用途：作为后续 `openspec propose` 的范围依据；实施必须走 apply 阶段。
> 行号只作导航（沿用 change 1.5 的约定），引用时以符号名为准。
> 本文所有计数与引用均在 `codex/issue-72-library-first-durability`（`ebc53f0`）的干净树上重新核对过。
> 2026-10-27 复核修订：修正 gen 脚本 dry-run 退出码说法；补记 lan-library-sync 升级条款、测试内第 4 处 `protocol_version` 写入、`validate_mutation` 漏列调用方、`sync_payloads` 完整迁移调用；校正若干行号。

---

## 0. 前提与结论

### 0.1 用户前提（本方案的判定基础）

1. 单用户使用，**没有旧音乐库数据、没有旧 SQLite 数据**需要兼容。
2. 同步**不会**出现跨音乐库版本的数据。

### 0.2 结论

在上述前提下，可以删除的历史兼容面分四档，合计约 **1700–1900 行**（含测试）：

| 档 | 主题 | 可删行数（估） | 风险 | 是否触及规格 |
| --- | --- | --- | --- | --- |
| **A** | v1→v2 可携带格式迁移（journal / backup / fence / 计划重放） | ~600 | 中 | 否（无规格强制要求该升级） |
| **B** | SQLite v19/v20 开发版兼容（`apply_root_sync_contexts` 特例） | ~290 | 低 | 否 |
| **S** | 同步侧历史兼容（协议列、旧标记、旧载荷形状） | ~200 | 中高 | 是（SYN-R07-S03） |
| **D** | 桌面文档 schema 兼容 | ~60 | 低 | 否 |

**但存在一个必须先定的判定点**（见 §7）：仓库是公开的且已发布 v0.1.0–v0.3.0，外部用户可能持有旧库。若需要继续对外部用户负责，A/B/D 的删除收益不变但影响面变大；若确认只对自己负责，四档可全删。

### 0.3 对上一轮分析的更正

上一轮报告中「C 档：546 行未跟踪探针文件」**不成立**。已逐个核对：

- `crates/echo-core/tests/probe_commit_manifest.rs` — 不存在，且从不在 `HEAD`
- `crates/echo-core/tests/probe_old_library.rs` — 不存在，且从不在 `HEAD`
- `crates/echo-desktop/tests/probe_prepare_root.rs` — 不存在，且从不在 `HEAD`
- `scripts/diagnostics/upgrade-library-manifest.mjs` — 不存在，且从不在 `HEAD`

`scripts/diagnostics/` 下只有已跟踪的 `artwork-palette-report.mjs`。工作树干净。**C 档删除量为 0，本文不再列。**

---

## 1. 必须保留的东西（先说边界，避免误删）

以下看起来像「兼容代码」，但**不能删**——它们服务的是本机现存数据或正在运行的协议：

| 保留项 | 位置 | 为什么不能删 |
| --- | --- | --- |
| `PortableRecordDocument::{V1,V2}` 双格式读取 | `domain/library/portable.rs` | 本机库 **1315 条 flat v1 记录**靠它读；删了等于丢数据 |
| `continuation/ordered.rs:62` 的 V1 分支 | `application/continuation/ordered.rs` | 恢复流程的活跃路径 |
| `ports/system.rs` 中 `read_record_document` 的 V1 默认实现 | `application/ports/system.rs` | 同上 |
| `LibraryManifest::is_compatible()`（manifest v1/v2/v3 读取） | `domain/library/mod.rs` | 本机 `manifest.json` 是 v3；兼容读取不等于升级 |
| **flat 墓碑路径**（`read_tombstone_document` 的 `for typed in [Some(deleted_kind), None]` 循环、`delete_tombstone` 的 legacy 分支） | `control_plane.rs:546`、`:575-578` | 本机 **12 个墓碑全部是 legacy flat 布局**（`records/tombstones/3b/….json`），这是活跃数据不是死代码 |
| 未来版本围栏（`format_version > CURRENT` 拒读/拒写/状态判定） | `control_plane.rs:180/197/375/470`（`:197` 拦读，`:180`/`:375` 拦写，`:470` 状态） | 保护当前数据不被旧写入器破坏 |
| 损坏库隔离 / 备份 / `quick_check` | `sqlite/connection.rs:277-377` | 与版本兼容无关的健壮性设施 |
| 迁移基础设施本体（`apply_migrations`、`apply_migration_set`、26 条已发布脚本、blake3 校验） | `sqlite/connection.rs`、`migrations/*.sql` | CODE_STANDARDS §5：已发布迁移不可改写 |
| `PROTOCOL_VERSION` 及其校验 | `echo-sync/src/protocol.rs:15, 223-225, 410-411, 418` | **活跃的逐 mutation 线上路径**，不是跨版本兼容 |
| `StateNamespace::LegacyEvidence` | `domain/library/durability/state.rs:29` | 名字含 "Legacy"，实为新协议 adoption/decision 模型的一部分 |
| `replica_enrollment_replace:{root}` 标记 | `sync_snapshot_apply.rs` | 与 `archive` 标记是兄弟项，`replace` 是活跃的 |

---

## 2. A 档：v1→v2 可携带格式迁移

### 2.1 判断依据

- `docs/library-durability-protocol.md` 明确写「读取旧 manifest **不自动改写版本或迁移目录**」。
- 主规格 `openspec/specs/library-durability/spec.md:123,127` 只要求**兼容读取**，并明说「**不要求**将既有旧库升级为新目录提交协议」。
- 全仓检索：**没有 spec 强制要求提供 v1→v2 记录升级**（`grep v1-v2 / 格式升级 / migration journal openspec/specs/` 无命中）。
- ⚠️ 检索词覆盖不到的边缘：`openspec/specs/lan-library-sync/spec.md:269`「历史库升级 MUST 保留 UUID、数据和未确认变更」及 `:272` 场景（WHEN 以「读取**并升级**历史可携带资料库后启用主服务」为前提）。这是条件式义务，不强制提供升级路径，不推翻本档结论；但删除升级写入器后该场景的 WHEN 前提在实现上不可达——**propose 阶段必须显式对齐**（见 §10 检查清单）。
- 唯一调用入口 `ControlPlanePort::apply_portable_upgrade` **无生产调用方**：调用点只有 3 个 impl 转发和 1 个测试。
- 本机 `~/Music/音乐库/echo/` **没有 `migrations/` 目录** → 无待恢复 journal，无 fence。

### 2.2 删除清单

**`crates/echo-core/src/infrastructure/filesystem/portable_migration.rs`（504 行，删约 261 行）**

| 行 | 符号 | 处置 |
| --- | --- | --- |
| 1–2 | 模块 doc（"Recoverable portable-format upgrade…"） | 改写为「可携带目录 IO 原语」 |
| 18 | `MAX_PLAN_BYTES` | 删 |
| 19 | `BACKUP` | 删 |
| 20 | `JOURNAL` | 删 |
| 22 | `FENCE_VERSION` | 删（保留 `MANIFEST`） |
| 24–49 | `Journal` / `Plan` / `Original` 三个 `deny_unknown_fields` 结构 | 删 |
| 179–183 | `decode<T>` | 删（仅被迁移路径使用） |
| 274–329 | `validate(plan)` | 删 |
| 331–347 | `recover()` | 删 |
| 349–356 | `apply()` | 删 |
| 358–444 | `apply_with_hook()` | 删 |
| 446–501 | `replay()` | 删 |

**必须保留**（被活跃代码大量引用，`grep` 计数）：`MAX_RECORD_BYTES`(17, 11 处)、`MANIFEST`(21)、`invalid()`(51)、`path()`(55, 18 处)、`record_relative()`(63, 19 处)、`original_relative()`(73, 6 处)、`read()`(87, 28 处)、`write()`(118)、`sync_directory()`(154)、`remove()`(166)、`encode()`(185, 13 处)、`record_files()`(217, 3 处)、`collect()`(229)。引用方：`durability.rs`、`durability/io.rs`、`durability/checkpoint.rs`、`durability/transaction.rs`、`control_plane.rs`。

> 建议：模块改名为 `portable_io.rs` 或保留文件名但更新 doc，避免「migration」名实不符。

**`crates/echo-core/src/infrastructure/filesystem/portable_migration/tests.rs`（498 行，删约 339 行）**

| 测试 | 行 | 处置 |
| --- | --- | --- |
| `partial_upgrade_is_fenced_and_new_adapter_recovers_the_persisted_plan` | 67 | 删 |
| `journal_before_fence_is_recoverable_and_formal_manifest_is_last` | 155 | 删 |
| `unmapped_original_aborts_before_any_journal_or_fence` | 211 | 删 |
| `corrupt_backup_stops_reads_and_writes_without_clearing_the_fence` | 241 | 删 |
| `migration_moves_flat_tombstones_only_after_an_equivalent_typed_write` | 332 | 删 |
| `actual_mid_write_filesystem_failure_recovers_after_the_blocker_is_removed` | 428 | 删 |
| `oversized_serialized_records_are_rejected_before_a_plan_is_published` | 478 | 删 |
| `typed_tombstones_with_the_same_uuid_coexist_and_delete_independently` | 279 | **保留但改写**：`apply()` 只用作 fixture，改为直接写 v2 manifest + `write_record_envelope` |
| `record_symlink_escape_fails_before_a_backup_or_fence` | 398 | **保留但改写**：断言从「无 backup/fence」改为「写入失败且 manifest 保持 v1」 |
| `future_format_and_uuid_paths_are_rejected_before_mutation` | 371 | **保留但改写**：`FENCE_VERSION` 字面量替换为 `u64::MAX`（该测试断言的是未来版本围栏，属保留项） |

辅助函数 `seed()`(50) 仅被删除项使用 → 删；`fixture()`(10)、`song()`(35) 保留。

**端口与适配器**

| 位置 | 处置 |
| --- | --- |
| `application/ports/system.rs:202-212` `ControlPlanePort::apply_portable_upgrade`（含默认实现） | 删 |
| `infrastructure/filesystem/control_plane.rs:582-590` 实现 | 删 |
| `application/testing/durability_ports/refusing.rs:141-148` 转发 | 删 |
| `infrastructure/sqlite/tests/lifecycle/ordered_fake.rs:130-137` 转发 | 删 |
| `infrastructure/filesystem/control_plane.rs:176` `migration::recover(...)` | 删（`prepare_locked_write` 内） |
| `infrastructure/filesystem/control_plane.rs:204-213` 待处理 journal 读拒绝 | 删（`prepare_read` 内） |
| `infrastructure/filesystem/control_plane.rs:730` 测试循环里的 `"migrations/v1-v2-journal.json"` | 删（改为只留 `manifest.json`） |

### 2.3 副作用

- `ControlPlanePort` 方法数 **15 → 14**。`crates/echo-core/tests/arch.rs:104` 断言是 `<= 15`，**只会更宽松，不会红**。
- `scripts/verify/check-scale.mjs` 的 `ControlPlanePort` allowlist 项保留（该 trait 仍 >6 方法，且 allowlist 只许收缩）。
- `openspec/changes/enforce-library-first-durability/evidence/9.7-ld-evidence-index-20261027.md` 与 `evidence/acceptance-matrix.md` 引用了 `portable_migration/tests.rs:67/155/241/371` 与 `LD-09b`（evidence 速记，正式场景 ID 为 `LD-R09-S02`）——该 change **尚未归档**，需在其实施时同步修正证据表。

---

## 3. B 档：SQLite v19/v20 开发版兼容

### 3.1 判断依据（实测）

- 0020 脚本的 git 历史只有 3 个提交，全部在 **2026-10-16**，且 `3297486 fix: make sync context migration idempotent` 正是引入 `IF NOT EXISTS` 变体的开发版提交，随后 `6e661b5 恢复已发布数据库迁移内容` 回滚了正文。
- 校验和（blake3，本机无哈希工具未直接复算，以下为旁证）：当前 `0020_root_sync_contexts.sql` = `fafdf4d7…`（旁证：本机库 `schema_migrations` version=20 行登记值即此前缀，由代码 blake3 算出）；开发版内容（`3297486`）= `d73256ef…`，**与 `LEGACY_ROOT_SYNC_CONTEXTS_CHECKSUM` 完全一致**（旁证：常量注释自述开发版场景、`3297486` 与初版 diff 恰为 `IF NOT EXISTS` 变体、同日回滚时间线）。实施时用一行 rust 复算闭环。
- 本机 `echo.sqlite` 的 `schema_migrations` 为 `1-18, 20, 21, 23-28`——**无版本 19**，且版本 20 存的是**当前**校验和 `fafdf4d7…`。
- 因此：`apply_root_sync_contexts` 的 v18→v20 队列升级在本机**早已跑完**；`LEGACY_ROOT_SYNC_CONTEXTS_CHECKSUM` 那条例外分支**当前并不作用于本机库**。
- 0020 正文的普通路径本身就能正确处理 v18 库（第 11 行前缀 `COALESCE((SELECT local_root_uuid FROM sync_binding WHERE singleton=1),(SELECT uuid FROM library_roots WHERE is_active=1),'')` 已优先绑定根）。特例函数额外提供的是：v19 开发库跳过队列升级、按 binding 归档再 select、无根 binding 拒绝。

### 3.2 删除清单

| 位置 | 符号 | 行数 |
| --- | --- | --- |
| `sqlite/connection.rs:60-63` | `LEGACY_ROOT_SYNC_CONTEXTS_CHECKSUM` 常量及注释 | 4 |
| `sqlite/connection.rs:152-157` | `apply_migration_set` 中 `\|\| (*version == 20 && applied == LEGACY_…)` 分支 | 6 |
| `sqlite/connection.rs:168-172` | `if *version == 20 && *script == …` 分派 | 5 |
| `sqlite/connection.rs:184-275` | `apply_root_sync_contexts()` 整体 | 92 |
| `sqlite/device_state.rs:55-89` | `backfill_legacy_play_counts()` | 35 |
| `sqlite/db_inherent.rs:76` | 该函数的调用 | 1 |
| `sqlite/device_state.rs:102-140` | `legacy_play_counts_are_backfilled_once_to_the_stable_device` | 39 |
| `sqlite/tests/sync_root_contexts.rs:341-373` | `legacy_connection()` 构造 v18 schema | 33 |
| `sqlite/tests/sync_root_contexts.rs:375-435` | `migration_assigns_single_ledger_to_binding_root_before_selecting_ui_root` | 61 |
| `sqlite/tests/sync_root_contexts.rs:437-467` | `migration_refuses_unrooted_binding_and_rolls_back_without_reassigning_ledger` | 31 |

合计约 **290 行**（含测试）。

### 3.3 关键前置检查

**必须先确认本机 `legacy_play_counts_backfilled` 已完成**——实测：`sync_state` 中该键 = `1`，`songs.play_count>0` 378 行，`sync_device_play_counts` 380 行。回填已生效，删除不会丢播放数。

**删除 `apply_root_sync_contexts` 的连带影响**：

- `sqlite/tests/durability_migration.rs` 的 `through_v27()`（行 6 起）用 `apply_migration_set` 逐条应用，其中第 20 条传的是 `include_str!("../migrations/0020_root_sync_contexts.sql")`，与 `migrations::ROOT_SYNC_CONTEXTS` **内容相同**，因此 `*script == migrations::ROOT_SYNC_CONTEXTS` 为真 → **当前走的是特例路径**。删除后需确认改为普通 `execute_batch` 仍通过（空库场景下二者等价）。
- `sqlite/tests/sync_payloads.rs` 在 100/160/202/221/297/317 行使用 `apply_migration_set`，但最高只到 0010，**不经过 0020**，不受影响。注意 `:232-233` 另有一处完整 `apply_migrations` 调用**会**经过 0020 特例路径，删除特例后与 `through_v27()` 同批确认普通 `execute_batch` 路径等价。
- 删除后，任何 v18 时代的库仍能靠 0020 普通正文正确迁移；**失去的只是 v19 开发库与额外安全校验**。本机库已过 20，不受影响。

---

## 4. S 档：同步侧历史兼容

### 4.1 删除清单

| 位置 | 符号 | 判断依据 | 处置 |
| --- | --- | --- | --- |
| `migrations/0010_lan_sync_ledger.sql:10` | `sync_binding.protocol_version INTEGER NOT NULL` | 全仓**无任何 SELECT**；生产 3 处写入硬编码 `1`（`sync_ledger.rs:185,258`、`sync_recovery.rs:305`），测试另有 1 处（`sqlite/tests/sync_root_contexts.rs:370`） | **需新增一条迁移** `ALTER TABLE sync_binding DROP COLUMN protocol_version`（0010 已发布不可改写，CODE_STANDARDS §5）；`sync_root_contexts.rs:370` 的测试 INSERT 须同批修改 |
| `sync/sync_snapshot_apply.rs:43,60,79` | `replica_enrollment_archive:{root}` 读/参数 | **无 INSERT/SET/UPDATE 写入方**；`complete_replica_media_archive` 只 DELETE | 删 3 处 + `pending_replica_media_archive`(52-66) + `complete_replica_media_archive`(70-84) |
| `apps/desktop/.../peer_publication.rs:254,323,327-352` | `clear_legacy_replica_media_archive_intent()` 及其 2 处调用 | 消费的是「上一版本遗留意图」，本机 `sync_state` 无该键 | 删 |
| `domain/sync.rs:460-483` | `decode_play_stats` 的旧形状回退分支（`{song_uuid, by_device}` → `claimant: None`） | 本机 `sync_mutations` 中 play-stats 载荷数 = **0**，含内嵌 `hlc` 的载荷 = **0** | 收紧为只接受 `PlayStatsRecord` |
| `domain/sync.rs:529-538` | `payload_changed_at_ms`（注释自称 "legacy portable record"） | **仍有活跃调用方**：`sync_ledger/host_ledger.rs:113` 作为 `changed_at_ms` 为 `None` 时的回退 | ⚠️ 见 §4.3，**不建议删** |
| `domain/sync.rs:540-563` | `confirm_candidate`（legacy 包装，无 `changed_at_ms`） | 调用方**全在 `crates/echo-core/tests/sync_domain.rs`**（178/208/219/233/542/555/777），无生产调用方 | 删包装，测试改用 `confirm_candidate_at` |
| `sync/sync_baseline.rs:298-319` + `:258` 调用 | `capture_legacy_plays()` | 把聚合 `songs.play_count` 搬进 `sync_device_play_counts`；与 B 档 `backfill_legacy_play_counts` 职责重复 | 删（与 B 档同批，避免留下单侧回填） |

合计约 **200 行**。

### 4.2 明确不可删的同步完整性代码

- `echo-sync/src/protocol.rs:15` `PROTOCOL_VERSION: u16 = 1`
- `protocol.rs:223-225` `validate_mutation` 的版本相等校验（活跃逐 mutation 路径，被 `protocol.rs:327`、`client.rs:128`、`server.rs:553`、`commit_routes.rs:118`、`client/apply.rs:238`（replica 侧 commit page 逐条校验）调用）
- `protocol.rs:410-411` `commit_digest` 版本校验、**`:418` `hasher.update(&commit.protocol_version.to_be_bytes())`**（改了会破坏摘要兼容）
- `sync_apply.rs:517`、`sync_snapshot.rs:250-254` 的 `!= 1` 身份校验
- `replica_enrollment_replace:{root}` 标记、inbox/cursors/aliases/tombstones 全部保留

> 注意：`rustls::version::TLS13`（`server.rs:415`、`client_tls.rs:76`）是 TLS 版本，与库版本无关，**勿误删**。

### 4.3 `payload_changed_at_ms` 的特别说明

它在**生产路径上是活的**：

- `apps/desktop/.../sync_provisioner.rs:489-493` 用 `local_mutation_changed_at_ms(...)` 取冻结时间，为 `Option<u64>`；
- `echo-sync/src/server.rs:607-612` 直接把 wire 的 `changed_at_ms`（`Option`）传下来；
- 两者为 `None` 时，`host_ledger.rs:112-113` 回退到 `payload_changed_at_ms`。

本机 475 条 `sync_mutations` 全部是 `song` 类型、**无内嵌 hlc**，所以回退分支对本机永远返回 0。但删掉它会把 `None` 情形从「回退到 0」变成「必须显式处理」，属于**行为收紧**而非死代码清理。**建议单独评估，不并入本次删除。**

---

## 5. D 档：桌面文档 schema 兼容

| 位置 | 符号 | 依据 | 处置 |
| --- | --- | --- | --- |
| `echo-desktop/src/player/session.rs:119-121` | `PersistedHistoryRecord::Legacy(String)` 变体 | 本机 `desktop-state.json` 的 `playbackSession.version = 3`，history 元素已是 `{entry_id, played_at_ms}` | 删变体 + `parse` 的 `Legacy(_) => None` 分支(134) |
| `session.rs:405` | `Ok(session) if matches!(session.version, 1..=SESSION_VERSION)` | 放宽范围以接受 v1/v2 | 收紧为 `== SESSION_VERSION`（或保留 `1..=`，见下） |
| `session.rs:622` | 测试 `old_session_without_priority_restores_with_an_empty_lane` | 构造 `{"version": 2, …}` | 保留：它验证的是 `priority` 字段的 `#[serde(default)]`，**不是** history legacy 兼容；只删掉其中 history 的 legacy 断言（若有） |
| `echo-desktop/src/platform/local_state.rs:224-226` | `RawDoc.version` 字段 | 注释自承「not used before 0.1.0」；**全仓无任何比较**，只有解析(534-538)与写回(596) | **最低价值项**，建议删字段并让 `parse_doc` 忽略该键（`unknown` flatten 已能保留未知键） |

合计约 **60 行**。

> ⚠️ `local_state/tests.rs:470,486` 断言 `raw["version"] == 7` 与 `futureDesktop` 一起「在 legacy mutation 后存活」。删 `version` 字段前必须确认该断言改为只断言 `futureDesktop`/`effects` 存活，否则会红。

---

## 6. 治理与验证路径

### 6.1 OpenSpec 流程（AGENT.md 强制）

1. **propose**：新建 change（建议名 `remove-legacy-format-compatibility`），产出 `proposal.md` / `design.md` / `tasks.md` / `specs/` delta。propose 阶段**不写代码**。
2. **apply**：按 `tasks.md` 实施，逐组验证。
3. **sync**：把 delta spec 同步进主规格并验证。
4. **archive**：归档 change（**即使无 delta spec 也要先归档**）。
5. **PR**：归档完成后再开 PR，并关联 Issue。

### 6.2 delta spec 的现实约束（重要）

- 本仓 archive 中 delta 只用过 `## ADDED Requirements`(64 次) 与 `## MODIFIED Requirements`(47 次)，**从无 `## REMOVED` 先例**。
- 场景 ID 是**顺序派生**的：`<AREA>-R<需求序号>-S<场景序号>`（`scripts/verify/spec-scenarios.mjs:83`）。**在中间删需求或场景，会让其后所有 ID 顺移**。
- 以 `library-durability` 为例（10 个需求 / 23 个场景）：若删掉 LD-04 与 LD-09，则 LD-05→LD-04、LD-06→LD-05、LD-07→LD-06、LD-08→LD-07、LD-10→LD-08，其下所有场景 ID 一并重编号。
- `openspec/changes/enforce-library-first-durability/tasks.md:306`（task 10.1）确立的纪律是**追加到末尾**，正是为了避免重编号。

**因此有两个选项，须在 propose 阶段二选一：**

| 选项 | 做法 | 代价 |
| --- | --- | --- |
| **① 删需求 + 重编号** | 用 `## REMOVED Requirements`（本仓首创），接受该 area 内 ID 顺移 | 一次性重编号：`traceability.md`、`scenario-commands.mjs`、`manifest.json`、`tests/scenarios|native/*`、`PRD-matrix.md` 全量联动；大 diff，易错 |
| **② 只改需求文本，不删需求**（推荐） | 保留 LD-04 / LD-09 的 Requirement 外壳，把文本收窄为「只保证已支持格式的**只读**兼容；不再保证任何格式升级路径」 | diff 小；但规格语义需精确措辞，且 `LD-09b` 类场景必须改写而非删除 |

### 6.3 场景变更时的同步面（8 处）

任一场景被删/改/重编号，需同步：

1. `openspec/specs/<area>/spec.md`（主规格）
2. `docs/traceability.md`（人工维护的映射源）
3. `scripts/verify/scenario-commands.mjs`（人工维护的 ID→命令表）
4. `scripts/verify/manifest.json`（生成物）
5. `tests/scenarios/<ID>.yaml` 或 `tests/native/<ID>.md`（生成物）
6. `docs/acceptance/PRD-matrix.md`（人工维护；若该 area 属于 A1–A14 家族）
7. 本 change 的 delta spec 与 `tasks.md`
8. 本 change 的 `evidence/` 证据文件

生成物用 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成。**注意：不带 `--write` 的 dry-run 只打印漂移报告并以 exit 0 结束**（脚本 `:16-17` 头注释「exits nonzero if the tree would drift」与实现不符，勿当校验门）；漂移由治理门对账拦截，重生成后以 `pnpm verify:governance` 验证。

### 6.4 涉及的具体场景（S 档需要动规格）

| 场景 | 标题 | 当前命令 | 影响 |
| --- | --- | --- | --- |
| `SYN-R07-S03` | 新作者不能抬高旧作者统计桶 | `legacy_play_stats_payload_still_authorizes_the_buckets_it_recorded` | 该测试**构造的就是旧形状载荷**（`{song_uuid, by_device}`）。收紧 `decode_play_stats` 后此测试与场景必须同步改写为「新形状载荷 + 越权桶被拒」 |

A/B/D 三档**不涉及任何规格场景**——已逐个核对 `scripts/verify/manifest.json` 与 `scenario-commands.mjs`，无一条命令引用 `portable_migration` 的测试名、`apply_portable_upgrade`、`backfill_legacy_play_counts`、`through_v27` 或 `confirm_candidate`。

### 6.5 验证命令

```bash
# 分档验证
cargo test -p echo-core --all-features
cargo test -p echo-desktop --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
pnpm --dir apps/desktop test

# 治理门（必须整门独占运行，勿与其他 cargo/扫描并行）
pnpm verify:governance

# 单场景核对（注意：`-- <id>` 会被当成 id，用 node 直调）
node scripts/verify/run-scenario.mjs LD-R09-S01
```

> 治理门运行时坑见记忆：整门须独占运行（并行 cargo 或只读扫描都会让 5 秒墙钟断言挂）；`pnpm verify:scenario -- <id>` 场景 ok 但整体 exit 1，单场景用 `node` 直调。

---

## 7. 未决判定点（必须先回答）

仓库 `github.com/zsxink/Echo` 是**公开**的，README 提供 macOS/Windows/Linux 下载，git tag 已有 `v0.1.0 v0.1.1 v0.1.2 v0.1.3 v0.1.4 v0.2.0 v0.3.0`。这与「个人使用」前提存在张力。

**问题**：这些兼容代码要保护的只是你自己的库，还是也包括下载了公开版本的外部用户？

| 回答 | 影响 |
| --- | --- |
| **只保护自己** | A/B/D 可全删；S 档按 §4 执行（含新增一条 `DROP COLUMN` 迁移） |
| **也要保护外部用户** | A 档仍可删（主规格本就不要求升级路径，且删除后旧库仍可**读取**）；B 档的 `apply_root_sync_contexts` 建议保留（它保护的是 v18 时代库的迁移路径）；D 档建议保留 `1..=SESSION_VERSION` 读取范围 |

**建议**：即便对外部用户负责，**A 档仍可删**——因为删除的是「v1→v2 记录升级写入器」，而**读取** v1 的能力（`PortableRecordDocument::V1`）完整保留。外部用户的旧库依然能打开、能被读取、能被新协议继续写入（`write_record` 在 manifest 为 v1 时走 flat 分支）。

---

## 8. 实施顺序与依赖

依赖关系（箭头 = 必须先做）：

```
B 档（回填/特例）──┐
                   ├──> S 档 capture_legacy_plays 删除
D 档（独立）       │
A 档（独立）       └──> S 档（其余）
```

1. **第 1 步 · B 档**：先确认本机 `legacy_play_counts_backfilled = 1`（已确认）→ 删特例函数与回填。
2. **第 2 步 · A 档**：删迁移写入器，保留 IO 原语；同步改写 3 个保留测试。
3. **第 3 步 · S 档**：删 `capture_legacy_plays`（依赖第 1 步已完成回填）、`replica_enrollment_archive`、`confirm_candidate` 包装；**新增 `DROP COLUMN` 迁移**并单独验证 schema 升级路径；改写 `SYN-R07-S03` 场景与测试。
4. **第 4 步 · D 档**：删 `PersistedHistoryRecord::Legacy` 与 `RawDoc.version`，同步改测试断言。
5. **第 5 步**：`gen-scenario-manifests.mjs --write` 重生成 → `prd-matrix.mjs` 校验 → `pnpm verify:governance` 整门。

> 建议**一档一个 commit**，便于 `git bisect` 与回滚。

---

## 9. 备份与回滚

**动手前**（退出 Echo，确保无写锁）：

```bash
# 1. SQLite + 桌面状态
cp -a "$HOME/Library/Application Support/com.zsxink.echo" \
      "$HOME/Library/Application Support/com.zsxink.echo.bak-$(date +%Y%m%d-%H%M%S)"

# 2. 音乐库控制面（media/ 可不必复制）
cp -a "$HOME/Music/音乐库/echo" "$HOME/Music/音乐库/echo.bak-$(date +%Y%m%d-%H%M%S)"
```

**回滚**：本方案不改动已发布迁移脚本，也不改写任何库内数据；回滚 = `git revert` 对应 commit + 恢复上述备份目录。

**数据存活性论证**（删除后本机数据仍在）：

- manifest 为 v3 → `is_compatible()` 通过，目录可写。
- 记录：1315 条 flat v1 + 46 条 v2；`PortableRecordDocument::V1` 读取路径保留 → **全部可读**。
- 收藏 34、歌单 14、playlist-items 449 全部是 flat v1 → **可读**。
- 墓碑 12 条全在 legacy flat 布局 → `read_tombstone_document` 的 `None` 分支与 `delete_tombstone` 的 legacy 分支**保留** → **可读可删**。
- `echo/` 下**无 `migrations/` 目录** → 无待恢复 journal，删除 `recover()` 不会丢任何待办。

---

## 10. 交付检查清单

- [ ] §7 判定点已回答并记录在 `proposal.md` 的非目标/范围中
- [ ] `lan-library-sync/spec.md:269/:272` 的「历史库升级」条款已在 proposal 中显式对齐（A 档删除升级写入器后该场景 WHEN 前提不可达）
- [ ] OpenSpec change 已 propose 并评审
- [ ] 备份已完成并验证可读
- [ ] 四档按 §8 顺序实施，逐档 commit
- [ ] 新增的 `DROP COLUMN` 迁移有升级、失败恢复与兼容测试（CODE_STANDARDS §5 / §9.1）
- [ ] CODE_STANDARDS §9 要求的「移除实现清单 + 引用检查 + 静态检查 + 测试」证据齐全（本仓无自动化残留检查门，属人工/评审义务）
- [ ] 场景若有变更：§6.3 的 8 处同步面全部更新
- [ ] `pnpm verify:governance` 整门独占运行通过
- [ ] delta spec 已 sync 到主规格并验证
- [ ] OpenSpec change 已归档（先于 PR）
- [ ] PR 已关联 Issue
