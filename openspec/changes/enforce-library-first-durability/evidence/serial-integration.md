# Serial integration checkpoint

Date: 2026-10-23. Branch: `codex/issue-72-library-first-durability`.

This checkpoint integrates restored implementation WIP. It does not establish
completion of the library-first change, a releasable migration, or native-platform
durability guarantees. No commit, archive, or task-checkbox update was made by
this worker. The original stash was preserved by the parent agent.

## Repairs made

- Replaced duplicate `ControlPlanePort` forwarding methods with calls through
  its optional `durability()` capability. Missing capability fails explicitly.
- Corrected the SQLite song projection to use `RelativeMediaPath::normalized()`.
- Production desktop assembly now shares the same `RootControlPlane` with the
  SQLite adapter and injects `LibraryHistoryPort` into `ScanDeps`.
- Completed missing legacy-test initializers for optional playlist creation time,
  cover reference, member addition time, and the history dependency. Absent
  historical values remain `None`.
- Registered the restored playback application module. Removed session identity
  from shared `PlayStatsRecord`; playback events are private typed durability
  state, included in directory batches and SQLite recovery/export state.
  Recorded timestamps use Unix milliseconds. A regression test covers a saved
  event whose projection fails, retry without another count, and absence of
  session identity from the shared statistics record.
- Added a structured `ImportResultDto::SavedPending` conversion retaining the
  operation, song, path and error classification. Updated the Rust IPC generator
  and ran it; the generated TypeScript union includes `savedPending`.
- Reused the restored sync-state enrichment helper in the existing commit
  preparation path, eliminating its dead-code warning without suppressing it.

## Actual verification

All commands below executed successfully (exit code 0) on this development
checkout. Final production checks completed without warnings.

| Command | Result |
| --- | --- |
| `cargo check -p echo-core` | Passed |
| `cargo check -p echo-desktop` | Passed |
| `cargo check -p echo-app` | Passed |
| `cargo fmt --all --check` | Passed |
| `cargo check -p echo-core --tests` | Passed; type-check only |
| `cargo check -p echo-desktop --tests` | Passed; type-check only |
| `cargo run -p echo-desktop --bin echo-generate-ipc` | Passed; generated file written |
| `cargo test -p echo-core play_stats_record_round_trips_through_json --lib` | 1 passed, 0 failed |
| `cargo test -p echo-core library_durability --lib` | 4 passed, 0 failed, 675 filtered |
| `git diff --check` | Passed |

The four durability tests are the three restored scan contracts (save before
projection/reuse identity, refuse unwritable library, preserve record when media
disappears) and the playback-state regression described above. These are memory
adapter tests; they do not exercise native fsync, rename, locking, GC, or a full
SQLite wipe/rebuild.

## Boundaries still requiring implementation and verification

- `commit_then_project` still invokes per-operation projection closures. The
  application does not yet force every entry point through ordered projection
  catch-up from the SQLite watermark. Local candidate capture must be completed
  before claiming that outbox facts survive loss of SQLite.
- The filesystem journal still retains its full chain and checkpoint map;
  bounded batching, compaction/GC, read-only browsing without a write lock, and
  complete per-boundary fault injection remain required.
- Projection of dangling relationships, stable typed recovery-state keys and a
  retry/read-back path remain incomplete. Existing `Debug` state keys are not
  accepted as completion of task 4.5.
- The playback use case is now compiled but is not wired into the production
  desktop recorder; the existing public database-first playback API remains.
  This checkpoint does not complete task 6.3 or 6.6.
- Legacy rescue/migration fencing, private/portable snapshot completeness, v3
  format compatibility, resource retention, confirmed/pending reconstruction,
  mutation adoption and native recovery matrices are not established by these
  checks. Historical playback-event capture is available in the state exporter,
  but the record-only history port is not a complete migration snapshot.
- Frontend handling/message keys/retry policy for new persistence results,
  TypeScript checks and user-visible partial recovery remain for subsequent
  tasks. Generating the new IPC union does not prove consumers handle it.

No workspace Clippy, full workspace tests, frontend gate, governance gate,
independent architecture review, clean-checkout baseline, or Windows/Linux native
verification was executed by this worker in this checkpoint.
