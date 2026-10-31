# Durable retry fences and GC (2026-10-24)

Independent review of the exact-byte validation cache found two existing P1
windows. A visible file after rename did not prove that its publication's parent
directory had been synced. Neither issue was caused by a cache hit.

1. A marker publication could fail after rename but before syncing its operation
   directory. Recovery materialized the visible commit, and a same-operation
   retry could succeed without completing that fence's sync.
2. A coverage publication could fail before syncing `echo/state`. Recovery could
   then use the visible anchor to collect transactions without first making the
   anchor durable. A matching receipt from an earlier interrupted compaction
   similarly skipped its unfinished parent-directory sync.

## Repair

- Recovery flushes each unmaterialized commit's prepared/marker files and their
  complete operation-directory-to-root ancestor chain before applying any of
  that commit's after-images or advancing its materialization watermark.
- A successful marker publication can supply a proof only within that same
  call. No recovery-success certificate is retained across calls. The existing
  validated materialization checkpoint proves the earlier fence ordering for
  already materialized operations.
- Matching prepared payloads and matching operation receipts still complete
  file and ancestor-directory sync on retry; equality is not durability proof.
- Coverage is flushed before it authorizes redo. GC validates and flushes the
  anchor and **all** receipts for its pending deletion set before the first
  removal. Failure on a later receipt therefore cannot follow earlier journal
  deletion. A completed coverage image with no leftover transactions does not
  incur repeated coverage/receipt flushes.
- `flush_existing` resolves a constrained regular file, flushes it, and syncs
  ancestors through the registered root. On Windows its file handle is writable
  for `FlushFileBuffers`; platform errors propagate. This is not Windows native
  certification.

## Verification and independent review

Seven `tests::fences` tests passed (1.31 s). They cover repeated prepared/fence
sync failure, coverage/receipt publication followed by another recovery failure,
all-ancestor ordering before materialization/GC, failure on the second receipt
preserving every journal, coverage redo ordering, and no redundant flush after
completed recovery. These are fault hooks at real filesystem boundaries, not
claims of actual power loss.

The implementation worker ran 58 `library_durability` tests before the final
no-leftover regression was added, then the final 7-test fence group, Core
Clippy, format, whitespace and scale checks. The main agent runs the entire final
durability group separately, including its playback-privacy test; its result is
recorded in the continuation checkpoint.

The same independent reviewer rechecked both repairs and found no remaining
actionable issue in this scope. Tasks 3.2 and 3.3 now satisfy their commit-fence
and ordered materialization contracts. This does not finish bounded private
history/resource GC (3.5), ordered Application projection (4.2), state-consumer
retry (4.5), or the native/platform/performance matrices.
