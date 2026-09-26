// Scenario → executable-command map for the 0.1.0 gate (task 13.9).
//
// 390 scenarios (spec-derived id set) × executable command. This
// table is the single authoritative source; manifest.json.scenarios[] and the
// tests/scenarios|native trees are generated from it (gen-scenario-manifests.mjs).
// Regenerated on top of the pre-alignment map so every id follows the
// authoritative spec numbering; renamed rows carry their previous value, new
// scenarios are wired to their test case (or attestation where only manual
// verification exists).

const COREC = (f) => `cargo test -p echo-core --all-features ${f}`;
const DESK = (f) => `cargo test -p echo-desktop --all-features ${f}`;
const REACT = (f) => `pnpm --filter @echo/desktop test -- --run ${f}`;
const REACT_T = (f, name) => `pnpm --filter @echo/desktop test -- --run ${f} -t "${name}"`;
const CHECK = (n) => `node scripts/verify/checks/task-${n}.mjs`;
const ATTEST = (id) => `node scripts/verify/checks/check-native-attestation.mjs ${id}`;

export const COMMANDS = {
  // ===== ci-release-pipeline =====
  "CRP-R01-S01": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R01-S02": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R01-S03": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R02-S01": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R02-S02": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R02-S03": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R03-S01": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R03-S02": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R04-S01": "node scripts/verify/checks/task-crp.mjs",
  // 三平台许可齐全: same structural contract as S01 — the workflow must
  // upload a NOTICE for each of macos/windows/linux, not just one.
  "CRP-R04-S02": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R05-S01": "node scripts/verify/checks/task-crp.mjs",
  "CRP-R05-S02": "node scripts/verify/checks/task-crp.mjs",

  // ===== portable-library-layout (PLL) =====
  // This whole area was absent from the table, so all seven rows were reading as
  // "needs an operator" through the fallback. Each has a targeted test: the
  // artist-folder publish path, control-plane usability, the manifest/record
  // round trips, the absolute-path-free payload, ignoring echo/tmp, and restore
  // projecting UUIDs while keeping media-less records.
  //
  // R03-S01/S02 used to be `COREC("application::restore")` — a whole-module
  // filter that only ever asserted *song* projection while the playbook claimed
  // 喜欢/歌单/成员顺序 coverage. After `restore-user-data-from-library-records`
  // each row points at the case that asserts the THEN clause it is registered
  // for, so breaking playlists/member-order/favorites now fails the scenario
  // instead of hiding behind a passing song test.,

  // ===== desktop-app-shell =====
  "DAS-R01-S01": COREC("root_switch::tests::empty_directory_activates"),
  "DAS-R01-S02": COREC("root_switch::tests::root_level_error_keeps_old_active_root"),
  "DAS-R01-S03": COREC("root_switch::tests::read_only_root_activates_readonly"),
  "DAS-R01-S04": COREC("boot::tests::clean_active_root_recovers_idle_and_releases_the_scan_exclusion"),
  "DAS-R02-S01": REACT("src/app/narrow.test.tsx"),
  "DAS-R02-S02": REACT("src/features/library/SongList.test.tsx"),
  "DAS-R02-S03": CHECK("13.8"), // 一期排除入口 scope guard
  // Batch menu (S04): the row opens the selection-wide menu from both the
  // context menu and the keyboard menu key — proven by SongRow's batch-menu
  // and keyboard-menu cases. S05 is the management-action side of the same
  // requirement (no sync, provided batch entry): task-13.8 asserts the UI has
  // no operable sync entry anywhere.,
  "DAS-R02-S04": REACT("src/features/library/SongRow.test.tsx"),
  "DAS-R02-S05": CHECK("13.8"),
  // CSS-only interaction behaviors: the brand operations are revealed/closed
  // by `:hover`/`:focus-within` in shell.css (task 7.4 + the compact brand
  // action change), the desktop hit targets are 36px (44px in narrow, asserted
  // by the narrow.css media query), and the search-box chrome is unchanged by
  // the import move. None of these is a JS-assertable behavior — each is a
  // rendered visual property verified by an operator against the prototype.,
  "DAS-R02-S06": ATTEST("DAS-R02-S06"),
  "DAS-R02-S07": ATTEST("DAS-R02-S07"),
  "DAS-R02-S08": REACT("src/app/accessibility.test.tsx"), // keyboard order reaches brand actions,
  "DAS-R02-S09": ATTEST("DAS-R02-S09"),
  "DAS-R02-S10": ATTEST("DAS-R02-S10"),
  "DAS-R03-S01": "pnpm --filter @echo/desktop test -- --run src/app/App.test.tsx -t \"keeps the import button in the brand area under the playlist view\"", // 新增: 品牌区导入入口所有视图可用
  "DAS-R03-S02": "pnpm --filter @echo/desktop test -- --run src/app/App.test.tsx -t \"keeps the import button in the brand area under the artist directory\"", // 新增: 品牌区导入入口所有视图可用
  "DAS-R03-S03": "pnpm --filter @echo/desktop test -- --run src/app/App.test.tsx -t \"hides the import button for a read-only library\"", // 新增: 品牌区导入入口所有视图可用
  "DAS-R04-S01": DESK("platform::local_state"), // （重编号自 DAS-R03-S01）
  "DAS-R04-S02": DESK("platform::local_state"), // （重编号自 DAS-R03-S02）
  "DAS-R04-S03": DESK("platform::local_state"), // （重编号自 DAS-R03-S03）
  "DAS-R05-S01": CHECK("9.6"), // window close→exit (automated), // （重编号自 DAS-R04-S01）
  "DAS-R05-S02": CHECK("9.6"), // window close→background (automated), // （重编号自 DAS-R04-S02）
  "DAS-R05-S03": CHECK("9.6"), // close during init (automated), // （重编号自 DAS-R04-S03）
  "DAS-R06-S01": CHECK("9.3"), // （重编号自 DAS-R05-S01）
  "DAS-R06-S02": CHECK("9.3"), // （重编号自 DAS-R05-S02）
  "DAS-R06-S03": CHECK("9.3"), // （重编号自 DAS-R05-S03）
  "DAS-R07-S01": CHECK("9.1"), // （重编号自 DAS-R06-S01）
  "DAS-R07-S02": CHECK("9.1"), // （重编号自 DAS-R06-S02）
  "DAS-R07-S03": CHECK("9.1"), // （重编号自 DAS-R06-S03）
  "DAS-R07-S04": ATTEST("DAS-R07-S04"), // 重编号自 DAS-R06-S04；无专属测试，人工 attestation
  "DAS-R07-S05": ATTEST("DAS-R07-S05"), // 重编号自 DAS-R06-S05；无专属测试，人工 attestation
  "DAS-R08-S01": REACT("src/app/narrow.test.tsx"), // （重编号自 DAS-R07-S01）
  "DAS-R08-S02": REACT("src/app/overlays.test.tsx"), // （重编号自 DAS-R07-S02）
  "DAS-R08-S03": REACT("src/app/overlays.test.tsx"), // （重编号自 DAS-R07-S03）
  "DAS-R09-S01": REACT("src/app/accessibility.test.tsx"), // （重编号自 DAS-R08-S01）
  "DAS-R09-S02": REACT("src/app/accessibility.test.tsx"), // （重编号自 DAS-R08-S02）
  "DAS-R09-S03": REACT("src/app/overlays.test.tsx"), // （重编号自 DAS-R08-S03）
  "DAS-R10-S01": CHECK("13.8"), // offline/no-network, // （重编号自 DAS-R09-S01）
  "DAS-R10-S02": CHECK("13.8"),
  // wire-desktop-system-dialogs: real OS dialogs/reveal, WebView stays pathless., // （重编号自 DAS-R09-S02）
  "DAS-R11-S01": CHECK("wire-dialogs"), // TauriDialogs native folder pick wired, // （重编号自 DAS-R10-S01）
  "DAS-R11-S02": CHECK("wire-dialogs"), // （重编号自 DAS-R10-S02）
  "DAS-R11-S03": CHECK("wire-dialogs"), // capability set stays free of dialog/fs, // （重编号自 DAS-R10-S03）
  "DAS-R12-S01": REACT("src/app/App.test.tsx"), // workspace-empty claims full workspace, // （重编号自 DAS-R11-S01）
  "DAS-R12-S02": REACT("src/app/App.test.tsx"),
  // R12–R14 由对应 gate 覆盖 (见下)。
  // DAS-R15: 应用在应用工作区阻止 WebView 默认右键菜单。App.tsx 在加载时向
  // document 注册全局 `contextmenu` preventDefault(非测试可见行为); 该开关由
  // 人工验证(在应用工作区右键不出现浏览器菜单)。, // （重编号自 DAS-R11-S02）
  "DAS-R13-S01": ATTEST("DAS-R13-S01"), // 重编号自 DAS-R12-S01；无专属测试，人工 attestation
  "DAS-R13-S02": ATTEST("DAS-R13-S02"), // 重编号自 DAS-R12-S02；无专属测试，人工 attestation
  "DAS-R14-S01": ATTEST("DAS-R14-S01"), // 重编号自 DAS-R13-S01；无专属测试，人工 attestation
  "DAS-R15-S01": ATTEST("DAS-R15-S01"), // 重编号自 DAS-R14-S01；无专属测试，人工 attestation
  "DAS-R15-S02": ATTEST("DAS-R15-S02"), // 重编号自 DAS-R14-S02；无专属测试，人工 attestation
  "DAS-R15-S03": ATTEST("DAS-R15-S03"), // 重编号自 DAS-R14-S03；无专属测试，人工 attestation
  "DAS-R15-S04": ATTEST("DAS-R15-S04"), // 重编号自 DAS-R14-S04；无专属测试，人工 attestation
  "DAS-R15-S05": ATTEST("DAS-R15-S05"), // 重编号自 DAS-R14-S05；无专属测试，人工 attestation
  "DAS-R15-S06": ATTEST("DAS-R15-S06"), // 重编号自 DAS-R14-S06；无专属测试，人工 attestation
  "DAS-R16-S01": ATTEST("DAS-R15-S01"),

  // ===== desktop-playback (DP) =====, // （重编号自 DAS-R15-S01）

  // ===== desktop-playback =====
  "DP-R01-S01": "cargo test -p echo-desktop --all-features --test player_smoke",
  "DP-R01-S02": "cargo test -p echo-desktop --all-features --test player_smoke",
  // 平台播放后端缺失: the per-platform libmpv resolution contract and its
  // explicit-error path are asserted by task-9.7 (the three-platform Gate).
  "DP-R01-S03": "node scripts/verify/checks/task-9.7.mjs",
  // Overlap is fine: both R01 scenarios resolve to the real-libmpv smoke suite.,
  "DP-R02-S01": REACT("src/features/player/QueuePanel.test.tsx"),
  "DP-R02-S02": REACT("src/features/player/QueuePanel.test.tsx"),
  "DP-R02-S03": REACT("src/features/player/QueuePanel.test.tsx"),
  "DP-R02-S04": REACT("src/features/player/QueuePanel.test.tsx"),
  // R02-S05..S07 were never registered and therefore fell through the
  // `COMMANDS[id] || ATTEST(id)` fallback, reading as "needs an operator" when
  // nobody had judged them at all. The queue suite proves each THEN clause:
  // play-next lane order, the current entry pinned first with the loop wrap
  // last, and clear-pending keeping the current song. Judged in
  // docs/native-attestation-playbook.md.,
  "DP-R02-S05": DESK("player::queue::tests::play_next_lane_is_fifo_and_projection_precedes_normal_entries"),
  "DP-R02-S06": DESK("player::queue::tests::view_projection_keeps_current_first_and_includes_loop_wrap"),
  "DP-R02-S07": DESK("player::queue::tests::clear_pending_keeps_current_and_history"),
  "DP-R02-S08": "cargo test -p echo-desktop --all-features promote_does_not_fold_ordinary_duplicates_or_current", // 新增命令: 提升不重复展开普通待播（优先区提升不折叠普通入队/当前曲）
  "DP-R02-S09": "cargo test -p echo-desktop --all-features priority_lane_physical_contiguity", // 新增命令: 优先区物理连续（当前曲后紧邻切片）
  "DP-R02-S10": "cargo test -p echo-desktop --all-features restore_with_duplicate_lane_ids_for_one_song_collapses_on_promote", // 新增命令: 优先区重复副本清理（同歌重复 lane id 合并）
  "DP-R02-S11": "cargo test -p echo-desktop --all-features promote_does_not_fold_ordinary_duplicates_or_current", // 新增命令: 去重判据不覆盖普通入队（普通重复保留、当前曲提升为空操作）
  "DP-R02-S12": "cargo test -p echo-desktop --all-features play_next_lane_is_fifo_and_projection_precedes_normal_entries", // 新增命令: 优先区消费（FIFO 播放，投影领先普通条目）
  "DP-R02-S13": "cargo test -p echo-desktop --all-features clear_pending_keeps_only_current_even_after_loop_wrap", // 新增命令: 清空队列（clear_pending 仅留当前曲）
  "DP-R02-S14": "cargo test -p echo-desktop --all-features priority_lane_round_trips_through_snapshot_and_rebuild", // 新增命令: 会话恢复的优先区（快照往返重建）
  "DP-R03-S01": DESK("player::coordinator::tests::mode_switch_keeps_current_item"),
  "DP-R03-S02": DESK("player::coordinator::tests::next_advances_in_order"),
  "DP-R03-S03": DESK("player::coordinator::tests::previous_moves_back_within_5_seconds"),
  "DP-R03-S04": "node scripts/verify/checks/check-native-attestation.mjs DP-R03-S04", // 新增命令: 沉浸模式尺寸一致
  "DP-R04-S01": DESK("player::coordinator::tests::error_skips_bad_entry_and_plays_next_sequential"),
  "DP-R04-S02": DESK("player::coordinator::tests::error_advance_all_bad_stops_without_spin"),
  "DP-R04-S03": DESK("player::coordinator::tests::error_advance_never_retries_same_entry_in_round"),
  // `repeat_one` covers both THEN clauses of R04-S04: natural end replays the
  // entry, and an explicit next ignores the single-repeat rule.,
  "DP-R04-S04": DESK("repeat_one"),
  "DP-R04-S05": DESK("player::coordinator::tests::mode_switch_keeps_current_item"),
  // 定位和音量 (DP-R04-S06) 的 THEN 是「拖动进度条 / 音量滑块 / 点击静音」三件事,
  // 所以命令必须同时覆盖三条 —— 只指 seek 那条等于把音量与静音留成未验收。
  // 这三条 coordinator 级用例共用 `_through_coordinator` 后缀, 且只匹配这三条。,
  "DP-R04-S06": DESK("_through_coordinator"),
  "DP-R05-S01": CHECK("9.2"), // file association + temp-item,
  "DP-R05-S02": CHECK("11.7"),
  "DP-R05-S03": DESK("player::coordinator::tests::recovered_blocked_entry_becomes_eligible_after_retry"),
  "DP-R06-S01": COREC("infrastructure::sqlite::tests::playback_sessions_are_idempotent"),
  "DP-R06-S02": COREC("infrastructure::sqlite::tests::playback_sessions_are_idempotent"),
  "DP-R06-S03": "cargo test -p echo-desktop --all-features entering_loading_starts_without_stale_progress_facts", // 新增命令: 进入 Loading 时不携带旧进度
  "DP-R06-S04": "cargo test -p echo-desktop --all-features file_loaded_still_drives_progress_from_mpv_reports", // 新增命令: 加载成功后的进度仍连续
  "DP-R07-S01": DESK("platform::local_state"),
  // Phase-one recovery regression: desktop queue/session paths plus the
  // committed playlist and failure-feedback UI path run as one registered
  // offline acceptance command.,
  "DP-R07-S02": "cargo test -p echo-desktop --all-features --lib && pnpm --dir apps/desktop test",
  // 临时播放项边界 (R07-S03/S04): 当前临时项导入成功后原位替换 queue entry 并
  // 恢复播放(S03), 导入失败保留原临时 entry 与播放状态(S04), 分别由 coordinator
  // 的 dedicated 测试证明。,
  "DP-R07-S03": DESK("player::coordinator::tests::importing_current_temporary_entry_replaces_it_in_place_and_resumes_playing"),
  "DP-R07-S04": DESK("player::coordinator::tests::importing_current_temporary_entry_preserves_pause_and_rejects_stale_completion"),
  "DP-R08-S01": CHECK("9.4"),
  "DP-R08-S02": REACT("src/player/useGlobalPlayerHotkeys.test.tsx"),
  "DP-R09-S01": CHECK("9.3"),
  "DP-R09-S02": CHECK("9.3"),
  "DP-R09-S03": CHECK("9.6"),
  // The three-day-history and session-restore family: history entry lookup,
  // snapshot round-trip across a restart, stale-entry filtering, rollback
  // keeping the timestamped history, and restore landing paused.
  // 列表循环的"上一首"按上下文取前一项并首项回绕 —— `coordinator/tests.rs` 那条
  // single-entry 用例只证明"单条目队列重播而不暂停"，证明不了走位与回绕，所以指向
  // queue 级的那条新用例（见 docs/native-attestation-playbook.md §7.3）。,
  "DP-R10-S01": "node scripts/verify/checks/task-9.4.mjs", // 新增命令: 使用媒体键
  "DP-R10-S02": "node scripts/verify/checks/task-11.8.mjs", // 新增命令: 使用应用快捷键
  "DP-R11-S01": DESK("player::queue::tests::previous_in_loop_walks_the_context_backwards_and_wraps_to_the_tail"),
  "DP-R11-S02": DESK("player::queue::tests::previous_returns_history_entry"),
  "DP-R11-S03": DESK("player::session::tests::state_store_save_load_round_trip_is_atomic_and_clears"),
  "DP-R12-S01": REACT_T("src/features/player/PlayerBar.test.tsx", "with a neutral button"),
  "DP-R12-S02": REACT_T("src/features/player/PlayerBar.test.tsx", "while the theme changes"),
  "DP-R12-S03": "cargo test -p echo-desktop --all-features restore_keeps_fresh_history_and_counts_only_blocked_entries", // 新增命令: 三天播放历史跨重启
  "DP-R12-S04": DESK("player::session::tests::restore_keeps_fresh_history_and_counts_only_blocked_entries"),
  "DP-R12-S05": DESK("player::deletion::tests::rollback_restores_timestamped_history_so_previous_remains_available"),
  // DP-R12 非沉浸播放模式视觉语义 — the persistent bar must show 随机播放 with a
  // neutral style while still reading as selected. PlayerBar.test.tsx pins both
  // halves in one render: the 喜欢 heart carries `.control.active` (the bar's only
  // accent-bearing selected class), and the mode button never does — under any
  // theme. The colour claim is proven through that class, not by asserting on
  // stylesheet text.,
  "DP-R13-S01": DESK("player::coordinator::tests::restore_session_recovers_queue_mode_and_settings_paused"),
  "DP-R13-S02": COREC("playback_restore"),
  "DP-R14-S01": COREC("application::playback_context::tests::resolves_all_pages_and_validates_selected_song"),
  "DP-R14-S02": COREC("application::playback_context::tests::recent_filters_case_insensitively_and_rejects_absent_selection"),
  "DP-R15-S01": "cargo test -p echo-core --all-features application::playback_context::tests::resolves_all_pages_and_validates_selected_song", // 新增命令: 从超过单页上限的资料库视图开始播放
  "DP-R15-S02": "cargo test -p echo-core --all-features application::playback_context::tests::recent_filters_case_insensitively_and_rejects_absent_selection", // 新增命令: 从最近视图开始播放
  "DP-R15-S03": COREC("application::playback_context::tests::playlist_uses_newest_member_first"),
  "DP-R15-S04": COREC("application::playback_context::tests::unknown_library_view_is_rejected_in_core"),

  // ===== immersive-lyrics (IL) =====,
  "DP-R16-S01": "cargo test -p echo-desktop --all-features restore_or_prime_primes_the_first_song_of_the_default_view", // 新增命令: 重新打开已有资料库选择初始待播放项
  "DP-R16-S02": "cargo test -p echo-desktop --all-features restore_or_prime_replaces_a_dropped_current_with_the_newest_candidate", // 新增命令: 上次播放歌曲被外部删除
  "DP-R16-S03": "cargo test -p echo-desktop --all-features restore_or_prime_yields_an_empty_bar_for_an_empty_library", // 新增命令: 资料库为空
  "DP-R16-S04": "cargo test -p echo-desktop --all-features restore_session_recovers_queue_mode_and_settings_paused", // 新增命令: 有效播放会话优先恢复
  "DP-R17-S01": "cargo test -p echo-desktop --all-features prime_context_paused_preserves_settings_without_playing", // 新增命令: 兜底选择保持用户播放设置

  // ===== immersive-lyrics =====
  "IL-R01-S01": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R01-S02": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R01-S03": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R02-S01": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R02-S02": REACT("src/features/player/PlayerBar.test.tsx"),
  "IL-R02-S03": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R03-S01": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R03-S02": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R03-S03": REACT("src/features/player/ImmersivePlayer.test.tsx"),
  "IL-R04-S01": COREC("lyrics"), // select_effective_lyrics,
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

  // ===== library-experience (LE) =====,

  // ===== library-experience =====
  "LE-R01-S01": COREC("infrastructure::sqlite::tests::catalog_all_songs_view"),
  "LE-R01-S02": COREC("infrastructure::sqlite::tests::catalog_search_no_results"),
  "LE-R01-S03": COREC("infrastructure::sqlite::tests::catalog_repository_gate_empty"),
  "LE-R01-S04": REACT("src/features/library/CollectionDirectory.test.tsx"), // 切换歌手或专辑目录,
  "LE-R02-S01": COREC("infrastructure::sqlite::tests::catalog_search_matches_full_query"),
  "LE-R02-S02": COREC("infrastructure::sqlite::tests::catalog_search_empty_query"),
  "LE-R02-S03": REACT("src/features/library/SongList.test.tsx"),
  // R02-S04 was never registered (fallback read as "needs an operator"): the
  // THEN clause is "drop the old root's results and only accept the new root's",
  // which is exactly the delayed-response test in useSongs.,
  "LE-R02-S04": REACT("src/features/library/useSongs.test.tsx"),
  // R02-S05 (切换活动资料库后查询): same delayed-response suite — dropping the
  // old root's in-flight query and only accepting the new root's.,
  "LE-R02-S05": REACT("src/features/library/useSongs.test.tsx"),
  "LE-R03-S01": COREC("infrastructure::sqlite::tests::catalog_all_songs_keyset_pages"),
  "LE-R03-S02": "pnpm --filter @echo/desktop test -- --run src/features/library/songSortControl.test.tsx -t \"uses 歌手 terminology, offers 专辑, and updates the selected sort\"", // 新增: 全部歌曲排序
  "LE-R03-S03": "pnpm --filter @echo/desktop test -- --run src/features/library/songSortControl.test.tsx -t \"uses 歌手 terminology, offers 专辑, and updates the selected sort\"", // 新增: 全部歌曲排序
  "LE-R03-S04": COREC("infrastructure::sqlite::tests::catalog_favorites_view"), // （重编号自 LE-R03-S02）
  "LE-R03-S05": COREC("infrastructure::sqlite::tests::catalog_recent_100"), // （重编号自 LE-R03-S03）
  "LE-R03-S06": "pnpm --filter @echo/desktop test -- --run src/features/library/songSortControl.test.tsx -t \"retains the existing artist preference and restores the album preference\"", // 新增: 全部歌曲排序
  "LE-R04-S01": REACT("src/features/library/SongList.test.tsx"),
  "LE-R04-S02": REACT("src/features/library/SongMenu.test.tsx"),
  "LE-R05-S01": REACT("src/features/library/SongMenu.test.tsx"), // detail relative-path only,
  "LE-R05-S02": REACT("src/features/library/SongMenu.test.tsx"),
  "LE-R05-S03": REACT("src/features/library/SongMenu.test.tsx"),
  "LE-R06-S01": REACT("src/features/library/SongList.test.tsx"),
  "LE-R06-S02": REACT("src/features/library/SongList.test.tsx"),
  "LE-R06-S03": REACT("src/features/library/SongList.test.tsx"),
  "LE-R06-S04": COREC("scan::tests::enumerate_failure_marks_run_failed_without_missing"),
  // R06-S05..S06 (打开本地目录 / 删除歌曲) belong to the song-operations UI in
  // SongMenu; S05 由 task-9.5 的 reveal-by-SongId adapter 证明。S07–S10 是删除
  // 的回收站边界: 不重启最终化 / 暂败重试 / 结果无法证明 / 路径边界保护 —— 由
  // trash 套件与 task-12.7 安全 gate 证明 (与 LL-R07 同一组测试)。,
  "LE-R06-S05": "node scripts/verify/checks/task-9.5.mjs", // 新增命令: 打开本地目录
  "LE-R06-S06": "pnpm --filter @echo/desktop test -- --run src/features/library/SongMenu.test.tsx -t \"dispatches delete and shows the 10-second undo affordance on success\"", // 新增命令: 删除歌曲（确认+10秒撤销）
  "LE-R06-S07": COREC("trash::tests::explicit_system_trash_success_persists_applied_then_finalizes"),
  "LE-R06-S08": COREC("trash::tests::system_trash_failure_keeps_the_verified_staging_for_retry"),
  "LE-R06-S09": COREC("trash::tests::post_call_trash_error_rechecks_missing_staging_and_stops_other_operations"),
  "LE-R06-S10": CHECK("12.7"), // trash 目标只接受绑定根下匹配的 echo/tmp/trash/<op-id>
  // The 50k budget bench is `#[ignore]`d so a normal `cargo test` stays fast —
  // without `-- --ignored` cargo runs zero tests, which `run-scenario.mjs`
  // correctly rejects rather than passing on an empty selection.,
  "LE-R07-S01": `${COREC("bench_50k_search_and_first_screen_p95_meet_prd_budgets")} -- --ignored`,
  "LE-R07-S02": REACT("src/features/library/SongList.test.tsx"),
  // R07-S03..S04 written inline below; S05–S09 (多选保留/全选/跨分页/查询上下文/
  // 虚拟列表) by the selection suite + SongList virtual-row cases.,
  "LE-R07-S03": "pnpm --filter @echo/desktop test -- --run src/features/library/CollectionDirectory.test.tsx -t \"shows artist cards and reuses the library song table with selection in the detail\"", // 新增命令: 聚合详情支持全选
  "LE-R07-S04": "node scripts/verify/checks/check-native-attestation.mjs LE-R07-S04", // 新增命令: 多选模式右键未选中歌曲
  "LE-R07-S05": REACT("src/features/library/batchLibraryOperations.test.tsx"),
  "LE-R07-S06": REACT("src/features/library/useSongSelection.test.tsx"),
  "LE-R07-S07": REACT_T("src/features/library/SongList.test.tsx", "restores a controlled bulk selection after a virtual row leaves and re-enters"),
  "LE-R07-S08": REACT_T("src/features/library/useSongSelection.test.tsx", "clears when the view/query selection scope changes"),
  "LE-R07-S09": REACT("src/features/library/SongList.test.tsx"), // 虚拟列表行 key 稳定 + viewport slice
  // library-nav-counts: backend-driven view counts, invalidation on changes.,
  "LE-R08-S01": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R08-S02": REACT("src/features/library/libraryNavCounts.test.tsx"),
  // R08-S03..S08 (批量: 歌单移除/队列操作/删除撤回/不可删项/只读根/失败):
  // 批量操作套件 + PlaylistsView 批量移除 + 只读表面禁用写操作。,
  "LE-R08-S03": REACT("src/features/playlists/PlaylistsView.test.tsx"),
  "LE-R08-S04": REACT("src/features/library/batchOperations.test.ts"),
  "LE-R08-S05": REACT("src/features/library/batchLibraryOperations.test.tsx"),
  "LE-R08-S06": REACT("src/features/library/batchOperations.test.ts"),
  "LE-R08-S07": REACT("src/features/library/BatchSongActions.test.tsx"),
  "LE-R08-S08": REACT("src/features/library/batchLibraryOperations.test.tsx"),
  "LE-R09-S01": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R09-S02": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R09-S03": REACT("src/features/library/libraryNavCounts.test.tsx"),
  "LE-R09-S04": REACT("src/features/library/libraryNavCounts.test.tsx"),

  // ===== local-library (LL) =====,
  "LE-R10-S01": "pnpm --filter @echo/desktop test -- --run src/features/library/SongList.test.tsx -t \"renders only a viewport slice of a 50,000-song library, not all rows\"", // 新增命令: 浏览大量歌曲（仅渲染可见范围）
  "LE-R10-S02": "pnpm --filter @echo/desktop test -- --run src/features/library/SongList.test.tsx -t \"keeps rows keyed by stable SongId at their absolute position\"", // 新增命令: 搜索或排序后保持定位（行 key 稳定）
  "LE-R11-S01": "pnpm --filter @echo/desktop test -- --run src/features/library/libraryNavCounts.test.tsx -t \"shows 喜欢的音乐's count without the user ever opening that view\"", // 新增命令: 未打开过的视图也显示计数
  "LE-R11-S02": "pnpm --filter @echo/desktop test -- --run src/features/library/libraryNavCounts.test.tsx -t \"prints the backend total, not the number of rows the page loaded\"", // 新增命令: 计数不受分页影响
  "LE-R11-S03": "cargo test -p echo-core --all-features catalog_counts_match_view_membership_over_sqlite", // 新增命令: 计数与视图定义一致（pending-delete/非活动根不计）
  "LE-R11-S04": "node scripts/verify/checks/check-native-attestation.mjs LE-R11-S04", // 新增命令: 最近添加上限（100 首）
  "LE-R12-S01": "pnpm --filter @echo/desktop test -- --run src/features/library/useSongs.test.tsx -t \"keeps the full query total stable while loading another page\"", // 新增: 分页歌曲列表显示匹配总数
  "LE-R12-S02": "pnpm --filter @echo/desktop test -- --run src/features/library/useSongs.test.tsx -t \"uses the filtered favorites search total\"", // 新增: 分页歌曲列表显示匹配总数
  "LE-R12-S03": "cargo test -p echo-core --all-features catalog_search_pages_deterministically_across_large_library", // 新增: 分页歌曲列表显示匹配总数
  "LE-R12-S04": "pnpm --filter @echo/desktop test -- --run src/features/library/useSongs.test.tsx -t \"keeps the full query total stable while loading another page\"", // 新增: 分页歌曲列表显示匹配总数
  "LE-R12-S05": "pnpm --filter @echo/desktop test -- --run src/features/library/useSongs.test.tsx -t \"sends normalized search through the recent view command\"", // 新增: 分页歌曲列表显示匹配总数
  "LE-R13-S01": ATTEST("LE-R13-S01"), // 重编号自 LE-R12-S01；无专属测试，人工 attestation
  "LE-R13-S02": ATTEST("LE-R13-S02"), // 重编号自 LE-R12-S02；无专属测试，人工 attestation
  "LE-R13-S03": ATTEST("LE-R13-S03"), // 重编号自 LE-R12-S03；无专属测试，人工 attestation
  "LE-R13-S04": ATTEST("LE-R13-S04"), // 重编号自 LE-R12-S04；无专属测试，人工 attestation
  "LE-R13-S05": ATTEST("LE-R13-S05"), // 重编号自 LE-R12-S05；无专属测试，人工 attestation
  "LE-R14-S01": ATTEST("LE-R14-S01"), // 重编号自 LE-R13-S01；无专属测试，人工 attestation
  "LE-R14-S02": ATTEST("LE-R14-S02"), // 重编号自 LE-R13-S02；无专属测试，人工 attestation
  "LE-R14-S03": ATTEST("LE-R14-S03"), // 重编号自 LE-R13-S03；无专属测试，人工 attestation
  "LE-R15-S01": ATTEST("LE-R15-S01"), // 重编号自 LE-R14-S01；无专属测试，人工 attestation
  "LE-R15-S02": ATTEST("LE-R15-S02"), // 重编号自 LE-R14-S02；无专属测试，人工 attestation
  "LE-R15-S03": ATTEST("LE-R15-S03"), // 重编号自 LE-R14-S03；无专属测试，人工 attestation
  "LE-R16-S01": ATTEST("LE-R16-S01"), // 重编号自 LE-R15-S01；无专属测试，人工 attestation
  "LE-R16-S02": ATTEST("LE-R16-S02"), // 重编号自 LE-R15-S02；无专属测试，人工 attestation
  "LE-R16-S03": ATTEST("LE-R16-S03"), // 重编号自 LE-R15-S03；无专属测试，人工 attestation
  "LE-R16-S04": ATTEST("LE-R16-S04"), // 重编号自 LE-R15-S04；无专属测试，人工 attestation
  "LE-R16-S05": ATTEST("LE-R16-S05"), // 重编号自 LE-R15-S05；无专属测试，人工 attestation
  "LE-R16-S06": ATTEST("LE-R16-S06"), // 重编号自 LE-R15-S06；无专属测试，人工 attestation
  "LE-R17-S01": ATTEST("LE-R17-S01"), // 重编号自 LE-R16-S01；无专属测试，人工 attestation
  "LE-R17-S02": ATTEST("LE-R17-S02"), // 重编号自 LE-R16-S02；无专属测试，人工 attestation
  "LE-R17-S03": ATTEST("LE-R17-S03"), // 重编号自 LE-R16-S03；无专属测试，人工 attestation
  "LE-R18-S01": "pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx -t \"searches within the current playlist through the scoped search command\"", // 新增: 歌单内搜索
  "LE-R18-S02": "pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx -t \"restores the full member list when the search is cleared\"", // 新增: 歌单内搜索
  "LE-R18-S03": "pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx -t \"shows the playlist search empty state with a clear action on no hit\"", // 新增: 歌单内搜索
  // The `&& cargo test -p echo-core ...` tail that manifest.json still carries
  // for this row is deliberately NOT in the command map: validate-scenario-commands
  // anchors its vitest regex with `$`, so a chained command is never resolved
  // against the test file and the row loses its test-name proof. Keep the two
  // representations separate rather than syncing them.
  "LE-R18-S04": "pnpm --filter @echo/desktop test -- --run src/features/library/useSongs.test.ts -t \"playlistId 变化触发新的抓取请求（缓存键区分布同歌单）\"", // 新增: 歌单内搜索
  "LE-R19-S01": "pnpm --filter @echo/desktop test -- --run src/features/library/SongList.test.tsx -t \"scrolls the matched song row to the top of the visible area\"", // 新增: 定位当前播放歌曲
  "LE-R19-S02": "pnpm --filter @echo/desktop test -- --run src/features/library/favoriteSync.test.tsx -t \"voices 当前没有正在播放的歌曲\"", // 新增: 定位当前播放歌曲
  "LE-R19-S03": "pnpm --filter @echo/desktop test -- --run src/features/library/favoriteSync.test.tsx -t \"voices 当前歌曲不在此列表中\"", // 新增: 定位当前播放歌曲
  "LE-R19-S04": "pnpm --filter @echo/desktop test -- --run src/features/library/SongList.test.tsx -t \"clamps an end-of-list target to the last reachable position\"", // 新增: 定位当前播放歌曲
  "LE-R19-S05": "node scripts/verify/checks/check-native-attestation.mjs LE-R19-S05", // 新增: 定位当前播放歌曲

  // ===== local-library =====
  "LL-R01-S01": COREC("root_switch::"),
  "LL-R01-S02": COREC("root_switch::"),
  "LL-R01-S03": COREC("root_switch::"),
  // Re-selecting a managed directory after the local database was wiped: the
  // identities come from `echo/records/`, the media tree is untouched.,
  "LL-R01-S04": COREC("root_switch::tests::prepare_reuses_record_identities_and_leaves_media_untouched"),
  "LL-R02-S01": COREC("scan::tests::manual_rescan_is_repeatable_and_converges"),
  "LL-R02-S02": COREC("watch::tests::watch_events_converge_under_out_of_order_and_duplicate_delivery"),
  "LL-R02-S03": COREC("scan::tests::progress_persisted_throttled_and_terminal_state_kept"),
  // R02-S04/R02-S05 were never registered. S04's THEN clause is "refuse the old
  // layout, do not scan or migrate it" — proven by the manifest version check.
  // S05's is "update records once watcher events settle" — proven by the
  // coalescer that normalises removal/rename/overflow into one rescan.,
  "LL-R02-S04": COREC("manifest_incompatible_version_fails_check"),
  // 监听外部新增和修改 (LL-R02-S05) 的 THEN 是「事件稳定后更新记录与索引」+「事件丢失
  // 时手动重扫收敛」, 涉及新增/删除/移动/修改四类。只指 coalescer 那条只证明事件被归一化,
  // 证明不了记录/索引更新 —— 改指 watch 集成层整组 (settle / rename 保 UUID / publish 复用
  // journal id / overflow 退化为重扫), 重扫收敛那半由 scan 侧的 *_converge_after_rescan 覆盖。,
  "LL-R02-S05": COREC("application::watch::tests::"),
  // S06 (设置页手动重扫) 与 S07 (无活动库不可重扫): SettingsView 的 rescan 套件
  // 分别证明「重扫进行中禁止重复点击并提交结果」与「无根目录/扫描中禁用入口」。,
  "LL-R02-S06": REACT_T("src/features/settings/SettingsView.test.tsx", "starts a scan for the active library and reports completion"),
  "LL-R02-S07": REACT_T("src/features/settings/SettingsView.test.tsx", "disables rescan when no library root is configured"),
  "LL-R03-S01": COREC("infrastructure::sqlite::tests::playback_sessions_are_idempotent"), // fixture format matrix,
  "LL-R03-S02": COREC("probe"),
  "LL-R03-S03": COREC("infrastructure::sqlite::tests::scan_pipeline_persists"),
  // S04 (m4a 内嵌标签) 与 S05 (wav 无标签兜底): 由 metadata tags 系列证明。,
  "LL-R03-S04": COREC("infrastructure::metadata::tags::tests::m4a_tags_parse_from_in_memory_import_source_bytes"),
  "LL-R03-S05": COREC("tagless_wav_reads_as_ok_with_empty_fields"),
  "LL-R04-S01": COREC("scan::tests::fast_skip_unchanged_files_and_relink_on_move"),
  "LL-R04-S02": COREC("scan::tests::external_missing_relinks_on_same_hash_path_keeping_relationships"),
  "LL-R04-S03": COREC("scan::tests::duplicate_hash_paths_get_deterministic_primary"),
  "LL-R04-S04": COREC("relink::tests::relink_plans_cover_path_hash_and_music_key_rules"),
  "LL-R05-S01": COREC("infrastructure::sqlite::tests::fts_and_short_like_search"),
  "LL-R05-S02": COREC("infrastructure::sqlite::tests::keyset_pages_are_deterministic"),
  "LL-R06-S01": COREC("cover_cache_key_and_db_reference"),
  "LL-R06-S02": COREC("lyrics"),
  "LL-R07-S01": COREC("trash::tests::persisted_trash_applied_is_the_only_automatic_database_finalization_proof"), // P0,
  "LL-R07-S02": COREC("trash::tests::external_staging_cleanup_becomes_unknown_and_preserves_relationships"),
  "LL-R07-S03": COREC("scan::tests::external_deletion_is_missing_not_pending_delete_and_keeps_relationships"),
  // S04 (文件恢复: 原路径恢复或同 BLAKE3 重扫恢复可用状态): 由外部缺失恢复测试证明。,
  "LL-R07-S04": COREC("scan::tests::external_missing_recovers_on_original_path_restoring_everything"), // P0,
  "LL-R08-S01": CHECK("12.7"), // P0 path/security automated,
  "LL-R08-S02": CHECK("12.6"), // P0 perf/stress automated,
  "LL-R08-S03": COREC("permission"), // privacy/offline automated

  // ===== playlist-management (PM) =====,
  "LL-R09-S01": "cargo test -p echo-core --all-features revision_and_added_at_survive_rescans", // 新增命令: 真实导入时间（注入系统时刻，重启不变）
  "LL-R09-S02": "cargo test -p echo-core --all-features revision_and_added_at_survive_rescans", // 新增命令: 重复或重扫不改写时间
  "LL-R09-S03": "node scripts/verify/checks/check-native-attestation.mjs LL-R09-S03", // 新增命令: 当前时区转换

  // ===== phase-one-acceptance =====
  "PHA-R01-S01": "node scripts/verify/checks/check-native-attestation.mjs PHA-R01-S01", // 新增命令: 歌单与队列日常流程验收
  "PHA-R01-S02": "node scripts/verify/ci-governance.mjs",

  // ===== playlist-management =====
  "PM-R01-S01": COREC("playlist"),
  "PM-R01-S02": COREC("playlist"),
  "PM-R01-S03": COREC("playlist"),
  "PM-R01-S04": COREC("playlist"),
  // R01-S05 (delete a playlist without touching songs or other playlists) was
  // never registered; this test asserts exactly "owns the playlist and its
  // memberships but not songs or other playlists".,
  "PM-R01-S05": COREC("delete_owns_playlist_and_members_but_not_songs_or_other_playlists"),
  "PM-R02-S01": COREC("playlist"),
  "PM-R02-S02": COREC("playlist"),
  "PM-R03-S01": COREC("playlist"),
  "PM-R03-S02": COREC("playlist"),
  "PM-R03-S03": COREC("playlist"),
  // R03-S04..S06: 从歌单视图单曲菜单添加、只读视图禁用入口、重复添加仍幂等 ——
  // 由 PlaylistsView 的 picker/只读测试与 core 幂等添加测试分别证明。S07 (批量
  // 添加部分失败) 由 AddToPlaylistDialog 的顺序提交 + 部分失败可见性测试证明。,
  "PM-R03-S04": REACT_T("src/features/playlists/PlaylistsView.test.tsx", "opens the add-to-playlist picker from a member's song menu"),
  "PM-R03-S05": REACT_T("src/features/playlists/PlaylistsView.test.tsx", "keeps 添加到歌单 disabled for a read-only playlist"),
  "PM-R03-S06": COREC("add_to_multiple_playlists_is_atomic_and_idempotent"),
  "PM-R03-S07": REACT_T("src/features/playlists/AddToPlaylistDialog.test.tsx", "runs a multi-song add sequentially and keeps partial failures visible"),
  "PM-R04-S01": COREC("playlist"),
  // R04-S02/S03: 批量移除成员与部分失败聚合 —— 由 PlaylistsView 的批量移除刷新与
  // batchOperations 的批量失败聚合测试证明。,
  "PM-R04-S02": REACT_T("src/features/playlists/PlaylistsView.test.tsx", "removes the selected members one by one and refreshes the playlist"),
  "PM-R04-S03": REACT("src/features/library/batchOperations.test.ts"),
  // R05-S05/S06: 删除在撤销窗口内的撤销, 与失效歌曲恢复 —— 由 PlaylistsView 的
  // 删除撤销刷新与 sqlite 的 missing 成员恢复(不产生重复)证明。,
  "PM-R05-S01": COREC("playlist_missing_members_stay_visible"),
  "PM-R05-S02": COREC("playlist_missing_members_stay_visible"),
  "PM-R05-S03": COREC("echo_delete_finalize_cascades_memberships"),
  "PM-R05-S04": COREC("playlist"),
  // R06/R07 rows never existed in this table at all — the whole sub-family fell
  // through the fallback. R06 is 歌单异步操作反馈, and both of its THEN clauses are
  // *failure* paths (成员留在原位 / 不显示"已加入播放队列" 成功反馈), so it is
  // registered against the tests written for exactly those paths: a happy-path
  // suite would manufacture a green for a clause nothing asserts. R07 is the
  // picker's create-then-add path and its rejection path.,
  "PM-R05-S05": REACT_T("src/features/playlists/PlaylistsView.test.tsx", "refreshes the sidebar after a batch delete and its undo"),
  "PM-R05-S06": COREC("playlist_missing_members_stay_visible_and_recover_without_duplicates"),
  "PM-R06-S01": REACT_T("src/features/playlists/PlaylistsView.test.tsx", "keeps the member in place"),
  "PM-R06-S02": REACT_T("src/features/playlists/PlaylistsView.test.tsx", "never claims"),
  "PM-R06-S03": REACT("src/features/library/batchOperations.test.ts"), // 批量操作结果聚合,
  "PM-R07-S01": REACT("src/features/playlists/AddToPlaylistDialog.test.tsx"),
  "PM-R07-S02": REACT("src/features/playlists/PlaylistNameDialog.test.tsx"),

  // ===== safe-file-ingestion (SFI) =====
  // R01: 多选导入与默认目标命名. S03 (无标签 wav) is proven by the dedicated
  // untagged-wav case; S01/S02 by the per-input mixed reports.,
  "PM-R08-S01": "node scripts/verify/checks/check-native-attestation.mjs PM-R08-S01", // 新增命令: 创建歌单时记录时间
  "PM-R08-S02": "node scripts/verify/checks/check-native-attestation.mjs PM-R08-S02", // 新增命令: 加入歌单时记录时间
  "PM-R08-S03": "node scripts/verify/checks/check-native-attestation.mjs PM-R08-S03", // 新增命令: 重复加入不重写时间
  "PM-R08-S04": "node scripts/verify/checks/check-native-attestation.mjs PM-R08-S04", // 新增命令: 歌单时间按当前时区呈现
  "PM-R09-S01": "pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx -t \"searches within the current playlist\"", // 新增: 歌单详情搜索
  "PM-R09-S02": "pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx -t \"restores the full member list when the search is cleared\"", // 新增: 歌单详情搜索
  // See the LE-R18-S04 note above: the `&& pnpm ... -t "locate"` tail in
  // manifest.json is not mirrored here, for the same regex-anchoring reason.
  "PM-R10-S01": "pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx -t \"rolls to a playing member of the playlist without a toast\"", // 新增: 歌单内定位当前播放歌曲
  "PM-R10-S02": "pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx -t \"voices 当前\"", // 新增: 歌单内定位当前播放歌曲

  // ===== portable-library-layout =====
  "PLL-R01-S01": COREC("default_target_is_artist_folder_with_artist_minus_title"),
  "PLL-R01-S02": COREC("control_plane_usable_detects_writable_and_readable"),
  // The layout↔RecordKind mapping is a structural invariant, not a behaviour
  // one test can carry: a kind with no directory is silently dropped.,
  "PLL-R01-S03": CHECK("15.1"),
  "PLL-R02-S01": COREC("manifest_round_trips_atomically"),
  "PLL-R02-S02": COREC("sync_payloads_carry_no_absolute_paths"),
  "PLL-R02-S03": COREC("tmp_files_are_ignored_as_records"),
  "PLL-R02-S04": DESK("playlist_mutations_materialize_records_and_tombstones"),
  "PLL-R02-S05": DESK("recorded_plays_materialize_additive_play_stats"),
  "PLL-R02-S06": DESK("unfavorite_materializes_a_false_record_so_it_never_returns"),
  "PLL-R03-S01": COREC(
    "application::continuation::tests::continuation_restores_songs_favorites_playlists_and_member_order",
  ),
  "PLL-R03-S02": COREC("restore_projects_records_and_keeps_missing_without_media"),
  "PLL-R03-S03": COREC("root_switch::tests::prepare_continues_object_records_before_scanning_media"),
  "PLL-R04-S01": COREC(
    "application::portable::tests::ensure_control_plane_initializes_a_fresh_writable_root",
  ),
  "PLL-R04-S02": COREC(
    "application::portable::tests::ensure_control_plane_heals_a_manifest_less_directory_with_records",
  ),
  "PLL-R04-S03": COREC(
    "application::portable::tests::ensure_control_plane_reports_an_unusable_surface_without_writing",
  ),
  "PLL-R04-S04": "cargo test -p echo-core --all-features song_record_without_added_at_reads_back_as_zero", // 新增命令: 历史对象记录缺入库时刻仍可兼容恢复
  "PLL-R05-S01": COREC("root_switch::tests::prepare_continues_records_then_scans_only_unrecorded_media"),
  "PLL-R05-S02": COREC("application::continuation::tests::dangling_records_are_counted_and_never_invented"),
  "PLL-R05-S03": COREC(
    "application::continuation::tests::tombstone_outranks_a_stale_record_and_is_never_resurrected",
  ),
  "PLL-R05-S04": COREC("application::continuation::tests::continuation_is_idempotent_across_repeated_opens"),
  "PLL-R06-S01": "cargo test -p echo-core --all-features application::portable::tests::ensure_control_plane_initializes_a_fresh_writable_root", // 新增命令: 首次启用即建立 manifest
  "PLL-R06-S02": "cargo test -p echo-core --all-features application::portable::tests::ensure_control_plane_heals_a_manifest_less_directory_with_records", // 新增命令: manifest 缺失时自愈
  "PLL-R06-S03": "cargo test -p echo-core --all-features application::portable::tests::ensure_control_plane_reports_an_unusable_surface_without_writing", // 新增命令: 控制面不可写时不破坏既有内容
  "PLL-R07-S01": "cargo test -p echo-core --all-features root_switch::tests::prepare_continues_object_records_before_scanning_media", // 新增命令: 打开资料库先接续再扫描
  "PLL-R07-S02": "cargo test -p echo-core --all-features restore_projects_records_and_keeps_missing_without_media", // 新增命令: 对象资料与媒体不一致（缺失媒体也还原记录）
  "PLL-R07-S03": "cargo test -p echo-core --all-features application::continuation::tests::tombstone_outranks_a_stale_record_and_is_never_resurrected", // 新增命令: 墓碑优先于陈旧记录
  "PLL-R07-S04": "cargo test -p echo-core --all-features application::continuation::tests::continuation_is_idempotent_across_repeated_opens", // 新增命令: 重复接续结果稳定
  "PLL-R07-S05": COREC(
    "application::continuation::tests::detached_legacy_records_are_superseded_by_the_matching_local_rows",
  ),

  // ===== phase-one-acceptance (PHA) =====
  // S02 is the regression quality gate itself: workspace tests + frontend build
  // + coverage + reconciliation + churn + purity + injection proofs live in the
  // governance gate. It cannot include `verify:scenario --all` without recursing
  // into itself, so the "registered flow scenarios pass" half is carried by
  // S01's acceptance walkthrough (manual, see the playbook).,

  // ===== safe-file-ingestion =====
  "SFI-R01-S01": COREC("import"), // per-input mixed results,
  "SFI-R01-S02": COREC("import"),
  "SFI-R01-S03": COREC("untagged_wav_import_uses_the_missing_tag_fallbacks"),
  // R02: 导入选择过滤器与支持矩阵一致.,
  "SFI-R02-S01": COREC("import"),
  // R03: 同名歌词侧车导入. S02 (LRC 不可读) 继承旧 SFI-R02-S02 的 sidecar 验证.,
  "SFI-R03-S01": COREC("sidecar"),
  "SFI-R03-S02": COREC("sidecar"),
  // R04: BLAKE3 去重与重名编号.,
  "SFI-R04-S01": COREC("dedup"),
  "SFI-R04-S02": COREC("import"),
  "SFI-R04-S03": COREC("recover::tests::nothing_recoverable_rolls_back_and_releases_cleanly"), // P0: 不可用记录不占用目标,
  "SFI-R04-S04": COREC("dedup"),
  // R05: 暂存、校验、原子移动与操作日志. S03/S04/S05 由 recover 与侧车失败测试证明;
  // S05 保持 P0 自动化(发布后 watcher 抢占 -> 唯一 UUID).,
  "SFI-R05-S01": COREC("import"),
  "SFI-R05-S02": COREC("import"),
  "SFI-R05-S03": COREC("recover::tests::crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice"), // P0,
  "SFI-R05-S04": COREC("sidecar_publish_conflict_is_audio_success_lyrics_failure_and_keeps_the_incumbent"),
  "SFI-R05-S05": COREC("recover::tests::recovery_never_creates_a_duplicate_when_a_watcher_preempts"), // P0
  // R06: 逐文件结果与资料库不可用反馈.,
  "SFI-R06-S01": COREC("import"),
  "SFI-R06-S02": COREC("import"),
  // R07: 源文件、系统关联与安全边界. S03/S04/S05 是旧 SFI-R06-S03/04/05 的编号迁移:
  // 非活动旧资料库打开走 open_path 分派谓词, 源文件保持与暂存冲突走 import/staging.,
  "SFI-R07-S01": `node scripts/verify/checks/task-9.1.mjs && cargo test -p echo-desktop --all-features runtime::services::tests::open_path`, // external open -> temporary item,
  "SFI-R07-S02": `node scripts/verify/checks/task-9.1.mjs && cargo test -p echo-desktop --all-features runtime::services::tests::open_path`, // in-library open resolves to the existing UUID,
  "SFI-R07-S03": `node scripts/verify/checks/task-9.1.mjs && cargo test -p echo-desktop --all-features runtime::services::tests::open_path`, // old-root file never resolves through the old UUID,
  "SFI-R07-S04": COREC("import"),
  "SFI-R07-S05": COREC("staging"),
  // R08: 单实例唤醒与重复打开.,
  "SFI-R08-S01": CHECK("9.1"),
  "SFI-R08-S02": CHECK("9.1"),
  // R09（normalize-os-file-open-paths）余下三条是结构契约，不是单条数据断言：
  // S03 归一化由 `open_targets(&[tauri::Url]) -> Vec<PathBuf>` 的类型签名强制、
  // S04 前端不得二次解码、S05 壳层不得回退到 `try_state` 查找。三者都由 task-9.1
  // 的门（open_targets 单测 + main.rs 结构断言）覆盖，故与同族 P0 一样必须是
  // automated 命令，不能落到人工 attestation 回退。,
  "SFI-R09-S01": CHECK("9.1"), // 带百分号编码的 file URL 归一化,
  "SFI-R09-S02": CHECK("9.1"), // 非 file scheme 丢弃,
  "SFI-R09-S03": CHECK("9.1"), // 归一化契约由类型强制,
  "SFI-R09-S04": CHECK("9.1"), // 前端不得二次解码,
  "SFI-R09-S05": CHECK("9.1"), // 纵深防御不得被放宽
  // R10: 跨平台路径与恢复后的幂等性.,
  "SFI-R10-S01": COREC("import"),
  "SFI-R10-S02": COREC("import"),
  // R11: 受控并发批量导入与非阻塞反馈. S01–S04 由导入批处理与结果分类器证明.
  // S05 (多线程重叠可验证) 需要两个文件在同一同步屏障等待后同时进入阶段 —— 当前
  // import 是顺序执行(无并发 worker), 该承诺由人工记录(attestation)验证, 直到
  // 导入实际并发化 (mirrors the scan worker bound test).,
  "SFI-R11-S01": COREC("batch_import_lands_under_media_and_keeps_dedup_and_numbering"),
  "SFI-R11-S02": REACT_T("src/features/import/importFeedback.test.ts", "treats imported, duplicate and skipped as non-failures"),
  "SFI-R11-S03": REACT_T("src/features/import/importFeedback.test.ts", "reports only real failures in the failure list"),
  "SFI-R11-S04": REACT("src/features/import/ImportBatchDialog.test.tsx"),
  "SFI-R11-S05": ATTEST("SFI-R11-S05"), // 实现为顺序导入; 并发重叠待导入并发化后证明

  // ===== sync-foundation (SYN) =====,

  // ===== sync-foundation =====
  "SYN-R01-S01": CHECK("3.10"), // 0005 schema landed without touching 0001,
  "SYN-R01-S02": COREC("infrastructure::sqlite::tests::sync_foundation"),
  "SYN-R02-S01": COREC("infrastructure::sqlite::tests::sync_foundation"),
  "SYN-R02-S02": CHECK("3.14"), // outbox prewritten, no operable sync entry,
  "SYN-R03-S01": COREC("infrastructure::sqlite::tests::sync_foundation"),
  "SYN-R04-S01": COREC("infrastructure::sqlite::tests::sync_payloads_carry_no_absolute_paths"),

  // ===== ci-release-pipeline (CRP) =====
  // The release-domain behaviors are executed by .github/workflows/release.yml
  // (per-platform `pnpm echo release`, artifact assertion, SHA256SUMS,
  // upload/download artifact, softprops/action-gh-release) and verified
  // end-to-end by the tagged v0.1.0 Release. Every CRP row resolves to the
  // task-crp structural drift gate so the domain stays inside the three-way
  // reconciliation (not a silent attestation marker) without recursing into
  // task-13.9 (which is itself a scenario command in --automated runs).,
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
  }
}