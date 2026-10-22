# Design

## Context

See [proposal.md](proposal.md) for motivation and [specs/lan-library-sync/spec.md](specs/lan-library-sync/spec.md) for the behavior contract. Today, startup restoration consults the active root's binding and starts a host or peer session without a persisted user-intent gate. Runtime stop and pause are in-memory operations. Root selection is mediated through the desktop service while SQLite sync contexts are selected per root. Transfer checkpoints and publication journals have independent durability contracts and cannot be treated as one disposable cache.

The implementation crosses the Core SQLite sync-context adapter, desktop runtime/provisioning, Tauri app lifecycle, resource transfer and publication recovery, and React status presentation. Core remains independent of Tauri, UI, and network protocol implementations.

## Goals / Non-Goals

**Goals:**
- Persist enabled/stopped/paused intent per local library root and restore only when both intent and binding are valid.
- Make startup restore and its retry worker cancellable at lifecycle transitions, with bounded attempts and visible terminal failure.
- Preserve correct library-switch semantics and prevent old-root retries or session state from crossing roots.
- Define cleanup by resource ownership and durability: remove disposable session state without deleting resources or durable recovery state still referenced by mutations, snapshots, journals, or checkpoints.
- Keep status/UI semantics aligned with persisted intent and support existing database upgrades.

**Non-Goals:**
- Change the LAN wire protocol, trust model, commit ordering, or cross-device shared data.
- Delete a user's local music, portable records, or confirmed history as part of stopping or leaving sync.
- Garbage-collect published commit history or secure monotone publication checkpoints during ordinary stop, pause, or process exit.
- Introduce a device-wide auto-start preference; this change uses per-root intent as the sole restore policy.

## Decisions

### Persist intent with per-root sync context

Store a typed lifecycle intent alongside the root-scoped synchronization context and include it in the same archive/select behavior as `sync_binding`. Existing databases migrate to a conservative non-running value (stopped/disabled), never infer permission to expose a network listener from the presence of an old binding. State transitions are performed by the runtime's explicit lifecycle operations through a narrow persistence port; Core owns the durable model and SQLite adapter, while the desktop shell decides when to start network workers.

Alternatives considered: storing intent in a device-wide local-state file would decouple it from root switching and risk applying one library's choice to another. Inferring intent from binding or old runtime state repeats the current defect. Adding a new shared wire field would expose local preference and is unnecessary.

### Separate user transitions from process-exit cleanup

Explicit stop and pause persist their respective intent before/while fencing workers; leave clears binding and intent. Normal process exit stops workers but does not overwrite the persisted intent, allowing a prior enabled session to restore. Every lifecycle transition advances a cancellation generation/token so delayed retries cannot act on a later root or superseded user intent. Retry waits must be interruptible; retries are finite with backoff and stop on user action, root change, successful start, or exhaustion. Exhaustion leaves a visible failed status and requires a new explicit action or a future startup attempt.

Alternatives considered: checking only status sequence after an uninterruptible sleep leaves long cancellation windows; relying on `Drop` conflates deliberate user choices with shutdown and cannot reliably coordinate retry ownership.

### Treat root switching as a guarded handoff

Do not stop the old session before the user has selected a real candidate. Keep old-root work available while candidate preparation runs; commit the root/context activation as the handoff boundary, then fence old-root workers and restore the selected root only according to its own persisted intent. If activation fails, retain or restore the old root's prior session. Route status refresh through the resulting root/status pair so the UI does not silently discard a valid transition as stale.

The existing service combines dialog selection with preparation and activation. Refactor the boundary only as needed to expose cancellation and the commit outcome; avoid moving Core activation rules into the Tauri shell. If the active-root commit can race with an in-flight sync cycle, use the existing transition/cycle synchronization primitives to serialize the handoff.

Alternatives considered: stopping before opening the picker breaks cancellation no-op; activating before fencing the old worker risks old-root writes after the root context changes. A prepare-then-fence/commit sequence should be selected based on the actual activation transaction boundary, and tests must prove no old-root work crosses it.

### Cleanup by ownership, preserve recovery records

Keep transfer blobs/checkpoints, process-local `uploaded_resources`, host-seen state, and publication journals conceptually distinct. Clear ephemeral/incomplete session state only when it is not referenced by pending mutations, snapshots, or recoverable publications. Retain valid resumable checkpoints when they are consistent with the active binding; malformed or conflicting cache records are invalid cache state and may be removed and reconstructed from authoritative resource declarations. Binding/epoch retirement and leave cleanup must have durable retry markers recorded before the last binding identity is discarded. Do not delete publication journals or security checkpoints merely because a listener stops; journal recovery must run before any cleanup that could remove its required files.

Alternatives considered: deleting an entire binding partition at every stop destroys restart recovery and can strand durable publication rows; retaining all partitions forever leaks storage and permits stale state to be reconsidered. A staged cleanup policy with explicit references gives deterministic recovery and reclamation.

### Keep UI status typed and root-aware

Expose stopped and paused as distinct persisted outcomes without implying a live network session. Refresh status after successful root switching and reject only genuinely stale events by root and sequence. Keep existing localized control language consistent with whether pairing is retained and whether the service is running.

## Risks / Trade-offs

- [Existing installations have bindings but no intent] → Migration defaults them to stopped, preventing unexpected network exposure; users can explicitly start them again.
- [Intent write and worker transition can fail at different points] → Define ordering so failure never leaves an unreported live listener; surface a recoverable error and test injected persistence/worker failures.
- [Root activation and runtime handoff are not one transaction across SQLite, threads, and filesystem] → Use a serialized handoff with explicit old/new root identity, idempotent cleanup, and recovery tests for each boundary.
- [Over-aggressive cache cleanup can destroy restart recovery or publication inputs] → Use existing reference/journal contracts, retain durable records, and test crash windows before removing files.
- [Bounded restore retries may stop before a temporarily unavailable host returns] → Show the failure state and provide explicit retry; future app restart may try again only while persisted intent remains enabled.

## Migration Plan

1. Add a root-scoped intent representation and schema migration with a conservative stopped default for existing bindings.
2. Wire lifecycle transitions, startup restoration, cancellable bounded retries, and per-root root-switch handoff.
3. Add cache cleanup/retry metadata only for resources whose binding and references are known; preserve existing durable publication and checkpoint recovery.
4. Update typed IPC/UI state and the main LAN sync specification; verify migration from pre-change databases and restart behavior.

Rollback must keep the new migration readable by the previous code only if the repository's migration policy permits it; otherwise rollback is application-level and retains the additive column/table. Do not reset or rewrite sync bindings, journals, or security checkpoints as a rollback shortcut.

## Open Questions

None that block implementation. The concrete retry-attempt limit and exact stale-cache retention interval are implementation policy choices; select conservative bounded defaults and test their externally observable guarantees.
