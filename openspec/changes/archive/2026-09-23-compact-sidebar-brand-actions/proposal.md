## Why

The current standalone settings entry and prominent import button consume valuable sidebar and toolbar space. Grouping these actions with Echo's brand mark keeps them easy to discover while leaving the library search and song workspace less crowded.

## What Changes

- Reveal compact Settings and Import actions when the pointer is anywhere over the Echo brand area, including the logo and wordmark.
- Remove the existing standalone Settings and toolbar Import controls while retaining their current actions.
- Keep the search field's existing appearance and behavior unchanged.
- Ensure keyboard users can reach both actions even when pointer hover is not available.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `desktop-app-shell`: Define compact brand-adjacent Settings and Import controls, their whole-brand hover trigger, and keyboard-accessible behavior.

## Impact

- Affects the desktop React application shell/sidebar and library workspace toolbar.
- Reuses the existing Settings view and library import flow; no IPC, Rust, data, or dependency changes are expected.
- Follows the desktop prototype and `docs/interface-terminology.md`; implementation stays in the presentation layer and follows `openspec/CODE_STANDARDS.md`.
