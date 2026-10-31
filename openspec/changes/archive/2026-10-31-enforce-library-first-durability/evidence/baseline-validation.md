# Published baseline path validation

Baseline: `2d07e7f` (published main). CodeGraph was consulted for symbol navigation; each path below was independently checked with `git cat-file -e 2d07e7f:<path>`. Development files and the preserved WIP stash are not acceptance evidence.

- `crates/echo-core/src/application/import/execution.rs`
- `crates/echo-core/src/application/scan/mod.rs`
- `crates/echo-desktop/src/runtime/services/favorites.rs`
- `crates/echo-desktop/src/runtime/services/playlists.rs`
- `crates/echo-desktop/src/runtime/player/recorder.rs`
- `apps/desktop/src-tauri/src/sync_provisioner/remote_apply.rs`
- `apps/desktop/src-tauri/src/sync_provisioner/peer_sync.rs`
- `apps/desktop/src-tauri/src/sync_provisioner/resource_publication.rs`
- `apps/desktop/src-tauri/src/sync_provisioner/resource_candidates.rs`
- `crates/echo-core/src/infrastructure/sqlite/sync/sync_apply.rs`
- `crates/echo-core/src/application/continuation.rs`
- `crates/echo-core/src/domain/library/portable.rs`
- `crates/echo-core/src/infrastructure/filesystem/control_plane.rs`
- `crates/echo-core/src/infrastructure/filesystem/portable_migration.rs`

Result: all 14 paths exist in the published baseline. Lines are navigation hints only. The import portable write follows the database commit within the same Core method, not a separate desktop method. Source behavior remains documented in `entrypoints.md` and design Context.
