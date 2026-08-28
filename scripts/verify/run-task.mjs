#!/usr/bin/env node
// Unified task verification runner.
//
// Usage:
//   pnpm verify:task -- <task-id...>     run the listed tasks
//   pnpm verify:task -- --all            run every registered task
//
// A task ID is verified against scripts/verify/manifest.json. For each task
// the runner:
//   - fails with a nonzero exit if the ID is unknown;
//   - fails if the entry defines no command and no executable command exists;
//   - runs each declared command (cwd = repo root) and fails on the first
//     nonzero exit;
//   - fails if any declared human/native evidence file is missing or empty.
//
// The runner never modifies lockfiles, so "lockfile unchanged" can be asserted
// by snapshotting hashes before and after a run.

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const MANIFEST =
  process.env.ECHO_VERIFY_MANIFEST || resolve(ROOT, "scripts", "verify", "manifest.json");

function fail(msg) {
  process.stderr.write(`error: ${msg}\n`);
  process.exitCode = 1;
}

function runCommand(cmd, taskId) {
  const result = spawnSync(cmd, {
    cwd: ROOT,
    shell: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    fail(`task ${taskId}: command exited ${result.status}: ${cmd}`);
    return false;
  }
  return true;
}

function checkEvidence(evidence, taskId) {
  for (const item of evidence || []) {
    const target = resolve(ROOT, item.file || item.name || "");
    if (!existsSync(target) || readFileSync(target, "utf8").trim() === "") {
      fail(`task ${taskId}: missing human/native evidence '${item.name}'`);
      return false;
    }
  }
  return true;
}

function verifyTask(id) {
  const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
  const entry = manifest.tasks.find((t) => t.id === id);
  if (!entry) {
    fail(`unknown task id: ${id}`);
    return false;
  }
  if ((!entry.commands || entry.commands.length === 0) && !(entry.evidence && entry.evidence.length)) {
    fail(`task ${id}: no verification command registered`);
    return false;
  }
  const commands = entry.commands || [];
  if (!runCommand(commands.map((c) => c.cmd).join(" && "), id)) {
    return false;
  }
  if (!checkEvidence(entry.evidence, id)) {
    return false;
  }
  process.stdout.write(`ok: task ${id} (${entry.title})\n`);
  return true;
}

const argv = process.argv.slice(2).filter((a) => a !== "--");
const ids = argv.length
  ? argv.flatMap((a) => (a === "--all" ? [] : a.split(/\s+/)))
  : [];
const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));

let requested;
if (argv.includes("--all")) {
  requested = manifest.tasks.map((t) => t.id);
} else if (ids.length) {
  requested = ids;
} else {
  fail("no task ids given (use -- <id...> or -- --all)");
  process.exit(1);
}

let ok = true;
for (const id of requested) {
  if (!verifyTask(id)) ok = false;
}
process.exit(ok ? 0 : 1);
