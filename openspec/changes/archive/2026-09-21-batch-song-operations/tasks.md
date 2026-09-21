## 1. Selection model and batch result primitives

- [x] 1.1 Add a typed `useSongSelection` hook/state module that stores selected Song UUIDs, toggles individual rows, selects only currently loaded rows, reports counts, clears on view/query/root identity changes, and verify unit tests cover toggle, clear, select-all and cross-page UUID persistence.
- [x] 1.2 Add typed batch action/result helpers for success, skipped and failed items, including stable display order and error summaries, and verify unit tests prove failures remain observable without converting them to success.
- [x] 1.3 Add the shared sequential batch runner for favorite, playlist membership, queue and delete commands, including reverse submission for “下一首播放”; verify runner tests cover ordering, partial failures and no unhandled rejected promises.

## 2. Shared song list selection and accessible interaction

- [x] 2.1 Extend `SongList` and `SongRow` with an explicit multi-select mode, mode-only accessible selection controls, loaded-only select-all, `bulk-selected` styling, and selection-mode exit behavior without changing row-click playback; verify existing playback/windowing tests still pass and new tests cover virtualized rows leaving/re-entering the viewport.
- [x] 2.2 Add row context-menu handling so right-clicking an unselected row makes it the sole selection while right-clicking an already selected row preserves the full selection; verify tests cover `contextmenu`, no accidental playback, and selected-count labeling.
- [x] 2.3 Add keyboard menu-key/`Shift+F10` handling and batch toolbar focus/escape behavior through the existing overlay stack; verify accessibility tests cover visible focus, Escape dismissal and focus restoration.
- [x] 2.4 Add the list/header CSS for the mode-only selection column, selected rows and loading/busy feedback without conflating playback `.selected` with bulk selection; verify `pnpm --dir apps/desktop format:check` passes.

## 3. Batch menu, toolbar and playlist picker integration

- [x] 3.1 Refactor song action descriptors/menu rendering so single-song and batch contexts share placement, overlay, focus and permission rules while hiding single-song-only detail/reveal/play actions from batch mode; verify menu tests cover read-only, unavailable and both right-click/toolbar entry paths.
- [x] 3.2 Extend `AddToPlaylistDialog` to accept one or many song IDs while preserving the existing single-song API behavior, execute the selected targets through the typed batch runner, and display aggregated success/duplicate/failure feedback; verify existing dialog tests and new multi-song tests pass.
- [x] 3.3 Add the right-click batch menu actions for favorite/unfavorite, add-to-playlist, play-next, enqueue, delete, and playlist-only remove, without a top batch toolbar; verify rendered labels, selected counts, disabled read-only actions and empty-selection guards with component tests.

## 4. Library workspace behavior

- [x] 4.1 Wire `LibraryWorkspace` to own the view-scoped selection key and connect the explicit multi-select mode, selection column, right-click menu and shared runner to all library views; verify search, sort, view and root changes clear selection while pagination preserves loaded selections.
- [x] 4.2 Implement library batch favorite/unfavorite refresh using committed `SongView` broadcasts, authoritative reset and count reconciliation; verify favorites-view removal, optimistic rollback and mixed success/failure feedback.
- [x] 4.3 Implement library batch delete with sequential `delete_song` calls, per-item unavailable/error reporting, count/list refresh and one 10-second toast action that invokes `undo_delete` for all successful operation ids; verify full success, partial failure, undo success and partial undo failure scenarios.
- [x] 4.4 Keep single-song menu behavior unchanged for play, detail, reveal and delete confirmation while routing batch-capable actions through the shared result surface; verify existing `LibraryWorkspace`, `SongMenu` and delete-flow tests remain green.

## 5. Playlist workspace behavior

- [x] 5.1 Wire `PlaylistsView` to the shared selection model and expose explicit multi-select mode, loaded-member selection, all-loaded selection and right-click batch menu without changing playlist ordering; verify selection reset on playlist identity changes and member refresh.
- [x] 5.2 Implement batch add-to-playlist and current-playlist removal, preserving member UUID/tombstone semantics and refreshing members/navigation only after submitted operations; verify duplicate, partial failure and unavailable-member scenarios.
- [x] 5.3 Ensure read-only playlists/root state hides or disables batch writes while still allowing permitted read/play actions; verify parity with existing single-song read-only tests.

## 6. Verification and delivery gates

- [x] 6.1 Add/adjust React tests for all change scenarios: multi-select, loaded-page persistence, right-click/keyboard menu, favorite reconciliation, playlist batch mutations, delete/undo window, unavailable-item feedback and read-only restrictions; verify `pnpm --dir apps/desktop test` passes.
- [x] 6.2 Run desktop type, lint and build checks and fix all findings: `pnpm --dir apps/desktop typecheck`, `pnpm --dir apps/desktop lint`, `pnpm --dir apps/desktop build`.
- [x] 6.3 Run formatting and repository governance checks: `pnpm --dir apps/desktop format:check`, `pnpm verify:governance`, and `openspec validate batch-song-operations --type change --strict`.
- [x] 6.4 Run Rust workspace format/lint checks to prove no Core or command boundary drift: `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
