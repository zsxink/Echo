## 1. Native fullscreen close lifecycle

- [x] 1.1 Change the macOS background-close path to exit native fullscreen before hiding, defer hiding until a resize event confirms windowed mode, and test intermediate/complete exit transitions with `cargo test -p echo-app fullscreen_close`.

## 2. Preserve other window lifecycle behavior

- [x] 2.1 Preserve the existing windowed close/reopen path; `cargo test -p echo-app main_window_activation` passed. Interactive native smoke validation is recorded under 3.1.
- [x] 2.2 Run desktop formatting, lint, type check, build, and frontend tests: `cargo fmt --all -- --check`, `cargo clippy -p echo-app --all-targets --all-features -- -D warnings`, `pnpm --filter @echo/desktop lint`, `pnpm --filter @echo/desktop typecheck`, `pnpm --filter @echo/desktop build`, and `pnpm --filter @echo/desktop test` (all passed; lint reports 6 existing warnings; 35 files / 309 tests passed).

## 3. Change validation

- [x] 3.1 `openspec validate "fix-macos-fullscreen-redraw" --strict` passed. Could not interactively verify fullscreen close/reopen because the Mac UI was locked and automatic unlock was unavailable; Rust transition and window-activation tests passed.
