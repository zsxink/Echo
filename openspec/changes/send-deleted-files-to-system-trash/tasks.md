## 1. Specification and dependency wiring

- [x] 1.1 Add the cross-platform `trash` dependency to the desktop workspace/package manifests, update `Cargo.lock`, and verify `cargo check --workspace` resolves a reproducible lockfile without adding the dependency to `echo-core`.
- [x] 1.2 Implement the production `TrashCrateBackend` over `RootRegistry`, including fixed staging-path construction, symlink/metadata checks, canonical-root containment, and conservative error mapping; verify unit tests reject unbound, missing, symlinked, and out-of-root targets and accept a valid directory through an injected/testable call boundary.
- [x] 1.3 Compose `TrashWithBackend<TrashCrateBackend>` at the Tauri desktop root and remove the backend-less production wiring; verify the native assembly no longer uses `DesktopTrash::default()` and the desktop crate tests pass.

## 2. In-session finalization

- [x] 2.1 Add a lifecycle-owned desktop finalization worker that polls the active root, invokes `FinalizeExpiredDeletes` off the async executor, logs retryable/unknown outcomes without leaking absolute paths, and stops/joins cleanly; verify worker unit tests cover immediate polling, no active root, retryable errors, and shutdown.
- [x] 2.2 Register the worker in the Tauri managed state after startup recovery while sharing the same production trash port; verify startup recovery and the worker use one adapter instance and root switches are resolved dynamically.
- [x] 2.3 Add/extend regression coverage for delete expiry without restart: a staged delete remains undoable before 10 seconds, is sent to the injected system-trash backend after expiry during the same process, and only then removes its Core relationships; verify the Core and desktop runtime test suites pass.

## 3. Cross-platform and delivery verification

- [x] 3.1 Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and targeted/full Rust tests; fix any dependency or lint regressions without changing the specified state semantics.
- [x] 3.2 Run `openspec validate --strict` and the repository's applicable scenario/static verification commands; verify the finalization path, path boundary, and no-backend production wiring are all covered by evidence.
- [x] 3.3 Perform native smoke verification on the available desktop OS (delete a test audio plus `.lrc`, wait beyond the undo window without restarting, confirm both entries appear in the OS trash and the library record is finalized) and document unavailable platform checks as follow-up evidence.
