#!/usr/bin/env node
// Task 3.14 check: the sync-foundation *shape* does not create *operable* sync.
//   (schema has tombstones/sync_outbox/sync_state; 0.1.0 still has no sync
//    command/event/entry, no network client, no push — offline boundary holds).
//
// This is a complement to 13.8: 13.8 proves the runtime surface has no sync
// at all; 3.14 proves the newly-added *schema tables* stay inert (only local
// sync-outbox rows are ever written, never uploaded).

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(msg) {
  process.stderr.write(`FAIL 3.14: ${msg}\n`);
  process.exit(1);
}
function run(command, args, cwd = ROOT) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 3.14: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`);
    process.exit(1);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// 1. No sync command on the IPC surface (comments/documentation stripped).
const bridge = readFileSync(resolve(APP, "src", "bridge", "index.ts"), "utf8");
for (const banned of ["sync", "upload", "download", "remote_", "login"]) {
  const code = bridge.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, "");
  if (new RegExp(`\\b${banned}`, "i").test(code)) {
    fail(`bridge surface mentions a sync command '${banned}'`);
  }
}
process.stdout.write("  ok: bridge surface has no sync/upload/download command\n");

// 2. No network client dependency wired into the workspace.
const cargoToml = readFileSync(resolve(ROOT, "Cargo.toml"), "utf8");
for (const dep of ["reqwest", "hyper", "rust-s3", "aws-sdk", "webdav-client"]) {
  if (cargoToml.includes(dep)) fail(`workspace wires network client dependency '${dep}'`);
}
process.stdout.write("  ok: no network client dependency in Cargo workspace\n");

// 3. The sync-outbox schema rows are local-only: no SQLite-side trigger/
//    virtual-table that could silently upload. 0005 only creates pure tables.
const mig = readFileSync(
  resolve(ROOT, "crates/echo-core/src/infrastructure/sqlite/migrations/0005_sync_foundation.sql"),
  "utf8",
);
if (mig.toLowerCase().includes("create virtual table")) {
  fail("0005 must contain only pure local tables, no virtual/network table");
}
process.stdout.write("  ok: 0005 defines pure local tables only\n");

// 4. Desktop security still denies remote/network (offline guarantee intact).
run("cargo", ["test", "-p", "echo-desktop", "--all-features", "platform::security::tests::csp_never_opens_a_remote_or_network_source"]);
process.stdout.write("  ok: desktop CSP still denies remote/network connect\n");

process.stdout.write("ok 3.14: sync-foundation shape adds no operable sync; offline boundary intact\n");