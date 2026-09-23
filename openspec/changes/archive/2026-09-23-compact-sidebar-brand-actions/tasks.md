## 1. Sidebar action placement and behavior

- [x] 1.1 Add the Settings button and an Import portal slot to the Echo brand area, move the existing import button into that slot in the app shell, and remove the old Settings footer and topbar Import controls while preserving empty-state import behavior; verify desktop typecheck and lint pass.
- [x] 1.2 Reveal the pill-shaped controls across the full brand row on hover and keyboard focus, with 36px desktop targets, 44px mobile targets, and no idle layout width; preserve the search styling.

## 2. Prototype alignment and delivery validation

- [x] 2.1 Update the desktop prototype markup/styles and regenerate prototype-derived CSS while preserving the existing search rules; verify the generated diff contains the expected brand/action styles and no `.search` changes.
- [x] 2.2 Run the desktop production build and `openspec validate compact-sidebar-brand-actions --strict`; confirm the implementation still uses the existing import command and Settings view.
