# Desktop Floating Lyrics — Tasks

> 依赖顺序：偏好契约 → 窗口壳 → 前端歌词窗 → 设置 → 平台行为 → 整体验证。每步含验证命令；分层内逐个可独立验证。

## 1. Local preference contract

- [ ] 1.1 Extend the `local_state` document with an optional `lyricsWindow` block (enabled / fontSize / opacity / textColor / highlightColor / backgroundEnabled / blurEnabled / locked / alwaysOnTop / position), keep uncorrupted legacy documents loadable by defaulting missing fields, and verify `cargo test -p echo-desktop platform::local_state` covers defaults, safe fallback on corruption, and unknown-enum fallback
- [ ] 1.2 Add `lyrics_window` read/write accessors and a `LyricsWindowPrefs::Safe` default to `DesktopStateStore` using the existing atomic `mutate` path, and verify the store tests cover write-failure-leaves-previous-file and round-trip through load

## 2. Lyrics window shell

- [ ] 2.1 Add a `lyrics` window capability with a minimal permission whitelist (`core:default`, window introspect, `get_lyrics`-style command exposure, player snapshot event subscription) that does not expand the main window's capability or CSP, and verify `cargo test -p echo-desktop platform::security` still passes and the capability diff-drift tests (if any) stay green
- [ ] 2.2 Assemble the `lyrics` window in the desktop `setup`/composition root with `decorations: false`, `alwaysOnTop: true`, `skipTaskbar: true`, and default size/position, wiring show/hide/close/drag and position persistence commands, and verify `cargo test -p echo-desktop` shell tests plus a `cargo clippy -p echo-desktop --all-targets` pass
- [ ] 2.3 Route the Vite SPA to render a `lyrics` mount (same bundle, distinct route/query) and verify `pnpm build` produces a bundle the `lyrics` window can load without expanding the CSP

## 3. Lyrics window frontend

- [ ] 3.1 Render the floating lyrics panel from the shared `player://snapshot` (`currentSongId`, `position`, `currentLyrics`) and `get_lyrics` for library songs, deriving the effective lyrics with the same source semantics, and verify `pnpm test` covers library vs temporary lyrics and failure-downgrades-to-empty
- [ ] 3.2 Implement current-line highlight via the same timestamp binary search and click-to-`seek` (snapshot-corrected, snapshots stay authoritative), clearing content on song switch, and verify the component tests cover line advance, seek, plain-text (no highlight/no seek), and no-lyrics empty state
- [ ] 3.3 Apply appearance prefs from the store (font size, opacity, text/highlight/background colors, blur toggle, locked, alwaysOnTop) to the panel and one scroll container, and verify the React tests confirm each pref maps to the rendered style/state without fabrication when the snapshot is stale

## 4. Settings surface

- [ ] 4.1 Add a "歌词悬浮窗" section to the settings dialog: enable toggle, appearance controls, and lock/always-on-top controls bound to the preference setters, and verify the settings component tests cover enable→assembly request and each control's request payload
- [ ] 4.2 Surface a safe default + corruption notice when prefs are unreadable, and verify the settings tests cover the fallback path without blocking the settings dialog

## 5. Platform behavior

- [ ] 5.1 macOS: ensure always-on-top uses the floating window level, the lyrics window survives main-window hide (background), and close semantics match the saved behavior, and verify `cargo test -p echo-desktop` + targeted shell tests on macOS
- [ ] 5.2 Windows/Linux: ensure always-on-top + skip-taskbar, lyrics window visibility follows the background/exit close preference, and drag/position persistence works, and verify `cargo test -p echo-desktop` plus a Linux/Windows cross-check build step (`cargo check --target x86_64-pc-windows-msvc` or Linux CI job) where the runner allows
- [ ] 5.3 Respect `prefers-reduced-motion` (instant line positioning, no smooth scroll) and keyboard/AT exposure (readable labels, visible focus, aria-current on the highlighted line), and verify the React tests cover the reduced-motion style binding and aria-current

## 6. Overall verification

- [ ] 6.1 Run `cargo fmt --check`, `cargo clippy --workspace --all-targets`, `pnpm format:check`, `pnpm lint`, `pnpm typecheck`, `pnpm test`, and `git diff --check`; record results in the implementation handoff
- [ ] 6.2 Run a three-platform manual smoke matrix (open lyrics window, always-on-top over another app, drag, follow across song switch, click-seek, appearance changes apply live, background-hide keeps it visible on macOS, reduced-motion instant jump) and record outcomes per platform

## 7. Docs & terminology sync

- [ ] 7.1 Add "歌词悬浮窗" to `docs/interface-terminology.md` and note the new capability in `docs/PRODUCT.md` (capabilities list) and/or `docs/ROADMAP.md` where the desktop experience is enumerated, and verify `git diff --check` passes with the doc updates
- [ ] 7.2 Run `openspec validate --change desktop-floating-lyrics --strict` (or the project's configured validate command) and confirm the change is spec-complete before archiving