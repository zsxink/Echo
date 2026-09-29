## Why

On macOS, clicking the red close button while Echo is in native full screen hides the main window without first ending the fullscreen Space, leaving a black fullscreen display. The shell must exit native full screen before honoring the background-close behavior so the app can remain in the menu bar and reopen as a normal window.

## What Changes

- When a background close is requested from native fullscreen on macOS, exit fullscreen first and hide the main window after the native transition finishes.
- Preserve the existing background playback and menu-bar behavior; reopening the window returns it to windowed mode.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `desktop-app-shell`: define recovery behavior after exiting native macOS full screen.

## Impact

- Rust Tauri shell lifecycle in `apps/desktop/src-tauri`.
- The existing `desktop-app-shell` capability; no Core, persistence, dependency, or public command changes.
- Product scope and platform boundaries follow `docs/PRODUCT.md`, `docs/ROADMAP.md`, and `docs/DESIGN.md`; code follows `openspec/CODE_STANDARDS.md`.
