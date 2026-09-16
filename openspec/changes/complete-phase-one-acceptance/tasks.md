## 1. Establish the failing baselines

- [ ] 1.1 Add focused regression tests reproducing empty-root playlist creation, stale-root list results, recent/favorites search, and non-all sorting; verify each fails before its implementation and passes afterward.
- [ ] 1.2 Stabilize the existing actor load-state and diagnostics-hook tests without weakening their assertions; verify `cargo test -p echo-desktop --lib player::actor::tests::load_drives_state_from_stopped_to_playing_via_file_loaded -- --exact` and `cargo test -p echo-desktop --lib platform::diagnostics::tests::install_hook_and_logger_are_side_effect_free -- --exact` pass.

## 2. Repair playlist command and mutation feedback

- [ ] 2.1 Thread the active library root ID from shell status into the create dialog and Tauri create command, rejecting unavailable roots before a mutation; verify a real/mocked create produces a valid playlist and opens it.
- [ ] 2.2 Make playlist list/member loads, member removal, and enqueue actions await commit acknowledgement and retain authoritative UI with a retryable error on failure; verify React tests cover success and rejection paths with unchanged counts/list on rejection.
- [ ] 2.3 Preserve duplicate-name validation during rename by supplying the authoritative sibling names or correctly mapping the backend conflict; verify create/rename validation tests and playlist integration tests pass.

## 3. Implement queue projection, history, and recovery correctness

- [ ] 3.1 Add a coordinator queue-view projection that returns current first and mode-correct pending entries without changing queue-entry identity; verify sequential, shuffle, repeat-one, insert-next, clear-pending, and duplicate-song unit cases.
- [ ] 3.2 Update UI snapshot DTO mapping and QueuePanel rendering to consume the projection and expose blocked state; verify runtime mapping and frontend queue tests assert current entry remains index zero after next/previous/mode changes.
- [ ] 3.3 Replace count-only history with timestamped, three-day, entry-ID-based records; persist and backward-compatibly restore it in desktop session state; verify expiry, restart, duplicate entries, deleted/missing records, and pending-order preservation.
- [ ] 3.4 Preserve restore verdicts as blocked entry state through coordinator/runtime/UI and report correct restored/blocked/drop counts; verify blocked entries remain visible, skip safely, and become retryable when availability returns.

## 4. Correct library query and playback-context boundaries

- [ ] 4.1 Include active-root identity in list query/cache/request generation and discard stale replies after a root switch; verify hook tests with delayed old-root replies.
- [ ] 4.2 Apply normalized search to all, favorites, and recent view definitions, and limit the all-songs sorting controls/requests to the all view; verify query and component tests for each view.
- [ ] 4.3 Add a desktop-resolved declarative playback-context path for paged library views so playback starts from the complete filtered, deterministically sorted view; verify tests with more than one page and root-switch validation.

## 5. Deliver acceptance evidence

- [ ] 5.1 Add or update a registered scenario covering offline active-root setup, playlist create/add, playback mode changes, previous-track history, restart recovery, and failure feedback; verify it is present in traceability and the verification manifest.
- [ ] 5.2 Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace --all-targets`, `pnpm --dir apps/desktop typecheck`, and `pnpm --dir apps/desktop test`; record any platform-native manual gates separately rather than marking them passed without evidence.
- [ ] 5.3 Run `openspec validate complete-phase-one-acceptance --strict` and the affected registered scenario commands; verify all planned artifacts and acceptance checks are green before requesting phase-one re-acceptance.
