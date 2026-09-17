#!/usr/bin/env node
// Task 7.3: the active rustc must match rust-toolchain.toml exactly.

import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const toolchain = readFileSync(resolve(ROOT, "rust-toolchain.toml"), "utf8").match(/^channel\s*=\s*"([^"]+)"/m)?.[1];
const rustc = spawnSync("rustc", ["--version"], { cwd: ROOT, encoding: "utf8" });
const active = rustc.stdout.trim().match(/^rustc\s+([^\s]+)/)?.[1];
if (!toolchain || rustc.status !== 0 || !active || active !== toolchain) {
  process.stderr.write(`FAIL toolchain: rust-toolchain.toml=${toolchain || "missing"}, rustc=${active || rustc.stderr.trim() || "unavailable"}\n`);
  process.exit(1);
}
process.stdout.write(`ok toolchain: rustc ${active} matches rust-toolchain.toml\n`);
