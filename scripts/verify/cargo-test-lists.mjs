#!/usr/bin/env node
// Compiled test inventories, obtained without changing the build shape.
//
// Both evidence validators need "which tests exist in package X". The obvious
// spelling — `cargo test -p <pkg> --all-targets --all-features -- --list` per
// package — silently changes the build shape: Cargo unifies features across the
// selected packages only, and `echo-desktop` enables `echo-core/testkit`, so
// `-p echo-core` compiles a *different* `echo_core` artifact than the workspace
// build does (observed hashes e2a9bc… vs 7613cc…). In CI that cost ~5 minutes of
// recompilation on every run, and it left `target/` holding the per-package
// shape — the very drift the engineering-governance spec forbids for any cargo
// call made before the cache is saved.
//
// This module instead builds the workspace once in exactly the shape the Test
// step uses (`cargo test --workspace --all-targets --all-features --no-run`),
// reads the executable-to-package mapping from Cargo's own JSON messages, and
// then asks each already-built binary for its test list. The binaries are warm
// whenever the Test step ran first, so the whole inventory is near-free, and no
// cargo invocation ever departs from the workspace shape.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

const WORKSPACE_SHAPE = ["test", "--workspace", "--all-targets", "--all-features"];

/** Package name declared by the manifest at `manifestPath`. */
function packageName(manifestPath, cache) {
  if (cache.has(manifestPath)) return cache.get(manifestPath);
  let name = null;
  try {
    const match = readFileSync(manifestPath, "utf8").match(/^\s*name\s*=\s*"([^"]+)"/m);
    name = match ? match[1] : null;
  } catch {
    name = null;
  }
  cache.set(manifestPath, name);
  return name;
}

/**
 * Map of package name -> test names, for every test target in the workspace.
 * Returns `{ tests, error }`; `error` is a human-readable message when the
 * workspace build or a listing call failed, so callers can fail loudly instead
 * of silently validating against an empty set.
 */
export function workspaceTestLists({ cwd, maxBuffer = 128 * 1024 * 1024 } = {}) {
  const build = spawnSync("cargo", [...WORKSPACE_SHAPE, "--no-run", "--message-format=json"], {
    cwd,
    encoding: "utf8",
    maxBuffer,
  });
  if (build.status !== 0) {
    return {
      tests: new Map(),
      error: `cargo ${WORKSPACE_SHAPE.join(" ")} --no-run exited ${build.status}: ${(build.stderr || build.stdout || "").slice(-2000)}`,
    };
  }

  const names = new Map();
  const executables = new Map();
  for (const line of String(build.stdout || "").split("\n")) {
    if (!line.trim()) continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      continue;
    }
    if (message.reason !== "compiler-artifact" || !message.executable) continue;
    // `--all-targets` builds each binary twice: once as the real program
    // (`profile.test === false`) and once as its test harness. Running
    // `--list` on the real program would launch it (the Tauri shell opens a
    // window); only the test harness answers `--list`.
    if (message.profile?.test !== true) continue;
    const pkg = packageName(message.manifest_path, names);
    if (!pkg) continue;
    if (!executables.has(pkg)) executables.set(pkg, []);
    executables.get(pkg).push(message.executable);
  }

  const tests = new Map();
  for (const [pkg, bins] of executables) {
    const collected = [];
    for (const bin of bins) {
      const listed = spawnSync(bin, ["--list"], { cwd, encoding: "utf8", maxBuffer });
      if (listed.status !== 0) {
        return {
          tests: new Map(),
          error: `listing '${bin}' exited ${listed.status}: ${(listed.stderr || listed.stdout || "").slice(-1000)}`,
        };
      }
      for (const line of String(listed.stdout || "").split("\n")) {
        if (/: test$/.test(line)) collected.push(line.replace(/: test$/, "").trim());
      }
    }
    tests.set(pkg, collected);
  }

  if (tests.size === 0) {
    return { tests, error: "cargo reported no test executables for the workspace" };
  }
  return { tests, error: null };
}
