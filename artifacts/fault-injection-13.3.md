# Echo 0.1.0 fault-injection report (task 13.3)

- date: 2026-11-01T02:22:51.464Z
- host: darwin arm64

## Injection harness

`Controller`/`CrashFs`/`CrashUow`/`CrashJournal` in `crates/echo-core/src/application/recover/`
inject a scripted failed write at every boundary. Each crash triggers under
`catch_unwind`, then recovery runs TWICE; the invariants asserted are:

- unique terminal state (Completed/RolledBack/Restored/DatabaseFinalized),
- the reserved UUID is reused (no duplicate UUID),
- no orphan final file and no foreign overwrite,
- `released_claims() == [operation]` after idempotent recovery,
- only durably-`TrashApplied` items auto-finalize (TrashApplied-only forward-roll),
- unknown outcomes preserve relationships (TrashOutcomeUnknown keeps the
  journal and disables writes).

## Executed fault injections (run green this check)

- `application::recover::tests::imports::crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice`
- `application::recover::tests::imports::recovery_never_creates_a_duplicate_when_a_watcher_preempts`
- `application::trash::tests::external_staging_cleanup_becomes_unknown_and_preserves_relationships`
- `application::trash::tests::disconnected_volume_becomes_unknown_and_disables_root_writes`
- `application::recover::tests::delete_restore::contradictory_stage_evidence_holds_and_deletes_nothing`
- `application::recover::tests::imports::truncated_source_is_rejected_and_leaves_nothing`
- `application::recover::tests::imports::nothing_recoverable_rolls_back_and_releases_cleanly`
- `infrastructure::filesystem::adapter::tests::fresh_root_acquires_write_capability_by_establishing_staging`

## Result

All fault injections green on this host. The Terminal recovery ×2, no-orphan,
no-overwrite, no-duplicate, TrashApplied-only and unknown-keeps-relationships
invariants hold.
