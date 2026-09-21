## 1. Specification and shared invalidation

- [x] 1.1 Add the OpenSpec delta artifacts for temporary-item conversion, protected player-bar import, and library refresh; verify `openspec validate --strict --type change fix-player-temporary-import` passes
- [x] 1.2 Add a typed shared library invalidation store, make song queries refresh authoritatively without blanking existing rows, and make navigation counts subscribe to it; verify focused frontend tests cover one invalidation reaching list and counts

## 2. Playback and command implementation

- [x] 2.1 Add coordinator coverage and implementation for replacing the current temporary entry with a library `SongId` while preserving queue entry identity, position, and playing/paused state; verify Rust unit tests cover matching, stale-entry protection, and transport restoration
- [x] 2.2 Update `import_current_temporary_file` to convert only the still-current temporary entry after an imported or duplicate result; verify existing import DTO serialization and command-facing tests remain green

## 3. Player-bar interaction and regression coverage

- [x] 3.1 Replace player-bar fire-and-forget import with guarded async state, result feedback, and success/duplicate invalidation; verify React tests cover loading/disabled, single invocation, success, duplicate, and failure/skipped paths
- [x] 3.2 Run formatting, lint, type-check, focused Rust/React tests, full workspace tests, and OpenSpec validation; verify the working tree diff contains no changes outside this change and the pre-existing trash implementation
