## Context

See `proposal.md` for motivation and the delta specs for behavioral contracts. Echo already separates shared library logic (`echo-core`) from desktop player, Tauri and React surfaces. The current import command has one batch-level operation and a blocking result modal; platform status-menu and temporary-file pathways exist but require completion and tightening.

The existing SQLite schema already contains `songs.added_at`, `songs.favorited_at`, `playlists.created_at` and `playlist_songs.added_at`. This change therefore audits and corrects their write/read semantics instead of introducing duplicate timestamp columns. These values are absolute epoch instants; timezone is a presentation conversion, not a second stored clock. The current macOS status implementation opens a control surface from one status item, while the required interaction places every control directly in the menu bar and keeps the application main window single-instance.

The design preserves the project dependency direction: React only consumes typed Tauri commands/events; desktop orchestration owns native dialogs, status items and player commands; Core owns import outcomes, transaction boundaries, timestamps and persistence. Core continues to have no UI, Tauri or mpv dependency.

## Goals / Non-Goals

**Goals:**

- Make desktop identity and macOS background controls reliable and consistent with one desktop playback session.
- Raise multi-file import throughput while retaining file isolation, idempotence, operation recovery and truthful per-file outcomes.
- Prove that import, favourite, playlist creation and playlist-member times use real commit instants, remain stable under idempotent operations and convert through the device's current timezone when presented.
- Make every system-open request replace the current queue with exactly the requested song, regardless of whether that file already belongs to the library.

**Non-Goals:**

- No streaming service, account, automatic import, bulk playlist editing, or new sync protocol.
- No change to managed-library queue construction, source-file mutation, deduplication rules, or cross-device conflict policy.
- No new timestamp columns, backfill with invented action times, or storage of a fixed timezone offset alongside every event.

## Decisions

### 1. Keep native integration and the single main-window lifecycle in the desktop platform layer

The Tauri configuration, bundle metadata and native labels will all derive from the single canonical display name `Echo`. The desktop runtime will expose a small transport-command adapter shared by main-window, media-key and menu-bar handlers, so menu-bar controls dispatch the same command/event path as the player rather than owning a second player instance.

On macOS, the native platform adapter will install one fixed-width `NSStatusItem` containing a custom AppKit view in the fixed visual order Previous, Play/Pause, Next, Echo brand icon. The view exposes four equal, non-overlapping horizontal regions and handles pointer events at the root: the local x-coordinate deterministically maps to exactly one action. Decorative child buttons render the four icons and retain separate accessibility labels/actions, but pointer hit testing is owned by the root view so overlapping or misrouted child targets cannot turn every click into one command. These controls remain visible in the menu bar itself; no click reveals a dropdown or popover containing the transport controls. The playback icon and enabled states derive from the authoritative playback snapshot, and the full row is visually verified against the supplied reference. The Echo region uses the approved black brand mark and is the sole window-opening affordance in this row. Windows and Linux retain their existing semantic tray commands and are not required to copy this macOS layout.

The main window remains addressable by one stable label. Echo-icon activation first resolves that existing window, then unminimizes, shows and focuses it; creation is permitted only when the labeled window does not exist, with repeated activation guarded so concurrent clicks cannot create duplicates. Closing or hiding follows the existing background-playback policy and does not spawn a replacement window while the original still exists.

Alternatives considered: ordinary native menu rows or an anchored custom popover. Both require an extra click and move the controls outside the menu bar, so they are rejected. Four separate `NSStatusItem`s are also rejected because the system spacing makes the control row consume too much menu-bar width. Allowing every Echo-icon activation to construct a window is rejected because it would split UI state and may create multiple playback surfaces.

### 2. Make concurrency a bounded worker-thread coordinator, not a Core/UI concern

The native file picker still returns opaque picked files at the Tauri boundary. Its paths are converted to independent import sources and handed to a bounded desktop coordinator. For batches of at least two files, the pool size is `min(batch_size, max(2, available_parallelism), 4)`: at least two workers, capped at four to avoid saturating disks. Each worker invokes the existing per-file Core import flow; filesystem copying, hashing and metadata parsing execute on those worker threads instead of the async/UI execution context. Final SQLite commit sections may serialize through the existing writer boundary.

The coordinator will not share mutable file readers or SQLite transactions across items. Destination claims, BLAKE3 checks and operation journals remain the authority for conflicts between concurrent items and watchers. It waits for all scheduled files, then returns one ordered batch result even if individual workers fail. Root unavailability is an upfront batch failure and follows the same failure-dialog classification.

Concurrency acceptance has two layers: a deterministic barrier test proves that two heavy file stages enter concurrently on distinct workers, and a controlled fake-I/O benchmark proves a multi-file batch completes faster than the same delays executed serially. A real fixture benchmark is recorded as evidence but is not the sole correctness gate because machine disks vary.

Alternative considered: unbounded task spawning or parallelising inside the React component. Unbounded work risks disk and memory exhaustion; UI concurrency would expose paths and duplicate business rules.

### 3. Replace the import modal with an import action state and failure-only detail overlay

The library shell owns a compact import state machine: `idle → picking → importing → idle`. The normal import button launches the native picker directly; while importing it is disabled and labelled `导入中…`. Cancellation returns to `idle` silently.

Once the authoritative batch result arrives, the shell refreshes songs/counts and applies one result classifier shared by toast and dialog construction:

| Result | Feedback |
|---|---|
| imported, renamed-and-imported, duplicate, normal skip | aggregate toast |
| unsupported, corrupt, permission denied, copy/hash/parse/publish failure, library unavailable | failure-detail dialog |

A mixed batch emits the non-failure summary toast and opens a dialog containing only failed entries and reasons. An all-failure batch opens only the failure dialog. Cancellation remains a silent no-op. The previous report/progress modal is removed, and the typed IPC contract continues to be generated rather than hand-edited.

Alternative considered: keep a modal with fewer rows. It still prevents browsing during an I/O-heavy operation and does not meet the requested low-interruption path.

### 4. Audit and reuse existing absolute timestamp fields

The audit traces every existing timestamp from mutation entry to SQLite and back through its read model:

- `songs.added_at` is assigned from the real backend wall clock at first successful import/scan creation, survives duplicate import, rescan and restart, and remains the source of recent-added ordering.
- `songs.favorited_at` is assigned on a successful false-to-true favourite transition, cleared on true-to-false, and replaced only by a later genuine re-favourite.
- `playlists.created_at` is assigned on successful playlist creation and is never rewritten by rename or cover changes.
- `playlist_songs.added_at` is assigned only when a new member relation commits; duplicate addition leaves it unchanged.

Where the current code does not meet those rules, it is corrected at the existing write boundary and covered by fixed-clock tests. No timestamp originates in the WebView, and no duplicate schema field or migration is added.

Epoch-based absolute instants remain timezone-neutral and preserve ordering across daylight-saving changes. Whenever a timestamp is exposed in a view, diagnostic or export, the boundary converts that instant using the device's timezone at read time. Tests set a known instant and timezone, verify the expected local representation, then switch timezone and verify the stored instant remains unchanged.

Alternative considered: adding an offset column or storing formatted local time. Both duplicate information and become stale when the user changes timezone; converting an absolute instant at presentation time satisfies “当前时区” without corrupting the event fact.

### 5. Every file-association request replaces the queue atomically

The desktop shell resolves a requested path against the active library only to choose identity semantics, never to decide queue size. A library match uses the existing song UUID and normal library metadata/statistics semantics. A non-library file builds a `TemporaryPlaybackItem` with the normal metadata probe and stays excluded from session persistence. Both branches invoke one player command that atomically clears all current queue entries, manual-next items, shuffle state and prior context, installs exactly the requested entry, and starts playback. Neither branch invokes import services.

Alternative considered: enqueueing the temporary file after the existing context. That makes next/previous revisit unrelated songs and violates the requested one-song playback list.

### 6. Patterns, boundaries and compatibility

- **Ports and Adapters:** Core uses its clock/repository/import ports; the desktop adapter implements platform picking, local-time acquisition and player bridging without leaking Tauri/mpv into Core.
- **Command / Event:** all presentation, status-menu and media-key actions converge on typed desktop player commands and playback snapshot events.
- **State machine:** the UI import state and the bounded coordinator lifecycle replace scattered boolean state; per-file Core operation journals remain the recovery state machine.
- **Unit of Work / Transaction Script:** each time-bearing mutation commits its record and fact fields in one SQLite transaction. File publication remains outside SQLite and retains its journal recovery boundary.

No SQLite schema migration is expected. Any IPC result refinement remains additive/backward-compatible; generated TypeScript bindings and the E2E mock are regenerated/updated from the Rust contract. No existing song IDs, playlist member IDs, filesystem layout or public queue ID changes.

## Risks / Trade-offs

- [Concurrent imports contend for disk, destination names or database writes] → cap worker count, keep transactions short, and retain existing hash/claim/journal invariants with concurrent regression tests.
- [One item blocks metadata parsing for too long] → run it as tracked blocking work and return that item’s failure without blocking other scheduled items.
- [Parallel speedup is hidden by serialized SQLite commits] → parallelize the expensive copy/hash/probe stages, prove overlap with a barrier test, and measure against a controlled serialized baseline.
- [Timestamp field exists but carries a synthetic sequence rather than a real instant] → audit every constructor/write call, correct only that write path, and test round-trip values with an injected fixed clock.
- [Timezone change makes displayed time appear different] → keep the stored instant immutable and define current-timezone conversion as presentation behavior.
- [Menu-bar state races window/player state] → use one snapshot source and idempotent transport commands; test repeated native command dispatch.
- [Rapid Echo-icon activation creates duplicate windows] → resolve one stable main-window label before creation, serialize the absent-window creation path, and regression-test repeated activation.
- [System open arrives during startup] → queue it through existing single-instance readiness handling, then execute the atomic temporary-session replacement once the player is ready.

## Rollout Plan

1. Audit existing timestamps and correct only incorrect write/read paths; assert schema snapshots remain unchanged unless implementation evidence proves an unavoidable compatibility defect.
2. Add the bounded worker pool and result classifier, then update generated IPC types, React shell and E2E mock together.
3. Add the macOS always-visible menu-bar control row, single-instance Echo-icon window activation and the all-path single-item file-open command, then collect automated and manual macOS evidence.
4. Rollback requires only reverting application code because no timestamp schema migration or historical backfill is planned.
