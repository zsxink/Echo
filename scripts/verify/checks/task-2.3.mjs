#!/usr/bin/env node
// Task 2.3 check: Unicode NFKC/case-fold/grapheme and platform-safe name rules (property-tested).
//
// Fails unless the `domain::text` tests pass and at least one actually ran.
// Shared fmt/clippy gates live in the governance gate's quality-core segment.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 2.3: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-core", "--all-features", "domain::text::"], { cwd: ROOT, encoding: "utf8" });
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);

// `cargo test <filter>` exits 0 when the filter matches nothing, so a renamed
// test would leave this check green while proving nothing.
const passed = [...`${test.stdout}${test.stderr}`.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
  (total, match) => total + Number(match[1]),
  0,
);
if (passed === 0) fail("cargo test", `filter 'domain::text::' matched no tests\n${test.stdout}\n${test.stderr}`);

process.stdout.write("ok 2.3: Unicode NFKC/case-fold/grapheme and platform-safe name rules (property-tested)\n");
