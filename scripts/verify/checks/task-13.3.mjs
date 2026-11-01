#!/usr/bin/env node
// Task 13.3 check: fault injection across every journal/fs/DB-commit boundary,
// killed ×2 (recovery run + idempotent re-run), plus the watcher-preempted,
// external-staging-cleanup and volume-disconnect injections, and the new
// write-capability acquisition path (13.2 P0 regression).
//
// The crash machinery lives under `crates/echo-core/src/application/recover/`:
// a `Controller`/`CrashFs`/`CrashUow`/`CrashJournal` injects a scripted panic at
// every journal-state write, every fs call and every DB-commit boundary, then
// recovery is run TWICE and the unique terminal state, reserved UUID, released
// claims, no-orphan/no-overwrite/no-duplicate invariants are asserted.
//
// This check executes the crash-at-every-boundary matrix (which already
// exhausts each journal state + fs + commit point twice), the three extra
// injections the task names, and the P0 write-capability acquisition tests,
// then writes the fault-injection report it is required to save.
//
// `cargo test <filter>` exits 0 when the filter matches nothing, so a moved or
// renamed test would otherwise let this check write a report claiming
// injections that never ran. Every filter must match at least one executed
// test, and the report lists the tests cargo actually ran.

import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const ARTIFACTS = resolve(ROOT, "artifacts");

function fail(msg) {
  process.stderr.write(`FAIL 13.3: ${msg}\n`);
  process.exit(1);
}

// Names of the tests cargo actually executed, for the evidence report.
const executed = [];

function runTests(filter) {
  const r = spawnSync("cargo", ["test", "-p", "echo-core", "--all-features", filter], {
    cwd: ROOT,
    encoding: "utf8",
  });
  const out = `${r.stdout || ""}${r.stderr || ""}`;
  if (r.status !== 0) {
    process.stderr.write(`FAIL 13.3: 'cargo test ${filter}' exited ${r.status}\n${out}\n`);
    process.exit(1);
  }
  const passed = [...out.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
    (total, match) => total + Number(match[1]),
    0,
  );
  if (passed === 0) {
    process.stderr.write(`FAIL 13.3: filter '${filter}' matched no tests\n${out}\n`);
    process.exit(1);
  }
  for (const match of out.matchAll(/^test (\S+) \.\.\. ok$/gm)) executed.push(match[1]);
}

// 1. Crash at every journal state + fs + DB-commit boundary, recovered TWICE.
runTests(
  "application::recover::tests::imports::crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice",
);

// 2. The named extra injections (watcher preemption, external staging cleanup,
//    volume disconnect) — all automated fault tests in echo-core.
for (const filter of [
  "application::recover::tests::imports::recovery_never_creates_a_duplicate_when_a_watcher_preempts",
  "application::trash::tests::external_staging_cleanup_becomes_unknown_and_preserves_relationships",
  "application::trash::tests::disconnected_volume_becomes_unknown_and_disables_root_writes",
]) {
  runTests(filter);
}

// 3. The three-location/hash/claim recovery matrix (contradictory evidence,
//    truncated source, nothing-recoverable rollback).
for (const filter of [
  "application::recover::tests::delete_restore::contradictory_stage_evidence_holds_and_deletes_nothing",
  "application::recover::tests::imports::truncated_source_is_rejected_and_leaves_nothing",
  "application::recover::tests::imports::nothing_recoverable_rolls_back_and_releases_cleanly",
]) {
  runTests(filter);
}

// 4. The P0 write-capability acquisition regression (13.2): a fresh root must
//    become writable by establishing its owned staging dir, so the first
//    import/delete is never permanently LibraryUnavailable.
runTests(
  "infrastructure::filesystem::adapter::tests::fresh_root_acquires_write_capability_by_establishing_staging",
);

// 5. Evidence: the required fault-injection report. The executed-test list is
//    taken from cargo's own output, so the report cannot claim an injection
//    that did not run.
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
  "`Controller`/`CrashFs`/`CrashUow`/`CrashJournal` in `crates/echo-core/src/application/recover/`",
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
  ...executed.map((name) => `- \`${name}\``),
  "",
  "## Result",
  "",
  "All fault injections green on this host. The Terminal recovery ×2, no-orphan,",
  "no-overwrite, no-duplicate, TrashApplied-only and unknown-keeps-relationships",
  "invariants hold.",
];
writeFileSync(report, lines.join("\n") + "\n");

process.stdout.write(
  `ok 13.3: crash-at-every-boundary ×2 + watcher/staging/volume injections green (${executed.length} tests); report at ${report}\n`,
);
