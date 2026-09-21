#!/usr/bin/env node
// Task 13.8 check: runtime has no *operable* sync — no account / telemetry /
// sync command/event/entry or business network requests; offline is the only
// mode; UI shows no operable sync / select-all / manual playlist sort / song-edit
// entry.
//
// This is a static drift + runtime test check: the guarantees that make it
// true are built into the code (the 0005 migration builds sync-FOUNDATION
// tables but nothing pushes/reads them remotely, the bridge exposes no
// sync/account command, the capability grants no network family, and the UI
// has no such entries — the CES E2E A14 asserts the render).
//
// Checks:
//  1. Sync/account/telemetry tables (tombstones / sync_state / sync_outbox)
//     are only local shape: no network client, no command/event/entry pushes
//     them remotely. Task 3.10 builds them; they must stay inert.
//  2. The generated IPC command surface + bridge have no sync/account/
//     telemetry/remote command; the event surface no such event.
//  3. No network client dependency is wired into the workspace (no reqwest /
//     hyper / aws / s3 / webdav / ndk-runtime network stack).
//  4. The desktop CSP/capability security tests still pass (no network connect
//     from the webview).
//  5. UI test suite proves the scope guard: A14 (no sync / select-all / manual
//     playlist sort / song edit). We run the SongList/Playlists component tests
//     and the browser E2E already asserts the rendered surface lacks them; the
//     `pnpm test:e2e` invocation is optional here (heavy) — the component
//     tests assert the absence directly.

import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(msg) {
  process.stderr.write(`FAIL 13.8: ${msg}\n`);
  process.exit(1);
}
function run(command, args, cwd = ROOT) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 13.8: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// 1. Migrations: sync-FOUNDATION tables are allowed ONLY in 0005 (and must be
//    inert — see per-migration table scan below); 0001 stays clean; no other
//    sync/account/telemetry table may appear anywhere.
//    The synchronization tables may *mention* in comments to document their
//    deliberate role (0001) or their inert shape (0005).
const migration0001 = readFileSync(
  resolve(ROOT, "crates/echo-core/src/infrastructure/sqlite/migrations/0001_initial.sql"),
  "utf8",
);
const createStmts0001 = migration0001.replace(/--[^\n]*\n/g, "").toLowerCase();
for (const banned of ["tombstones", "sync_state", "sync_outbox", "account", "telemetry"]) {
  if (createStmts0001.includes(`create table ${banned}`)) {
    fail(`0001 migration creates a sync/account/telemetry table '${banned}'`);
  }
}
process.stdout.write("  ok: 0001 migration creates no sync/account/telemetry tables\n");

// 1b. Sync-foundation tables may appear ONLY in 0005, and must be present there
//     (the data shape is ready) while still inert (nothing pushes them).
const migration0005 = readFileSync(
  resolve(ROOT, "crates/echo-core/src/infrastructure/sqlite/migrations/0005_sync_foundation.sql"),
  "utf8",
);
const foundIn0005 = migration0005.replace(/--[^\n]*\n/g, "").toLowerCase();
for (const table of ["tombstones", "sync_state", "sync_outbox"]) {
  if (!foundIn0005.includes(`create table ${table}`)) {
    fail(`0005 migration does not create sync-foundation table '${table}'`);
  }
}
const migrationsDir = resolve(ROOT, "crates/echo-core/src/infrastructure/sqlite/migrations");
const otherMigrations = readdirSync(migrationsDir).filter(
  (f) => f.endsWith(".sql") && !f.startsWith("0005_sync_foundation") && !f.startsWith("0001_initial"),
);
for (const file of otherMigrations) {
  const body = readFileSync(resolve(migrationsDir, file), "utf8").replace(/--[^\n]*\n/g, "").toLowerCase();
  for (const banned of ["tombstones", "sync_state", "sync_outbox"]) {
    if (body.includes(`create table ${banned}`)) {
      fail(`migration ${file} creates sync-foundation table '${banned}' outside 0005`);
    }
  }
}
process.stdout.write("  ok: sync-foundation tables exist in 0005 and only there (0001 and others clean)\n");

// 2. Command/event surface: no sync/account/telemetry/remote command.
const bridge = readFileSync(resolve(APP, "src", "bridge", "index.ts"), "utf8");
for (const banned of ["sync", "account", "telemetry", "remote_", "login", "upload", "download"]) {
  // Ignore comments/documentation lines; command names are the interesting bits.
  if (new RegExp(`\\b${banned}`, "i").test(bridge.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, ""))) {
    fail(`bridge surface mentions a sync/account/telemetry command '${banned}'`);
  }
}
process.stdout.write("  ok: command/event surface exposes no sync/account/telemetry\n");

// 3. No network client deps in the workspace.
const cargoToml = readFileSync(resolve(ROOT, "Cargo.toml"), "utf8");
for (const dep of ["reqwest", "hyper", "rust-s3", "aws-sdk", "webdav-client"]) {
  if (cargoToml.includes(dep)) fail(`workspace wires network client dependency '${dep}'`);
}
process.stdout.write("  ok: no business network client dependency in Cargo workspace\n");

// 4. Desktop security suite still asserts offline/no-network.
run("cargo", ["test", "-p", "echo-desktop", "--all-features", "platform::security::tests::csp_never_opens_a_remote_or_network_source"]);
process.stdout.write("  ok: desktop CSP still denies remote/network connect\n");

// 5. UI scope guard (A14): no sync / select-all / manual playlist sort / song
//    edit entry. Two independent proofs, because either alone was how this gate
//    previously lied:
//
//    (a) Source truth — the shell must not *contain* a sync entry at all. Phase
//        one has no sync (`docs/ROADMAP.md` "不包含：资料库同步与可操作的同步
//        入口"), and neither does the prototype (grep
//        `docs/prototype/echo-desktop-player.html` for 同步/sync: zero hits). A
//        disabled control reading 同步/资料库已同步 used to be rendered and was
//        reported as "present but inert"; that fabricated a state the app cannot
//        know. Absence is the assertion now.
//    (b) Executed proof — the shell component suite must actually assert it, by
//        name, so deleting the assertion fails this gate.
const SYNC_PATTERN = /同步|\bsync\b/i;
const scannedSources = [
  ["src/app/App.tsx", readFileSync(resolve(APP, "src", "app", "App.tsx"), "utf8")],
  [
    "src/features/settings/SettingsView.tsx",
    readFileSync(resolve(APP, "src", "features", "settings", "SettingsView.tsx"), "utf8"),
  ],
  [
    "src/features/library/SongMenu.tsx",
    readFileSync(resolve(APP, "src", "features", "library", "SongMenu.tsx"), "utf8"),
  ],
  [
    "src/styles/shell.css",
    readFileSync(resolve(APP, "src", "styles", "shell.css"), "utf8"),
  ],
];
for (const [name, source] of scannedSources) {
  const withoutDocComments = source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*|\/\*)/.test(line))
    .join("\n");
  if (SYNC_PATTERN.test(withoutDocComments)) {
    fail(`${name} still renders or styles a sync entry — phase one ships no sync`);
  }
}
process.stdout.write("  ok: no sync string survives in the shell, settings, song menu or shell CSS\n");

const appTest = readFileSync(resolve(APP, "src", "app", "App.test.tsx"), "utf8");
if (!appTest.includes("renders no sync entry anywhere in the activated shell")) {
  fail("App.test.tsx lost the named 同步 scope-guard test; the UI proof below would be hollow");
}
process.stdout.write("  ok: App.test.tsx carries the named sync scope-guard assertion\n");

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
// NOTE: `"test": "vitest run"` already includes `run`. Passing an extra
// `-- --run <file>` makes vitest treat the leftover `--` as part of the filter
// set, silently dropping every filter and running the WHOLE suite — which is
// how this gate used to look green without ever proving the scope guard.
function runDesktopTest(filter) {
  const out = run("pnpm", ["--filter", "@echo/desktop", "test", filter], APP);
  // Vitest colours its reporter when `CI` is set; strip ANSI before the count
  // regex, exactly as run-scenario.mjs does, or CI misreads the summary.
  const plain = out.replace(/\u001B\[[0-9;]*[A-Za-z]/g, "");
  const files = Number((plain.match(/Test Files\s+(\d+)/) ?? [])[1] ?? 0);
  if (files !== 1) {
    fail(`'${filter}' matched ${files} test files (expected exactly 1) — the filter did not apply`);
  }
}

runDesktopTest("src/app/App.test.tsx");
runDesktopTest("src/features/library/SongList.test.tsx");
runDesktopTest("src/features/playlists/PlaylistsView.test.tsx");
process.stdout.write(
  "  ok: UI scope-guard (no sync / select-all / manual playlist sort / song edit) asserted by App + component suites\n",
);

process.stdout.write("ok 13.8: runtime is offline-only; no account/telemetry/sync/network; UI has no operable excluded entries\n");