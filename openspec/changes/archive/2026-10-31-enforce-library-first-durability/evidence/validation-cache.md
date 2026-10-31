# Exact-byte validation certificates (2026-10-24)

This implements the A0 prerequisite in the handoff. It does not complete tasks
3.7 or 10.4: batching, the 10k measurement and whole-sync convergence remain open.

`RootControlPlane` explicitly owns an `Arc<ValidationCache>` with at most 128
FIFO entries. A certificate contains the document kind (commit or coverage),
registered root UUID and BLAKE3 of the complete bytes just read. It contains no
parsed payload, filesystem metadata, error, replay or recovery result.

Every call still performs constrained filesystem reads and parses the current
document. Manifest ownership, operation identity, commit marker, predecessor,
sequence and materialization-watermark checks remain outside the certificate.
Only a successful intrinsic batch/digest or coverage checksum validation can
populate it. Missing/changed/symlinked documents cannot be served from cached
content. Empty, evicted, contended or poisoned caches run full validation.

The prepared-only `operation_status` path now also checks requested operation ID
and current manifest ownership on every read; it previously omitted those checks.

## Actual automated results

- `cargo test -p echo-core library_durability --lib`: 52 passed, 0 failed,
  21.67 s, including the separately added retained-evidence projection test.
- Cache regressions: 10 passed. Coverage includes cold/hot reads, equal-length
  tampering, deletion, different/repointed roots, changed manifest/marker,
  removed predecessor, coverage/materialization watermark, another adapter,
  prepared/committed operation identity, symlink, eviction and unavailable lock.
- `cargo clippy -p echo-core --lib --tests -- -D warnings`,
  `cargo fmt --all --check`, `git diff --check`, and
  `node scripts/verify/check-scale.mjs`: passed; allowlist unchanged.

One native macOS temporary-directory measurement used a 1 MiB private after-image
and eight actual constrained reads/parses: a shared warm adapter took 138.302 ms;
eight independent cold adapters took 332.729 ms. Warm intrinsic validation
counts were one full validation and seven certificate hits. Another test used
one cold plus eight hot reads and recorded `(full=1, hits=8)`. These counters only
exist in test builds; timing is a single observation, not a throughput guarantee.

The unchanged 513-commit native paging test and complete regression/governance
results are recorded in the continuation checkpoint once they finish. No fixture
size, ignore annotation, directory-first requirement or required sync boundary
was weakened for these measurements.
