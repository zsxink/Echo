## Why

The add-to-playlist picker hides a song's existing memberships, so users cannot tell which selections are already satisfied. Making membership visible while keeping the command additive removes that ambiguity without implying unsupported removal behavior.

## What Changes

- Add an authoritative Core query for the playlists containing a song and expose it through the desktop IPC boundary.
- Preselect existing memberships for single-song picker sessions, distinguish them from newly selected playlists, and submit only newly selected targets.
- Keep existing memberships read-only in the picker; define batch sessions as unselected additive operations because a single checkbox cannot represent per-song membership differences.
- Specify failure, no-change, and batch behavior in playlist and library experience requirements.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `playlist-management`: Define membership lookup, single-song preselection, additive diff submission, and batch picker semantics.
- `library-experience`: Define visible picker states and feedback when membership lookup fails or no changes are submitted.

## Impact

- Core playlist repository, application use case, memory and SQLite adapters.
- Desktop command registration, generated IPC types, bridge, and `AddToPlaylistDialog` UI/tests.
- No database migration or remote sync format change; membership is queried from existing playlist membership records.
- Follows the Core / desktop adapter boundary and repository pattern. Interface changes remain additive; existing add/remove commands keep their contracts.
- Product and design context: local-first library behavior and the playlist picker prototype; engineering constraints: `openspec/CODE_STANDARDS.md`.
