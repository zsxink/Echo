# Task 3.6: Initial baseline and fixed snapshots

## Coverage

- `sync_baseline::initial_baseline_preserves_chain_current_state_and_undoable_song` verifies a historical baseline retains the existing chain/current state and the undoable song without replaying old outbox work as new edits.
- `sync_snapshot::snapshot_pages_remain_fixed_during_concurrent_later_publication_and_restart` verifies a frozen high-water mark and pages survive later publication and process restart.
- `sync_snapshot::snapshot_keeps_final_tombstone_and_aliases_in_one_frozen_record_order` verifies typed aliases and final tombstones are included in the fixed snapshot ordering.
- `sync_snapshot::snapshot_excludes_pending_projection_and_preserves_last_live_state_during_undo_window` verifies a pending delete does not replace the last valid portable object during its undo window.
- `sync_snapshot::snapshot_retains_safe_resource_manifests_and_session_hash_references_until_expiry` verifies staged resources remain referenced for the snapshot session lifetime.
- `sync_snapshot::snapshot_tokens_bind_to_host_identity_reject_invalid_pages_and_expire_durably` verifies token binding, page validation, and durable expiry.
- `sync_baseline::failed_baseline_rolls_back_all_snapshots_and_marker_then_retries` verifies baseline failure rollback and retry.

## Validation

- `cargo test -p echo-core --locked` — passed on 2026-10-18: 609 passed, 0 failed, 1 ignored; integration suites passed (7 architecture tests, 7 logging tests, 1 sync cursor test, and 2 enrollment tests).

## Scope

This records the repository test coverage for the snapshot/baseline behavior. Real-device acceptance remains in tasks 8.3 and 8.4.
