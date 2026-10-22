# Tasks

## 1. Persistent Intent

- [x] 1.1 Add a typed per-library sync intent to the Core persistence boundary and SQLite schema; verify migration tests default existing bindings to non-running and preserve other root contexts.
- [x] 1.2 Archive and restore the intent with each root's sync context; verify root-context tests cover enabled, stopped, and paused isolation.
- [x] 1.3 Add persistence-port tests for explicit lifecycle transitions and failure ordering; verify failed writes cannot leave a newly enabled network session unrecorded.

## 2. Runtime Lifecycle and Restore

- [x] 2.1 Persist enabled/stopped/paused transitions from start, stop, pause, resume, and leave operations while keeping process-exit cleanup from overwriting intent; verify runtime lifecycle tests cover each transition.
- [x] 2.2 Gate startup restoration on valid binding, matching active root, and enabled intent; verify no-binding, stopped, and paused startup tests perform no network start attempt.
- [x] 2.3 Replace detached uninterruptible restore retry with cancellable bounded retry tied to root and intent generation; verify stop, pause, leave, root switch, success, retry exhaustion, and retry backoff wake-up tests.
- [x] 2.4 Surface retry exhaustion as a visible typed status with an explicit user retry path; verify IPC serialization and UI status tests distinguish failed, stopped, and paused states.

## 3. Cache and Publication Lifecycle

- [x] 3.1 Map references among transfer checkpoints/blobs, pending mutations, active snapshots, publication journals, and process-local provisioner state; verify focused tests establish which files/state are retained or removed for stop, pause, exit, leave, and epoch change.
- [x] 3.2 Implement reference-aware cleanup for disposable session state and durable retry markers for binding cleanup; verify repeated cleanup and restart recovery tests pass without deleting referenced resources or publication recovery state.
- [x] 3.3 Make invalid/corrupt transfer cache state recoverable from authoritative resource declarations; verify missing/mismatched blob and checkpoint tests recover by retransferring rather than remaining permanently blocked.
- [x] 3.4 Prevent unvalidated cross-binding cache reuse and clean retired binding partitions when safe; verify binding/epoch isolation and cache-size reclamation tests.

## 4. Root Handoff and Desktop Status

- [x] 4.1 Restructure library selection so cancellation is a no-op and sync is handed off only at the activation boundary; verify cancel leaves runtime, status, retries, and caches unchanged.
- [x] 4.2 Restore the target root's session only from its own enabled intent and preserve/restore the old session after activation failure; verify successful switch, failed switch, and switch-back tests.
- [x] 4.3 Refresh and order root-aware sync status after switching; verify frontend tests show stopped/paused/active states without dropping a valid new-root status.
- [x] 4.4 Align localized settings text with stop-versus-leave semantics and explain cache cleanup; verify relevant component tests assert the user-visible copy and action availability.

## 5. Specification and Integration Verification

- [x] 5.1 Update the main LAN sync specification with approved intent, root-switch, and cache lifecycle behavior; verify `openspec validate --strict --change persist-lan-sync-session-intent` succeeds.
- [x] 5.2 Run focused Rust tests for echo-core, echo-sync, and desktop sync runtime plus desktop UI tests; verify all pass.
- [x] 5.3 Run formatting, lint, and the repository's required cross-platform sync verification commands from `openspec/CODE_STANDARDS.md`; verify results are recorded and no unrelated generated churn is included.
