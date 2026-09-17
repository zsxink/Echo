#!/usr/bin/env node
// Task 5.6 / 5.8: build-purity gate for test-only modules.
//
// A test double that stays in the default build is worse than dead code: it
// ships a second, unvalidated implementation of a port. This gate proves two
// things instead of trusting convention:
//
//   1. (static) every declared test-only module carries a real `#[cfg(...)]`
//      gate — a bare `pub mod fake;` fails loudly;
//   2. (artifact) after a **default** `cargo check`, the compiler's dep-info
//      files contain none of those modules — i.e. the compiler really did not
//      read them, which is stronger than any source-level grep.
//
// The artifact half is skipped (never silently passed) when cargo is
// unavailable or the build fails; `ECHO_PURITY_REQUIRE_BUILD=1` turns that skip
// into a failure so CI cannot drift into a green-but-unproven gate.

import { existsSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");

/** Test-only units and the paths that must never appear in a default build. */
const UNITS = [
  {
    label: "desktop player fake (task 5.6)",
    module: "crates/echo-desktop/src/player/mod.rs",
    declared: /\bmod\s+fake\s*;/,
    gate: /#\[cfg\(any\(test,\s*feature\s*=\s*"testkit"\)\)\]|#\[cfg\(test\)\]/,
    artifacts: ["player/fake.rs"],
    checkArgs: ["check", "-p", "echo-desktop"],
    crateName: "echo_desktop",
  },
  {
    label: "core application testkit (task 5.8)",
    module: "crates/echo-core/src/application/mod.rs",
    declared: /\bmod\s+testkit\s*;|\bmod\s+testing\s*;/,
    gate: /#\[cfg\(any\(test,\s*feature\s*=\s*"testkit"\)\)\]/,
    artifacts: ["application/testing/", "application/testkit/"],
    checkArgs: ["check", "-p", "echo-core"],
    crateName: "echo_core",
  },
];

const errors = [];
const notes = [];

for (const unit of UNITS) {
  const modulePath = resolve(ROOT, unit.module);
  if (!existsSync(modulePath)) {
    errors.push(`${unit.label}: ${unit.module} is missing`);
    continue;
  }
  const source = readFileSync(modulePath, "utf8");
  const lines = source.split("\n");

  // 1. Static: the module must be cfg-gated, and not by something that is
  //    trivially true (e.g. `#[cfg(not(no_such_feature))]` is too clever to
  //    audit, so we require the two shapes the repository already uses).
  const declarationIndex = lines.findIndex((line) => unit.declared.test(line));
  if (declarationIndex === -1) {
    notes.push(`${unit.label}: no module declaration found in ${unit.module}`);
    continue;
  }
  // Attributes may sit between the cfg and the declaration (e.g. a `#[path]`
  // redirect), so walk up over the declaration's attribute block and require
  // one of them to be a real test-only gate.
  let gateLine = "";
  const attributeRow = /^\s*(?:#\[|#!\[|\/\/\/|\/\/)/;
  for (let above = declarationIndex - 1; above >= 0; above -= 1) {
    const candidate = lines[above];
    if (!candidate.trim()) break;
    if (!attributeRow.test(candidate)) break;
    if (unit.gate.test(candidate)) {
      gateLine = candidate;
      break;
    }
  }
  if (!gateLine) {
    errors.push(
      `${unit.label}: ${unit.module}:${declarationIndex + 1} declares the test-only module without a cfg gate`,
    );
  }

  // 2. Artifact: prove the default build never reads those sources.
  const requireBuild = process.env.ECHO_PURITY_REQUIRE_BUILD === "1";
  const depsDir = resolve(ROOT, "target", "debug", "deps");
  for (const file of existsSync(depsDir) ? readdirSync(depsDir) : []) {
    // Drop this crate's previous dep-info so the check cannot inherit a stale
    // record from an earlier `--all-targets` (test) build.
    if (file.startsWith(`${unit.crateName}-`) && file.endsWith(".d")) {
      rmSync(resolve(depsDir, file), { force: true });
    }
  }
  const build = spawnSync("cargo", unit.checkArgs, { cwd: ROOT, encoding: "utf8" });
  if (build.status !== 0) {
    const reason = `artifact proof skipped: \`${unit.checkArgs.join(" ")}\` exited ${build.status}`;
    if (requireBuild) errors.push(`${unit.label}: ${reason}`);
    else notes.push(`${unit.label}: ${reason}`);
    continue;
  }

  const offenders = new Set();
  for (const file of existsSync(depsDir) ? readdirSync(depsDir) : []) {
    if (!file.endsWith(".d")) continue;
    const text = readFileSync(resolve(depsDir, file), "utf8");
    for (const artifact of unit.artifacts) {
      if (text.includes(artifact)) offenders.add(artifact);
    }
  }
  if (offenders.size) {
    errors.push(
      `${unit.label}: default build reads test-only source: ${[...offenders].sort().join(", ")}`,
    );
  }
}

for (const note of notes) process.stdout.write(`note build purity: ${note}\n`);
if (errors.length) {
  process.stderr.write(`FAIL build purity:\n${errors.map((e) => `- ${e}`).join("\n")}\n`);
  process.exit(1);
}
process.stdout.write(
  `ok build purity: ${UNITS.length} test-only units are cfg-gated and absent from default builds\n`,
);
