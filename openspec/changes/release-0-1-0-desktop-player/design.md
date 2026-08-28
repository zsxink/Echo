## Context

Echo 当前仓库只有产品/架构文档和单文件交互原型，没有 Rust workspace、Tauri 应用、数据库迁移或真实播放器。动机见 `proposal.md`，完整产品口径见 `prd.md`，可观察行为见 `specs/*/spec.md`。

本设计受以下约束：

- `echo-core` 是跨平台共享业务核心，负责资料库、扫描、索引、导入和持久化；不得依赖 Tauri、mpv、React 或桌面系统 API。
- 音频播放不进入 Core。桌面播放、队列、媒体键、托盘与文件关联属于桌面平台层。
- SQLite 与用户资料库文件系统是两个不能形成单一原子事务的资源；任何跨资源写操作必须可恢复。
- 0.1.0 离线运行，只允许一个活动资料库根目录，不初始化任何远端连接器或同步流程。
- UI 以原型为视觉/交互基准，但规格已明确排除模拟同步、全选批量、歌单手动排序和歌曲编辑。
- 三平台仍是 0.1.0 的发布目标，CI 持续保留 macOS、Windows、Linux 的编译检查；当前工程骨架阶段只以 macOS 本机构建与安装 Gate 作为实现前置条件。Windows/Linux 的原生产物、运行时装载和人工冒烟验证明确递延，完成前不得宣称相应平台已经可发布。

## Goals / Non-Goals

**Goals:**

- 建立可长期演进的六边形分层，使 Core 用例可以在无 UI、无 mpv、无真实用户目录的情况下测试。
- 明确数据库 schema、索引、事务边界、状态机、不变量和恢复算法，避免把文件/数据库一致性交给偶然执行顺序。
- 为 React 提供小而稳定的类型化 command/event 契约；不允许 UI 直接接触 SQL、绝对路径拼接或 libmpv。
- 把扫描、导入、删除、播放、应用启动和根目录切换设计成显式状态机，所有长期任务可取消、可观测、可恢复。
- 使 50,000 首曲库的搜索、列表和封面加载满足 PRD 性能目标，并保持键盘/辅助技术可用。
- 从首版建立顺序迁移、契约测试、故障注入、三平台打包和许可证审查门槛。

**Non-Goals:**

- 不提前实现远端 JSON、同步调度、冲突裁决、墓碑/outbox 表或连接器；只保留未来迁移不会破坏的稳定 UUID、覆盖层读取语义和顺序迁移能力。
- 不把播放器抽象塞进 Core，也不让 Core 保存设备播放实例状态。
- 不为未来移动端创建尚无调用方的通用 UI/播放器接口；移动端只复用 Core 的领域和应用边界。
- 不提供插件系统、脚本 API、可配置导入模板或媒体标签写回。
- 不以“所有操作都异步”替代明确线程模型；SQLite、哈希、标签解析等阻塞工作必须进入受控执行器。

## Decisions

### 1. 仓库与分层采用 Ports and Adapters

建议目录：

```text
Cargo.toml
crates/
  echo-core/
    src/
      domain/              # 实体、值对象、状态机、不变量
      application/         # 用例、Port、事务编排、DTO（与 UI DTO 分离）
      infrastructure/
        sqlite/            # Repository、迁移、FTS5
        filesystem/        # 扫描、暂存、哈希、文件监听 Adapter
        metadata/          # lofty 与歌词/封面解析 Adapter
  echo-desktop/
    src/
      ipc/                 # Tauri command/event DTO 映射
      player/              # libmpv actor、队列、播放会话
      platform/            # 托盘、媒体控制、文件关联、回收站、打开目录
      runtime/             # 启动编排与资源生命周期
apps/
  desktop/
    src/                   # React UI
    src-tauri/             # 极薄 Tauri binary，装配 echo-desktop
fixtures/
  audio/                   # 小型、许可清晰的测试音频/歌词样本
```

依赖方向固定为：

```text
React UI → Tauri IPC → echo-desktop → echo-core/application → echo-core/domain
                                   ↘ libmpv / OS adapters
echo-core/infrastructure ─implements→ echo-core/application ports
```

`echo-core` 保持一个 crate，内部按 Domain/Application/Infrastructure 模块分层，避免首版为形式上的“纯净架构”拆成大量微型 crate；通过 `pub(crate)`、模块依赖测试和 feature 边界防止反向依赖。未来若移动端编译或依赖体积需要，再把 infrastructure 拆出。

采用的模式：

- **Ports and Adapters**：隔离 SQLite、文件系统、标签解析、Tauri、mpv 与平台 API。
- **Repository + Unit of Work**：隐藏 SQL，并使一个用例内的关联记录在同一 SQLite 事务提交。
- **Command/Event**：Tauri command 表达请求，event 表达已经发生的状态变化。
- **State Machine**：扫描、导入、删除、播放与启动使用枚举状态和合法转换。
- **Actor**：libmpv 句柄由单线程 actor 独占，避免跨线程 FFI 误用。
- **Strategy/Adapter**：三平台托盘、媒体控制、系统回收站与打开目录共享契约。

替代方案：把 SQL、Tauri command 和播放器都放在 `src-tauri`。该方案初期文件少，但会让 UI/平台类型渗入领域规则，无法复用 Core，也难以故障注入，因此拒绝。

### 2. Core 的用例与 Port 保持业务语义

主要应用用例：

| 用例组 | 用例 |
|---|---|
| 资料库 | `PrepareLibraryCandidate`、`ActivateLibrary`、`GetLibraryStatus`、`StartScan`、`CancelScan`、`ReconcileFsChanges`、`RelinkLibrary` |
| 查询 | `QuerySongs`、`GetSongDetail`、`GetRecentSongs`、`GetFavoriteSongs` |
| 歌曲 | `SetFavorite`、`RecordPlayback`、`DeleteSong`、`UndoDelete`、`FinalizeExpiredDeletes` |
| 导入 | `PlanImport`、`RunImportBatch`、`RetryImportItems`、`RecoverOperations` |
| 歌单 | `CreatePlaylist`、`RenamePlaylist`、`DeletePlaylist`、`SetPlaylistMembership`、`RemovePlaylistSong` |

应用层定义小接口：

- `LibraryRepository`、`SongRepository`、`PlaylistRepository`、`OperationJournalRepository`
- `UnitOfWork`，只暴露事务闭包/事务上下文，不暴露连接对象
- `LibraryFileSystem`，负责受根目录约束的枚举、暂存、原子 rename、权限与规范路径
- `MetadataReader`、`ContentHasher`、`CoverCache`、`LyricsParser`
- `FileEventSource`、`Clock`、`IdGenerator`、`SystemTrashPort`

`SystemTrashPort` 在 Core 中只作为删除完成的外部能力；具体桌面实现由 `echo-desktop` 注入。Core 不能引用 Tauri 或操作系统类型。播放器没有 Core Port；桌面层只在达到统计阈值时调用 `RecordPlayback(song_uuid, playback_session_id)`。

所有业务标识使用新类型（`SongId`、`PlaylistId`、`LibraryRootId`、`OperationId`），路径使用经过校验的 `RelativeMediaPath`；禁止用裸 `String` 混用 UUID、绝对路径和相对路径。

### 3. SQLite 从 0001 迁移建立本地真相源

采用应用私有目录中的 `echo.db`，启用 `foreign_keys=ON`、WAL、`busy_timeout` 和启动完整性快检。迁移文件只追加不改写，每个迁移在事务内执行并记录 checksum。

首版核心表：

| 表 | 关键字段与约束 |
|---|---|
| `schema_migrations` | `version`、`checksum`、`applied_at` |
| `library_roots` | `uuid`、本机 `absolute_path`、规范路径键、`is_active`、读写能力、可用状态、扫描 generation/时间、受控暂存目录随机名称与 marker 版本；同一时刻最多一个 active |
| `songs` | `uuid`、`library_root_uuid`、`relative_path`、规范路径键、BLAKE3、文件 size/mtime、格式、标题/艺人/专辑/时长、稳定 sort keys、收藏、播放次数、加入时间、可用/删除状态、解析错误 |
| `song_lyrics` | `song_uuid`、来源、文本类型、原始文本、类型化时间行 JSON、来源 mtime、解析错误 |
| `song_overrides` | 可空覆盖字段和版本；0.1.0 只读取/迁移，不提供写入 UI |
| `cover_assets` | 内容 hash、MIME、尺寸、缓存 key；二进制封面不直接塞入常用查询行 |
| `playlists` | `uuid`、`library_root_uuid`、展示名、Unicode 标准化判重键、创建/更新时间；名称长度按 grapheme cluster 校验 |
| `playlist_songs` | `playlist_uuid`、`song_uuid`、单调追加 `position`、加入时间；唯一 `(playlist_uuid, song_uuid)` |
| `operation_journal` | `operation_uuid`、kind、state、版本化类型载荷、路径、预留 UUID、hash、undo deadline、错误与时间 |
| `operation_items` | 一个 journal 下音频、LRC 等子资源的独立步骤、源定位、暂存/目标相对路径、hash、错误和活动 target claim；活动 claim 对 `(library_root_uuid, normalized_target_path)` 条件唯一 |
| `scan_runs` / `scan_issues` | 扫描 generation、汇总和损坏/权限/重复文件诊断 |
| `recorded_play_sessions` | `playback_session_uuid` 唯一、`song_uuid`、记录时间，用于播放次数幂等 |

0.1.0 的 0001 **不创建** `tombstones`、`sync_state`、`sync_outbox`。`docs/DESIGN.md` 早期 Phase 0 清单中的同步预留由本 change 收窄；二期以新的顺序迁移按已批准同步协议创建，避免首版锁定未经行为验证的 schema。

关键索引：

- 唯一 `(library_root_uuid, normalized_relative_path)`；路径比较键按当前文件系统语义生成，展示路径保留原始 Unicode。
- 唯一或条件唯一 `(library_root_uuid, blake3_hash)` 对可用歌曲生效；重复物理文件记录为扫描诊断而非第二歌曲。
- `songs(library_root_uuid, added_at, uuid)`、`(library_root_uuid, title_sort, artist_sort, uuid)`、`(library_root_uuid, artist_sort, title_sort, uuid)`、`(library_root_uuid, play_count, title_sort, uuid)`。
- `playlist_songs(playlist_uuid, position)` 与 `songs(is_favorite, ...)`。
- `playlists(library_root_uuid, normalized_name_key)` 唯一，不能依赖 SQLite `NOCASE` 完成 Unicode 判重。

搜索使用独立 `song_search` FTS5 表，索引标准化标题、艺人、专辑和 `song_uuid UNINDEXED`。采用 trigram tokenizer 支持中文/拉丁文包含匹配；长度小于 3 个 Unicode 字符的查询回退到受活动根目录限制的标准化 `LIKE`。Repository 在歌曲事务内显式维护 FTS 行，不依赖容易与外部内容失配的触发器。查询参数严格绑定并转义 MATCH 语法。

替代方案：只使用 `LIKE '%query%'`。50,000 首仍可能可用，但每次三字段全表扫描难以稳定满足 p95 目标，故只作为短查询回退。

### 4. SQLite 并发使用单写者和受限只读连接

`rusqlite` 是阻塞 API。桌面 runtime 提供：

- 一个独占写连接的数据库 actor，顺序执行迁移、用例事务和 FTS 更新；
- 最多 2–4 个只读连接用于分页查询；
- 所有数据库工作在阻塞线程执行，不跨 `.await` 持有事务、锁或 statement；
- 写事务保持小批量，每批扫描提交不超过配置的记录数/时间预算。

扫描 worker 不直接写 SQL，而是产生 `ScannedMedia` 结果并发送给应用用例批量 reconcile。这样 UI 查询不会被数分钟的单一写事务阻塞。

替代方案：全局 `Mutex<Connection>`。实现更少，但长扫描会阻塞查询且容易在异步边界持锁，因此拒绝。

### 5. 根目录切换使用候选态，两阶段激活

状态：

```text
Unconfigured
  → CandidateValidating
  → CandidateScanning
  → ActiveAvailable / ActiveReadOnly
  ↘ CandidateFailed（原 active 不变）
Active* → Unavailable → Relinking/Retrying → Active*
```

选择新目录时先创建/复用 `candidate` 根记录，验证可读性、路径边界并完成候选首扫。候选扫描只有在目录完整枚举、未取消、数据库 reconcile 成功、没有根级权限/路径边界错误且单文件错误已形成 summary 时才算成功；空目录允许成功，枚举中断或根级错误不得激活。重选同一规范路径复用原 `LibraryRootId`。

根切换由桌面 runtime 以 `Prepare → QuiesceOldRoot → CommitActivation → RebindRuntime` 执行，并以 `root_epoch` 拒绝旧任务的迟到结果：

1. `Prepare` 完成候选验证与成功首扫，不改变旧 active。
2. `QuiesceOldRoot` 冻结旧 watcher、取消可取消扫描并等待写协调器归零。正在复制/发布的导入和已进入撤销窗口的删除属于阻断操作：UI 保留全局可见的操作入口并要求用户完成、撤销或等待，不能静默跨根继续。未知 journal 状态直接禁止切换。
3. 播放协调器停止旧根当前项、等待 PlayerActor `unloaded(generation)`，移除旧根 queue entries 并保存一次汇总提示；临时项不受影响。这样运行时行为与重启时过滤非活动根规则一致。
4. `CommitActivation` 只在一个 SQLite 事务内翻转唯一 active 和递增 `root_epoch`；失败时解冻旧 runtime，候选保持可重试。
5. `RebindRuntime` 以新 epoch 启动 watcher、查询 invalidation 和播放解析。旧 epoch 的 watcher/scan/command 结果全部丢弃。

旧根目录记录和歌曲保留但不出现在活动查询中，也不移动用户文件。切换屏障覆盖播放中、扫描中、导入中和删除撤销中的集成测试。

只读根目录进入 `ActiveReadOnly`：允许扫描、搜索和播放，禁用导入、用户删除和其他文件写操作。目录失联只改变 availability，不清除记录。

### 6. 扫描是 generation 驱动的可取消流水线

扫描流程：

```text
Queued → Enumerating → Parsing/Hashing → Reconciling → Completed
                    ↘ Cancelling → Cancelled
                    ↘ Failed
```

实现要点：

1. 枚举不跟随目录符号链接；候选文件 canonicalize 后必须仍在根目录内，防止循环和路径逃逸。
2. 扩展名只做廉价候选过滤，随后探测容器和音轨；支持矩阵以 spec 为准。
3. 对 size/mtime 未变且已有 hash/解析结果的文件跳过重算；变化文件进入受限并行 worker。
4. 哈希、lofty 解析、歌词解析和缩略图处理使用有界 `spawn_blocking`/专用 rayon 池，默认并发不超过 `min(CPU, 4)`，避免抢占播放和 UI。
5. 结果按小批次 reconcile。每次全扫带 `generation_id`；只有扫描完整完成后才把“上一 generation 出现、本 generation 未出现”的歌曲标记 missing。取消或枚举失败不得批量误删。
6. 文件监听事件按规范路径去重并防抖；创建/修改等待文件 size/mtime 连续两次稳定再解析。watcher 在 reconcile 前查询活动 target claim：命中导入 operation 的路径时延迟处理，若文件已经完整发布则复用 journal 预留 `SongId`，不得创建第二 UUID。watcher overflow、根目录替换或无法归类的 rename 降级为增量/全量重扫。
7. 相对路径未变优先复用 UUID；旧路径缺失且 hash 唯一相同则重关联。首次发现多个同 hash 路径时以规范路径键最小者为主路径，其余记 issue；主路径消失后按同一规则提升。hash 不同时只在“唯一缺失候选 + 唯一新文件 + 标准化音乐键相同 + 时长误差不超过 2 秒”时使用音乐键弱重关联；任何歧义都保留旧 missing 记录并创建新 UUID，避免错误合并。

扫描 event 以进度快照而非逐文件洪泛发送：最多每 100 ms 一次，包含 phase、discovered、processed、created、updated、missing、skipped、failed。

### 7. 元数据、歌词和封面分离存储

`MetadataReader` 输出类型化字段与诊断：

- 标签字符串做 Unicode NFKC、控制字符清理和展示兜底；原始文件不修改。
- 时长/格式从媒体流探测，不能仅信任标签。
- 扫描解析并保存覆盖层、内嵌和同名 `.lrc` 的候选状态，展示时选择优先级最高的有效来源；高优先级来源损坏时回退到下一有效来源。LRC parser 保留原始文本并生成排序后的 `{timestamp_ms, text, original_index}`，无有效时间戳则标为 plain text。
- 损坏歌词不阻断歌曲入库；诊断与来源单独记录。

封面在应用缓存目录按内容 hash 保存，并生成列表/详情两个有尺寸上限的缩略图。React 通过只读、严格 key 校验的自定义 asset protocol 获取，不接收任意文件路径。缓存 key 和数据库引用事务一致；孤儿缓存由后台 GC 按容量上限清理。

替代方案：把原始封面 BLOB 放进 `songs`。查询简单，但会放大常用曲库页、SQLite WAL 和内存压力，因此拒绝。

### 8. 导入使用版本化操作日志和同盘暂存

每个输入分配 `OperationId` 和预留 `SongId`。首次获得写能力时，Echo 使用 exclusive-create 在根目录建立随机、持久化的专属目录（例如 `.echo-staging-<128-bit-random>`），并写入包含 application magic、`LibraryRootId` 和格式版本的所有权 marker；只有 marker 完全匹配的目录才可写入、清理或被扫描器忽略。同名目录已存在、marker 无效或目录是 symlink/reparse point 时必须另选随机名，无法安全建立则禁用写能力，绝不接管或忽略用户目录。

导入在受控目录的 `import/<operation-id>` 下暂存。每个 operation 由总状态和逐资源 `operation_items` 组成；item 固定保存 `kind(audio|lrc)`、受桌面可信边界保护的外部源定位、暂存/目标相对路径、预期 size/BLAKE3、attempt 和 target claim。计划阶段必须先以 SQLite 条件唯一约束持久化目标路径 claim 与预留 `SongId`，publish、数据库提交、回滚或人工终止后才释放。所有外部副作用使用“先持久化意图，再执行调用，再持久化结果”的状态。状态机：

```text
Planned
 → CopyPending → CopyApplied
 → ValidatePending → Validated
 → PublishPending → PublishApplied
 → DatabaseCommitted
 → Completed
 ↘ FailedRecoverable / RolledBack
```

不变量：

- `PublishApplied` 之前不存在可见最终半文件；`DatabaseCommitted` 之前曲库查询不到该歌。
- journal 在执行副作用前提交 `*Pending`，状态转换使用 compare-and-set，重试幂等。副作用完成后仅在验证文件位置、size/hash 后提交 `*Applied`，不能用内存中的“调用已返回”作为恢复依据。
- 复制过程中流式计算 BLAKE3，完成后校验 size/hash；最终 rename 必须在同一文件系统。若平台 rename 语义不满足，使用“新建 exclusive 目标 + fsync + rename”且绝不替换。
- 目标名先 Unicode 规范化，再清理控制字符、分隔符和 Windows 保留名；按平台字节/组件限制截断，保留扩展名并附短 hash 后缀保证可区分。
- 去重在计划时和提交前各检查一次，解决并发导入/监听竞态。内容相同返回已有 UUID；同名不同内容寻找最小 `(n)`，所有创建使用 create-new 语义。
- watcher、扫描 reconcile、导入和删除共享每根目录的写协调器与持久化 path claim。publish 后、`DatabaseCommitted` 前到达的 watcher 事件必须等待 operation，或从 claim 取得同一预留 `SongId` 完成记录；唯一 hash 约束只是最后防线，不能用来选择另一个 UUID。
- 每个文件只保证自身以同盘 rename 原子发布；两个独立目标文件不宣称跨文件事务原子。音频是主资源，`.lrc` 是同一 journal 的可选子资源；数据库只在音频 `PublishApplied` 后可见。LRC 失败不会回滚已验证音频，但结果必须是“音频成功、歌词失败”；恢复会清理未完成侧车或继续发布完整侧车，不得留下半侧车。

启动恢复规则：

- 对每个 `*Pending/*Applied` item 同时检查源、暂存、目标三处的存在性、文件类型、size 和 BLAKE3。目标匹配预期即将 publish 视为已应用并前滚；暂存匹配且目标不存在则重试；两处均无匹配或目标被不同内容占用则进入 `FailedRecoverable`，关闭该根写能力并保留诊断，绝不覆盖。
- `CopyPending/CopyApplied/ValidatePending/Validated`：有效暂存可继续校验；无完整暂存且未发布则清理安全残留并回滚。
- `PublishPending/PublishApplied`：最终文件 hash 正确则规范化为 `PublishApplied` 并用预留 UUID 补写数据库；只有暂存正确则重试 exclusive publish；目标内容冲突则隔离并报告人工恢复。
- `DatabaseCommitted`：验证文件/记录一致后标 Completed。
- 任意恢复可重复执行；不得用新的 UUID 或再次复制。

故障注入覆盖每个状态写入前后和每次 copy/fsync/rename/SQLite commit 调用前后，恢复至少运行两次，并断言唯一终态、无孤儿最终文件、无幽灵记录、无覆盖。

### 9. 用户删除与外部缺失采用不同模型

外部删除只把 `songs.availability` 标为 `missing`，保留 UUID、收藏、统计和歌单关系；恢复相同 hash 后重新可用。

Echo 用户删除同样以逐资源 item 保存原路径、受控暂存路径、size/hash，并对 rename 与系统 trash 使用意图态/结果态：

```text
Requested
 → StagePending → StageApplied
 → HiddenInDatabase (undo_deadline = now + 10s)
 → RestorePending → RestoreApplied → Restored
   or TrashPending → TrashApplied → DatabaseFinalized
                   ↘ TrashOutcomeUnknown
```

- 删除 command 先由桌面 `DeletionCoordinator` 快照目标 SongId 的 current、queue、history 和 shuffle entry IDs；若包含当前项，则停止/推进并等待 PlayerActor 确认 `unloaded(generation)` 后才调用 Core。只有 Core 返回 `StageApplied/HiddenInDatabase` 后才提交队列移除；unload、暂存或数据库隐藏失败时恢复仍有效的队列快照和明确的 paused 状态，不写成功事件。Windows 文件句柄未释放或 unload 超时则不写 journal、不改数据库。
- 音频与同名 `.lrc` 分别以同盘 rename 移入专属受控目录的 `trash/<operation-id>`；每个 rename 前写 `StagePending`，验证暂存 hash 后写 `StageApplied`。全部必需 item applied 后，同一 DB 事务把歌曲标 pending-delete；默认曲库/歌单查询和待播队列过滤该 UUID，收藏、统计和歌单关系仍留在数据库，撤销只需恢复文件与歌曲状态。
- 10 秒内撤销把文件移回原路径并恢复歌曲/关系；若原路径被占用，恢复到安全编号路径并保持 UUID。
- 超时后先提交 `TrashPending`，`SystemTrashPort` 将整个操作暂存目录移入系统废纸篓/回收站；仅当平台调用明确返回成功时在当前进程提交 `TrashApplied`。`TrashApplied` 是数据库 finalize 的唯一自动前滚依据。恢复时 `TrashPending` 且暂存目录仍在可重试 trash；若暂存消失但 journal 没有 `TrashApplied`，无论 item 是否曾 `StageApplied` 都进入 `TrashOutcomeUnknown`，保留歌曲、收藏、统计和歌单关系，禁止该根继续写入并提供人工确认/恢复指引，绝不凭“路径不存在”推断 trash 成功。平台如能提供可跨重启验证的 receipt，可作为额外诊断，但不能削弱上述安全默认值。
- 应用崩溃后，恢复矩阵逐 item 检查原/暂存两处和预期 hash：`StagePending` 下原缺失而暂存匹配即规范化为 applied；`RestorePending` 下原路径匹配即规范化为 restored；两处证据矛盾时不删除任一文件。未过期操作恢复剩余倒计时；已过期且暂存仍可验证的操作继续请求 trash。系统回收站失败保留暂存和 journal；结果未知保留数据库关系并停止写操作。
- 只读根目录不提供删除入口；扫描器忽略 trash 暂存区。

该设计比“确认后立即调用系统 trash 再伪造撤销”复杂，但真实满足可撤销和崩溃恢复，也遵守不永久删除。

### 10. Tauri IPC 使用生成的类型契约与粗粒度 commands

`echo-desktop/ipc` 定义 serde DTO，并通过构建时生成器产出只读 TypeScript 类型；CI 运行生成器并拒绝未提交差异。Core 领域类型先映射到 IPC DTO，禁止给领域实体直接派生 Tauri/TypeScript 特性。

代表性 commands：

| 域 | Commands |
|---|---|
| 启动/偏好 | `get_bootstrap_state`、`set_theme`、`set_close_behavior` |
| 资料库 | `choose_library_root`、`retry_library`、`start_scan`、`cancel_scan`、`query_songs`、`get_song_detail` |
| 导入/文件 | `choose_and_import_files`、`retry_import_items`、`delete_song`、`undo_delete`、`reveal_song` |
| 收藏/歌单 | `set_favorite`、`list_playlists`、`create_playlist`、`rename_playlist`、`delete_playlist`、`set_song_playlists`、`remove_playlist_song` |
| 播放 | `play_context`、`play_temporary_file`、`player_control`、`queue_command`、`restore_playback_session` |

代表性 events：

- `library://status`、`library://songs-invalidated`
- `operation://progress`、`operation://completed`
- `player://snapshot`、`player://error`
- `app://file-open-result`、`app://platform-capability`

Command 返回 `Result<T, IpcErrorDto>`；错误包含稳定 `code`、用户安全 message key、`retryable`、可选 `operation_id` 和结构化字段，绝不直接序列化 Rust debug 字符串或完整路径。Event 带单调 `sequence`；React 丢弃旧序列，监听重建后先拉取 snapshot，避免只靠事件恢复状态。

前端不能获得通用文件系统权限。目录/文件选择在 Rust/Tauri dialog 边界完成并立即交给用例；Tauri capability 只开放显式 commands、dialog、window-state 和必要 opener 能力。

### 11. libmpv 由单线程 PlayerActor 独占

`echo-desktop/player` 创建专用线程，唯一持有 libmpv handle。调用方通过有界 channel 发送类型化 `PlayerCommand`；actor 循环处理命令和 `mpv_wait_event`，把属性变化归一为 `PlayerSnapshot`。任何 libmpv/FFI 类型不越过模块边界。

```text
Stopped
  → Loading(item, generation)
  → Playing ↔ Paused
  → Ended → Loading(next) / Stopped
  ↘ Failed → Loading(next) / Stopped
```

- 每次 load 递增 generation；旧异步事件不能覆盖新歌曲状态。
- 观察 `pause`、`time-pos`、`duration`、`volume`、`mute`、`core-idle`、`eof-reached` 和错误事件。
- 进度内部可高频更新，但 IPC snapshot 前台最多 10 Hz、后台最多 1 Hz；seek/暂停/曲目切换立即发送。
- 音频模式关闭视频/封面渲染；封面由 React 展示，避免嵌入跨平台视频窗口。
- 退出顺序：停止接收 command → 保存会话 → 停止媒体集成 → terminate mpv → join actor → 关闭 DB/runtime。
- 打包使用经许可证审核的可再分发 libmpv 构建，并固定 ABI/校验和；不得静默依赖开发机已安装 mpv。当前工程骨架阶段在 macOS 验证动态库位置、universal rpath、签名和许可证文件；Windows 的应用目录 DLL 搜索与 Linux 的运行时依赖检查保留为后续平台 Gate，不得以未验证状态承诺发布。
- 错误推进独立于 repeat-one：本轮按 `queueEntryId` 记录失败集合，每个 entry 最多自动尝试一次；单曲循环失败时跳到下一未失败 entry，随机模式从未失败 bag 选择，全部候选失败后进入 Stopped 并只汇总提示一次，禁止自旋重载。

替代方案：mpv JSON IPC 子进程。它隔离崩溃但增加可执行文件发现、生命周期、IPC 解析和打包差异；项目已决定 libmpv，首版采用进程内 actor，并以最小 FFI 模块隔离 `unsafe`。

### 12. 队列与播放会话留在桌面层

`PlaybackSession` 保存：`QueueEntry { queue_entry_id, item: Library(SongId) | Temporary(...) }` 列表、current `queue_entry_id`、history entry IDs、当前 position、mode、volume、mute、shuffle bag entry IDs、加载 session ID。资料库项只用 `SongId` 解析文件；同一 SongId 的每次追加拥有不同 queue entry ID，不能折叠。临时项仅在内存持有已校验路径和快照元数据，持久化前过滤。

规则：

- `play_context(view_query, selected_song)` 先让 Core 返回确定性 UUID 列表/游标快照，再建立从所选项开始的上下文，避免 UI 把过期绝对路径传给播放器。
- “下一首播放”插入 current 之后；“加入队列”追加；清空只移除待播。
- 随机模式维护本轮未播放 `queue_entry_id` bag，当前上下文耗尽前不重复；上下文改变时按 entry ID 重建，重复 SongId 仍保持为独立项。
- 上一首：当前真实位置大于 5 秒时回到当前歌曲开头，否则回到历史上一首；该阈值属于桌面播放器内部一致规则。
- 会话写入应用私有 JSON/SQLite desktop-state（不进入 Core schema），使用 temp + fsync + atomic replace；临时项从持久化快照过滤。
- 启动恢复把外部缺失或根目录暂不可用的 UUID 保留为 blocked，过滤 Echo 已永久删除或不属于活动根的 UUID并汇总一次提示，恢复位置但保持暂停。显式系统文件打开请求在安全初始化后覆盖普通恢复上下文并允许自动播放。

播放统计使用 actor 累积的单调真实播放时长，而非 `time-pos`。每次 load 生成 `playback_session_id`；累计达到 `min(30s, duration*0.5)` 后只调用一次 Core `RecordPlayback`，数据库对 session ID 幂等。暂停不累计，seek 不增加累计时长。

### 13. 媒体键、托盘和文件关联通过平台 Adapter

`PlatformIntegration` 组合：

- `TrayAdapter`：Tauri tray/menu；macOS 状态栏，Windows/Linux 托盘。
- `MediaControlAdapter`：macOS Now Playing/媒体命令、Windows 系统媒体传输控制、Linux MPRIS；统一 publish snapshot 与 command callback。
- `FileAssociationAdapter`：打包注册格式；macOS open-file 事件和 Windows/Linux argv 均进入同一队列。
- `SystemTrashAdapter`、`RevealInFolderAdapter`、`WindowLifecycleAdapter`。

单实例插件必须最先注册；二次启动只把参数发送给主实例。启动未完成时，文件打开请求进入有界 FIFO；runtime ready 后逐个验证。只有当前活动根内的路径解析为 UUID；已保留但非活动旧根中的路径与完全库外路径都创建临时项并立即播放。旧根临时项不读取旧根覆盖层、不累计统计、不持久化，并提示用户可在设置中显式切换根，绝不静默激活旧根。

窗口关闭事件由 Rust 拦截：macOS 默认 hide，Windows/Linux 默认 exit；用户偏好覆盖默认。托盘“退出”走显式 shutdown，不再次被 close handler 拦截。窗口状态恢复时先不可见创建、校验是否仍落在当前显示器可见区域，再显示，避免闪烁/离屏。

### 14. React 使用 Core server state、播放器实时态和 UI 临时态三分法

前端建议：

```text
src/
  app/                 # 路由、provider、错误边界、shortcut
  bridge/              # 唯一 invoke/listen 封装和生成类型
  features/
    library/ playlists/ player/ lyrics/ settings/ import/
  components/          # 无业务依赖的组件
  styles/              # DESIGN tokens、主题、reduced-motion
```

- Core 查询/变更使用 query cache（如 TanStack Query）；`songs-invalidated` 只按 key 失效/局部 patch，不把整个数据库镜像进全局 store。
- `PlayerSnapshotStore` 使用 `useSyncExternalStore` 或等价外部 store 接收节流 event；播放控制执行乐观“pending”而不伪造最终成功，最终以 snapshot 为准。
- 浮层、焦点返回、歌词手动滚动等 UI 临时态留在组件 reducer；不进入全局 store。
- 歌曲列表使用 windowing（如 TanStack Virtual），键为 SongId；排序/搜索改变时重建索引但保留当前播放标识，焦点目标不存在时回退到搜索/列表容器。
- 搜索输入 150 ms debounce，并通过 request id/abort 取消过期响应；清空即时执行。
- 封面 URL 只接受 asset key；组件处理 loading/error/placeholder，不拼接用户路径。

信息架构与视觉直接复用 `docs/interface-terminology.md`、`docs/prototype/` 和 `brand-spec.md` 的 token。原型 HTML 只作为参考，不复制其模拟业务状态或巨型单文件脚本。

### 15. 可访问性和浮层采用统一 Overlay Manager

Overlay Manager 维护栈与返回焦点，层级为：阻断对话框 → 设置/歌单选择器 → 菜单/排序/队列 → 歌词专注 → 沉浸式播放器 → 窄屏侧栏。一次 Escape 只 pop 一层；点击外部先消费事件，避免同一次 click 触发底层危险按钮。

- Dialog 使用 `role=dialog/alertdialog`、焦点陷阱、标题/描述关联。
- Menu 使用 roving tabindex、方向键/Home/End/Enter/Escape。
- Toast 成功用 polite live region；需要用户处理的错误保留在页面/对话框，不只使用会消失 Toast。
- 播放进度和音量使用原生 range 语义并提供时间/百分比文本。
- reduced-motion 关闭黑胶持续旋转和平滑歌词滚动；当前行、播放态和焦点信息仍保留。
- 颜色 token 在三主题下自动化检查 AA，对收藏/错误/选中同时提供图标、文本或语义状态。

### 16. 安全、隐私和权限最小化

- Tauri CSP 默认拒绝远端脚本/连接；`connect-src` 不开放网络，asset protocol 只接受白名单 key 和尺寸枚举。
- capability 文件按主窗口最小授权；不向 JS 暴露通用 shell、fs、SQL、任意 opener 或任意路径读取。
- 所有从文件对话框、argv、watcher 进入的路径先 canonicalize；导入目标通过受根约束 join 验证，不允许 `..`、绝对子路径、符号链接逃逸或 TOCTOU 覆盖。
- 日志使用 tracing 结构化字段：operation ID、SongId、错误 code、相对路径 hash；默认不记绝对路径、歌词、标签全文或音频内容。用户主动导出诊断时才可选择包含脱敏路径映射。
- 没有账号、遥测和远端请求；依赖/打包 CI 运行许可证清单、漏洞审计和产物校验。

### 17. 错误模型与可观测性

Core 使用可匹配错误 enum：`Validation`、`Permission`、`Unavailable`、`Conflict`、`UnsupportedMedia`、`CorruptMedia`、`Io`、`Storage`、`Cancelled`、`InvariantViolation`。基础设施错误保留 source/context，桌面边界映射稳定 code 和本地化文案 key。

长期操作均有 `OperationId`、phase、progress 和最终 summary。用户可重试的错误必须携带重试目标，不可恢复错误给出安全下一步。panic hook 只生成本机崩溃诊断，不自动上传。

关键不变量启动检查：

- active 根目录最多一个；
- 所有 playlist member 指向 songs（外键）；
- 可见可用歌曲的相对路径和 hash 唯一约束成立；
- journal 中间态能被对应 handler 识别；未知版本停止写操作并保留数据；
- 播放快照 generation 单调。

### 18. 测试分层与验收追踪

| 层 | 测试 |
|---|---|
| Domain | 名称清理、UUID/重关联、排序 tie-break、队列/随机 bag、播放计数、状态机转换；属性测试覆盖 Unicode/路径和重复执行 |
| Application | Fake Repository/FileSystem/Clock/Trash 的用例测试；每个 journal 状态故障注入后恢复两次验证幂等 |
| Infrastructure | 临时目录 + 真实 SQLite/FTS5/迁移；合法小音频 fixtures 覆盖所有格式、标签、封面、歌词和损坏输入 |
| Desktop | Fake PlayerAdapter 的 command/event 契约；真实 libmpv 使用短静音/音调文件做 smoke；托盘/文件关联/关闭行为平台测试 |
| IPC | Rust DTO 序列化 golden test、生成 TypeScript diff、过期 event sequence 与错误映射测试 |
| React | Vitest + Testing Library + axe；视图、空/错态、Overlay/Escape、虚拟列表身份、reduced-motion、三主题 |
| E2E | mock bridge 浏览器流程 + macOS 原生产物 Gate；Windows/Linux 原生产物冒烟在后续平台 Gate 执行。所有平台最终覆盖扫描→导入→播放→歌单→重启、临时文件、失联、删除撤销 |

规划阶段为 `specs/` 每个 Scenario 建立不可复用的稳定 ID，并在 `traceability.md` 逐场景登记 Requirement、任务、测试层、具体测试/人工步骤和实际验证命令；不得以 Requirement 整行继承代替场景映射，也不得用行号作为身份。自动审计比较 spec 场景 ID、追踪表和测试 manifest 的集合完全相等；任务完成条件包含其全部关联场景通过。13.9 只执行和汇总既有追踪表，不能到发布前才补建。Core 90% 覆盖率作为门槛，不以 UI 行覆盖率替代关键流程测试。CI 矩阵执行 Linux/Windows/macOS Rust + 前端检查，并产出未签名内部包；签名/notarization 在候选发布流水线执行。

## Risks / Trade-offs

- **[libmpv 三平台分发、ABI 与许可证复杂]** → 固定经审核构建及校验和，最小 FFI 模块隔离 `unsafe`；当前先以 macOS 本地 Gate 检查动态库装载和许可证清单，Windows/Linux 保留为后续 Gate。真实产物三平台 smoke 仍是发布门槛。
- **[文件监听丢事件、rename 语义和编辑中的半文件不同]** → 防抖+稳定性检测、generation 全扫、overflow 降级重扫；监听只做加速，扫描最终收敛。
- **[BLAKE3 全文件扫描对机械盘/大库耗时]** → size/mtime 缓存、有界并发、增量可见、取消与进度；不牺牲稳定身份去使用不可靠的部分 hash。
- **[音乐键弱匹配可能误合并]** → 仅唯一候选且时长容差满足时使用；任何歧义不合并并记录诊断。
- **[资料库内受控暂存目录会被其他软件看到或与用户目录冲突]** → 每根使用随机持久化名称、exclusive-create 和所有权 marker；扫描器只排除验证归 Echo 所有的目录，绝不接管同名用户目录；完成/恢复后只清理 marker 匹配的子项。
- **[系统回收站实现不一致或失败]** → 先在受控暂存完成可撤销事务，Trash Adapter 失败保留可恢复数据，绝不退化为永久删除。
- **[FTS5 trigram 索引占用更多磁盘且短查询退化]** → 独立索引、短查询受根限制 LIKE、性能基准和索引重建工具；不把 FTS 内容作为唯一数据源。
- **[SQLite 单写者可能成为扫描瓶颈]** → 解析/哈希并行，写入批处理且事务短；指标确认瓶颈后再扩大写策略，不引入多写者锁争用。
- **[大量播放器 event 压垮 WebView]** → snapshot 节流、generation/sequence 丢弃陈旧事件，seek/状态变化单独即时发送。
- **[Linux 桌面环境的托盘、MPRIS、DBus 和单实例差异]** → 平台 capability 探测、降级提示、AppImage/deb 首批支持；Snap/Flatpak/rpm 单独验证后再宣称支持。
- **[窗口恢复到已断开的显示器]** → 显示前验证几何位置，回退到主显示器居中可见区域。
- **[前端缓存与 Core 真相漂移]** → event 只做失效提示，恢复/重连先拉 snapshot；所有 mutation 最终以 Core 返回和序列号为准。
- **[二期同步 schema 尚未确定]** → 0.1.0 不预建同步表，只冻结 UUID、覆盖层读取语义和迁移机制；二期通过新 migration 落地已批准协议。

## Migration Plan

1. **工程落地与平台 Gate**：建立 Rust/前端 workspace、工具链锁定、三平台 CI、最小 Tauri 壳和薄 binary；当前在开始文件写与完整播放器实现前，必须用 macOS 本地最小安装包验证随包 libmpv 装载、single-instance 冷/热唤醒、菜单栏、文件关联和显式退出。Windows/Linux 的托盘、libmpv 装载、文件关联与安装包验证递延至后续平台 Gate；在各自 Gate 通过前不得承诺发布。任一已启用平台阻断时先作范围/降级决策，不能只记录 issue 后继续承诺发布。
2. **数据库 0001**：实现迁移、Repository、FTS5、测试 fixtures 和 schema/invariant 检查；生成一个空 `echo.db` 并完成向前/失败回滚测试。
3. **只读资料库切片**：实现活动根目录、扫描/监听、元数据/封面/歌词、搜索排序和只读 UI；此阶段不开放任何文件写按钮。
4. **文件写切片**：加入 import/delete journal、故障注入和启动恢复。只有所有状态点断电测试通过后才在 UI 开启导入/删除。
5. **播放切片**：接入 PlayerActor、队列、统计、会话恢复、常驻播放栏和歌词；随后接入媒体控制、托盘、文件关联。
6. **管理与体验**：歌单、收藏、沉浸模式、主题、窄屏和完整可访问性。
7. **候选发布**：三平台安装/升级/卸载、只读/可移动盘、Unicode/长路径、10k/50k 性能、许可证、签名和 PRD A1–A14 验收。

0.1.0 没有旧应用数据库需要数据迁移，但从第一次内部构建起不得改写 0001。对任何已有数据库执行迁移前先 checkpoint WAL，并使用 SQLite backup API 生成一致备份；`synchronous=FULL` 优先保护文件操作日志。迁移失败时事务回滚并以只读故障页启动，不自动删除/重建用户数据库。应用二进制回退不承诺读取更新版本 schema；候选发布前保留数据库备份，回退需同时恢复对应备份。文件写能力可通过本地安全开关关闭，以便在发现风险时退回只读扫描/播放，而不是发布已知覆盖风险。

发布平台固定为 macOS 12+ universal、Windows 10 22H2/11 x64、glibc 2.35+ x86_64 Linux；macOS 采用非 App Store notarized DMG，Linux 提供 AppImage/deb。libmpv 使用随应用动态分发、固定 checksum 的 LGPL-compatible 构建，Windows DLL 搜索限定应用目录，macOS 修正 rpath 并一并签名，Linux 产物检查运行时依赖。媒体内容探测由 Core 的独立 `MediaProbe`（优先 Symphonia 一类纯 Rust 探测器）确认音轨/时长，lofty 只负责标签、封面和歌词，不把 libmpv 引入 Core。标签单字段上限 4 KiB、歌词候选上限 2 MiB、内嵌封面输入上限 20 MiB，超限记录诊断并跳过该资产，防止异常媒体耗尽内存。
