#!/usr/bin/env node
// Task 7.1: the repository-local governance gate used by CI.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const checks = [
  // One place runs the shared Rust static gates for the whole workspace. Task
  // checks assert their own targeted behavior instead of repeating fmt/clippy.
  ["quality-core format", "cargo", ["fmt", "--all", "--", "--check"], "quality"],
  ["quality-core lint", "cargo", ["clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"], "quality"],
  ["quality-core tests", "cargo", ["test", "--workspace", "--all-targets", "--all-features"], "quality"],
  ["scenario reconciliation", "node", ["scripts/verify/reconcile-scenarios.mjs"], "registry"],
  ["scenario command validity", "node", ["scripts/verify/validate-scenario-commands.mjs"], "rust-static"],
  ["scenario registry drift", "node", ["scripts/verify/gen-scenario-manifests.mjs"], "registry"],
  ["scale limits", "node", ["scripts/verify/check-scale.mjs"], "static"],
  // The fixture gate was previously registered in the manifest but wired into
  // no CI job and no scenario, so it could go red for months unnoticed. It runs
  // here (the governance job installs ffmpeg) and reports an older committed
  // encoder version as environment drift rather than a failure, so a runner
  // whose ffmpeg differs from the declared toolchain still gates determinism,
  // committed-integrity, manifest coverage and the licensing guard.
  ["fixture determinism and licensing", "node", ["scripts/verify/checks/task-1.6.mjs"], "static"],
  ["lint inheritance", "node", ["scripts/verify/check-lint-inheritance.mjs"], "static"],
  // apps/desktop/src/styles/*.css are generated from the prototypes; a hand
  // edit would silently diverge the app from its visual source of truth.
  ["prototype css drift", "node", ["scripts/verify/checks/prototype-css-drift.mjs"], "static"],
  ["toolchain pin", "node", ["scripts/verify/check-toolchain.mjs"], "static"],
  ["verification validity", "node", ["scripts/verify/check-verification-validity.mjs"], "static"],
  ["scenario evidence coverage", "node", ["scripts/verify/check-scenario-evidence.mjs"], "rust-static"],
  ["scenario command proof", "node", ["scripts/verify/check-scenario-command-proof.mjs"], "static"],
  ["build purity", "node", ["scripts/verify/check-build-purity.mjs"], "purity"],
  ["coverage gate", "node", ["scripts/verify/checks/task-12.8.mjs"], "coverage"],
  // Listed after the workspace build so that, when a binary exists, it embeds a
  // dist the frontend step already produced rather than an absent one.
  ["embedded frontend freshness", "node", ["scripts/verify/checks/check-embedded-frontend.mjs"], "embedded"],
];

// CI runs the mutation-based injection proofs in a separate job so they can
// overlap with the slower cargo and release gates. Local governance still
// includes them by default.
if (!process.argv.includes("--without-injection")) {
  checks.push(["gate injection proofs", "node", ["scripts/verify/injection-suite.mjs"], "injection"]);
}

const SECTION_ORDER = ["quality", "registry", "rust-static", "static", "coverage", "purity", "embedded", "injection"];
const argv = process.argv.slice(2);
if (argv.includes("--list-sections")) {
  process.stdout.write(`${SECTION_ORDER.join(" ")}\n`);
  process.exit(0);
}
const sectionsFlag = argv.find((arg) => arg.startsWith("--sections="));
let selectedSections = null;
if (sectionsFlag) {
  const raw = sectionsFlag.slice("--sections=".length);
  const requested = raw.split(",").map((s) => s.trim()).filter(Boolean);
  if (!requested.length) {
    process.stderr.write("FAIL governance: --sections requires at least one section\n");
    process.exit(1);
  }
  const unknown = requested.filter((s) => !SECTION_ORDER.includes(s));
  if (unknown.length) {
    process.stderr.write(`FAIL governance: unknown section(s): ${unknown.join(", ")}\n`);
    process.exit(1);
  }
  selectedSections = new Set(requested);
}
const withoutInjection = argv.includes("--without-injection");
const runnable = checks.filter(([, , , section]) =>
  (!selectedSections || selectedSections.has(section))
  && !(withoutInjection && section === "injection"));
if (!runnable.length) {
  process.stderr.write("FAIL governance: section selection produced no checks\n");
  process.exit(1);
}

// An injection proof that could not be evaluated is not a proof. The suite
// names a skip when its violation needs an artifact this machine lacks; here
// that has to be a failure, exactly as `ECHO_PURITY_REQUIRE_BUILD=1` turns the
// build-purity skip into one.
process.env.ECHO_INJECTION_REQUIRE_ALL = "1";

for (const [label, command, args] of runnable) {
  process.stdout.write(`\n== ${label} ==\n`);
  const result = spawnSync(command, args, { cwd: ROOT, stdio: "inherit" });
  if (result.status !== 0) {
    process.stderr.write(`FAIL governance: ${label} exited ${result.status}\n`);
    process.exit(result.status || 1);
  }
}
process.stdout.write("ok governance: CI core gate passed\n");
