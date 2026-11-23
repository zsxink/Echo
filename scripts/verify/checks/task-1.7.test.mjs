import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, delimiter, join } from "node:path";
import { test } from "node:test";

const POSIX = process.platform === "win32" ? "fixture tool stubs use POSIX executable scripts" : false;
const LOCAL = ["cargo", "deny", "check", "licenses", "bans", "sources"];
const ADVISORIES = ["cargo", "deny", "check", "advisories"];
const AUDIT = ["cargo", "audit", "--ignore", "RUSTSEC-2099-0001"];
const PNPM = ["pnpm", "audit", "--prod", "--audit-level", "high"];

const stub = `#!/usr/bin/env node
const [path, ...args] = process.argv.slice(1);
const tool = require("node:path").basename(path);
const cmd = [tool, ...args];
require("node:fs").appendFileSync(process.env.AUDIT_CALLS, JSON.stringify(cmd) + "\\n");
const outcome = JSON.parse(process.env.AUDIT_STUB_OUTCOMES).find(({ command }) =>
  JSON.stringify(command) === JSON.stringify(cmd));
if (outcome) {
  console.error(outcome.output);
  process.exit(outcome.status);
}
`;

function run({ offline = false, outcomes = [], licenses = "echo-app\necho-core\necho-desktop\n" } = {}) {
  const root = mkdtempSync(join(tmpdir(), "echo-audit-test-"));
  try {
    const check = join(root, "scripts", "verify", "checks", basename(new URL("./task-1.7.mjs", import.meta.url).pathname));
    const bin = join(root, "bin");
    const callsFile = join(root, "calls.jsonl");
    mkdirSync(join(root, "scripts", "verify", "checks"), { recursive: true });
    mkdirSync(join(root, "apps", "desktop"), { recursive: true });
    mkdirSync(bin);
    copyFileSync(new URL("./task-1.7.mjs", import.meta.url), check);
    writeFileSync(join(root, "deny.toml"), '[advisories]\nignore = ["RUSTSEC-2099-0001"]\n');
    writeFileSync(join(root, "LICENSES.md"), licenses);
    for (const name of ["cargo", "cargo-deny", "cargo-audit", "pnpm"]) {
      writeFileSync(join(bin, name), stub, { mode: 0o755 });
    }
    const run = spawnSync(process.execPath, [check], {
      cwd: root,
      encoding: "utf8",
      timeout: 15_000,
      env: {
        ...process.env,
        PATH: [bin, process.env.PATH].join(delimiter),
        AUDIT_CALLS: callsFile,
        AUDIT_STUB_OUTCOMES: JSON.stringify(outcomes),
        ECHO_AUDIT_OFFLINE_ONLY: offline ? "1" : "0",
      },
    });
    assert.ifError(run.error);
    const calls = readFileSync(callsFile, "utf8").trimEnd().split("\n").map((line) => JSON.parse(line));
    return { ...run, calls };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

const DENY_VERSION = ["cargo-deny", "--version"];
const AUDIT_VERSION = ["cargo-audit", "--version"];

test("offline PR uses one combined policy command and no advisory tools", { skip: POSIX }, () => {
  const result = run({ offline: true });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.calls, [DENY_VERSION, LOCAL]);
  assert.match(result.stderr, /online advisory checks .*skipped/);
});

test("online scan keeps one Cargo advisory call, cargo-audit ignores and production pnpm audit", { skip: POSIX }, () => {
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.calls, [DENY_VERSION, AUDIT_VERSION, LOCAL, ADVISORIES, AUDIT, PNPM]);
});

test("policy failures and incomplete license inventories block before online checks", { skip: POSIX }, () => {
  const denied = run({ outcomes: [{ command: LOCAL, status: 4, output: "licenses FAILED" }] });
  assert.equal(denied.status, 1);
  assert.match(denied.stderr, /cargo deny check licenses bans sources.*exited 4/);
  assert.deepEqual(denied.calls, [DENY_VERSION, AUDIT_VERSION, LOCAL]);

  const incomplete = run({ licenses: "echo-app\necho-core\n" });
  assert.equal(incomplete.status, 1);
  assert.match(incomplete.stderr, /LICENSES.md does not list workspace member `echo-desktop`/);
  assert.deepEqual(incomplete.calls, [DENY_VERSION, AUDIT_VERSION, LOCAL]);
});

test("online advisory findings block, but network failures preserve other scans and cache fallback", { skip: POSIX }, () => {
  const finding = run({ outcomes: [{ command: ADVISORIES, status: 1, output: "error: RUSTSEC-2099-0002" }] });
  assert.equal(finding.status, 1);
  assert.match(finding.stderr, /cargo deny check advisories reported findings/);
  assert.match(finding.stderr, /RUSTSEC-2099-0002/);
  assert.deepEqual(finding.calls, [DENY_VERSION, AUDIT_VERSION, LOCAL, ADVISORIES]);

  const cachedAudit = ["cargo", "audit", "--no-fetch", "--ignore", "RUSTSEC-2099-0001"];
  const unavailable = run({
    outcomes: [
      { command: ADVISORIES, status: 1, output: "failed to fetch advisory database: ENOTFOUND" },
      { command: AUDIT, status: 1, output: "failed to fetch advisory database: ENOTFOUND" },
      { command: cachedAudit, status: 0, output: "0 vulnerabilities" },
    ],
  });
  assert.equal(unavailable.status, 0, unavailable.stderr);
  assert.match(unavailable.stderr, /offline .*cargo deny check advisories skipped/);
  assert.match(unavailable.stderr, /no vulnerabilities \(cached advisory DB\)/);
  assert.deepEqual(unavailable.calls, [DENY_VERSION, AUDIT_VERSION, LOCAL, ADVISORIES, AUDIT, cachedAudit, PNPM]);
});
