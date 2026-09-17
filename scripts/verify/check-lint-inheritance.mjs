#!/usr/bin/env node
// Reject crate-level lint policies so every workspace crate receives the
// workspace's complete Rust and Clippy configuration.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const defaultRoot = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const ROOT = resolve(process.env.ECHO_LINT_CHECK_ROOT || defaultRoot);
const workspaceToml = readFileSync(resolve(ROOT, "Cargo.toml"), "utf8");

function fail(message) {
  process.stderr.write(`FAIL lint inheritance: ${message}\n`);
  process.exitCode = 1;
}

function workspaceMembers(toml) {
  const members = toml.match(/^members\s*=\s*\[([\s\S]*?)\]/m);
  if (!members) throw new Error("workspace Cargo.toml has no members array");
  return [...members[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

for (const member of workspaceMembers(workspaceToml)) {
  const cargoToml = resolve(ROOT, member, "Cargo.toml");
  const source = readFileSync(cargoToml, "utf8");
  const lintHeader = source.match(/^\[lints\]\s*$/m);
  if (!lintHeader || lintHeader.index === undefined) {
    fail(`${member} has no [lints] section; add 'workspace = true'`);
    continue;
  }
  const afterHeader = source.slice(lintHeader.index + lintHeader[0].length);
  const nextSection = afterHeader.search(/^\[/m);
  const lintBlock = nextSection === -1 ? afterHeader : afterHeader.slice(0, nextSection);

  const lines = lintBlock
    .split("\n")
    .map((line) => line.replace(/#.*/, "").trim())
    .filter(Boolean);
  if (!lines.includes("workspace = true")) {
    fail(`${member} does not inherit workspace lints`);
  }
  const overrides = lines.filter((line) => line !== "workspace = true");
  if (overrides.length) {
    fail(`${member} declares crate-level lint overrides: ${overrides.join(", ")}`);
  }
  const scopedOverrides = [...source.matchAll(/^\[lints\.([^\]]+)\]/gm)].map((match) => match[1]);
  if (scopedOverrides.length) {
    fail(`${member} declares crate-level lint override sections: ${scopedOverrides.join(", ")}`);
  }
}

if (process.exitCode) process.exit(process.exitCode);
process.stdout.write("ok lint inheritance: every workspace crate uses only lints.workspace = true\n");
