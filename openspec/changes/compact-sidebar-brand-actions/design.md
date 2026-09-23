## Context

See `proposal.md` for motivation and `specs/desktop-app-shell/spec.md` for observable behavior. `App.tsx` owns the Echo mark, Settings visibility, and sidebar layout; `LibraryWorkspace.tsx` currently owns the import request, result feedback, and toolbar/empty-state triggers. The search field styling lives in the prototype-derived shell styles.

This is a desktop presentation-layer change. It does not alter Tauri commands, Core behavior, filesystem access, persistence, or shared UI-independent logic.

## Goals / Non-Goals

**Goals:**
- Make the Echo brand row the common home and pointer trigger for Settings and Import.
- Keep both actions keyboard reachable and visually sized to the 28px brand mark.
- Remove the old standalone Settings footer link and large topbar Import button without changing search styling or import semantics.

**Non-Goals:**
- Redesign the library toolbar/search field or empty-library import prompt.
- Change import rules, result handling, read-only behavior, or settings contents.

## Decisions

- Keep the brand and Settings state in `App.tsx`, and keep import orchestration, progress, and feedback in `LibraryWorkspace`. Provide a brand action slot from the shell and use React Portal to render the existing import button into that slot. This changes where the control appears without moving its state owner or changing the typed `choose_and_import_files` flow.
- Render the two icon buttons as children of one brand-area hit target. The hit target spans the full existing brand row. Hide the action group's visual width while idle and reveal it on `:hover` or `:focus-within`; keep controls in the tab order with accessible names and a visible focus ring. This avoids pointer-only access and does not reserve visible layout width when idle.
- Use the existing Settings view and `Icon` component, adding matching gear and upload glyphs to its existing glyph map. Size the buttons to the 28px brand mark and use existing theme tokens.
- Update the prototype source and regenerate prototype-derived CSS rather than hand-editing generated declarations. Leave `.search` declarations unchanged.
- Keep the change within React presentation and CSS: UI calls the existing typed bridge command; Rust/Core and IPC contracts remain untouched. This follows the existing presentation-to-platform adapter boundary without introducing a new dependency or shared abstraction.

Alternatives considered:
- Keeping import orchestration in `LibraryWorkspace` would require a cross-tree event, portal, or registration callback to make the sidebar action invoke it. Moving the small shell-level orchestration to `App.tsx` gives the brand and workspace empty state one normal callback while keeping the backend boundary unchanged.
- Moving import orchestration to `App.tsx` would broaden shell responsibilities and duplicate cross-view behavior; a portal keeps the library feature as the owner while placing the button at the shell-level brand slot.
- Removing hidden buttons from the DOM until hover would make them inaccessible to keyboard users; retaining focusable buttons and revealing the group on focus avoids that trade-off.

## Risks / Trade-offs

- **Keyboard focus could land on visually hidden actions** → reveal the whole group on `:focus-within`, keep focus styles visible, and verify tab navigation.
- **The brand row could feel crowded at narrow widths** → use compact icon-only controls with the same 28px footprint as the mark; verify the narrow sidebar.
- **Moving import state could drop a feedback path** → preserve cancellation, invalidation, refresh, toast, and failure-dialog scenarios when relocating the handler.

## Migration Plan

No data or API migration is required. Revert the UI and prototype/CSS edits to roll back. Validate the desktop type/lint/build checks and OpenSpec requirements before pushing the feature branch.
