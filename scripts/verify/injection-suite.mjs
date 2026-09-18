#!/usr/bin/env node
// Task 7.6 — executable failure proofs for the agent-facing gates.
//
// `check-verification-validity.mjs` proves *statically* that every registered
// check has a failure route. That is not the same as proving the route is
// wired to the condition the check is supposed to guard: a gate can carry
// `process.exit(1)` and still only ever reach it when its own input is
// malformed, never when the repository actually violates the rule.
//
// This suite closes that gap by violating the condition for real, running the
// gate, and observing it fail. Three mechanisms, cheapest and least invasive
// first:
//
//   fixture  — run the gate against a throwaway root the gate already accepts
//              (`ECHO_SCALE_ROOT`, `ECHO_LINT_CHECK_ROOT`). No repository file
//              is touched at all.
//   argument — drive the gate with an argument that names a violation (a
//              binary path, a scenario id).
//   mutate   — rewrite exactly one repository file, run the gate, then restore
//              the original bytes. Every target is snapshotted in memory *and*
//              on disk before the write, and the restore is verified by hash.
//
// Entries also assert the gate passes on the untouched tree ("positive
// control"), so a gate that is simply always red cannot pass this suite. The
// few gates that cannot be green on some legitimate trees (a dev-mode binary,
// a scenario whose whole point is that no attestation exists yet) declare
// `baseline: false` with the reason, and carry an explicit passing case in
// exchange.
//
// Safety: every mutated path must resolve inside the repository, is snapshotted
// before the write, and is restored in a `finally`. The suite additionally
// asserts `git status --porcelain` is byte-identical before and after, so a
// failed injection cannot leave the working tree polluted.
//
// Usage:
//   node scripts/verify/injection-suite.mjs [--list] [--only <substring>]
//
// Exits 0 only when every entry failed exactly as claimed, every positive
// control passed, and the working tree was restored byte-for-byte.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const BACKUP_DIR = mkdtempSync(join(tmpdir(), "echo-injection-backup-"));
const WORK_DIR = mkdtempSync(join(tmpdir(), "echo-injection-work-"));
const TIMEOUT_MS = 10 * 60 * 1000;

const results = [];
const failures = [];

// ---------------------------------------------------------------------------
// Reversible tree mutation
// ---------------------------------------------------------------------------

/** absPath -> { bytes, hash }; one entry per file, whatever the mutation. */
const snapshots = new Map();
/** absPath created by this run; deleted on restore. */
const createdFiles = new Set();
/** absPath -> `git status --porcelain -- <path>` as found, for the final check. */
const touched = new Map();
let restored = true;

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function guardPath(target) {
  const abs = resolve(target);
  if (abs !== ROOT && !abs.startsWith(`${ROOT}/`)) {
    throw new Error(`refusing to mutate outside the repository: ${abs}`);
  }
  return abs;
}

/** The tracked status of one path, so restoration can be asserted per file.
 *  A whole-tree `git status` comparison would trip over unrelated edits made
 *  while the suite runs (this suite is a tool, not the only writer). */
function recordedStatus(abs) {
  if (touched.has(abs)) return touched.get(abs);
  const status = spawnSync("git", ["status", "--porcelain", "--", abs], {
    cwd: ROOT,
    encoding: "utf8",
  });
  const value = (status.stdout || "").trim();
  touched.set(abs, value);
  return value;
}

/** Snapshot a file that already exists so its bytes can be put back exactly. */
function snapshot(target) {
  const abs = guardPath(target);
  recordedStatus(abs);
  if (snapshots.has(abs)) return abs;
  if (!existsSync(abs)) throw new Error(`mutation target does not exist: ${abs}`);
  const bytes = readFileSync(abs);
  snapshots.set(abs, { bytes, hash: digest(bytes) });
  // A crash must not be the only copy of the original.
  writeFileSync(join(BACKUP_DIR, digest(Buffer.from(abs)).slice(0, 16)), bytes);
  return abs;
}

function writeTarget(target, content) {
  const abs = snapshot(target);
  writeFileSync(abs, content);
  return abs;
}

/** Replace one occurrence, failing loudly if the anchor moved. */
function replaceIn(target, needle, replacement) {
  const abs = snapshot(target);
  const source = readFileSync(abs, "utf8");
  if (!source.includes(needle)) {
    throw new Error(`injection anchor not found in ${abs}: ${JSON.stringify(needle)}`);
  }
  writeFileSync(abs, source.replace(needle, replacement));
  return abs;
}

/** Replace every occurrence. Needed when the gate tests a substring the
 *  replacement would otherwise still contain (`X` -> `X-DISABLED` reads as a
 *  violation to a human and as no-op to `String.prototype.includes`). */
function replaceAllIn(target, needle, replacement) {
  const abs = snapshot(target);
  const source = readFileSync(abs, "utf8");
  if (!source.includes(needle)) {
    throw new Error(`injection anchor not found in ${abs}: ${JSON.stringify(needle)}`);
  }
  if (replacement.includes(needle)) {
    throw new Error(`replacement still contains the anchor it must remove: ${needle}`);
  }
  writeFileSync(abs, source.split(needle).join(replacement));
  return abs;
}

/** Replace inside one CSS rule only. A whole-file `replaceIn` silently hits
 *  the first match anywhere, which for a shared declaration value is usually a
 *  *different* rule — the injection then "passes" while violating nothing. */
function replaceWithinBlock(target, blockHeader, needle, replacement) {
  const abs = snapshot(target);
  const source = readFileSync(abs, "utf8");
  const start = source.indexOf(blockHeader);
  if (start === -1) throw new Error(`injection block not found in ${abs}: ${blockHeader}`);
  const end = source.indexOf("}", start);
  const block = source.slice(start, end);
  if (!block.includes(needle)) {
    throw new Error(`injection needle ${JSON.stringify(needle)} not inside ${blockHeader}`);
  }
  writeFileSync(
    abs,
    source.slice(0, start) + block.replace(needle, replacement) + source.slice(end),
  );
  return abs;
}

function createProbe(target, content) {
  const abs = guardPath(target);
  if (existsSync(abs)) throw new Error(`probe path already exists: ${abs}`);
  // Record the *pre-creation* status: an untracked file becomes `?? ...` the
  // moment it is written, so recording afterwards would report the (correct)
  // cleanup as drift.
  recordedStatus(abs);
  mkdirSync(resolve(abs, ".."), { recursive: true });
  writeFileSync(abs, content);
  createdFiles.add(abs);
  return abs;
}

/** Put every touched file back; verify byte-equality. A tracked file that does
 *  not return to its original bytes is a hard failure. A probe we created is
 *  not: it is untracked scratch, and the start-of-run sweep reclaims it, so a
 *  refused removal is reported and nothing more. */
function restoreAll() {
  for (const [abs, { bytes, hash }] of snapshots) {
    try {
      writeFileSync(abs, bytes);
      if (digest(readFileSync(abs)) !== hash) {
        restored = false;
        process.stderr.write(`FAIL restore: ${abs} did not return to its original bytes\n`);
      }
    } catch (error) {
      restored = false;
      process.stderr.write(`FAIL restore: ${abs}: ${error.message}\n`);
    }
  }
  for (const abs of createdFiles) {
    try {
      rmSync(abs, { force: true });
    } catch (error) {
      process.stderr.write(`warn injection-suite: probe left behind (${abs}: ${error.message})\n`);
    }
  }
  snapshots.clear();
  createdFiles.clear();
}

function resetTree() {
  restoreAll();
}

/** Probe paths this suite owns. Swept before the run so an interrupted earlier
 *  run cannot make every probe entry fail with "already exists" — the failure
 *  would look like a broken gate rather than a broken harness. */
function sweepStaleProbes() {
  const swept = [];
  const blocked = [];
  for (const dir of [join(ROOT, "scripts", "verify", "checks"), ATT_DIR]) {
    if (!existsSync(dir)) continue;
    for (const name of readdirSync(dir)) {
      if (name.startsWith("__probe_") || name.startsWith("__injection_probe")) {
        const path = join(dir, name);
        try {
          rmSync(path, { force: true });
          swept.push(path);
        } catch (error) {
          blocked.push(`${path}: ${error.message}`);
        }
      }
    }
  }
  return { swept, blocked };
}

/** Removing our own scratch dirs must never decide the verdict: they live under
 *  the OS temp dir and their removal can be refused by a sandbox. */
function cleanupScratch() {
  const blocked = [];
  for (const dir of [BACKUP_DIR, WORK_DIR]) {
    try {
      rmSync(dir, { recursive: true, force: true });
    } catch (error) {
      blocked.push(`${dir}: ${error.message}`);
    }
  }
  return blocked;
}

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => {
    restoreAll();
    process.stderr.write(`\ninjection-suite: ${signal} — working tree restored\n`);
    process.exit(130);
  });
}
process.on("uncaughtException", (error) => {
  restoreAll();
  process.stderr.write(`injection-suite: ${error.stack}\n`);
  process.exit(1);
});

// ---------------------------------------------------------------------------
// Running a gate
// ---------------------------------------------------------------------------

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    cwd: options.cwd || ROOT,
    encoding: "utf8",
    env: { ...process.env, ...(options.env || {}) },
    timeout: options.timeout || TIMEOUT_MS,
  });
}

function runGate(entry, extra = {}) {
  const [command, args] = entry.check;
  return run(command, [...args, ...(extra.args || [])], { env: extra.env });
}

function outputOf(result) {
  return `${result.stdout || ""}${result.stderr || ""}`;
}

// Fixtures -----------------------------------------------------------------

function fixture(name) {
  const dir = join(WORK_DIR, name);
  mkdirSync(dir, { recursive: true });
  return dir;
}

function executable(dir, name, body) {
  const path = join(dir, name);
  writeFileSync(path, body);
  chmodSync(path, 0o755);
  return path;
}

/** A `cargo`/`pnpm` that always fails, for proving a delegating check's
 *  failure route is actually wired to its child's exit status. */
function failingBinary(name) {
  const dir = fixture(`failing-${name}`);
  executable(dir, name, "#!/bin/sh\necho \"$0: injected failure\" >&2\nexit 1\n");
  return { PATH: `${dir}:${process.env.PATH || ""}` };
}

const ATT_DIR = join(ROOT, "artifacts", "native-attestations");
const ATT_FIELDS = ["os", "versions", "desktop-env", "operator", "result", "evidence-path", "date"];

// ---------------------------------------------------------------------------
// The registry
//
// Each entry: { id, guard, check, expect, baseline, inject }
//   guard    what the gate claims to protect (the condition being violated)
//   check    [command, args] that runs the gate
//   expect   regex the gate's output must match when it fails
//   baseline whether the gate must be green on the untouched tree
//   inject   () => { env?, args? } — performs the violation
// ---------------------------------------------------------------------------

const ENTRIES = [
  // ---- 14.1 lint inheritance -------------------------------------------
  {
    id: "lint-inheritance/no-workspace-inherit",
    guard: "a workspace crate whose [lints] does not inherit the workspace config",
    check: ["node", ["scripts/verify/check-lint-inheritance.mjs"]],
    expect: /FAIL lint inheritance: .*does not inherit workspace lints/,
    baseline: true,
    inject() {
      const dir = fixture("lint-no-inherit");
      writeFileSync(
        join(dir, "Cargo.toml"),
        '[workspace]\nmembers = ["crates/bad"]\nresolver = "2"\n',
      );
      mkdirSync(join(dir, "crates", "bad"), { recursive: true });
      writeFileSync(
        join(dir, "crates", "bad", "Cargo.toml"),
        '[package]\nname = "bad"\nversion = "0.0.0"\n\n[lints]\n# inherits nothing\n',
      );
      return { env: { ECHO_LINT_CHECK_ROOT: dir } };
    },
  },
  {
    id: "lint-inheritance/crate-level-override",
    guard: "a crate that inherits the workspace lints *and* adds its own override line",
    check: ["node", ["scripts/verify/check-lint-inheritance.mjs"]],
    expect: /FAIL lint inheritance: crates\/bad declares crate-level lint overrides: unsafe_code/,
    baseline: true,
    inject() {
      const dir = fixture("lint-override");
      writeFileSync(
        join(dir, "Cargo.toml"),
        '[workspace]\nmembers = ["crates/bad"]\nresolver = "2"\n',
      );
      mkdirSync(join(dir, "crates", "bad"), { recursive: true });
      writeFileSync(
        join(dir, "crates", "bad", "Cargo.toml"),
        '[package]\nname = "bad"\nversion = "0.0.0"\n\n[lints]\nworkspace = true\nunsafe_code = "allow"\n',
      );
      return { env: { ECHO_LINT_CHECK_ROOT: dir } };
    },
  },
  {
    id: "lint-inheritance/scoped-override-section",
    guard: "a crate that adds a whole `[lints.<tool>]` override section",
    check: ["node", ["scripts/verify/check-lint-inheritance.mjs"]],
    expect: /FAIL lint inheritance: crates\/bad declares crate-level lint override sections: clippy/,
    baseline: true,
    inject() {
      const dir = fixture("lint-scoped-override");
      writeFileSync(
        join(dir, "Cargo.toml"),
        '[workspace]\nmembers = ["crates/bad"]\nresolver = "2"\n',
      );
      mkdirSync(join(dir, "crates", "bad"), { recursive: true });
      writeFileSync(
        join(dir, "crates", "bad", "Cargo.toml"),
        '[package]\nname = "bad"\nversion = "0.0.0"\n\n[lints]\nworkspace = true\n\n[lints.clippy]\nall = "deny"\n',
      );
      return { env: { ECHO_LINT_CHECK_ROOT: dir } };
    },
  },

  // ---- 14.2 scale ratchet ----------------------------------------------
  {
    id: "scale/oversized-source-file",
    guard: "a source file above the 1000-line hard limit",
    check: ["node", ["scripts/verify/check-scale.mjs"]],
    expect: /FAIL scale gate:[\s\S]*exceeds 1000/,
    baseline: true,
    inject() {
      const dir = fixture("scale-oversize");
      const target = join(dir, "crates", "echo-core", "src");
      mkdirSync(target, { recursive: true });
      writeFileSync(join(target, "huge.rs"), `${"// filler\n".repeat(1101)}`);
      return { env: { ECHO_SCALE_ROOT: dir } };
    },
  },
  {
    id: "scale/public-trait-width",
    guard: "a public trait with more than six methods",
    check: ["node", ["scripts/verify/check-scale.mjs"]],
    expect: /FAIL scale gate:[\s\S]*methods exceeds 6/,
    baseline: true,
    inject() {
      const dir = fixture("scale-trait");
      const target = join(dir, "crates", "echo-core", "src");
      mkdirSync(target, { recursive: true });
      const methods = Array.from({ length: 7 }, (_, i) => `    fn m${i}();`).join("\n");
      writeFileSync(join(target, "wide.rs"), `pub trait Wide {\n${methods}\n}\n`);
      return { env: { ECHO_SCALE_ROOT: dir } };
    },
  },
  {
    id: "scale/allowlist-expansion",
    guard: "growing the scale allowlist instead of shrinking it",
    check: ["node", ["scripts/verify/check-scale.mjs"]],
    expect: /allowlist was expanded or changed/,
    baseline: true,
    inject() {
      const path = join(ROOT, "scripts", "verify", "scale-allowlist.json");
      const parsed = JSON.parse(readFileSync(path, "utf8"));
      parsed.files.push("crates/echo-core/src/application/probe.rs");
      writeTarget(path, `${JSON.stringify(parsed, null, 2)}\n`);
      return {};
    },
  },

  // ---- 14.3 toolchain pin ----------------------------------------------
  {
    id: "toolchain/channel-mismatch",
    guard: "rust-toolchain.toml pinning a different channel than the active rustc",
    check: ["node", ["scripts/verify/check-toolchain.mjs"]],
    expect: /FAIL toolchain: rust-toolchain\.toml=99\.0\.0/,
    baseline: true,
    inject() {
      replaceIn(
        join(ROOT, "rust-toolchain.toml"),
        'channel = "1.96.0"',
        'channel = "99.0.0"',
      );
      return {};
    },
  },

  // ---- 14.4 scenario manifest fields -----------------------------------
  {
    id: "scenario-manifests/dropped-required-field",
    guard: "a scenario YAML missing a field the registry agrees on",
    check: ["node", ["scripts/verify/validate-scenario-manifests.mjs"]],
    expect: /FAIL scenario manifest validation:[\s\S]*missing non-empty requirement/,
    baseline: true,
    inject() {
      replaceIn(join(ROOT, "tests", "scenarios", "DAS-R01-S01.yaml"), "\nrequirement:", "\n# requirement:");
      return {};
    },
  },

  // ---- 14.5 verification validity --------------------------------------
  {
    id: "verification-validity/no-op-check",
    guard: "a registered check that cannot fail",
    check: ["node", ["scripts/verify/check-verification-validity.mjs"]],
    expect: /FAIL verification validity:[\s\S]*__injection_probe_noop\.mjs: no observable non-zero failure route/,
    baseline: true,
    inject() {
      createProbe(
        join(ROOT, "scripts", "verify", "checks", "__injection_probe_noop.mjs"),
        '#!/usr/bin/env node\nconsole.log("this check never fails");\n',
      );
      const path = join(ROOT, "scripts", "verify", "manifest.json");
      const manifest = JSON.parse(readFileSync(path, "utf8"));
      manifest.tasks.push({
        id: "probe",
        title: "injected no-op check",
        commands: [
          { desc: "injected by scripts/verify/injection-suite.mjs", cmd: "node scripts/verify/checks/__injection_probe_noop.mjs" },
        ],
      });
      writeTarget(path, `${JSON.stringify(manifest, null, 2)}\n`);
      return {};
    },
  },

  // ---- 14.6 build purity ------------------------------------------------
  {
    id: "build-purity/ungated-test-only-module",
    guard: "a test-only module declared in the default build (no cfg gate)",
    check: ["node", ["scripts/verify/check-build-purity.mjs"]],
    expect: /FAIL build purity:[\s\S]*declares the test-only module without a cfg gate/,
    baseline: true,
    inject() {
      replaceIn(
        join(ROOT, "crates", "echo-desktop", "src", "player", "mod.rs"),
        "#[cfg(test)]\npub mod fake;",
        "pub mod fake;",
      );
      return {};
    },
  },

  // ---- 14.7 scenario churn ratchet -------------------------------------
  {
    id: "scenario-churn/tightened-ceiling",
    guard: "the scenarios-per-command ratio rising above the frozen ceiling",
    check: ["node", ["scripts/verify/check-scenario-churn.mjs"]],
    expect: /FAIL scenario churn[\s\S]*exceeds frozen ceiling/,
    baseline: true,
    inject() {
      replaceIn(
        join(ROOT, "scripts", "verify", "check-scenario-churn.mjs"),
        "maxDuplication: 1.8,",
        "maxDuplication: 1.0,",
      );
      return {};
    },
  },

  // ---- embedded-frontend freshness -------------------------------------
  {
    id: "embedded-frontend/stale-binary",
    guard: "a binary that embeds no frontend asset from the current dist",
    check: ["node", ["scripts/verify/checks/check-embedded-frontend.mjs"]],
    expect: /FAIL embedded-frontend: [\s\S]*embeds no frontend asset at all/,
    // The default binary on a developer machine is often a `tauri dev` build,
    // which is reported as a SKIP by design — so there is no reliable green
    // baseline. The negative case below is the discriminator.
    baseline: false,
    // The gate compares a binary against the *current* dist, so the proof needs
    // a dist to compare against.
    requires: () =>
      existsSync(join(ROOT, "apps", "desktop", "dist", "index.html"))
        ? null
        : "apps/desktop/dist/index.html is absent — run `pnpm --dir apps/desktop build` first",
    inject() {
      const dir = fixture("embedded-frontend");
      const binary = join(dir, "garbage-binary");
      writeFileSync(binary, "not an executable and definitely not a bundled frontend\n");
      return { args: ["--bin", binary] };
    },
  },

  // ---- native attestation (the "out-of-repo artifact" gate) ------------
  {
    id: "native-attestation/missing-record",
    guard: "a human-scenario row whose attestation record does not exist",
    check: ["node", ["scripts/verify/checks/check-native-attestation.mjs", "__probe_no_record__"]],
    expect: /no attestation record at artifacts\/native-attestations/,
    baseline: false, // the whole point of the gate is that no record exists yet
    inject() {
      return {};
    },
  },
  {
    id: "native-attestation/empty-record",
    guard: "an attestation record that exists but records nothing",
    check: ["node", ["scripts/verify/checks/check-native-attestation.mjs", "__probe_empty__"]],
    expect: /record is empty/,
    baseline: false,
    inject() {
      createProbe(join(ATT_DIR, "__probe_empty__.log"), "\n   \n");
      return {};
    },
  },
  {
    id: "native-attestation/incomplete-record",
    guard: "an attestation record missing one of the seven required fields",
    check: ["node", ["scripts/verify/checks/check-native-attestation.mjs", "__probe_incomplete__"]],
    expect: /missing required field 'operator'/,
    baseline: false,
    inject() {
      const body = `${ATT_FIELDS.filter((f) => f !== "operator")
        .map((f) => `${f}: probe`)
        .join("\n")}\nresult: pass\n`;
      createProbe(join(ATT_DIR, "__probe_incomplete__.log"), body);
      return {};
    },
  },
  {
    id: "native-attestation/complete-record (positive control)",
    guard: "the gate accepts a complete, passing record instead of always failing",
    check: ["node", ["scripts/verify/checks/check-native-attestation.mjs", "__probe_complete__"]],
    expect: /ok: native attestation __probe_complete__ recorded and complete/,
    baseline: false,
    expectPass: true,
    inject() {
      const body = `${ATT_FIELDS.map((f) => `${f}: probe`).join("\n")}\nresult: pass\n`;
      createProbe(join(ATT_DIR, "__probe_complete__.log"), body);
      return {};
    },
  },

  // ---- delegating checks: the child's exit status must propagate --------
  {
    id: "delegation/cargo-exit-status",
    guard: "a check that shells out to cargo ignoring a failing child",
    check: ["node", ["scripts/verify/checks/task-2.1.mjs"]],
    expect: /FAIL 2\.1: cargo test/,
    baseline: true,
    inject() {
      return { env: failingBinary("cargo") };
    },
  },
  {
    id: "delegation/pnpm-exit-status",
    guard: "a check that shells out to pnpm ignoring a failing child",
    check: ["node", ["scripts/verify/checks/task-1.2.mjs"]],
    expect: /FAIL 1\.2: 'typecheck' exited 1/,
    baseline: true,
    inject() {
      return { env: failingBinary("pnpm") };
    },
  },
  {
    id: "delegation/runner-propagates-check-failure",
    guard: "the task runner reporting success when a registered check fails",
    check: ["node", ["scripts/verify/run-task.mjs", "2.1"]],
    expect: /error: task 2\.1: command exited 1/,
    baseline: true,
    inject() {
      return { env: failingBinary("cargo") };
    },
  },

  // ---- wire-desktop-system-dialogs (a pure-node assertion cluster) ------
  {
    id: "wire-dialogs/missing-plugin",
    guard: "the shell declaring neither OS dialog plugin",
    check: ["node", ["scripts/verify/checks/task-wire-dialogs.mjs"]],
    expect: /FAIL wire-desktop-system-dialogs: tauri-plugin-dialog missing/,
    baseline: true,
    inject() {
      replaceAllIn(
        join(ROOT, "apps", "desktop", "src-tauri", "Cargo.toml"),
        "tauri-plugin-dialog",
        "tauri-plugin-dial0g",
      );
      return {};
    },
  },
  {
    id: "wire-dialogs/privileged-capability",
    guard: "the webview getting a dialog/fs/shell privilege it must not hold",
    check: ["node", ["scripts/verify/checks/task-wire-dialogs.mjs"]],
    expect: /FAIL wire-desktop-system-dialogs: capability grants privileged permission\(s\): dialog:default/,
    baseline: true,
    inject() {
      const path = join(ROOT, "apps", "desktop", "src-tauri", "capabilities", "main.json");
      const capability = JSON.parse(readFileSync(path, "utf8"));
      capability.permissions = [...(capability.permissions || []), "dialog:default"];
      writeTarget(path, `${JSON.stringify(capability, null, 2)}\n`);
      return {};
    },
  },
  {
    id: "wire-dialogs/status-view-grid-area",
    guard: "the initialize/status view losing its workspace grid area",
    check: ["node", ["scripts/verify/checks/task-wire-dialogs.mjs"]],
    expect: /FAIL wire-desktop-system-dialogs: \.workspace-empty does not declare grid-area: workspace/,
    baseline: true,
    inject() {
      replaceWithinBlock(
        join(ROOT, "apps", "desktop", "src", "styles", "app-extras.css"),
        ".workspace-empty {",
        "grid-area: workspace",
        "grid-area: none",
      );
      return {};
    },
  },
];

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

const { swept, blocked: blockedSweep } = sweepStaleProbes();
for (const path of swept) {
  process.stdout.write(`note injection-suite: removed stale probe ${path.replace(`${ROOT}/`, "")}\n`);
}
for (const blocked of blockedSweep) {
  process.stderr.write(`warn injection-suite: could not remove a stale probe (${blocked})\n`);
}

const argv = process.argv.slice(2);
if (argv.includes("--list")) {
  for (const entry of ENTRIES) process.stdout.write(`${entry.id}\n`);
  process.exit(0);
}
const onlyIndex = argv.indexOf("--only");
const only = onlyIndex === -1 ? null : argv[onlyIndex + 1];
const selected = only ? ENTRIES.filter((entry) => entry.id.includes(only)) : ENTRIES;

/** CI sets this so an environment-dependent skip cannot quietly stand in for a
 *  proof — the same contract `check-build-purity.mjs` uses for its artifact
 *  half. Locally, an unevaluable entry is named instead of hidden. */
const requireAll = process.env.ECHO_INJECTION_REQUIRE_ALL === "1";

const startedAt = Date.now();

for (const entry of selected) {
  const record = { id: entry.id, state: "pass", detail: "" };
  try {
    // An entry whose violation cannot even be expressed on this machine is
    // reported by name, never counted as a proof.
    const unmet = entry.requires ? entry.requires() : null;
    if (unmet) {
      record.state = requireAll ? "fail" : "skip";
      record.detail = unmet;
      results.push(record);
      if (record.state === "fail") failures.push(record);
      continue;
    }

    // Positive control: the gate must be green before anything is violated.
    if (entry.baseline !== false) {
      const before = runGate(entry);
      if (before.status !== 0) {
        record.state = "fail";
        record.detail = `positive control failed: gate exits ${before.status} on the untouched tree\n${outputOf(before)}`;
        results.push(record);
        failures.push(record);
        continue;
      }
    }

    const extra = entry.inject() || {};
    const after = runGate(entry, extra);

    if (entry.expectPass) {
      if (after.status !== 0 || !entry.expect.test(outputOf(after))) {
        record.state = "fail";
        record.detail = `expected the gate to accept a valid case, got exit ${after.status}\n${outputOf(after)}`;
      }
    } else if (after.status === 0) {
      record.state = "fail";
      record.detail = `gate stayed green under violation (${entry.guard})\n${outputOf(after)}`;
    } else if (!entry.expect.test(outputOf(after))) {
      record.state = "fail";
      record.detail = `gate failed for the wrong reason; expected ${entry.expect}\n${outputOf(after)}`;
    }
  } catch (error) {
    record.state = "fail";
    record.detail = error.message;
  } finally {
    resetTree();
  }
  if (record.state === "fail") failures.push(record);
  results.push(record);
}

const driftedPaths = [...touched.keys()].filter(
  (abs) => recordedStatus(abs) !== currentStatus(abs),
);

function currentStatus(abs) {
  const status = spawnSync("git", ["status", "--porcelain", "--", abs], {
    cwd: ROOT,
    encoding: "utf8",
  });
  return (status.stdout || "").trim();
}

const skipped = results.filter((record) => record.state === "skip");
const proven = results.filter((record) => record.state === "pass");

// ---------------------------------------------------------------------------
// Completeness: this suite is the register of what has been *proven*, so it
// also owns the gap. Two classes of check exist, and each needs a different
// kind of evidence:
//
//   self-contained — the assertion lives inside the check. Only an injected
//                    violation can show it is wired. Every one of these must
//                    appear in ENTRIES above.
//   delegating     — the assertion lives in the child command it runs. An
//                    injected failing *test* would prove the child, not the
//                    check; what can silently pass is ignoring the child's
//                    exit status, so the evidence is the structural rule in
//                    check-verification-validity.mjs (every spawning check must
//                    compare `.status`), demonstrated concretely by the
//                    delegation/* entries for the cargo, pnpm and runner
//                    families.
//
// A new self-contained check therefore cannot land without its proof, and the
// delegating remainder is printed rather than hidden.
// ---------------------------------------------------------------------------

function registeredChecks() {
  const manifest = JSON.parse(
    readFileSync(resolve(ROOT, "scripts", "verify", "manifest.json"), "utf8"),
  );
  const found = new Set();
  for (const task of manifest.tasks || []) {
    for (const command of task.commands || []) {
      const match = (command.cmd || "").match(
        /(?:^|\s)node\s+(scripts\/verify\/(?:checks\/)?[A-Za-z0-9_.-]+\.mjs)/,
      );
      if (match) found.add(match[1]);
    }
  }
  return [...found].sort();
}

const provenPaths = new Set(
  ENTRIES.flatMap((entry) => entry.check[1]).filter((arg) => arg.endsWith(".mjs")),
);

const unproven = [];
const delegating = [];
for (const path of registeredChecks()) {
  if (provenPaths.has(path)) continue;
  const source = readFileSync(resolve(ROOT, path), "utf8");
  if (/spawnSync\(|execFileSync\(/.test(source)) delegating.push(path);
  else unproven.push(path);
}

if (delegating.length) {
  process.stdout.write(
    `note injection-suite: ${delegating.length} delegating check(s) are covered structurally ` +
      `(a nonzero child status must fail the check) and demonstrated for the cargo, pnpm and runner families\n`,
  );
}

for (const record of results) {
  if (record.state === "pass") {
    process.stdout.write(`ok   ${record.id}\n`);
  } else if (record.state === "skip") {
    process.stderr.write(`skip ${record.id}: ${record.detail}\n`);
  } else {
    process.stderr.write(`FAIL ${record.id}: ${record.detail}\n`);
  }
}

const treeIntact = restored && driftedPaths.length === 0;

for (const blocked of cleanupScratch()) {
  process.stderr.write(`warn injection-suite: scratch dir not removed (${blocked})\n`);
}

process.stdout.write(
  `injection-suite: ${proven.length}/${results.length} gates proven to fail under violation` +
    (skipped.length ? `, ${skipped.length} not evaluable here` : "") +
    ` (${((Date.now() - startedAt) / 1000).toFixed(1)}s)\n`,
);

if (!treeIntact) {
  process.stderr.write(
    `FAIL injection-suite: the working tree was not restored byte-for-byte.\n` +
      `  restore verified: ${restored}\n` +
      `  paths whose git status changed: ${driftedPaths.join(", ") || "(none)"}\n` +
      `  originals kept in ${BACKUP_DIR}\n`,
  );
  process.exit(1);
}
if (unproven.length) {
  process.stderr.write(
    `FAIL injection-suite: ${unproven.length} self-contained check(s) have no injected-failure proof:\n` +
      `${unproven.map((p) => `- ${p}`).join("\n")}\n` +
      `a check that asserts inside itself must come with an entry in ENTRIES\n`,
  );
  process.exit(1);
}
if (failures.length) {
  process.stderr.write(`injection-suite: ${failures.length} gate proof(s) failed\n`);
  process.exit(1);
}
process.stdout.write("injection-suite: every registered gate failed exactly as claimed\n");
