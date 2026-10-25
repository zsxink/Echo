# Claim arbitration and UUID reuse — 2026-10-26

Task 5.3 had two uncovered identity races and one asymmetric target claim check.

## Scan racing an import

Added scan_reconciles_against_import_committed_while_its_worker_is_blocked. It pauses the scan's file hasher, completes an import of identical bytes on the same root, then releases scanning. The old implementation used a planner built before parsing and minted a second identity. The scan now refreshes its authoritative song snapshot after each parse batch and before reconciliation; the test confirms one song remains and it keeps the imported SongId.

The test is a two-worker barrier: import commits while the scan worker is paused. Existing scan tests continue to verify the configured worker bound and that cancellation does not mark unprocessed files missing.

## Watcher claim lookup

Changed watcher reconciliation to inspect the persisted incomplete operation items before consulting the runtime's active-operation callback. The callback can lag the journal. Published files are now applied under their reserved SongId even when the runtime list is empty. Added an operation envelope to the regression fixture so it models the durable claim correctly.

The in-progress claim test also verifies that a FailedRecoverable import continues to defer watcher events until recovery; it cannot be treated as a finished operation while it still owns its path.

## Local and synchronized target claims

Added a SQLite regression test that acquires a local operation claim and confirms remote publication preparation refuses that path. After release, remote preparation succeeds and the reverse local claim attempt is refused. The local check is inside the same writer transaction as the journal-item upsert and matches only an unreleased remote claim for the same root and normalized target. Remote preparation already checked the inverse direction.

Same-hash deduplication, conflicting target numbering, and create-new protection are covered by the existing import tests listed in tasks.md.

## Verification

- RED: watcher without the active callback minted a new SongId; local operation upsert succeeded over an active remote publication claim.
- GREEN: the scan/import barrier, persisted watcher claim, and bilateral SQLite claim tests pass.
- cargo test -p echo-core --lib scan: 58 passed.
- cargo test -p echo-core --lib import: 63 passed.
- cargo test -p echo-core --lib watch: 27 passed, including the final recoverable-claim assertion.
- cargo test -p echo-core --lib sync_resource_publication: 21 passed.
- cargo clippy -p echo-core --all-targets -- -D warnings: passed.
- cargo fmt --all -- --check, git diff --check, and openspec validate enforce-library-first-durability --strict: passed.
- External tag changes remain read-only input to Echo. This work does not write tags to music files.

