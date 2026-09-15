#!/usr/bin/env node
// Task 12.7 check: security hardening across path traversal, symlink/reparse,
// TOCTOU, malicious metadata, asset keys and the Tauri capability/CSP surface.
//
// Acceptance (tasks 12.7, design §16):
//  - 路径穿越 / symlink/reparse escape: a candidate whose canonical target
//    leaves the root is rejected; directory symlinks are never descended;
//    publishing never follows/replaces a symlink at the target.
//  - TOCTOU 覆盖: re-enumeration re-validates canonical containment, the
//    exclusive publish reservation refuses a swapped-in replacement, and
//    discard refuses a symlink.
//  - 恶意标签/LRC: control characters and oversized hostile inputs are
//    cleaned or diagnosed, never a panic, never a partial resource.
//  - 任意 asset key: cover assets only accept opaque `cv1-<64hex>` keys;
//    covers never expose arbitrary paths.
//  - Tauri capability: no shell/fs/sql/opener/process/http permission family;
//    CSP never opens a network source; no certificate/secret proxying.
//
// Runs the echo-core security tests (walker/staging/adapter/hostile), the
// echo-desktop platform::security suite (CoverProtocol/CSP/capability) and
// the IPC capability drift test, then clippy + fmt.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 12.7: ${msg}\n`);
  process.exit(1);
}
function run(command, args, cwd = ROOT) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 12.7: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// --- Core security dimensions ---
const CORE_TESTS = [
  // path traversal / symlink / reparse
  "walker_rejects_escaping_file_symlinks",
  "walker_does_not_follow_directory_symlinks",
  "walker_toctou_swap_to_symlink_is_rejected_on_re_enumeration",
  "symlinked_staging_directory_is_never_followed",
  // exclusive publish never overwrites a raced target / symlink
  "publish_reserves_the_target_exclusively_and_never_replaces",
  "forged_staged_handles_are_rejected",
  // malicious tags / LRC
  "hostile_tags_with_controls_and_oversized_text_are_cleaned_or_diagnosed",
  "hostile_lrc_with_many_timestamps_one_line_deduplicates_and_stays_bounded",
  "lrc_empty_or_unreadable_is_a_note_not_a_blocker",
  // arbitrary asset key
  "cover_cache_rejects_bad_keys_and_arbitrary_paths",
];
for (const name of CORE_TESTS) {
  run("cargo", ["test", "-p", "echo-core", "--all-features", name]);
  process.stdout.write(`  ok: echo-core ${name}\n`);
}

// --- Desktop security surface (CoverProtocol / CSP / capability) ---
const DESKTOP_FILTERS = [
  "platform::security::tests::csp_never_opens_a_remote_or_network_source",
  "platform::security::tests::cover_uri_rejects_path_traversal_and_absolute_paths",
  "platform::security::tests::cover_key_rejects_bytes_that_can_form_a_path",
  "platform::security::tests::minimal_capability_has_no_privileged_family",
  "platform::security::tests::committed_capability_grants_only_the_minimal_set",
  "platform::security::tests::tauri_conf_csp_matches_the_pinned_policy",
];
for (const filter of DESKTOP_FILTERS) {
  run("cargo", ["test", "-p", "echo-desktop", "--all-features", filter]);
  process.stdout.write(`  ok: echo-desktop ${filter}\n`);
}

// --- Static guard: filesystem adapter discards never follow symlinks ---
const adapter = readFileSync(
  resolve(ROOT, "crates/echo-core/src/infrastructure/filesystem/adapter.rs"),
  "utf8",
);
if (!adapter.includes("file_type().is_symlink()")) {
  fail("adapter discard path no longer guards symlinks");
}

// --- clippy + fmt on the touched crates ---
run("cargo", ["clippy", "-p", "echo-core", "-p", "echo-desktop", "--all-targets", "--all-features", "--", "-D", "warnings"]);
run("cargo", ["fmt", "--all", "--", "--check"]);

process.stdout.write("ok 12.7: security tests pass (no root escape, no arbitrary read, no remote/network access)\n");