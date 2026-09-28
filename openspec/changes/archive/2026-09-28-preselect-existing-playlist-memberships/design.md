## Context

See `proposal.md` for motivation and the delta specs for observable behavior. The picker currently loads only playlist summaries; membership data is available from Core in the playlist-to-members direction. The change crosses Core, the desktop service/Tauri IPC boundary, generated TypeScript IPC types, and the React picker. It reuses the existing playlist membership table and does not change persistence or sync formats.

## Goals / Non-Goals

**Goals:**

- Retrieve one song's current playlist IDs with one authoritative Core repository query.
- Keep Core independent of Tauri and UI types; expose a typed desktop command and generated bridge type.
- Separate existing membership from newly selected targets in single-song sessions, while preserving batch additive semantics.
- Cover query errors, no-op confirmation, and membership immutability with component and repository/service tests.

**Non-Goals:**

- Removing a song from a playlist in the add picker.
- Fetching per-song membership for each song in a batch.
- Changing duplicate-add behavior, schema, sync payloads, or playlist creation flow.

## Decisions

1. **Use a focused repository query.** Extend `PlaylistRepository` with a song-to-playlists lookup, implement it in SQLite with a single indexed membership-table query and in-memory repositories by filtering stored memberships. A small application use case delegates to the port. This follows the existing repository and Ports-and-Adapters boundaries; loading every playlist's members would cost one IPC/requested collection per playlist and would expose more data than needed.

2. **Expose a typed desktop command.** Add an `AppServices` catalog/playlist operation and a Tauri command accepting the established snake_case `song_id` argument, parse it as `SongId`, return playlist IDs as strings, and register it through the existing command list. Update the TypeScript bridge command map through the checked-in IPC generator, rather than editing generated types by hand. This is an additive IPC contract; no migration or compatibility shim is needed because desktop UI and binary ship together.

3. **Keep existing IDs outside mutable selection.** In the single-song picker, store authoritative memberships as a separate immutable set and user additions as the mutable set. Render preselected items as disabled/read-only with explicit “已在其中” semantics; the confirm payload is `selected - existing`. The picker can close successfully when this difference is empty without invoking the mutation. Inline creation continues to add the returned ID to mutable additions.

4. **Treat batch as additive across the whole selection.** Do not query N songs or infer a single membership set from them. Keep batch selection initially empty and explain that selected playlists apply to every selected song; Core's existing idempotent add handles songs already present. This makes the shared checkbox semantics explicit and avoids an N×playlist lookup pattern.

5. **Make lookup failure non-blocking and visible.** Separate playlist-list errors from membership-query errors. If the song membership query fails, leave all items unselected, show a warning that current membership could not be loaded, and preserve the add action. This follows the established recoverable UI error pattern while avoiding a false claim that the empty set is authoritative.

## Risks / Trade-offs

- [The async lookup could complete after the picker closes or a song context changes] → Cancel state writes on unmount and key the request to the selected single-song ID.
- [Read-only preselected choices can be confused with disabled playlists] → Include visible “已在其中” text plus disabled semantics and tests for accessible state.
- [Batch users cannot inspect partial membership in this picker] → State that targets apply to all selected songs and retain idempotent additive behavior; per-song membership management remains out of scope.
- [Generated IPC types may drift if the generator is skipped] → Run the generator and the repository's IPC drift validation.

## Migration Plan

No data migration is required. Add the Core query, expose and generate the additive desktop IPC command, then ship the picker using that command. Rollback consists of reverting the code and OpenSpec delta; the stored membership model and existing add/remove commands remain unchanged.

## Validation

- `cargo fmt --all --check`
- `cargo test -p echo-core`
- `cargo test -p echo-desktop`
- Desktop component tests for preselection, difference-only submission, no-op confirmation, and query fallback.
- Desktop typecheck/lint and IPC generation drift checks as defined by package scripts.
- `openspec validate --strict`

## Architecture and Standards

The repository port and use case belong to Core application; SQLite and in-memory repositories are infrastructure/test adapters; `AppServices` and Tauri command form the desktop platform boundary; React owns presentation state only. The repository pattern limits SQL to infrastructure, and Ports-and-Adapters keeps the shared Core portable. Applicable standards: `openspec/CODE_STANDARDS.md` sections 2.1, 3.1–3.3, 4.1–4.2, 6, 8, and 9.1. The IPC addition is additive and does not alter stored data, cross-device protocol, or existing command semantics.
