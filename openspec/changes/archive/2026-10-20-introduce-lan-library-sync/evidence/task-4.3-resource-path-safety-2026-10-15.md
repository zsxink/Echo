# 4.3 resource publication and path safety

The existing publication journal requires the exact previous durable digest,
retains old immutable resource versions, and associates the new complete set only
after checking the previous local file and all new lengths/digests. New integration
tests now verify that externally edited old bytes remain untouched and the new
version stays isolated without a completed association. Foreign destination bytes
and failed full-file validation preserve the journal for retry.

Production fixes:

- Completed publication retries recheck the current file length and digest, so
  externally changed or removed media cannot return a successful result.
- Staging ownership verifies the root and all ancestors, including the `echo/`
  parent and operation slot. A matching marker beneath a symlink cannot grant
  write capability. Existing handles are revalidated before reading, publishing,
  or cleaning up after a slot is replaced with a link.
- Missing destination spellings on case-sensitive filesystems reject existing
  case-folded or canonically equivalent Unicode names. Case-insensitive
  filesystems continue to reject these through their existing exclusive-create
  behavior. Incoming resource sets also reject normalized duplicate destinations.
- The shared manifest path boundary rejects nonportable Windows reserved names,
  invalid characters, trailing dots/spaces, absolute/drive paths, and traversal.

Verification on the local macOS filesystem:

- `cargo test -p echo-core --lib sync_resource_publication --locked`: 9 passed.
- `cargo test -p echo-core --lib infrastructure::filesystem --locked`: 60 passed.
- `cargo test -p echo-core --test arch --locked`: 7 passed.
- `cargo clippy -p echo-core --all-targets --all-features --locked -- -D warnings`: passed.
- `git diff --check`: passed.

The publication tests exercise actual read-only staging permissions and successful
retry after permission restoration. Disk-full and permission-loss destination
write failures are injected into the same bounded staging-copy writer used by the
real filesystem adapter; no disk volume was filled. Windows installation and
native filesystem behavior still require the Windows build/device acceptance
covered separately by tasks 8.3 and 8.4.
