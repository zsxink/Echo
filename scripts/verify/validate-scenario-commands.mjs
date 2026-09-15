#!/usr/bin/env node
// Validate that every scenario's cargo/vitest filter actually matches a real
// test — the release gate must never "pass" a scenario against zero tests.
//
// A `cargo test -- <filter>` that matches nothing exits 0, which is a silent
// false-pass. This validator loads the compiled test lists (given by files
// holding one fully-qualified test path per line — /tmp/core-lib-tests.txt,
// /tmp/desk-lib-tests.txt) and asserts every scenario command that is a cargo
// filter matches at least one of them.
//
// Also asserts every referenced task-check script exists and every REACT vitest
// file exists.
//
// Usage: node scripts/verify/validate-scenario-commands.mjs
//   CCORE_TESTS  path to echo-core lib test list (default /tmp/core-lib-tests.txt)
//   CDESK_TESTS  path to echo-desktop lib test list (default /tmp/desk-lib-tests.txt)

import { existsSync, readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { scenarioCommands } from "./scenario-commands.mjs";

const ROOT = resolve(dirname(fileURLToPath(new URL(".", import.meta.url))), "..");
const coreTests = readFileSync(process.env.CCORE_TESTS || "/tmp/core-lib-tests.txt", "utf8").split("\n").filter(Boolean);
const deskTests = readFileSync(process.env.CDESK_TESTS || "/tmp/desk-lib-tests.txt", "utf8").split("\n").filter(Boolean);

let bad = 0;
const fail = (m) => { console.error(`error: ${m}`); bad += 1; };

for (const { id, command } of scenarioCommands()) {
  // Integration test binaries (--test) are validated separately below.
  let m = command.match(/^cargo test -p echo-desktop --all-features --test player_smoke/);
  if (m) {
    const p = resolve(ROOT, "crates", "echo-desktop", "tests", "player_smoke.rs");
    if (!existsSync(p)) fail(`${id}: player_smoke integration test missing`);
    continue;
  }
  // cargo test lib filters
  m = command.match(/^cargo test -p echo-core --all-features\s+(.+)$/);
  if (m) {
    const filter = m[1].trim();
    const hits = coreTests.filter((t) => t.includes(filter));
    if (!hits.length) fail(`${id}: cargo echo-core filter '${filter}' matches 0 tests`);
    continue;
  }
  m = command.match(/^cargo test -p echo-desktop --all-features\s+(.+)$/);
  if (m) {
    const filter = m[1].trim();
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

  // task-check scripts
  m = command.match(/^node scripts\/verify\/checks\/task-([0-9.]+)\.mjs/);
  if (m) {
    const p = resolve(ROOT, "scripts", "verify", "checks", `task-${m[1]}.mjs`);
    if (!existsSync(p)) fail(`${id}: task-check script missing ${p}`);
    continue;
  }

  // attestation checker
  m = command.match(/^node scripts\/verify\/checks\/check-native-attestation\.mjs/);
  if (m) continue;

  fail(`${id}: unrecognized command '${command}'`);
}

process.stdout.write(`validate-scenario-commands: ${bad ? `${bad} problems` : "all scenario commands match real tests / existing files"}\n`);
process.exit(bad ? 1 : 0);