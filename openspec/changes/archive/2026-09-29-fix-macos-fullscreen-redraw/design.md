## Context

See proposal.md for the reported failure. The Tauri shell owns the native window lifecycle. Under the macOS background-close preference, the existing close-request handler prevents native close and hides the window immediately. When that request arrives during native fullscreen, hiding before leaving fullscreen can strand the fullscreen Space and display a black screen.

## Goals / Non-Goals

**Goals:** End native fullscreen before hiding the main window for a background close, and wait for the native transition to complete.

**Non-Goals:** Changing normal windowed background-close behavior, changing playback state, implementing app-managed fullscreen, or altering Windows/Linux behavior.

## Decisions

- In the existing close-request path, prevent close as today. If the macOS main window is fullscreen, request `set_fullscreen(false)` and record a pending hide; otherwise hide immediately.
- Complete the pending hide only after a subsequent resize event observes the main window in windowed mode. Intermediate resize notifications while it is still fullscreen keep the hide pending. This uses the native lifecycle as the synchronization point instead of a guessed animation delay.
- Scope deferred-hide state and handling to macOS. Keep the existing windowed background-close path and all other platforms unchanged. No public command, generated IPC DTO, Core dependency, or new crate is needed.
- Cover the pending-hide state transitions with a deterministic Rust unit test and validate the macOS shell build and existing desktop checks.

## Risks / Trade-offs

- [The native resize event can arrive during the exit animation] → Keep hiding pending while fullscreen remains active; hide only after a resize event reports windowed state.
- [Fullscreen state reads or exit requests can fail] → Log the failure and leave the window visible rather than hiding it in an unconfirmed fullscreen state; the user can retry the close action.

## Migration Plan

No data or configuration migration is required. The change is rolled back by reverting the deferred-hide state and close-request sequencing together.
