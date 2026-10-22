# Verification Record

Change: `persist-lan-sync-session-intent`
Platform: macOS (arm64), Rust 1.98.1 toolchain pin

## Formatting, lint, types

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass |
| `cargo run -p echo-desktop --bin echo-generate-ipc` (twice) | idempotent; only the intended +9 lines |
| `npx tsc --noEmit -p tsconfig.app.json` | pass |
| `npx eslint .` | pass |
| `npx prettier --check .` | pass |

## Rust tests

| Command | Result |
|---|---|
| `cargo test -p echo-core --all-features` | 674 passed, 1 ignored |
| `cargo test -p echo-sync --all-features` | 65 passed, 3 ignored |
| `cargo test -p echo-desktop` | 421 + 2 + 1 + 1 + 1 + 4 + 3 passed |
| `cargo test -p echo-app` | 106 passed, 1 ignored |

New focused tests:

- `echo-core` `sync_cache_cleanup`: markers are durable, idempotent, attempt-counted,
  and survive reopening the database.
- `echo-sync` `resource_transfer::tests`: corrupt checkpoint, mismatched blob,
  conflicting declaration, cross-binding reuse, retired-epoch reclamation, and the
  full lifecycle reference map (stop/pause/exit vs leave vs epoch change).
- `echo-app` `sync_runtime::tests::session_intent`: bounded exhaustion, explicit
  retry, pause/leave interrupting backoff, root switch superseding a pending
  restore, successful restore clearing attempt accounting.
- `echo-app` `sync_runtime::handoff`: cancel is a no-op, target root restored from
  its own intent, stopped/paused/unbound targets stay closed, failed switch
  restores the previous session, abort after a root change refuses to guess.
- `echo-app` `sync_provisioner::tests::cache_cleanup`: detach marker recorded before
  the binding identity is discarded and completed only after the partition is gone.

## Frontend tests

| Command | Result |
|---|---|
| `npx vitest run` | 373 passed (40 files) |
| `npx vite build` | pass |

New component/unit tests cover stop-versus-pause copy, the exhausted-restore retry
path, host stop-versus-leave semantics, and root-aware status ordering (a late
event for the root the user just left is rejected; the new root's own
stopped/paused/active status is adopted).

## Specification and release gates

| Command | Result |
|---|---|
| `openspec validate --strict persist-lan-sync-session-intent` | valid |
| `openspec validate --strict --all` | 20 passed |
| `node scripts/verify/reconcile-scenarios.mjs` | 437 = 437 = 437 |
| `node scripts/verify/ci-governance.mjs` | pass (33/33 injected violations detected) |
| `node scripts/verify/checks/task-13.9.mjs` | pass; automated scenarios 387/387 |

## Notes

- One pre-existing defect surfaced while implementing task 1.x: the new
  `sync_binding.session_intent` column had no counterpart on
  `recovery_sync_binding`, whose archival `INSERT ... SELECT live.*` broke three
  host-recovery tests. Migration `0026` now adds the column there too, keeping
  recovery archives column-for-column with the live table.
- `commands.rs` grew past the 1000-line scale gate; the library-selection handoff
  command moved to `sync_library_selection.rs` instead of relaxing the gate.
- No unrelated generated churn: `git status` shows only files owned by this change
  plus the intended IPC regeneration.
