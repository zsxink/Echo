# Shared data inventory (implementation baseline)

This inventory records the current repository state before LAN sync work. Paths
are implementation evidence; they do not claim that the LAN protocol already
exists. `echo/records` is the current portable continuation format, not a
network protocol.

| Shared fact/resource | Current write path | SQLite/outbox state | Portable record/resource state | Gap to close |
|---|---|---|---|---|
| Song identity, tags, media path/hash, added time | `SqliteTx::upsert_song` → `infrastructure/sqlite/statements.rs::upsert_song`; import and scan use the repository | Full song snapshot enters `sync_outbox` in the same SQLite transaction | `SongRecord` in `domain/library.rs`; emitted by import, recovery, and portable materialization | Audio bytes have a hash/path, but there is no library-authorized resource manifest or network resource lifecycle. |
| Favorite and favorite time | `set_song_favorite` in `statements.rs`, reached by `application/favorite.rs` and desktop favorite service | Independent favorite snapshot enters outbox transactionally; favorite is not folded into song snapshot | `FavoriteRecord`; desktop `runtime/services/favorites.rs` writes it after commit | Portable file fan-out is separate from SQLite commit and can fail; server confirmation/application does not exist. |
| Playlist identity/name | `statements.rs::create_playlist`; desktop playlist service | Playlist mutation enters outbox transactionally | `PlaylistRecord` | Playlist `cover_asset_key` is SQLite-only and absent from `PlaylistRecord`. |
| Playlist membership and append order | `statements.rs::add_member`; `SqliteDatabase::remove_member` in `infrastructure/sqlite/mod.rs`; `playlist_songs.position` and `member_uuid` | Add/remove write playlist-scoped member-set snapshots; removal and playlist deletion also write SQLite tombstones in their transaction. No independent playlist-item outbox revision/HLC is created by these paths. | Desktop `add_to_playlists` and `remove_playlist_song` write `PlaylistItemRecord`/tombstone after the SQLite transaction; `PlaylistItemRecord` and continuation support typed member identity | DB and portable fan-out are split across commits; item identity, aliasing and outcomes need one recoverable transaction. |
| Song metadata/lyrics override | `song_overrides` table and continuation/materialization scaffolding | Schema revision/HLC and `KIND_OVERRIDE` exist, but no production user override mutation/write entry was found | `OverrideRecord` carries title/artist/album/lyrics; continuation validates/reports but does not project it | No current mutation use case; when added, user-cover reference and asset identity must be represented too. |
| Per-device play counts | `record_playback` transaction; desktop `runtime/player/recorder.rs::record_play_stats` writes portable data after the count commits | No corresponding `enqueue_sync` call for play-stats was found; SQLite count and portable write are not one transaction | `PlayStatsRecord` with per-device monotone buckets exists | A crash between DB commit and portable write can leave the portable record behind; authenticated writer ownership and LAN merge are absent. |
| Final song/playlist deletion | song delete/finalize helpers in `statements.rs`; `SqliteDatabase::delete` and `remove_member` in `infrastructure/sqlite/mod.rs` | Song, playlist, and member SQLite tombstones are written in the local SQL transaction; song delete outbox is also written at finalization | `TombstoneRecord` exists; continuation applies tombstones; desktop/application services fan out portable tombstones after SQL commit | Portable fan-out is not atomic with SQL. `application/delete.rs` writes a portable song tombstone immediately after setting `PendingDelete` and removes it on undo, conflicting with the LAN rule that the 10-second window is local intent only. |
| Audio and same-basename `.lrc` | Import/delete journal owns staging and trash operations; media path is under `media/` | Operation journal captures local filesystem recovery, not peer transfer state | Song record identifies audio path/hash; lyrics can be embedded, sidecar, or override | No typed resource manifest, sidecar hash/length, resumable transfer, peer reference retention, or resource publication cursor. |
| Song/playlist/artist artwork | Services call `CoverCache::put`; SQLite stores content-hash/key mappings in `cover_assets`, `playlists.cover_asset_key`, and `artist_covers`; image bytes live in the disk CoverCache | No matching outbox enqueue found in the current write paths | No artwork resource reference in `SongRecord`, `PlaylistRecord`, or `OverrideRecord` | Artist cover is user-selected artwork and falls under the spec's user-cover/resource boundary; all three artwork references and bytes need portable/resource projection. |
| Display/sort preferences and playback session | Desktop-state and UI preference stores | Not part of catalog outbox | Not in portable records | These remain device-local by design. Playlist member append order is shared data and must not be confused with local display sorting. |

## Existing fixtures and limits

- `crates/echo-core/src/domain/library.rs` has serialization round-trip tests for
  song, favorite, playlist item, override, play-stats, and tombstone shapes.
- `crates/echo-core/src/application/continuation/tests.rs` has portable
  continuation fixtures for UUID preservation, favorite, playlist/member order,
  play stats, and tombstone precedence.
- `crates/echo-core/src/infrastructure/sqlite/tests/sync_payloads.rs` checks
  current local outbox payloads and delete/favorite behavior.
- These are source-level unit fixtures, not pinned cross-version network
  protocol fixtures. No mutation, commit, snapshot, resource, alias, checkpoint,
  or digest-chain fixture exists yet.

## Completion status

Task 1.2 audit is complete. This is a baseline inventory, so its gap column
records what was missing before implementation. Follow-on work has since moved
play-stat bucket capture into the SQLite transaction and stopped publishing
PendingDelete as a shared tombstone; the SQLite ledger and domain state machine
track final delete intent. Remaining implementation is tracked in tasks 3.3–4.5:
portable recovery, artwork/sidecar resource manifests and transfer lifecycle,
plus atomic membership and alias handling. These baseline rows must not be read
as proof that the remaining resources already synchronize.

## Corrections measured against the baseline (2026-10-18)

The table above is a pre-implementation baseline, so three of its rows no longer
describe `main`. Each correction below is reproducible from the cited lines.

- **Per-device play counts.** `record_playback` does enqueue a play-stats
  mutation (`infrastructure/sqlite/mod.rs`, the `KIND_PLAY_STATS` snapshot in the
  same transaction as the count), and the payload now carries the complete
  `PlayStatsRecord` — `revision`, `updated_by_device_id`, `hlc`, `song_uuid`,
  `by_device`. Host merge is `domain/sync.rs::merge_authorized_play_stats`, which
  accepts the legacy `{song_uuid, by_device}` shape for rows written by earlier
  releases, so an existing library is not permanently rejected.
  `sqlite/tests/sync_payloads/deletion_and_stats.rs:87` asserts the producer
  shape and drains the queue through the host port.
- **Playlist membership.** `SqliteDatabase::remove_member` enqueues a typed
  `playlist-item` delete before erasing the row, which is what lets the host's
  `exclude_final_playlist_members` filter the member out of the append-only
  union.   `deletion_and_stats.rs:118` asserts one tombstone, `:205` covers a delete
  queued before its add is acknowledged, and `:234` covers retrying the old
  delete after the member was re-added.
- **Playlist deletion.** `PlaylistRepository::delete` enqueues a typed
  `playlist` delete (guarded by `EXISTS` so a replay stays idempotent) instead of
  only writing the local tombstone table.
