# attestation 场景（原 41 条）：判定、处置与结果

> 判定在 HEAD `7927f4c` 实测得出，登记落在 `009034f` 及后续提交。本文档回答三件事：
> ① 为什么这 41 个场景会落到 `check-native-attestation.mjs`；
> ② 逐条判定它们是「已有针对性测试可登记」「有测试但覆盖不全，先补测试」还是「确实必须人工举证」；
> ③ **最终状态与还剩多少**（§0 结果列 / §7）。
>
> 复验命令（任何数字都可重跑）：
> ```bash
> node --input-type=module -e '
> import { COMMANDS } from "./scripts/verify/scenario-commands.mjs";
> import { allScenarioIds } from "./scripts/verify/spec-scenarios.mjs";
> const un = allScenarioIds().filter(d => !(d.id in COMMANDS));
> console.log(un.length);'      # 判定时 → 41；登记后 → 9
> ```

## 0. 结论摘要

| 类 | 条数 | 含义 | 处置 | 结果 |
|---|---|---|---|---|
| **甲** 可登记为自动化 | **28** | 存在针对性测试，且覆盖 spec 的 `THEN` 全部要点 | 改 `scenario-commands.mjs` 登记真命令 | ✅ 已登记（`009034f`） |
| **丙** 有测试但覆盖不全 | **4** | 存在相关测试，但只覆盖正常路径/部分要点，登记即假绿 | **先补测试**，再登记 | ✅ 用例已补并登记（§3） |
| **乙** 必须人工举证 | **9** | 核心是真实 OS 交互（进程存活 / 托盘 / 应用身份 / 真实打开文件 / 真实布局 / 端到端验收） | 操作者在真机执行本文 §5 的步骤并写 log | ⏳ 待操作者（对应任务 8.4） |

28 + 4 + 9 = 41。**施工结果：人工/兜底面 41 → 9**，`COMMANDS` 表 179 → 211 条；仍留在兜底上的 9 条与
§4 的乙类逐条相同（`DAS-R12-S01/S02`、`DAS-R13-S01`、`DP-R03-S04`、`DP-R08-S03`、
`DP-R10-S01/S02/S03`、`PHA-R01-S01`），可用上面那条复验命令直接对出来。

乙类 9 条正是 `task-9.6.mjs` 注释所说的 *"cross-platform native validation is
deferred to the platform Gate"*，也与 `scenario-commands.mjs` 顶部注释列举的 `live tray gesture /
OS file-open integration / the 3-platform smoke itself` 一一对应 —— **注释描述的那条分支确实存在，
只是它同时被当成了"未登记"的兜底，把另外 32 条也吸了进去。**

> **本文档在施工中被修正过三次**（同一条教训，三种不同长相）：
> 0. **normalize-os-file-open-paths（2026-09-20）覆盖口径更正**：`SFI-R06-S01/S02/S03`
>    （外部文件直接打开 / 活动库内文件 / 非活动旧库文件）此前登记的命令是
>    `task-12.7.mjs`（安全加固：路径穿越/TOCTOU/恶意标签/asset key），与三条场景
>    THEN 子句（路径 → 临时项 / 路径 → 既有 UUID / 旧根不得绕过隔离）**无交集** ——
>    正是下文教训 2 的形态：命令命中真测试、校验器放行，但证明的不是 THEN。
>    现已改指 `task-9.1.mjs`（壳侧归一化）+ `runtime::services::tests::open_path`
>    三个定向测试（分派谓词），覆盖描述同步更正于 `docs/traceability.md`。
>    新增治理门禁 `check-scenario-command-proof.mjs`（manifest 16.3）机械校验
>    「场景命令必须有可执行的失败证明」，防止同类错配再次静默通过。
> 1. `PM-R06-S01` 起初被判为甲类，依据是 `AddToPlaylistDialog.test.tsx` 的 "membership commit
>    fails"。落地时核对用例正文发现它断言的是**添加**歌曲失败（"添加失败，请重试" + 选择器保持），
>    而该场景要求的是**移除成员**失败（成员留原位、导航计数不变）。后者在 `PlaylistsView.tsx:163`
>    有实现、无测试 ⇒ 改判丙类。
> 2. `DP-R11-S01`（列表循环上一首）登记到了 `previous_replays_single_entry_instead_of_pausing`。
>    读正文才发现它构造的是**单条目**队列，断言"再按一次上一首回到 0:00 且仍在播放"，
>    与 THEN 的「切到前一项 / 首项回绕 / 重新投影待播」无关 ⇒ 属丙类漏判，已补 queue 级用例。
> 3. `DP-R04-S06`（定位和音量）只登记了 seek 那一条，漏掉音量与静音；`LL-R02-S05`
>    （监听外部新增和修改）只登记了 coalescer 归一化那一条，漏掉"记录/索引更新"这一核心。
>
> ⇒ **教训：判"有没有测试覆盖这个 `THEN`"必须读用例正文，且要逐个子句对，不能只看用例标题像不像。**
> 第 2、3 条都是**命令确实命中了一个真测试**、`validate-scenario-commands.mjs` 也放行了，但那个测试
> 证明的不是这条 `THEN` —— 这类错误**没有门禁能自动发现**（校验器只能验"命令命中 ≥1 个测试"），
> 所以本文 §7.3 记录了一次人工语义复核。

## 1. 根因：一个 `||` 把两件事混成了一个出口

`scripts/verify/scenario-commands.mjs`：

```js
export function scenarioCommands() {
  for (const [id, item] of byId) {
    const command = COMMANDS[id] || ATTEST(id);   // ← 兜底
    ...
  }
}
```

- `COMMANDS` 表有 **179** 条；`allScenarioIds()` 有 **218** 个场景；差的 **41** 条**从未登记**。
- 未登记 → 静默降级为 `check-native-attestation.mjs <ID>`。
- 连带后果：`gen-scenario-manifests.mjs:115` 的 `missing command for <ID>` 防护**永不触发**（兜底已补齐），
  所以「漏登记」这件事一直没有被任何门禁抓住。
- 又因为生成器的 `isNative()` 只认 traceability 里的 `tests/native/*.md` 路径，这 41 条**不在**那个集合里，
  于是它们的 yaml 被写成 `layer: automated` + `automated_filter: the command above`，
  而命令却是人工举证 —— **yaml 自相矛盾**，就是这个 `||` 的直接产物。

⇒ **判定规则用项目自己的既定策略**（`scenario-commands.mjs` 顶部注释）：

> `YAML rows → the fastest targeted test proving the behavior`
> `Truly manual OS rows → check-native-attestation.mjs <ID>`

即：只要该行为的核心能被一个针对性测试证明，就该登记真命令；只有核心是真实 OS 交互的才走人工。

## 2. 甲类：可登记为自动化（28 条）

> 本表是**判定依据**（每条当时看到的针对性测试），不是最终登记值的抄本：**权威登记见
> `scripts/verify/scenario-commands.mjs` 的 `COMMANDS` 表**。其中 3 条的登记目标在落地/复核时改过
> （`DP-R11-S01`、`DP-R04-S06`、`LL-R02-S05`，见文首修正记录与 §7.3）。
> 列当前登记值：
> ```bash
> node --input-type=module -e '
> import { COMMANDS } from "./scripts/verify/scenario-commands.mjs";
> for (const [id, c] of Object.entries(COMMANDS)) if (/^(DP|LE|LL|PM|PLL|PHA)-/.test(id)) console.log(id, c);'
> ```

命令形式沿用现有 helper（`scenario-commands.mjs`）：
`COREC(f)` = `cargo test -p echo-core --all-features <f>`；`DESK(f)` = `cargo test -p echo-desktop --all-features <f>`；
`REACT(f)` = `pnpm --filter @echo/desktop test -- --run <f>`；`REACT_T(f, name)` = 同前但用 `-t "<name>"` 收窄到单个用例。

| 场景 ID | 要验的行为（spec `THEN` 摘要） | 现有针对性测试 | 建议 |
|---|---|---|---|
| DP-R02-S05 | 下一首播放 A、B、C 顺序，普通待播保持相对顺序 | `player/queue/tests.rs` `play_next_lane_is_fifo_and_projection_precedes_normal_entries`、`insert_next_goes_immediately_after_current` | `DESK("player::queue::tests::play_next_lane_is_fifo_and_projection_precedes_normal_entries")` |
| DP-R02-S06 | 切歌/自然结束/历史回退后列表首项=当前项，回绕项在尾部 | `player/queue/tests.rs` `view_projection_keeps_current_first_and_includes_loop_wrap` | `DESK("player::queue::tests::view_projection_keeps_current_first_and_includes_loop_wrap")` |
| DP-R02-S07 | 清空待播但不删歌曲/歌单成员，当前曲继续播 | `player/queue/tests.rs` `clear_pending_keeps_current_and_history`、`clear_pending_keeps_only_current_even_after_loop_wrap` | `DESK("player::queue::tests::clear_pending_keeps_current_and_history")` |
| DP-R04-S04 | 单曲循环自然结束重播同项；手动下一首忽略单曲重复 | `player/coordinator/tests.rs` `repeat_one_repeats_current_on_end`、`explicit_next_advances_repeat_one_without_changing_selected_mode` | `DESK("repeat_one")` —— **不带模块前缀**，一次命中上面两条（覆盖该场景的两个 `THEN`） |
| DP-R04-S05 | 切模式保留当前曲与待播，不丢 FIFO 优先区 | `player/coordinator/tests.rs` `mode_switch_keeps_current_item` | `DESK("player::coordinator::tests::mode_switch_keeps_current_item")` |
| DP-R04-S06 | 更新 mpv 位置/音量/静音并显示新状态 | `player/coordinator/tests.rs` `seek_updates_snapshot_through_coordinator`、`set_volume_clamps_and_clears_mute_through_coordinator`、`unmute_restores_last_nonzero_volume_through_coordinator` | `DESK("player::coordinator::tests::seek_updates_snapshot_through_coordinator")` |
| DP-R05-S03 | 根不可用时拒绝/暂停并提示，恢复后可重试，不删队列引用 | `player/coordinator/tests.rs` `recovered_blocked_entry_becomes_eligible_after_retry` | `DESK("player::coordinator::tests::recovered_blocked_entry_becomes_eligible_after_retry")` |
| DP-R11-S01 | 列表循环上一首切前一项，首项回绕，重投影待播 | `player/coordinator/tests.rs` `previous_moves_back_within_5_seconds`、`previous_restarts_current_after_5_seconds` | `DESK("player::coordinator::tests::previous_moves_back_within_5_seconds")` |
| DP-R11-S02 | 三天内回退到最近有效历史项并保持待播集合 | `player/queue/tests.rs` `previous_returns_history_entry` | `DESK("player::queue::tests::previous_returns_history_entry")` |
| DP-R11-S03 | 重启后历史仍可用于上一首，且恢复不自动出声 | `player/session.rs` `state_store_save_load_round_trip_is_atomic_and_clears`、`priority_lane_round_trips_through_snapshot_and_rebuild` | `DESK("player::session::tests::state_store_save_load_round_trip_is_atomic_and_clears")` |
| DP-R11-S04 | 过期/失效历史被跳过，保留当前与待播顺序 | `player/session.rs` `restore_keeps_fresh_history_and_counts_only_blocked_entries` | `DESK("player::session::tests::restore_keeps_fresh_history_and_counts_only_blocked_entries")` |
| DP-R11-S05 | 删除失败回滚后仍保留快照时的时间戳历史 | `player/deletion.rs` `rollback_restores_timestamped_history_so_previous_remains_available` | `DESK("player::deletion::tests::rollback_restores_timestamped_history_so_previous_remains_available")` |
| DP-R13-S01 | 重启恢复歌曲/队列/位置并保持暂停 | `player/coordinator/tests.rs` `restore_session_recovers_queue_mode_and_settings_paused`、`restore_session_replay_keeps_persisted_mute` | `DESK("player::coordinator::tests::restore_session_recovers_queue_mode_and_settings_paused")` |
| DP-R13-S02 | 无效/负数/越界位置被归一化，不阻塞恢复 | `domain/playback_restore.rs` `restore_disposition_covers_available_missing_pending_and_absent_or_foreign`、`retry_requires_an_active_root_and_available_song` | `COREC("playback_restore")` |
| LE-R02-S04 | 切换根后丢弃旧根结果，只接受新根查询 | `features/library/useSongs.test.tsx` `discards a delayed old-root response after the active root changes` | `REACT("src/features/library/useSongs.test.tsx")` |
| LL-R02-S04 | 旧布局目录被明确拒绝，不扫描/不迁移 UUID | `domain/library.rs` `manifest_incompatible_version_fails_check`、`layout_validator_rejects_echo_under_media` | `COREC("manifest_incompatible_version_fails_check")` |
| LL-R02-S05 | 监听事件稳定后更新记录与索引；可用手动重扫收敛 | `infrastructure/filesystem/watcher.rs` `coalescer_normalizes_removal_and_rename_and_overflow`、`application/scan.rs` `external_missing_recovers_on_original_path_restoring_everything` | `COREC("coalescer_normalizes_removal_and_rename_and_overflow")` |
| PM-R01-S05 | 删歌单及其成员关系，不删歌曲文件/记录/其他歌单成员 | `application/playlist.rs` `delete_owns_playlist_and_members_but_not_songs_or_other_playlists` | `COREC("application::playlist::tests::delete_owns_playlist_and_members_but_not_songs_or_other_playlists")` |
| PM-R07-S01 | 选择器内新建并选中，确认后追加到新歌单末尾 | `features/playlists/AddToPlaylistDialog.test.tsx` `selects a playlist created from the picker so the original song can be added`、`passes the active root into the inline create dialog` | `REACT("src/features/playlists/AddToPlaylistDialog.test.tsx")` —— 该用例就是「创建后选中并把原歌曲追加」，比只断言"建成了"更贴 `THEN` |
| PM-R07-S02 | 无效名/创建失败时保留选择器上下文与输入，不伪造歌单 | `features/playlists/PlaylistNameDialog.test.tsx` `rejects a name longer than 40 graphemes without calling create_playlist`、`rejects a create when no active root is available` | `REACT("src/features/playlists/PlaylistNameDialog.test.tsx")` |
| PLL-R01-S01 | 发布到 `media/<艺人>/<艺人> - <标题>.<ext>`，歌词同目录同基名 | `application/import/tests/execution.rs` `default_target_is_artist_folder_with_artist_minus_title` | `COREC("default_target_is_artist_folder_with_artist_minus_title")` |
| PLL-R01-S02 | 控制面不可写时不作为可管理资料库启用，只读仍可浏览 | `infrastructure/filesystem/control_plane.rs` `control_plane_usable_detects_writable_and_readable`、`application/root_switch.rs` `read_only_root_activates_readonly_and_disables_writes` | `COREC("control_plane_usable_detects_writable_and_readable")` |
| PLL-R02-S01 | 无需另一设备 SQLite 即可识别逻辑 ID/版本/对象 UUID 与载荷 | `infrastructure/filesystem/control_plane.rs` `manifest_round_trips_atomically`、`record_round_trips_through_shard_path` | `COREC("manifest_round_trips_atomically")` |
| PLL-R02-S02 | 只含 `media/...` 相对路径，不含盘符/主目录/DB 位置/认证 | `infrastructure/sqlite/tests/schema.rs` `sync_payloads_carry_no_absolute_paths` | `COREC("sync_payloads_carry_no_absolute_paths")` |
| PLL-R02-S03 | 同步资料不含 `echo/tmp/<op>/` 与任何暂存文件 | `infrastructure/filesystem/control_plane.rs` `tmp_files_are_ignored_as_records`、`staging.rs` `staging_dir_is_emitted_as_echo_tmp_with_matching_marker` | `COREC("tmp_files_are_ignored_as_records")` |
| PLL-R03-S01 | 新设备重建一致的 UUID/喜欢/歌单/成员顺序并可播放 | `application/continuation/tests.rs` `continuation_restores_songs_favorites_playlists_and_member_order` —— **更正**：原登记为整模块 `COREC("application::restore")`，该过滤器下只有断言**歌曲**投影的用例，而本行写的是「喜欢/歌单/成员顺序」，属于「用成功路径测试冒充未覆盖的验收」。现指向真正断言歌单数量、成员顺序与 `is_favorite` 恢复的用例 | `COREC("application::continuation::tests::continuation_restores_songs_favorites_playlists_and_member_order")` |
| PLL-R03-S02 | 资料先到、媒体未到时显示不可用并保留 UUID，媒体到达后恢复且不产生第二首 | `application/portable.rs` `restore_projects_records_and_keeps_missing_without_media` | `COREC("restore_projects_records_and_keeps_missing_without_media")` |
| PHA-R01-S02 | 回归质量门：Rust/前端/格式/静态检查与登记场景均通过 | `scripts/verify/ci-governance.mjs` —— 含 `cargo test --workspace --all-targets --all-features`、前端构建、覆盖率、三方对账、命令棘轮、纯净性、注入证明共 14 道 | 直接写字符串 `"node scripts/verify/ci-governance.mjs"`（**该门禁很重**，全量场景会因此明显变长；若不可接受，可改为指向 `CHECK("1.2")` 只覆盖前端那一半） |

## 3. 丙类：有测试但覆盖不全 —— **已按「先补测试再登记」收口**（4 条）

这四条的 spec `THEN` 明确要求**失败路径或视觉语义**，而当时存在的测试只覆盖了正常路径 / 逻辑侧。
**直接登记就是用成功路径测试冒充失败路径验收 —— 属于假绿，比留人工更糟。** 所以先补用例，再登记。

| 场景 ID | spec 要求（缺的那部分加粗） | 当时只有 | 已补的用例（现登记指向它） |
|---|---|---|---|
| **PM-R06-S01** 成员移除失败 | 失败时**成员留在原位**、**导航计数不变**、显示移除失败信息 | `PlaylistsView.tsx:163` 有实现（`移除歌曲失败，请重试`）但**无任何测试** | `PlaylistsView.test.tsx` — `keeps the member in place and reports it when 从歌单移除 fails`（断言：失败文案、两个成员顺序不变、`playlist_members` 只取过一次、`onLibraryChanged` 未被调用） |
| **PM-R06-S02** 入队失败 | 失败时**不显示"已加入播放队列"**、保持服务端确认状态、**显示失败信息** | `SongMenu.test.tsx` 只断言 `onEnqueue` 回调被调用（连 bridge 都没碰） | `PlaylistsView.test.tsx` — `never claims 加入播放队列 succeeded when the enqueue fails`（断言：失败文案、`/已将/` 不出现、`queue_command` 确实被发过） |
| **DP-R12-S01** 非沉浸切换随机播放 | 随机图标**保持中性样式**、界面以**可读语义**表明已选中 | `player/queue/tests.rs` `shuffle_bag_*`（**顺序逻辑**，非样式） | `PlayerBar.test.tsx` — `switches to 随机播放 with a neutral button and leaves the queue alone`（断言：`aria-pressed=true` + 可读名 = 随机播放、class 恰为 `control`、唯一发出的控制指令是 `mode:shuffle`、当前曲不变） |
| **DP-R12-S02** 主题切换保持随机 | 随机图标**不得变为新主题强调色**、模式与队列不受影响 | `player/coordinator/tests.rs` `play_context_keeps_the_user_mode`（**模式不被改**，非颜色） | `PlayerBar.test.tsx` — `keeps 随机播放 neutral while the theme changes`（断言：换主题前后 `outerHTML` 逐字不变、不带 `active`）+ `keeps the theme accent on the 喜欢 heart, never on the mode button` |

**DP-R12 的"颜色"是怎么被证明的**：`styles/player.css` 里带强调色的选中态**只有一个** ——
`.control.active { color: var(--accent) }`，而它只被 喜欢 心形按钮使用；`#playback-mode` 在样式表里
只设了 `flex` / `overflow`，**没有任何颜色规则**，按钮的 `className` 也是无条件的 `"control"`。
所以「随机按钮永远不带 `active`」与「随机按钮不会被刷成主题强调色」是同一件事，测试就把这两半在
**同一次渲染里**钉住（心形带 `active`、随机按钮不带）。这样断言的是真实类契约，而不是去读样式表文本
（vitest 会把 `.css` 一律替换成空模块，`?raw` / `?inline` 都拿不到内容，实测均为 `""`）。

> 这四条都做过**正控**（把违规注进去、确认按名失败、再逐字节还原）：
> 给 `#playback-mode` 加上 `active`、把入队失败也报成功、把移除失败也当成功刷新列表 —— 补的用例全部
> 按名变红，而文件里原有的 6 / 27 条用例保持绿色，证明它们不是空断言。

## 4. 乙类：必须人工举证（9 条）

判据：`THEN` 的核心是**真实进程存活 / 真实托盘或菜单栏入口 / 真实应用身份外观 / 操作系统显式打开文件 /
真实渲染尺寸**，任何单测都无法证明。操作步骤见 §5。

| 场景 ID | 为什么不能自动化 |
|---|---|
| DAS-R12-S01 关闭窗口后继续后台播放 | 要证明**进程与平台入口在窗口隐藏后仍存在** |
| DAS-R12-S02 从后台入口退出 | 要**真实点击菜单栏/托盘菜单**并观察进程结束、入口消失 |
| DAS-R13-S01 可见应用身份一致 | 要观察窗口标题、菜单栏入口、Dock/任务栏、bundle 名的**真实外观** |
| DP-R03-S04 沉浸模式尺寸一致 | 要**真实布局度量**（面板宽度、行高、封面尺寸、可视 8 行），jsdom 无布局引擎 |
| DP-R08-S03 文件关联覆盖普通恢复 | 要**操作系统显式打开文件**触发冷启动路径 |
| DP-R10-S01 关闭窗口退出 | 同 DAS-R12-S01，反向偏好 |
| DP-R10-S02 关闭窗口后台运行 | 同 DAS-R12-S01 |
| DP-R10-S03 托盘控制 | 要**真实托盘交互**（播放控制/查看状态/显示窗口/退出） |
| PHA-R01-S01 歌单与队列日常流程 | 一期验收基线：**验收者**在真实应用里走完 6 步端到端旅程并逐项确认 |

> 注：这 8 条的**可测试内核**（CloseBehavior 偏好持久化、`WindowState::clamp_to_visible`、
> 状态菜单文案映射）已由 `task-9.6.mjs` / `task-9.3.mjs` / `platform/status_menu.rs` 覆盖。
> 人工举证补的是**外壳接线与真实外观**那一层，两者不重复。

## 5. 乙类人工步骤（操作者照做）

通用前置：`pnpm --dir apps/desktop build`，以**裸二进制**启动（`cargo build` 后直接跑 `target/debug/Echo`），
确保嵌入的是当前前端；准备 1 首 ≥30s 的本地歌曲。每条完成后写 §6 的 log。

### DAS-R12-S01 关闭窗口后继续后台播放
1. 设置里把「关闭主窗口时」设为**保持后台运行**；开始播放。
2. 点击窗口关闭按钮 → 主窗口应**隐藏**（不是退出）。
3. 观察菜单栏（macOS）/ 系统托盘（Windows/Linux）出现 Echo 入口。
4. 确认音频**仍在播放**。
- 判定：窗口隐藏 + 进程存活 + 入口存在 + 继续出声。证据：入口与隐藏窗口的截图各一。

### DAS-R12-S02 从后台入口退出
1. 按上一条进入后台运行状态。
2. 从菜单栏/托盘入口选择**退出**。
3. 观察：进程结束、入口消失、**不残留无法操作的后台入口**。
4. 重新启动，确认会话可恢复。
- 判定：干净退出 + 会话已保存。证据：退出前后截图 + 重启后界面截图。

### DAS-R13-S01 可见应用身份一致
1. 启动 Echo，逐处查看：窗口标题、菜单栏/托盘入口名、Dock/任务栏名、`.app` bundle 名。
2. 全部应为 `Echo`。
- 判定：四处一致。证据：逐处截图（可拼一张）。

### DP-R03-S04 沉浸模式尺寸一致
1. 普通模式下打开播放队列面板，记录：面板宽度、歌曲行高、封面尺寸、可视行数。
2. 切到沉浸模式并打开同一面板，记录同样四项。
3. 再切回普通模式复核。
- 判定：四项数值**一致**，仅面板在窗口内的位置可不同；可视行数上限 8。证据：两模式截图 + 测量值。

### DP-R08-S03 文件关联覆盖普通恢复
1. 先正常播放一首到中途，退出 Echo（制造可恢复会话）。
2. 在文件管理器中**双击**一个音频文件（或用「打开方式 → Echo」）。
3. 观察：安全初始化完成后，**该显式打开的文件成为当前项并自动播放**，普通恢复不得抢回当前播放。
- 判定：当前项 = 显式打开的文件。证据：截图 + 说明恢复会话内容。

### DP-R10-S01 关闭窗口退出
1. 偏好设为**退出应用**；开始播放。
2. 点关闭按钮 → 应**停止播放并结束进程**，且**不保留**后台托盘控制。
- 判定：进程结束、无托盘。证据：活动监视器/任务管理器截图。

### DP-R10-S02 关闭窗口后台运行
1. 偏好设为**保持后台运行**；点关闭按钮。
2. 应保持播放与进程，并显示对应平台的菜单栏状态项/系统托盘入口。
- 判定：同 DAS-R12-S01。证据：截图。

### DP-R10-S03 托盘控制
1. 从菜单栏/托盘依次执行：播放/暂停、下一首、查看状态、显示主窗口、退出。
2. 每步核对与主窗口显示状态一致。
- 判定：五项操作均生效且状态一致。证据：逐项截图或录屏 + 步骤记录。

### PHA-R01-S01 歌单与队列日常流程（一期验收基线）
1. 在可写活动资料库中：创建歌单 → 添加歌曲 → 播放其中一首 → 切换播放模式 → 上一首回退 → 退出并重启。
2. 每步核对：**只在提交成功后更新界面**；当前队列项持续置顶；三天内历史可回退；重启后资料库/歌单/会话可理解地恢复。
- 判定：6 步全部符合。证据：逐步截图 + 每步说明。

## 6. attestation log 格式（门禁实际校验的字段）

门禁 `check-native-attestation.mjs` 读 `artifacts/native-attestations/<ID>.log`，要求：

- 非空；
- 以下 **7 个字段各占一行、行首即为字段名**（`^field:`，多行模式）：
  `os`、`versions`、`desktop-env`、`operator`、`result`、`evidence-path`、`date`；
- `result:` 必须匹配 `pass` / `pass (...)` / `ok` / `ok (...)`（大小写不敏感）。

可直接复制的模板（把 `<...>` 换成实测值）：

```
os: macOS 14.5 (arm64)
versions: Echo 0.1.0 @ 6866700 / mpv <ver> / Node <ver>
desktop-env: Aqua, 27in 5K @2x, zh-CN
operator: <姓名>
result: pass (4/4 steps observed)
evidence-path: artifacts/native-attestations/evidence/<ID>/
date: 2026-09-18

steps:
1. ...
```

> `result:` 未填成 pass/ok 会红；缺任一字段会红；文件不存在会红并指名。

自检（写完立即验）：
```bash
node scripts/verify/checks/check-native-attestation.mjs DAS-R12-S01
```

## 7. 执行顺序与**实际执行记录**

1. **甲类 28 条** —— ✅ 已在 `scripts/verify/scenario-commands.mjs` 的 `COMMANDS` 表登记
   （**必须改这张表**，`manifest.json` 是生成物，手改会被静默抹掉），随后
   `node scripts/verify/gen-scenario-manifests.mjs --write` 重生三棵树。
2. **丙类 4 条** —— ✅ 先补用例（§3 表），再登记；四条都用**正控**验过会失败。
3. **乙类 9 条** —— ⏳ 待操作者按 §5 执行、按 §6 写 log。这是当前唯一的阻塞面。
4. 回归（顺序有讲究，**一道都不能漏**）：

   ```bash
   node scripts/verify/reconcile-scenarios.mjs         # 三方 218 = 218 = 218
   node scripts/verify/validate-scenario-commands.mjs  # 每条命令真能命中测试 ← 最容易漏
   node scripts/verify/validate-scenario-manifests.mjs
   node scripts/verify/check-scenario-churn.mjs        # 重复度棘轮
   pnpm verify:scenario -- --all                       # 期望：只剩未填 log 的乙类 9 行红
   ```

   `validate-scenario-commands.mjs` 才是"命令真能命中测试"的那道门禁。本次施工中
   `LE-R07-S01` 的 `-- --ignored` 就是被它抓出来的：校验器把 `-- --ignored`（cargo 的测试二进制
   分隔符）当成了 filter 的一部分 → 0 命中。**上一轮只跑了 reconcile 与 churn，漏了它**，
   于是这个红被带进了一个已提交的 commit。修法是让校验器剥离 `-- <args>`，而不是删掉
   `-- --ignored`（删了就退回"bench 跑 0 个测试"的假绿）。

### 7.1 登记 32 条时**又**暴露的两个门禁缺陷（已修）

这两条都不是"缺检查"，而是"检查在，但被绕过了"——和本文 §1 是同一类问题，所以留在这里。

1. **`validate-scenario-manifests.mjs` 的引号往返不闭合**：生成器 `yamlEscape` 会把命令里的
   `"` 转义成 `\"`，而校验器 `scalar()` 只 `replace(/^"|"$/g,"")` **剥掉外层引号、不解转义**。
   于是命令里只要出现一个双引号（本次 DP-R12/PM-R06 用了 vitest 的 `-t "<测试名>"`），
   比对结果就是 `\"` vs `"` → 报**幻影 `command drift`**。它同时把注入套件的
   `scenario-manifests/dropped-required-field` 打成红：**正控（未改动树）自己先失败**。
   修法是让 `scalar()` 真正解转义（`\\` → `\`、`\"` → `"`）。这条在本次之前从未触发过，
   因为此前没有任何一条命令带引号——**"从没红过"和"不会红"是两件事**。
2. **`validate-scenario-commands.mjs` 不校验 vitest 的 `-t` 选择器**：`cargo` 侧它会验"filter 至少命中
   1 个测试"，但 vitest 侧原来只验文件存在。一旦允许 `-t "<名字>"`，写错名字就会选 0 个测试——
   正是该校验器存在的理由（"the release gate must never pass a scenario against zero tests"）。
   已补：把 `-t` 后的名字拿回文件里 `.includes()` 核对，并做了正控（换成不存在的名字 → 指名报错、退出 1）。

### 7.2 churn 口径：登记这些行会**推高**重复度，别靠调阈值解决

分母是**不同命令数**：① 每条场景用互不相同的命令 → 分母增大、重复度**下降**；
② 多条场景共用同一命令 → 分母不变、重复度**上升**。

本次的真实教训：4 条丙类最初都登记成"整文件"级命令（PM 两条共用 `PlaylistsView.test.tsx`、
DP 两条共用 `PlayerBar.test.tsx`），而且**替换掉了 4 条互不相同的 attestation 字符串** →
不同命令数 123 → 120，重复度 **1.77x → 1.82x，超过冻结阈值 1.8x**。

处置**不是**把阈值从 1.8 抬到 1.85，而是给这四条各自加上 `-t "<测试名>"` 收窄到**唯一证明它的那个用例**
（新增 `REACT_T(f, name)` helper）：不同命令数回到 123，重复度回到 **1.77x**，阈值一字未动，
而且顺带把"一条用例坏掉同时影响两条场景"的簇也拆开了——这正是棘轮想量到的东西。

> 将来逼近阈值时，优先把**语义仍然贴切**的重复命令收窄到具体测试名，
> **不要**为了凑数把命令改成不相关的测试，也不要调阈值。

### 7.3 甲类登记的**语义复核**（本次施工做了，但不是全部）

7.1 修的两条都是"门禁被绕过"；这一条是**门禁本来就管不到的地方**：
`validate-scenario-commands.mjs` 只能证明"命令命中 ≥1 个真测试"，证明不了"命中的测试就是这条
`THEN`"。所以命令写对了、测试也存在，仍可能是在验另一件事——**这正是 DP-R11-S01 那种错的生存空间。**

本次做的筛选（可重跑，思路是"拿判定依据当线索"）：把 §2 每一行里提到的测试名个数，与该条命令
**实际命中**的测试个数对比，出现的 11 行再逐条对着 spec 的 `THEN` 原句判：

```bash
cargo test -p echo-desktop --all-features -- --list > /tmp/desk.txt
cargo test -p echo-core    --all-features -- --list > /tmp/core.txt
# 然后对每条命令取 filter 子串，统计 /tmp/*.txt 里 includes(filter) 的行数，
# 与该行 §2 提到的测试名个数比较。
```

结果分三类：

- **真缺陷，已改指（3 条）**：
  - `DP-R11-S01` —— 命令指向的用例只证明"单条目队列重播"。已新增
    `player::queue::tests::previous_in_loop_walks_the_context_backwards_and_wraps_to_the_tail`
    （断言：切前一项 + 首项回绕 + 按新位置重新投影 + 不消费优先区）并改指它；该用例做过正控
    （把回绕改成 `saturating_sub` → 按名失败 → 逐字节还原）。
  - `DP-R04-S06` 定位和音量 —— `THEN` 是"进度条 / 音量滑块 / 静音"三件事，原来只指 seek 那条。
    改指 `DESK("_through_coordinator")`，该子串**恰好**命中 seek / set_volume / unmute 三条
    coordinator 级用例（已用 `-- --list` 核过是 3 条，不多不少）。
  - `LL-R02-S05` 监听外部新增和修改 —— `THEN` 的核心是"事件稳定后更新**记录与索引**"，
    原来只指 coalescer 归一化那条（那是"事件被合并"，不是"记录被更新"）。改指
    `COREC("application::watch::tests::")`（8 条：settle 收敛 / rename 保 UUID / publish 复用
    journal id / overflow 退化为重扫 …）。"手动重扫收敛"那半由 `application::scan::tests::*_converge_after_rescan` 覆盖。
- **看着可疑、读过后判定够用（单测即可证明整条 `THEN`）**：`DP-R02-S05`、`DP-R13-S01`、`PLL-R02-S03`。
- **需要两条测试才能覆盖、但一条 cargo filter 无法同时命名（残留不精确）**：`DP-R02-S07`、
  `DP-R11-S03`、`LL-R02-S04`、`PLL-R01-S02`、`PLL-R02-S01`。这些条目的主用例已覆盖 `THEN` 的主要子句，
  次要子句由 §2 列出的另一条测试证明；cargo 只接受**一个**子串过滤器，两条测试名不共享子串时无法合并。
  ⚠️ 这**不是**"已完整验收"，而是"主路径已自动验收 + 次子句仅有具名证据"。

> **残留不精确的正确解法是任务 7.7**（按模块聚合 + 每个场景追溯到一个具体测试名）——这正是它被移出
> 本 change、另开一个的原因。在那之前，这 5 条的不精确是**已知且已登记**的，不是被忽略的。

> ⚠️ 这个筛选是**启发式**：线索来自我给 §2 写的判断，不是直接来自 spec。它能抓出"登记范围窄于自己
> 的判断"，抓不出"判断本身错了"（`DP-R11-S01` 就属于后者，是靠读正文才发现的）。
> 唯一可靠的复核方式仍是**逐条读 spec 的 `THEN` 子句 + 读用例正文**。

> ⚠️ **发现但本次未修的既有错配**：`DP-R03-S01/S02/S03` 三个场景是「队列显示歌曲信息 /
> 队列超过八首 / 队列不超过八首」，命令却是 `mode_switch_keeps_current_item`、
> `next_advances_in_order`、`previous_moves_back_within_5_seconds` —— 与队列面板展示无关。
> 修它要逐条重新论证 R03 该指向哪个投影/面板测试，属独立工作。

> ⚠️ **孤儿生成物（同一类问题的另一面）**：`tests/scenarios/DP-R07-S03.yaml` 来自**已归档**的
> 0.1.0 change，不在活动 registry 里。`gen-scenario-manifests.mjs --write` **不清理**不再登记的产物，
> 也没有门禁按目录枚举去发现它——`validate-scenario-manifests.mjs` 会为它打一行 `warning ... is not in
> the current registry` 然后继续（这是刻意的：它负责字段有效性，登记归属归 reconcile）。
> 结果就是"多出来的生成物"既不会被删也不会被查。要不要删属于独立决定（它是一份 0.1.0 的归档证据）。
