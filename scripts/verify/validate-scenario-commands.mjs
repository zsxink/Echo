#!/usr/bin/env node
// Static validation of the scenario registry: every scenario must name the
// evidence it actually depends on, and every evidence target must exist — the
// release gate must never "pass" a scenario against zero tests.
//
// A `cargo test -- <filter>` that matches nothing exits 0, which is a silent
// false-pass. This validator obtains the compiled test lists from Cargo at
// runtime and asserts every `rust` evidence target matches a listed test of a
// package the scenario's command runs. The remaining kinds are checked
// against the filesystem:
//
//   rust        target matches at least one test in the command's package list
//   vitest      file exists; with `name`, the name appears in the file
//   node        path exists and stays inside `scripts/verify/`
//   attestation target equals the scenario id (the native matrix row itself)
//
// The command syntax is still checked: only cargo/pnpm/node/python3 shapes the
// runners understand are admitted, and node/python paths may not escape their
// pinned directories.
//
// Usage: node scripts/verify/validate-scenario-commands.mjs

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { scenarioCommands } from "./scenario-commands.mjs";

const ROOT = resolve(dirname(fileURLToPath(new URL(".", import.meta.url))), "..");
let bad = 0;
const fail = (m) => { console.error(`error: ${m}`); bad += 1; };

const testLists = new Map();
function cargoTestList(packageName) {
  if (testLists.has(packageName)) return testLists.get(packageName);
  const result = spawnSync("cargo", ["test", "-p", packageName, "--all-features", "--", "--list"], {
    cwd: ROOT,
    encoding: "utf8",
  });
  const tests = result.status === 0
    ? `${result.stdout || ""}\n${result.stderr || ""}`
      .split("\n")
      .filter((line) => /: test$/.test(line))
      .map((line) => line.replace(/: test$/, "").trim())
    : [];
  if (result.status !== 0) {
    fail(`could not list ${packageName} tests (cargo exited ${result.status}): ${result.stderr || result.stdout}`);
  }
  testLists.set(packageName, tests);
  return tests;
}

const NODE_COMMAND = /^node (scripts\/verify\/[A-Za-z0-9_./-]+\.mjs)(?:\s|$)/;
const AUDIO_COMMAND = /^python3 (scripts\/audio-effects\/(probe|inspect-filter-runtime|generate_fixtures)\.py)(?:\s|$)/;

// The command must be one the runners understand; each `&&` segment is checked
// on its own so a chained import subgate (node + cargo) stays admissible.
function checkCommand(id, command) {
  const packages = new Set();
  for (const segment of command.split(" && ")) {
    const cargo = segment.match(/^cargo test -p (echo-core|echo-desktop|echo-app) --all-features(?:\s.*)?$/);
    if (cargo) {
      packages.add(cargo[1]);
      continue;
    }
    if (/^pnpm --filter @echo\/desktop test -- --run(?:\s|$)/.test(segment)) continue;
    const node = segment.match(NODE_COMMAND);
    if (node) {
      if (node[1].includes("..")) fail(`${id}: verify script path escapes scripts/verify: ${node[1]}`);
      else if (!existsSync(resolve(ROOT, node[1]))) fail(`${id}: verify script missing ${node[1]}`);
      continue;
    }
    const audio = segment.match(AUDIO_COMMAND);
    if (audio) {
      if (audio[1].includes("..")) fail(`${id}: audio-effects path escapes scripts/audio-effects: ${audio[1]}`);
      else if (!existsSync(resolve(ROOT, audio[1]))) fail(`${id}: audio-effects tool missing ${audio[1]}`);
      continue;
    }
    fail(`${id}: unrecognized command '${segment}'`);
  }
  return [...packages];
}

function checkEvidence(id, evidence, packages) {
  if (!Array.isArray(evidence) || evidence.length === 0) {
    fail(`${id}: no evidence declared`);
    return;
  }
  for (const item of evidence) {
    if (!item || typeof item !== "object" || typeof item.target !== "string" || !item.target) {
      fail(`${id}: evidence entry without a target: ${JSON.stringify(item)}`);
      continue;
    }
    switch (item.kind) {
      case "rust": {
        if (packages.length === 0) {
          fail(`${id}: rust evidence '${item.target}' but the command runs no cargo package`);
          break;
        }
        const hits = packages.flatMap((pkg) => cargoTestList(pkg)).filter((t) => t.includes(item.target));
        if (!hits.length) fail(`${id}: rust evidence '${item.target}' matches 0 tests in ${packages.join("/")}`);
        break;
      }
      case "vitest": {
        const p = resolve(ROOT, "apps", "desktop", item.target);
        if (!existsSync(p)) {
          fail(`${id}: vitest evidence file missing ${item.target}`);
        } else if (item.name && !readFileSync(p, "utf8").includes(item.name)) {
          fail(`${id}: vitest evidence name '${item.name}' is not in ${item.target}`);
        }
        break;
      }
      case "node": {
        if (item.target.includes("..") || !item.target.startsWith("scripts/verify/")) {
          fail(`${id}: node evidence escapes scripts/verify: ${item.target}`);
        } else if (!existsSync(resolve(ROOT, item.target))) {
          fail(`${id}: node evidence missing ${item.target}`);
        }
        break;
      }
      case "attestation": {
        if (item.target !== id) fail(`${id}: attestation target must be the scenario id, got '${item.target}'`);
        break;
      }
      default:
        fail(`${id}: unknown evidence kind '${item.kind}'`);
    }
  }
}

for (const { id, command, evidence } of scenarioCommands()) {
  const packages = checkCommand(id, command);
  checkEvidence(id, evidence, packages);
}

process.stdout.write(
  `validate-scenario-commands: ${bad ? `${bad} problems` : "all scenario evidence resolves and every command is well-formed"}\n`,
);
process.exit(bad ? 1 : 0);
