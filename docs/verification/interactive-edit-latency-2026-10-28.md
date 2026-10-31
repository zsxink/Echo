# Interactive edit latency verification — 2026-10-28

## Result

The release `AppServices` mutation path stayed below the 50 ms P95 target for both favorite and empty-playlist creation with 1,000- and 10,000-song synthetic metadata libraries. Each command had 200 timed samples. The measurement includes the durable directory commit, any required foreground SQLite catch-up, planning, and response construction. Background projection is flushed and checked after the timed loops.

| Fixture | Operation | P50 | P95 | P99 | Max |
| --- | --- | ---: | ---: | ---: | ---: |
| 1,000 songs | Favorite | 23.03 ms | 25.07 ms | 30.94 ms | 50.21 ms |
| 1,000 songs | Create playlist | 25.99 ms | 31.91 ms | 41.11 ms | 45.06 ms |
| 10,000 songs | Favorite | 35.96 ms | 37.94 ms | 57.34 ms | 61.58 ms |
| 10,000 songs | Create playlist | 39.04 ms | 41.71 ms | 49.10 ms | 50.11 ms |

Commands (both used `ECHO_PERF_HISTORY=10`, `ECHO_PERF_SAMPLES=200`, Release mode, and `ECHO_PERF_MAX_P95_MS=50`):

```text
ECHO_PERF_SONGS=1000 ECHO_PERF_HISTORY=10 ECHO_PERF_SAMPLES=200 ECHO_PERF_MAX_P95_MS=50 cargo test -p echo-desktop --test local_edit_performance --release -- --ignored --nocapture --test-threads=1
ECHO_PERF_SONGS=10000 ECHO_PERF_HISTORY=10 ECHO_PERF_SAMPLES=200 ECHO_PERF_MAX_P95_MS=50 cargo test -p echo-desktop --test local_edit_performance --release -- --ignored --nocapture --test-threads=1
```

Both commands exited 0 after enforcing the P95 limit. Fixture preparation (about 9 seconds and 66 seconds respectively) is outside timed samples.

## Implementation verified

- Interactive writes reuse the authenticated journal replay and checkpoint file index while the transaction-directory fingerprint is unchanged.
- Favorite, playlist creation, and playlist membership edits acknowledge after the durable directory marker; SQLite projection is rebuildable and runs asynchronously.
- When SQLite is behind, these local edit lanes apply only the authenticated journal suffix after the matching SQLite watermark. They avoid replaying a large checkpoint and avoid a global unresolved-record retry on each edit. Cold recovery, checkpoint-behind recovery, and other edit types keep the full projection path.
- The projection worker reads the authenticated journal without holding the directory writer lock. Its quiet period coalesces bursts; a bounded 30-second maximum delay prevents a busy UI from repeatedly starting large projection transactions.
- The active journal remains bounded at 512 entries. Approaching the cap performs durable materialization and compaction before accepting another entry.

## Scope and limits

The fixtures contain 1,000 and 10,000 synthetic portable song metadata records with no real audio files. The timed boundary starts at `AppServices`, so it includes the backend command body but excludes React event scheduling, the native Tauri IPC transport, and browser painting. React behavior tests for favorite mutation/synchronization and playlist creation passed 32/32; they use mocked IPC and are not click-to-feedback measurements. SQLite eventual projection was verified after each run, but per-operation P50/P95/P99 projection lag was not instrumented. The 50 ms result therefore verifies the production service/durable-commit path on these synthetic fixtures, not the full native-window acceptance requirement in OpenSpec 13.5.

Environment observed: macOS Darwin 27.0, arm64, temporary fixture on local journaled APFS. The DiskManagement framework was unavailable for identifying the CPU model. No real user library or audio was accessed.

Additional checks: `cargo test -p echo-core library_durability` passed 257/257; `cargo test -p echo-core library_projection_worker` passed 6/6; the active-journal-limit test passed 1/1; the watermark/suffix consistency test passed 1/1; `cargo check -p echo-desktop` and `git diff --check` passed; React favorite and playlist tests passed 32/32. `cargo clippy -p echo-core --all-targets -- -D warnings` remains blocked by three diagnostics in other changed files outside this optimization (`sqlite/sync/durability.rs` line 127, `filesystem/registry.rs` line 198, and `durability_projection/sync_state/tests/boundaries.rs` line 159). `pnpm` could not start because its pinned package-manager self-install attempted to write under `~/Library/pnpm`, outside the workspace; the installed `apps/desktop/node_modules/.bin/vitest` ran the targeted React tests directly.

## Playlist membership follow-up — 2026-10-28

Membership planning now reads playlist and member records from the already-verified journal replay instead of reopening portable record files. The append path also avoids building the same complete playlist snapshot twice. The benchmark runs in Release mode, flushes projection before each membership sample, and includes a 20-operation preparation history plus the preceding favorite/create measurements.

| Fixture | Operation | Samples | P50 | P95 | P99 | Max |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1,000 songs | Add to playlist | 30 | 22.50 ms | 22.50 ms | 30.28 ms | 30.28 ms |
| 1,000 songs | Remove from playlist | 30 | 24.46 ms | 24.46 ms | 29.27 ms | 29.27 ms |
| 10,000 songs | Add to playlist | 30 | 35.79 ms | 37.87 ms | 40.01 ms | 40.01 ms |
| 10,000 songs | Remove from playlist | 30 | 35.93 ms | 37.89 ms | 44.15 ms | 44.15 ms |

The later 10,000-song row also measured favorite at 37.21 ms P95 and playlist creation at 49.35 ms P95. A fresh default Release run (3,000 songs, 64 existing operations, 100 samples per operation) measured favorite P95 28.09 ms, playlist creation P95 34.04 ms, add P95 31.81 ms, and remove P95 32.49 ms. All four operations met the 50 ms P95 target in both runs.

Import inspection found and fixed the rescan bug. A successful import row existed in SQLite but had `availability = Missing`, so the catalog's available-song query excluded it until a scan restored availability. The import transaction now explicitly sets the reserved song to `Available` alongside the metadata/resource commit. The real-file integration benchmark verifies the row is `Available` and that the catalog count increases after each import, without scanning. The import-refresh regression was also tightened so its test exercises `useImport`'s invalidation instead of invalidating a second time from the test shell.

The slow import path was in the standalone directory commit used for both the reservation and final song record. That path repeated strict recovery and materialized-tree verification even though import already had an authenticated write session. Import reservation and final song commit now use the exclusive session fast path; other scan and watcher commits retain strict standalone verification. This removed the measured 1.8–2.8 second cold outliers.

Six actual short audio fixtures (7–88 KB, MP3/FLAC/M4A/OGG/Opus/WAV) were each imported once through `AppServices::import_single_path` in a Release benchmark with a 3,000-song library. These are individual observations, not a P95 distribution:

| Format | Size | Import time |
| --- | ---: | ---: |
| MP3 | 17 KB | 424 ms |
| FLAC | 21 KB | 448 ms |
| M4A | 16 KB | 454 ms |
| OGG | 7 KB | 426 ms |
| Opus | 15 KB | 426 ms |
| WAV | 88 KB | 444 ms |

The benchmark confirms each imported song is immediately `Available` and appears in the catalog without rescanning. A traced 1,000-song run after the change measured 122–140 ms per import; reservation directory commits took 19–24 ms and final `commit_song` took 48–55 ms. The 3,000-song, 100-operation run measured 424–454 ms per import, with no remaining second-scale outlier. These are individual fixture observations, not an import P95 distribution. Import remains a multi-step durable file operation and is not expected to meet the 50 ms interaction target. Measurements use the single-file service entry point and exclude native picker/IPC time; multi-select scheduling is not included.
