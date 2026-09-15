#!/usr/bin/env node
// Task 13.3 check: fault injection across every journal/fs/DB-commit boundary,
// killed ×2 (recovery run + idempotent re-run), plus the watcher-preempted,
// external-staging-cleanup and volume-disconnect injections, and the new
// write-capability acquisition path (13.2 P0 regression).
//
// The crash machinery is `crates/echo-core/src/application/recover.rs`: a
// `Controller`/`CrashFs`/`CrashUow`/`CrashJournal` injects a scripted panic at
// every journal-state write, every fs call and every DB-commit boundary, then
// recovery is run TWICE and the unique terminal state, reserved UUID, released
// claims, no-orphan/no-overwrite/no-duplicate invariants are asserted.
//
// This check executes the crash-at-every-boundary matrix (which already
// exhausts each journal state + fs + commit point twice), the three extra
// injections the task names, and the P0 write-capability acquisition tests,
// then writes the fault-injection report it is required to save.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const ARTIFACTS = resolve(ROOT, "artifacts");

function fail(msg) {
  process.stderr.write(`FAIL 13.3: ${msg}\n`);
  process.exit(1);
}
function run(command, args, cwd = ROOT, what = "") {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 13.3: ${what || command} exited ${r.status}\n${r.stderr || r.stdout}\n`);
    process.exit(1);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// 1. Crash at every journal state + fs + DB-commit boundary, recovered TWICE.
run("cargo", [
  "test", "-p", "echo-core", "--all-features",
  "recover::tests::crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice",
], ROOT, "crash-at-every-boundary ×2");

// 2. The named extra injections (watcher preemption, external staging cleanup,
//    volume disconnect) — all automated fault tests in echo-core.
for (const filter of [
  "recover::tests::recovery_never_creates_a_duplicate_when_a_watcher_preempts",
  "trash::tests::external_staging_cleanup_becomes_unknown_and_preserves_relationships",
  "trash::tests::disconnected_volume_becomes_unknown_and_disables_root_writes",
]) {
  run("cargo", ["test", "-p", "echo-core", "--all-features", filter], ROOT, filter);
}

// 3. The three-location/hash/claim recovery matrix (contradictory evidence,
//    truncated source, nothing-recoverable rollback).
for (const filter of [
  "recover::tests::contradictory_stage_evidence_holds_and_deletes_nothing",
  "recover::tests::truncated_source_is_rejected_and_leaves_nothing",
  "recover::tests::nothing_recoverable_rolls_back_and_releases_cleanly",
]) {
  run("cargo", ["test", "-p", "echo-core", "--all-features", filter], ROOT, filter);
}

// 4. The P0 write-capability acquisition regression (13.2): a fresh root must
//    become writable by establishing its owned staging dir, so the first
//    import/delete is never permanently LibraryUnavailable.
run("cargo", [
  "test", "-p", "echo-core", "--all-features",
  "adapter::tests::fresh_root_acquires_write_capability_by_establishing_staging",
], ROOT, "fresh-root write-capability acquisition");

// 5. Evidence: the required fault-injection report. We record exactly which
//    tests executed (they ARE the fault injections — a scripted crash at each
//    journal state / fs call / commit boundary), and the P0 staging fix.
mkdirSync(ARTIFACTS, { recursive: true });
const report = resolve(ARTIFACTS, "fault-injection-13.3.md");
const lines = [
  "# Echo 0.1.0 fault-injection report (task 13.3)",
  "",
  `- date: ${new Date().toISOString()}`,
  `- host: ${process.platform} ${process.arch}`,
  "",
  "## Injection harness",
  "",
  "`Controller`/`CrashFs`/`CrashUow`/`CrashJournal` in `crates/echo-core/src/application/recover.rs`",
  "inject a scripted failed write at every boundary. Each crash triggers under",
  "`catch_unwind`, then recovery runs TWICE; the invariants asserted are:",
  "",
  "- unique terminal state (Completed/RolledBack/Restored/DatabaseFinalized),",
  "- the reserved UUID is reused (no duplicate UUID),",
  "- no orphan final file and no foreign overwrite,",
  "- `released_claims() == [operation]` after idempotent recovery,",
  "- only durably-`TrashApplied` items auto-finalize (TrashApplied-only forward-roll),",
  "- unknown outcomes preserve relationships (TrashOutcomeUnknown keeps the",
  "  journal and disables writes).",
  "",
  "## Executed fault injections (run green this check)",
  "",
  "1. `crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice`",
  "   — every journal-state write + publish + DB-commit point, recovered twice.",
  "2. `recovery_never_creates_a_duplicate_when_a_watcher_preempts` — watcher",
  "   preemption of an in-flight import leaves exactly one song.",
  "3. `external_staging_cleanup_becomes_unknown_and_preserves_relationships`",
  "   — external cleanup of the staging dir → TrashOutcomeUnknown, no inference.",
  "4. `disconnected_volume_becomes_unknown_and_disables_root_writes` — volume",
  "   disconnect mid-trash → unknown, root writes disabled.",
  "5. `contradictory_stage_evidence_holds_and_deletes_nothing` — conflicting",
  "   state evidence never deletes a user file.",
  "6. `truncated_source_is_rejected_and_leaves_nothing` — short/size-mismatched",
  "   source is rejected with nothing staged.",
  "7. `nothing_recoverable_rolls_back_and_releases_cleanly` — clean rollback.",
  "8. `fresh_root_acquires_write_capability_by_establishing_staging` — the 13.2",
  "   P0 regression: a brand-new root becomes writable on acquire, so the first",
  "   import/delete is not permanently LibraryUnavailable.",
  "",
  "## Result",
  "",
  "All fault injections green on this host. The Terminal recovery ×2, no-orphan,",
  "no-overwrite, no-duplicate, TrashApplied-only and unknown-keeps-relationships",
  "invariants hold.",
];
writeFileSync(report, lines.join("\n") + "\n");

process.stdout.write(`ok 13.3: crash-at-every-boundary ×2 + watcher/staging/volume injections green; report at ${report}\n`);