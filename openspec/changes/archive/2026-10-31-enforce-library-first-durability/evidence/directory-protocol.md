# Directory protocol implementation checkpoint

Date: 2026-10-23. Branch: `codex/issue-72-library-first-durability`.

This is a serial integration checkpoint, not completion of the change or a
three-platform durability certification. No task checkbox or commit was made by
this protocol worker.

## Implemented behavior

- `LibraryBatch` validates stable non-nil operation/library/object identities,
  typed duplicate targets, foreign-library envelopes and state binding fields,
  invalid member parents, canonical contained resource paths, lower-case BLAKE3
  hashes, content-addressed asset names, duplicate resources, and unambiguous
  typed record preconditions. Absent historical timestamps remain absent.
- `ControlPlanePort` now has **15 methods, equal to main's 15**. The one-method
  `ControlPlaneInventoryPort` supertrait retains the existing cheap raw-file
  presence check, including malformed published JSON; its implementation is not
  a parsed-record enumeration. `LibraryDurabilityPort` has **6 methods**. There
  are no duplicate durability forwarding methods or scale-allowlist additions.
- Writers hold both an adapter mutex and an `fs2` exclusive file lock. The OS
  handle is scoped to the operation, including errors. Independent adapter and
  child-process tests prove writer exclusion while readers continue.
- Manifest, record, inventory, asset, status and original-operation reads do not
  create/acquire `write.lock`, recover migrations, materialize records or write
  progress markers. Verified committed after-images overlay unfinished on-disk
  materialization. A read-only mount without a lock file remains browsable.
  Pending legacy-format migration requires an explicit writable upgrade call.
- Prepared payloads use atomic exclusive publication (flushed temporary then
  exclusive hard link); commit markers contain protocol version 1, real local
  sequence and payload digest. Sequences and predecessor digests are checked.
  Reusing an operation with another payload is rejected. An uncommitted frozen
  batch may rebase its sequence after another writer advances the head; its
  business identity and payload remain unchanged.
- The publication primitive exposes a fault hook after each file sync,
  publication/rename boundary and ancestor-directory sync. Declared resource
  bytes are length/hash checked and flushed with their ancestors before the
  marker. Failures after publication or an unreadable uncertain marker retain
  the operation as `library_saved_pending_recovery`. Prepared-only operations
  report `NotSaved`; retrying them through the saved-operation recovery use case
  is refused.
- Replay validates the real chain, materialization checkpoint and unchanged
  materialized-file digests, then applies records/state in order. C1 failure
  followed by C2 editing the same target cannot roll the value back after retry.
  Broken predecessors, duplicate sequences, corrupt coverage, and corrupt
  materialization checkpoints fail closed while preserving their original bytes.

## Active-chain compaction and its exact limits

`LibraryReplay` carries an explicit complete `LibraryCheckpoint` coverage image
plus a suffix of real `LibraryCommit` values. It never manufactures an original
commit from a snapshot. Every 32 active commits, compaction:

1. Flushes immutable original-operation receipts to
   `echo/state/operations/<uuid-prefix>/<operation-id>.json`.
2. Flushes `echo/state/coverage-checkpoint.json`, containing version 1, a
   canonical image checksum, library ID, the last **real** sequence/head digest,
   latest typed records/state, and retained resource declarations.
3. Only after the complete image and ancestor directories are durable, removes
   covered prepared payloads/markers and empty transaction directories, syncing
   their parents. Interrupted cleanup is idempotent; receipts supply any already
   removed prepared payload. Unknown files are retained and block cleanup.

Ordinary recovery never enumerates historical receipt shards. The active chain
is below 32 commits after successful compaction; 256 transaction directories is a
hard safety cap. Snapshots are limited to 128 MiB and fail explicitly when they
cannot be represented safely. A typed final tombstone replaces that target in
the coverage image; archived original operations and original record files
retain historical evidence. Snapshot projection applies final tombstones first
so a deleted playlist name can safely be reused.

Old-operation retries fetch the **original** complete payload by operation ID,
return its original sequence, reject differing payloads and do not allocate a
new operation. A fresh SQLite projection can start from the coverage image; an
older watermark is verified against its original receipt before moving to that
image, then consumes the actual suffix. Tests cover both cases and repeated
projection without network mutations.

**This does not finish task 3.5.** Full original-operation receipts deliberately
retain historical storage, addressed by ID instead of scanned on each edit.
The image's typed private state still retains playback events, terminal pending
items/receipts and synchronization-ledger history. Those states can themselves
grow with history until the explicit snapshot limit is reached. A later typed
archive/retention design must move inactive private state out of the live image
without losing idempotence, unresolved pending/conflict/snapshot/delete-intent
references or original frozen payloads. No physical assets or media resources
are deleted in this checkpoint; this conservative policy protects unique bytes
but does not reclaim retired resource storage.

## Protocol layout and compatibility

- `echo/transactions/<operation-id>/prepared.json`: complete after-image batch,
  origin, expected hashes, resource declarations, sequence, predecessor and
  canonical commit digest.
- `echo/transactions/<operation-id>/committed.json`: version 1 commit fence,
  matching sequence and digest. Prepared alone is not a committed user change.
- `echo/state/materialized-checkpoint.json`: version 1 materialization progress,
  actual head sequence/digest and file checksum map.
- `echo/state/coverage-checkpoint.json`: version 1 self-checked complete image
  and chain anchor; bounded by current retained state, not a list of old commits.
- `echo/state/operations/<prefix>/<operation-id>.json`: immutable original
  operation evidence, read individually for idempotence/recovery.
- Typed tombstones use `records/tombstones/<deleted-kind>/<prefix>/<uuid>.json`;
  song/favorite/member identities do not share a deletion namespace. Alias and
  confirmed/pending state remain separately typed; compaction preserves their
  payloads and does not grant network trust or invent confirmation receipts.

This checkpoint retains the existing portable manifest/envelope compatibility
implementation (current envelope version 2). It does **not** deliver the full
legacy rescue, v3 manifest fencing against older clients, arbitrary unknown
state-schema validation, migration completeness reporting, or a full backup
inventory. The protocol-version numbers above must not be confused with a
completed v3 upgrade. The focused domain validation is not full completion of
all task 2.1 typed dependency schemas.

## Native filesystem assumptions

Tests run on the current macOS development filesystem. POSIX directory handles
are opened and `sync_all` is required; Windows uses
`FILE_FLAG_BACKUP_SEMANTICS` to open directories and also requires `sync_all`.
Unsupported flush, exclusive publication or replacement returns an error; it
never silently claims reliable success. Atomic replace/exclusive hard-link
semantics and directory flushing have not been executed on Windows or Linux in
this checkpoint. The read-only test removes `write.lock`, changes actual POSIX
permissions and verifies that reads create no file; it is not a hardware
write-protected/removable-volume certification. Fault hooks simulate failures
at actual filesystem boundaries, not literal power loss.

## Validation

Final command results are recorded below after execution. The named
`library_durability_*` group includes real temporary-directory protocol tests,
resource/state/file/directory/rename faults, an actual rename-to-directory
failure, compaction faults, and original-operation/SQLite reconstruction tests.

The remaining whole-change gates (full workspace Clippy/tests, complete UI and
IPC integration, governance, scenario reconciliation, native three-platform
matrix and actual legacy libraries) are not certified by this checkpoint.

Actual successful commands on this development checkout:

| Command | Result |
| --- | --- |
| `cargo test -p echo-core library_durability --lib` | 29 passed, 0 failed; 22.92 s; child-process lock test also ran 1 isolated child test |
| `cargo test -p echo-core infrastructure::filesystem::portable_migration --lib` | 10 passed, 0 failed |
| `cargo test -p echo-core infrastructure::filesystem::control_plane --lib` | 11 passed, 0 failed |
| `cargo check -p echo-core --tests` | Passed after port compatibility updates |

The last three production crate checks passed before a scale failure exposed the
legacy migration file at 1002 lines. The migration tests were then split into
`portable_migration/tests.rs` along the existing module boundary; the allowlist
was not changed. The final production/format/scale commands are recorded in the
handoff after the remaining read-only candidate regression check.

Task mapping is deliberately partial: the protocol models and validation cover
part of 2.1; focused port consolidation covers the measurable interface portion
of 2.2; this document supplies protocol/layout/version semantics for 2.4 and
platform assumptions for 3.6. Native lock/read-only behavior and publication /
replay fault matrices cover the filesystem portions of 3.1–3.3. Safe active-chain
compaction is implemented and tested, while complete private-history compaction
and resource reclamation under 3.5 remain open. No blanket claim is made for the
whole of these tasks, cancellation across all application lifecycles, v3 rescue,
or Windows/Linux native validation.

Final verification after the compatibility repair:

| Command | Result |
| --- | --- |
| `cargo test -p echo-core root_switch::tests::prepare_degrades_to_read_only_on_a_newer_manifest --lib` | 1 passed; existing projected song preserved, new file not scanned, explicit Cancelled summary |
| `cargo check -p echo-core` | Passed without warnings |
| `cargo check -p echo-desktop` | Passed without warnings |
| `cargo check -p echo-app` | Passed without warnings |
| `cargo fmt --all --check` | Passed |
| `node scripts/verify/check-scale.mjs` | Passed; main/current ControlPlanePort 15/15, Inventory 1, Durability 6; allowlist unchanged |
| `git diff --check` | Passed |

The additional read-only preparation branch prevents an unsupported future
manifest from reaching the write-capable scan's `EnsureControlPlane` call. It
returns an explicitly cancelled/not-scanned summary and retains the existing
projection, instead of minting new UUIDs or misreporting a completed scan.
Both the portable-migration and root-switch test modules were split into nearby
`tests.rs` files to keep source files under the existing size limit; no scale
exception was added. The root worker separately owns the full Core suite and
remaining application/projection regressions.
