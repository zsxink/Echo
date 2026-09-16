## Context

See [proposal.md](proposal.md) for motivation and the delta specs for observable contracts. The present desktop queue stores a logical append-order vector with a movable current index, but serializes and publishes that vector directly; this leaks historical entries ahead of the current item into the queue panel. Its `history` is an in-memory, count-capped list of IDs (100), so it has neither timestamps nor a three-day expiry rule. Session restore also discards the supplied blocked disposition when rebuilding entries.

Playlist creation crosses React → bridge → Tauri → `AppServices`, but the React dialog currently supplies `root: ""` while the Tauri command parses a required `LibraryRootId`. Library fetch cache identity excludes the active root; recent/favorites shortcuts bypass the general search path; and playback contexts are assembled from only the loaded page.

The implementation remains layered: React renders IPC DTOs, desktop runtime owns playback/session state, and Core remains independent of player and WebView.

## Goals / Non-Goals

**Goals:**

- Make all audited phase-one user paths deterministic, failure-safe, and covered by the same checks used for release acceptance.
- Preserve stable `QueueEntryId` identity, duplicate queue entries, session restore, and the existing no-absolute-path WebView boundary.
- Make the UI queue an explicit playback-order projection instead of exposing storage layout.

**Non-Goals:**

- No cloud sync, accounts, streaming catalog, batch actions, playlist drag-reordering, or changes to Core's player independence.
- No migration of existing song/playlist data and no history shared between devices; playback history stays private desktop state.

## Decisions

### 1. Separate logical queue storage from display/playback-order projection

Keep the existing entry-ID-based logical queue as the coordinator's source of identity and traversal. Add a single coordinator queue-view method that returns `current` first and then only currently pending entries; the runtime snapshot mapper, queue panel, `clearPending`, and tests consume this view. For shuffle, the projection uses the remaining shuffle bag order plus any newly pending IDs not yet selected; it never puts already-played history before current. For sequential and repeat-one, it uses the corresponding pending order without moving storage entries.

This avoids reordering the backing vector on every transition, which would complicate insertion, duplicate IDs, deletion rollback, and persisted session compatibility. Rebuilding the vector to make current index zero was considered and rejected because it conflates traversal/history state with UI order and risks changing list-cycle semantics.

### 2. Persist timestamped entry history in the desktop session

Replace the bare history-ID list with timestamped history records keyed by `QueueEntryId`, recorded when an entry actually becomes current. Use wall-clock instants solely for the three-day retention boundary; retain monotonic timing for playback statistics. Prune expired records on record, restore, and previous lookup. Persist only library entry IDs and timestamps in the versioned desktop session; filter temporary and deleted/missing IDs during restoration. The `previous` operation scans newest-to-oldest for a valid non-current entry and does not mutate pending order.

The current count-cap-only list cannot meet time retention or cross-restart requirements. A SQLite playback-history table was considered and rejected: the requirement is a private playback-session navigation aid, not a library-domain or syncable record, and it would need unnecessary Core migration and deletion coupling.

### 3. Carry blocked status through session rebuild and UI DTOs

Represent restored availability as entry metadata or a coordinator-side blocked-ID set, preserved from restore verdict through queue view and `UiQueueEntry`. The runtime uses it both to render a clear blocked state and to skip/retry only after availability is rechecked. Restore summaries count actual blocked entries rather than all restored entries. `QueueEntryId` remains the joining key so duplicate songs cannot collapse.

Treating blocked as a storage-only note was rejected because it currently makes both the summary and UI misleading and violates the recovery contract.

### 4. Make active-root identity and query identity first-class frontend inputs

Pass `status.activeRoot` into the create playlist dialog; disable the entry point until it is known and reject a missing root locally before invoking IPC. Include root identity in the song-query key and invalidate/reset page/cursor/request generation whenever it changes. Route search through view-aware desktop queries so recent and favorites receive the same normalized search filter; hide the all-songs sort control outside `all`.

For playback started from a paged library view, add a desktop command that receives the declarative view/filter/sort/selected-song request and resolves the complete deterministic ID sequence on the desktop/Core query boundary before calling the player. The playlist path can retain its ordered members query. Fetching every frontend page to construct a context was rejected because it races pagination/root switching, may omit data, and makes a large queue depend on rendered browsing progress.

### 5. Treat async UI success as a commit acknowledgement

Wrap playlist membership mutation and enqueue mutations in awaited handlers. Refresh local lists/counts and show success only after a resolved desktop call; on rejection retain the last authoritative state and use the existing non-blocking error surface. Use request/version guards for list loads so failures do not silently turn a known non-empty playlist into an empty one.

Optimistic UI was considered only for favorite toggling because that operation already has authoritative rollback. It is not used for playlist create/remove or queue commands because their current implementation lacks a reliable correction snapshot at the mutation point.

### 6. Restore regression checks before adding new acceptance evidence

Diagnose and fix the actor test's load-event/state transition rather than relaxing its expected `Playing` state. Ensure diagnostic-hook tests isolate their temporary output and no test logger writes to the asserted clean target. Add focused regression tests, then register a native/mock bridge flow that covers create → add → play → modes → previous → restart and run it together with existing Rust/frontend quality gates.

## Risks / Trade-offs

- [Wall clocks can jump] → Retention comparisons handle future timestamps conservatively and prune only records demonstrably older than three days; playback-duration accounting remains monotonic.
- [Session schema compatibility] → Bump or tolerate an additive session version with safe parsing of legacy ID-only history; malformed values are skipped rather than preventing startup.
- [Large complete playback contexts] → Resolve IDs on the desktop query boundary with pagination/streaming internally and set a documented bounded command payload from the WebView.
- [Async root switching races] → Tag frontend requests with the root/query generation and discard stale responses; runtime revalidates active-root membership before resolving a song.
- [Actor-test flakiness] → Make completion signaling explicit in the test double/event path and avoid timing sleeps as a correctness condition.

## Migration Plan

1. Add backward-compatible session parsing and write the new history representation; old desktop session data upgrades in memory on next save.
2. Ship queue projection and blocked DTO changes behind the same desktop/frontend release so an old UI never receives an incompatible snapshot shape.
3. Add regression and end-to-end evidence, run the full quality gate, then enable the change for the existing desktop-state location.
4. If rollout exposes a session decoding issue, ignore only the malformed playback-session field and start with an empty paused session; never alter Core library/playlist records.
