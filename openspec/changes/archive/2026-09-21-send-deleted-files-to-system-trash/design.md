## Context

See `proposal.md` for the user-facing motivation. The existing Core delete state machine already stages audio and sidecar resources, persists the undo deadline, and only removes database relationships after a `SystemTrashPort` success receipt. The desktop implementation currently has a policy wrapper and test backend seam, but the production composition root passes a backend-less `DesktopTrash`; finalization is also only invoked during startup recovery.

The change must preserve the Core/platform boundary: `echo-core` remains unaware of Tauri, the OS, and any trash library. Absolute paths are resolved only by desktop infrastructure from the shared `RootRegistry`; the port continues to receive `LibraryRootId` and `OperationId`.

## Goals / Non-Goals

**Goals:**

- Provide a real cross-platform desktop adapter that moves the whole Echo delete staging directory to the OS recycle bin/trash.
- Schedule finalization in the running desktop process and retry safe transient failures without changing the Core journal state machine.
- Enforce that only an Echo-owned staging directory below the registered library root is handed to the OS adapter.
- Keep startup recovery, explicit success receipts, outcome-unknown isolation, and database cleanup semantics unchanged.

**Non-Goals:**

- No Core dependency on a platform trash crate.
- No permanent-delete API, trash browsing/restoration UI, or user-configurable retention policy.
- No mobile implementation or changes to SQLite schema, portable record format, Tauri command signatures, or sync protocol.

## Decisions

### 1. Use Ports and Adapters with a production `trash` backend

Keep `SystemTrashPort` in Core and keep the existing `TrashBackend` policy seam in `echo-desktop`. Add a concrete desktop backend backed by the cross-platform `trash` crate, and compose it as `TrashWithBackend<TrashCrateBackend>` in the Tauri root.

The `trash` crate is selected because it exposes one synchronous operation for moving files and directories to the OS trash on macOS, Windows, and freedesktop-compatible Linux desktops. A custom per-platform shell command was rejected because it would duplicate OS behavior, complicate quoting/error handling, and violate the existing adapter boundary. The backend maps any library error to the existing conservative `Other` outcome; the policy still supports bounded lock retries for injected/platform-specific backends and never fabricates success.

### 2. Resolve and validate the staging target inside the backend

`TrashCrateBackend` receives only root and operation IDs. It resolves the registered root through `RootRegistry`, constructs the fixed `echo/tmp/trash/<uuid-simple>` path, rejects symlink components and non-directory/missing targets, canonicalizes the target, and verifies containment under the canonical root before calling `trash::delete`.

This keeps absolute paths out of Core and prevents arbitrary-path deletion through the system-trash port. The backend does not infer success from a missing path after a failed call; only `trash::delete` returning `Ok(())` is converted into the port's success receipt.

### 3. Add a bounded runtime finalization worker

Add a desktop runtime-owned worker that runs on a dedicated blocking thread. It wakes immediately and then at a short fixed interval, reads the current active root from the Core repository, and invokes the existing `FinalizeExpiredDeletes` use case with the shared production `SystemTrashPort`. The worker logs a compact operation/report outcome and keeps running after retryable errors; it does not mutate journal state outside the Core use case.

The worker is owned by the Tauri managed state and has an explicit stop channel plus join-on-drop lifecycle. This avoids detached background work surviving application shutdown and keeps synchronous filesystem/OS trash calls off the Tauri async executor. Root switches are handled by resolving the active root on every tick rather than capturing an absolute path.

The scheduler is intentionally coarse-grained: the existing 10-second undo contract remains exact, while the first poll after expiry may add at most one scheduler interval. It is idempotent because Core ignores non-forward candidates and finalizes only persisted `TrashApplied` receipts.

### 4. Reuse the existing irreversible state machine

Do not add a new timer-specific state or database field. The worker calls the same `FinalizeExpiredDeletes` path used during startup. A successful OS call persists `TrashApplied` and then finalizes the database; a pre-call failure leaves a retryable pending operation; an ambiguous filesystem/backend result follows the existing `TrashOutcomeUnknown` isolation path.

## Risks / Trade-offs

- [Risk] The third-party trash library can fail on an unsupported/headless Linux desktop. → Treat the call as a conservative failure, retain staged evidence, and retry without deleting Core records; native acceptance covers supported desktop environments.
- [Risk] A worker tick can coincide with undo or delete staging. → Core remains the sole journal state authority; deadline and operation-state checks make the race resolve to either a valid undo or a valid post-deadline finalization.
- [Risk] A backend call can return an error after the OS has started moving the directory. → Keep the existing post-error staging-evidence check and outcome-unknown isolation; never retry when the evidence is no longer intact.
- [Risk] The `trash` crate performs synchronous OS/filesystem work. → Run it only from startup's existing blocking recovery path or the dedicated finalization worker, never from Tauri async commands.
- [Risk] The trash crate has platform implementation caveats on some Linux/FreeBSD mount queries. → Keep the dependency limited to the desktop platform crate, document the version, and make failures conservative; Core remains portable and testable with fakes.

## Migration Plan

1. Ship the adapter and worker with the existing journal schema; no data migration is needed.
2. On first launch after upgrade, startup recovery still processes old staged operations, then the live worker handles newly expired deletes.
3. If an upgrade must be rolled back, the journal and staging layout remain compatible; removing the worker only reverts to startup retry behavior, while already trashed operations retain their persisted receipts.

## Open Questions

None.
