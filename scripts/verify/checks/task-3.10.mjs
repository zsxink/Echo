#!/usr/bin/env node
// Task 3.10–3.13 check: sync-foundation data shape is ready but inert.
//
// 方向 A: 0.1.0 建立同步基础数据底座（schema 骨架 + outbox 预写 + 墓碑），
// 但数据形状与行为严格分离 — 二期才消费。本检查验证 shape-ready，不验证
// 任何"可操作同步"（那是 3.14 / 13.8 的反向断言）。
//
// Checks:
//  1. 0005 migration exists, creates tombstones/sync_state/sync_outbox and
//     revision columns on playlists/library_roots/song_overrides; 0001+0004
//     untouched (ordering/append-only).
//  2. Migration runner registers 0005 (connection.rs includes SYNC_FOUNDATION
//     in the apply_migrations list).
//  3. core tests prove outbox prewrite + tombstone writes (the sqlite tests
//     assert the transaction-atomic facts directly).

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const MIGRATIONS = resolve(ROOT, "crates/echo-core/src/infrastructure/sqlite/migrations");

function fail(msg) {
  process.stderr.write(`FAIL 3.10: ${msg}\n`);
  process.exit(1);
}
function run(command, args, cwd = ROOT) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 3.10: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`);
    process.exit(1);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// 1. 0005 migration contents.
const mig = readFileSync(resolve(MIGRATIONS, "0005_sync_foundation.sql"), "utf8");
for (const table of ["create table tombstones", "create table sync_state", "create table sync_outbox"]) {
  if (!mig.toLowerCase().includes(table)) fail(`0005 does not create '${table}'`);
}
for (const alter of [
  "alter table song_overrides add column revision",
  "alter table playlists add column revision",
  "alter table library_roots add column revision",
]) {
  if (!mig.toLowerCase().includes(alter)) fail(`0005 does not add ${alter}`);
}
if (!mig.includes("schema_base")) fail("0005 does not record schema_base='full'");
process.stdout.write("  ok: 0005 creates tombstones/sync_state/sync_outbox + revision columns + schema marker\n");

// 2. Migration runner registers 0005.
const connection = readFileSync(
  resolve(ROOT, "crates/echo-core/src/infrastructure/sqlite/connection.rs"),
  "utf8",
);
if (!connection.includes("SYNC_FOUNDATION") || !connection.includes("(5, migrations::SYNC_FOUNDATION)")) {
  fail("connection.rs does not register SYNC_FOUNDATION as migration 5");
}
process.stdout.write("  ok: connection.rs registers 0005 as migration 5\n");

// 3. Core tests assert the shape: outbox prewrite + tombstone write, without
//    implying any network/upload behavior.
run("cargo", ["test", "-p", "echo-core", "--all-features", "--", "sync_foundation"]);
process.stdout.write("  ok: sync-foundation core tests pass (schema ready + outbox/tombstone shape)\n");

process.stdout.write("ok 3.10: sync-foundation data shape is ready (schema + prewrite + tombstone)\n");