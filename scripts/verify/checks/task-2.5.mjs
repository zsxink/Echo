#!/usr/bin/env node
// Task 2.5 check: Core error classification.
//
// Fails unless the `error::privacy_tests` tests pass and at least one actually
// ran: infrastructure errors keep their source and public errors never leak
// absolute paths (privacy tests from task 1.8, still valid against the 2.5
// classification). Shared fmt/clippy gates live in quality-core.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 2.5: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-core", "--all-features", "error::privacy_tests::"], { cwd: ROOT, encoding: "utf8" });
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);

// `cargo test <filter>` exits 0 when the filter matches nothing, so a renamed
// test would leave this check green while proving nothing.
const passed = [...`${test.stdout}${test.stderr}`.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
  (total, match) => total + Number(match[1]),
  0,
);
if (passed === 0) fail("cargo test", `filter 'error::privacy_tests::' matched no tests\n${test.stdout}\n${test.stderr}`);

process.stdout.write("ok 2.5: Core error classification (paths kept, sources preserved)\n");
