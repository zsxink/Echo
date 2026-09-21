## Context

The import use case builds a batch-local `ImportConflictIndex` from the repository's complete root snapshot and from paths currently present on disk. The repository snapshot is intentionally broader than the visible catalog because scanning and relinking need to see missing records. Delete hides a song by changing its availability to `PendingDelete` while retaining its hash and path until the existing journal finalization succeeds.

## Goals / Non-Goals

**Goals:**

- Make all non-`Available` song records invisible to import duplicate and logical-target conflict decisions.
- Keep the full repository snapshot semantics needed by scan, relink, recovery, playlists, and delete finalization.
- Preserve old records, UUIDs, tombstones, undo behavior, and startup finalization compatibility.
- Add a regression test that reproduces Issue #12 without waiting on wall-clock time.

**Non-Goals:**

- No database migration or deletion-scheduler redesign.
- No change to the 10-second undo deadline or to system-trash retry policy.
- No change to catalog visibility rules, scan relinking, or user-facing IPC DTOs.

## Decisions

### Filter at the import conflict boundary

The import planner will inspect `SongAvailability` in both conflict checks: the batch-start snapshot records a song's hash and logical target only when the song is `Available`, and the post-publish race check applies the same filter. The filesystem enumeration remains authoritative for actual target occupancy, so a stale unavailable database path cannot reserve a target that is no longer present, while a real file still prevents overwrite.

Filtering in `batch_state` is preferred over adding an availability predicate to `SongRepository::all_in_root` or `all_songs_in_root`: those APIs are also used by scan/relink and must retain missing and pending records to preserve stable UUIDs and recovery semantics.

### Keep finalization independent

The existing `FinalizeExpiredDeletes` startup path remains unchanged. Re-import no longer depends on a delete row being finalized, so the running session can safely create a new record while the old delete journal continues through its existing undo/trash state machine.

### Regression coverage at the use-case level

The test seeds a same-hash `PendingDelete` record without a live target file, imports the same source, and asserts a new UUID is committed while the old unavailable row remains. This exercises both duplicate-content and database-path reservation behavior through the real import planner and test repository.

## Risks / Trade-offs

- [Risk] A stale unavailable record may coexist with a newly imported record carrying the same hash → This is intentional for the delete/re-import lifecycle; the old row remains unavailable and the new row is the playable identity. Existing finalization removes the old row only after its durable trash proof.
- [Risk] A real file at the old path could still block the target → Filesystem enumeration continues to reserve actual paths, preventing overwrite and preserving the no-clobber contract.
- [Risk] A future import caller may bypass `PlanImport::batch_state` → The decision is localized to the single shared import planner entry point; new import paths must use that use case and its regression test provides the contract.

## Migration Plan

No schema or data migration is required. Deploying the code changes only the next import's conflict snapshot; existing `PendingDelete`/`Missing` rows and journal states are read unchanged. Rollback is code-only and does not require restoring data.

## Open Questions

None.
