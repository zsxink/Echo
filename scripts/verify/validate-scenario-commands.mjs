#!/usr/bin/env node
// Validate that every scenario's cargo/vitest filter actually matches a real
// test — the release gate must never "pass" a scenario against zero tests.
//
// A `cargo test -- <filter>` that matches nothing exits 0, which is a silent
// false-pass. This validator obtains the compiled test lists from Cargo at
// runtime and asserts every scenario command that is a cargo filter matches at
// least one of them. It deliberately has no dependency on caller-created
// files outside the repository.
//
// Also asserts every referenced task-check script exists and every REACT vitest
// file exists.
//
// Usage: node scripts/verify/validate-scenario-commands.mjs

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { scenarioCommands } from "./scenario-commands.mjs";

const ROOT = resolve(dirname(fileURLToPath(new URL(".", import.meta.url))), "..");
let bad = 0;
const fail = (m) => { console.error(`error: ${m}`); bad += 1; };

function cargoTestList(packageName) {
  const result = spawnSync("cargo", ["test", "-p", packageName, "--all-features", "--", "--list"], {
    cwd: ROOT,
    encoding: "utf8",
  });
  if (result.status !== 0) {
    fail(`could not list ${packageName} tests (cargo exited ${result.status}): ${result.stderr || result.stdout}`);
    return [];
  }
  return `${result.stdout || ""}\n${result.stderr || ""}`
    .split("\n")
    .filter((line) => /: test$/.test(line))
    .map((line) => line.replace(/: test$/, "").trim());
}

const coreTests = cargoTestList("echo-core");
const deskTests = cargoTestList("echo-desktop");

for (const { id, command } of scenarioCommands()) {
  // Integration test binaries (--test) are validated separately below.
  let m = command.match(/^cargo test -p echo-desktop --all-features --test player_smoke/);
  if (m) {
    const p = resolve(ROOT, "crates", "echo-desktop", "tests", "player_smoke.rs");
    if (!existsSync(p)) fail(`${id}: player_smoke integration test missing`);
    continue;
  }
  // cargo test lib filters
  //
  // A trailing `-- <args>` is cargo's separator for the test binary (e.g.
  // `-- --ignored` to also run `#[ignore]`d tests), NOT part of the filter.
  // Without stripping it the whole tail is compared as one filter string and
  // matches nothing — which reports a correct command as broken, and would push
  // someone to remove `-- --ignored` and reintroduce the "runs zero tests"
  // false-pass this validator exists to prevent.
  const cargoFilter = (raw) => raw.trim().replace(/\s+--\s+.*$/, "");
  m = command.match(/^cargo test -p echo-core --all-features\s+([^\s&][^&]*?)(?:\s+&&|$)/);
  if (m) {
    const filter = cargoFilter(m[1]);
    const hits = coreTests.filter((t) => t.includes(filter));
    if (!hits.length) fail(`${id}: cargo echo-core filter '${filter}' matches 0 tests`);
    continue;
  }
  m = command.match(/^cargo test -p echo-desktop --all-features\s+([^\s&][^&]*?)(?:\s+&&|$)/);
  if (m) {
    const filter = cargoFilter(m[1]);
    // Cargo's `--lib` / `--test` selectors do not name one test. They still
    // execute a real target, so the runtime runner supplies the final count.
    if (filter.startsWith("-")) continue;
    const hits = deskTests.filter((t) => t.includes(filter));
    if (!hits.length) fail(`${id}: cargo echo-desktop filter '${filter}' matches 0 tests`);
    continue;
  }

  // vitest file references
  m = command.match(/^pnpm --filter @echo\/desktop test -- --run\s+(.+)$/);
  if (m) {
    const file = m[1].trim();
    const p = resolve(ROOT, "apps", "desktop", file);
    if (!existsSync(p)) fail(`${id}: vitest file missing ${file}`);
    continue;
  }

  // verify scripts — the per-task checks under `checks/` plus top-level
  // aggregate gates (the phase-one acceptance scenario points at the governance
  // gate). The regex pins the path to `scripts/verify/` and a `..` segment is
  // rejected, so widening this cannot be used to reference a file elsewhere.
  m = command.match(/^node (scripts\/verify\/[A-Za-z0-9_./-]+\.mjs)(?:\s|$)/);
  if (m) {
    const p = resolve(ROOT, m[1]);
    if (m[1].includes("..")) fail(`${id}: verify script path escapes scripts/verify: ${m[1]}`);
    else if (!existsSync(p)) fail(`${id}: verify script missing ${p}`);
    continue;
  }

  // attestation checker
  m = command.match(/^node scripts\/verify\/checks\/check-native-attestation\.mjs/);
  if (m) continue;

  fail(`${id}: unrecognized command '${command}'`);
}

process.stdout.write(`validate-scenario-commands: ${bad ? `${bad} problems` : "all scenario commands match real tests / existing files"}\n`);
process.exit(bad ? 1 : 0);
