#!/usr/bin/env node
// Task 16.1 check — the shell-side OS file-open normalization contract
// (normalize-os-file-open-paths, `safe-file-ingestion`: 操作系统文件打开载荷必须
// 先归一化为本地路径).
//
// Fails (nonzero) unless:
//   1. the `echo-app` binary compiles and is clippy-clean;
//   2. the `open_targets` pure-function unit tests pass on this host (they pin
//      the behavior: percent-decoding, non-file scheme dropping, order);
//   3. the URL→path conversion has a single owner in the shell and the
//      shell↔runtime contract is carried by the path type, so handing a URL
//      string downstream cannot compile.
//
// Note: `task-9.1.mjs` separately guards the FIFO wiring that shares this
// boundary; this check owns the normalization contract itself.

import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 16.1: ${msg}\n`);
  process.exit(1);
}

function run(cmd, args, what) {
  const result = spawnSync(cmd, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(`${what}: ${cmd} ${args.join(" ")} exited ${result.status}`);
  }
  return result.stdout;
}

// 1. The shell must compile and stay clippy-clean.
run("cargo", ["check", "-p", "echo-app", "--locked"], "echo-app compiles");
// 2. Behavior: the conversion semantics of the single owner.
const testOut = run(
  "cargo",
  ["test", "-p", "echo-app", "--locked", "open_targets"],
  "echo-app open_targets tests",
);
for (const name of [
  "decodes_percent_encoding_and_keeps_order",
  "drops_non_file_schemes",
  "mixed_input_keeps_only_file_targets_in_order",
]) {
  if (!testOut.includes(`test open_targets::tests::${name} ... ok`)) {
    fail(`open_targets test not green: ${name}`);
  }
}

// 3. Type-carried contract: one owner, path-typed all the way into the runtime.
//
// The shell was split into `bootstrap.rs` (Tauri run loop), `file_open_gate.rs`
// (readiness-gated delivery) and `main.rs` (a thin entry point), so the scan
// covers every shell source instead of one file. The gate hooks below are the
// call sites the contract is carried by, wherever they live.
const SHELL_SRC = resolve(ROOT, "apps", "desktop", "src-tauri", "src");

function readShellSources(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(dir, entry.name);
    if (entry.isDirectory()) return readShellSources(path);
    return entry.name.endsWith(".rs") ? [path] : [];
  });
}

const shellSources = readShellSources(SHELL_SRC).map((path) => ({
  path,
  text: readFileSync(path, "utf8"),
}));
const shellText = shellSources.map(({ text }) => text).join("\n");
const openTargets = readFileSync(resolve(SHELL_SRC, "open_targets.rs"), "utf8");
const runtime = readFileSync(
  resolve(ROOT, "crates", "echo-desktop", "src", "runtime", "mod.rs"),
  "utf8",
);

const shellRequired = [
  "open_targets::open_targets(&urls)",
  "fn deliver_file_open(",
  "fn record_gate_open(",
];
for (const token of shellRequired) {
  if (!shellText.includes(token)) {
    fail(`the shell sources are missing the normalization contract: '${token}'`);
  }
}
for (const { path, text } of shellSources) {
  if (text.includes("url.to_string()")) {
    fail(
      `${path.slice(ROOT.length + 1)} stringifies a URL again (url.to_string()) — payloads must go through open_targets`,
    );
  }
}
const openTargetsRequired = ["fn open_targets(urls: &[tauri::Url]) -> Vec<PathBuf>"];
for (const token of openTargetsRequired) {
  if (!openTargets.includes(token)) {
    fail(`open_targets.rs is missing the conversion owner: '${token}'`);
  }
}
const runtimeRequired = [
  "pub fn receive_file_open(&self, path: PathBuf) -> Option<PathBuf>",
  "pending_opens: PendingOpen<PathBuf>",
  "pub fn mark_frontend_ready(&self) -> Vec<PathBuf>",
];
for (const token of runtimeRequired) {
  if (!runtime.includes(token)) {
    fail(`runtime/mod.rs is missing the path-typed contract: '${token}'`);
  }
}

process.stdout.write("ok 16.1: OS file-open payloads are normalized to paths at the shell boundary\n");
