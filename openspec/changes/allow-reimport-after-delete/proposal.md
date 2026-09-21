## Why

Issue #12 exposes a lifecycle gap in local import: a song hidden by Echo's delete flow remains in the duplicate-content snapshot while its undo window is pending, so importing the same file is rejected even though the song is no longer available in the library. Expired deletes are also finalized only during startup recovery, leaving stale pending-delete rows in a running session.

## What Changes

- Exclude songs whose availability is not `Available` from import content and target-conflict snapshots.
- Preserve the existing undo window, operation journal, tombstone, and retry-safe finalization semantics; finalization scheduling is not changed by this fix.
- Add regression coverage for immediate re-import after delete and confirm the existing finalization behavior remains intact.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `safe-file-ingestion`: a deleted or otherwise unavailable song must not make an import appear to be a duplicate or reserve its former target path.
- `local-library`: unavailable song records must not participate in a new import's identity snapshot, while they remain durable until existing finalization succeeds.

## Impact

- Core import conflict snapshot and SQLite song-root query semantics.
- Rust unit/integration tests and OpenSpec validation commands.
- No public IPC payload, runtime scheduler, or database schema change; existing `SongAvailability` and operation-journal states remain compatible.
