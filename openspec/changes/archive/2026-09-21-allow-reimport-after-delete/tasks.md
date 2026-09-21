## 1. Import conflict semantics

- [x] 1.1 Update import planning and post-publish duplicate checks to include content hashes and logical target reservations only for `Available` songs, while retaining filesystem occupancy checks.
- [x] 1.2 Add a regression test for re-importing content whose prior song is `PendingDelete`, asserting a new song is committed and the old record remains unavailable.

## 2. Verification and delivery

- [x] 2.1 Run focused Core import tests covering duplicate detection and the new regression.
- [x] 2.2 Run formatting, lint/type checks applicable to the changed Rust workspace and validate the OpenSpec change with `openspec validate <change-name> --strict`.
