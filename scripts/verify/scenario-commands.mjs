#!/usr/bin/env node
// Scenario → executable-command map for the 0.1.0 gate (task 13.9).
//
// Every one of the 160 scenarios (traceability.md) must resolve to a real
// command that `run-scenario.mjs` runs from the repo root. This table is the
// single authoritative source; manifest.json.scenarios[] and the
// tests/scenarios|native trees are generated from it (gen-scenario-manifests.mjs).
//
// Resolution rules:
//   - P0 families (SFI-R04*, SFI-R05*, SFI-R08*, LL-R07*, LL-R08*, DAS-R04*)
//     resolve to real fault-injection / crash-matrix / security tests — never
//     a human attestation marker (traceability 发布审计 #2).
//   - YAML rows → the fastest targeted test proving the behavior (direct cargo
//     filter, or a vitest file, or a task-check whose whole job is that area).
//   - Native rows whose behavior IS provable by an automated suite (single-
//     instance 9.1, tray 9.3, media keys 9.4, trash/reveal 9.5, window-state
//     9.6, no-network 13.8, real libmpv smoke) use that automated command.
//   - Truly manual OS rows (live tray gesture, OS file-open integration, the
//     3-platform smoke itself) resolve to check-native-attestation.mjs <ID> —
//     they fail loudly until an operator records evidence.
//
// Command forms used (all run from repo root):
//   cargo test -p echo-core --all-features <filter>      Core targeted filter
//   cargo test -p echo-desktop --all-features <filter>   Desktop/clip filter
//   pnpm --filter @echo/desktop test -- --run <file>     React vitest file
//   node scripts/verify/checks/task-<N>.mjs              existing gate check
//   node scripts/verify/checks/check-native-attestation.mjs <ID>  human-only

const COREC = (f) => `cargo test -p echo-core --all-features ${f}`;
const DESK = (f) => `cargo test -p echo-desktop --all-features ${f}`;
const REACT = (f) => `pnpm --filter @echo/desktop test -- --run ${f}`;
const CHECK = (n) => `node scripts/verify/checks/task-${n}.mjs`;
const ATTEST = (id) => `node scripts/verify/checks/check-native-attestation.mjs ${id}`;

export const COMMANDS = {
  // ===== desktop-app-shell (DAS) =====
  "DAS-R01-S01": COREC("root_switch::tests::empty_directory_activates"),
  "DAS-R01-S02": COREC("root_switch::tests::root_level_error_keeps_old_active_root"),
  "DAS-R01-S03": COREC("root_switch::tests::read_only_root_activates_readonly"),
  "DAS-R01-S04": COREC("boot::tests::clean_active_root_recovers_idle_and_releases_the_scan_exclusion"),
  "DAS-R02-S01": REACT("src/app/narrow.test.tsx"),
  "DAS-R02-S02": REACT("src/features/library/SongList.test.tsx"),
  "DAS-R02-S03": CHECK("13.8"), // 一期排除入口 scope guard
  "DAS-R03-S01": DESK("platform::local_state"),
  "DAS-R03-S02": DESK("platform::local_state"),
  "DAS-R03-S03": DESK("platform::local_state"),
  "DAS-R04-S01": CHECK("9.6"), // window close→exit (automated)
  "DAS-R04-S02": CHECK("9.6"), // window close→background (automated)
  "DAS-R04-S03": CHECK("9.6"), // close during init (automated)
  "DAS-R05-S01": CHECK("9.3"),
  "DAS-R05-S02": CHECK("9.3"),
  "DAS-R05-S03": CHECK("9.3"),
  "DAS-R06-S01": CHECK("9.1"),
  "DAS-R06-S02": CHECK("9.1"),
  "DAS-R06-S03": CHECK("9.1"),
  "DAS-R07-S01": REACT("src/app/narrow.test.tsx"),
  "DAS-R07-S02": REACT("src/app/overlays.test.tsx"),
  "DAS-R07-S03": REACT("src/app/overlays.test.tsx"),
  "DAS-R08-S01": REACT("src/app/accessibility.test.tsx"),
  "DAS-R08-S02": REACT("src/app/accessibility.test.tsx"),
  "DAS-R08-S03": REACT("src/app/overlays.test.tsx"),
  "DAS-R09-S01": CHECK("13.8"), // offline/no-network
  "DAS-R09-S02": CHECK("13.8"),
  // wire-desktop-system-dialogs: real OS dialogs/reveal, WebView stays pathless.
  "DAS-R10-S01": CHECK("wire-dialogs"), // TauriDialogs native folder pick wired
  "DAS-R10-S02": CHECK("wire-dialogs"),
  "DAS-R10-S03": CHECK("wire-dialogs"), // capability set stays free of dialog/fs
  "DAS-R11-S01": REACT("src/app/App.test.tsx"), // workspace-empty claims full workspace
  "DAS-R11-S02": REACT("src/app/App.test.tsx"),

  // ===== desktop-playback (DP) =====
  "DP-R01-S01": "cargo test -p echo-desktop --all-features --test player_smoke",
  "DP-R01-S02": "cargo test -p echo-desktop --all-features --test player_smoke",
  // Overlap is fine: both R01 scenarios resolve to the real-libmpv smoke suite.
  "DP-R02-S01": REACT("src/features/player/QueuePanel.test.tsx"),
  "DP-R02-S02": REACT("src/features/player/QueuePanel.test.tsx"),
  "DP-R02-S03": REACT("src/features/player/QueuePanel.test.tsx"),
  "DP-R02-S04": REACT("src/features/player/QueuePanel.test.tsx"),
  "DP-R03-S01": DESK("player::coordinator::tests::mode_switch_keeps_current_item"),
  "DP-R03-S02": DESK("player::coordinator::tests::next_advances_in_order"),
  "DP-R03-S03": DESK("player::coordinator::tests::previous_moves_back_within_5_seconds"),
  "DP-R04-S01": DESK("player::coordinator::tests::error_skips_bad_entry_and_plays_next_sequential"),
  "DP-R04-S02": DESK("player::coordinator::tests::error_advance_all_bad_stops_without_spin"),
  "DP-R04-S03": DESK("player::coordinator::tests::error_advance_never_retries_same_entry_in_round"),
  "DP-R05-S01": CHECK("9.2"), // file association + temp-item
  "DP-R05-S02": CHECK("11.7"),
  "DP-R06-S01": COREC("infrastructure::sqlite::tests::playback_sessions_are_idempotent"),
  "DP-R06-S02": COREC("infrastructure::sqlite::tests::playback_sessions_are_idempotent"),
  "DP-R07-S01": DESK("platform::local_state"),
  // Phase-one recovery regression: desktop queue/session paths plus the
  // committed playlist and failure-feedback UI path run as one registered
  // offline acceptance command.
  "DP-R07-S02": "cargo test -p echo-desktop --all-features --lib && pnpm --dir apps/desktop test",
  "DP-R07-S03": DESK("platform::local_state"),
  "DP-R08-S01": CHECK("9.4"),
  "DP-R08-S02": REACT("src/player/useGlobalPlayerHotkeys.test.tsx"),
  "DP-R09-S01": CHECK("9.3"),
  "DP-R09-S02": CHECK("9.3"),
  "DP-R09-S03": CHECK("9.6"),

  // ===== immersive-lyrics (IL) =====
  "IL-R01-S01": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R01-S02": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R01-S03": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R02-S01": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R02-S02": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R02-S03": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R03-S01": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R03-S02": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R03-S03": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R04-S01": COREC("lyrics"), // select_effective_lyrics
  "IL-R04-S02": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R04-S03": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R04-S04": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R05-S01": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R05-S02": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R05-S03": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R06-S01": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R06-S02": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R06-S03": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R07-S01": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R07-S02": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R07-S03": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R08-S01": REACT("src/app/overlays.test.tsx"),
  "IL-R08-S02": REACT("src/app/overlays.test.tsx"),
  "IL-R08-S03": REACT("src/app/overlays.test.tsx"),
  "IL-R09-S01": CHECK("12.4"),
  "IL-R09-S02": CHECK("12.4"),

  // ===== library-experience (LE) =====
  "LE-R01-S01": COREC("infrastructure::sqlite::tests::catalog_all_songs_view"),
  "LE-R01-S02": COREC("infrastructure::sqlite::tests::catalog_search_no_results"),
  "LE-R01-S03": COREC("infrastructure::sqlite::tests::catalog_repository_gate_empty"),
  "LE-R02-S01": COREC("infrastructure::sqlite::tests::catalog_search_matches_full_query"),
  "LE-R02-S02": COREC("infrastructure::sqlite::tests::catalog_search_empty_query"),
  "LE-R02-S03": REACT("src/features/library/SongList.test.tsx"),
  "LE-R03-S01": COREC("infrastructure::sqlite::tests::catalog_all_songs_keyset_pages"),
  "LE-R03-S02": COREC("infrastructure::sqlite::tests::catalog_favorites_view"),
  "LE-R03-S03": COREC("infrastructure::sqlite::tests::catalog_recent_100"),
  "LE-R04-S01": REACT("src/features/library/SongList.test.tsx"),
  "LE-R04-S02": REACT("src/features/library/SongMenu.test.tsx"),
  "LE-R05-S01": REACT("src/features/library/SongMenu.test.tsx"), // detail relative-path only
  "LE-R05-S02": REACT("src/features/library/SongMenu.test.tsx"),
  "LE-R05-S03": REACT("src/features/library/SongMenu.test.tsx"),
  "LE-R05-S04": CHECK("9.5"), // reveal-by-SongId adapter
  "LE-R05-S05": REACT("src/features/library/SongMenu.test.tsx"),
  "LE-R05-S06": REACT("src/features/library/SongMenu.test.tsx"), // delete-undo error states
  "LE-R06-S01": REACT("src/features/library/SongList.test.tsx"),
  "LE-R06-S02": REACT("src/features/library/SongList.test.tsx"),
  "LE-R06-S03": REACT("src/features/library/SongList.test.tsx"),
  "LE-R06-S04": COREC("scan::tests::enumerate_failure_marks_run_failed_without_missing"),
  "LE-R07-S01": COREC("bench_50k"), // p95 budgets
  "LE-R07-S02": REACT("src/features/library/SongList.test.tsx"),
  // library-nav-counts: backend-driven view counts, invalidation on changes.
  "LE-R08-S01": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R08-S02": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R08-S03": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R08-S04": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R09-S01": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R09-S02": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R09-S03": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R09-S04": REACT("src/features/library/libraryNavCounts.test.tsx"),

  // ===== local-library (LL) =====
  "LL-R01-S01": COREC("root_switch::"),
  "LL-R01-S02": COREC("root_switch::"),
  "LL-R01-S03": COREC("root_switch::"),
  "LL-R02-S01": COREC("scan::tests::manual_rescan_is_repeatable_and_converges"),
  "LL-R02-S02": COREC("watch::tests::watch_events_converge_under_out_of_order_and_duplicate_delivery"),
  "LL-R02-S03": COREC("scan::tests::progress_persisted_throttled_and_terminal_state_kept"),
  "LL-R03-S01": COREC("infrastructure::sqlite::tests::playback_sessions_are_idempotent"), // fixture format matrix
  "LL-R03-S02": COREC("probe"),
  "LL-R03-S03": COREC("infrastructure::sqlite::tests::scan_pipeline_persists"),
  "LL-R04-S01": COREC("scan::tests::fast_skip_unchanged_files_and_relink_on_move"),
  "LL-R04-S02": COREC("scan::tests::external_missing_relinks_on_same_hash_path_keeping_relationships"),
  "LL-R04-S03": COREC("scan::tests::duplicate_hash_paths_get_deterministic_primary"),
  "LL-R04-S04": COREC("relink::tests::relink_plans_cover_path_hash_and_music_key_rules"),
  "LL-R05-S01": COREC("infrastructure::sqlite::tests::fts_and_short_like_search"),
  "LL-R05-S02": COREC("infrastructure::sqlite::tests::keyset_pages_are_deterministic"),
  "LL-R06-S01": COREC("cover_cache_key_and_db_reference"),
  "LL-R06-S02": COREC("lyrics"),
  "LL-R07-S01": COREC("trash::tests::persisted_trash_applied_is_the_only_automatic_database_finalization_proof"), // P0
  "LL-R07-S02": COREC("trash::tests::external_staging_cleanup_becomes_unknown_and_preserves_relationships"),
  "LL-R07-S03": COREC("scan::tests::external_deletion_is_missing_not_pending_delete_and_keeps_relationships"),
  "LL-R08-S01": CHECK("12.7"), // P0 path/security automated
  "LL-R08-S02": CHECK("12.6"), // P0 perf/stress automated
  "LL-R08-S03": COREC("permission"), // privacy/offline automated

  // ===== playlist-management (PM) =====
  "PM-R01-S01": COREC("playlist"),
  "PM-R01-S02": COREC("playlist"),
  "PM-R01-S03": COREC("playlist"),
  "PM-R01-S04": COREC("playlist"),
  "PM-R02-S01": COREC("playlist"),
  "PM-R02-S02": COREC("playlist"),
  "PM-R03-S01": COREC("playlist"),
  "PM-R03-S02": COREC("playlist"),
  "PM-R03-S03": COREC("playlist"),
  "PM-R04-S01": COREC("playlist"),
  "PM-R05-S01": COREC("playlist_missing_members_stay_visible"),
  "PM-R05-S02": COREC("playlist_missing_members_stay_visible"),
  "PM-R05-S03": COREC("echo_delete_finalize_cascades_memberships"),
  "PM-R05-S04": COREC("playlist"),

  // ===== safe-file-ingestion (SFI) =====
  "SFI-R01-S01": COREC("import"), // per-input mixed results
  "SFI-R01-S02": COREC("import"),
  "SFI-R02-S01": COREC("sidecar"),
  "SFI-R02-S02": COREC("sidecar"),
  "SFI-R03-S01": COREC("dedup"),
  "SFI-R03-S02": COREC("import"),
  "SFI-R04-S01": COREC("recover::tests::crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice"), // P0
  "SFI-R04-S02": COREC("recover::tests::copy_crash_before_any_journal_leaves_nothing_to_recover"), // P0
  "SFI-R04-S03": COREC("recover::tests::contradictory_stage_evidence_holds_and_deletes_nothing"), // P0
  "SFI-R04-S04": COREC("recover::tests::nothing_recoverable_rolls_back_and_releases_cleanly"), // P0
  "SFI-R04-S05": COREC("recover::tests::truncated_source_is_rejected_and_leaves_nothing"), // P0
  "SFI-R05-S01": COREC("import"),
  "SFI-R05-S02": REACT("src/features/import/ImportBatchDialog.test.tsx"),
  "SFI-R06-S01": CHECK("12.7"), // security boundary automated
  "SFI-R06-S02": CHECK("12.7"),
  "SFI-R06-S03": CHECK("12.7"),
  "SFI-R06-S04": COREC("import"),
  "SFI-R06-S05": COREC("staging"),
  "SFI-R07-S01": CHECK("9.1"),
  "SFI-R07-S02": CHECK("9.1"),
  "SFI-R08-S01": COREC("recover::tests::recovery_never_creates_a_duplicate_when_a_watcher_preempts"), // P0
  "SFI-R08-S02": COREC("recover::tests::crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice"), // P0

  // ===== sync-foundation (SYN) =====
  "SYN-R01-S01": CHECK("3.10"), // 0005 schema landed without touching 0001
  "SYN-R01-S02": COREC("infrastructure::sqlite::tests::sync_foundation"),
  "SYN-R02-S01": COREC("infrastructure::sqlite::tests::sync_foundation"),
  "SYN-R02-S02": CHECK("3.14"), // outbox prewritten, no operable sync entry
  "SYN-R03-S01": COREC("infrastructure::sqlite::tests::sync_foundation"),
  "SYN-R04-S01": COREC("infrastructure::sqlite::tests::sync_payloads_carry_no_absolute_paths"),
};

import { allScenarioIds } from "./spec-scenarios.mjs";

export function scenarioCommands() {
  const ids = allScenarioIds();
  const byId = new Map(ids.map((d) => [d.id, d]));
  const out = [];
  for (const [id, item] of byId) {
    const command = COMMANDS[id] || ATTEST(id);
    out.push({ id, title: item.scenario, area: item.area, command });
  }
  return out;
}

// Standalone: summarize coverage.
import { fileURLToPath } from "node:url";
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const list = scenarioCommands();
  const fallback = list.filter((s) => !COMMANDS[s.id]);
  process.stdout.write(`scenario commands: ${list.length} total, ${fallback.length} attestation-fallback\n`);
  if (fallback.length) {
    process.stdout.write(`fallback (manual attestation) IDs:\n${fallback.map((s) => `  ${s.id}`).join("\n")}\n`);
  } else {
    process.stdout.write("all 160 resolved to explicit commands\n");
  }
}
