# Native pagination transport diagnosis (2026-10-24)

The original 513-commit fixture remains unchanged. This evidence does not mark
the pagination, 10k performance, or whole-change acceptance complete.

## Failure and bounded reproduction

The previous complete attempt failed after 3090.61 seconds at
`merge_cycle.rs:209` with `SyncConnectionFailed`; see
`/tmp/echo-library-first-20261024-native-513.log`. Its public error discarded
the underlying transport classification, so that run alone cannot prove which
request or transport stage failed.

A temporary, subsequently removed diagnostic test used the real native fixture:
successfully pull, block the current-thread runtime for 16 seconds, then pull
again. It failed after 16.40 seconds with exactly `SyncConnectionFailed` and
safe reqwest classification `timeout=false connect=false request=true
body=false`. Log: `/tmp/echo-native-runtime-idle-probe.log`. Blocking for six
seconds before the first request passed (6.37 seconds), so initial listener/TLS
startup was not established as the cause.

The first reproduction crosses the host's existing 15-second idle header-read
bound while preventing both the server and reqwest connection maintenance from
being polled. The next request races reuse of the connection with its expiry.
The native pull loop previously performed directory recovery, validation and
fsync through synchronous `apply_commit_page` on this same executor. This is a
concrete independently reproduced failure path; the complete 513 rerun below
must still establish whether it resolves the original failure.

## Implemented boundary and regression

`pull_host_changes` awaits one owned `spawn_blocking` page-apply job before
requesting the next page. The job preserves the authenticated host check,
directory-first commit, cursor ordering and shared catalog-change signal.
The executor can continue processing HTTP/TLS maintenance while the writer
works. No request timeout or production durability requirement changed.

Transport mapping now records only four boolean reqwest classifications in
structured tracing, with the same boolean-only stderr observation in test
builds. Neither reqwest Display/Debug nor the URL/token/source chain is logged;
the existing public `SyncConnectionFailed` remains unchanged.

The permanent regression wraps the real directory adapter with a one-shot
250 ms manifest read delay. On the default current-thread Tokio runtime it
requires a 25 ms asynchronous heartbeat to run before directory completion,
then checks the real song projection and two host page reads. Temporarily
restoring the old synchronous apply made this test fail in 0.66 seconds with
`directory IO blocked the current-thread transport executor`; restoring the
fix made it pass in 0.63 seconds. This avoids a permanent 16-second sleep or
changing the 513 fixture.

Actual checks on the fixed tree:

- `cargo test -p echo-app --bin Echo sync_provisioner::error_tests -- --nocapture`:
  3 passed, including an actual loopback HTTP timeout classification (0.11 s).
- `cargo test -p echo-app --bin Echo -- --skip native_pull_drains_more_than_one_commit_page_without_losing_local_work`:
  108 passed, 1 pre-existing ignored, 1 explicitly filtered (13.44 s).
- `cargo test -p echo-app --bin Echo native_pull_keeps_transport_responsive_during_slow_directory_apply -- --nocapture`:
  final regression rerun passed (0.62 s), after explicitly releasing the test
  adapter's start-notification mutex before its artificial delay.
- `cargo clippy -p echo-app --all-targets --all-features -- -D warnings`: passed
  (3.56 s); `/tmp/echo-native-513-clippy.log`.
- `cargo fmt --all --check`, scale gate, and `git diff --check`: passed.

The parent independently reran the permanent slow-directory regression: 1 passed,
0 failed, 0.65 s (`/tmp/echo-library-first-20261024-parent-blocking-apply.log`).
Independent review found no actionable regression in the transport classification
or the owned page-apply job. Review also traced production shutdown: the dedicated
current-thread runtime waits for started blocking jobs on drop, the provisioner
holds its gate across the sync cycle, and root handoff stops/joins the sync worker
and waits for its cycle lock before activation. The new regression itself does
not directly exercise cancellation, panic, or failure during root handoff; this
is a limitation, not evidence that the whole lifecycle matrix passed.

## Complete unchanged rerun and remaining CPU cost

The fixed-tree command was started unchanged, then stopped at the user's request
to wrap up the current subtask. It did not reach its pagination assertions and
has not passed:

```sh
cargo test -p echo-app --bin Echo native_pull_drains_more_than_one_commit_page_without_losing_local_work -- --nocapture
```

Log: `/tmp/echo-library-first-20261024-native-513-fixed.log`. The parent confirmed
the exact cargo/test process tree and sent SIGTERM at about 14 minutes (test
process 14m07s), while host preparation still consumed one CPU and no final test
result existed. Both processes exited. This is an interrupted run, not a passing
or failing assertion result; the complete unchanged command remains required.
A preceding diagnostic rerun was stopped to rebuild the fix; it does not count
as a completed test.
The inherited old governance process tree (82413/85537/86797) was also stopped
with SIGTERM and confirmed exited; it was not a passing latest-tree validation.

The host preparation CPU cost is separate. Native sampling identifies writer
`stage_confirmation`/`mark_published` → `commit_projection` → full library and
sync-state after-image export → filesystem `chain` → JSON decode. A0 skips
intrinsic checks for exact certified bytes, but deliberately still reads/parses
each current document and validates live ownership/chain relationships. The
current full exports repeat historical state throughout active commits, and
the coverage snapshot grows with retained state. This change adds no replay or
parsed-payload cache and does not treat mtime/length as trust.

Task 4.2's explicit FrozenWriteSet and strict pure projector remain necessary
to remove the repeated full-export cost safely. Selecting only the mutation's
object would omit aliases, terminal events, dependent rows and cursors and is
not a valid optimization. No 10k or cross-platform performance claim is made.
The incremental `synchronize_once` and snapshot paths retain their existing
synchronous port surfaces and need their own scoped blocking/lifecycle work;
this fix only changes the explicit host-history draining loop.
