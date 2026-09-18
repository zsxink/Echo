## 1. Audit and correct existing time facts

- [ ] 1.1 Trace `songs.added_at`, `songs.favorited_at`, `playlists.created_at` and `playlist_songs.added_at` from mutation entry through SQLite and read models; document findings in focused tests and verify no duplicate timestamp migration is introduced.
- [ ] 1.2 Correct any existing write path that does not use the backend's real wall-clock commit instant; verify fixed-clock tests cover initial import, duplicate import, rescan, restart, favourite/unfavourite/re-favourite, playlist create/rename and duplicate member addition.
- [ ] 1.3 Verify recent-added, favourites and playlist ordering consume the persisted authoritative fields, then test absolute-instant round trips and current-system-timezone conversion with at least UTC and one non-UTC/DST timezone.

## 2. Bounded multi-thread import and typed results

- [ ] 2.1 Implement a tracked worker pool sized `min(batch_size, max(2, available_parallelism), 4)` for copy/hash/probe stages while preserving per-file Core transactions, destination claims and input-result order; verify concurrent success, collision, duplicate and single-failure native tests.
- [x] 2.2 Add a deterministic synchronization-barrier test proving two file tasks enter a heavy stage concurrently on distinct workers, plus a controlled fake-I/O benchmark proving the batch is faster than the same serialized workload.
- [ ] 2.3 Preserve root-unavailable, cancellation and recovery semantics under the coordinator; verify no temporary files or partial records remain after an unavailable root, worker error or restart-recovery test.
- [ ] 2.4 Define one typed result classifier: imported/renamed/duplicate/normal-skip are non-failures; unsupported/corrupt/permission/copy/hash/parse/publish/library-unavailable are failures. Regenerate `apps/desktop/src/ipc/ipc-types.generated.ts` and verify IPC drift and serialization tests.

## 3. Desktop identity, menu bar and external-file playback

- [ ] 3.1 Change Tauri/bundle metadata and native labels to the exact user-visible name `Echo`; verify the macOS bundle configuration and app-shell tests contain no user-visible lowercase `echo` identity.
- [x] 3.2 Replace the macOS dropdown/popover and four-spaced-item implementations with one compact, fixed-width menu-bar control ordered Previous, Play/Pause, Next and black Echo brand icon; route root-view clicks through four non-overlapping x-coordinate regions, wire transport actions to the authoritative command/snapshot path, and verify accessibility labels/actions, exact boundary routing, single dispatch, play/pause state refresh, exact order and absence of a control dropdown/popover.
- [x] 3.3 Make the Echo menu-bar icon show, unminimize and focus the one stably labeled main window, guarding creation so repeated or rapid activation cannot create a second main window or playback session; verify hidden, minimized, already-visible and repeated-click cases with focused native tests.
- [ ] 3.4 Make every OS file-open request atomically replace the active context with exactly one entry and start playback without import; verify separate tests for active-library UUID reuse and external temporary items, next/previous boundaries, session filtering, no import writes and cold-start priority.

## 4. React import interaction and feedback

- [ ] 4.1 Remove the blocking import-progress/report dialog from the normal path and implement the shell-level `idle → picking → importing → idle` import state around the native picker; verify component tests cover cancellation, disabled `导入中…` state and re-enable after completion.
- [ ] 4.2 Apply the single result classifier: emit a non-failure aggregate toast when such results exist, show a focused failure-only detail dialog when failures exist, and show only the dialog for all-failure batches; verify imported/renamed/duplicate/skip, each failure class, mixed and all-failure UI tests.
- [ ] 4.3 Update the E2E mock and browser scenarios for the new import state/result contract; verify `pnpm --filter @echo/desktop test` and `pnpm --filter @echo/desktop test:e2e` pass.

## 5. Cross-layer regression and release verification

- [x] 5.1 Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace`; fix all formatting, lint and regression failures.
- [x] 5.2 Run `pnpm --filter @echo/desktop format:check`, `pnpm --filter @echo/desktop lint`, `pnpm --filter @echo/desktop typecheck`, `pnpm --filter @echo/desktop build`, and `pnpm --filter @echo/desktop test`.
- [ ] 5.3 Run `pnpm verify:task` and `openspec validate improve-desktop-import-and-macos-integration --strict`; record macOS screenshot/manual evidence for the `Echo` app name, always-visible Previous/Play-Pause/Next/Echo menu-bar row with no dropdown, single-instance main-window activation, background control synchronization, multi-thread import speed/feedback, current-timezone presentation and Finder single-file playback for both library and external files.
