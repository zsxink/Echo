# Task 2.5 song alias implementation progress (2026-10-15)

Task 2.5 and RemoteApply task 3.4 remain unchecked.

Implemented:

- Host confirmation chooses the first live host-confirmed same-BLAKE3 song UUID. The canonical song retains its first import time, while the new UUID becomes an immutable typed song alias in the same log/SQLite transaction.
- Original candidate/outbox bytes and inbox request digests remain unchanged. Host-only normalization redirects old song/favorite/override/play-stat keys and playlist song references before arbitration.
- Replica commit apply persists typed song aliases transactionally. Canonical projection recognizes pending alias overlays, merges local device statistic buckets and moves noncolliding playlist memberships, while retaining the old hidden song row for pending request root checks and resource/journal recovery.
- Alias deletion targets the original canonical UUID. A subsequent same-byte reimport uses a new UUID; historical alias retries cannot remap to it or revive the deleted UUID.
- Same path/different content receives a stable UUID-suffixed destination; matching sidecar paths receive the same suffix.
- Distinct resources submitted with a duplicate audio import remain durable held declarations in alias-keyed resource manifests and resource references; the first canonical resource set is retained.

Remaining acceptance boundaries:

- A local playlist containing both song UUIDs keeps both original membership rows until the host confirms its playlist mutation and emits a typed member alias. The song alias transaction deliberately preserves the historical member identity instead of deleting it. Full UI/queued-delete convergence for this intermediate state remains to be verified.
- Conflicting alias-sidecar/cover versions are held locally but are not distributed as conflict resources through initial snapshots or presented as a resolution flow. Their canonical association needs the full resource conflict lifecycle.
- Old local song rows are hidden using pending_delete availability and a private database path placeholder. Catalog, recovery, scanner and stale playback-ID interactions need independent review before treating the alias state as production-complete.
- Tests exercise SQLite host/replica application. No native two-computer run is claimed.

Validation:

- `cargo test -p echo-core song_alias --locked`: 5 passed.
- `cargo test -p echo-core sync_projection --locked`: 6 passed.
- `cargo clippy -p echo-core --all-targets --all-features --locked -- -D warnings`: passed.
- `cargo fmt -p echo-core --check`: passed.
- `git diff --check`: passed.

The exact test module is `infrastructure::sqlite::tests::lan_ledger::song_aliases`; `sync_payloads::song_aliases` is not its module path and matches no tests.
