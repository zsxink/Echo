# Tasks

以下记录本 change 的实施进度；仅在指定行为和验证均完成后勾选。测试名称前缀在对应任务中新增，命令从仓库根目录执行；不能用过滤后 0 个测试冒充通过。LD 编号对应 `specs/library-durability/spec.md`，入口规格与 design D1–D8 共同约束范围。

## 1. 固化契约与可失败测试基线

- [x] 1.1 按 design 证据重新用 CodeGraph 核对所有本地/同步写入口，交付“入口→资料库提交→投影→成功”清单，覆盖 CRUD、成员、喜欢、统计、封面、写回、删除/撤销、快照、增量、ACK 和主确认；验证每个入口有归属层与当前失败窗口（LD-01/10）。
- [x] 1.2 新增合成 issue #72 fixture（15 songs、2 portable songs、11 favorites、3 members、无 portable playlist），分别提供可证明 DB/alias 与不可证明孤立引用版本；验证 fixture 无真实用户路径、无真实资源、能稳定复现旧恢复丢关系（LD-04/05/06）。
- [x] 1.3 新增统一规范用户状态比较器及 `library_durability` 测试组，比较 UUID、路径/哈希/added_at、喜欢 false/true、空歌单、成员 UUID/顺序/时间、资源、覆盖、设备桶、typed tombstone/alias、pending；以 `cargo test -p echo-core library_durability` 验证失败定位，不能只比计数（LD-03）。
- [x] 1.4 更新 `docs/DESIGN.md`、`docs/PRODUCT.md`、`docs/ROADMAP.md` 与 `openspec/CODE_STANDARDS.md` §5 的 source-of-truth、完整目录备份和重装恢复原则，核对旧空目录加入描述与现行 LAN 本地合并一致（`CODE_STANDARDS.md` 现仍写“首次加入只接受空目录”）；交付文档 diff 与规格对照，不能把规划写成已验收（LD-09/10）。
- [x] 1.5 以 `main` 已发布实现为唯一现状基线复核 design 的证据表：修正不存在或已移动的路径（`infrastructure/filesystem/migration` → `portable_migration.rs`；`sync_provisioner/remote_resources.rs` → `resource_publication.rs` / `resource_candidates.rs`），并声明行号只作导航；验证每条证据都能在干净检出上打开（LD-10）。

## 2. 领域提交模型与端口

- [x] 2.1 新增 typed change set、来源、预期版本、完整 after-image、资源声明、操作身份、本库提交序列/前驱摘要及结果状态模型；验证不合法跨库/跨类型/重复目标/不完整依赖被拒绝，Domain 无 FS/SQLite/网络依赖（LD-01/02/10）。
- [x] 2.2 增加聚焦的目录提交/读取/恢复与投影端口，设计可失败 fake，并明确旧 SQLite UoW 只投影；保留 `ControlPlanePort::durability()` 能力访问器，删除 `ControlPlanePort` 上指向同一实现的五个转发方法（`commit_library_batch`、`recover_library_batches`、`library_operation_status`、`write_library_asset`、`read_library_asset`），调用方统一经 `durability()` 取 `LibraryDurabilityPort`；验证 `ControlPlanePort` 方法数不高于既有基线且规模白名单不因本 change 扩大（LD-10）。
- [x] 2.3 扩展新协议 portable 用户资料字段，包含歌单创建/成员加入时间、资源原件引用及必要关系；分别定义 confirmed 完整 payload、pending/receipt、实际受管资源位置，避免新业务值继承旧确认收据；用新协议 round-trip、缺字段与未知格式测试验证，并确认支持的旧格式可读取且读取不回写或修复历史记录（LD-03/07/08/09）。

  验证：2026-10-25 核对 manifest v3/envelope v2 独立版本、完整字段往返、缺省/未知格式及旧格式无改写；补齐 [契约补充清单](evidence/contracts-20261024.md) 当时缺少的 v3 正向往返与围栏。专项在仅排除缺失导入测试模块声明的临时副本实际通过，生产实现未改；原工作区 E0583 与 governance 阻塞仍保留。范围、测试及限制见 [本次复核](evidence/progress-closeout-20261025.md)。不要求升级或回填旧音乐库，不代表 7.8 文档与完整兼容矩阵已完成。
- [x] 2.4 文档化目录协议版本、字段语义和 typed tombstone/alias 规则，使用 `cargo test -p echo-core library_durability` 核对序列化 fixture 与说明一致，确保缺历史字段保持 absent（LD-05/09/10）。

  验证：2026-10-24 新增 [目录协议字段说明](../../../docs/library-durability-protocol.md)，独立审查通过；主 Agent `library_durability --lib` 74/74、新增历史缺省字段回归 1/1、`portable_v --lib` 4/4、Core Clippy/fmt/scale/strict 通过。缺省时间/确认元数据保持省略，历史 Song 的已发布 0 回退单独说明；当前 v2 与目标 v3 升级边界明确，不替代 2.1/7.8/8.2。

- [x] 2.5 区分共享对象与目录私有 state：逐设备播放桶留在 `PlayStatsRecord`，幂等播放事件身份写入 `echo/state/playback-events/`；验证同步导出与跨设备载荷不含会话标识，`PortableRecord` round-trip 与既有载荷完整性测试通过（LD-03/07/08）。

  验证：2026-10-24 最终持久化组 60/60、实际 producer/host payload 测试 1/1 通过；见 [播放私有状态证据](evidence/playback-state-privacy.md)。生产 recorder 全入口验收仍属于 6.3。

## 3. 文件系统权威提交与恢复

- [x] 3.1 实现资料库级跨进程排他写许可、格式/权限检查及安全 exclusive-create journal；读路径 MUST NOT 取写锁，也不得因锁文件缺失而失败，只读卷上已提交资料仍可浏览；验证同库两写者、切库/取消/退出、只读卷浏览、符号链接/越界/长路径、未知格式不能绕过锁或污染另一库（LD-01/09/10）。

  验证：2026-10-24 实例准入/排空、切库及后台退出接线，退出后回退重启与自动恢复竞争两项缺陷均经有效 RED/GREEN 和独立复审。主线程完整 Core 814/0/1 ignored、所有集成、Desktop 429/429、native 短组 130/0/1 ignored/1 显式过滤 513，workspace all-targets/all-features check/Clippy、fmt/scale/strict 通过；最后测试整理后专项 7/7 与 gate 6/6 复跑通过。详见 [生命周期验收清单](evidence/lifecycle-acceptance-20261024.md)。三平台实机认证与原样 513 仍属于后续验收，不计为已通过。
- [x] 3.2 实现 prepared payload、资源声明、checksum 与本库 seq/前驱提交标记的刷盘/原子发布，用 fault fake 拦截每次文件/父目录 sync 和 rename；验证标记前无业务提交，结果不确定可查询同操作，标记后丢 SQLite 仍恢复（LD-01/02）。
- [x] 3.3 实现按本库顺序的 records/state 材料化、目录 checkpoint 与幂等重放，验证 C1 投影失败后 C2 更新同对象，重启/重复重放不回退 C2；断链/同 seq 不同摘要/损坏已提交数据拒绝写入并保留证据（LD-02/06）。

  验证：2026-10-24 修复重试 fence/coverage 的连续故障窗口并独立复查通过；主 Agent 最终持久化组 60/60，详见 [刷盘与 GC 复查](evidence/retry-fences.md)。此处是目录材料化顺序，Application 投影 4.2 与完整私有历史/资源 GC 3.5 仍未完成。
- [x] 3.4 汇总实现用户资源原件与实际受管版本位置持久化（覆盖原件归 6.2，远端媒体 owner/version 归 7.4）；Echo 不写音乐文件标签，外部修改后的文件由扫描/监听重新校验并提交，不涉及源音乐字节替换（LD-02/08）。

  验证（2026-10-26）：实现已在位，本轮补齐证明。**声明语义**——`batch.resources` 表示"已发布资源"，`assets::validate_resource` 在 `publish_prepared` 与 marker 之前逐项核对长度与 BLAKE3，不符即整批拒绝且不产生 prepared 正文、marker、操作状态或链上提交；只接受 `echo/assets/<blake3>`（路径必须逐字等于摘要地址）与 `echo/media/<kind>/<owner>/<declaration-digest>/<logical-path>` 两类位置，控制面路径（`echo/state/*`、`echo/records/*`、`echo/media` 目录）与 `echo/tmp/` 永不是资源来源，摘要必须是 64 位小写十六进制。**新增 `durability/tests/resources.rs` 五项**：长度不符/同长度异字节两种篡改在产生任何证据前被拒且磁盘字节保持原样、缺失受管版本报 `io` 而不合成占位、被改写原件在提交侧报 `durable resource hash mismatch` 读取侧报 `immutable asset is corrupt` 并保留篡改字节、六类非法地址/路径各自被拒、原件与受管版本声明经 `read_operation` → 强制压缩后的覆盖镜像 → 全新 SQLite `read_recovery_records` 逐字段（路径/哈希/长度）保持不变。**不写源标签**——`rg -n 'save_to_path|write_tags|set_tag' crates/` 生产代码 0 命中（唯一命中为 `metadata/tags/tests.rs:239` fixture）；新增 `application/watch/durability_tests.rs::library_durability_watcher_reverifies_external_edit_that_kept_size_and_mtime` 把源文件 mtime 钉到整秒后原地改写同长度字节，断言 size/mtime 无法揭示差异、监听仍重算哈希并以同一 `Song` UUID 与 `added_at` 提交新资源声明、读回源文件等于外部写入值。命令（目标 + 命中数）：`cargo test -p echo-core library_durability` **223 passed / 0 failed**、`cargo test -p echo-core --lib resources` **21 passed / 0 failed**（5 新增 + 16 既有匹配）、`cargo test -p echo-core --lib external_edit_that_kept` **1 passed**、`cargo clippy -p echo-core --all-targets` 0 警告、`cargo fmt --all --check` 与 `git diff --check` 通过。详见 [资源原件与受管位置证据](evidence/3.4-resource-originals-and-managed-locations-20261026.md)。覆盖原件专属规则归 6.2、远端媒体 owner/version 归 7.4、三平台原生认证归 9.3，均不因本项勾选。
- [x] 3.5 实现安全 checkpoint/压缩/GC，必须先刷盘完整覆盖清单再回收日志，引用图保护候选、快照、冲突、删除意图和未完成发布资源；checkpoint 不得随历史无界增长，链长度应与活跃窗口而非总历史成正比，超过安全上限时拒绝写入并报告；故障注入验证 GC 任一步中断后均可恢复且无唯一字节被删（LD-02/08）。

  验证（2026-10-26）：`cleanup_covered` 在任何删除前完成 ANCHOR 水位校验、逐份 receipt 存在性校验、`coverage_gc`/`receipt_gc` 重刷与 `gc_proof` 祖先目录统一同步；缺失 receipt 或水位不符直接拒绝。新增 `durability/tests/retention.rs` 三项故障注入：(1) 超过 `MAX_ACTIVE` 时 `chain()` 明确报错、写入在 marker 前被拒且已存 journal 原样保留；(2) 在 `gc:remove` 每步中断后重启从锚点重做，仅剩 receipt 也能还原冻结的原始 `LibraryCommit`；(3) 压缩前后 `echo/media/`、`echo/assets/`、`echo/tmp/` 文件集合逐项不变，被引用的 Pending/Conflict/DeleteIntent/ResourceLocation/Receipt state 全部保留，且用回收后目录重建全新 SQLite 仍能读回两份资源声明、从不把 `echo/tmp/` 当作资源来源。详见 [checkpoint/GC 与引用保护证据](evidence/3.5-checkpoint-gc-retention-20261026.md)。验证：`cargo test -p echo-core library_durability` 217 passed / 0 failed，`cargo test -p echo-core --lib retention` 8 passed，Clippy 0 warnings，fmt/diff 检查通过。
- [x] 3.7 验证扫描与批量导入在小规模代表场景下可安全提交并恢复，确保批次边界不延后唯一权威落盘、不重复分配身份且 SQLite 故障可由目录追赶；记录基本提交次数与 IO 观察，但不要求 10k 量级测量或消除所有全链扫描（LD-01/02/10）。

  验证（2026-10-26）：新增小规模 scan/import 观测测试：3 首扫描各产生 1 个目录 commit（每批 1 条 Song after-image + 1 项媒体资源），不变重扫没有新增 commit；2 首导入产生 4 个目录 commit（每首先预留稳定身份、再提交 Song after-image），包含 2 条 Song 与 2 项媒体资源。失败恢复由 scan `saved identity before projection`、import `recovery commits the directory record before the SQLite row` 及 `fresh database` recovery 测试覆盖；重试保留同一 UUID，不延后首次目录提交。命令：`cargo test -p echo-core --lib small_scan_commits_one_record_and_resource_per_song`、`cargo test -p echo-core --lib small_import_records_reservation_and_song_commit_per_success`、`cargo test -p echo-core --lib scan`、`cargo test -p echo-core --lib import`。记录的是协议级目录提交与资源声明次数，不是原生文件系统 syscall 计时；原样 513 与 10k 测量仍属于后续独立优化。逐项背景见 [性能续作清单](evidence/performance-20261024.md)。
- [x] 3.6 增加平台 FS 适配说明与协议恢复文档（可放 `docs/lan-sync-recovery.md` 的本地资料库章节），运行 `cargo test -p echo-core library_durability`，记录 POSIX/Windows 目录同步/替换能力及不支持时的明确失败语义（LD-09/10）。

  验证：2026-10-24 `library_durability --lib` 52/52 通过；`docs/lan-sync-recovery.md` 的“文件系统适配与失败语义”核对了现有 POSIX/Windows 实现，明确 Windows/Linux 尚待原生验收。此项不替代 10.3。

## 4. SQLite 可重建投影

- [x] 4.1 新增顺序 migration 保存 applied operation、本库投影水位和必要 typed state，不改已发布迁移；验证支持的当前数据库升级、重复打开、migration 回滚失败与错误库 ID，执行 `cargo test -p echo-core migration`（LD-03/04/10）。

  验证：2026-10-24 migration 20/20 通过，旧 v27 升级/失败回滚/重开保留证据及独立审查见 [SQLite 投影迁移证据](evidence/projection-migration.md)；该数据库 schema migration 与旧音乐库数据回填无关。
- [x] 4.2a 协调器核心：修缺陷 1（歌单重试丢稳定操作身份）与缺陷 3（`durability_projection/retained.rs` 保留成员被无条件删除），落实投影 MUST 从 SQLite 已记录的投影水位出发按目录链顺序幂等追赶，per-operation 闭包只是触发点而不得各自直写业务表；普通成功发生于两阶段完成，DB 失败返回已保存待投影；建立可控次序 fake 骨架，先只覆盖已接入入口（喜欢、歌单增删改名、成员增删、封面、播放记录），验证绝无用户 DB-first 写入，且 C1 投影失败、C2 成功后的重启/追赶使 C1 事实也进入投影（LD-01/02）。实现可先采用简单串行方案；是否增加并发、缓存或队列由实际测量决定，不设吞吐门槛。4.2a 完成后放行 5.1 / 5.2 / 6.1 / 6.3 / 6.4 / 6.5 / 7.1 / 7.3 / 7.4 / 8.1 / 4.3。

  进度：2026-10-25 依 F1 拆分自原 4.2；协调器骨架（`plan_library_change` 统一"事务内规划→显式回滚→目录提交→恢复→按目录链顺序 `project_commits` 追赶"）与已接入入口清单见 [交接](evidence/handoff-20261025.md)。剩余 DB-first 入口（`recover/mod.rs`、trash 等）归 4.2b。

  验证：2026-10-25 缺陷 1（重试从目录基线补全已保存歌单并返回原身份，追赶失败经 `saved_pending_catch_up` 归因到首个未投影提交的稳定操作身份）与缺陷 3（`retry` 仅在成员可重投影时释放旧行，保留成员被阻塞时已投影行不丢）均经 RED→GREEN：`playlist_create_retry_after_saved_pending_keeps_operation_identity`、`retained_member_with_blocked_reorder_keeps_projected_row`（暂存修复复跑确认旧代码丢行 left:1/right:2）。可控次序 fake 骨架 `RefusingAuthority`（经 `configure_library_control` 安装、按开关拒绝目录提交）覆盖喜欢/歌单增删改名/成员增删/封面/播放记录 8 个入口，断言拒绝时 15 张业务与投影表行数指纹及目录 replay 完全不变、解除后同权威恢复正常提交投影；memory fake 增补同名重试补全已保存歌单。`cargo test -p echo-core --lib library_durability` 180 通过/0 失败、`--lib memory` 12/0、`--lib` 全量 869/0/1 ignored、`cargo clippy -p echo-core --lib --all-targets` 0 警告、`cargo fmt --all --check` 通过。已知范围外：echo-sync `ledger_protocol` 2 例回归（缺陷 4）仍使 `pnpm verify:governance` 退出 101。详见 [4.2a 收尾](evidence/4.2a-closeout-20261025.md)。
- [x] 4.2b 全部入口改造：`recover/mod.rs`、`trash.rs`、`delete.rs`、`restore.rs`、`continuation.rs` 及四处 per-operation `project` 闭包全部改到协调器，"绝无用户 DB-first 写入"的可控次序 fake 扩展到全部入口并全绿，方可在汇总处勾选 4.2；6.6（删除 DB-first materializer）与 9.6（CodeGraph 清理复核）均以 4.2b 为前置（LD-01/02/10）。前置：5.1、6.4、6.5。

  进度（2026-10-26）：修复 `trash::finalize_persisted_trash` 的具体 DB-first 窗口：`Finalized` intent 与 typed Song Tombstone 同一稳定目录 batch 提交后才运行 SQLite 删除；相同 batch 重试复用已冻结 after-image。新增目录提交拒绝、数据库回滚后保留 Tombstone、同 batch 恢复测试。另修 candidate root continuation 失败后仍 plain-scan、可能重铸 UUID 的路径：除不支持的新 manifest 外，continuation/准入错误现会阻止候选扫描，测试确认 scan generation 为 0。

  完成（2026-10-26）：production continuation 将 V1/V2 snapshot 按 typed identity 和 `(HLC, revision, tombstone tie-wins)` 规则选出未被真实 replay 覆盖的基线，经 SQLite SQL-only legacy projector 投影；随后只按验证过的目录链推进水位。legacy 投影不写回目录、不制造 commit/watermark/outbox。`RestoreLibrary` 与 scan/import/watch/recovery 的 song 写入共用 `commit_song`，先追赶目录已保存前缀，再执行调用点投影。过期回收将 Finalized intent 与 typed Song Tombstone 同批提交，目录提交成功后才删 SQLite catalog。真实 SQLite `RefusingAuthority` 可控次序测试覆盖删除拒绝、撤销拒绝、恢复重试、回收提交拒绝后同操作重试；RED 发现并修复了恢复条目已 `Restored` 但歌曲仍 `PendingDelete` 时被错误排除的问题。scan/watch/import/restore/continuation/root-switch 既有及新增定向验证均通过。详见 [trash Tombstone 顺序及 continuation 失败保护](evidence/4.2b-trash-tombstone-progress-20261026.md)。音乐标签仍只由 Echo 读取，不写回源文件。
- [x] 4.3 实现从目录重建临时 SQLite、完整性比对后替换活动投影，隔离损坏/落后/领先或其他库数据库；投影 MUST 容忍悬空父引用（可解析子集入正表，未解析项入 typed `library_recovery_records` 并计 partial），不得因单条悬空引用使重建整体失败；验证不依赖旧数据库、重建无上传回声、失败保持旧活动库（LD-03/06/07）。

  进度：2026-10-25 悬空引用保留与依赖补齐后自动关联已由 4.5 覆盖；临时 SQLite 重建、完整性比对后替换活动投影、损坏/落后/领先/异库隔离与"失败保持旧活动库"的验收未完成。

  实施进展（2026-10-26）：新增独立临时 SQLite 候选投影，校验 manifest library ID、replay/head、水位、quick check、foreign key 与完整 typed retained evidence；候选悬空引用按 partial 保留，且不得生成 outbox。`project_library_commits` 的增量投影失败时会核对 manifest 和最新目录 head，再尝试原子 root 替换。活动事务替换该 root 的歌曲、歌单、扫描摘要、搜索镜像、艺术家封面与 library projection 证据；跨 root 行保持不变，未完成文件操作会阻止替换。

  收尾（2026-10-26）：三项残留已闭合。(1) **全局 SQLite 损坏**：`SqliteDatabase::open` 在任何读取之前按字节原样把损坏文件及 `-wal`/`-shm` 重命名隔离到同目录唯一名，再建空的已迁移投影继续启动；损坏判定刻意收窄——写锁、只读卷、缺权限、已发布迁移校验和不匹配仍让打开失败并保留原文件。(2) **sync context root**：协议行是目录证据而非读模型投影（无协议表外键引用 `songs`/`playlists`，只有 `retry_library_sync_state` 可在已验证会话下重投影），LD-07 要求重建保留未确认意图与冲突，因此重建前快照选中上下文与 19 张 live/19 张 archived 协议表行数，投影后比对，任何变动即回滚整个事务——由拒绝改为证明不变。(3) **组合矩阵**：落后、领先、异库、断链、候选内容损坏、SQL 注入失败、最终 COMMIT 失败、只读库、坏库隔离均有真实 SQLite 用例。协议历史保护做了 RED 验证（临时关掉比对后用例报 `rebuild must reject changed protocol history`，恢复后通过）。命令：`cargo test -p echo-core --lib library_durability` 230 passed / 0 failed、`cargo test -p echo-core --lib projection_rebuild` 17 passed、`cargo clippy -p echo-core --all-targets -- -D warnings` 0 警告、`cargo fmt --all --check` 与 `git diff --check` 通过。详见 [投影候选与活动替换证据](evidence/4.3-projection-rebuild-20261026.md)。
- [x] 4.5 把 `library_recovery_records` 的键改为稳定 typed key（不得使用 `Debug` 格式），并提供读回路径使 confirmed/pending/悬空引用在重建时可被重新尝试；验证补齐依赖后重试自动关联，无需重新导入（LD-06/07）。

  验证：2026-10-25 稳定 serde typed key、confirmed/pending 读回及重开、追加父对象自动关联、真实目录依赖补齐重试均在临时副本实际通过；原工作区测试因缺失导入模块 E0583 未能执行，不计为门禁通过。见 [本次复核](evidence/progress-closeout-20261025.md)。collector、全入口协调器与 adoption 仍分别留在对应未完成任务。
- [x] 4.4 为投影边界及 SQLite 损坏/只读/commit 失败补集成测试并更新数据库恢复说明，运行 `cargo test -p echo-core library_durability` 和 `cargo test -p echo-core continuation`（LD-01/02/03/10）。

  进度：2026-10-25 上述两条命令现已可在原工作区直接执行（`library_durability` 过滤命中 178 条，当前 177 passed / 1 failed，失败为缺陷 1 的 RED 用例）；投影边界、坏库隔离与只读/commit 失败的专项测试与恢复说明仍未补齐，本轮未据此勾选。

  验证（2026-10-26）：投影边界专项测试已齐——坏库隔离（`corrupt_sqlite_open_quarantines_the_original_and_starts_fresh`、`open_never_quarantines_a_healthy_database_it_cannot_upgrade`）、只读库（`read_only_projection_database_keeps_the_active_view`）、SQL 写入失败回滚（`rebuild_failure_rolls_back_the_active_projection`）、最终 COMMIT 失败回滚（`rebuild_commit_failure_keeps_the_previous_projection`）、候选校验与 root 隔离。`docs/lan-sync-recovery.md` 的 `### SQLite 投影重建` 已更新：补上冷启动隔离语义（字节原样、从不读取/修复/删除、仅文件损坏才隔离、锁/只读/权限/校验和不隔离）、只读与提交失败的重试路径，以及 sync context 协议历史不因重建丢失的约束。命令：`cargo test -p echo-core library_durability` 全部目标 1 + 230 + 1 passed / 0 failed，`cargo test -p echo-core continuation` 17 passed / 0 failed，`cargo clippy -p echo-core --all-targets -- -D warnings` 0 警告，`cargo fmt --all --check` 与 `git diff --check` 通过。

## 5. 本地导入、扫描与监听接入

- [x] 5.1 导入改为资源校验安全发布→目录提交→SQLite 投影，预留 UUID/added_at/claim 在目录可恢复，去除 DB commit 后拼 SongRecord；明确“预留 claim”与“提交 Song”两段操作身份的自识别与互引用键，重试不得重新分配 UUID，界面只报一个导入结果；以导入边界故障、失去 SQLite 后重建测试验证，运行 `cargo test -p echo-core import`（LD-01/02/03；safe-file-ingestion）。

  进度（2026-10-25 收尾）：`application/recover/mod.rs` 的 DB-first 残留已消除——`commit_record` 改为复用 live 导入的 `scan::durable::commit_song`（目录批次含 portable Song record + Claim 先提交，SQLite 投影在闭包内随后运行），`materialize_recovered_song` 整个删除，`recover/` 与 `import/` 下已无 `portable_materialize` / `write_record_guarded` 调用。两段操作身份的自识别互引用成立：预留 claim 以 `operation.as_uuid()` 为 state id 携带 `song_uuid`/`target`/`content_hash`/`added_at`（`import/execution.rs:20-51` 的 `persist_reservation`），Song 提交复用同一 `SongId`（`recover/mod.rs:847` `song_from_parsed(song_id, …)`），恢复侧 `reserved_claims()`（`recover/mod.rs:220-239`）一次 replay 扫描把每个 operation 的 `added_at` 映射出来，重驱动不再取恢复墙钟。RED→GREEN 五条新用例：`recovery_commits_the_directory_record_before_the_sqlite_row`（在 `RecordWrite` 崩溃，断言 SQLite 行此时必须为空）、`recovery_reuses_the_reserved_added_at_not_the_recovery_clock`（推进 24h 仍取 claim 值）、`batch_crash_keeps_the_unstarted_sibling_importable`（批量中途崩溃后未启动的兄弟文件仍可独立导入）、`one_recovery_run_converges_every_incomplete_import_operation`（一次 `run()` 收敛两个未完成操作，各自保留自己的 UUID 与 `added_at`；把 claim map 改成只留一条会红）、`library_durability_import_recovery_rebuilds_reserved_identity_in_a_fresh_database` + `library_durability_claim_alone_never_becomes_a_song_in_a_fresh_database`（wipe 后仅凭目录 replay 重建出同一 UUID/`added_at`；只有 claim 没有 Song record 时重建**不得**凭空造歌）。验证：`cargo test -p echo-core --lib` 884/0/1、`import` 61/0、`library_durability` 190/0、`cargo clippy -p echo-core --all-targets -- -D warnings` 0 警告、`cargo fmt --all --check` exit 0。历史复核曾因歌词结果未贯通 UI 而保留未勾；该缺口现已按后续证据修复。

  进度（2026-10-25 续）：修复并闭合“音频导入成功、同名 `.lrc` 失败”被 IPC 丢弃的问题。`ImportResultDto` 的 `imported` / `savedPending` 现在保留独立的 `LyricsImportResultDto`；Tagged enum 显式使用 `rename_all_fields = "camelCase"`，避免 serde 产生与生成 TypeScript 不一致的 snake_case 字段。导入分类器仍将音频成功计为成功，但把歌词失败列入逐文件结果；对话框明确显示音频已导入、歌词未导入，并按原批次顺序展示。`cargo test -p echo-core import`（61/61）、Desktop IPC DTO 定向测试（1/1）、相关前端测试（54/54）、TypeScript 类型检查、IPC 生成、fmt、diff check 和严格 OpenSpec 验证均通过。证据：[歌词部分成功 IPC/UI 收尾](evidence/5.1-import-lyrics-ipc-20261025.md)。5.1 现已完成；IO 成本观测与原生跨平台发布/权限故障验证保留在各自后续的 9.4 / 9.3 任务，不再作为 5.1 的额外门槛。Echo 只读取外部标签，不回写源音乐文件。
- [x] 5.2 改造扫描 `persist_parsed_index` 及刷新路径，新增/改名/标签变化先提交 Song；无变化不生候选，missing/卷卸载仅本机可用性；以扫描后 wipe/改名/标签/只读/重复事件测试验证，运行 `cargo test -p echo-core scan`（LD-01/03）。

  进度：2026-10-25 `cargo test -p echo-core scan` 52/0/0 通过，目录优先已接线；但"无变化不生候选"未成立——`application/scan/durable.rs:211-221` 在业务字段未变时早退走 `project()`，该分支仍 `upsert_song` → `statements.rs:129` `enqueue_object_snapshot` → `sync/sync.rs:134`，后者计算 payload digest 但不去重，无条件插入 `sync_mutations`/`sync_outbox` 并推进 `sync_local_highwater.local_revision`，违反 `local-library` delta"无变化重扫不增加 revision、播放计数或同步候选"；另：目录侧只抑制了对象版本，SQLite 侧的同步候选仍被铸出，两者不是同一件事。测试面尚缺扫描后 wipe 重建用例（`continuation/tests.rs:524` 与 `restore.rs:260` 都是直接写入记录，未经扫描）、缺扫描侧的重复事件专项与真实只读卷扫描用例；且改名用例在 `application::watch::durability_tests`，测试名不含 "scan"，`cargo test -p echo-core scan` 过滤器只命中 4 条 `library_durability_scan_*`，改名覆盖不被任务指定的命令执行。

  进度（2026-10-25 续）：**"无变化不生候选"已修**——`upsert_song` 的同步发射点改走 `sync::enqueue_song_snapshot_if_changed`（按对象最新非 rejected mutation 的 payload_digest 精确比对，相同则返回既有 revision，不推进 highwater、不插 mutations/outbox；其余发射点与 baseline 重发保持无条件铸造——enqueue 层全局去重会吞并发 favorite fact 与 host baseline 重发，已试错回退）。RED→GREEN：`infrastructure::sqlite::sync::queue::tests::unchanged_reupsert_does_not_mint_another_candidate_or_advance_the_revision`（修前 left 2 / right 1）。`cargo test -p echo-core --lib` 870/0/1、`scan` 过滤 52/0/0、`cargo test -p echo-desktop` 全目标 0 failed、clippy 0 警告、fmt 通过。详见 [收尾记录](evidence/defect4-rescan-closeout-20261025.md)。**5.2 仍不勾选**：扫描后 wipe 重建、扫描侧重复事件专项、真实只读卷扫描用例与改名用例不被 `scan` 过滤器命中的缺口均未动。

  进度（2026-10-25 本轮补证）：新增真实 SQLite 扫描链用例 `sqlite_scan_reparsing_unchanged_song_does_not_create_a_candidate`：通过同字节重写触发重解析，确认候选身份和对象 revision 不变。`cargo test -p echo-core scan` 53/53、该回归 1/1、`cargo fmt --all --check` 和 `git diff --check` 通过。其余 wipe 重建、扫描侧重复事件和真实只读卷专项仍未闭合，故不勾选。

  最终验收（2026-10-25）：新增 `sqlite_scan_after_projection_wipe_rebuilds_the_saved_identity`，用空 SQLite 投影库和原目录控制面执行真实扫描，恢复相同 Song UUID、added_at 和标签；已有同哈希移动、外部标签变化、只读控制面拒写、missing/卸载/权限恢复、重复扫描用例覆盖其余场景。`cargo test -p echo-core scan` 54/54、`cargo test -p echo-core watcher` 18/18、Core all-targets Clippy、fmt 与 diff 检查通过。旧的进度段记录了当时尚未补齐的情况；本段为最终验收结论。
  复验补缺（2026-10-26）：桌面生命周期测试发现目录 replay 会把新 Song 先投影为 `missing`，成功解析后的本地索引更新刻意不覆盖 availability，导致本次扫描创建的歌曲仍不可用。创建分支现于解析和投影成功后显式写入 `available`；`cancellation_during_final_directory_commit_never_marks_other_songs_missing` 与 Core `scan` 58/58、Desktop `--lib` 444/444 通过。
- [x] 5.3 打通导入、扫描和同步发布的 claim/UUID 复用，保留有界重处理并发；用双 worker 屏障、watcher 抢先、取消、同 hash/不同内容目标冲突测试验证不重复、不覆盖（LD-02/05）。

  验证（2026-10-26）：真实 SQLite 屏障回归覆盖扫描解析停住时导入先发布同 hash 内容，扫描放行后从最新目录/SQLite 权威快照复用导入 UUID，不再从扫描启动时的旧快照铸造第二个身份。watcher 现先读持久化未完成 journal claim；运行时 operation 列表漏项时也复用预留 UUID，CopyPending 与 FailedRecoverable claim 都延后事件直到操作恢复。同步发布与本地 operation 在各自事务中互查同 root、规范化 target 的活动 claim；同路径只有一方能持有，释放后可继续。取消/并发上限由 cancel_mid_scan_never_marks_missing、stress_scan_bounded_hashes_and_cancels_without_corrupting_songs、bounded_workers_never_exceed_the_configured_bound 覆盖；同 hash 去重与不同内容目标避让/不覆盖由 precommit_dedup_catches_a_concurrent_duplicate_without_a_second_song、same_name_different_content_gets_minimal_conflict_number、publish_never_overwrites_a_target_created_after_planning 覆盖。cargo test -p echo-core --lib scan 58/58、import 63/63、watch 27/27、sync_resource_publication 21/21 通过。代码与 RED/GREEN 证据见 [claim arbitration](evidence/claim-arbitration-20261026.md)。标签仍为只读；Echo 只读取外部软件写入的音乐文件标签。
- [x] 5.4 更新导入/重扫的恢复与逐项结果说明，运行 `cargo test -p echo-core library_durability` 加上述入口测试，记录歌词失败与已保存待投影如何分别返回，不用整体 toast 掩盖部分失败（LD-01/06/10）。

  进度：2026-10-25 导入侧逐项结果已分离（`ImportResultDto` 六种 kind；失败弹窗逐条显示；`savedPending` 带稳定 operationId 并提供同操作重试），但**歌词失败在 IPC 层被丢弃**：Core 的 `ImportOutcome::Imported`/`SavedPending` 携带 `lyrics: Box<LyricsImportResult>`（含 `Failed { code, message }`，`application/import/logic.rs:66-92` 注释明确"歌词复制失败 MUST 单独报告并不得伪装为完整成功"），而 `crates/echo-desktop/src/ipc/dto.rs:465-508` 的 `From<ImportOutcome> for ImportResultDto` 用 `..` 丢弃该字段，界面只看到"已导入"；重扫侧只有聚合计数，逐文件诊断写入 SQLite `scan_issues`（含 `lyrics_limit` 等码）但无 IPC 出口，界面拿不到逐项结果。恢复与逐项结果说明未更新，故不勾选。

  完成（2026-10-26）：歌词失败及 `savedPending` 原有的 Core→IPC→逐项 UI 路径保留并回归覆盖；扫描新增 `ScanRunRepository::issues(root, generation)` 读回、`ScanSnapshot.issues` IPC 类型及 IPC 生成物，App 菜单和设置页重扫都显示每个相对路径、诊断及歌曲记录是否保留，不再只显示汇总失败数。证据见 [逐项扫描结果与恢复反馈](evidence/5.4-scan-item-report-20261026.md)。验证：`cargo test -p echo-core --lib library_durability` 195/195、`cargo test -p echo-core --lib scan` 58/58、`cargo test -p echo-desktop --lib` 444/444、相关 Vitest 45/45、Prettier、TypeScript、IPC 生成通过；Core Clippy 复验待 4.3 实现切片合并后统一跑。Echo 只读取外部软件写入的音乐标签。

## 6. 喜欢、歌单、统计和资源编辑接入

- [x] 6.1 Core 收藏/歌单/成员用例统一目录提交，桌面服务仅转发；补 true→false、空歌单、重命名、多目标重复成员、时间/顺序、级联删除的 wipe round-trip 测试，运行 `cargo test -p echo-core playlist`、`cargo test -p echo-desktop favorite`（LD-01/02/03）。

  进度：2026-10-25 目录优先与桌面纯转发已就位：`application/playlist/durable.rs` 六个用例全部委派协调器，桌面侧 `runtime/services/playlists.rs`、`favorites.rs` 只做转发；`cargo test -p echo-core playlist` 54 passed / 1 failed、`cargo test -p echo-desktop favorite` 4/0/0 通过。wipe round-trip 仍不完整：新建空库的重建用例只有 `lifecycle/favorite.rs:123`、`lifecycle/playlist_cover.rs:42,248`、`lifecycle/playlist_delete.rs:49`（级联删除）三处，重命名（`playlist_rename.rs:85`）、成员添加（`playlist_members.rs:108`）与成员移除（`playlist_remove.rs:107`）都只在同一库内重投影；`application/continuation/tests.rs:524` 的真实规模 wipe 重建只覆盖 87 首歌曲与 11 条喜欢记录（3 条 true）、0 个歌单；空歌单用例是本轮新增，且因"歌单创建重试丢稳定操作身份"缺陷被 `#[ignore]`（该标记已于同日夜间解除，用例转为 RED，套件按设计带 1 个失败）。故不勾选。

  进度（2026-10-25 深夜续）：**四处缺失的 wipe round-trip 已补齐**——`lifecycle.rs` 新增共用 `fresh_database`（tempdir 内新建空 `SqliteDatabase` + 仅 upsert 库根），空歌单（`playlist_create.rs`，重建后同名同身份且成员 0）、重命名（`playlist_rename.rs`，重建后 "latest rename"）、成员添加（`playlist_members.rs`，双歌单成员身份/位置/顺序保持）、成员移除（`playlist_remove.rs`，重放"移除→重加"后携带重加身份而非被移除身份）四处用例各自追加大范围重建断言（`project_library_commits` 两轮幂等）。验证：`cargo test -p echo-core playlist` lib **57 passed / 0 failed**（此前 54/1）、`cargo test -p echo-desktop favorite` 4 passed / 0 failed、`cargo test -p echo-core --lib library_durability` 180/0、`--lib` 全量 870/0/1、clippy 0 警告（两处超长测试按仓库惯例 `#[allow(clippy::too_many_lines)]`）、fmt 通过。**6.1 仍不勾选**：`true→false`、多目标重复成员、时间/顺序、级联删除等既有覆盖未逐一按 delta spec 复核，且 6.1 首句"统一目录提交"的桌面侧复核（`runtime/services` 仅转发）本轮未重验。
  进度（2026-10-25 缺陷 2 收尾，归属 6.1）：handoff 缺陷 2（歌单投影时间戳）已修——`durability_projection/records.rs` 歌单/成员 INSERT 回退 `unwrap_or(0)` 改为 `unwrap_or(hlc.wall_secs*1000)`（HLC 秒 → 列毫秒），`ON CONFLICT` 保持既有行 `created_at=playlists.created_at`/`added_at=playlist_songs.added_at`，`updated_at=MAX(playlists.updated_at,excluded.updated_at)` 单调推进；并修复被其掩盖的成员侧击穿：`retained.rs` retry 释放循环从整行 `DELETE` 改为把旧行泊位到所在歌单末尾空闲槽（`UNIQUE(playlist_uuid,position)` 槽照常释放、换位/交换语义不变，行连同真实 `added_at` 存活后经 DO UPDATE 恢复真实位置；`CHECK(position>=0)` 因此选末尾追加而非负数占位；LD-08 的 unresolved 行不动语义不变）。RED→GREEN：新增 `library_durability_projection_keeps_existing_playlist_timestamps_across_replays`（RED 实测 `(0, 1699999000000)` vs 期望 `(1699999000000, 1700000100000)`、成员 added_at 被回退值覆盖）与 `library_durability_projection_derives_missing_timestamps_from_the_record_clock`（RED `0` vs `1700000000000`）。验证（命令 + 目标 + 命中数）：`cargo test -p echo-core --lib durability_projection` RED 阶段 37 passed / 2 failed → 修复后 **39/0**；`--lib durability` **192/0**、`--lib lifecycle` **31/0**、`cargo test -p echo-core` 全量**真实 exit 0**（lib 877 passed / 0 failed / 1 ignored 既有性能基准，集成目标全绿，日志 /tmp/echo-core-full-1025.log）、`cargo clippy -p echo-core --all-targets --all-features -- -D warnings` 0 警告、`cargo fmt --all` 通过。**6.1 仍不勾选**：上段"true→false、多目标重复成员、时间/顺序、级联删除按 delta spec 逐项复核 + 桌面侧转发重验"的剩余口径不变。

  进度（2026-10-25 6.1 复核收尾，勾选）：delta spec 剩余口径逐项闭环——**true→false**：识别出唯一真实缺口是"删除与取消喜欢不复活"场景要求 fresh 重建后 false 不被旧 true 记录复活（既有 `favorite_failure_is_saved_and_later_false_catches_up` 只做同库追赶、`favorite_time_survives_toggle_and_fresh_projection` 的 fresh 重建落在最终 true 态），新增 `lifecycle/favorite.rs::library_durability_favorite_false_survives_fresh_rebuild`（true→false→`recover`→`fresh_database` 重建→两轮幂等投影→断言 `!favorite()` 且重建库 `favorited_at` 为 NULL），同组 3/3 通过；**多目标重复成员**：`playlist_members.rs` 的 `&[first, second, first]` 用例已含去重（2 成员、身份互异、重复 append 不产生新 commit）+ fresh 重建身份/位置保持，无需新增；**时间/顺序**：缺陷 2 两用例 + `favorite_time_survives_toggle_and_fresh_projection` 覆盖；**级联删除**：`playlist_delete.rs` fresh 重建覆盖；**桌面侧仅转发**：重验 `crates/echo-desktop/src/runtime/services/playlists.rs`（create/rename/cover/delete/add/remove）与 `favorites.rs`（set_favorite）全部经 `library_edits() -> DurableLibraryEdits` 转发。缺陷 2 修复后任务命令复跑（命令 + 目标 + 命中数）：`cargo test -p echo-core --lib playlist` **58 passed / 0 failed**（上轮 57 基础上 +1 为缺陷 2 新用例名含 "playlist" 被过滤器命中）、`cargo test -p echo-core --lib lifecycle` **32/0**（+1 即新 true→false 用例）、`cargo test -p echo-desktop favorite` **4 passed / 0 failed** 全 target exit 0、`cargo clippy -p echo-core --all-targets --all-features -- -D warnings` 0 警告、`cargo fmt --all --check` 通过。任务文字全部口径（统一目录提交、桌面仅转发、六类 wipe round-trip、两条命令）均已落地验证，勾选。
- [x] 6.2 歌单封面和既有覆盖资源迁入内容寻址原件，读模型 cache key 派生；验证清空全部应用缓存后离线恢复、丢原缓存的历史缺口报告和相同资源复用，运行 `cargo test -p echo-desktop cover` 及 `cargo test -p echo-core library_durability`（LD-06/08）。

  进度：2026-10-25 歌单封面已迁入内容寻址原件（`lifecycle/playlist_cover.rs` 有 5 条用例与两处新建空库重建），但**艺术家自定义封面仍违反 LD-08**：`crates/echo-desktop/src/runtime/services/catalog.rs:161-199` 的 `set_artist_cover` 只把用户字节写入应用 cover cache，落库仅存 cache key（`set_artist_cover_key`），没有写入 `echo/assets/<hash>` 的可校验原件与对象引用；"清空全部应用缓存后离线恢复"与"丢原缓存的历史缺口报告"亦未验证。故不勾选。

  验证（2026-10-26，勾选）：上述缺口已在 `802ba1c` 补齐——新增 `application/artwork.rs`（1 字节–5 MiB、jpeg/png/webp 校验；`save` 写 `echo/assets/<blake3>` 并产出 `PortableAssetReference`；`resolve` 读原件并逐字节复核长度+哈希后才落派生 cache key；`resolve_legacy` 对只有旧 cache key 的历史封面显式报"历史封面无可恢复原件"）与 `domain/library/durability/artist_cover.rs`（`echo.library.artist-cover.v1` 类型化状态，`validate_artist_key` 要求 NFKC 归一、非空、≤4096，`id()` 由库 UUID + artist key 派生稳定身份），歌单封面 `application/playlist/cover.rs` 改为共用同一模块（删除本地重复 `validate`），艺术家封面经 `application/artist_cover.rs` 走 `library_edits()` 目录提交，`catalog.rs` 只转发。三类用例覆盖任务全部口径：**清空全部应用缓存后离线恢复**——`runtime/services/tests/artist_cover.rs` 与 `runtime/services/tests/playlist_cover.rs` 在 `cover_cache.gc(&[])` 清空后仍由目录原件重建出相同 `cover_key` 且字节一致；`lifecycle/artist_cover.rs` 进一步在删掉 SQLite 与 cache 后从 `echo/assets/<hash>` 恢复（断言不产生 `echo/write.lock`、`sync_mutations` 为 0、缩略图可解出 4px 宽）；**丢原缓存的历史缺口报告**——`lifecycle/playlist_cover.rs::library_durability_playlist_cover_legacy_cache_gap_is_explicit` 与 `lifecycle/artist_cover.rs::..._reports_historical_gap_and_missing_original_without_writes`（原 cache 被 gc 后 `resolve_legacy` 报错、目录提交不变；原件被改写或删除时 `resolve` 报冲突/不可用且不落 cache）；**相同资源复用**——同一字节写两次只产生一个 `echo/assets` 文件且 `recover` 逐字段相等（`assert_eq!(replay, before)`），清空封面后原件仍保留（"clear retains historically referenced originals"）。命令（目标 + 命中数）：`cargo test -p echo-desktop cover` **21 passed / 0 failed**（lib 20 + 集成 1）、`cargo test -p echo-core library_durability` **214 passed / 0 failed**（含本轮新增 6.3 用例 1 条）、`cargo test -p echo-desktop recorder` **9 passed / 0 failed**。勾选。
- [x] 6.3 播放完成统计以目录稳定事件先提交设备桶再投影，失败不打断播放但可追踪重试；幂等事件身份存 `echo/state/playback-events/`，共享 `PlayStatsRecord` 只保留设备桶，验证重复 session/重放/ACK/重装不增计、同步导出不含会话标识、资源不可写不留 DB-only 增量，运行 `cargo test -p echo-desktop recorder` 和逐设备统计测试（LD-01/02/03/07/08）。

  进度：2026-10-25 播放事件身份与设备桶已落地（播放记录走同一协调器，`lifecycle/playback.rs` 覆盖会话身份复用），但持久待重试队列、界面可诊断，以及 ACK 丢失 / 重装 / 真正不可写场景的测试仍缺；上述专项未执行完，故不勾选。

  验证（2026-10-26，勾选）：`802ba1c` 补齐持久重试队列与目录证明，本轮补上最后一条"资源不可写不留 DB-only 增量"。**目录稳定事件先提交**——`sqlite/local_library_edit/playback.rs` 在一次 `plan_library_change` 内冻结私有 `PlaybackEvent`（`StateKind::PlaybackEvent`，id = session UUID）、共享 `PlayStatsRecord`（只含 `by_device`）与 `FrozenMutation`；`qualified_at` 作为计划时钟传入（`*_at` 变体同时写 `sync_mutations.created_at`/`sync_outbox.created_at`），使 prepared-but-unsaved 重试重放字节完全相同。**失败不打断播放但可追踪重试**——`platform/playback_retry.rs` 提供 `PlaybackRetryStore`（`*.playback-retry.json`，fs2 独占锁、拒绝符号链接、temp+`sync_all`+rename+父目录同步、损坏/未来版本保留原字节、上限 65536、同 session 异身份拒绝）与九种 `PlaybackRetryReason`；`runtime/player/recorder.rs` 的 `attempt` 按 InactiveLibrary → LibraryChanged → EnrollmentPending → 目录事件证明 → AuthorChanged → MissingSong 顺序裁决，已保存事件只补投影绝不换作者计数，`retry_pending` 1s 节流 + `1<<attempts.min(6)` 退避，`spawn_stats_recorder` 在 `recv_timeout` 超时时驱动重试，错误只进累加器不影响播放。**幂等身份在 `echo/state/playback-events/`**——`durability/tests/playback_privacy.rs` 断言 `echo/state/playback-events/<session>.json` 物化存在且共享 `PlayStats` 记录既不含 session id 也不含 `recorded_sessions`。**重复 session/重放/重装不增计**——`lifecycle/playback.rs` 覆盖同 session 重复记录返回 `false` 且计数不变、投影失败后重放（`SavedPending` 携带同一 operation id、replay 恰 2 次提交）、另一首歌复用 session 报 `Conflict` 且目录不变、新建空库重建后 `play_count` 为 2 且 `(recorded_play_sessions, sync_outbox, sync_mutations) = (2,0,0)`（无上传回声）；`recorder/retry_tests.rs` 覆盖重开后同一 session 只计 1 次、响应丢失时用已保存事件补投影且 `device_play_counts` 求和仍为 1（不换作者）、作者/库/歌曲不可证明时保持 pending 且不计数、非活跃根不重试。**同步导出不含会话标识**——`sync_payloads/deletion_and_stats.rs::recording_a_play_enqueues_a_complete_play_stats_record_the_host_accepts` 断言 `PlayStatsRecord` 完整形状、`by_device[device]==1`、共享 payload 无 session/`recorded_sessions`/`playback_session`、`PortableRecord` 往返、`export_playback_events` 恰 1 条私有状态且 `kind.directory()=="playback-events"`；`recorder/retry_tests.rs` 同样断言共享记录不含 session 文本。**资源不可写不留 DB-only 增量**——本轮新增 `lifecycle/playback.rs::library_durability_playback_unwritable_library_leaves_no_database_only_increment`：注入 `prepared` 阶段不可用后连续两次记录均失败，且 `play_count`/`device_play_counts`/`recorded_play_sessions`/`sync_outbox`/`sync_mutations` 全为 0、目录无任何 `PlaybackEvent`；恢复可写后同一 session 恰好计 1 次、重复调用不再增计、`device_play_counts` 恰为 `{device: 1}`。命令（目标 + 命中数）：`cargo test -p echo-desktop recorder` **9 passed / 0 failed**、逐设备统计 `cargo test -p echo-desktop --lib stats` 与 `cargo test -p echo-core library_durability`（**214 passed / 0 failed**，含本用例）全绿、`cargo test -p echo-core lifecycle::playback` **4 passed / 0 failed**。勾选。
- [x] 6.4 删除/撤销迁入目录本机意图及最终集合，保留 10 秒唯一终态、物理回收独立恢复与候选引用保护；验证窗口内重启、终结/撤销竞争、远端墓碑、未确认导入后删除，运行 `cargo test -p echo-core delete`、`cargo test -p echo-core trash`（LD-02/07/08）。

  验证（2026-10-25）：目录 `DeleteIntent` 在隐藏/SQLite 投影前提交；固定并贯通 `final_mutation_id`；撤销及恢复先取消目录意图，trash 收到持久化 `TrashApplied` 后先终结目录意图再删 catalog 行，重启重试不更换身份；已认证远端增量墓碑会 supersede 同对象所有 open 意图；目录 compaction 保留活跃意图。直接场景覆盖原撤销 deadline 重启保持、终结/撤销两种胜出顺序、远端墓碑、恢复崩溃后取消、未确认资源声明在 catalog 删除/重启后仍有 retention hold 与 receipt。`cargo test -p echo-core delete` 58/58、`cargo test -p echo-core trash` 20/20、`cargo test -p echo-core --lib resource_retention` 2/2、桌面已确认远端墓碑集成 1/1；Core Clippy、fmt 和 `git diff --check` 通过。细节与命令见 [6.4 验收证据](evidence/6.4-progress-20261025.md)。
- [x] 6.5 外部标签编辑和文件移动只读接入同一提交协调器，Echo 不修改源音乐文件；只有目录提交后才更新 SQLite，外部变化失败重试保持原 Song UUID，验证新旧哈希、投影失败重放、移动及缺少媒体时关系保留，运行 `cargo test -p echo-core scan` 与 `cargo test -p echo-core watcher`（LD-01/02/08）。

  验证（2026-10-25）：按产品边界，Echo 不提供音乐文件标签写入或源文件替换；外部编辑器修改后由 scan/watch 读取并提交新事实。`library_durability_scan_refresh_preserves_saved_after_image_on_projection_failure` 覆盖目录先保存新标签/哈希、SQLite 失败、重试保留原 UUID 与 added_at，并断言源文件字节未被 Echo 改写；`library_durability_watcher_rename_saves_identity_before_failed_projection` 覆盖移动的目录提交、投影失败和同身份重试；missing 测试确认目录记录及关系保留。`cargo test -p echo-core scan` 54/54、`cargo test -p echo-core watcher` 18/18、变化恢复专项 1/1、fmt 和 diff 检查通过。源文件旧字节替换保护只适用于将来若增加 Echo 写回功能，不属于本 change。
- [x] 6.6 删除旧桌面 DB-first materializer 路径和无消费者导出/fake，保留只读 builder 或测试辅助的理由；点名清理 `SqliteDatabase::record_playback` 这一公开 DB-first 入口与无调用者的孤儿模块，用 CodeGraph 调用链及 `rg -n 'write_record_guarded|materialize_playlist|record_play_stats|record_playback' crates apps/desktop/src-tauri` 复核所有剩余生产写入口均受协调器控制，运行 `cargo test -p echo-desktop --lib` 和架构测试（LD-10）。

  验收（2026-10-26）：CodeGraph 调用链确认目录写入辅助已无生产调用者；当前删除完成用例先调用 `delete::intent::finalize` 持久化目录墓碑，再以 SQLite 事务删除 catalog 行并释放 claim。播放、收藏和歌单生产写入均经目录协调器；`SqliteDatabase::record_playback`、`materialize_playlist`、`record_play_stats` 已无生产定义/调用。删除 `write_record_guarded`，保留说明为 undo 删除时清理旧临时墓碑的 `delete_record_guarded` 和纯记录 builder；更新过时的播放器注释。指定 `rg` 仅命中两个解释性注释。`cargo test -p echo-desktop --lib` 444/0/0，`cargo test -p echo-core --test arch` 8/8，均通过。
- [x] 6.7 同步更新各入口持久化/错误说明，记录与现有歌单 UI change 的封面字段交汇，验证接口不存在平台绕过 Core 的用户变更，并运行本组 round-trip 集成测试（LD-03/08/10）。

  验证（2026-10-26）：三项口径全部闭环，细节见 [6.7 证据](evidence/6.7-entrypoints-and-cover-intersection-20261026.md)。**入口说明**：`docs/library-durability-protocol.md` 新增 `## 各入口持久化与错误语义`，逐条列出导入、扫描/外部标签/移动、喜欢、歌单（创建/改名/删除/成员）、歌单与艺术家封面、播放统计、删除/撤销/回收、启动续跑、LAN 入站、来源 ACK/主机确认 11 类入口的"目录先提交事实"与"失败语义"，并写明错误只在 Core 分类（`library_saved_pending_recovery`/`library_saved_pending_projection`）、IPC 附带原 `operationId`、`ErrorPolicy` 把两者标为非"新命令可重试"。**封面字段交汇**：新增 `### 与歌单封面 UI change 的字段交汇`，指明唯一交汇字段是 `PlaylistView.coverKey`（由 `PortableRecord::Playlist.cover_asset` 派生，非独立用户值），并逐条对齐 `align-immersive-player-and-playlist-directory` 的 `playlist-management` 规格（预览/保存后同步/重命名不丢封面/失败保留/禁止 localStorage 模拟身份），前端使用点记录在 `PlaylistsView.tsx:59-77,723-725`、`PlaylistNameDialog.tsx:32-48,248`、`AddToPlaylistDialog.tsx:222-226`。**无平台绕过**：`apps/desktop/src-tauri/src` 全目录 grep 直接 SQL 写（`with_tx|writer.run|execute_batch|INSERT INTO|UPDATE|DELETE FROM`，排除 tests）**0 命中**；8 个用户变更 Tauri 命令（`commands.rs:216,303,315,327,339,352,360,376`）全部只调 `services.*` 并映射 `IpcErrorDto`；桌面服务全部经 `library_edits() -> DurableLibraryEdits` 或 `application::artist_cover` 转发；桌面层仅 2 处直接取目录适配器（`services/recovery.rs:44` 只读诊断、`player/recorder.rs:109` 事件证明+投影追赶），均非用户变更旁路。**round-trip 集成测试**（命令 + 命中数）：`cargo test -p echo-core --lib "lifecycle::"` **36/0**、`cargo test -p echo-core playlist` **58/0**、`cargo test -p echo-desktop favorite` **4/0**、`cargo test -p echo-desktop cover` **21/0**、`cargo test -p echo-desktop recorder` **9/0**、`cargo test -p echo-desktop --lib stats` **5/0**、`cargo test -p echo-core library_durability` **214/0**、`cargo test -p echo-core --test arch` **8/0**。其中 `lifecycle/ordered_fake.rs::library_durability_refused_commit_leaves_every_wired_entrypoint_untouched` 是"目录拒绝提交时 8 个入口的 SQLite 指纹与目录 replay 均不变、解除拒绝后同权威可恢复"的联合证据。本任务不包含对方 change 的对话框/预览/焦点实现，也不勾选任何 8.x 内容。

## 7. LAN 首次快照、增量、ACK 与主确认接入

- [x] 7.1 改造远端快照/增量与来源 ACK 为 Core 目录提交用例，先校验全快照再发布可见集合；保留本地资料/候选，SQLite 对象与已应用水位在目录提交后同事务投影；验证快照分页失败/取消、重复 page、metadata-only 喜欢/空歌单、所有提交点崩溃（LD-01/02/03；LAN spec）。

  验证（2026-10-26）：矩阵与两处真实缺陷详见 [7.1 证据](evidence/7.1-snapshot-incremental-directory-first-20261026.md)。**缺陷 A**：`sync/durability/commit.rs` 原以 `operation_status(...).is_some()` 判"已保存"，但只有 `prepared.json`、无提交标记的操作返回 `Ok(Some(NotSaved))`——`Some` 不等于已保存；`"prepared"` 窗口断电后每次重试都进"读已保存操作"分支并报 `saved LAN operation is absent from the verified directory chain`，operation 身份被永久毒化。改为比较 `status != CommitStatus::NotSaved`，prepared-only 操作重新规划提交，冻结正文一致性仍由 `commit` 自身保证。**缺陷 B**：提交闭包只做 `tx.commit()`，从未调 `durability_projection::project_commits_in_transaction`，因此目录提交后业务行可见而 `library_projection_watermarks`/`library_applied_operations` 恒为空——水位证据丢失，`applied_library_commit_seq` 恒为 0。改为闭包内 `durability.recover(root)` → `verify_covered_watermark` → `project_commits_in_transaction(&tx, …)` → `tx.commit()`，行与水位成为同一原子 SQLite 提交，与 `local_library_edit.rs:256-288` 的本地编辑口径完全一致。**新增矩阵** `crates/echo-core/src/infrastructure/sqlite/tests/sync_snapshot_apply_durability.rs` 5 条，全部装真实 `RootControlPlane`（其余快照套件不装控制面，`#[cfg(test)]` 下尾部退化为裸 `tx.commit()`，顺序本身从未被测）：`snapshot_publishes_the_visible_set_only_through_the_directory`（非末页业务行/游标/目录提交全 0；末页 `(2,1,1)`、恰好 1 次目录提交、batch 含 `echo.sync.peer-cursor.v1` 且 `sequence=12`；新库仅凭该目录重放得 `(2,1,1,0)`）、`incremental_page_projects_rows_and_watermark_after_the_directory`（真实主机 `stage_confirmation`→`mark_published`→`published_commits_after` 发布增量提交，接收端 `(1,0,1)` 且水位/`library_applied_operations` 各 1 行，同页重放 `applied=0` 且目录提交仍 1，新库重放得 1 首歌）、`snapshot_commit_faults_recover_with_one_operation_and_no_partial_set`（`prepared`/`committed` 两个发布窗口分别注入故障，均无可见行与游标，`committed` 故障返回 `library_saved_pending_recovery`，恢复后重试同页得 `(0,1,1)`、目录提交仍 1、水位 1）、`discarded_snapshot_leaves_no_directory_operation_and_allows_a_fresh_one`、`snapshot_keeps_metadata_only_favorite_and_empty_playlist_through_the_directory`（metadata-only 喜欢/空歌单/`SongAlias`/`PlaylistMemberAlias` 均进目录 batch，`playlist_songs=0` 不伪造成员，alias 以 `StateKind::Alias` 保留）。**命令 + 命中数**：`cargo test -p echo-core --lib sync_snapshot_apply_durability` **5/0**、`cargo test -p echo-core --lib sync` **202/0**、`cargo test -p echo-core` **lib 955/0/1 ignored** + 集成套件 8/0、7/0、1/0、2/0、19/0、0/0、`cargo test -p echo-sync --all-features` **65/0 + 1/0 + 0/3 ignored + 2/0**、`cargo test -p echo-desktop` **452/0** 等全绿、`cargo clippy -p echo-core --all-targets --all-features -- -D warnings` 0 警告、`cargo fmt --all -- --check` 通过。**未在本项内解决**：新库重放后 `sync_peer_cursors` 为 0——对端游标以 `StateKind::Cursor` 完整留在目录，但 `durability_projection/sync_state/related.rs:17-26` 对 `StateTarget::PeerCursor` 返回 `RequiresStateRecovery`，重放记为未解析；本项只断言"证据保留而非丢失"，游标/资源位置/完成凭据的实际恢复归 7.4，未在此声称已恢复。
- [x] 7.2 持久化真实 confirmed payload、receipt、alias/tombstone 和 pending/前驱，修复旧 sync metadata 附到新值的误确认语义；验证离线 A→B 多候选、迟到 ACK、已发送正文不变、重复成员 alias 与旧删除不影响新成员，运行 `cargo test -p echo-core sync`（LD-05/07）。

  验证（2026-10-26）：矩阵与缺陷详见 [7.2 证据](evidence/7.2-real-confirmed-payload-20261026.md)。**真实缺陷**：目录信封的 v2 `sync` 块随记录落盘，而 `control_plane::write_record` 在非持久批内会**原样携带**旧 `sync` 块，因此"本地新值 + 旧 `object_version`"是可持久化的合法信封；`retained::retry` 又用 `dependencies::unresolved(...)` 重算并**覆盖** `unresolved_reason`，完全不看 `sync`，既无法把这种信封标成待核对（标记会被清掉），也让任何把 `object_version` 当确认证据的读路径可以为一新值建立虚假 confirmed baseline。**修复**：新增 `durability_projection/confirmation.rs::unproven()` ——无 `sync` 块或 `ObjectVersion::INITIAL`（genesis，`filesystem/durability/tests/identity.rs` 构造的正是它）视为"不声明"；否则按信封自带 binding 精确查 `sync_commits(commit_seq,library_id,authority_id,epoch)`，缺失即未证；命中后必须过 `sync::apply::verify_remote_commit`（重算 `echo.lan.commit.v1` 摘要）、`commit.object` 必须等于信封对象（否则是借来的版本号）、且 `agrees()` 逐字段比对账本载荷与磁盘值（只比双方都主张的字段，歌的 `relative_path` 主机作用域不比）。`retry` 在投影前一次性算出 `unproven` 键集，报告态改为 `unresolved.or_else(|| unproven.contains(&key).then_some(UnverifiedConfirmationMetadata))`，因此依赖已解决时**仍投影本地值**（目录是真相）且**仍报告**（不被抹平），同时**不写** `sync_object_versions` baseline，字节从不改写。枚举贯通 `ProjectionUnresolvedReason` → `RecoveryIssueReasonDto::unverifiedConfirmationMetadata` → 生成的 TS 联合 → `LibraryRecoveryStatus.tsx` 文案。**新增矩阵** `durability_projection/tests/stale_sync.rs` 5 条：陈旧元数据（值 `new` 而账本 `old`：投影 `new`、报告未证、保留字节逐字节不变、baseline 0）、值相符即确认（无报告，证明不是"凡有 sync 块就报警"）、借来的版本号（seq 4 确认的是另一歌单）、迟到 ACK 只清报告不改字节、genesis 不声明。**点名场景**（均已执行）：离线 A→B 多候选 `independent_replica_enrollment_preserves_unbound_offline_edits` + `library_durability_sync_export_versions_legacy_reanchor_without_changing_event_id`、迟到 ACK 本项新增用例、已发送正文不变 `queued_request_body_stays_identical_when_local_audio_goes_missing`、重复成员 alias `concurrent_duplicate_playlist_append_persists_alias_in_log_and_snapshot_state`、旧删除不影响新成员 `retrying_an_old_member_delete_does_not_remove_the_readded_member`。**命令 + 命中数**：`cargo test -p echo-core sync` **209/0**、`cargo test -p echo-core --lib durability_projection` **51/0**、`cargo test -p echo-core` **lib 960/0/1 ignored** + 集成套件 8/0、7/0、1/0、0/0、1/0、2/0、19/0、0/0、`cargo clippy -p echo-core --all-targets -- -D warnings` 0 警告、`cargo fmt --all` 已应用、`pnpm generate:ipc` 更新 1 行。**未在本项内解决**：`agrees()` 是字段级比对，账本载荷多出的未建模字段不参与判定，完整 wire 兼容矩阵归 7.8。
- [x] 7.3 主机确认改为规划/序列预留→目录权威提交→SQLite 账本→资源/安全检查点→连续 published→ACK；同一“规划-提交-投影”序列在多个确认分支重复出现时必须抽成一个 helper，不得复制粘贴；验证重复 mutation 同结果、相同 ID 异正文拒绝、每个 publication 窗口断电、ACK 后丢 SQLite 可恢复业务资料，运行 `cargo test -p echo-sync`（LD-01/02/07）。

  进度：2026-10-25 夜间复核：本项指定的验证命令 `cargo test -p echo-sync` 当时**为红**——
  `--all-features` 下 lib 65/0、arch 1/0、`discovery_network` 3 条全 ignored，
  但 `ledger_protocol` 0 passed / 2 failed，报
  `Unavailable { resource: "library durability", hint: "directory commit adapter is not configured" }`。
  根因是本 change 引入的 `sync/durability/commit.rs::commit_prepared_projection` 只在
  `#[cfg(test)]` 下容忍缺失控制面（echo-core 作为 echo-sync 的依赖编译时 `cfg(test)` 为假），
  详见 [交接](evidence/handoff-20261025.md) 的"缺陷 4"。

  收尾（2026-10-26）：**该回归已清除**——`cargo test -p echo-sync --all-features` 现为
  lib 65/0、arch 1/0、`discovery_network` 0 passed / 3 ignored、`ledger_protocol` 2/0，0 failed。
  **helper 抽取**：`sync_ledger/host_ledger.rs:18` 新增 `commit_confirmation(control, tx, mutation)`，
  把四条确认分支（精确 inbox 重试、终态拒绝、对象已删除、新接受提交）共有的
  "序列身份 → 目录权威提交 → SQLite 账本投影"尾部收敛为唯一实现；四个调用点分别在
  `host_ledger.rs:83/137/144/179`，分支只负责往 `tx` 写自己的行，helper 消耗事务，
  因此分支无法在目录接受同一 after-image 之前提交账本，也无法在 `SavedPendingProjection`
  之后继续推进。`grep -c "commit_projection(control"` 剩余裸拷贝 **0**。发布路径
  `mark_published`（`host_ledger.rs:184-229`）仍经 `commit_publication_projection`
  与 `identity("host-published", "<library>:<seq>")` 走同一尾部。
  **验证**（命令 + 命中数）：`cargo test -p echo-core --lib "mark_published"` **12 passed / 0 failed**
  覆盖每个 publication 窗口——目录标记后故障返回 `library_saved_pending_recovery` 且
  `sync_commits.publication_state` 保持非 published、重试复用原 delta
  （`..._saved_pending_recovery_retry_reuses_original_delta`）；目录成功而 SQLite 延迟
  `COMMIT` 失败返回 `library_saved_pending_projection`、目录 delta 保留、修复后同 checkpoint
  重试成功（`..._saved_pending_projection_keeps_directory_delta`）；连续前缀与摘要
  （`mark_published_cannot_skip_pending_predecessor_and_retries_are_idempotent`、
  `mark_published_rejects_missing_predecessor_without_updating_state`、
  `mark_published_rejects_forked_predecessor_digest_without_updating_state`）、
  检查点失败阻塞后续 ACK（`checkpoint_failure_blocks_later_ack_until_pending_predecessor_is_recovered`）、
  并发来源只放行一个 pending（`concurrent_origins_admit_only_one_pending_publication`）。
  **ACK 后丢 SQLite 可恢复业务资料**：
  `..._retains_facts_resources_across_window_and_old_retries` 跑满 35 次真实确认跨过 32 提交
  覆盖窗口，再 `wipe_and_project` 全新数据库并 `read_recovery_records`，断言 pending/处置/
  alias/receipt/cursor/资源位置六类 state 与资源字节逐项保留、旧 checkpoint 重试与旧
  mutation 重放都不改写已发布正文。**重复 mutation 同结果 / 同 ID 异正文拒绝**：
  `host_ledger_stages_confirmation_idempotently_and_tracks_publication`（同 candidate 二次
  `stage_confirmation` 返回完全相同的 `StagedConfirmation`；改正文后 `{"title":"different body"}`
  报错）。`cargo build -p echo-core --lib`、`cargo test -p echo-core --lib sync` 198/0、
  `cargo clippy -p echo-core --all-targets -- -D warnings` 0 警告、`cargo fmt --all --check`
  与 `git diff --check` 通过。
- [x] 7.4 远端资源 publication 保存目录位置声明与完成凭据，对 `media/` 和受管 `echo/media/` 均验证 hash/length/owner/version；验证媒体先后、中途替换失败、Windows 文件占用、旧文件冲突、丢 DB 后关联原 UUID，运行 `cargo test -p echo-core sync_resource_publication`（此过滤器实际覆盖远端 publication 集成测试；`remote_resources` 不匹配测试名，会零测试通过）（LD-02/08）。

  验证（2026-10-26）：实现、边界、五个新增用例和 26/26 命中详见 [远端资源目录持久化证据](evidence/7.4-remote-resource-directory-20261026.md)。目录记录精确的当前 publication 指针，支持 DB 丢失后的离线路径重建；资源完整性由 directory adapter 在完成 marker 前重验。旧资源冲突和文件占用模拟覆盖；Windows 原生句柄行为归 9.3，未冒称跨平台实机验证。
- [x] 7.5 把候选终结、冲突原件和资源引用纳入目录重放/压缩，禁止重建制造 outbox 回声；验证启停/换库/epoch 清理不丢引用，运行 `cargo test -p echo-core resource_retention` 及多设备契约（LD-02/07/08）。

  验证（2026-10-26）：重放资源、终态候选和 epoch archive holds 的 source/scope 与重建行为见 [资源保留证据](evidence/7.5-resource-retention-20261026.md)。目录压缩后 fresh projection、root switch、多次 rebuild 和 no-echo 测试 4/4；host epoch rotation 原件保留测试在同组；`echo-sync` 引用图生命周期契约 1/1。原子重建失败继续保留之前 holds。
- [x] 7.6 更新 `docs/lan-sync-recovery.md` 的 ACK、媒体与已同步边界和重装安全恢复；验证无钥匙串可离线恢复但不自动联网，可信账本缺失阻止旧 epoch，网络协议如需新增资源字段则先补版本协商规格与契约，不能默丢字段（LD-03/07/09/10）。

  验证（2026-10-26）：ACK 仅覆盖 mutation 与 publication checkpoint、媒体 ready 与对象同步的差别、离线目录恢复及信任 ledger/旧 epoch 围栏写入 [恢复文档](../../../docs/lan-sync-recovery.md)，调用链与证据见 [安全恢复边界](evidence/7.6-safe-recovery-boundary-20261026.md)。检查了 Core recovery 只执行本地 replay/project，可信 sync state 仅由已验证 live-session provisioner 消费；`startup_refuses_restored_or_forked_history_before_repair`、`unconfigured_runtime_stays_offline_and_stop_is_idempotent`、`restore_does_not_provision_without_a_binding` 各 1/1。7.8 确认本 change 没有引入必需的新 LAN resource wire 字段。
- [ ] 7.7 实现可信原会话重放与新 binding/作者的显式意图接纳转换，先核对旧 receipt/主规范状态，再持久保存旧→新 mutation 映射，保留原冻结时间/UUID/成员顺序/统计桶；验证旧 ACK 丢失、身份改变、重新配对与转换任一步中断均不重复应用，无法判定只保留待处理；加入旧 A 桶本地 7/主 5、新作者 B 无权抬高 A 或挪差额的场景，旧计数本地保留且 B 只增自己的桶，运行 `cargo test -p echo-core library_durability` 和 `cargo test -p echo-core sync`（LD-07）。
- [x] 7.8 建立本地 manifest、record envelope、LAN wire/capability 的独立兼容矩阵；验证支持的旧目录可读取且读取不改写资料、新协议目录拒绝不支持客户端写入、混版本资源字段按现有协商规则处理且无静默丢字段，并将结果写入协议/恢复文档（LD-04/09/10）。

  验证（2026-10-26）：三条版本轴与实际行为、测试证据、wire未知字段规则及“无新增必需 wire 字段”的范围记录见 [兼容矩阵证据](evidence/7.8-version-compatibility-20261026.md) 和 [协议说明](../../../docs/library-durability-protocol.md)。`manifest_v3_explicitly_reads_legacy_but_refuses_invalid_and_future_versions` 1/1，`library_durability_new_manifest_fences_legacy_writers_without_changing_record_envelopes` 1/1，`portable_v` 4/4，`cargo test -p echo-sync --all-features protocol` 6/6。支持的旧版读取由对应 decoder 与不改写提交链约束；未来 envelope/manifest/wire 版本分别拒绝，LAN serde 对未知可选 JSON 字段的既有忽略行为已明确说明。本变更没有新增必须由旧端理解的 resource wire 字段。

- [ ] 7.9 单机模式及解除同步时彻底清理同步专用持久事实：覆盖 SQLite 队列/投影、目录 state 与纯同步历史 transaction 证据的安全压缩删除；验证清理重试幂等、重启不重新导入、同步上下文保留时资料不误删，并逐项断言歌曲/喜欢/歌单/成员/媒体不变，运行 `cargo test -p echo-core sync` 与恢复投影专项（LAN/LD-02/03/07/10）。

  当前边界（2026-10-28）：解绑和无同步上下文启动已清理 SQLite 同步表、恢复投影/archive 与 epoch tombstone；目录不可变提交链中的纯同步历史字节仍需安全压缩移除，本任务保持未完成，不能据数据库清理宣称所有同步资料已物理删除。

## 8. 类型化 IPC 与用户可见恢复状态

- [x] 8.1 Core→desktop→IPC 提供未保存、已保存待恢复、已保存待投影、已完成、部分恢复及稳定操作身份/分类原因；运行 `cargo run -p echo-desktop --bin echo-generate-ipc` 并核对生成物，序列化测试覆盖各态，不能手改生成类型（LD-01/06/10）。

  验证（2026-10-27）：五态 DTO 与分类原因由生成器产出——`LibraryDurabilityOutcomeDto`（`notSaved` / `savedPendingRecovery` / `savedPendingProjection` / `complete` / `recoveryPartial`）、`LibraryDurabilityReasonDto`（`operationRejected` / `recoveryRequired` / `projectionPending` / `unresolvedProjection` / `legacyUnverified`），挂载于 `IpcErrorDto.durabilityOutcome` 与 `LibraryRecoveryStatusDto.outcome`。`cargo run -q -p echo-desktop --bin echo-generate-ipc` 重写生成物后 diff 仅含生成器新增内容（未手改）；`ipc::` 31/31 含 `committed_generated_file_is_in_sync_with_the_generator` 钉住漂移。序列化测试覆盖各态：`every_outcome_has_stable_tag_identity_and_classified_reason`（逐态 tag/reason/操作身份 + round-trip）、`absent_operation_identity_serializes_as_explicit_null`（`operationId: string | null`）、`rejected_mutation_is_classified_not_saved_and_names_no_operation`（未保存态无操作身份）、`mutation_path_never_downgrades_a_saved_operation`（已保存态不被降级）、`read_errors_carry_no_persistence_verdict`（读路径不伪造持久化裁决）。生产侧：新增 `IpcErrorDto::from_mutation` 供 13 个写入/导入命令使用（此前 `NotSaved` 在生产中不可达），并让 `delete_song_coordinated` 保留 Core 错误（此前已提交待投影的删除被压平成普通拒绝）。Core 侧 LD-01a：`favorite_directory_refusal_is_not_saved_and_leaves_no_projection_fact` 断言目录拒绝时错误非 `LibrarySavedPending`、业务表指纹与目录 replay 均不变、重试只新增一条提交（`controlled_order` 5/5）。命令记录：`cargo test -p echo-core --lib library_durability` 243/243、`cargo test -p echo-desktop --lib ipc::` 31/31、`--lib player::` 240/240、`cargo clippy -p echo-desktop -p echo-core --all-targets -- -D warnings` 与 `cargo fmt --all --check` 通过、`pnpm --filter @echo/desktop typecheck` 通过。UI 消费见 8.2。
- [x] 8.5 为每个新增错误码补齐前端 message key 映射（不得把 key 字符串直接当用户文案显示），并把新码显式列入 `ErrorPolicy` 重试判定表，以断言钉住；验证 `pnpm --filter @echo/desktop typecheck` 与 IPC 漂移检查通过（LD-01/06/10）。
- [x] 8.2 在现有资料库设置/错误详情中呈现新协议恢复状态、只读、未解析记录、待投影与同操作重试；不提供旧库回填进度或历史修复入口。喜欢/歌单乐观状态按提交结果处理，统计错误不中断播放但可查看；通过 React 测试验证焦点、键盘、错误、重新打开与批量逐项反馈（LD-01/06）。

  验证（2026-10-27）：设置侧 `LibraryRecoveryStatus` 按 `LibraryRecoveryStatusDto.outcome` 渲染五态，只读/未解析记录/历史未验证/待投影各有独立文案；待投影提供「更新已保存资料」= `retry_library_projection`（重投影已保存前缀，附「不会创建新的收藏或歌单」说明），不提供旧库回填进度或历史修复入口（`src/features/settings/LibraryRecoveryStatus.tsx`）。播放统计以只读 `PlaybackRetryDiagnostics` 呈现（操作身份 + 原因 + 重试次数，空队列不渲染），打开面板只读一次、不重发原事件。乐观状态统一走 `savedPendingNotice.ts`：`notifySavedPending` 是唯一分类器（结构化 `durabilityOutcome` 优先，回退错误码），已保存待更新保留乐观状态并只按原操作身份重试，仅真正被目录拒绝才回滚并重查；`LibraryWorkspace` / `CollectionDirectory` / `PlaylistsView` / `PlayerBar` / `PlaylistNameDialog` / `AddToPlaylistDialog` 全部接入。批量操作新增第五态 `pending`（`已保存待更新 N`，不计入失败、不触发回滚），且待投影项不再触发滞后投影的重查。修正过程中发现并修复两处真实缺陷：其一，`publishSongUpdate` 原先整体替换行，播放条只持有 `song_detail` 字段，已保存待更新的喜欢会抹掉行的标题/音质——改为**补丁合并**语义（`SongUpdate`），三个订阅点均改为 merge；其二，批量成功后无条件重查在待投影时会用滞后投影覆盖已提交真相。React 覆盖：`LibraryRecoveryStatus.test.tsx` 8/8（只读、待投影+同操作重试、catch-up 失败仍显示「已保存在音乐库」、重新打开重读、未解析记录、历史未验证、状态不可读、播放统计只读、焦点+键盘重读）、`savedPendingFeedback.test.tsx` 7/7（行心保留、按原操作重试、仅有错误码也能识别、真正拒绝仍回滚并重查、播放条心保留且标题不被抹掉、恢复按钮可聚焦且带 action 不自动消失、批量逐项反馈）、`PlaylistNameDialog.test.tsx` 8/8（保留已输入名称、同操作恢复、恢复失败仍称已保存）、`AddToPlaylistDialog.test.tsx` 14/14（待投影计为已保存并可按原操作恢复）。命令记录：`pnpm --filter @echo/desktop format:check` / `lint` / `typecheck` / `build` 与 `npx vitest run`（49 文件 / 445 条）全部通过。反证：将合并改回整体替换、将 `pendingBatchItems` 门控改回无条件重查，对应测试均转红，确认断言真实有效。
- [x] 8.3 更新界面术语和恢复说明：完整音乐库包含 `media/`+`echo/`，只保留音频无法推断喜欢/歌单；同步凭据需重新配置，不能把缺历史资料显示成恢复成功；验证文案不泄露底层 SQL 或凭据（LD-03/06/09）。
- [x] 8.4 执行 `pnpm --filter @echo/desktop format:check`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop test`、`pnpm --filter @echo/desktop build`，并验证 UI 消费全部新增类型且无未处理异步错误（LD-10）。

  验证：2026-10-24 上述五项与 Chromium e2e 实际通过，42 文件 / 385 条前端测试；术语、类型消费与未完成范围见 [前端验收证据](evidence/frontend-validation-20261024.md)。后续 IPC/UI 修改仍须重跑受影响检查，9.1/9.2 未因此完成。

## 9. 整体恢复、故障与交付验证

- [ ] 9.1 完成主机加两从综合 fixture：本地导入/扫描、喜欢取消、空/非空歌单、封面/覆盖、统计、alias、删除、离线候选与 LAN 传播混合，停写并稳定备份后移走各机全部应用数据，逐一断网恢复并比对规范快照，重复恢复不重复/不回声（LD-01…LD-09）。
- [x] 9.2 对复制/校验、文件/目录刷盘、commit marker、顺序链/材料化 checkpoint、SQLite commit、cursor、独立安全检查点、ACK、GC 逐项强制中断，覆盖本地和同步；验证未提交无普通成功、已提交可仅凭目录恢复、部分损坏报告，形成可重复证据（LD-01/02/06/07）。

  进度：2026-10-27 逐项矩阵与实测见
  [9.2 强制中断矩阵](evidence/9.2-interruption-matrix-20261027.md)。九个中断点各有具名用例
  （目录侧 21 个 `commit_with_hook` / 12 个 `recover_with_hook` phase，应用侧
  `Site::{Copy,Publish,Commit,State,TrashMove,RecordWrite}` × `Phase::{Before,After}`）。
  实测 exit 0：`cargo test -p echo-core --lib library_durability` 243 passed；
  `-- infrastructure::filesystem::durability::tests application::recover` 82 passed；
  `application::recover` 20 passed；`--lib sync` 215 passed；
  `cargo test -p echo-desktop --lib sync_security` 5 passed；全 lib 972 / 460 passed。
  未覆盖项已明确记录：全部为进程内注入，无真实断电；OS 级断电归 9.3，
  旧会话接纳中断归被阻塞的 7.7。
- [x] 9.3 macOS 原生临时库验证；按用户明确要求跳过 Windows/Linux 原生验证。验证重装式目录恢复边界、只读、注入式磁盘满、Unicode/大小写/符号链接与双进程锁；卷断开和真实 ENOSPC 未执行并列为未知项。不得对真实用户库执行清空数据库测试（LD-08/09/10）。

  证据： [macOS 临时库验证](evidence/9.3-macos-native-20261027.md)。记录 OS/build、架构、Rust/Cargo 版本、步骤、结果和未覆盖项；所有验证均运行在测试创建的临时目录/数据库中，没有触碰真实用户资料库。
- [x] 9.4 在小规模代表场景下验证扫描、受控批量导入及同步收敛的功能正确性和持久化顺序；允许较长的同步等待，不设大库吞吐或约 1 秒发现/取消性能门槛；记录可观察到的持久 IO 成本，不以延迟唯一权威落盘换性能（LD-01/10）。

  进度（2026-10-27）：三类场景均在小规模代表数据上实测通过，详见
  [9.4 小规模顺序与 IO 成本](evidence/9.4-small-scale-ordering-and-io-20261027.md)。
  **扫描**：3 首新歌 = 3 次 authority commit（每次 1 record + 1 resource），无变化重扫新增 0 次
  commit，身份稳定（`library_durability_small_scan_commits_one_record_and_resource_per_song`）。
  **受控批量导入**：2 首 = 4 次 commit（身份预留 + 歌曲 after-image），record 总数 2、resource 总数 2
  （`library_durability_small_import_records_reservation_and_song_commit_per_success`）。
  **同步收敛**：主机 + 单从真实 LAN fixture 41 passed / 0 failed / 2 ignored；先拉后推、ACK 前只声明
  一次资源、入站对象先于媒体、已收敛周期只读一页提交均有计数 port 实测断言。
  实测命令（串行，exit 0）：`cargo test -p echo-core --lib application::scan::tests::durability` 6 passed；
  `application::import::logic::tests` 37 passed；`cargo test -p echo-app sync_provisioner` 41 passed；
  `--lib library_durability` 243 passed；`echo-core --lib` 972 passed / 1 ignored；`echo-desktop --lib`
  460 passed。`echo-app` 是纯 binary crate，`--lib` 会报 `no library targets found`，正确形式是不带 `--lib`。
  两条 `#[ignore]` 是既有 512+ 规模用例（`513 real host publications exceed the governance time budget` /
  `real LAN host rate limit makes 513 uploads take several minutes`），按任务口径不设大库吞吐门槛，
  不计入通过、归后续独立验收。未覆盖项已记录：无三平台原生语义（归 9.3）、无主机 + 两从（归 9.1）。
- [x] 9.5 执行与本 change 已完成范围相称的短 CI 验证：`cargo fmt --all --check`、受影响 crate 的 Clippy/测试、前端相关检查、`pnpm verify:governance` 和小规模代表性集成场景；确认每条测试确有执行且证据与 LD/入口规格对应。完整 workspace、原样 513 和三平台原生验证仍作为整体交付或后续独立验收（LD-10）。

  进度：2026-10-25 夜间复核（口径更正：此前的 `178/0/1`、`58/0/1`、`865/0/2` 把 `#[ignore]` 的缺陷复现用例计入了 passed，也未区分 lib 与 arch 目标）：`cargo fmt --all --check` exit 0；`cargo test -p echo-core --lib` 865 passed / 1 failed / 1 ignored（共 867，1 failed 即缺陷 1 的 RED 用例、1 ignored 是既有性能基准）；`cargo test -p echo-core library_durability` lib 176 passed / 1 failed（690 filtered out）+ arch 1 passed；`cargo test -p echo-core scan` 52/0/0、`playlist` 54 passed / 1 failed、`import` 55/0/0（+ arch 1）；`cargo test -p echo-desktop --lib` 443/0/0、`cargo test -p echo-desktop favorite` 4/0/0；`node scripts/verify/check-scale.mjs` exit 0；`openspec validate enforce-library-first-durability --strict` exit 0。**`pnpm verify:governance` 已跑完并失败**（exit 101）：`cargo test -p echo-sync --test ledger_protocol` 的 `sqlite_host_commit_digest_matches_the_wire_protocol` 与 `maximum_size_mutation_remains_transportable_after_confirmation` 双双失败，报 `Unavailable { resource: "library durability", hint: "directory commit adapter is not configured" }`。该依赖由本 change 引入（`sync/durability/commit.rs` 的 `commit_prepared_projection` 在 `cfg(not(test))` 下要求目录提交适配器，而 echo-core 作为 echo-sync 的依赖时 `cfg(test)` 不成立），`crates/echo-sync/` 未被本分支改动 ⇒ 属本分支引入的回归，9.5 / 11.2 因此继续未勾选。（核对 `echo-sync` 整包时须带 `--all-features`：不带则 `discovery_network` 目标因 `mdns_sd` 特性未开而编译失败，那是特性开关问题、不是回归；全特性下该目标 3 条全 ignored。）完整 workspace all-targets 与三平台原生验证未做。故不勾选。
  更新（2026-10-25）：上述缺陷 4 已按方向二修复（测试安装真实控制面，`ledger_protocol` 回到 2/2，见
  [缺陷 4 收尾](evidence/defect4-rescan-closeout-20261025.md)）；`pnpm verify:governance` 整门复跑
  **exit 0**（注入套件 33/33，明细与首跑两处根因见 10.5 注记与
  [handoff-20261025.md](evidence/handoff-20261025.md) 第 11 点）。9.5 仍未勾选：剩余差量是
  干净工作区复现（须待用户授权提交后）与前端相关检查的逐项对应复核。
  更新（2026-10-26 中期复核修复）：03ba11f 提交前未复跑治理门，门在 arch、coverage、注入证明（前端
  lint 正向控制）三段红。已修复：① `projection_rebuild.rs` 的 `#[cfg(unix)]` 平台分支违反 arch 守卫，下沉为
  `infrastructure/filesystem/private_dir.rs` 的配对 cfg 原语（`DirBuilder::mode(0o700)` 创建即属主受限，
  含单测）；② coverage 89.76%（03ba11f 新增 974 行中 184 行未覆盖；父提交 48d72f1 实测 90.01% 与 11.2 记录
  一致）——补 `restore_relinks_present_media_onto_projected_records`、
  `restore_rejects_media_without_a_projected_record` 与
  `library_projection_port_defaults_reject_unsupported_operations` 三个测试后回到 90.03%；其中第一个
  测试暴露并修复真缺陷：restore 的 Keep/Relink 复活只改内存快照、从未调用
  `set_availability` 专用变更（`upsert_song` 机制性不写 availability），复活的歌在投影中永远
  Missing——已按 scan 路径同款规则补齐；③ `SettingsView.tsx` 跨 feature 深导入
  `../library/ScanReportDialog` 违反 `no-restricted-imports`，改为 `library/index.ts` 公共入口导出；
  ④ 新原语的 `pub(crate)` 触发 clippy `redundant_pub_crate`，改 `pub`（模块本身 `pub(crate)`，可见性等效）。
  acceptance-matrix 同步修复 7 个失效锚点（LD-04/LD-05/LD-09 对齐真实 spec 场景，rescue/backfill 行按
  spec 非目标移除）。最终 `pnpm verify:governance` 单独复跑 **exit 0**（coverage 90.03%，注入套件
  33/33，`openspec validate --strict` exit 0）。9.5/11.2 仍不勾：干净工作区复现待用户授权提交后执行，
  三平台原生验证未做。

  更新（2026-10-27，勾选）：完整短 CI 已执行并交付
  [evidence/9.5-short-ci-20261027.md](evidence/9.5-short-ci-20261027.md)。实测（全部串行）：
  `cargo fmt --all --check` 0；四个 crate Clippy `--all-features -D warnings` 各 0；
  `cargo test -p echo-core` lib 972/0/1 + 集成 38/0/0；`-p echo-desktop` lib 460/0/0 + 集成 12/0/0；
  `-p echo-app` 133/0/2 + arch 2/0/0；`-p echo-sync --all-features` lib 65/0/0、arch 1、ledger_protocol 2、
  discovery_network 0/0/3（3 条 `#[ignore]` 真实 mDNS 用例）；前端 format/lint/typecheck/build 各 0，
  vitest 49 文件 445 用例全过；`generate:ipc` 幂等（sha1 `33884fce…` 不变）；
  `pnpm verify:governance` **整门独占复跑 exit 0**，15 项检查全绿，注入套件 **33/33**。
  首跑曾 `GOV_EXIT=1`：注入阳性对照内 `host_resources.rs:237` 的 5 秒墙钟断言在机器有其他负载时失败，
  同树独占复跑不可复现，判定为负载敏感而非回归（归因边界见该证据第三节，未把元凶钉死到单一并行命令）。
  仍**不**声称：干净工作区复现（11.2，待用户授权提交后）与三平台原生验证（9.3）。
  另记一项既存事实：`openspec validate --all --strict` 为 exit 1，唯一失败项是
  `spec/diagnostic-logging` 的 Purpose 占位符（`origin/main` 既有，非本 change 引入），
  本 change 自身 `✓`；须在 10.5 归档前处理。
- [x] 9.6 使用 CodeGraph 与引用搜索复核无 DB-first/DB-only 生产替代入口、旧 materializer/fake/导出已清理，独立审查事务/迁移/身份/API/权限边界，修正后仅重跑受影响验证；交付审查与清理证据（LD-10）。

  证据：`evidence/9.6-db-first-residue-review-20261027.md`。四个边界（事务/迁移/身份/API 权限）独立复核完成；
  生产可达的 DB-first 用户事实写入仅剩 `resolve_host_sync_conflict`（`sync/sync_conflicts.rs:139-189`），
  已连同所需范围扩张一并上报（见该文件第六节），未擅自改实现。复核用的 Clippy 四条均 exit 0。
- [x] 9.7 汇总 LD-01…LD-10 的自动化证据，逐条对应 acceptance-matrix 的“Required acceptance evidence”，确认无“规划命令当通过”的伪证据；单独标明完整 workspace 与三平台原生验证属于整体发布或后续独立验收，不作为本 change 短 CI 的门槛；交付证据索引（LD-10）。

  证据：`evidence/9.7-ld-evidence-index-20261027.md`。28 条 acceptance-matrix 行全部绑定到 `rg` 可命中的
  真实用例名与 `file:line`（87 条 Rust + 9 条前端，零 MISSING）；状态如实为 完整 20 / 部分 7 / 无 1
  （LD-09c 确为无，仓库内不存在 media-only 恢复测试，仅有未被断言的界面文案）；并单列 7 条伪证据风险。

## 10. 场景登记、三方对账与归档门禁

> 本组是交付前置条件，不是收尾工作。delta 同步会向主规格新增 requirement 与 scenario；不登记就无法通过 `pnpm verify:governance`，而现状的“440 = 440 = 440”会被打破。

- [x] 10.1 确认并记录编号纪律：新增 requirement 一律追加到目标主规格末尾，新增 scenario 一律追加到所属 requirement 末尾。本 change 的 `portable-library-layout` 已按此修正（原先插入会使 `PLL-R07-S05` 指向新场景）；同步时用 `node scripts/verify/reconcile-scenarios.mjs` 验证既有稳定 ID 无位移（LD-10）。

  进度：2026-10-25：全量复核发现任务描述的"已按此修正"与实际不符——delta
  `portable-library-layout` 的 `对象资料接续与对账` requirement 仍是插入式排列（4 个新场景
  排在既有场景之前，同步会使既有 `PLL-R07-S01 打开资料库先接续再扫描` 位移到 S07）。已重排：
  既有 5 场景保持主规格原序在前，新增 `打开新协议资料库后投影再扫描`、`读取支持的旧格式`、
  `重复投影结果稳定`、`未解析引用保持原样` 依次追加为 S06–S09。逐 area 复核全部 delta
  （脚本比对 MODIFIED requirement 的既有场景须为 delta 场景列表的有序前缀）：其余
  MODIFIED（`local-library::单一资料库根目录与本地持久化`、`portable-library-layout` 其余
  两条、`safe-file-ingestion::暂存、校验、原子移动与操作日志`）均为纯追加或不变；各 delta
  无 REMOVED/RENAMED requirement，ADDED requirement 均只会追加到目标主规格末尾，既有
  requirement 编号不位移。`openspec validate enforce-library-first-durability --strict` 通过。
  最终验证（2026-10-25，随 10.5 同步完成后）：对 7 个修订区域逐 requirement 比对同步前后
  （`git show HEAD:` 基线）的 scenario 标题有序子序列——既有 requirement/场景全部保留且
  相对顺序不变，新增场景均为追加（LL +6、LAN +7、LE +4、PM +4、PLL +7、SFI +3、SYN +7，
  合计 +38 含 LD 先行的 23）；`reconcile-scenarios.mjs` spec 515 = trace 515 = manifest 515
  exit 0，既有派生 ID 无位移。
- [x] 10.2 为 `library-durability` 注册新 area 前缀并加入 `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX`，在 `docs/traceability.md` 新增对应段；验证 `node scripts/verify/reconcile-scenarios.mjs` 的 spec/trace/manifest 三方计数一致（LD-10）。

  进度：2026-10-25：AREA_PREFIX 已注册 `"library-durability": "LD"`（按字母序插入
  `immersive-lyrics` 与 `library-experience` 之间）；`docs/traceability.md` 已新增
  `## library-durability` 段（10 requirement / 23 scenario 全行，任务列按本 change 任务号
  映射，测试层暂记 Gate，manifest 路径为 10.3 待生成的规划路径）。**不勾选原因**：主规格
  目录 `openspec/specs/library-durability/` 尚未建立（10.5 同步），LD 段当前不参与
  reconcile 三方计数——注册后验证均绿但属惰性通过：`reconcile-scenarios.mjs` 461 = 461 =
  461 exit 0、`prd-matrix.mjs` 14 条路径与 461 场景一致 exit 0、
  `validate-scenario-commands.mjs` exit 0、`check-scenario-churn.mjs` 461 场景 287 命令
  1.61x exit 0。LD 场景计入三方计数并 >0 的复验留在 10.3（命令登记）与 10.5（同步）之后。
  2026-10-25 下午更新：主规格 `openspec/specs/library-durability/spec.md` 已随 10.3 建立，
  LD 23 场景正式计入三方计数——`reconcile-scenarios.mjs` **484 = 484 = 484** exit 0，
  traceability LD 段测试层列已按实际登记更新（22 行 Core、LD-R10-S01 治理门 Gate），
  `prd-matrix.mjs` 校验 14 路径 × 484 场景一致（其 `--write` 已移除，PRD 手工维护）。
  10.2 自身的勾选仍待 10.5 对其余 7 个修订区域同步后的既有 ID 无位移复验。
  最终验证（2026-10-25，随 10.5 同步完成后）：`reconcile-scenarios.mjs` spec 515 = trace
  515 = manifest 515 exit 0（LD 23 + 其余 7 区域新增 31 全部计入三方计数）；既有 ID 无位移
  复验见 10.1 最终验证段。10.2 完成。
- [x] 10.3 按修订后的 delta 实际场景清单在 `scripts/verify/scenario-commands.mjs` 登记可执行命令，运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 生成 `manifest.json.scenarios[]` 与 `tests/scenarios/*.yaml`；验证 `node scripts/verify/validate-scenario-commands.mjs` 无“matches 0 tests”，不得使用已移除的旧库回填场景或过滤后零测试命令占位（LD-10）。

  验证：2026-10-25：主规格 `openspec/specs/library-durability/spec.md` 先行建立（10.5 对 LD 新区域的
  同步动作提前完成——gen 的孤儿命令校验要求 spec 侧先有 LD 场景；delta 转主规格格式仅改标题行与
  `## Requirements`，10 requirement / 23 scenario 与 delta 逐字一致）。`scenario-commands.mjs` 登记
  LD 23 键：22 条各指向一条语义贴合的 `cargo test -p echo-core --all-features <name>` 测试
  （本分支 library_durability 测试族），LD-R10-S01（可执行验证）复用治理门入口
  `node scripts/verify/ci-governance.mjs`（PHA-R01-S02 先例，不内联 verify:scenario 防自递归），
  无零测试占位。`gen-scenario-manifests.mjs` dry-run 484 场景（46 新路径）后 `--write` 落盘
  （manifest.scenarios 484、381 yaml、103 native md，manifest 纯追加 138 行）；
  `validate-scenario-commands.mjs` exit 0（所有命令匹配真实测试/文件）；
  `reconcile-scenarios.mjs` spec 484 = trace 484 = manifest 484 exit 0。冒烟：
  `node scripts/verify/run-scenario.mjs LD-R01-S01` ok、`LD-R09-S03` exit 0。
  **既有问题注记（非本分支引入，全部区域一致）**：traceability 记录的
  `pnpm verify:scenario -- <id>` 形式因 `run-scenario.mjs` 把 `--` 当未知 id 而恒 exit 1
  （场景本身执行并 ok）；复现/验证请用 `node scripts/verify/run-scenario.mjs <id>` 直调形式。
- [x] 10.4 重跑 `node scripts/verify/check-scenario-churn.mjs`，确认重复度与单命令簇未超出冻结上限；若新增场景使比例上升，按模块拆分为独立命令而不是抬高上限（LD-10）。

  验证：2026-10-25（随 10.3 同批）：`check-scenario-churn.mjs` exit 0——484 场景 309 命令
  1.57x、最大簇 16，均在冻结上限内；LD 新增 22 条独立命令 + 1 条复用，重复度较此前
  461/287 = 1.61x 反而下降，无需拆分。
- [ ] 10.5 同步 delta 到主规格并复核既有场景未丢（逐 requirement 对比 scenario 标题集合），执行 `openspec validate --all --strict` 与 `pnpm verify:governance`；确认 tasks 与必要原生证据全部完成后归档本 change。PR 必须在同步及归档之后创建，规划阶段不勾选任务、不归档、不声称修复已生效（LD-10）。

  进度（2026-10-25 晚）：**delta→主规格同步已全部完成**（LD 新区域已随 10.3 先行）。各区域
  合并方式：`local-library` R01 整块替换 + 2 个 ADDED requirement 追加（11 req/40 scen）；
  `lan-library-sync` 3 个 ADDED 追加（planning-only，无 manifest ID）；`library-experience`、
  `playlist-management` 各 1 个 ADDED 追加；`portable-library-layout` 3 个 MODIFIED 整块替换
  （28→35 scen，从后向前替换避免行号错位）；`safe-file-ingestion` 1 个 MODIFIED 整块替换
  （5→7 scen）+ 1 个 ADDED；`sync-foundation` 3 个 ADDED 追加。**既有场景未丢复核**：逐
  requirement 比对 `git show HEAD:` 基线与当前的 scenario 标题有序子序列，7 区域既有
  requirement/场景全部保留且顺序不变、新增均为追加（详见 10.1 最终验证段）。
  **新场景登记（31 个新 ID：LL-R10/R11 6、LE-R21 4、PM-R13 4、PLL 7、SFI 3、SYN 7）**：
  `scenario-commands.mjs` 逐键登记语义贴合命令（24 条独立 echo-core 测试、2 条 echo-desktop、
  1 条 echo-app、复用 4 条既有命令承载同行为双视角、SFI-R12-S01 治理门承载同 LD-R10-S01
  先例），`validate-scenario-commands.mjs` exit 0；`docs/traceability.md` 追加 31 行（测试层
  Core×30 + Gate×1，任务列映射本 change 任务号）；`docs/acceptance/PRD-matrix.md`（手工表）
  补 61 行归属 + 11 个计数（PLL/SYN 在 A_FAMILIES 范围外，如 LD 不入 PRD）；
  `gen-scenario-manifests.mjs --write` 生成 515 场景（412 yaml + 103 native，新增 31 yaml）。
  **对账与门禁**：`reconcile-scenarios.mjs` 515 = 515 = 515 exit 0；`check-scenario-churn.mjs`
  515 场景 332 命令 1.55x、最大簇 16 exit 0；`prd-matrix.mjs` 14 路径 × 515 场景一致
  （1172 归属边）exit 0；`openspec validate --specs` 21 passed；`openspec validate --all
  --strict` 22/23——唯一失败 `diagnostic-logging` 为 Purpose 占位符 WARNING（`--strict`
  放大为 error），issue #38 遗留、本 change 未触碰该区域。`pnpm verify:governance` 整门
  复跑 **exit 0**（2026-10-25，`node scripts/verify/ci-governance.mjs`，日志 1896 行零 FAIL）：
  主段 35 个测试套件全绿（含 `cargo test --workspace --all-features` 的 echo-app 132 + 2 ignored、
  echo-core 875 + 1 ignored）、churn 段按 515 场景复核 1.55x、注入套件 **33/33** 门在违规下
  如宣称失败。首跑（/tmp/governance-515.log）曾 exit 1，两个根因与处置见
  [handoff-20261025.md](evidence/handoff-20261025.md) 第 11 点：门 15.1 正则不识别
  typed 墓碑布局（确定性，已修 `task-15.1.mjs`）；门 1.5 的 workspace 101 为瞬态争用
  （首跑与本地并行 cargo 命令共用 target，静止复现 exit 0）。**不勾选原因**：任务尾部的归档
  步骤要求 tasks 与必要原生证据全部完成（4.2b/5.1 等实施任务未完成），同步部分完成后
  不归档、不提前勾选。9.5/11.2 的干净工作区复现仍待用户授权提交后执行。

## 11. 分支与交付流程

- [ ] 11.1 本 change 的全部实现与产物改动必须在 `codex/issue-72-library-first-durability` 分支上完成，通过关联 issue #72 的 PR 合入 `main`；禁止直接向 `main` 提交（AGENTS 约束）。验证：`git log --oneline main..HEAD` 只包含本 issue 的提交，且 `git status` 在 `main` 上干净。
- [ ] 11.2 任何阶段提交前 `main` 必须保持可编译且门禁可跑：`cargo check -p echo-core`、`cargo check -p echo-desktop`、`cargo fmt --all --check` 与 `pnpm verify:governance` 不得因本 change 的开发中状态而红；验证时用干净工作区复现一次（LD-10）。

  进度：2026-10-25 夜间复核：本项曾被本分支引入的回归直接违反——`pnpm verify:governance`
  **exit 101**（`FAIL governance: architecture and workspace tests exited 101`），失败点为
  `cargo test -p echo-sync --test ledger_protocol` 的两条用例。缺陷 4 修复后的整门复跑
  （后台任务，2026-10-25 10:10–11:07）**仍是 exit 101，但失败点已转移**：
  `cargo test --workspace` 阶段 `echo-app` 的
  `sync_provisioner::tests::merge_cycle::native_pull_drains_more_than_one_commit_page_without_losing_local_work`
  运行超 60 秒被 SIGTERM——即 [native 513 证据](evidence/native-513.md) 记录的下载侧 513
  首歌用例超时（既有性能边界，非缺陷 4 回归；ledger_protocol 已不再失败）。当日用户决策：
  **513 大数据量用例退出治理路径，功能验证先用少量数据通过，速度问题后续优化**——该用例按
  上传侧同款标 `#[ignore = "513 real host publications exceed the governance time budget;
  re-enable after the rate optimization"]`（保留代码便于优化后恢复）。少量数据验证：
  `cargo test -p echo-app --bin Echo` 132 passed / 0 failed / 2 ignored（513 上传+下载两条
  均忽略）。含全部本轮改动的整门第二次复跑已启动，结果落笔时未出；**复跑 exit 0 之前本项
  与 9.5 不得按通过引用**（且 11.2 的最终验证须在干净工作区复现）。

  补记：第二次整门复跑（后台任务 bqyjobr4a）前 9 步全绿后仍 **exit 1**，失败点再次转移：
  coverage gate（task-12.8）`cargo llvm-cov -p echo-core --all-features
  --ignore-filename-regex application/testing --fail-under-lines 90` 报 89.69% < 90%
  （3323/32219 行未覆盖）。定责：CI（ci.yml:305 `pnpm verify:governance -- --without-injection`
  含 llvm-cov）在 main 为绿 ⇒ 覆盖率回归为本分支引入；整个
  `durability_projection/sync_state` 模块为本分支新增（`git diff main...HEAD --stat`），
  其中 `sync_state/related.rs` 312 行中 146 行未测（`song_alias`/`member_alias` 两函数
  整体零测试）为最大稀释源（分支净缺口约 102 行；`ports/system.rs`、`recover/mod.rs`
  的低覆盖在 main 基线同样存在，不归本分支）。修复：`sync_state/tests/boundaries.rs` 新增
  两条针对性测试——`..._restores_song_alias_after_canonical_record_arrives`（全真链路：
  source 上 stage/mark_published 两首同哈希歌，真实 exporter 产出 commit 事实与
  song-alias 事实；第一幕 canonical record 未投影时保持 MissingSong 不伪造 owner，第二幕
  record 到位后同一保留事实重试成功写入 `sync_song_aliases`）与
  `..._rejects_member_alias_without_anchored_alias_proof`（sequence 无对应 commit →
  MissingSyncDependency；sequence 指向真 commit 但 canonical JSON 无 playlist_member
  alias 证明 → InvalidStateFact；表零写入）。**更正（2026-10-25 午间）**：此前此处记
  「修复后 89.93% ≥ 90%，门过」为误判——两次复测（新增测试后的树、与重构后最终树）
  的后台任务通知均报 exit 0，但那是 `命令; echo "EXIT: $?"` 包装里 `echo` 自身的退出码，
  任务输出文件内的真实标记分别为 `EXIT: 1`（89.93%）与 `COV EXIT: 1`（89.94%，
  3242/32219 行未覆盖）——**门自始未过**，89.9x < 90 本就不可能通过
  `--fail-under-lines 90`。最终树口径缺口：covered 28977 行，过线需 ≥ 28998 行
  （**至少再补 21 行**）。`related.rs` 修复后仍有 71 行未测（74.55%，receipt/conflict
  两函数余下路径）为最大可补来源；继续补测后重跑，真实 exit 0 才可勾选
  （`cargo test -p echo-core --lib durability_projection` 34/0/0、clippy/fmt 绿已确认）。
  **最终结果（2026-10-25 下午）**：第三轮补测——`boundaries.rs` 新增 stray receipt
  fact（author/mutation 与锚定 commit 不匹配 → InvalidStateFact）与 conflict 行
  resolved 后幂等重试（命中旧行已 resolved 的早退分支，不覆盖既有 resolution）——
  后裸跑（无任何包装，任务退出码即门退出码）
  `cargo llvm-cov -p echo-core --all-features --ignore-filename-regex application/testing
  --fail-under-lines 90` **真实 exit 0：TOTAL 90.01%，missed 3219/32219，covered
  29000 ≥ 28998（余量 2 行）**；前置 `cargo test -p echo-core --lib --
  durability_projection::sync_state::tests::boundaries` 14/0/0、clippy
  `-D warnings` 与 fmt 均绿。**治理门第三次整门复跑（2026-10-25 下午，后台裸跑）真实
  exit 0**：`ok governance: CI core gate passed`，coverage 段 `ok 12.8`，
  injection-suite 33/33 正向控制全过；9.5 / 11.2 的勾选前还差最后一步——本条原要求的
  **干净工作区复现**（当前工作区含本分支未提交改动，干净复现须在提交后进行）。
  补记（2026-10-26）：03ba11f（10-26 02:30 提交）再次把门弄红且提交前未复跑——arch 段
  （sqlite 层 `#[cfg(unix)]` 平台分支）、coverage 段（89.76% < 90：新增 974 行中 184 行未测，
  父提交 48d72f1 worktree 实测 90.01% 与上文记录一致）、前端 lint（`SettingsView.tsx` 跨
  feature 深导入）三处。修复明细见 9.5 的 2026-10-26 更新（含 restore 复活不落投影的真缺陷）。
  修复后治理门第四次整门复跑（后台裸跑，无并行 cargo）**真实 exit 0**：
  `ok governance: CI core gate passed`，coverage 90.03%，injection-suite 33/33。
  **教训已两次验证：任何提交前必须整门复跑**——本分支已两次出现「上一次门绿 → 提交 →
  门红」（10-25 缺陷 4 轮与 10-26 03ba11f）。干净工作区复现仍待用户授权提交后执行，
  11.2 继续不勾选。
- [ ] 11.3 实现分支上的未完成代码不作为本 change 的现状或验收证据；设计与证据基线以 `main` 已发布实现为准。继续规划修订时，应记录分支 WIP 与规划证据的边界，不要求清理或暂存实现；交付审查须确认实现状态与本 change 规格相符后，才可完成对应任务（LD-10）。

## 12. 历史兼容代码删除（docs/legacy-compat-removal-plan.md）

- [ ] 12.1 按计划 §8 顺序（B→A→S→D）删除四档历史兼容代码，保留项遵守计划 §1 清单（flat v1 记录/墓碑读取、未来版本围栏、`PortableRecordDocument::V1/V2`、`StateNamespace::LegacyEvidence`、`replica_enrollment_replace`、`PROTOCOL_VERSION` 校验、`payload_changed_at_ms` 等全部不动）。**B 档**：删 `LEGACY_ROOT_SYNC_CONTEXTS_CHECKSUM`、`apply_migration_set` 的 v20 特例分派、`apply_root_sync_contexts`、`backfill_legacy_play_counts` 及关联测试（`legacy_connection`、两条 migration 用例、`legacy_play_counts_are_backfilled_once_to_the_stable_device`），确认 `durability_migration.rs::through_v27` 与 `sync_payloads.rs:232` 的完整迁移调用改走普通 `execute_batch` 后仍通过。**A 档**：删 `ControlPlanePort::apply_portable_upgrade`（端口默认实现、`control_plane.rs` 实现、`refusing.rs`/`ordered_fake.rs` 转发）、`prepare_locked_write` 的 `migration::recover`、`prepare_read` 的 journal 拒绝及测试循环里的 `"migrations/v1-v2-journal.json"`；`portable_migration.rs` 删 journal/backup/fence/plan/validate/recover/apply/replay 写入器并更新模块 doc（保留全部 IO 原语），`portable_migration/tests.rs` 删 7 个迁移测试与 `seed()`，按计划改写 `typed_tombstones_…`、`record_symlink_escape_…`、`future_format_…` 三个保留测试；同步修正本 change `evidence/9.7-ld-evidence-index-20261027.md` 与 `evidence/acceptance-matrix.md` 对已删测试的锚点。**S 档**：删 `replica_enrollment_archive` 读/辅助、`clear_legacy_replica_media_archive_intent`、`capture_legacy_plays`、`confirm_candidate` 包装（测试改用 `confirm_candidate_at`），收紧 `decode_play_stats` 只接受 `PlayStatsRecord`；新增已发布迁移 `0029` 执行 `ALTER TABLE sync_binding DROP COLUMN protocol_version`，移除 3 处生产写入与测试 INSERT 的该列，迁移须有升级、失败恢复与既有库兼容测试（CODE_STANDARDS §5/§9.1）；改写 `SYN-R07-S03` 场景与 `legacy_play_stats_payload_still_authorizes_the_buckets_it_recorded` 测试为新形状载荷 + 越权桶被拒，主规格与 delta spec 同步，并按计划 §6.3 更新 `scenario-commands.mjs`、`docs/traceability.md` 后重生成 manifest。**D 档**：删 `PersistedHistoryRecord::Legacy` 变体与 parse 分支、`local_state.rs` 的 `RawDoc.version` 字段（`parse_doc` 忽略该键），同步修正 `local_state/tests.rs:470,486` 断言；保留 `1..=SESSION_VERSION` 读取范围与 `old_session_without_priority_…` 测试。逐档运行 `cargo test -p echo-core --all-features`、`cargo test -p echo-desktop --all-features`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all --check`；收口执行 `node scripts/verify/gen-scenario-manifests.mjs --write`、`pnpm --dir apps/desktop test`、`node scripts/verify/reconcile-scenarios.mjs` 并以 `pnpm verify:governance` 整门独占复跑通过（LD-10）。

## 13. 交互编辑异步投影与低延迟确认（用户 2026-10-27 验收目标）

- [ ] 13.1 为活动资料库建立单写入者本地权威状态，能在 SQLite 滞后时为连续收藏/歌单操作分配完整同步 mutation：binding、author、sequence/predecessor、revision、HLC、base version、原始正文和资源引用。先列出并覆盖所有仍依赖 SQLite 读取的计划前置条件；以旧规划器对照测试验证同一输入和交错次序产生等价状态，含本地与 LAN 入站交错、重复名称、删除/墓碑、成员顺序和根切换（LD-01/02/05/07）。
- [ ] 13.2 将 SQLite 投影从交互请求中分离为有界、可退出和可重启的顺序 worker。业务行、已应用 operation ID/digest 和连续投影水位在单事务提交；水位前发生错误按退避重试，队列以目录链为持久来源，不能无限缓存、丢通知或覆盖较新的读己之写。SQLite 被写锁阻塞时，目录提交成功仍立即确认；测试重试、积压背压、退出/切库排空和 SQLite 完全重建（LD-01/02/03/10）。
- [ ] 13.3 从交互热路径移出不影响本次提交正确性的全链恢复/重复验证、业务文件材料化、checkpoint 与 GC。每一步必须有目录内持久状态、重启续跑和引用保护；提交标记发布后的错误保留同一操作身份且不得报未保存。用故障注入覆盖发布前、发布后/目录同步、材料化、checkpoint、GC 各边界，并保持外部篡改检测、未知格式围栏和损坏拒绝语义（LD-01/02/04/06/09）。
- [ ] 13.4 让 Tauri 收藏与歌单请求在目录提交确认后返回 UI 所需 DTO/稳定状态，不读取滞后 SQLite；前端列表、收藏按钮、连续操作和应用重开均支持读己之写与明确投影水位。覆盖投影失败、重复响应、快速连续点击、窗口关闭、切库及响应丢失，确保已保存用户意图不回滚且 UI 不显示陈旧结果（library-experience/local-library）。
- [ ] 13.5 在 macOS 实际桌面应用和真实临时音乐库上验收用户目标：1,000 与 10,000 首记录，对收藏、创建歌单、加入歌单、移出歌单每种操作至少 200 次，分别报告点击至已保存反馈、目录可靠提交、SQLite 投影延迟的 P50/P95/P99。四种操作的点击至已保存反馈 P95 < 50 ms；另用 MP3/FLAC/M4A/OGG/Opus/WAV 验证导入后无需重扫即可见，并单独报告单曲导入耗时，不把导入纳入 50 ms 门槛。同时运行 `cargo test -p echo-core`、`cargo test -p echo-desktop`、桌面前端测试及 `pnpm verify:governance`。报告硬件/文件系统和所有未覆盖故障，不得以 Python 原型替代实际应用证据。

  验证进度（2026-10-28）：Release 服务路径在 3,000 首合成记录上对四种操作各测 100 次，P95 为 28.09/34.04/31.81/32.49 ms；10,000 首、每项 30 次的严格 P95 上限基准也通过，P95 为 37.21/49.35/37.87/37.89 ms。导入 6 种真实短音频在 3,000 首样本为 424–454 ms，另一个 1,000 首跟踪样本为 122–140 ms；导入没有 P95 分布，需在 13.5 中单独验证。数据和范围见 [交互编辑延迟验证](../../../docs/verification/interactive-edit-latency-2026-10-28.md)。这不是实际 Tauri 窗口点击计时：1k/10k 各操作 200 次、真实桌面点击至反馈、SQLite 投影延迟分布及整门 `pnpm verify:governance` 尚未验证，因此本项保持未勾选。

## 14. 同步写入捕获成本（用户 2026-10-29 继续优化）

- [x] 14.1 为非墓碑 Favorite-only 入站页实现受限定向捕获：要求认证 replay 的 peer anchor、SQLite 等头水位和父 Song after-image证明，遇到页内 alias/资源、其他对象、相关 alias/墓碑、匹配 mutation或不完整状态时完整捕获；共用完整导出的事实编码，保存 Favorite、ledger、confirmed、receipt 和 peer cursor。用 `cargo test -p echo-core --lib focused_favorite_page` 比较定向/完整目录的规范用户资料和必要同步事实重建、重复重放及关键 fallback，另运行 `cargo check -p echo-core`、修改 Rust 文件的 rustfmt 与 `openspec validate enforce-library-first-durability --strict`（LD-01/02/03/07/10）。批次大小下降不等于延迟验收，Release/桌面同步分段测量另行记录。
