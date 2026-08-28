#!/usr/bin/env node
// Task 1.3 check: toolchain pinning, editorconfig, unified verify runners and
// the self-test proving the verifier's contract.
//
// Verifies that:
//   - the pinning/config files exist and are non-empty (rust-toolchain.toml,
//     .nvmrc, .editorconfig, Cargo.lock, pnpm-lock.yaml);
//   - the root package.json defines verify:task, verify:scenario and
//     verify:self-test scripts;
//   - `node scripts/verify/self-test.mjs` exits 0 (unknown id / missing
//     command / failing command / missing human evidence all nonzero, and the
//     lockfiles are unchanged after a run);
//   - `pnpm install --frozen-lockfile` succeeds from the repo root and leaves
//     Cargo.lock and pnpm-lock.yaml byte-for-byte (sha256) unchanged.
//
// Fails (nonzero) on the first failure.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 1.3: ${msg}\n`);
  process.exit(1);
}

// Step 1: pinning/config files exist and are non-empty.
const REQUIRED_FILES = [
  "rust-toolchain.toml",
  ".nvmrc",
  ".editorconfig",
  "Cargo.lock",
  "pnpm-lock.yaml",
];
for (const name of REQUIRED_FILES) {
  const path = resolve(ROOT, name);
  if (readFileSync(path, "utf8").trim() === "") {
    fail(`file '${name}' is missing or empty`);
  }
}

// Step 2: root package.json defines the verify scripts.
const pkg = JSON.parse(readFileSync(resolve(ROOT, "package.json"), "utf8"));
for (const script of ["verify:task", "verify:scenario", "verify:self-test"]) {
  if (typeof pkg.scripts?.[script] !== "string" || !pkg.scripts[script].trim()) {
    fail(`package.json does not define a '${script}' script`);
  }
}

// Step 3: self-test proves the verifier's contract (unknown id, missing
// command, failing command, missing human evidence -> nonzero, lockfiles
// unchanged).
const selfTest = spawnSync(process.execPath, ["scripts/verify/self-test.mjs"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (selfTest.status !== 0) {
  fail(`self-test exited ${selfTest.status}\n${selfTest.stdout}${selfTest.stderr}`);
}

// Step 4: `pnpm install --frozen-lockfile` must succeed and leave the
// lockfiles unchanged.
const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");
const LOCKFILES = ["Cargo.lock", "pnpm-lock.yaml"];
const before = LOCKFILES.map((f) => sha256(resolve(ROOT, f)));

const install = spawnSync("pnpm", ["install", "--frozen-lockfile"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (install.status !== 0) {
  fail(`pnpm install --frozen-lockfile exited ${install.status}\n${install.stdout}${install.stderr}`);
}

const after = LOCKFILES.map((f) => sha256(resolve(ROOT, f)));
if (JSON.stringify(before) !== JSON.stringify(after)) {
  fail("lockfiles changed after pnpm install --frozen-lockfile");
}

process.stdout.write("ok 1.3: toolchain pinning, editorconfig, verify runners and self-test pass\n");