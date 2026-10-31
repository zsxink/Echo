# Durable publication IO follow-up (2026-10-23)

This is a measured local optimization, not completion of task 3.7 or of the change.

## Safety boundary

- `RootControlPlane::commit` now acquires the same thread and cross-process writer guards, recovers migration, and checks format through `prepare_write(false)`. The transaction then runs the single pre-commit durability recovery. Other legacy write methods still use `prepare(false)` and recover before writing.
- Materialization skips publication only when the previous materialization checkpoint already records the expected digest **and** a new constrained `portable::read` returns the exact expected bytes. Missing files, modified files and invalid paths still reject recovery. Matching disk bytes without checkpoint evidence still cross file-sync, rename and ancestor-directory-sync boundaries again.
- The commit marker, operation receipt bodies, checkpoint ordering, resource validation and compaction/GC sync boundaries are unchanged. `transaction.rs` is an internal extraction to keep implementation size and Clippy limits; no public API changed.

## Controlled native hook measurement

Same debug build, native temporary directories, 128 sequential direct `commit_with_hook` calls. Each batch has one unchanged playlist envelope plus all immutable pending states accumulated so far, adding one new pending state per commit. Timings below are cumulative for the first 32/64/128 commits. Hooks count actual successful file and directory sync boundaries. This measures repeated after-image publication; it does not isolate the separate `RootControlPlane` preparation recovery removal, because the harness calls the transaction directly. These are single local observations, not a statistical or 10k scale acceptance result.

| Commits | Before time | After time | Before file syncs | After file syncs | Before directory syncs | After directory syncs |
| --- | --- | --- | --- | --- | --- | --- |
| 32 | 12.472 s | 2.361 s | 689 | 162 | 2,883 | 744 |
| 64 | 33.814 s | 5.851 s | 2,402 | 323 | 9,862 | 1,483 |
| 128 | 106.569 s | 14.694 s | 8,900 | 645 | 36,108 | 2,961 |

At 128 commits, record file syncs fall from 128 to 1 and state file syncs from 8,256 to 128. Prepared, marker, materialization checkpoint and receipt file syncs remain 128 each, coverage file syncs remain 4, and all their directory sync and GC counts remain unchanged. Skipped record/state publications account for the entire IO reduction.

Logs: `/tmp/echo-durability-io-before.log`, `/tmp/echo-durability-io-after.log`. The temporary harness was removed from the production test tree after measurement; no slow test was ignored or reduced. To reproduce, temporarily append this function to `durability/tests/publication.rs`, run `ECHO_MEASURE_DURABILITY_IO=128 cargo test -p echo-core library_durability_measure_cumulative_publication_io --lib -- --nocapture`, then remove it:

```rust
#[test]
fn library_durability_measure_cumulative_publication_io() {
    let Some(value) = std::env::var_os("ECHO_MEASURE_DURABILITY_IO") else { return; };
    let maximum: usize = value.to_str().unwrap().parse().unwrap();
    let (_dir, registry, root, library, _) = fixture();
    let original = batch(library, uuid::Uuid::new_v4(), "stable record");
    let mut states = Vec::new();
    let mut counts = BTreeMap::<String, usize>::new();
    let started = std::time::Instant::now();
    for index in 1..=maximum {
        states.push(DurableStateChange::new(StateKind::Pending, uuid::Uuid::new_v4(), serde_json::json!({"index": index})));
        let mut change = original.clone();
        change.operation_id = uuid::Uuid::new_v4();
        change.state = states.clone();
        commit_with_hook(&registry, root, &change, &mut |phase, _| {
            *counts.entry(phase.to_owned()).or_default() += 1;
            Ok(())
        }).unwrap();
        if [32, 64, 128].contains(&index) {
            eprintln!("durability_measure commits={index} elapsed_ms={} hooks={counts:?}", started.elapsed().as_millis());
        }
    }
}

```

## Regression validation

- `cargo test -p echo-core library_durability_ --lib`: **41 passed**, 0 failed, 0 ignored (original 37 plus four regression functions), 25.90 s. `/tmp/echo-durability-regression.log`.
- New regressions verify unchanged checkpointed record/state after-images avoid publication, both missing and modified checkpointed files reject even with an identical committed successor, record/state rename failures before checkpoint publication redo every sync boundary, and unsupported future formats reject before writable durability recovery.
- `cargo test -p echo-core application::scan --lib`: **28 passed**, 0 failed, 0 ignored. `/tmp/echo-durability-scan-regression.log`.
- `cargo test -p echo-app --bin Echo host_resources`: **2 passed**, 0 failed, 0 ignored. `/tmp/echo-durability-host-resources-regression.log`.
- `cargo clippy -p echo-core --all-targets -- -D warnings`: passed. `/tmp/echo-durability-clippy.log`.
- `cargo fmt --all` and `git diff --check`: passed.

## Original 513-commit native paging test

Command: `cargo test -p echo-app --bin Echo native_pull_drains_more_than_one_commit_page_without_losing_local_work -- --nocapture`.

The unchanged test was attempted; it is not counted as passed. At 1 min 12 s the host database contained 58 songs; at 1 min 39 s it contained 68 songs / 68 sync commits. At approximately 2 minutes the host materialization sequence reached 146 with 439 tracked paths; operation receipts already occupied about 14.15 MB. The run was stopped with SIGTERM at 3 min 20 s with 101 host songs / 101 sync commits, still preparing the host commits before the multi-page pull assertions. This bounded attempt did not reach 513 or execute the paging assertions; no fixture count or ignore annotation was changed.

A 3-second native `sample` showed CPU around 97% with the active writer dominated by `commit_projection` → `commit_then_project` → directory `commit_with_hook` → `recover_with_hook` / `chain` → `LibraryCommit::validate` / `calculate_digest` and serialization of complete `DurableStateChange` payloads. Full SQL after-images repeatedly enlarge the active suffix and original operation receipts; this remains a CPU/serialization bottleneck after removing redundant publications. The sample is evidence of sampled stacks, not a complete allocation or complexity proof. Reworking SQL export/frozen pending completeness was explicitly outside this focused optimization.

Log: `/tmp/echo-durability-native-513.log`; native stack sample: `/tmp/echo-durability-native-513.sample.txt`.
