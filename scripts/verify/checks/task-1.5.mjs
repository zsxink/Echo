#!/usr/bin/env node
// Task 1.5 check: the Rust CI matrix declares the platform tiering and the
// single-leg frontend matrix declares the format/lint/typecheck/test/build
// quality commands.
//
// Fails (nonzero) unless `.github/workflows/ci.yml` structurally contains:
//   - the Linux `rust` leg and the main-only macOS/Windows `rust-desktop` legs,
//     with the `rust` job steps (fmt, clippy `-D warnings`, test) and the
//     `frontend` job steps (`pnpm install --frozen-lockfile`,
//     format/lint/typecheck/test/build, plus the headless E2E);
//   - the cache writer/reader split, the exact task-gate inventory, the
//     governance-section execution locations, and the duplicate-execution
//     reverse locks;
//   - the audited doc-only `paths-ignore` whitelist.
//
// The commands themselves run in the Rust/frontend CI matrices and, locally, in
// the governance gate's `quality-core` segment. This check deliberately does not
// re-run them: a task check proves its own behavior, not the shared static gates.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { assertPlatformMatrix, assertPlatformName, assertShardMatrix, matrixEntries, parseWorkflow } from "../workflow-matrix.mjs";
import { assertNoDefaultReleaseLatency, assertReleaseLatencyCarrier, RELEASE_LATENCY_WORKFLOW, RELEASE_LATENCY_FLAG } from "../release-edit-latency.mjs";
import { assertManualBenchmark, assertNoDefaultBenchmark, MANUAL_BENCHMARK_WORKFLOW, MANUAL_BENCHMARK_FLAG } from "../manual-50k-benchmark.mjs";
import { assertMacosPackaging } from "../ci-macos-packaging-structure.mjs";
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
function hasStep(workflow, jobId, name) {
  return (workflow?.jobs?.[jobId]?.steps || []).some((s) => s.name === name);
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

// The executable task inventory: only gates with no equivalent in the workspace
// or frontend suites stay here. Everything else is `archived: true` in the
// manifest, with its carrying gate named in `archivedReason`. Keeping the
// expected list here (rather than deriving it from ci.yml) is what makes a
// deleted or duplicated task fail instead of silently shrinking coverage.
const EXPECTED_SHARDS = {
  core: "1.1 1.3 13.3".split(" "),
};

// Audited doc-only whitelist. These files are referenced only in prose (no gate
// reads them), so a change touching nothing else may skip CI. Deliberately
// absent: README.md (tasks 17.1/18.1 read it), openspec/changes/archive/**
// (audio-effects gates read archived evidence) and artifacts/**
// (check-native-attestation reads its logs).
const DOC_ONLY_PATHS = [
  "docs/PRODUCT.md",
  "docs/DESIGN.md",
  "docs/ROADMAP.md",
  "docs/interface-terminology.md",
  "docs/arch-standards-remaining.md",
];

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
  // Linux carries the full workspace suite on PR and main; macOS/Windows run
  // theirs in the main-only `rust-desktop` job, whose platform Gates still
  // compile and test platform code on every PR.
  const RUST_MATRIX = {
    "Linux/workspace": { os: "ubuntu-latest", cache: "linux-workspace", packages: "--workspace" },
  };
  assertShardMatrix(matrixEntries(workflow, "rust", "ci.yml"), RUST_MATRIX, "ci.yml rust");
  assertPlatformName(workflow.jobs.rust.name, "Rust (${{ matrix.platform }} ${{ matrix.shard }})", "ci.yml rust name");

  const RUST_DESKTOP_MATRIX = {
    "macOS/desktop": { os: "xcode-27", cache: "macos-desktop", packages: "--workspace" },
    "Windows/desktop": { os: "windows-latest", cache: "windows-desktop", packages: "--workspace" },
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
  const CACHE_WRITERS = ["governance-static", "linux-workspace", "macos-desktop", "windows-desktop"];
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
  // Normalize Rustup before it contributes its ambient toolchain executables
  // to the cache key. Do not let normalization mask dependency/env inputs or
  // accidentally apply the Linux-only setup to a platform-specific cache.
  const rustupStepName = "Isolate Linux Rustup home";
  const rustupCommand = "bash scripts/verify/isolate-linux-rustup-home.sh";
  for (const [file, jobs, participants] of [
    ["ci.yml", workflow.jobs, ["rust", "task-gates", "governance-static"]],
    ["dependency-audit.yml", auditWorkflow.jobs, ["audit"]],
  ]) {
    for (const id of new Set([...Object.keys(jobs), ...participants])) {
      const steps = jobs[id]?.steps || [];
      const candidates = steps.filter((s) => s.name === rustupStepName || String(s.run || "").includes("isolate-linux-rustup-home.sh"));
      const label = `${file} job '${id}'`;
      if (!participants.includes(id)) {
        if (candidates.length) fail(`${label} must not isolate Linux Rustup home`);
        continue;
      }
      if (candidates.length !== 1) fail(`${label} must configure exactly one Linux Rustup home isolation step`);
      const [isolation] = candidates;
      const checkout = steps.findIndex((s) => String(s.uses || "").startsWith("actions/checkout@"));
      const install = steps.findIndex((s) => String(s.uses || "").startsWith("dtolnay/rust-toolchain@"));
      const cache = steps.findIndex((s) => s.name === CACHE_STEP);
      const isolationIndex = steps.indexOf(isolation);
      if (!(checkout >= 0 && checkout < isolationIndex && isolationIndex < install && install < cache)) {
        fail(`${label} must isolate Linux Rustup home after checkout and before toolchain installation and cache`);
      }
      if (isolation.name !== rustupStepName || isolation.run !== rustupCommand ||
          isolation.if !== undefined || isolation["continue-on-error"] !== undefined) {
        fail(`${label} must execute unconditional Linux Rustup home isolation using '${rustupCommand}'`);
      }
      const restore = steps[cache];
      if (restore.uses !== "Swatinem/rust-cache@v2" ||
          restore.with?.["env-vars"] !== undefined ||
          (restore.with?.["add-rust-environment-hash-key"] !== undefined &&
            String(restore.with?.["add-rust-environment-hash-key"]) !== "true")) {
        fail(`${label} must preserve the Rust cache action's environment key inputs`);
      }
    }
  }
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
  "pnpm --filter @echo/desktop test -- --run --reporter=verbose",
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
// `if: false` parses as the YAML boolean, which `|| ""` would fold into the
// same empty string as "unset". Test for presence, not emptiness.
if (test.if !== undefined) fail("rust Test must not be conditionally skipped");
if (test["continue-on-error"] === true) fail("rust Test must not set continue-on-error");
if (test.run !== "cargo test ${{ matrix.packages }} --all-targets --all-features") {
  fail("rust Test run must be 'cargo test ${{ matrix.packages }} --all-targets --all-features'");
}
if (clippy.if !== undefined) fail("rust Clippy must not be conditionally skipped");
if (clippy.run !== "cargo clippy --workspace --all-targets --all-features -- -D warnings") {
  fail("rust Clippy run must be 'cargo clippy --workspace --all-targets --all-features -- -D warnings'");
}
// 16.1's static half runs in governance-static (the workspace test covers its
// compile and behavior halves); 9.7/9.8 are no-ops on Linux and run for real in
// the platform Gates. Both steps must stay out of the Linux leg.
for (const gone of ["Task 16.1", "Task 9.7 9.8"]) {
  if (hasStep(workflow, "rust", gone)) {
    fail(`rust must not carry step '${gone}' (it moved to governance-static or the platform Gates)`);
  }
}
if (rustStatic.if !== undefined) fail("rust Rust static gates must not be conditionally skipped");
if (rustStatic.run !== "pnpm verify:governance -- --sections=rust-static") {
  fail("Rust static gates run must be 'pnpm verify:governance -- --sections=rust-static'");
}
if (rustStatic.env?.ECHO_PURITY_REQUIRE_BUILD !== undefined) {
  fail("Rust static gates must not set ECHO_PURITY_REQUIRE_BUILD (purity moved to governance-static)");
}

// rust-desktop: the main-only macOS/Windows legs. They carry the full workspace
// suite plus fmt and clippy; a job-level `if` cannot see matrix.*, which is why
// they are a separate job.
const DESKTOP_JOB_IF = "github.event_name != 'pull_request'";
if (String(workflow.jobs["rust-desktop"].if || "").trim() !== DESKTOP_JOB_IF) {
  fail(`rust-desktop job if must be '${DESKTOP_JOB_IF}'`);
}
const desktopFormat = step(workflow, "rust-desktop", "Format check");
if (desktopFormat.run !== "cargo fmt --all -- --check") {
  fail("rust-desktop Format check run must be 'cargo fmt --all -- --check'");
}
const desktopClippy = step(workflow, "rust-desktop", "Clippy");
if (desktopClippy.run !== "cargo clippy --workspace --all-targets --all-features -- -D warnings") {
  fail("rust-desktop Clippy run must be 'cargo clippy --workspace --all-targets --all-features -- -D warnings'");
}
const desktopTest = step(workflow, "rust-desktop", "Test");
if (desktopTest.if !== undefined) fail("rust-desktop Test must not be conditionally skipped");
if (desktopTest["continue-on-error"] === true) fail("rust-desktop Test must not set continue-on-error");
if (desktopTest.run !== "cargo test ${{ matrix.packages }} --all-targets --all-features") {
  fail("rust-desktop Test run must be 'cargo test ${{ matrix.packages }} --all-targets --all-features'");
}
// These gates only make sense on the Linux workspace shard. Clippy is allowed
// here now (it is the only main carrier of macOS/Windows clippy); the task and
// static gates would duplicate the workspace shard's work.
for (const forbidden of ["Task 16.1", "Task 9.7 9.8", "Rust static gates"]) {
  if (hasStep(workflow, "rust-desktop", forbidden)) {
    fail(`rust-desktop must not carry step '${forbidden}' (it only exists for the workspace shard)`);
  }
}

const RUST_ORDER = ["Format check", "Clippy", "Test", "Rust static gates"];
const rustSteps = workflow.jobs.rust.steps.map((s) => s.name).filter(Boolean);
const positions = RUST_ORDER.map((name) => rustSteps.indexOf(name));
if (!positions.every((v, i) => v >= 0 && (i === 0 || v > positions[i - 1]))) {
  fail(`rust step order must be ${RUST_ORDER.join(" < ")}`);
}
const DESKTOP_ORDER = ["Format check", "Clippy", "Test"];
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

// Frontend: the headless E2E runs after the Build step, because `--no-build`
// relies on that build having just produced the dist it drives.
const frontendSteps = workflow.jobs.frontend.steps.map((s) => s.name).filter(Boolean);
const buildIndex = frontendSteps.indexOf("Build");
const e2eIndex = frontendSteps.indexOf("Task 13.1 (headless E2E)");
if (buildIndex < 0 || e2eIndex < 0 || e2eIndex < buildIndex) {
  fail("frontend 'Build' must precede 'Task 13.1 (headless E2E)' (--no-build relies on it)");
}
const e2e = step(workflow, "frontend", "Task 13.1 (headless E2E)");
if (e2e.run !== "node scripts/verify/checks/task-13.1.mjs --no-build") {
  fail("Task 13.1 run must be 'node scripts/verify/checks/task-13.1.mjs --no-build'");
}
assertEnv(e2e, "CHROME_PATH", "/usr/bin/google-chrome", "frontend Task 13.1");
assertEnv(e2e, "CHROME_NO_SANDBOX", "1", "frontend Task 13.1");
assertRunIncludes(step(workflow, "frontend", "Test"), "pnpm --filter @echo/desktop test -- --run", "frontend Test");

const governanceStatic = step(workflow, "governance-static", "Run governance static gates");
// Exact equality: a run that merely *mentions* the flags (`echo "--sections=..."`)
// would keep the section strings present while executing nothing.
if (governanceStatic.run !== "pnpm verify:governance -- --sections=registry,static") {
  fail("governance-static run must be 'pnpm verify:governance -- --sections=registry,static'");
}
// Coverage is main-only: PRs must not install cargo-llvm-cov, and main must
// still run the gate. Both the install and the gate carry the same skip
// condition, pinned exactly so a permanently-skipped step cannot hide.
const COVERAGE_IF = "github.event_name != 'pull_request'";
const coverageInstall = step(workflow, "governance-static", "Install coverage tooling");
if (String(coverageInstall.if || "").trim() !== COVERAGE_IF) {
  fail(`governance-static 'Install coverage tooling' if must be '${COVERAGE_IF}'`);
}
const coverageStep = step(workflow, "governance-static", "Run coverage gate");
if (coverageStep.run !== "pnpm verify:governance -- --sections=coverage") {
  fail("governance-static coverage step run must be 'pnpm verify:governance -- --sections=coverage'");
}
if (String(coverageStep.if || "").trim() !== COVERAGE_IF) {
  fail(`governance-static 'Run coverage gate' if must be '${COVERAGE_IF}'`);
}
const t161 = step(workflow, "governance-static", "Task 16.1 static contract");
if (t161.run !== "pnpm verify:task -- 16.1") fail("Task 16.1 run must be 'pnpm verify:task -- 16.1'");
if (t161.if !== undefined) fail("governance-static Task 16.1 must not be conditionally skipped");
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
const releaseCommand = `node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,workspace-test,frontend-test ${MANUAL_BENCHMARK_FLAG} ${RELEASE_LATENCY_FLAG}`;
if (release.run !== releaseCommand) fail(`release-gate run must be '${releaseCommand}'`);
try {
  assertNoDefaultBenchmark(workflow);
  assertNoDefaultReleaseLatency(workflow);
  assertReleaseLatencyCarrier(parseWorkflow(
    readFileSync(resolve(ROOT, ".github", "workflows", RELEASE_LATENCY_WORKFLOW), "utf8"),
    RELEASE_LATENCY_WORKFLOW,
  ));
  assertManualBenchmark(parseWorkflow(
    readFileSync(resolve(ROOT, ".github", "workflows", MANUAL_BENCHMARK_WORKFLOW), "utf8"),
    MANUAL_BENCHMARK_WORKFLOW,
  ));
} catch (error) {
  fail(error.message);
}

// The release gate carries no cargo and no pnpm test command any more: every
// scenario it would run is a subset of the workspace/frontend suites or the
// manual 50k bench (explicitly deferred). Pinning the absence keeps a
// "just re-run it here" edit from silently re-spending the compile time this
// job was trimmed of.
{
  const forbidden = (workflow.jobs["release-gate"].steps || []).filter((s) => {
    const run = String(s.run || "");
    return /\bcargo\s+test\b/.test(run) || /\bpnpm\b.*\btest\b/.test(run);
  });
  if (forbidden.length) {
    fail(`release-gate must not run cargo/pnpm test commands (delegated), found [${forbidden.map((s) => s.name).join(", ")}]`);
  }
  const rustSteps = (workflow.jobs["release-gate"].steps || []).filter((s) =>
    String(s.name || "").startsWith("Install Rust toolchain") || String(s.name || "") === "Cache Rust build artifacts",
  );
  if (rustSteps.length) {
    fail(`release-gate must not install/cache a Rust toolchain (it runs no cargo), found [${rustSteps.map((s) => s.name).join(", ")}]`);
  }
}

// macos-native-gate exists only to run task-8.12 (it needs the staged dylibs).
// Asserting merely that the job runs unconditionally would leave a
// permanently-green empty job if its one real step were deleted.
const nativeGate = step(workflow, "macos-native-gate", "Verify libmpv smoke gate");
if (nativeGate.run !== "pnpm verify:task -- 8.12") {
  fail("macos-native-gate run must be 'pnpm verify:task -- 8.12'");
}
if (String(nativeGate.if || "").trim() !== "") fail("macos-native-gate must not be conditionally skipped");

try {
  assertMacosPackaging(workflow, parseWorkflow(
    readFileSync(resolve(ROOT, ".github", "workflows", "release.yml"), "utf8"),
    "release.yml",
  ));
} catch (error) {
  fail(error.message);
}

// The three performance scenarios share the task-gates core shard's toolchain
// and `linux-workspace` cache. Only a successful classifier's explicit skip
// may suppress the step on a PR. Verify its inputs, outcome and execution
// location so a missing or uncertain diff still runs the gate. The shard's cache
// step must stay read-only so the `--release` artifacts are never saved into a
// key the Rust shards restore.
// The injection proof suite is local-only. It proves the gates' own failure
// behavior, not product behavior; a job running it re-spends three runner
// instances per PR. Pinning the absence here makes re-adding it a red gate.
{
  const injectionRuns = Object.entries(workflow.jobs).flatMap(([jobId, job]) =>
    (job.steps || [])
      .filter((s) => String(s.run || "").includes("injection-suite.mjs"))
      .map((s) => `${jobId}:${s.name}`),
  );
  if (injectionRuns.length) {
    fail(`ci.yml must not run the injection proof suite in a job (local-only), found [${injectionRuns.join(", ")}]`);
  }
}

// Real small acceptance stays in the default workspace, not an ignored test.
const smallCorrectness = readFileSync(resolve(ROOT, "crates", "echo-desktop", "tests", "local_edit_correctness.rs"), "utf8");
for (const test of ["import_is_visible_without_rescan", "favorite_success_has_a_durable_after_image_and_visible_new_value", "playlist_membership_is_durable_and_projected"]) {
  const declaration = new RegExp(`^#\\[test\\]\\s*^fn ${test}\\(\\)`, "m");
  if (!declaration.test(smallCorrectness)) fail(`default workspace correctness test '${test}' must remain unconditional and nonignored`);
}
if (/#!?\[(?:ignore|cfg(?:_attr)?)\b/.test(smallCorrectness)) {
  fail("default workspace correctness file must not conditionally skip tests");
}

const taskGatesRun = step(workflow, "task-gates", "Verify task gates");
if (taskGatesRun.if !== undefined || taskGatesRun.run !== "pnpm verify:task -- ${{ matrix.tasks }}") {
  fail("task-gates tasks must remain unconditional");
}
if (String(step(workflow, "task-gates", "Cache Rust build artifacts").with?.["save-if"] ?? "") !== "false") {
  fail("task-gates cache step must remain a read-only cache consumer");
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

// `cargo fmt --all -- --check` is platform-independent. `rust` covers Linux on
// every event; `rust-desktop` covers macOS/Windows on main. Re-adding it to any
// third job (or to a PR-running macOS/Windows leg) must fail.
{
  const jobs = jobsRunning("cargo fmt --all -- --check");
  if (JSON.stringify(jobs) !== JSON.stringify(["rust", "rust-desktop"])) {
    fail(`'cargo fmt --all -- --check' must run in exactly ['rust', 'rust-desktop'] (Linux + main-only macOS/Windows), got [${jobs.join(", ")}]`);
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

// Every job that carries a required check must run on both PR and main. A
// job-level `if` or `continue-on-error` would keep the strings above intact
// while the job itself stops running or stops failing.
for (const jobId of ["rust", "frontend", "governance-static", "release-gate", "task-gates", "macos-platform-gate", "macos-native-gate", "windows-platform-gate"]) {
  assertAlwaysRuns(workflow, jobId);
}

// ---------------------------------------------------------------------------
// Task shards: the union must cover the four unique gates exactly once. 13.1
// runs in the Frontend job and 16.1 in governance-static, so neither may appear
// in a shard.
// ---------------------------------------------------------------------------
const matrix = workflow.jobs["task-gates"]?.strategy?.matrix;
if (!matrix || Object.keys(matrix).some((key) => key !== "include")) {
  fail("task-gates must use an include-only matrix");
}
const shards = matrix.include || [];
if (shards.length !== 1) fail(`task-gates must have 1 shard, got ${shards.length}`);
const byShard = new Map(shards.map((entry) => [entry.shard, String(entry.tasks || "").split(/\s+/).filter(Boolean)]));
for (const [name, expected] of Object.entries(EXPECTED_SHARDS)) {
  const actual = byShard.get(name);
  if (!actual) fail(`task-gates is missing shard '${name}'`);
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`task-gates shard '${name}' must be [${expected.join(" ")}], got [${actual.join(" ")}]`);
  }
}
const union = [...byShard.values()].flat();
if (union.length !== 3) fail(`task-gates shard union must be 3 tasks, got ${union.length}`);
if (new Set(union).size !== union.length) fail("task-gates shard union contains duplicates");
if (union.includes("16.1")) fail("16.1 must run in governance-static, not task-gates");
if (union.includes("13.1")) fail("13.1 must run in the Frontend job, not task-gates");

// Archived tasks: the manifest must say which real gate carries their assertion.
// A reason that names no carrier, or names one that ci.yml no longer executes,
// is how an archived assertion silently stops running.
{
  const manifest = JSON.parse(readFileSync(resolve(ROOT, "scripts", "verify", "manifest.json"), "utf8"));
  const rustTest = step(workflow, "rust", "Test");
  const linuxCarriesWorkspace = () => {
    const entries = matrixEntries(workflow, "rust", "ci.yml");
    return (
      entries.length === 1 &&
      entries[0].packages === "--workspace" &&
      rustTest.run === "cargo test ${{ matrix.packages }} --all-targets --all-features"
    );
  };
  const frontendCarriesSuite = () =>
    String(step(workflow, "frontend", "Test").run || "").includes("pnpm --filter @echo/desktop test -- --run");
  const platformGatesCarry = () =>
    ["macos-platform-gate", "windows-platform-gate"].every(
      (id) => workflow.jobs[id] && workflow.jobs[id].if === undefined,
    );
  const CARRIERS = [
    { phrase: "cargo test --workspace --all-targets --all-features", check: linuxCarriesWorkspace },
    { phrase: "workspace test", check: linuxCarriesWorkspace },
    { phrase: "Frontend job", check: frontendCarriesSuite },
    { phrase: "platform Gate", check: platformGatesCarry },
  ];
  for (const task of manifest.tasks.filter((t) => t.archived)) {
    const reason = String(task.archivedReason || "");
    const named = CARRIERS.filter((carrier) => reason.includes(carrier.phrase));
    if (!named.length) {
      fail(`archived task '${task.id}' must name its carrying gate (workspace test / Frontend job / platform Gate)`);
    }
    for (const carrier of named) {
      if (!carrier.check()) {
        fail(`archived task '${task.id}' names '${carrier.phrase}' but ci.yml does not execute that carrier`);
      }
    }
  }
}

// Sections carried by a `--sections=` selection are checked for *inclusion* in
// that step's selection; the remaining sections are carried by a dedicated
// step, so the step existing at all is the assertion.
const SECTION_SELECTOR_STEP = {
  registry: ["governance-static", "Run governance static gates"],
  static: ["governance-static", "Run governance static gates"],
  coverage: ["governance-static", "Run coverage gate"],
  "rust-static": ["rust", "Rust static gates"],
  purity: ["governance-static", "Run build purity gate"],
};
const SECTION_DEDICATED_STEP = {
  quality: ["rust", "Test"],
  embedded: ["macos-platform-gate", "Verify embedded frontend freshness"],
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

// ---------------------------------------------------------------------------
// Doc-only skip whitelist. The list may only shrink or grow through an explicit
// edit here, which is the reviewable proof that a new entry is not a gate input.
// ---------------------------------------------------------------------------
for (const event of ["push", "pull_request"]) {
  const actual = workflow.on?.[event]?.["paths-ignore"];
  if (JSON.stringify(actual) !== JSON.stringify(DOC_ONLY_PATHS)) {
    fail(
      `ci.yml ${event}.paths-ignore must be exactly [${DOC_ONLY_PATHS.join(", ")}], ` +
        `got [${Array.isArray(actual) ? actual.join(", ") : actual}]`,
    );
  }
}

process.stdout.write(
  "ok 1.5: Linux workspace + main-only macOS/Windows workspace matrix and single-leg frontend matrix declare the quality commands; CI structure (tiering, task inventory, sections, duplicate-execution locks, doc-only whitelist) verified\n",
);
