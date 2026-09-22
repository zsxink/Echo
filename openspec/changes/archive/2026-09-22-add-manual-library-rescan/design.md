## Context

See `proposal.md` for the user-facing motivation. The desktop already exposes a typed `start_scan` command and a library status hook with the active root and scanning state. The scanner owns metadata persistence and intentionally skips a file when its stored size and modification time still match. The frontend also has separate invalidation mechanisms for library queries and embedded cover keys, while lyrics are fetched by song id.

## Goals / Non-Goals

**Goals:**

- Add a discoverable rescan action to the existing settings dialog.
- Reuse the existing scan contract and preserve its background/transactional behavior.
- Protect the action against duplicate clicks and show success/failure feedback.
- Invalidate mounted library, cover, and lyrics consumers only after a successful scan commit.
- Keep the original audio files untouched.

**Non-Goals:**

- Do not add a second scan API or change the scanner's size/mtime fast-skip policy.
- Do not implement a forced metadata parse for files whose size and modification time are unchanged.
- Do not add metadata editing, cover editing, lyric editing, or override-layer controls.
- Do not change the existing file-watcher implementation or startup lifecycle.

## Decisions

### Reuse `start_scan` from the settings surface

The settings view passes the opaque active-root id from `useLibraryStatus` to the existing typed `start_scan` bridge command. This keeps path handling and scan ownership in the desktop/Core boundary. A new endpoint was considered, but would duplicate an already sufficient command and create another public IPC contract.

### Use local UI busy state plus authoritative library status

The button is disabled when there is no active root, when the backend reports a scan in progress, while the command promise is pending, or while the directory picker is active. The local busy state is necessary because `start_scan` is awaited as one command and a status event may not arrive before the command completes; the backend status remains authoritative for externally started scans.

### Refresh consumers after commit

After `start_scan` resolves successfully, the settings view calls the existing cover-key invalidation and library invalidation signals. Library queries and navigation counts re-read their authoritative snapshots; cover consumers re-request the current song keys; the lyrics hook subscribes to the same library invalidation so an already-open player re-fetches its effective lyrics. Failed scans do not invalidate already-visible content.

### Keep incremental scan semantics

The manual action means “re-check the library,” not “parse every file unconditionally.” The existing size/mtime fast path is retained for large-library performance. External tag writers normally update modification time; files whose size and modification time are both preserved remain outside this change's guarantee.

## Risks / Trade-offs

- [Risk] `start_scan` is a long-running command, so the backend may not emit intermediate progress to this dialog. → Mitigation: keep the local busy label and disable competing actions; use the existing library status when available.
- [Risk] A scan can finish while a mounted consumer is unobserved or a response is in flight. → Mitigation: invalidations trigger fresh authoritative reads and existing cancellation guards prevent stale responses from winning.
- [Risk] A scan can complete with per-file diagnostics. → Mitigation: treat command success as scan completion, preserve existing content, and leave detailed per-file diagnostics to the existing scan reporting path.

## Migration Plan

No database migration or data migration is required. Ship the settings UI and consumer invalidation changes together; rollback removes the button and its frontend invalidation calls while leaving the existing `start_scan` command and stored data unchanged.
