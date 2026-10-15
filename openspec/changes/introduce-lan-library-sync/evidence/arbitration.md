# Core arbitration evidence

`crates/echo-core/src/domain/sync.rs::confirm_candidate` is a pure Core domain
decision. The caller supplies the object's confirmed base and the next host
sequence; the function does not inspect wall clocks, HTTP, SQLite, or files.

- It rejects an object-key mismatch and non-monotone per-object host sequence.
- Each origin has a cursor. The next candidate must be exactly the next origin
  sequence and name the prior mutation ID. A missing/out-of-order predecessor
  does not mutate state.
- A candidate with a stale base from another device is accepted in host commit
  order and returns the replaced value plus accepted value as a conflict record.
- Consecutive edits from the same origin advance the predecessor chain and do
  not conflict with that device's own earlier confirmation.
- `CommitSeq` comparison is scoped by `(library_id, authority_id, epoch)`;
  wall-clock timestamps are not inputs to ordering.

Validation: `cargo test -p echo-core` covers identity scope, object kind,
overflow, LWW replacement/conflict retention, same-device chains and predecessor
rejection. The SQLite atomic inbox/log/publication work remains in tasks 3.x.
