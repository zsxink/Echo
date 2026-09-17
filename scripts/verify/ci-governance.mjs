#!/usr/bin/env node
// Task 7.1: the repository-local governance gate used by CI.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const checks = [
  ["scenario reconciliation", "node", ["scripts/verify/reconcile-scenarios.mjs"]],
  ["scenario command validity", "node", ["scripts/verify/validate-scenario-commands.mjs"]],
  ["scenario manifest validity", "node", ["scripts/verify/validate-scenario-manifests.mjs"]],
  ["scale limits", "node", ["scripts/verify/check-scale.mjs"]],
  ["lint inheritance", "node", ["scripts/verify/check-lint-inheritance.mjs"]],
  ["toolchain pin", "node", ["scripts/verify/check-toolchain.mjs"]],
  ["verification validity", "node", ["scripts/verify/check-verification-validity.mjs"]],
  ["scenario command churn", "node", ["scripts/verify/check-scenario-churn.mjs"]],
  ["build purity", "node", ["scripts/verify/check-build-purity.mjs"]],
  ["architecture and workspace tests", "cargo", ["test", "--workspace", "--all-targets", "--all-features"]],
  ["generated contract drift", "cargo", ["test", "-p", "echo-desktop", "ipc::generate::tests::committed_generated_file_is_in_sync_with_the_generator"]],
  ["coverage gate", "node", ["scripts/verify/checks/task-12.8.mjs"]],
];

for (const [label, command, args] of checks) {
  process.stdout.write(`\n== ${label} ==\n`);
  const result = spawnSync(command, args, { cwd: ROOT, stdio: "inherit" });
  if (result.status !== 0) {
    process.stderr.write(`FAIL governance: ${label} exited ${result.status}\n`);
    process.exit(result.status || 1);
  }
}
process.stdout.write("ok governance: CI core gate passed\n");
