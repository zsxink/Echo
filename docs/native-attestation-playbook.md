# 41 个 attestation 场景：判定与处置

> 在 HEAD `7927f4c` 上实测得出。本文档回答两件事：
> ① 为什么这 41 个场景会落到 `check-native-attestation.mjs`；
> ② 逐条判定它们是「已有针对性测试可登记」「有测试但覆盖不全，先补测试」还是「确实必须人工举证」。
>
> 复验命令（任何数字都可重跑）：
> ```bash
> node --input-type=module -e '
> import { COMMANDS } from "./scripts/verify/scenario-commands.mjs";
> import { allScenarioIds } from "./scripts/verify/spec-scenarios.mjs";
> const un = allScenarioIds().filter(d => !(d.id in COMMANDS));
> console.log(un.length);'      # → 41
> ```

## 0. 结论摘要

| 类 | 条数 | 含义 | 处置 |
|---|---|---|---|
| **甲** 可登记为自动化 | **29** | 存在针对性测试，且覆盖 spec 的 `THEN` 全部要点 | 改 `scenario-commands.mjs` 登记真命令 |
| **丙** 有测试但覆盖不全 | **3** | 存在相关测试，但只覆盖正常路径/部分要点，登记即假绿 | **先补测试**，再登记 |
| **乙** 必须人工举证 | **9** | 核心是真实 OS 交互（进程存活 / 托盘 / 应用身份 / 真实打开文件 / 真实布局 / 端到端验收） | 操作者在真机执行本文 §5 的步骤并写 log |

29 + 3 + 9 = 41。乙类 9 条正是 `task-9.6.mjs` 注释所说的 *"cross-platform native validation is
deferred to the platform Gate"*，也与 `scenario-commands.mjs` 顶部注释列举的 `live tray gesture /
OS file-open integration / the 3-platform smoke itself` 一一对应 —— **注释描述的那条分支确实存在，
只是它同时被当成了"未登记"的兜底，把另外 32 条也吸了进去。**

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

## 2. 甲类：可登记为自动化（30 条）

命令形式沿用现有 helper（`scenario-commands.mjs`）：
`COREC(f)` = `cargo test -p echo-core --all-features <f>`；`DESK(f)` = `cargo test -p echo-desktop --all-features <f>`；
`REACT(f)` = `pnpm --filter @echo/desktop test -- --run <f>`。

| 场景 ID | 要验的行为（spec `THEN` 摘要） | 现有针对性测试 | 建议 |
|---|---|---|---|
| DP-R02-S05 | 下一首播放 A、B、C 顺序，普通待播保持相对顺序 | `player/queue/tests.rs` `play_next_lane_is_fifo_and_projection_precedes_normal_entries`、`insert_next_goes_immediately_after_current` | `DESK("player::queue::tests::play_next_lane_is_fifo_and_projection_precedes_normal_entries")` |
| DP-R02-S06 | 切歌/自然结束/历史回退后列表首项=当前项，回绕项在尾部 | `player/queue/tests.rs` `view_projection_keeps_current_first_and_includes_loop_wrap` | `DESK("player::queue::tests::view_projection_keeps_current_first_and_includes_loop_wrap")` |
| DP-R02-S07 | 清空待播但不删歌曲/歌单成员，当前曲继续播 | `player/queue/tests.rs` `clear_pending_keeps_current_and_history`、`clear_pending_keeps_only_current_even_after_loop_wrap` | `DESK("player::queue::tests::clear_pending_keeps_current_and_history")` |
| DP-R04-S04 | 单曲循环自然结束重播同项；手动下一首忽略单曲重复 | `player/coordinator/tests.rs` `repeat_one_repeats_current_on_end`、`explicit_next_advances_repeat_one_without_changing_selected_mode` | `DESK("player::coordinator::tests::repeat_one")` |
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
| PM-R06-S01 | 移除失败时成员留在原位、计数不变、显示失败 | `features/playlists/AddToPlaylistDialog.test.tsx` `keeps the authoritative picker open when membership commit fails` | `REACT("src/features/playlists/AddToPlaylistDialog.test.tsx")` |
| PM-R07-S01 | 选择器内新建并选中，确认后追加到新歌单末尾 | `features/playlists/PlaylistNameDialog.test.tsx` `creates the playlist with the active root and trimmed name`、`AddToPlaylistDialog.test.tsx` `selects a playlist created from the picker so the original song can be added` | `REACT("src/features/playlists/PlaylistNameDialog.test.tsx")` |
| PM-R07-S02 | 无效名/创建失败时保留选择器上下文与输入，不伪造歌单 | `features/playlists/PlaylistNameDialog.test.tsx` `rejects a name longer than 40 graphemes without calling create_playlist`、`rejects a create when no active root is available` | `REACT("src/features/playlists/PlaylistNameDialog.test.tsx")` |
| PLL-R01-S01 | 发布到 `media/<艺人>/<艺人> - <标题>.<ext>`，歌词同目录同基名 | `application/import/tests/execution.rs` `default_target_is_artist_folder_with_artist_minus_title` | `COREC("default_target_is_artist_folder_with_artist_minus_title")` |
| PLL-R01-S02 | 控制面不可写时不作为可管理资料库启用，只读仍可浏览 | `infrastructure/filesystem/control_plane.rs` `control_plane_usable_detects_writable_and_readable`、`application/root_switch.rs` `read_only_root_activates_readonly_and_disables_writes` | `COREC("control_plane_usable_detects_writable_and_readable")` |
| PLL-R02-S01 | 无需另一设备 SQLite 即可识别逻辑 ID/版本/对象 UUID 与载荷 | `infrastructure/filesystem/control_plane.rs` `manifest_round_trips_atomically`、`record_round_trips_through_shard_path` | `COREC("manifest_round_trips_atomically")` |
| PLL-R02-S02 | 只含 `media/...` 相对路径，不含盘符/主目录/DB 位置/认证 | `infrastructure/sqlite/tests/schema.rs` `sync_payloads_carry_no_absolute_paths` | `COREC("sync_payloads_carry_no_absolute_paths")` |
| PLL-R02-S03 | 同步资料不含 `echo/tmp/<op>/` 与任何暂存文件 | `infrastructure/filesystem/control_plane.rs` `tmp_files_are_ignored_as_records`、`staging.rs` `staging_dir_is_emitted_as_echo_tmp_with_matching_marker` | `COREC("tmp_files_are_ignored_as_records")` |
| PLL-R03-S01 | 新设备重建一致的 UUID/喜欢/歌单/成员顺序并可播放 | `application/restore.rs` `restore_is_idempotent_across_repeats`、`restore_projects_records_and_keeps_missing_without_media`、`portable.rs` `project_songs_upserts_with_stable_uuid` | `COREC("application::restore")` |
| PLL-R03-S02 | 资料先到、媒体未到时显示不可用并保留 UUID，媒体到达后恢复且不产生第二首 | `application/portable.rs` `restore_projects_records_and_keeps_missing_without_media` | `COREC("restore_projects_records_and_keeps_missing_without_media")` |
| PHA-R01-S02 | 回归质量门：Rust/前端/格式/静态检查与登记场景均通过 | `scripts/verify/ci-governance.mjs` —— 含 `cargo test --workspace --all-targets --all-features`、前端构建、覆盖率、三方对账、命令棘轮、纯净性、注入证明共 14 道 | 直接写字符串 `"node scripts/verify/ci-governance.mjs"`（**该门禁很重**，全量场景会因此明显变长；若不可接受，可改为指向 `CHECK("1.2")` 只覆盖前端那一半） |

## 3. 丙类：有测试但覆盖不全 —— **登记前必须先补测试**（3 条）

这三条的 spec `THEN` 明确要求**失败路径或视觉语义**，而现有测试只覆盖了正常路径 / 逻辑侧。
**如果直接登记，就是用成功路径测试冒充失败路径验收 —— 属于假绿，比留人工更糟。**

| 场景 ID | spec 要求（缺的那部分加粗） | 现有测试只覆盖 | 缺口 |
|---|---|---|---|
| **PM-R06-S02** 入队失败 | 失败时**不显示"已加入播放队列"**、保持服务端确认状态、**显示失败信息** | `features/library/SongMenu.test.tsx` `enqueue appends to the queue via onEnqueue`（**成功路径**） | 缺「入队命令 reject 时 UI 不给成功反馈」的用例 |
| **DP-R12-S01** 非沉浸切换随机播放 | 随机图标**保持中性样式**、界面以**可读语义**表明已选中 | `player/queue/tests.rs` `shuffle_bag_*`（**顺序逻辑**，非样式） | 缺 PlayerBar 随机按钮的样式/`aria-pressed` 断言 |
| **DP-R12-S02** 主题切换保持随机 | 随机图标**不得变为新主题强调色**、模式与队列不受影响 | `player/coordinator/tests.rs` `play_context_keeps_the_user_mode`（**模式不被改**，非颜色） | 缺「主题切换后按钮 class 不含 accent 色」的断言 |

处置建议：为这三条各补一个用例（前一个在 `SongMenu.test.tsx`，后两个在 `PlayerBar.test.tsx`），
再登记。补测试的量很小，且堵住的是**真实的验收漏洞**。

## 4. 乙类：必须人工举证（8 条）

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

通用前置：`pnpm --dir apps/desktop build`，以**裸二进制**启动（`cargo build` 后直接跑 `target/debug/echo`），
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

## 7. 执行顺序（甲乙丙各一条路）

1. **甲类 30 条**：在 `scripts/verify/scenario-commands.mjs` 的 `COMMANDS` 表登记 §2 的建议命令
   （**必须改这张表**，`manifest.json` 是生成物，手改会被静默抹掉），然后
   `node scripts/verify/gen-scenario-manifests.mjs --write`。
2. **丙类 3 条**：先在对应测试文件补用例，再按甲类登记。
3. **乙类 8 条**：操作者按 §5 执行，按 §6 写 log。
4. 回归：`node scripts/verify/reconcile-scenarios.mjs`（三方 218 = 218 = 218）、
   `node scripts/verify/check-scenario-churn.mjs`（重复度棘轮，当前 1.76x）、
   最后 `pnpm verify:scenario -- --all` —— 红的应当**只剩未填 log 的乙类行**。

> ⚠️ 甲类登记会**降低** `check-scenario-churn` 的分母（同一命令被更多场景引用时重复度上升）。
> 登记后必须重跑该门禁；若越过 1.8x 阈值，应改用更精确的 filter 或按 §2 的合并口径调整。
