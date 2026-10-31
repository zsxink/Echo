# Persistence entrypoint evidence

This inventory separates the implementation observed in the repository on 2026-10-23 from the target contract. Paths and line numbers are navigation aids; re-run CodeGraph after implementation because symbols and line numbers can move. These observations are not proof of end-to-end recovery.

| Entry | Current path and behavior | Target behavior and required proof |
| --- | --- | --- |
| Local import | `crates/echo-core/src/application/import/execution.rs` publishes and verifies staged media, then `commit` updates the database; the portable `SongRecord` is written later in the same Core import method. Existing operation journal records import resource steps, but is not the durable user-data authority. | Publish/verify resources → commit reserved UUID, added time, song image and resource claims to the library journal → project SQLite/outbox → return complete. Fault points around every publish, directory sync, commit marker and DB commit; recover with same operation ID. |
| Scan and refresh | `crates/echo-core/src/application/scan/mod.rs::persist_parsed_index` updates SQLite song/index, lyrics and cover state; new/changed Song rows are not first committed through a directory authority. | Persist stable identity and changed business fields before projection. A no-change scan creates no candidate; missing media changes local availability only. Verify wipe/rebuild after scan and read-only/control-plane failure. |
| Favorite toggle | `crates/echo-desktop/src/runtime/services/favorites.rs::set_favorite` calls the SQLite `SetFavorite` use case, then writes a `FavoriteRecord`. A portable write error can follow a committed DB change. | Persist both true and false (or typed deletion fact) with stable identity before SQLite projection. Verify true→false round-trip after removing DB and failure when `echo/` is unavailable. |
| Playlist create/rename/delete | `crates/echo-desktop/src/runtime/services/playlists.rs::{create_playlist,rename_playlist,delete_playlist}` mutate SQLite via Core repository and then materialize portable records/tombstones. Multi-record effects do not share one library commit boundary. | Persist complete playlist definition, timestamps and cascade set as one recoverable library commit before projection. Include empty playlist, delete/undo and partial materialization recovery. |
| Playlist members | `crates/echo-desktop/src/runtime/services/playlists.rs::{add_to_playlists,remove_playlist_song,materialize_members}` and `crates/echo-core/src/infrastructure/sqlite/repos_catalog.rs::add_member` show SQLite mutation followed by member-record materialization; member identity is minted in SQLite repository. | Reserve member UUID, position and known joined-at time in the durable change set; commit relation/tombstone before projection. Verify ordering, deduplication, retry and cascade cases after DB loss. |
| Playlist cover and overrides | Playlist cover handling is in `runtime/services/playlists.rs::set_playlist_cover`; portable playlist fields do not yet make the cached image an independently recoverable resource. Song overrides are projected by continuation/materialization paths. | Save content-addressed original bytes plus object references in `echo/assets/` before DB projection. Cache keys/thumbnails remain derived. Clear app caches and DB to verify recovery; preserve missing-resource gaps. |
| Playback statistics | `crates/echo-desktop/src/runtime/player/recorder.rs::record_play_stats` reads/merges SQLite device counters and writes a portable `PlayStatsRecord` after the DB update; recorder errors do not stop playback. | Commit stable completion event and device bucket to library first, then project. Playback remains uninterrupted on a persistence failure, but UI/log reports unsaved or retryable stats; replay/ACK/reinstall must not double count. |
| Song delete/undo and writeback | Core delete/undo is under `crates/echo-core/src/application/delete.rs`; filesystem activity is coordinated with operation state and SQLite. Existing portable materialization is not a shared library commit for all related objects/resources. | Persist local undo intent, resource claim and final object set with typed tombstones in one recoverable library boundary; preserve old bytes around explicit writeback. Verify 10-second terminal outcome, interrupted replacement and retained resource references. |
| Startup continuation | `crates/echo-core/src/application/continuation.rs::ContinueFromRecords::run` reads portable records and projects songs, favorites, playlists, members, stats and overrides in dependency order. It degrades when the control plane is not usable, but currently cannot reconstruct data that was only in SQLite. | Recover and validate format/journal/committed chain before projection or scan; preserve corrupt/unknown/orphan evidence and report partial recovery. Verify empty-DB reconstruction, migration replay, read-only behavior and no fabricated history. |
| LAN inbound snapshot and delta | Desktop `src-tauri/src/sync_provisioner/remote_apply.rs` and `crates/echo-core/src/infrastructure/sqlite/sync/sync_apply.rs` apply the remote ledger/business data in SQLite. Resource transfer has its own publication/checkpoint path; neither is the common library commit. | Validate complete snapshot before visibility; commit remote objects, cursor, inbox/receipt and resource declarations to the receiving library, then project DB state. Verify failed pages do not advance watermarks and object-before-media stays unavailable. |
| Source ACK and host confirmation | Sync ACK/apply/host confirmation currently flow through SQLite sync adapters and desktop provisioner; portable record writes do not cover the full receipt/ledger transaction. | Persist exact confirmed payload, operation identity and required publication facts to library before returning success ACK or exposing a snapshot/delta. Drop DB after ACK and verify business recovery; separately require trusted ledger/checkpoint to resume hosting. |
| Root join / first sync | Current product contract is `openspec/specs/lan-library-sync/spec.md`, requirement “首次加入本地优先合并”: retain managed local data and media, queue local objects, then validate and merge host snapshot. Do not use older empty-directory wording in product docs. | Durability work preserves this merge policy while changing local persistence order. Verify cancellation and snapshot failure leave local data intact; no peer trust is inferred from library records. |

## 复核（2026-10-25）

上表是 2026-10-23 的观察快照，其中若干"当前行为"已被本分支的未提交实现取代。
以 [交接](handoff-20261025.md) 与 [tasks.md](../tasks.md) 的逐项记录为准，本表仅作导航。

- **扫描与刷新**：现状已不是"仅更新 SQLite"。`application/scan/durable.rs::commit_song`
  已在目录侧提交 Song 后再投影，改名路径 `application/watch.rs::rename_path` 也改走
  `commit_song`。但"无变化扫描不产生候选"仍未成立（目录侧抑制了对象版本，SQLite 侧
  仍铸出同步候选），且扫描后 wipe 重建没有用例——表中"扫描后 wipe/重建"这一验证项
  依旧缺失。
- **喜欢切换、歌单创建/改名/删除、歌单成员**：这些入口已接入共享提交协调器
  `infrastructure/sqlite/local_library_edit.rs::plan_library_change`（目录提交在前、
  SQLite 投影在后），不再是"先改库再写记录"。但**歌单创建的重试仍会丢失稳定操作身份**：
  重试时协调器的追赶失败被原样返回（`local_library_edit.rs:235-249` 只对播放事件做
  `saved_pending_projection` 映射），追赶恢复后又会因新铸的 `PlaylistId` 撞上
  `playlists` 的 `UNIQUE (library_root_uuid, normalized_name_key)`。
  该缺陷已用 RED 用例固定（`tests/lifecycle/playlist_create.rs`，**已解除 `#[ignore]`**，
  当前 `cargo test -p echo-core --lib` 按设计带 1 个失败）。
- **启动续跑**：`application/continuation.rs` 仍直写业务表绕过水位追赶，表中
  "cannot reconstruct data that was only in SQLite" 的旧描述已不准确，但"未走统一
  协调器"这一差距仍在。
- **主机确认 / ACK（"Source ACK and host confirmation" 行）**：本分支新增了目录优先的确认提交
  路径（`crates/echo-core/src/infrastructure/sqlite/sync/durability/commit.rs`，`origin/main`
  上不存在），但其中的 `commit_prepared_projection` 只在 `#[cfg(test)]` 下容忍缺失控制面，
  而 echo-core 作为 `echo-sync` 的依赖编译时 `cfg(test)` 为假 ⇒ `ledger_protocol` 2/2 失败，
  `pnpm verify:governance` exit 101。该行"portable record writes do not cover the full
  receipt/ledger transaction"仍是未完成项，但**当前还多出一处分支引入的回归**须先修；
  详见 [交接](handoff-20261025.md) 的"缺陷 4"。
- 其余行（导入、封面、播放统计、删除/撤销、LAN 入站、ACK、首次加入）未在本分支改动，
  仍按 2026-10-23 的观察成立。

当前状态、逐项差距与修复形状见 [交接](handoff-20261025.md)；依赖关系与排期见
[任务依赖 DAG](task-dag-20261025.md)（其依赖边未经编译或测试执行验证，只作排期参考）。

## Cross-cutting write contract

Expected production path: typed Core change set → durable library commit (stable operation ID, sequence/predecessor, complete after-image and resource declarations) → ordered idempotent SQLite projection catching up from the recorded watermark (not only the latest operation) → user-visible complete result. If the library commit succeeded but a later step failed, return a saved-pending state tied to that same operation; if the durable commit itself failed, do not leave a DB-only user fact. LAN receipt, credentials and host authority remain distinct from recoverable user data. Legacy DB contents may be used only by an audited rescue migration while still available. A projection must tolerate dangling parents: resolvable rows land in the tables, unresolved relations stay in `library_recovery_records` and count as partial recovery instead of aborting the whole rebuild. Shared/sync payloads never carry playback session or event identity; that idempotency fact lives in `echo/state/playback-events/`.

## Verification command inventory

These are repository commands to use when their corresponding implementation exists; listing them is not evidence that they were run or passed.

| Concern | Relevant command or check |
| --- | --- |
| Durable protocol / migration / recovery | `cargo test -p echo-core library_durability`; `cargo test -p echo-core migration`; `cargo test -p echo-core continuation` |
| Import / scan / playlist / favorite / delete / playback | `cargo test -p echo-core import`; `cargo test -p echo-core scan`; `cargo test -p echo-core playlist`; `cargo test -p echo-desktop favorite`; `cargo test -p echo-core delete`; `cargo test -p echo-desktop recorder` |
| LAN sync protocol | `cargo test -p echo-core sync`; `cargo test -p echo-sync` |
| Architecture and repository gates | `cargo test -p echo-core --test arch`; `node scripts/verify/check-scale.mjs`; `pnpm verify:governance` |
| Formatting, lint, full tests and OpenSpec | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features`; `pnpm exec openspec validate --all --strict` |
| Native filesystem behavior | Manual/integration gates on macOS, Windows and Linux for directory flush, atomic replacement, permission/read-only, locked resource, long/Unicode path and crash/restart cases. No three-platform execution evidence is recorded by this inventory. |

