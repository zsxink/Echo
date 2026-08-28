## 执行与验收约定

- 任务 1.3 建立统一执行器：`pnpm verify:task -- <task-id...>` 按任务 manifest 调用 Rust、前端、E2E、故障注入或原生检查，并对每个 ID 独立返回结果；`pnpm verify:scenario -- <scenario-id...>` 按 `traceability.md` 和测试 manifest 执行场景。任务不得用人工描述替代 manifest 中的实际命令。
- 每组标题下给出覆盖本组所有 task ID 的实际聚合命令；单项开发时使用相同命令只传该 task ID。命令、fixture、过滤器或人工步骤未登记，或命令未通过，任务不得勾选。
- 原生人工步骤也必须由执行器生成带平台版本、步骤、预期结果、操作者和证据路径的 manifest，并以非零退出码表示缺失或失败。

## 1. 工程骨架与交付基线

本组验收：`pnpm verify:task -- 1.1 1.2 1.3 1.4 1.5 1.6 1.7 1.8 1.9 1.10`

- [x] 1.1 创建根 Cargo workspace、`crates/echo-core`、`crates/echo-desktop` 和极薄的 `apps/desktop/src-tauri`，验证 `cargo metadata --no-deps` 只呈现预期成员且 `echo-core` 不依赖 Tauri/mpv。
- [x] 1.2 初始化 `apps/desktop` 的 React + TypeScript + Vite 工程与 pnpm workspace，启用 TypeScript strict、ESLint、Prettier 和 Vitest，验证 `pnpm install --frozen-lockfile && pnpm typecheck && pnpm build` 通过。
- [x] 1.3 固定 Rust stable、Node、pnpm 与依赖锁文件，建立统一 `.editorconfig`、任务/场景测试 manifest、`verify:task` 与 `verify:scenario` 根脚本；使用脚本自测 fixture 验证未知 ID、缺命令、失败命令和缺人工证据均返回非零，锁文件无变更。
- [x] 1.4 建立 `domain/application/infrastructure` 与 `ipc/player/platform/runtime` 模块可见性规则和架构测试，验证测试能拒绝 `echo-core` 引入 Tauri、mpv、React DTO 或平台 `cfg` 业务分支。
- [x] 1.5 配置三平台 CI 矩阵执行 Rust format/clippy/test 和前端 format/lint/typecheck/test/build；本阶段仅在本地验证 macOS，Windows/Linux 实际流水线结果留待后续平台 Gate 确认。
- [x] 1.6 建立 `fixtures/audio` 的许可清晰小型样本生成/来源清单，覆盖保证格式、标签、封面、同步/纯文本歌词、损坏容器和无音轨 MP4，验证 fixtures checksum 测试通过且仓库不包含未授权媒体。
- [x] 1.7 配置 `cargo-deny`、`cargo-audit`、前端依赖审计和许可证清单，验证 `cargo deny check && cargo audit` 及前端生产依赖审计无阻断项。
- [x] 1.8 建立 Rust/前端统一错误日志与本地诊断目录约定，验证测试日志默认不含完整绝对路径、歌词、标签全文或文件内容。
- [x] 1.9 建立 macOS 最小可安装 Tauri Gate，随包加载固定 libmpv，并验证本地冷/热 single-instance、菜单栏、保证格式文件关联和显式退出；macOS Gate 阻断则失败，在通过或批准范围调整前不得开始 5.x 文件写及 8.x 完整播放器任务。Windows/Linux 的相同验证递延至后续平台 Gate，未执行前不得宣称通过。
- [x] 1.10 固化 macOS libmpv 来源/checksum/ABI/许可证和最小包依赖报告，验证 universal rpath+签名与产物装载；报告进入 CI artifact。Windows 应用目录 DLL 搜索和 Linux glibc 2.35 AppImage/deb 装载递延至后续平台 Gate，未验证平台不允许只记录 issue 后继续承诺发布。

## 2. Core 领域模型、状态机与 Ports

本组验收：`pnpm verify:task -- 2.1 2.2 2.3 2.4 2.5 2.6 2.7 2.8`

- [ ] 2.1 实现 `SongId`、`PlaylistId`、`LibraryRootId`、`OperationId`、`PlaybackSessionId` 和 `RelativeMediaPath` 新类型，验证序列化、解析失败和不同 ID 不可混用的单元测试通过。
- [ ] 2.2 实现歌曲、根目录、歌单成员、歌词候选、媒体诊断和值对象，验证可用/missing/pending-delete、读写能力和歌词来源优先级的不变量测试通过。[local-library][playlist-management]
- [ ] 2.3 实现 Unicode NFKC/case-fold、grapheme cluster 长度、展示兜底和平台安全名称规则，使用属性测试验证中文、日文、emoji、组合字符、Windows 保留名、控制字符与长组件。[safe-file-ingestion][playlist-management]
- [ ] 2.4 实现扫描、活动根切换、导入/删除逐资源 `Pending/Applied`、根切换屏障和播放器领域状态枚举及合法转换，验证所有非法倒退/跳跃转换返回可匹配错误且状态机属性测试通过。
- [ ] 2.5 定义 Core 错误分类 `Validation/Permission/Unavailable/Conflict/UnsupportedMedia/CorruptMedia/Io/Storage/Cancelled/InvariantViolation`，验证基础设施错误保留 source 且公共错误不泄露路径。
- [ ] 2.6 定义 Repository、UnitOfWork、LibraryFileSystem、MetadataReader、MediaProbe、ContentHasher、CoverCache、LyricsParser、FileEventSource、Clock、IdGenerator、SystemTrashPort 小接口，验证接口不暴露 SQLite/Tauri/mpv 具体类型。
- [ ] 2.7 为所有 Port 提供内存/临时目录测试替身与可控时钟/ID，验证用例测试可模拟权限撤销、崩溃点、回收站失败和 watcher 乱序而不访问真实用户目录。
- [ ] 2.8 实现稳定排序描述、服务端 cursor 和播放上下文描述类型，验证四种排序的 tie-break、cursor 翻页无重复/遗漏以及 50,000 UUID 不经 IPC 传入 UI。[library-experience][desktop-playback]

## 3. SQLite 迁移、Repository 与搜索

本组验收：`pnpm verify:task -- 3.1 3.2 3.3 3.4 3.5 3.6 3.7 3.8 3.9`

- [ ] 3.1 创建不可改写的 `0001` 迁移，包含 `schema_migrations`、带随机暂存目录/marker 字段的 `library_roots`、`songs`、`song_lyrics`、`song_overrides`、`cover_assets`、`playlists`、`playlist_songs`、带条件唯一 target claim 的 `operation_journal/items`、`scan_runs/issues`、`recorded_play_sessions`，明确不创建同步表；验证全新建库 schema snapshot 测试通过。
- [ ] 3.2 为 active 根、规范相对路径、BLAKE3、四种排序、收藏、歌单名称和追加位置建立唯一/查询索引及外键级联，验证重复路径/hash/歌单成员被约束且 pending/missing 行符合设计。
- [ ] 3.3 实现迁移 runner、checksum、`foreign_keys=ON`、WAL、`synchronous=FULL`、busy timeout、quick-check 和 SQLite backup API，验证迁移失败事务回滚且原数据库/备份可重新打开。
- [ ] 3.4 实现单写者数据库 actor 与 2–4 个只读连接，验证并发扫描批写、分页搜索和收藏 mutation 不出现 busy/死锁，且没有事务跨 `.await`。
- [ ] 3.5 实现 Library/Song/Playlist/Operation Repository 与 UnitOfWork，验证导入提交、pending delete、删除 finalize、歌单多目标添加和收藏切换均为单事务权威快照。
- [ ] 3.6 建立 FTS5 trigram `song_search` 并在 Repository 事务内显式维护，验证中文、日文、拉丁大小写、组合字符和完整查询词包含匹配。[local-library][library-experience]
- [ ] 3.7 为少于 3 个 Unicode 字符的查询实现受活动根限制的标准化 LIKE 回退，验证 1–2 字查询语义与 trigram 长查询一致且参数不能注入 MATCH/LIKE。
- [ ] 3.8 实现 keyset cursor 分页与最近 100 首查询，验证所有排序升/降序在同值、并发插入和重扫后保持确定性。[library-experience]
- [ ] 3.9 实现 `recorded_play_sessions` 幂等写入和 play_count 更新，验证同一加载会话重复上报只计一次、不同会话可各计一次。[desktop-playback]

## 4. 媒体解析、资料库扫描与监听

本组验收：`pnpm verify:task -- 4.1 4.2 4.3 4.4 4.5 4.6 4.7 4.8 4.9 4.10`

- [ ] 4.1 实现活动根候选成功判据和 `Prepare→QuiesceOldRoot→CommitActivation→RebindRuntime` 屏障，验证空目录可激活、根级错误保留旧 active、重选旧路径复用 root ID、只读根禁用写能力，并覆盖播放中/扫描中/导入中/删除撤销中切根。[desktop-app-shell][local-library][desktop-playback]
- [ ] 4.2 实现受根目录约束的文件系统 Adapter，以及随机受控暂存目录的 exclusive-create/所有权 marker 校验；扫描只排除 marker 匹配的专属目录，验证已有同名用户目录、伪造/损坏 marker、symlink/reparse point、`..`、大小写和 Unicode 路径均不会被忽略、接管、清理或用于逃逸根目录。
- [ ] 4.3 用独立 MediaProbe 探测音轨、格式和时长，用 lofty 解析标签/封面/歌词，验证 `.mp3/.flac/.m4a/有音轨.mp4/.ogg/.opus/.wav` 成功且伪装扩展名、无音轨 MP4 和损坏文件形成单文件诊断。[local-library]
- [ ] 4.4 实现标签 4 KiB、歌词候选 2 MiB、封面 20 MiB 输入上限与兜底，验证超限资产被安全跳过且歌曲其余字段仍可入库。
- [ ] 4.5 实现 LRC parser 与候选选择，验证覆盖层 > 有效内嵌 > 有效侧车、损坏高优先级回退、无时间戳纯文本、乱序/越界时间戳和空歌词语义。[local-library][immersive-lyrics]
- [ ] 4.6 实现按内容 hash 的封面缓存和列表/详情缩略图、自定义只读 asset key，验证大图不进入歌曲列表查询、错误 key/任意路径被拒绝、缓存 GC 不删除仍被引用资产。
- [ ] 4.7 实现 generation 扫描流水线、取消令牌、有界解析 worker 和小批量 reconcile，验证扫描期间 UI 查询可用，取消/枚举失败不会把未见歌曲误标 missing。[local-library]
- [ ] 4.8 实现 size/mtime 快速跳过、全文件 BLAKE3、原路径/同 hash/唯一音乐键重关联，验证改名移动保留 UUID、重复主路径确定性提升、歧义弱匹配不自动合并。[local-library]
- [ ] 4.9 实现 watcher 防抖、文件稳定性双采样、活动 target claim 等待/预留 SongId 复用、全扫期间事件缓存和 overflow 重扫，注入新增/修改/删除/rename 的乱序、重复、丢失及 publish 后 DB commit 前抢先事件，验证最终状态收敛且 UUID 等于 journal 预留值。[local-library][safe-file-ingestion]
- [ ] 4.10 实现扫描 progress/summary/issue 持久化和取消/手动重扫用例，验证进度节流不高于设计频率、终态不丢失且坏文件不阻断其他歌曲。[local-library]

## 5. 安全导入、删除与崩溃恢复

本组验收：`pnpm verify:task -- 5.1 5.2 5.3 5.4 5.5 5.6 5.7 5.8 5.9 5.10`

- [ ] 5.1 实现逐输入 `PlanImport` 和预留 OperationId/SongId，验证混合批次每个输入都有成功/重复/不支持/失败结果且单项失败不回滚其他项。[safe-file-ingestion]
- [ ] 5.2 实现默认 `歌手/歌手 - 歌曲名.扩展名`、未知艺人/未命名歌曲、平台字符清理、短 hash 截断和最小 `(n)` 冲突编号，验证三平台 golden cases 且绝不覆盖既有文件。[safe-file-ingestion]
- [ ] 5.3 实现专属受控暂存目录、逐资源源定位/暂存/目标/hash、条件唯一 target claim、流式复制+BLAKE3、exclusive 目标保留、fsync 和每文件原子 publish，验证数据库仅在完整音频发布后可见、源文件内容/名称/位置不变、同名用户目录绝不被写入。
- [ ] 5.4 实现同名 `.lrc` 可选子资源与独立结果，验证嵌入歌词优先、LRC 成功配对最终基础名、LRC 失败形成“音频成功/歌词失败”且不留半侧车。
- [ ] 5.5 实现导入 journal 的逐资源 `Copy/Validate/Publish Pending→Applied`、target claim 生命周期与三位置存在性/hash 恢复矩阵，在每次状态写、copy/fsync/rename/DB commit 及 watcher 抢先点前后故障注入并恢复两次，验证唯一终态、同一预留 UUID、无孤儿最终文件、重复文件和幽灵记录。
- [ ] 5.6 实现开始前/提交前双重 BLAKE3 去重和幂等重试，验证并发导入与 watcher 竞争时相同内容只出现一个逻辑歌曲。[safe-file-ingestion]
- [ ] 5.7 实现 Echo 主动删除逐资源 `Stage/Restore Pending→Applied`、专属受控 `trash/<operation-id>`、pending-delete 和 10 秒 undo，验证状态写/rename 任一侧崩溃均可由原/暂存/hash 唯一恢复，撤销保留 UUID、收藏、统计、歌单 position；原路径被占用时安全编号恢复。[library-experience][playlist-management]
- [ ] 5.8 实现 `TrashPending→TrashApplied→DatabaseFinalized` 与 `TrashOutcomeUnknown`、SystemTrashPort 和不可逆点前滚；验证只有已持久化 `TrashApplied` 才自动 finalize，暂存被外部清理、卷断开或调用成功后状态写入前崩溃均保留数据库关系并关闭该根写能力，绝不凭路径缺失推断成功。
- [ ] 5.9 区分外部 missing 与 Echo 删除，验证外部缺失保留 UUID/收藏/统计/歌单/blocked 队列，文件或同 hash 路径恢复后重新可用。[local-library][desktop-playback]
- [ ] 5.10 在 runtime ready 前执行 `RecoverPendingOperations` 并协调 watcher/player 启动，验证恢复期间同一路径不能并发扫描、导入、删除或播放。[safe-file-ingestion]

## 6. 曲库查询、收藏、详情与歌单用例

本组验收：`pnpm verify:task -- 6.1 6.2 6.3 6.4 6.5 6.6 6.7 6.8`

- [ ] 6.1 实现全部歌曲、最近 100 首、喜欢的音乐和歌单查询，用 spec 场景验证视图集合、稳定顺序、active 根隔离和 pending-delete 隐藏。[library-experience]
- [ ] 6.2 实现标题/艺人/专辑完整查询词包含搜索和当前视图叠加，验证清空恢复、无结果、过期请求取消和 50k 数据正确性。[library-experience]
- [ ] 6.3 实现收藏 mutation 与权威结果，验证曲库行、喜欢视图、详情和当前播放栏通过同一 SongId 状态一致。[library-experience]
- [ ] 6.4 实现只读歌曲详情 DTO，包含有效元数据、格式/音频参数、相对路径、统计、封面/歌词来源，不返回绝对路径；验证序列化 golden test。[library-experience]
- [ ] 6.5 实现歌单创建/重命名/删除和 grapheme/NFKC/case-fold 判重，验证 1–40 用户感知字符、成员保持和删除歌单不影响歌曲文件。[playlist-management]
- [ ] 6.6 实现按 `position` 的成员添加、原子添加到多个歌单、幂等重复添加、移除和再次追加，验证顺序不依赖时间戳且不出现重复成员。[playlist-management]
- [ ] 6.7 实现歌单 missing/blocked 成员展示查询和 Echo 永久删除后的级联，验证外部失效可恢复、主动删除 finalize 后成员消失。[playlist-management]
- [ ] 6.8 为本组所有用例建立 Repository 集成测试并分别执行 `cargo test -p echo-core --all-features catalog` 与 `cargo test -p echo-core --all-features playlists`，验证正常、空、错误、只读和不可用状态。

## 7. 桌面 Runtime、Tauri IPC 与本机偏好

本组验收：`pnpm verify:task -- 7.1 7.2 7.3 7.4 7.5 7.6 7.7 7.8`

- [ ] 7.1 实现启动 supervisor 顺序“单实例 → 偏好 → DB 迁移/备份 → journal 恢复 → Core 查询 → PlayerActor → watcher/平台集成 → IPC ready”，验证初始化期间文件打开不丢失且未知 journal 禁止写操作。
- [ ] 7.2 在 `echo-desktop/ipc` 定义 serde camelCase DTO/IpcError 并生成只读 TypeScript 类型，验证生成器测试与 `git diff --exit-code` 能检测 Rust/TS 契约漂移。
- [ ] 7.3 实现 bootstrap、资料库、查询、收藏、歌单、导入/删除和播放的粗粒度 commands，验证 UI 无通用 SQL/fs/shell command 且所有 mutation 返回提交后 revision/snapshot。
- [ ] 7.4 实现带 sequence/revision 的 library/operation/player/file-open events 与 snapshot 重拉，验证重复、乱序、断序和窗口重建不会让旧事件覆盖新状态。
- [ ] 7.5 实现 Rust 侧目录/导入文件选择和 reveal-by-SongId，验证 WebView 不接收完整绝对路径且取消对话框不产生成功态。[desktop-app-shell][safe-file-ingestion]
- [ ] 7.6 实现主题、关窗行为、窗口状态和播放会话本机存储的 temp+fsync+atomic replace，验证损坏/非法偏好回退到珊瑚主题及平台默认关窗值。[desktop-app-shell]
- [ ] 7.7 配置 Tauri CSP、自定义 cover protocol 和最小 capability，运行权限测试验证无网络 connect、无任意文件读取、无任意 opener/shell/SQL 权限。
- [ ] 7.8 实现结构化 tracing、panic hook 和安全错误映射，验证用户错误可重试字段正确且 production 日志隐私测试通过。

## 8. libmpv、播放协调器与队列

本组验收：`pnpm verify:task -- 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 8.12`

- [ ] 8.1 定义桌面 `PlayerPort`、`PlayerCommand`、`PlayerSnapshot` 和 FakePlayer，验证队列/统计/平台控制测试无需加载 libmpv。
- [ ] 8.2 实现最小 `unsafe` libmpv Adapter 与专用 OS 线程 actor，验证唯一句柄、命令 channel、有界 event loop、generation 丢弃和有序销毁测试。[desktop-playback]
- [ ] 8.3 实现 audio-only mpv 配置并禁用用户脚本、ytdl、非必要网络协议和用户配置，验证只能加载 Rust 校验后的本地路径。
- [ ] 8.4 归一化 load/file-loaded/end/property/error 事件及前台 10 Hz/后台 1 Hz snapshot 节流，验证切歌/seek/暂停即时事件和 10 分钟播放无 event 泄漏。
- [ ] 8.5 实现 PlaybackCoordinator 的视图上下文、current/history、加入队列、下一首、清空待播和错误跳过，验证重复 SongId 以独立 queueEntryId 正常出现。[desktop-playback]
- [ ] 8.6 实现按 queueEntryId 持久化的 current/history/shuffle bag、顺序/随机/单曲循环与“>5 秒上一首回到开头”，验证重复 SongId 不折叠、随机一轮不重复、恢复后不重洗、切模式不丢当前项。[desktop-playback]
- [ ] 8.7 实现按 queueEntryId 的本轮错误集合，验证错误推进绕过 repeat-one、随机只选未失败项、每项至多自动尝试一次且全部损坏后停止而不自旋。[desktop-playback]
- [ ] 8.8 实现 seek、volume、mute 最近非零值和命令失败权威回滚，验证 FakePlayer 与真实 mpv smoke 均保持 UI snapshot 一致。
- [ ] 8.9 实现播放会话原子持久化和启动 paused 恢复，验证 blocked missing 保留、已永久删除/非活动根丢弃、重复 SongId entries 保留、临时项过滤、显式文件打开优先并自动播放。[desktop-playback]
- [ ] 8.10 用单调时钟累计真实播放时间并调用幂等 `RecordPlayback`，验证 `min(30s, 50%)`、暂停、seek、重复事件和临时项边界。[desktop-playback]
- [ ] 8.11 实现桌面 DeletionCoordinator 的 current/queue/history/shuffle 快照、PlayerActor unload 屏障与提交/回滚，验证 Windows 真实文件锁、unload/暂存/数据库隐藏失败均恢复有效队列并保持 paused、成功后重复 queue entries 与悬空 history 引用全部移除且下一项状态一致。[library-experience][desktop-playback]
- [ ] 8.12 使用保证格式 fixtures 运行真实 libmpv smoke，验证每种格式、损坏文件、无音轨 MP4、seek/切歌/退出和资源释放。

## 9. 三平台系统集成与分发 Spike

本组验收：`pnpm verify:task -- 9.1 9.2 9.3 9.4 9.5 9.6 9.7 9.8`

- [ ] 9.1 在 1.9 Gate 原型上生产化 Tauri single-instance 和初始化请求 FIFO，验证 macOS/Windows/Linux 冷启动、热启动、重复启动和初始化期间文件打开只由主实例处理。[desktop-app-shell][safe-file-ingestion]
- [ ] 9.2 配置保证格式文件关联并统一 macOS open-file 与 Windows/Linux argv，验证活动根文件解析原 UUID，非活动旧根与完全库外文件均创建不使用旧根覆盖层、不入库/统计/持久化的临时项，且只提示显式切根。[safe-file-ingestion][desktop-playback]
- [ ] 9.3 在 1.9 Gate 原型上生产化 macOS 菜单栏与 Windows/Linux 托盘，提供摘要、播放/暂停、上一首、下一首、显示窗口、退出；验证后台状态和主窗口 snapshot 一致。[desktop-app-shell][desktop-playback]
- [ ] 9.4 实现 macOS Now Playing、Windows SMTC 和 Linux MPRIS 媒体控制 Adapter，验证前台/后台媒体键只触发一次协调器 command，能力不可用时降级到窗口控制。
- [ ] 9.5 实现系统回收站和 reveal Adapter，验证回收站不可逆点、Windows 文件锁重试、Linux 无法定位时打开父目录、所有入口只接收 SongId/OperationId。
- [ ] 9.6 实现窗口 close/hide/explicit quit、window-state 可见区域校验，验证 macOS 默认后台、Windows/Linux 默认退出、用户覆盖、断开显示器后窗口回到可见区域。[desktop-app-shell]
- [ ] 9.7 将 1.10 Gate 的 libmpv 来源/checksum/ABI/许可证 manifest 接入正式构建，验证 macOS universal rpath+签名、Windows 应用目录 DLL 搜索、Linux glibc 2.35 产物依赖检查与 Gate 基线无回退。
- [ ] 9.8 将 1.9 最小包扩展为三平台候选安装包，验证 macOS 12+ universal、Windows 10 22H2/11 x64、Ubuntu 22.04 AppImage/deb 上启动、播放、托盘、文件关联和显式退出；任何阻断差异直接使任务失败并回到平台 Gate 决策。

## 10. React 应用壳、曲库与管理界面

本组验收：`pnpm verify:task -- 10.1 10.2 10.3 10.4 10.5 10.6 10.7 10.8 10.9 10.10`

- [ ] 10.1 从原型/brand spec 提取设计 token、三主题、字体、间距和图标资产到可维护样式层，验证珊瑚默认、深钴蓝/松石绿切换及收藏红色语义不被主题覆盖。[desktop-app-shell]
- [ ] 10.2 实现 bridge 唯一封装、Core query cache、Player external store 和组件局部 reducer 三分状态，验证过期 command/event 不覆盖新 revision 且组件不直接 invoke 任意字符串。
- [ ] 10.3 实现首次启动/未配置/候选扫描/只读/不可用工作区，验证选择取消、切换失败保留旧库、进度/错误/重试和写操作禁用。[desktop-app-shell][local-library]
- [ ] 10.4 实现侧边导航、顶部栏、资料库工作区和常驻播放栏布局，验证同步、全选批量、手动歌单排序和歌曲编辑入口均不渲染。[desktop-app-shell]
- [ ] 10.5 实现全部/最近 100/喜欢/歌单视图、搜索、四排序双方向和服务端 cursor，验证完整查询词包含、清空/无结果、稳定顺序和请求取消。[library-experience]
- [ ] 10.6 用窗口化列表实现歌曲行、播放标识、收藏、下一首、加入歌单和更多菜单，验证 50,000 条只渲染视口范围且菜单始终绑定 SongId。[library-experience]
- [ ] 10.7 实现加载、空曲库、搜索空、部分扫描失败、资料库不可用和文件 missing 状态，验证错误保留已有可用内容且提供正确下一步。[library-experience]
- [ ] 10.8 实现只读歌曲详情和 reveal、删除确认/10 秒 undo/前滚错误状态，验证只展示相对路径且取消/失败不伪造删除成功。[library-experience]
- [ ] 10.9 实现歌单创建/重命名/删除、选择器、多歌单成员、移除和 blocked 成员 UI，验证 40 grapheme 校验、重复幂等、追加顺序和删除歌单不删文件。[playlist-management]
- [ ] 10.10 实现多选导入批次进度、逐文件结果、重复/编号/歌词部分成功和失败重试，验证混合批次无需重做成功项。[safe-file-ingestion]

## 11. React 播放、沉浸式播放器与歌词

本组验收：`pnpm verify:task -- 11.1 11.2 11.3 11.4 11.5 11.6 11.7 11.8`

- [ ] 11.1 实现常驻播放栏的封面/信息/收藏、传输控制、进度、音量、模式、队列和空态，验证所有状态以 PlayerSnapshot 为权威。[immersive-lyrics]
- [ ] 11.2 实现播放队列面板、blocked/错误项、下一首/追加/清空待播和浏览曲库空态，验证清空不停止当前歌曲且不改变歌单/资料库。[desktop-playback]
- [ ] 11.3 实现黑胶封面、歌曲元信息、展开/收起和曲目切换不退出的沉浸式播放器，验证宽屏/窄屏和无封面占位。[immersive-lyrics]
- [ ] 11.4 实现同步歌词当前行、seek 后定位、点击行 seek、乱序/越界处理和 UI 进度插值，验证真实 mpv 快照与歌词行一致。[immersive-lyrics]
- [ ] 11.5 实现纯文本歌词、无歌词和来源错误/回退状态，验证切歌立即清除上一首残留且不伪造纯文本当前行。[immersive-lyrics]
- [ ] 11.6 实现歌词专注模式、手动滚动暂停 5 秒、“回到当前行”和队列覆盖，验证播放控制不中断且 Escape 只关闭最上层。[immersive-lyrics]
- [ ] 11.7 实现临时播放项标识和“导入到资料库”入口，验证收藏/加入歌单/统计被禁用，显式导入后才产生正常 SongId。[safe-file-ingestion][desktop-playback]
- [ ] 11.8 实现应用快捷键、range 键盘步进和媒体状态辅助文本，验证输入框/对话框内 Space 不误触播放，进度 5 秒、音量 5% 步进正确。[desktop-app-shell][desktop-playback]

## 12. 可访问性、响应式、性能与安全硬化

本组验收：`pnpm verify:task -- 12.1 12.2 12.3 12.4 12.5 12.6 12.7 12.8`

- [ ] 12.1 实现单一 Overlay Manager、焦点陷阱/恢复、roving menu 和既定 Escape 栈，覆盖对话框→设置/歌单→菜单/队列→歌词专注→沉浸→侧栏的自动化测试。[desktop-app-shell]
- [ ] 12.2 完成 Tab/Shift+Tab/Enter/Space/方向键/Home/End 全键盘路径和 screen reader 名称/状态，运行 Testing Library + axe 验证无阻断可访问性问题。
- [ ] 12.3 实现 760px 窄屏侧边栏/遮罩、次要列收敛和沉浸布局，验证 resize 不丢视图、搜索、焦点或播放状态。[desktop-app-shell][immersive-lyrics]
- [ ] 12.4 实现 reduced-motion 与三主题 WCAG 2.2 AA 检查，验证唱片/平滑滚动停止但当前行、焦点和播放态仍清楚。
- [ ] 12.5 建立 50k 合成库 benchmark，验证搜索 p95 ≤200 ms、视图首屏 p95 ≤500 ms、虚拟 DOM 行数受视口限制，并把基准结果保存为 CI artifact。
- [ ] 12.6 对扫描/hash/标签/封面 worker 做 CPU/内存/取消压力测试，验证默认并发不超过 `min(CPU,4)`、播放不中断、缓存容量和输入上限生效。
- [ ] 12.7 运行路径穿越、symlink/reparse、TOCTOU 覆盖、恶意标签/LRC、任意 asset key 和 Tauri capability 安全测试，验证无根目录逃逸、任意读取或远端网络访问。
- [ ] 12.8 执行 `cargo llvm-cov --workspace --all-features --fail-under-lines 90` 并审查领域关键分支，验证 Core 覆盖率门槛不靠排除故障路径达成。

## 13. 集成验收、打包与发布交付

本组验收：`pnpm verify:task -- 13.1 13.2 13.3 13.4 13.5 13.6 13.7 13.8 13.9 13.10`

- [ ] 13.1 建立 mock bridge 浏览器 E2E，覆盖 PRD A1–A14 的非平台流程，验证 `pnpm test:e2e` 全部通过。
- [ ] 13.2 建立原生端到端临时资料库流程“扫描→搜索→播放→收藏→歌单→导入→删除撤销→重启”，验证三平台数据、UUID、队列和偏好一致。
- [ ] 13.3 在导入/删除每个 journal 状态写、文件系统调用和 DB commit 前后强制终止并重启两次，并注入 watcher 抢占、暂存目录外部清理和卷断开；验证三位置/hash/claim 恢复矩阵、无孤儿最终文件、无覆盖、无重复 UUID、唯一终态、只有 `TrashApplied` 才前滚及未知结果保留关系；保存故障注入报告。
- [ ] 13.4 执行 watcher 乱序/丢失、根目录卸载/恢复、权限撤销、只读根、Unicode/长路径和外部改名移动矩阵，验证手动重扫最终收敛且关联稳定。
- [ ] 13.5 执行真实 libmpv、文件关联冷/热启动、后台关窗、托盘/媒体键、reveal/回收站三平台人工冒烟并记录版本/桌面环境/结果。
- [ ] 13.6 运行完整质量命令 `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace --all-features` 以及 `pnpm format:check && pnpm lint && pnpm typecheck && pnpm test -- --run && pnpm build`，全部通过后才进入候选打包。
- [ ] 13.7 在 CI 构建 macOS notarized DMG、Windows 安装包、Linux AppImage/deb，验证全新安装、覆盖安装、卸载不删除资料库、libmpv 装载、产物 checksum 和许可证文件。
- [ ] 13.8 复核运行期无账号、遥测、同步 command/event/table 或业务网络请求，验证离线防火墙测试通过且 UI 不显示可操作同步/全选/歌单排序/歌曲编辑。
- [ ] 13.9 执行 `pnpm verify:scenario -- --all`，比较 specs、`traceability.md` 和测试 manifest 的 Scenario ID 集合完全相等，逐项执行 160 个场景（数量必须与校验器从 specs 生成值一致）及 PRD A1–A14；缺失/重复映射、缺实际命令/证据或任何 P0 失败均阻断 0.1.0。
- [ ] 13.10 更新 README、架构/开发/测试/打包/故障恢复文档和第三方 notices，并运行 `openspec validate release-0-1-0-desktop-player --strict` 确认实现交付仍与规格一致。
