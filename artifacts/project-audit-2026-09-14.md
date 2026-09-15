# Echo 0.1.0 项目复核报告

- 日期：2026-09-14
- 范围：`openspec/changes/release-0-1-0-desktop-player/tasks.md` 全部 108 个任务 + `wire-desktop-system-dialogs` change
- 口径：**多端只算 macOS**（9.8 / 13.2–13.5 / 13.7 的三平台要求按 macOS 判定）
- 方法：读 tasks.md + `scripts/verify/manifest.json` + 三路独立代码审查 + 实跑
  （`cargo test --workspace --all-features`、`pnpm typecheck/test`、13.2/13.3/13.9/13.6/12.8 check 脚本、
  `run-task` 未知 ID 探测）

**结论：0.1.0 尚未可交付。** 6 个任务真没做完，2 个已勾任务实测红，9 个已勾任务无验证登记，
9 处实现/验收错误，另有 154 项改动全部未提交。

---

## 一、真未完成（6 个）

| 任务 | 判定 | 证据 |
|---|---|---|
| **9.8** 三平台候选安装包 | 未做 | `apps/desktop/src-tauri/tauri.conf.json` 无任何 `bundle.targets` / dmg / msi / appimage / deb 配置 |
| **13.4** watcher 乱序/权限撤销/只读根矩阵 | 未做 | 无 check 脚本、无 manifest 登记 |
| **13.5** 真实 libmpv + 文件关联 + 托盘/媒体键 + reveal/回收站人工冒烟 | 未做 | 无人工证据；按 macOS-only 口径仍需做一次 macOS 冒烟 |
| **13.7** CI 构建三平台安装包 | 未做 | `.github/workflows/ci.yml` 无打包产物步骤 |
| **13.9** 全量场景 + PRD A1–A14 | **被阻断** | `scripts/verify/reconcile-scenarios.mjs` 抛 `no prefix for spec area 'sync-foundation'` |
| **13.10** README/文档更新 + `openspec validate --strict` | 未做 | 仓库无 README；`docs/acceptance/platform-gate-handoff.md` 不存在 |

### 已实现、macOS 实测通过，但没登记也没勾选（补登记即可）

| 任务 | 实跑结果 |
|---|---|
| **13.2** 原生 E2E 临时库流程 | `native_e2e` 1 passed；证据 `artifacts/native-e2e-13.2.txt` |
| **13.3** 故障注入 ×2 恢复矩阵 | 全绿；报告 `artifacts/fault-injection-13.3.md` |

两者 check 脚本存在（`scripts/verify/checks/task-13.2.mjs` / `task-13.3.mjs`）但**未登记进 manifest**，
因此 `pnpm verify:task -- 13.2` 报 unknown task id，永不参与回归。

### wire-desktop-system-dialogs change

代码**已真实接线**（原 design.md 记录的假实现已修复）：
`main.rs:109-123` 走 `AppServices::with_runtime` + `TauriDialogs`，`main.rs:361-362` 注册
`tauri-plugin-dialog` / `tauri-plugin-opener`，`dialogs.rs:35-79` 三方法均为原生调用。
但 `openspec/changes/wire-desktop-system-dialogs/tasks.md` **15 项全部未勾选**，真机验证无证据。

---

## 二、已勾选但实测红（2 个）

| 任务 | 失败 |
|---|---|
| **13.6** 完整质量门 | `cargo fmt --all -- --check` 失败：`crates/echo-core/src/infrastructure/sqlite/mod.rs:66` 一处 `use query::{...}` 需合并为单行 |
| **12.8** 覆盖率 ≥90% | `cargo llvm-cov -p echo-core --all-features --fail-under-lines 90` 编译失败：`application/catalog.rs:229` 与 `:244` 的 `RECENT_VIEW_LIMIT` 在 `--cfg=coverage` 构建下解析不到（**非 coverage 的 `cargo test -p echo-core --all-features --lib` 355 passed 全绿**，只在 coverage 构建路径暴露） |

---

## 三、已勾选但无验证登记（9 个，违反 tasks.md 头部约定）

tasks.md 第 4 行明写「命令未登记…任务不得勾选」。执行器实测：

```
$ node scripts/verify/run-task.mjs -- 8.9 8.10 8.11 8.12 10.1 10.2 10.3 10.4 10.5
error: unknown task id: 8.9   ... （5 个全报 unknown）
```

`scripts/verify/manifest.json` 的 `tasks` 数组在 8.8 后直跳 9.1、在 9.7 后从 10.6 起，
缺 8.9/8.10/8.11/8.12/10.1/10.2/10.3/10.4/10.5 共 9 个；`checks/` 目录也无对应脚本。
这 9 个任务的 `[x]` 目前没有任何聚合命令支撑。

同样未登记的还有 13.2 / 13.3 / 13.9 与 `task-wire-dialogs.mjs`（脚本存在但永不运行）。

---

## 四、实现错误与验收失实（9 处）

### 1. 8.9 / 8.10 / 8.11 —— 模块 + 单测齐全，生产零接线（最严重）

grep 全 `crates/echo-desktop/src/`：

- `player/session.rs` 的 `snapshot_queue` / `rebuild_queue` / `SessionPersistence` / `StateStoreSession`
  仅自身与单测引用 → **重启丢失播放队列**
- `player/recording.rs` 的 `PlaybackStatsRecorder` / `NullRecorder` 零接线 → `record_playback`
  全仓库只有 sqlite 测试在调 → **`play_count` 永不增长、「最近 100 首」视图恒空**
  （`runtime/services.rs:236` 的 `CatalogQuery::recent_100()` 有数据依赖）
- `player/deletion.rs` 的 `DeletionCoordinator` 被绕过：`runtime/services.rs:404` 的 `delete_song`
  直接 `DeleteSongs::new(...).delete()` → 删除正在播放的歌曲**没有 unload 屏障**

这是本项目第 4–6 次出现「规则在 coordinator、接线在 runtime，最后一根线没接」的模式。
**建议**：verify check 应强制断言「coordinator 公开的响应式方法必须存在生产调用链」。

### 2. 8.10 内部三处缺陷 + 假绿测试

- `recording.rs:172` `observe_position(&mut self, _position: f64) {}` —— 空实现，seek 未处理
- `recording.rs:177-188` `tick()` 算出 `total` 后 `let _ = total;`，从不累加
- `recording.rs:206` `maybe_record` 在 `playing_since.is_some()` 时直接 return → 播放中永不触发
- 测试全靠 `#[cfg(test)] force_accumulate`（:193）绕开单调时钟，真实时钟零覆盖

### 3. 8.9 blocked 语义未实现

`session.rs:253` `let _ = verdict_by_song; // reserved for caller-authored blocked handling`；
`:229` 把 `Blocked` 与 `Restore` 合并为同一分支 → 「blocked missing 保留」实际等同普通恢复。

### 4. 8.12 跨平台假绿

`player_smoke.rs:210-215` `vendored_libmpv()` 只找 macOS 路径，Linux/Windows 直接 `return`；
测试报绿但零验证。macOS 本机**真实通过**（2 passed，9.71s，time-pos/duration 事件真实到达）。

### 5. 10.4 —— 同步入口真的渲染了（规格违反）

`apps/desktop/src/app/App.tsx:196-206`：

```tsx
<button type="button" className="link-button sync-button" disabled
        title="同步将在后续版本提供" data-testid="sync-button">
  <span className="sync-label">同步</span>
  <span className="status-dot" aria-hidden="true" />
  <span id="sync-label">资料库已同步</span>
</button>
```

违反 10.4「同步入口均不渲染」、13.8「UI 不显示可操作同步入口」、ROADMAP 一期「不包含可操作同步入口」。
「资料库已同步」还是**伪造状态**——根本没有同步功能。（按钮 `disabled`，但规格写的是"不渲染"）

### 6. 13.8 验收失实

`task-13.8.mjs:116-120` 只跑 `SongList.test.tsx` + `PlaylistsView.test.tsx` 后
**无条件**打印 `ok: UI scope-guard ... asserted by component suites`。
`App.test.tsx` 不在列表 → 上述同步按钮从未被任何断言覆盖。
任务完成注释却声称「UI 不显示可操作同步/全选/歌单排序/歌曲编辑入口」。

### 7. task-wire-dialogs.mjs 是纯字符串匹配假绿

只做 `main.includes("TauriDialogs::new")`、`dialogsSrc.includes("blocking_pick_folder")`、
`includes("reveal_item_in_dir")`，把字符串写进注释也能通过；且未注册进 manifest → 零防护。

### 8. 10.2 / 10.3 缺口

- 10.2：「Core query cache」模块不存在（`app/store*` 无文件，注释先于实现）；全 src `useReducer` 零命中
- 10.3：候选扫描的**进度 UI 与 `cancel_scan` 从未渲染/调用**（`status.scanning` 只在类型与测试夹具出现）

### 9. 文档引用失效

`docs/acceptance/PRD-matrix.md` 引用了三个不存在的路径：
`scripts/verify/checks/task-10.5.mjs`、`task-5.8.mjs`、`docs/acceptance/platform-gate-handoff.md`。

---

## 五、健康的（实测通过）

- `cargo test --workspace --all-features` 全绿：echo-desktop 210 passed、native_e2e 1 passed、
  player_smoke 2 passed（9.71s，真实 libmpv）、privacy 3 passed；echo-core 355 passed
- 前端：`pnpm typecheck` 通过；vitest **20 文件 133 测试全绿**
- `verify:task` 抽查：13.8 / 12.5 / 11.1 通过；12.5 benchmark 达标（搜索 p95 20ms ≤200ms、首屏 p95 2.6ms ≤500ms）
- 架构测试 6 passed（分层/unsafe/依赖边界）；隐私日志测试 7 passed

---

## 六、建议处理顺序

1. **提交通前状态** —— 154 项改动全部未提交，main 分支只到 9.7/12.3，无任何保护。先建提交点。
2. **修 8.9/8.10/8.11 接线** —— 三个死模块直接导致重启丢队列、最近播放恒空、删除无屏障，属用户可见故障。
3. **删同步按钮** —— 一行删除，同时把 `App.test.tsx` 纳入 13.8 的断言范围。
4. **补 manifest 登记** —— 8.9–8.12、10.1–10.5、13.2、13.3、13.9、wire-dialogs，并写真实聚合命令。
5. **修 13.6 / 12.8 红** —— `cargo fmt` 一处；coverage 构建的 `RECENT_VIEW_LIMIT` 导入。
6. **打通 13.9** —— 给 `spec-scenarios.mjs` 的 `AREA_PREFIX` 补 `sync-foundation`，补 traceability 与 yaml。
7. **收尾** —— 13.5 macOS 人工冒烟 → 13.10 文档 + openspec validate → 9.8/13.7 打包（可考虑降到 macOS-only 先发）。

---

## 修复轮结果（2026-09-15 凌晨）

| 问题 | 状态 |
|---|---|
| 同步按钮伪造状态（违反 10.4/13.8） | ✅ 已删除；13.8 现在真断言（源扫描 + App.test.tsx 命名守卫 + 击穿验证过） |
| 8.9 会话持久化零接线 | ✅ 已接线（并行会话完成 save/restore 两半，复验通过） |
| 8.10 播放统计零接线 | ✅ `spawn_stats_recorder` + `CorePlaybackRecorder` 生产装配；真时钟测试替代 force_accumulate |
| 8.11 删除绕过协调器 | ✅ `delete_song_coordinated` + 卸载屏障 + actor Stop 真静音（补 Pause 写入） |
| 8.9–8.12/10.1–10.5 未登记 | ✅ 全部登记，9 个新 check 脚本，逐一验证 PASS |
| 13.2/13.3 未登记 | ✅ 已登记并 PASS |
| 13.6 fmt 红 | ✅ PASS |
| 12.8 覆盖率编译错 | ✅ 确认为构建竞态；❌ 真实缺口：覆盖率 82.76% < 90%，需补测试 |
| 13.9 reconcile 抛错 | ✅ sync-foundation 纳入三方对账（SYN 前缀，+6 场景，166=166=166），13.9 全绿 166/166 |
| PRD-matrix 缺 platform-gate-handoff.md | ✅ 已生成（45 行 native 场景清单） |
| README 缺失 / openspec validate | ✅ README.md 新建；validate 10/10 |
| wire-dialogs 15 项未勾 | ✅ 1.1–2.2、3.1–3.3 已验证勾选；3.4（人工 dev）/3.5（归档）留待人工 |
| capability 死权限 | ✅ `allow-start-dragging` 回退（前端无调用，违反最小权限测试） |

### 仍开放
- 12.8：覆盖率 82.76% < 90% —— 需要补真实测试
- 9.8 安装包 / 13.7 CI 打包 / 13.4 watcher 矩阵 / 13.5 macOS 人工冒烟
- wire-dialogs 3.4/3.5（人工验证与归档）
- 全部改动仍未提交
