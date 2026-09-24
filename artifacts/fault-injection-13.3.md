# Echo 0.1.0 fault-injection report (task 13.3)

- date: 2026-09-24T08:07:15.167Z
- host: darwin arm64

## Injection harness

`Controller`/`CrashFs`/`CrashUow`/`CrashJournal` in `crates/echo-core/src/application/recover.rs`
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

1. `crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice`
   — every journal-state write + publish + DB-commit point, recovered twice.
2. `recovery_never_creates_a_duplicate_when_a_watcher_preempts` — watcher
   preemption of an in-flight import leaves exactly one song.
3. `external_staging_cleanup_becomes_unknown_and_preserves_relationships`
   — external cleanup of the staging dir → TrashOutcomeUnknown, no inference.
4. `disconnected_volume_becomes_unknown_and_disables_root_writes` — volume
   disconnect mid-trash → unknown, root writes disabled.
5. `contradictory_stage_evidence_holds_and_deletes_nothing` — conflicting
   state evidence never deletes a user file.
6. `truncated_source_is_rejected_and_leaves_nothing` — short/size-mismatched
   source is rejected with nothing staged.
7. `nothing_recoverable_rolls_back_and_releases_cleanly` — clean rollback.
8. `fresh_root_acquires_write_capability_by_establishing_staging` — the 13.2
   P0 regression: a brand-new root becomes writable on acquire, so the first
   import/delete is not permanently LibraryUnavailable.

## Result

All fault injections green on this host. The Terminal recovery ×2, no-orphan,
no-overwrite, no-duplicate, TrashApplied-only and unknown-keeps-relationships
invariants hold.
