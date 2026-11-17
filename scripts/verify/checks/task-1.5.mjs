#!/usr/bin/env node
// Task 1.5 check: the three-platform Rust CI matrix declares fmt/clippy/test and
// the single-leg frontend matrix declares the format/lint/typecheck/test/build
// quality commands.
//
// Fails (nonzero) unless `.github/workflows/ci.yml` structurally contains the
// three Rust platform/runner pairs, the Linux frontend leg, platform-based
// names, the `rust` job steps (fmt, clippy `-D warnings`, test) and the
// `frontend` job steps (`pnpm install --frozen-lockfile`,
// format/lint/typecheck/test/build).
//
// The commands themselves run in the Rust/frontend CI matrices and, locally, in
// the governance gate's `quality-core` segment. This check deliberately does not
// re-run them: a task check proves its own behavior, not the shared static gates.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { assertPlatformMatrix, assertPlatformName, assertShardMatrix, matrixEntries, parseWorkflow } from "../workflow-matrix.mjs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 1.5: ${msg}\n`);
  process.exit(1);
}

function step(workflow, jobId, name) {
  const steps = workflow?.jobs?.[jobId]?.steps;
  if (!Array.isArray(steps)) fail(`ci.yml job '${jobId}' has no steps`);
  const found = steps.find((s) => s.name === name);
  if (!found) fail(`ci.yml job '${jobId}' is missing step '${name}'`);
  return found;
}
function assertRunIncludes(s, needle, label) {
  if (!String(s.run || "").includes(needle)) fail(`${label} run must include '${needle}'`);
}
function assertIfIncludes(s, needle, label) {
  if (!String(s.if || "").includes(needle)) fail(`${label} if must include '${needle}'`);
}
function assertEnv(s, key, value, label) {
  if (String(s.env?.[key]) !== value) fail(`${label} env ${key} must be '${value}'`);
}
function assertAlwaysRuns(workflow, jobId) {
  const job = workflow?.jobs?.[jobId];
  if (!job) fail(`ci.yml job '${jobId}' is missing`);
  if (job.if !== undefined) fail(`ci.yml job '${jobId}' must not be conditionally skipped`);
  if (job["continue-on-error"] === true) fail(`ci.yml job '${jobId}' must not set continue-on-error`);
}
function sectionsOf(run) {
  const match = String(run || "").match(/--sections=([a-z,-]+)/);
  return match ? match[1].split(",") : [];
}

// The executable task inventory is partitioned across three shards; 16.1 runs
// in the Rust Linux job because it needs the built Tauri shell. Keeping the
// expected lists here (rather than deriving them from ci.yml) is what makes a
// deleted or duplicated task fail instead of silently shrinking coverage.
const EXPECTED_SHARDS = {
  core: "1.1 1.3 1.8 3.1 4.1 5.1 6.1 11.5 12.5 13.3".split(" "),
  desktop: "7.1 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 10.6 11.1 11.2 11.4 16.2".split(" "),
  frontend: "10.1 10.2 10.3 10.4 10.5 11.3 11.6 13.1 10.7 10.8 10.9 10.10 12.1 12.2 12.3".split(" "),
};

// ---------------------------------------------------------------------------
// Structural check: parse the workflow and verify each platform-to-runner pair,
// display-name expression, and exact quality commands CI runs.
// ---------------------------------------------------------------------------
const ciPath = resolve(ROOT, ".github", "workflows", "ci.yml");
let ci;
try {
  ci = readFileSync(ciPath, "utf8");
} catch {
  fail(`cannot read ${ciPath}`);
}

let workflow;
try {
  workflow = parseWorkflow(ci, "ci.yml");
  const RUST_MATRIX = {
    "Linux/workspace":   { os: "ubuntu-latest",  cache: "linux-workspace",  packages: "--workspace",    clippy: true },
    "macOS/core":        { os: "xcode-27",       cache: "macos-core",       packages: "-p echo-core",   clippy: true },
    "Windows/core":      { os: "windows-latest", cache: "windows-core",     packages: "-p echo-core",   clippy: true },
  };
  assertShardMatrix(matrixEntries(workflow, "rust", "ci.yml"), RUST_MATRIX, "ci.yml rust");
  assertPlatformName(workflow.jobs.rust.name, "Rust (${{ matrix.platform }} ${{ matrix.shard }})", "ci.yml rust name");

  const RUST_DESKTOP_MATRIX = {
    "macOS/desktop":     { os: "xcode-27",       cache: "macos-desktop",    packages: "-p echo-sync -p echo-desktop -p echo-app", clippy: false },
    "Windows/desktop":   { os: "windows-latest", cache: "windows-desktop",  packages: "-p echo-sync -p echo-desktop -p echo-app", clippy: false },
  };
  assertShardMatrix(matrixEntries(workflow, "rust-desktop", "ci.yml"), RUST_DESKTOP_MATRIX, "ci.yml rust-desktop");
  assertPlatformName(workflow.jobs["rust-desktop"].name, "Rust desktop (${{ matrix.platform }})", "ci.yml rust-desktop name");
  // The frontend checks are jsdom + Node toolchain work and no test file reads
  // process.platform, so a single Linux leg covers them; macOS/Windows frontend
  // *build* coverage stays with the Tauri-context builds in rust/rust-desktop.
  // The platform map is pinned one-entry-exactly: re-adding a macOS or Windows
  // leg must fail here rather than silently re-spend the quota.
  assertPlatformMatrix(
    matrixEntries(workflow, "frontend", "ci.yml"),
    { Linux: { os: "ubuntu-latest" } },
    "ci.yml frontend",
  );
  assertPlatformName(workflow.jobs.frontend.name, "Frontend (${{ matrix.platform }})", "ci.yml frontend name");
} catch (error) {
  fail(error.message);
}

// ---------------------------------------------------------------------------
// Cache governance: every Rust build cache step must name its shared key, and
// the writer/reader split must be exactly the pinned sets. Wrapped in a block
// because Task 2's audit-trigger assertions below already declare `auditWorkflow`
// in module scope.
// ---------------------------------------------------------------------------
{
  const CACHE_STEP = "Cache Rust build artifacts";
  const MAIN_ONLY = "${{ github.ref == 'refs/heads/main' }}";
  const CACHE_WRITERS = ["governance-static", "linux-workspace", "macos-core", "macos-desktop", "windows-core", "windows-desktop"];
  // rust/rust-desktop 的 shared-key 是模板表达式，必须各自展开成该 job 实际写入
  // 的 key，否则写者集合永远等于 ["${{ matrix.cache }}", "governance-static"]。
  // 按 job 展开而不是合并两个矩阵：两个 job 都写入时合并会对同一批 key 重复计数。
  const writerKeys = [];
  const readerKeys = [];
  for (const [jobId, job] of Object.entries(workflow.jobs)) {
    const cacheStep = (job.steps || []).find((s) => s.name === CACHE_STEP);
    if (!cacheStep) continue;
    const sharedKey = String(cacheStep.with?.["shared-key"] || "");
    const saveIf = String(cacheStep.with?.["save-if"] ?? "");
    if (!sharedKey) fail(`ci.yml job '${jobId}' ${CACHE_STEP} must set shared-key`);
    if (saveIf !== MAIN_ONLY && saveIf !== "false") {
      fail(`ci.yml job '${jobId}' ${CACHE_STEP} save-if must be 'false' or '${MAIN_ONLY}', got '${saveIf}'`);
    }
    if (saveIf === MAIN_ONLY) {
      writerKeys.push(...(sharedKey === "${{ matrix.cache }}" ? matrixEntries(workflow, jobId, "ci.yml").map((entry) => entry.cache) : [sharedKey]));
    } else {
      readerKeys.push(sharedKey);
    }
  }
  if (JSON.stringify(writerKeys.sort()) !== JSON.stringify(CACHE_WRITERS)) {
    fail(`ci.yml cache writers must be exactly [${CACHE_WRITERS.join(", ")}], got [${writerKeys.join(", ")}]`);
  }
  for (const key of readerKeys) {
    if (key !== "${{ matrix.cache }}" && !CACHE_WRITERS.includes(key)) {
      fail(`ci.yml reader cache key '${key}' has no writer`);
    }
  }
  // governance-static's key is job-private: the spec exempts it from the cargo
  // shape invariant *because* nobody else builds into it. Pin that readership so
  // the exemption cannot silently become false.
  const governanceReaders = Object.entries(workflow.jobs)
    .filter(([, job]) => (job.steps || []).some((s) => s.name === CACHE_STEP && String(s.with?.["shared-key"] || "") === "governance-static"))
    .map(([jobId]) => jobId);
  if (JSON.stringify(governanceReaders) !== JSON.stringify(["governance-static"])) {
    fail(`ci.yml cache key 'governance-static' must be read only by job 'governance-static', got [${governanceReaders.join(", ")}]`);
  }
  // ...and the scheduled audit workflow is its only other reader, so it must
  // restore the same key rather than compiling its own cold tree every night.
  const auditWorkflow = parseWorkflow(
    readFileSync(resolve(ROOT, ".github", "workflows", "dependency-audit.yml"), "utf8"),
    "dependency-audit.yml",
  );
  const auditKeys = Object.values(auditWorkflow.jobs).flatMap((job) =>
    (job.steps || []).filter((s) => s.name === CACHE_STEP).map((s) => String(s.with?.["shared-key"] || "")),
  );
  if (auditKeys.length !== 1 || auditKeys[0] !== "governance-static") {
    fail(`dependency-audit.yml ${CACHE_STEP} must use shared-key 'governance-static', got [${auditKeys.join(", ")}]`);
  }
  // The scheduled workflow is the online half's one guaranteed execution point on
  // a quiet day. Deleting the step that runs 1.7 would otherwise be silent: the
  // triggers stay right, the cache key stays right, and advisories simply stop
  // being scanned.
  const auditRunSteps = Object.values(auditWorkflow.jobs).flatMap((job) =>
    (job.steps || []).map((s) => String(s.run || "")),
  );
  if (!auditRunSteps.some((run) => run.includes("verify:task -- 1.7"))) {
    fail("dependency-audit.yml must actually run 'pnpm verify:task -- 1.7'");
  }
}

// A job-level `if` is evaluated before the matrix expands, so `matrix` is not an
// available context there: referencing it makes the whole workflow invalid and
// every job disappears. Match both `matrix.x` and the subscript spelling
// `matrix['x']` (GitHub rejects both), and require a non-identifier character
// before `matrix` so a hypothetical job id or word containing it does not trip.
const MATRIX_CONTEXT = /(^|[^A-Za-z0-9_-])matrix\s*[.[]/;
for (const [jobId, job] of Object.entries(workflow.jobs)) {
  if (MATRIX_CONTEXT.test(String(job.if || ""))) {
    fail(`ci.yml job '${jobId}' if must not reference the matrix context (job-level if is evaluated before the matrix expands)`);
  }
}

const REQUIRED = [
  "cargo fmt --all -- --check",
  "cargo clippy --workspace --all-targets --all-features -- -D warnings",
  "pnpm install --frozen-lockfile",
  "pnpm --filter @echo/desktop format:check",
  "pnpm --filter @echo/desktop lint",
  "pnpm --filter @echo/desktop typecheck",
  "pnpm --filter @echo/desktop test -- --run",
  "pnpm --filter @echo/desktop build",
];

for (const needle of REQUIRED) {
  if (!ci.includes(needle)) {
    fail(`ci.yml is missing "${needle}"`);
  }
}

// ---------------------------------------------------------------------------
// Step-level structure: the exact commands and conditions that carry the PR/main
// tiering and the workspace-shard static gates. A rename or move must fail here.
// ---------------------------------------------------------------------------
const format = step(workflow, "rust", "Format check");
if (format.run !== "cargo fmt --all -- --check") fail("rust Format check run must be 'cargo fmt --all -- --check'");

const test = step(workflow, "rust", "Test");
const clippy = step(workflow, "rust", "Clippy");
const rustStatic = step(workflow, "rust", "Rust static gates");
const TEST_IF = "github.event_name != 'pull_request' || matrix.shard == 'workspace'";
if (String(test.if || "").trim() !== TEST_IF) fail(`rust Test if must be '${TEST_IF}'`);
if (test["continue-on-error"] === true) fail("rust Test must not set continue-on-error");
if (test.run !== "cargo test ${{ matrix.packages }} --all-targets --all-features") {
  fail("rust Test run must be 'cargo test ${{ matrix.packages }} --all-targets --all-features'");
}
if (String(clippy.if || "").trim() !== "matrix.clippy") fail("rust Clippy if must be 'matrix.clippy'");
if (clippy.run !== "cargo clippy --workspace --all-targets --all-features -- -D warnings") {
  fail("rust Clippy run must be 'cargo clippy --workspace --all-targets --all-features -- -D warnings'");
}
for (const stepName of ["Task 16.1", "Task 9.7 9.8", "Rust static gates"]) {
  const s = step(workflow, "rust", stepName);
  if (String(s.if || "").trim() !== "matrix.shard == 'workspace'") {
    fail(`rust ${stepName} if must be "matrix.shard == 'workspace'"`);
  }
}
// The step-level `if` above cannot tell which command runs. Pin both commands
// so relocating a task gate cannot silently change what it executes.
if (step(workflow, "rust", "Task 16.1").run !== "pnpm verify:task -- 16.1") {
  fail("Task 16.1 run must be 'pnpm verify:task -- 16.1'");
}
if (step(workflow, "rust", "Task 9.7 9.8").run !== "pnpm verify:task -- 9.7 9.8") {
  fail("Task 9.7 9.8 run must be 'pnpm verify:task -- 9.7 9.8'");
}
if (rustStatic.run !== "pnpm verify:governance -- --sections=rust-static") {
  fail("Rust static gates run must be 'pnpm verify:governance -- --sections=rust-static'");
}
if (rustStatic.env?.ECHO_PURITY_REQUIRE_BUILD !== undefined) {
  fail("Rust static gates must not set ECHO_PURITY_REQUIRE_BUILD (purity moved to governance-static)");
}

// rust-desktop: the shards that carry the rest of the workspace tests. They are
// a separate job because a job-level `if` cannot see matrix.*, and this job must
// be absent on pull requests while running in full on main.
const DESKTOP_JOB_IF = "github.event_name != 'pull_request'";
if (String(workflow.jobs["rust-desktop"].if || "").trim() !== DESKTOP_JOB_IF) {
  fail(`rust-desktop job if must be '${DESKTOP_JOB_IF}'`);
}
const desktopTest = step(workflow, "rust-desktop", "Test");
// `if: false` parses as the YAML boolean, which `|| ""` would fold into the
// same empty string as "unset". Test for presence, not emptiness.
if (desktopTest.if !== undefined) fail("rust-desktop Test must not be conditionally skipped");
if (desktopTest["continue-on-error"] === true) fail("rust-desktop Test must not set continue-on-error");
if (desktopTest.run !== "cargo test ${{ matrix.packages }} --all-targets --all-features") {
  fail("rust-desktop Test run must be 'cargo test ${{ matrix.packages }} --all-targets --all-features'");
}
// These gates only make sense on the Linux workspace shard. The step inventory
// above is not pinned (harmless additions stay green), but these four must not
// drift in: Clippy already runs workspace-wide in the core shards, and the
// task/static gates would duplicate the workspace shard's work.
for (const forbidden of ["Clippy", "Task 16.1", "Task 9.7 9.8", "Rust static gates"]) {
  if (workflow.jobs["rust-desktop"].steps.some((s) => s.name === forbidden)) {
    fail(`rust-desktop must not carry step '${forbidden}' (it only exists for the workspace shard)`);
  }
}

const RUST_ORDER = ["Format check", "Task 16.1", "Task 9.7 9.8", "Clippy", "Test", "Rust static gates"];
const rustSteps = workflow.jobs.rust.steps.map((s) => s.name).filter(Boolean);
const positions = RUST_ORDER.map((name) => rustSteps.indexOf(name));
if (!positions.every((v, i) => v >= 0 && (i === 0 || v > positions[i - 1]))) {
  fail(`rust step order must be ${RUST_ORDER.join(" < ")}`);
}
const DESKTOP_ORDER = ["Test"];
const desktopSteps = workflow.jobs["rust-desktop"].steps.map((s) => s.name).filter(Boolean);
const desktopPositions = DESKTOP_ORDER.map((name) => desktopSteps.indexOf(name));
if (!desktopPositions.every((v, i) => v >= 0 && (i === 0 || v > desktopPositions[i - 1]))) {
  fail(`rust-desktop step order must be ${DESKTOP_ORDER.join(" < ")}`);
}

// The cache-shape invariant the spec states ("MUST NOT execute a cargo call of a
// different feature / target selection / check shape before the cache is
// saved") is only real if something scans for it. Pinning the Test step's exact
// `run` proves the shape of *that* step; it says nothing about a cargo call
// added after it, which is precisely how a later "just one more check" edit
// would silently poison the cache the readers restore.
//
// After the pinned Test step, the only admissible cargo invocations are: the
// same shape again (a re-run), or a `-- --list` of the same shape (which cargo
// treats as the same build). Anything else — a different package set, a
// `--lib`/`--test` target selection, a `check`/`clippy` subcommand — changes
// what lands in target/, so the readers' restored cache no longer matches what
// they compile.
function assertNoShapeDrift(jobId, testStepName, expectedRun) {
  const steps = workflow.jobs[jobId].steps;
  const testIndex = steps.findIndex((s) => s.name === testStepName);
  if (testIndex < 0) fail(`ci.yml job '${jobId}' is missing step '${testStepName}'`);
  const allowed = [expectedRun, `${expectedRun} -- --list`];
  for (const s of steps.slice(testIndex + 1)) {
    const run = String(s.run || "");
    if (!/\bcargo\b/.test(run)) continue;
    const normalized = run.trim();
    if (allowed.includes(normalized)) continue;
    fail(
      `ci.yml job '${jobId}' step '${s.name}' runs '${normalized}' after the cache-shape Test step; ` +
        `only '${expectedRun}' or its '-- --list' may follow, or the saved cache will not match its readers`,
    );
  }
}
assertNoShapeDrift("rust", "Test", "cargo test ${{ matrix.packages }} --all-targets --all-features");
assertNoShapeDrift("rust-desktop", "Test", "cargo test ${{ matrix.packages }} --all-targets --all-features");

const governanceStatic = step(workflow, "governance-static", "Run governance static gates");
// Exact equality: a run that merely *mentions* the flags (`echo "--sections=..."`)
// would keep the section strings present while executing nothing.
if (governanceStatic.run !== "pnpm verify:governance -- --sections=registry,static,coverage") {
  fail("governance-static run must be 'pnpm verify:governance -- --sections=registry,static,coverage'");
}
const purityStep = step(workflow, "governance-static", "Run build purity gate");
if (purityStep.run !== "pnpm verify:governance -- --sections=purity") {
  fail("governance-static purity step run must be 'pnpm verify:governance -- --sections=purity'");
}
assertEnv(purityStep, "ECHO_PURITY_REQUIRE_BUILD", "1", "governance-static purity step");

const auditStep = step(workflow, "governance-static", "Verify dependency audit gate");
if (auditStep.run !== "pnpm verify:task -- 1.7") fail("governance-static audit step run must be 'pnpm verify:task -- 1.7'");
const AUDIT_ENV = "${{ github.event_name == 'pull_request' && '1' || '0' }}";
if (String(auditStep.env?.ECHO_AUDIT_OFFLINE_ONLY) !== AUDIT_ENV) {
  fail(`governance-static audit step env ECHO_AUDIT_OFFLINE_ONLY must be '${AUDIT_ENV}'`);
}
if (workflow.jobs["dependency-audit"]) fail("ci.yml job 'dependency-audit' must not exist (folded into governance-static)");
// A step-level `if` would keep every string above intact while the scan stops
// running — exactly the failure this tiering exists to prevent. Presence, not
// emptiness: `if: false` is the YAML boolean and `|| ""` would hide it.
if (auditStep.if !== undefined) fail("governance-static audit step must not be conditionally skipped");
// The skip must never be silent: the gate itself carries the note naming what
// did not run, and this pins its presence so a later edit cannot drop it.
const auditSource = readFileSync(resolve(ROOT, "scripts", "verify", "checks", "task-1.7.mjs"), "utf8");
for (const needle of ["ECHO_AUDIT_OFFLINE_ONLY", "1.7: note — online advisory checks"]) {
  if (!auditSource.includes(needle)) fail(`task-1.7 must keep '${needle}'`);
}
// The scheduled workflow is the only place the online half runs on a quiet day,
// so it must stay schedule/dispatch-only: a `pull_request` trigger would re-run
// it on every PR and defeat the point of taking it out of ci.yml.
const auditWorkflow = parseWorkflow(
  readFileSync(resolve(ROOT, ".github", "workflows", "dependency-audit.yml"), "utf8"),
  "dependency-audit.yml",
);
const auditTriggers = Object.keys(auditWorkflow.on || {}).sort();
if (JSON.stringify(auditTriggers) !== JSON.stringify(["schedule", "workflow_dispatch"])) {
  fail(`dependency-audit.yml must trigger on exactly schedule + workflow_dispatch, got [${auditTriggers.join(", ")}]`);
}

const release = step(workflow, "release-gate", "Run release gate without external artifacts");
if (release.run !== "node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf") {
  fail("release-gate run must use --skip-static --delegated=governance,perf");
}

// macos-native-gate exists only to run these two task gates (8.12/13.2 need the
// staged dylibs). Asserting merely that the job runs unconditionally would
// leave a permanently-green empty job if its one real step were deleted.
const nativeGate = step(workflow, "macos-native-gate", "Verify libmpv smoke and native e2e gates");
if (nativeGate.run !== "pnpm verify:task -- 8.12 13.2") {
  fail("macos-native-gate run must be 'pnpm verify:task -- 8.12 13.2'");
}
if (String(nativeGate.if || "").trim() !== "") fail("macos-native-gate must not be conditionally skipped");

// The three performance scenarios used to occupy their own job while sharing
// this shard's toolchain and `linux-workspace` cache. They now run as a step in
// task-gates' core shard. Both halves are pinned: the step must exist with the
// exact run, and its `if` must keep it on every event (the core shard is in
// every matrix expansion), so a PR cannot lose the perf gate. The shard's cache
// step must stay read-only so the `--release` artifacts are never saved into a
// key the Rust shards restore.
const perfStep = step(workflow, "task-gates", "Run performance scenarios");
if (perfStep.run !== "pnpm verify:perf") fail("task-gates performance step run must be 'pnpm verify:perf'");
assertIfIncludes(perfStep, "matrix.shard == 'core'", "task-gates performance step");
if (workflow.jobs.perf) fail("ci.yml job 'perf' must not exist (folded into task-gates core)");
const taskGatesCache = step(workflow, "task-gates", "Cache Rust build artifacts");
if (String(taskGatesCache.with?.["save-if"] ?? "") !== "false") {
  fail("task-gates cache step must keep save-if: false so --release perf artifacts are never saved");
}

// ---------------------------------------------------------------------------
// Duplicate-execution reverse locks. Every deletion in this change is pinned
// here, so re-adding the duplicate fails instead of quietly re-spending quota.
// Only platform-independent commands are pinned this way: a command that
// genuinely differs per platform (or per job's downstream steps) is not a
// duplicate, and a blanket scan would flag the frontend builds that each job
// needs for its own Tauri context.
// ---------------------------------------------------------------------------
function jobsRunning(command) {
  return Object.entries(workflow.jobs)
    .filter(([, job]) => (job.steps || []).some((s) => String(s.run || "").trim() === command))
    .map(([jobId]) => jobId);
}

// `cargo fmt --all -- --check` is platform-independent, and the rust job runs it
// on Linux, macOS and Windows already. A copy in rust-desktop re-ran the same
// bytes on the same two platforms (it carried one until this change).
{
  const jobs = jobsRunning("cargo fmt --all -- --check");
  if (JSON.stringify(jobs) !== JSON.stringify(["rust"])) {
    fail(`'cargo fmt --all -- --check' must run in exactly ['rust'] (it covers all three platforms), got [${jobs.join(", ")}]`);
  }
}
// The perf gate has exactly one execution location now.
{
  const jobs = jobsRunning("pnpm verify:perf");
  if (JSON.stringify(jobs) !== JSON.stringify(["task-gates"])) {
    fail(`'pnpm verify:perf' must run in exactly ['task-gates'], got [${jobs.join(", ")}]`);
  }
}
// task-8.12 is the authoritative real-libmpv run: it stages the dylibs and fails
// on the skip branch. A bare player_smoke run elsewhere cannot find the rpath
// deps, silently skips, and yet reads as coverage (task-13.2 did exactly that).
{
  const t132 = readFileSync(resolve(ROOT, "scripts", "verify", "checks", "task-13.2.mjs"), "utf8");
  if (/"--test",\s*"player_smoke"/.test(t132)) {
    fail("task-13.2 must not run player_smoke bare (task-8.12 is the authoritative real-libmpv run)");
  }
  const t812 = readFileSync(resolve(ROOT, "scripts", "verify", "checks", "task-8.12.mjs"), "utf8");
  for (const needle of ["DYLD_LIBRARY_PATH", "skipping real playback smoke"]) {
    if (!t812.includes(needle)) fail(`task-8.12 must keep '${needle}' (it is the only real-libmpv guard)`);
  }
}

const injection = workflow.jobs["injection-proofs"];
if (JSON.stringify(injection?.strategy?.matrix?.shard) !== JSON.stringify([1, 2, 3])) {
  fail("injection-proofs matrix.shard must be [1, 2, 3]");
}
const shardStep = step(workflow, "injection-proofs", "Prove each governance gate");
assertRunIncludes(shardStep, "--shard=${{ matrix.shard }}/3", "injection shard step");
// Shard 1 needs dist for the embedded-frontend comparison; shard 2 needs it
// because task-9.1's `cargo check -p echo-app` fails without it. Shard 3 builds
// nothing that reads dist, so it must skip the build.
const bundleStep = step(workflow, "injection-proofs", "Build the frontend bundle the desktop shell embeds");
assertIfIncludes(bundleStep, "matrix.shard != 3", "injection frontend build step");
const shellStep = step(workflow, "injection-proofs", "Build the shell binary checked by embedded-frontend proof");
assertIfIncludes(shellStep, "matrix.shard == 1", "shell build step");
const embeddedStep = step(workflow, "injection-proofs", "Verify embedded frontend freshness");
assertIfIncludes(embeddedStep, "matrix.shard == 1", "embedded freshness step");
if (embeddedStep.run !== "pnpm verify:frontend-fresh") fail("embedded freshness run must be 'pnpm verify:frontend-fresh'");

// Every job that carries a required check must run on both PR and main. A
// job-level `if` or `continue-on-error` would keep the strings above intact
// while the job itself stops running or stops failing.
for (const jobId of ["rust", "frontend", "governance-static", "injection-proofs", "release-gate", "task-gates", "macos-platform-gate", "macos-native-gate", "windows-platform-gate"]) {
  assertAlwaysRuns(workflow, jobId);
}

// ---------------------------------------------------------------------------
// Task shards: the union must cover all 42 non-Linux tasks exactly once, and
// 16.1 must be in the Rust Linux job instead.
// ---------------------------------------------------------------------------
const matrix = workflow.jobs["task-gates"]?.strategy?.matrix;
if (!matrix || Object.keys(matrix).some((key) => key !== "include")) {
  fail("task-gates must use an include-only matrix");
}
const shards = matrix.include || [];
if (shards.length !== 3) fail(`task-gates must have 3 shards, got ${shards.length}`);
const byShard = new Map(shards.map((entry) => [entry.shard, String(entry.tasks || "").split(/\s+/).filter(Boolean)]));
for (const [name, expected] of Object.entries(EXPECTED_SHARDS)) {
  const actual = byShard.get(name);
  if (!actual) fail(`task-gates is missing shard '${name}'`);
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`task-gates shard '${name}' must be [${expected.join(" ")}], got [${actual.join(" ")}]`);
  }
}
const union = [...byShard.values()].flat();
if (union.length !== 42) fail(`task-gates shard union must be 42 tasks, got ${union.length}`);
if (new Set(union).size !== union.length) fail("task-gates shard union contains duplicates");
if (union.includes("16.1")) fail("16.1 must run in Rust Linux, not task-gates");

// Sections carried by a `--sections=` selection are checked for *inclusion* in
// that step's selection; the remaining sections are carried by a dedicated
// step, so the step existing at all is the assertion.
const SECTION_SELECTOR_STEP = {
  registry: ["governance-static", "Run governance static gates"],
  static: ["governance-static", "Run governance static gates"],
  coverage: ["governance-static", "Run governance static gates"],
  "rust-static": ["rust", "Rust static gates"],
  purity: ["governance-static", "Run build purity gate"],
};
const SECTION_DEDICATED_STEP = {
  quality: ["rust", "Test"],
  embedded: ["injection-proofs", "Verify embedded frontend freshness"],
  injection: ["injection-proofs", "Prove each governance gate"],
};

// The section list is fixed here as well as cross-checked against
// `--list-sections`: if a section were deleted from both the selector and the
// workflow, a purely derived check would happily agree with itself.
const EXPECTED_SECTIONS = [
  "quality",
  "registry",
  "rust-static",
  "static",
  "coverage",
  "purity",
  "embedded",
  "injection",
];

const listed = spawnSync(process.execPath, [resolve(ROOT, "scripts", "verify", "ci-governance.mjs"), "--list-sections"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (listed.status !== 0) fail(`ci-governance.mjs --list-sections exited ${listed.status}`);
const sections = listed.stdout.trim().split(/\s+/).filter(Boolean);
for (const section of EXPECTED_SECTIONS) {
  if (!sections.includes(section)) fail(`ci-governance no longer lists section '${section}'`);
}
for (const section of sections) {
  if (!EXPECTED_SECTIONS.includes(section)) {
    fail(`ci-governance section '${section}' has no execution location in ci.yml`);
  }
  const selector = SECTION_SELECTOR_STEP[section];
  if (selector) {
    const [jobId, stepName] = selector;
    if (!sectionsOf(step(workflow, jobId, stepName).run).includes(section)) {
      fail(`ci-governance section '${section}' has no execution location in ci.yml`);
    }
    continue;
  }
  const dedicated = SECTION_DEDICATED_STEP[section];
  if (!dedicated) fail(`ci-governance section '${section}' has no execution location in ci.yml`);
  step(workflow, dedicated[0], dedicated[1]);
}

process.stdout.write(
  "ok 1.5: three-platform Rust matrix and single-leg frontend matrix declare the quality commands (executed by the CI matrices and quality-core); CI structure (tiering, task shards, sections, duplicate-execution locks) verified\n",
);