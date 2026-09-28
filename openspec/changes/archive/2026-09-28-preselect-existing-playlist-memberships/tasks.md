## 1. Authoritative Membership Query

- [x] 1.1 Add the song-to-playlists repository query and implement SQLite plus in-memory adapters; verify empty, existing, and failure behavior with Core tests (`cargo test -p echo-core`).
- [x] 1.2 Add the Core use case and desktop service/Tauri IPC command with a typed return, then regenerate IPC types; verify command/service coverage and generator drift.

## 2. Picker State and Feedback

- [x] 2.1 Separate immutable existing memberships from user additions; render existing memberships read-only with explicit status, submit only the added difference, and close without mutation or success toast when unchanged.
- [x] 2.2 Keep batch picker unselected with clear additive-scope copy; handle single-song membership loading and failure with visible loading/warning states while preserving add capability.
- [x] 2.3 Add component coverage for preselection, difference-only submission, no-op confirmation, query failure fallback, and batch behavior; verify with the desktop component test command.

## 3. Contract and Integration Validation

- [x] 3.1 Update the existing playlist-management and library-experience specifications, refresh the picker deviation comment, and verify `openspec validate --strict` passes.
- [x] 3.2 Run applicable Rust formatting/tests, desktop typecheck/lint/tests, and IPC drift validation; review the final diff for additive IPC compatibility and unchanged removal semantics.
