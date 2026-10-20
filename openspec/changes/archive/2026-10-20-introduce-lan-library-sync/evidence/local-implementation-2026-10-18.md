# Local implementation and verification — 2026-10-18

This record covers the current local working tree. It does not claim native-device, signed-installer, or multi-computer acceptance.

## Completed locally

- Portable v1 → v2 migration is journaled and recoverable. Tests cover partial writes, restart recovery, typed tombstones, unsupported versions, and filesystem failures; storage format notes are in `docs/DESIGN.md`.
- Resource imports, snapshot references, tombstone retention, remote deletion isolation, and operation-origin suppression have Core and integration coverage.
- Pairing code lifetime/consumption, certificate pinning and renewal, credential revocation, frozen enrollment snapshots, H → incremental catch-up, replication cursors, cancellation, and host resource publication are implemented and covered by sync/Core/app tests.
- The desktop settings flow exposes host/peer setup, direct IP plus six-digit pairing, discovery, device revocation, pause/resume, and restored-host recovery. A sync toolbar adds manual sync and queue/byte status; it refuses to show “已同步” unless queues are empty and media readiness is positively known.
- Native pending queue counts are uncapped and resource byte counters deduplicate immutable hashes and use persisted transfer checkpoints. Upload ACKs and persisted download checkpoints publish throttled progress during a sync cycle; unknown host/pre-enrollment values remain absent.
- Windows file sharing/lock errors leave publication resumable and media unready while allowing object state to advance. Completed operation staging is removed only when its journal, digest, and length prove a verified replacement exists. Staging with unknown provenance is intentionally retained.
- Restored-host recovery and host conflict listing/resolution are exposed through typed Tauri IPC and the settings UI. Ordinary field restoration creates a fresh local mutation; final tombstones direct the user to re-import with a new UUID. The recovery guide covers epoch fencing, peer re-pairing, diagnostics, and interrupted media publication.
- Settings include a root-scoped free-space hint and live upload/download byte checkpoints. Status events are fenced by library root, session, and event sequence. macOS `Info.plist` declares the local-network usage prompt and `_echo-sync._tcp` Bonjour service.
- The phase-one guard now asserts opt-in/default-disabled sync and no WebView remote network source.

## Local checks run

- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo build --workspace --all-features` — passed.
- Desktop format check, ESLint, typecheck, all 41 Vitest files / 371 tests, and production build — passed. Coverage includes storage-space hints, status ordering, and conflict recovery UI.
- `cargo test --workspace --all-features` — passed, including 646 Core tests, 420 desktop library tests, 66 app tests, and 51 sync library tests; the pre-existing slow app case and three mDNS multicast environment cases remain ignored.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo build --workspace --all-features`, and `cargo deny check` — passed. `cargo deny` reports existing duplicate `base64` versions and one unmatched license allowance, but no policy failure.
- `node scripts/verify/checks/task-13.8.mjs` — passed.
- `pnpm exec openspec validate introduce-lan-library-sync --strict` and `pnpm exec openspec validate --all --strict` — passed.

## Remaining acceptance

- Local implementation and tests for 4.5, 5.3, 5.4, and 7.1–7.4 are complete. The credential vault follows the approved design: a separate Echo app-data vault with Unix owner-only permissions and Windows current-user ACLs; it is not an Apple Keychain integration and does not trigger a system authorization prompt.
- Task 7.6's static desktop gates and macOS permission configuration are complete. Its signed installer checks remain grouped with native acceptance.
- Run the two-device matrix (macOS + Windows) and signed macOS/Windows installer acceptance last (8.3/8.4 and the installer portion of 7.6). These require the user's real devices and are intentionally still unchecked. There is no Linux machine available, so Linux installer behavior is explicitly outside this acceptance run and remains unverified.
- Task 8.6 is conditional on preparing a PR. This work has not prepared a PR, and archiving stays after native acceptance.
