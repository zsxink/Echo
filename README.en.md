# <img src="docs/prototype/echo-icon.svg" width="28" height="28" alt="" /> Echo

[简体中文](README.md)

Echo is a local-first, cross-platform music player. Scan and manage your own music library, view cover art and lyrics, and organize playlists. Echo has no account system or streaming catalog. Desktop devices can currently sync libraries over a local network.

> **Latest release:** [v0.3.0](https://github.com/zsxink/Echo/releases/tag/v0.3.0) — macOS, Windows, and Linux installers (DMG / EXE / deb / AppImage)

## Product

- **Local-first:** Choose a local library root and Echo scans its music files. Playback, library management, and search work offline.
- **No accounts:** Your library stays yours; there are no accounts or recommendation feeds. LAN sync uses a desktop host to transfer changes and media between devices.
- **Everyday listening:** Import, browse, search, sort, favorite, and organize tracks into playlists. Use the playback queue, immersive vinyl player, and lyrics that follow playback.
- **Cross-platform roadmap:** The desktop app supports macOS, Windows, and Linux. LAN sync is implemented. Android, remote MinIO / S3 storage, and iOS remain planned work.

## Desktop features

- **Library management:** Select and scan a library root, parse tags, cover art, and lyrics, watch for directory changes, or rescan manually. Imported tracks are copied into `media/` and organized by artist and title. Duplicate content is not imported twice; conflicting filenames receive a suffix, and existing files are never overwritten.
- **System integration:** macOS provides a menu-bar status item and Control Center Now Playing integration. Windows and Linux provide a system tray. Echo can remain available after the main window closes. Set Echo as the default music player to open files from your file manager.
- **Playback:** Manage a persistent queue, playback position, volume, and keyboard shortcuts. The immersive vinyl player includes lyrics from embedded tags, matching `.lrc` files, or user overrides; tracks without lyrics show an empty state.
- **Personalization:** Choose from three themes (Coral, Cobalt, and Turquoise). Choose whether closing the main window quits Echo or leaves it running in the background.
- **Non-destructive overlays:** Cover art, lyrics, and metadata overlays do not modify the original files. Overlays take precedence over embedded data in the app.
- **LAN library sync:** A desktop host coordinates library changes between devices. Each device keeps its own SQLite projection; Echo does not sync SQLite database files.
- **Audio effects:** The 0.2.0 P0 implementation includes a ten-band equalizer, built-in presets, spatial effects, and user presets. Native audio validation on all three desktop platforms remains a release gate.

## Product principles

1. The music library comes before accounts and recommendation feeds.
2. Playback controls remain accessible, including in immersive mode.
3. Sync status should be clear and should not interrupt listening.
4. Cover art and track details help identify music; they do not replace efficient search.

## Related tool

[MusicTag](https://github.com/zsxink/MusicTag) edits tags, cover art, and lyrics in local music files and writes changes back to those files. Echo scans, organizes, and plays a local library. See the [MusicTag repository](https://github.com/zsxink/MusicTag) or [releases](https://github.com/zsxink/MusicTag/releases).

## Status and roadmap

The latest desktop release is v0.3.0. GitHub Actions builds macOS, Windows, and Linux installers with `SHA256SUMS`; see [Releases](https://github.com/zsxink/Echo/releases). Runtime, native E2E, and recovery-matrix validation is complete on macOS. Native E2E and recovery coverage on Windows and Linux is still expanding.

LAN sync has been implemented and archived. Device-matrix, real-device propagation-latency, and concurrency/scale acceptance tests remain outstanding. The 0.2.0 audio-effects P0 implementation is also in place, but native audio validation on all three desktop platforms remains a release gate.

| Stage | Focus | Status |
|---|---|---|
| Phase 1 | Desktop music player | v0.3.0 released; some cross-platform native validation continues |
| Phase 2 | LAN library sync | Implemented; real-device and scale acceptance tests remain |
| Phase 3 | Android over LAN | Planned |
| Phase 4 | Remote storage sync with MinIO / S3 | Planned |
| Phase 5 | iOS | Planned |
| 0.2.0 | Audio effects and EQ P0 | Implemented; native release validation remains |

See the detailed [roadmap](docs/ROADMAP.md), [product scope](docs/PRODUCT.md), [design](docs/DESIGN.md), and [interface terminology](docs/interface-terminology.md).

## UI prototype

See the [desktop player prototype](docs/prototype/echo-desktop-player.html) and the icons in the same directory.

## Technology

The Rust Core handles the library, imports, tag parsing, and sync rules independently of the UI and player. The desktop UI uses Tauri 2, React, and TypeScript; playback uses mpv/libmpv with platform-specific integration. The planned mobile stack is Flutter with media_kit.

## Development

### Repository layout

```
crates/
  echo-core/        Domain models, use cases, and SQLite infrastructure
  echo-desktop/     Desktop runtime, playback coordination, IPC DTOs, platform integration
apps/
  desktop/
    src/            React shell (bridge, features, player, styles)
    src-tauri/      Tauri shell, commands, dialogs, security capabilities, native integration
    e2e/            Native E2E tests (real shell + gate injection)
openspec/           Specifications and change management
tests/
  scenarios/        Executable scenario manifest
  native/           Multi-platform manual scenario manifest
scripts/
  verify/           Verification runner and task/scenario manifest
fixtures/           Audio, cover art, and subtitle fixtures
```

### Requirements

| Dependency | Version | Notes |
|---|---|---|
| Node.js | ≥ 22 (see `.nvmrc`) | Frontend and scripts |
| pnpm | 11 (`packageManager` pins `pnpm@11.18.0`) | Enable with `corepack enable` if needed |
| Rust | 1.98.1 (`rust-toolchain.toml`) | Includes `rustfmt` and `clippy`; first build takes longer |
| libmpv | Vendored for macOS in the repository | Windows and Linux require platform-specific libraries |

### Run locally

```sh
git clone <repo> && cd Echo
pnpm install
pnpm echo dev
```

`pnpm echo dev` runs `generate:ipc`, builds the frontend, then starts `tauri dev`. The Tauri development configuration loads the built frontend from `apps/desktop/dist`, so the frontend must be built before launch.

Other commands:

```sh
pnpm echo release       # Generate IPC types, build frontend, and create Tauri packages
pnpm echo build         # Build without launching
pnpm echo help          # List available commands
```

The first `cargo` build downloads and compiles Rust dependencies and may take a while. Pushing a tag such as `v0.1.0` triggers the [release workflow](.github/workflows/release.yml), which builds installers and publishes a GitHub Release with `SHA256SUMS` and third-party license information. Current packages are not signed or notarized. On macOS, the first launch may require approval in Privacy & Security; signing and notarization remain future work.

App data is stored under `$APP_DATA/com.zsxink.echo` (a single SQLite database and a `covers/` cache).

### Verification

Task and scenario commands are registered in `scripts/verify/manifest.json`:

```sh
node scripts/verify/run-task.mjs -- <task-id>…      # Run selected tasks, e.g. 8.10 13.8
pnpm verify:scenario -- <SCENARIO-ID>               # Run a scenario
pnpm verify:scenario -- --all                       # Run all scenarios
node scripts/verify/reconcile-scenarios.mjs         # Reconcile specs, traceability, and manifest
```

Quality checks:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace --all-features
pnpm --dir apps/desktop typecheck
pnpm --dir apps/desktop test
```

### Specifications and changes

OpenSpec specifications live in `openspec/specs/<area>/spec.md`; proposed changes and their deltas live under `openspec/changes/`. Scenario IDs are derived deterministically by `scripts/verify/spec-scenarios.mjs`; `docs/traceability.md` is the naming authority. Change validation uses registered check scripts as evidence for completed tasks, and `openspec validate` checks change structure.

### Phase 1 scope note

The original Phase 1 scope excluded sync UI and network connections. That is historical scope, not the current product limitation: desktop LAN sync was implemented in Phase 2. See the [roadmap](docs/ROADMAP.md) for implementation and acceptance status.

## License

Echo uses a custom license based on the Business Source License 1.1 for personal learning and research. Commercial use, resale, and redistribution are prohibited. On the Change Date (2099-12-31), the code will be available under GPL-2.0-or-later. See [`LICENSE`](LICENSE).

Third-party dependency licenses are listed in [`LICENSES.md`](LICENSES.md), maintained through `cargo deny` and the frontend production-dependency audit.
