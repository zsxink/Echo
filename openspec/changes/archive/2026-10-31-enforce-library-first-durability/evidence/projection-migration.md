# SQLite projection migration (2026-10-24)

Task 4.1 adds migration 0028 after the released migrations through 0027.
`git diff main --name-only -- crates/echo-core/src/infrastructure/sqlite/migrations`
lists only `0028_library_durability.sql`; published scripts remain unchanged.

The new tables retain the local-library projection watermark, applied operation
identity/sequence/digest and typed recovery rows, including unresolved reasons.
Root foreign keys and typed row-kind constraints prevent cross-root or unknown
row kinds from silently entering these tables. Actual library-ID mismatches are
rejected by replay validation before any SQL mutation, as covered in the final
durability group.

The new `migration_0028_preserves_legacy_evidence_and_rolls_back_partial_upgrade`
test constructs the complete actual v27 schema from the unchanged scripts. It
injects invalid SQL after v28 DDL, checks that every new table and version entry
is rolled back, then applies the real migration. Reapplying performs no writes;
all earlier checksums and the original root timestamps remain unchanged. Valid
typed evidence survives database reopen; an unregistered root and an invalid
row kind are rejected.

Main-agent validation: `cargo test -p echo-core migration --lib` **20 passed**,
0 failed, 1.20 s. The existing bad-library/foreign-watermark replay tests also
passed in the 60-test durability group. Core Clippy passed after the test was
added. Independent review found no actionable issue in this migration scope and
confirmed task 4.1 can be checked.

This does not certify the full legacy rescue/identity mapping, v3 directory
upgrade, new active-state projection or replacement of a damaged active SQLite
database. Those tasks remain open.
