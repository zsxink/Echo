#!/usr/bin/env node
// Task 1.5 check: three-platform CI matrix declares the Rust fmt/clippy/test and
// the frontend format/lint/typecheck/test/build quality commands.
//
// Fails (nonzero) unless `.github/workflows/ci.yml` structurally contains the
// three platform/runner pairs, platform-based names, the `rust` job steps (fmt,
// clippy `-D warnings`, test) and the `frontend` job steps (`pnpm install
// --frozen-lockfile`, format/lint/typecheck/test/build).
//
// The commands themselves run in the Rust/frontend CI matrices and, locally, in
// the governance gate's `quality-core` segment. This check deliberately does not
// re-run them: a task check proves its own behavior, not the shared static gates.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { assertPlatformMatrix, assertPlatformName, matrixEntries, parseWorkflow } from "../workflow-matrix.mjs";
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

// The executable task inventory is partitioned across four shards; 16.1 runs
// in the Rust Linux job because it needs the built Tauri shell. Keeping the
// expected lists here (rather than deriving them from ci.yml) is what makes a
// deleted or duplicated task fail instead of silently shrinking coverage.
const EXPECTED_SHARDS = {
  core: "1.1 1.3 1.8 3.1 4.1 5.1 6.1 11.5 12.5 13.3".split(" "),
  desktop: "7.1 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 10.6 11.1 11.2 11.4 16.2".split(" "),
  "frontend-a": "10.1 10.2 10.3 10.4 10.5 11.3 11.6 13.1".split(" "),
  "frontend-b": "10.7 10.8 10.9 10.10 12.1 12.2 12.3".split(" "),
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
  assertPlatformMatrix(
    matrixEntries(workflow, "rust", "ci.yml"),
    { Linux: { os: "ubuntu-latest" }, macOS: { os: "xcode-27" }, Windows: { os: "windows-latest" } },
    "ci.yml rust",
  );
  assertPlatformMatrix(
    matrixEntries(workflow, "frontend", "ci.yml"),
    { Linux: { os: "ubuntu-latest" }, macOS: { os: "xcode-27" }, Windows: { os: "windows-latest" } },
    "ci.yml frontend",
  );
  assertPlatformName(workflow.jobs.rust.name, "Rust (${{ matrix.platform }})", "ci.yml rust name");
  assertPlatformName(workflow.jobs.frontend.name, "Frontend (${{ matrix.platform }})", "ci.yml frontend name");
} catch (error) {
  fail(error.message);
}

const REQUIRED = [
  "cargo fmt --all -- --check",
  "cargo clippy --workspace --all-targets --all-features -- -D warnings",
  "cargo test --workspace --all-targets --all-features",
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
// tiering and the Linux-only static gates. A rename or move must fail here.
// ---------------------------------------------------------------------------
const test = step(workflow, "rust", "Test");
assertIfIncludes(test, "github.event_name != 'pull_request'", "rust Test");
assertIfIncludes(test, "matrix.platform == 'Linux'", "rust Test");
// The two substring assertions above cannot tell `||` from `&&`; with `&&`
// every PR skips the full workspace test and so does main. Pin the whole
// expression, and pin that the step cannot be made advisory.
const TEST_IF = "github.event_name != 'pull_request' || matrix.platform == 'Linux'";
if (String(test.if || "").trim() !== TEST_IF) {
  fail(`rust Test if must be '${TEST_IF}'`);
}
if (test["continue-on-error"] === true) fail("rust Test must not set continue-on-error");
if (test.run !== "cargo test --workspace --all-targets --all-features") {
  fail("rust Test run must be 'cargo test --workspace --all-targets --all-features'");
}

const format = step(workflow, "rust", "Format check");
if (format.run !== "cargo fmt --all -- --check") fail("rust Format check run must be 'cargo fmt --all -- --check'");
const clippy = step(workflow, "rust", "Clippy");
if (clippy.run !== "cargo clippy --workspace --all-targets --all-features -- -D warnings") {
  fail("rust Clippy run must be 'cargo clippy --workspace --all-targets --all-features -- -D warnings'");
}

const rustStatic = step(workflow, "rust", "Rust static gates");
assertIfIncludes(rustStatic, "matrix.platform == 'Linux'", "Rust static gates");
if (rustStatic.run !== "pnpm verify:governance -- --sections=rust-static,purity") {
  fail("Rust static gates run must be 'pnpm verify:governance -- --sections=rust-static,purity'");
}
assertEnv(rustStatic, "ECHO_PURITY_REQUIRE_BUILD", "1", "Rust static gates");

const task161 = step(workflow, "rust", "Task 16.1");
assertIfIncludes(task161, "matrix.platform == 'Linux'", "Task 16.1");
if (task161.run !== "pnpm verify:task -- 16.1") fail("Task 16.1 run must be 'pnpm verify:task -- 16.1'");

const task978 = step(workflow, "rust", "Task 9.7 9.8");
assertIfIncludes(task978, "matrix.platform == 'Linux'", "Task 9.7 9.8");
if (task978.run !== "pnpm verify:task -- 9.7 9.8") fail("Task 9.7 9.8 run must be 'pnpm verify:task -- 9.7 9.8'");

const governanceStatic = step(workflow, "governance-static", "Run governance static gates");
// Exact equality: a run that merely *mentions* the flags (`echo "--sections=..."`)
// would keep the section strings present while executing nothing.
if (governanceStatic.run !== "pnpm verify:governance -- --sections=registry,static,coverage") {
  fail("governance-static run must be 'pnpm verify:governance -- --sections=registry,static,coverage'");
}

const release = step(workflow, "release-gate", "Run release gate without external artifacts");
if (release.run !== "node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf") {
  fail("release-gate run must use --skip-static --delegated=governance,perf");
}

const perf = step(workflow, "perf", "Run performance scenarios");
if (perf.run !== "pnpm verify:perf") fail("perf run must be 'pnpm verify:perf'");

const injection = workflow.jobs["injection-proofs"];
if (JSON.stringify(injection?.strategy?.matrix?.shard) !== JSON.stringify([1, 2])) {
  fail("injection-proofs matrix.shard must be [1, 2]");
}
const shardStep = step(workflow, "injection-proofs", "Prove each governance gate");
assertRunIncludes(shardStep, "--shard=${{ matrix.shard }}/2", "injection shard step");
const shellStep = step(workflow, "injection-proofs", "Build the shell binary checked by embedded-frontend proof");
assertIfIncludes(shellStep, "matrix.shard == 1", "shell build step");
const embeddedStep = step(workflow, "injection-proofs", "Verify embedded frontend freshness");
assertIfIncludes(embeddedStep, "matrix.shard == 1", "embedded freshness step");
if (embeddedStep.run !== "pnpm verify:frontend-fresh") fail("embedded freshness run must be 'pnpm verify:frontend-fresh'");

// Every job that carries a required check must run on both PR and main. A
// job-level `if` or `continue-on-error` would keep the strings above intact
// while the job itself stops running or stops failing.
for (const jobId of ["rust", "governance-static", "injection-proofs", "release-gate", "perf", "task-gates"]) {
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
if (shards.length !== 4) fail(`task-gates must have 4 shards, got ${shards.length}`);
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
  purity: ["rust", "Rust static gates"],
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
  "ok 1.5: three-platform CI matrix declares the Rust + frontend quality commands (executed by the CI matrices and quality-core); CI structure (tiering, task shards, sections) verified\n",
);