## Why

重新打开一个已存在的资料库目录后，歌单与「我的喜欢」全部为空（GitHub issue #1）。用户数据只落在本机 `echo.sqlite`：卸载重装、清除应用数据目录（macOS `~/Library/Application Support/com.zsxink.echo/`）即永久丢失，把资料库目录拷到另一台机器也不会跟随。

`portable-library-layout` 已经定义了目标方向（资料库目录自包含、SQLite 只作可再生加速层），但实现只完成了这个目标的前半段。**活动资料库 `/Users/xian/Music/Echo` 上的实测证据**（数据库快照时间 2026-09-19 23:31:51）：

| 事实 | 实测 |
|---|---|
| `echo/manifest.json` | **不存在**（`InitPortableLibrary` 全仓零生产调用） |
| `echo/records/playlists/`、`playlist-items/`、`overrides/`、`tombstones/`、`play-stats/` | **目录从未创建**（对应构造器零生产调用） |
| `echo/records/songs/` 87 条记录 vs SQLite 87 首歌 | **UUID 交集 0**（记录与当前库完全脱节，两边各 87） |
| 87 条记录的 `media_path` vs `media/` 下音频 | **87 ↔ 87 双射**（记录有磁盘无 0、磁盘有记录无 0） |
| `echo/records/favorites/` 11 条记录 | 全部是 song 记录的子集，其中 **3 条 `is_favorite: true`**；而 SQLite 中 `is_favorite=1` 为 **0** |
| 读回路径 | `RestoreLibrary`、`ProjectPortableRecords` **全仓零生产调用**；`choose_library_root` 只做「派生 root id → 扫描 `media/` → 激活」 |
| 时间线 | DB `library_roots.created_at` = 09-19 23:26:49，87 首歌 `added_at` 全为 23:26:50（一次扫描铸出的全新 UUID）；而 87 条 song 记录 mtime 全为 09-16 |

结论：对象资料被**单向**写了一半，且**没有任何读回或对账**。同一目录被重新扫描时，已由该目录拥有的 87 首歌被铸成 87 个**全新 UUID**（UUID 交集 0），3 个「我的喜欢」随之丢失——正是 `portable-library-layout` 明令禁止的 "不得分配替代 UUID"。而且由于没有 manifest，即使把 `RestoreLibrary` 接上，它也会在第一步以 "no portable manifest present" 拒绝这个真实资料库。

> 附带一条与提案无关但必须记录的事实：核对期间该数据库**正在被使用**（23:26 前后活动根从 `/Users/xian/Music/测试Echo` 变为 `/Users/xian/Music/Echo`，歌曲数 70→87，播放统计 59→2），即用户恰好在此刻复现了 issue 的第一步「清除应用数据目录后重开同一资料库」。本 change 的所有数值都以 23:31:51 的快照与当时的活动根为准。

这不是「缺一个功能」，而是**已批准规格与实现之间的落地差异**（issue 原文亦如此定性），因此本 change 以补齐执行链路为主，并顺带修正规格中与实际对象种类不符之处。

## What Changes

- **控制面存在性与自愈**：受管可写资料库在启用时 MUST 建立 `echo/manifest.json`；已有 `records/` 但缺 manifest 的目录打开时自愈重建，而不是被当成新库扫描。
- **逻辑对象全部材料化**：歌单、歌单成员、覆盖层、墓碑、播放统计的每次已提交变更都必须落到 `echo/records/`（目前只有 song 与 favorite 有写入点）。删除走墓碑，取消不再产生悬空记录。
- **打开资料库时接续**：`choose_library_root` / 根切换流程在扫描 `media/` 之前 MUST 先读取对象资料并按其中的 UUID 接续本机状态；对已由资料库拥有的文件不得铸造新 UUID。
- **对账与幂等**：接续/投影 MUST 幂等；墓碑优先于陈旧记录；本次修复**不**为既有悬空记录做破坏性清理（见 design 的迁移决策）。
- **播放统计可携带**：新增对象种类承载播放统计，采用**按设备分别计数、可加合并**的载荷，避免并发播放被 LWW 计数覆盖。
- **规格勘误**：`受管理资料库目录布局` 的对象目录清单与实际 `RecordKind`（含 `overrides`）对齐，并纳入新的播放统计目录。
- **门禁诚实化**：`PLL-R03-S01/S02` 目前的命令只断言歌曲投影，却在 `docs/native-attestation-playbook.md` 中被描述为覆盖「喜欢/歌单/成员顺序」；本 change 补齐能真正证伪「歌单与喜欢丢失」的场景与命令。
- **文档同步**：`docs/DESIGN.md:139` 现在写「SQLite 是…运行时真相源」，与 issue 的约束冲突；改为「SQLite 是本机运行时投影，资料库目录是持久真相源、SQLite 必须可从中重建」。

不新增 IPC 命令、不改协议、不引入网络依赖（一期仍无远端同步）。

## Capabilities

### New Capabilities

无。本 change 只修正既有能力与既有实现之间的差异，不引入新的能力域。

### Modified Capabilities

- `portable-library-layout`：`受管理资料库目录布局`、`可携带对象资料`、`新设备资料库重建` 三条需求的行为变更（对象种类集合、播放统计载荷、触发条件从「连接同步资料后」扩展到「打开本机资料库目录」）；新增 `控制面存在性与自愈`、`对象资料接续与对账` 两条需求。
- `local-library`：`单一资料库根目录与本地持久化` 增补「本机数据库是可再生层」的约束与「清除应用数据后重选同一目录」的场景。

## Impact

- **Core 应用层**：`crates/echo-core/src/application/portable.rs`（`InitPortableLibrary`/`ProjectPortableRecords` 接入生产链路）、`portable_materialize.rs`（playlist / playlist-item / override / tombstone / play-stats 构造器的调用点）、`playlist.rs`（歌单与成员用例的提交后材料化）、`favorite.rs`、`restore.rs`（投影范围从 `RecordKind::Song` 扩到全部对象种类）、`root_switch.rs`（`PrepareLibraryCandidate` 在扫描前接线）、`recover.rs`（导入恢复路径的对象资料补齐）。
- **Core 领域与基础设施**：`domain/library.rs`（新增播放统计对象种类、目录名与载荷类型）、`infrastructure/filesystem/control_plane.rs`（新目录下的读写与分片）、`infrastructure/sqlite/`（投影与对账所需查询/事务）、可能新增一条顺序迁移（若对账需要新的持久状态）。
- **Desktop/装配**：`crates/echo-desktop/src/runtime/services/library.rs`（`choose_library_root` 接续与自愈）、`crates/echo-desktop/src/runtime/services/playlists.rs`、`favorites.rs`、`crates/echo-desktop/src/runtime/player/recorder.rs`（播放统计写入）、`crates/echo-desktop/src/runtime/app.rs`（装配点）。
- **Tauri 壳**：`apps/desktop/src-tauri/src/main.rs`（启动时自愈/接续的装配与失败呈现）。若为呈现接续/自愈结果新增可观察字段或命令，必须同步 `main.json` 的 permissions 与 `security.rs` 的 `MAIN_WINDOW_PERMISSIONS`，并重跑 `cargo check -p echo-app` 重生 `gen/schemas/`、`ipc-types.generated.ts`。
- **门禁与追溯**：`scripts/verify/scenario-commands.mjs`（唯一权威源）→ `gen-scenario-manifests.mjs --write` 重生成 `manifest.json.scenarios[]` / `tests/scenarios/*.yaml`；`docs/traceability.md` 场景表；`docs/native-attestation-playbook.md` 中 `PLL-R03` 的覆盖描述更正。
- **文档**：`docs/DESIGN.md`（§5 存储边界、§4 曲库与扫描中的「真相源」措辞）、`docs/ROADMAP.md` 若涉及一期验收口径。
- **兼容性**：既有资料库目录（无 `manifest.json`、记录种类不全）必须仍可打开——首开走一次性自愈与接续，不要求用户重新导入。这是本 change 最重要的兼容约束。
