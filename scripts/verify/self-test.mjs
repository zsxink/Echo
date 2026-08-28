#!/usr/bin/env node
// Task 1.3 self-test: the verify:task runner must exit nonzero for unknown
// IDs, missing commands, failing commands and missing human evidence, and must
// never modify lockfiles.
//
// It drives the real runner against a temporary fixture manifest and asserts
// each failure mode plus one passing case, then checks the repo lockfiles are
// byte-for-byte unchanged after a full run.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const RUNNER = resolve(ROOT, "scripts", "verify", "run-task.mjs");
const LOCKFILES = ["Cargo.lock", "pnpm-lock.yaml"].map((f) =>
  resolve(ROOT, f),
);

let failures = 0;
function assert(cond, label) {
  if (cond) {
    process.stdout.write(`ok   ${label}\n`);
  } else {
    process.stderr.write(`FAIL ${label}\n`);
    failures += 1;
  }
}

function hashOf(path) {
  try {
    return createHash("sha256").update(readFileSync(path)).digest("hex");
  } catch {
    return null;
  }
}

function runRunner(manifestPath, args) {
  return spawnSync(process.execPath, [RUNNER, ...args], {
    cwd: ROOT,
    encoding: "utf8",
    env: { ...process.env, ECHO_VERIFY_MANIFEST: manifestPath },
  });
}

const dir = mkdtempSync(join(tmpdir(), "echo-verify-selftest-"));
const fixture = resolve(dir, "manifest.json");
writeFileSync(
  fixture,
  JSON.stringify({
    version: 1,
    tasks: [
      { id: "ok", title: "passing", commands: [{ desc: "x", cmd: "node -e 'process.exit(0)'" }] },
      { id: "missing-command", title: "no command" },
      { id: "failing", title: "failing", commands: [{ desc: "x", cmd: "node -e 'process.exit(3)'" }] },
      {
        id: "missing-evidence",
        title: "missing evidence",
        commands: [{ desc: "x", cmd: "node -e 'process.exit(0)'" }],
        evidence: [{ name: "native.txt", file: "./nonexistent/native.txt" }],
      },
    ],
    scenarios: [],
  }),
  "utf8",
);

// Failure modes must return nonzero.
const cases = [
  [["bogus-id"], "unknown id is nonzero"],
  [["missing-command"], "missing command is nonzero"],
  [["failing"], "failing command is nonzero"],
  [["missing-evidence"], "missing human evidence is nonzero"],
  [["ok", "bogus-id"], "partial failure is nonzero"],
];
for (const [args, label] of cases) {
  const r = runRunner(fixture, args);
  assert(r.status !== 0, `${label} (got ${r.status})`);
}

// Successful single task returns zero.
const okRun = runRunner(fixture, ["ok"]);
assert(okRun.status === 0, "passing task is zero");

// Lockfiles must be unchanged after a run.
const before = LOCKFILES.map(hashOf);
runRunner(fixture, ["ok", "failing", "--all"]);
const after = LOCKFILES.map(hashOf);
assert(
  JSON.stringify(before) === JSON.stringify(after),
  "lockfiles unchanged by verify run",
);

rmSync(dir, { recursive: true, force: true });

if (failures) {
  process.stderr.write(`self-test: ${failures} failure(s)\n`);
  process.exit(1);
}
process.stdout.write("self-test: all task-verifier assertions pass\n");
