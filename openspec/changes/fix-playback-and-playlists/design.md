## Context

See `proposal.md` for motivation and the three delta specs for behavior. The desktop player already has an authoritative Rust-side player/coordinator and a snapshot-driven React store; `PlayerSnapshot` exposes `position`, while the session restore path deliberately loads the restored item without seeking to its saved position. The playlist name dialog already owns creation validation and the typed `create_playlist` command, but the song-level picker does not complete its nested creation handoff. The desktop shell has close preferences and platform integration boundaries, yet the requested background lifecycle is not delivered end to end.

This remains a desktop/platform change. `echo-core` must stay independent of Tauri, mpv, React and platform tray APIs. Existing IPC DTO generation remains the source of truth for UI snapshots.

## Goals / Non-Goals

**Goals:**

- Persist and restore the authoritative player position through the desktop session boundary, while preserving paused-on-restore semantics.
- Correct presentation-only mode styling without changing queue or playback mode state-machine behavior.
- Reuse the existing playlist creation command and dialog from the picker, preserving asynchronous failure state and active-root validation.
- Connect close preference, window hiding, platform menu/tray controls and explicit exit to one coherent desktop lifecycle.
- Normalize all user-visible desktop product identity to `Echo`.

**Non-Goals:**

- Changing Core queue, shuffle, song identity, database schema, playlist synchronization, or media playback engine behavior.
- Adding mobile background playback or a new cross-platform abstraction beyond the desktop platform adapter.
- Changing the three existing theme choices or using theme color to express random-mode selection.

## Decisions

### Keep playback-session persistence at the desktop player boundary

Extend the existing desktop session representation and its serialization/restore flow to include the last authoritative position for a persistent library queue entry. Snapshot publication updates the in-memory session candidate using validated finite non-negative values; mutations that change playback context and controlled lifecycle transitions flush it. Restore will load the entry paused and issue a bounded seek only after the player reports that the item is ready, preventing a seek from racing asynchronous mpv loading.

The persisted position will be clamped to the known duration when available and dropped for temporary/non-library items. Corrupt or out-of-range values will fall back safely instead of blocking startup.

Alternative considered: storing position in React or browser local storage. Rejected because the Rust desktop runtime is authoritative, survives WebView lifecycle changes, and must also save state on tray-driven exit.

### Make random-mode color a presentation rule, not a playback-state rule

Adjust the non-immersive transport control class/attribute mapping so the random mode icon stays neutral while retaining semantic selected state (`aria-pressed`/accessible label). Leave the player coordinator and IPC `mode` unchanged. Theme tokens remain available for focus and press interactions.

Alternative considered: adding a special mode-color token or changing the mode enum. Rejected because the defect is confined to visual emphasis and neither change contributes behavior.

### Compose picker creation from the existing playlist dialog and command

The song action's playlist picker will own a nested creation state. Selecting “新建歌单” opens `PlaylistNameDialog` with the active root and current playlist names. Its successful callback reloads the authoritative playlist list, adds the returned playlist ID to the picker selection, and returns focus to the picker; confirmation then uses the existing add-to-playlist mutation. The outer picker remains mounted during validation or IPC errors so input and prior selections survive.

Alternative considered: let the picker call `create_playlist` directly. Rejected because it would duplicate validation, cover handling and recoverable-error behavior already centralized in `PlaylistNameDialog`.

### Treat close, tray controls and exit as a desktop lifecycle adapter

Use the Tauri runtime/platform layer as the lifecycle adapter: window close is prevented and hidden only when the persisted close behavior is background mode; explicit app/tray exit marks the exit path, persists the player session, stops the desktop player and then exits. Tray/menu commands route through the same runtime player command methods used by the UI so they cannot create a second player instance. Tray/menu text and notifications derive from a single `Echo` product-name constant/configuration source.

Alternative considered: keep the window alive off-screen without tray integration. Rejected because it offers no discoverable background control and fails the desktop shell contract.

### Normalize identity at every desktop-owned source

Audit Tauri configuration, Rust menu/tray/window strings, React settings/about text and package/bundle display metadata. Replace user-facing `echo` naming with `Echo`, while retaining lowercase technical identifiers such as package IDs, command names, paths and crate names where platform tooling requires them.

## Risks / Trade-offs

- [Seek races with asynchronous player load] → wait for the desktop player-ready event and apply one bounded, idempotent restore seek; test that restore remains paused.
- [Frequent playback snapshots cause excessive disk writes] → throttle/debounce position persistence and force a final flush on context-changing and exit lifecycle boundaries.
- [Nested picker/dialog overlays lose focus or state] → retain the picker state beneath the blocking dialog and use the existing overlay/focus-stack conventions in component tests.
- [Platform tray behavior differs across macOS, Windows and Linux] → keep platform-specific creation in the Tauri adapter, preserve a visible quit path on tray initialization failure, and cover deterministic Rust/unit paths plus platform smoke checks.
- [Replacing lowercase identifiers broadly breaks package integration] → limit normalization to user-visible display fields; do not rename technical package, crate, IPC or filesystem identifiers.

## Migration Plan

1. Add backward-compatible optional position data to the desktop-local session format; missing or invalid values restore from the beginning without migration or data loss.
2. Release the UI and desktop runtime changes together so tray exit uses the new final session flush.
3. Roll back by accepting the optional field but ignoring it; existing local session files remain readable.

