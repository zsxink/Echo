# Tasks

以下记录本 change 的实施进度；仅在指定行为和验证均完成后勾选。测试名称前缀在对应任务中新增，命令从仓库根目录执行；不能用过滤后 0 个测试冒充通过。LD 编号对应 `specs/library-durability/spec.md`，入口规格与 design D1–D8 共同约束范围。

## 1. 固化契约与可失败测试基线

- [x] 1.1 按 design 证据重新用 CodeGraph 核对所有本地/同步写入口，交付“入口→资料库提交→投影→成功”清单，覆盖 CRUD、成员、喜欢、统计、封面、写回、删除/撤销、快照、增量、ACK 和主确认；验证每个入口有归属层与当前失败窗口（LD-01/10）。
- [x] 1.2 新增合成 issue #72 fixture（15 songs、2 portable songs、11 favorites、3 members、无 portable playlist），分别提供可证明 DB/alias 与不可证明孤立引用版本；验证 fixture 无真实用户路径、无真实资源、能稳定复现旧恢复丢关系（LD-04/05/06）。
- [x] 1.3 新增统一规范用户状态比较器及 `library_durability` 测试组，比较 UUID、路径/哈希/added_at、喜欢 false/true、空歌单、成员 UUID/顺序/时间、资源、覆盖、设备桶、typed tombstone/alias、pending；以 `cargo test -p echo-core library_durability` 验证失败定位，不能只比计数（LD-03）。
- [x] 1.4 更新 `docs/DESIGN.md`、`docs/PRODUCT.md`、`docs/ROADMAP.md` 与 `openspec/CODE_STANDARDS.md` §5 的 source-of-truth、完整目录备份和重装恢复原则，核对旧空目录加入描述与现行 LAN 本地合并一致（`CODE_STANDARDS.md` 现仍写“首次加入只接受空目录”）；交付文档 diff 与规格对照，不能把规划写成已验收（LD-09/10）。
- [x] 1.5 以 `main` 已发布实现为唯一现状基线复核 design 的证据表：修正不存在或已移动的路径（`infrastructure/filesystem/migration` → `portable_migration.rs`；`sync_provisioner/remote_resources.rs` → `resource_publication.rs` / `resource_candidates.rs`），并声明行号只作导航；验证每条证据都能在干净检出上打开（LD-10）。

## 2. 领域提交模型与端口

- [ ] 2.1 新增 typed change set、来源、预期版本、完整 after-image、资源声明、操作身份、本库提交序列/前驱摘要及结果状态模型；验证不合法跨库/跨类型/重复目标/不完整依赖被拒绝，Domain 无 FS/SQLite/网络依赖（LD-01/02/10）。
- [ ] 2.2 增加聚焦的目录提交/读取/恢复与投影端口，设计可失败 fake，并明确旧 SQLite UoW 只投影；删除 `ControlPlanePort` 上指向同一实现的转发方法（`durability()`、`commit_library_batch`、`recover_library_batches`、`library_operation_status`、`write_library_asset`、`read_library_asset`），调用方统一经 `durability()` 取 `LibraryDurabilityPort`；验证 `ControlPlanePort` 方法数不高于既有基线且规模白名单不因本 change 扩大（LD-10）。
- [ ] 2.3 扩展 portable 用户资料字段，包含歌单创建/成员加入时间、资源原件引用及必要关系；分别定义 confirmed 完整 payload、pending/receipt、实际受管资源位置，避免新业务值继承旧确认收据；用 v1/v2/v3 JSON round-trip、缺字段与未知格式测试验证兼容（LD-03/07/08/09）。
- [x] 2.4 文档化目录协议版本、字段语义和 typed tombstone/alias 规则，使用 `cargo test -p echo-core library_durability` 核对序列化 fixture 与说明一致，确保缺历史字段保持 absent（LD-05/09/10）。

  验证：2026-10-24 新增 [目录协议字段说明](../../../docs/library-durability-protocol.md)，独立审查通过；主 Agent `library_durability --lib` 74/74、新增历史缺省字段回归 1/1、`portable_v --lib` 4/4、Core Clippy/fmt/scale/strict 通过。缺省时间/确认元数据保持省略，历史 Song 的已发布 0 回退单独说明；当前 v2 与目标 v3 升级边界明确，不替代 2.1/7.8/8.2。

- [x] 2.5 区分共享对象与目录私有 state：逐设备播放桶留在 `PlayStatsRecord`，幂等播放事件身份写入 `echo/state/playback-events/`；验证同步导出与跨设备载荷不含会话标识，`PortableRecord` round-trip 与既有载荷完整性测试通过（LD-03/07/08）。

  验证：2026-10-24 最终持久化组 60/60、实际 producer/host payload 测试 1/1 通过；见 [播放私有状态证据](evidence/playback-state-privacy.md)。生产 recorder 全入口验收仍属于 6.3。

## 3. 文件系统权威提交与恢复

- [ ] 3.1 实现资料库级跨进程排他写许可、格式/权限检查及安全 exclusive-create journal；读路径 MUST NOT 取写锁，也不得因锁文件缺失而失败，只读卷上已提交资料仍可浏览；验证同库两写者、切库/取消/退出、只读卷浏览、符号链接/越界/长路径、未知格式不能绕过锁或污染另一库（LD-01/09/10）。
- [x] 3.2 实现 prepared payload、资源声明、checksum 与本库 seq/前驱提交标记的刷盘/原子发布，用 fault fake 拦截每次文件/父目录 sync 和 rename；验证标记前无业务提交，结果不确定可查询同操作，标记后丢 SQLite 仍恢复（LD-01/02）。
- [x] 3.3 实现按本库顺序的 records/state 材料化、目录 checkpoint 与幂等重放，验证 C1 投影失败后 C2 更新同对象，重启/重复重放不回退 C2；断链/同 seq 不同摘要/损坏已提交数据拒绝写入并保留证据（LD-02/06）。

  验证：2026-10-24 修复重试 fence/coverage 的连续故障窗口并独立复查通过；主 Agent 最终持久化组 60/60，详见 [刷盘与 GC 复查](evidence/retry-fences.md)。此处是目录材料化顺序，Application 投影 4.2 与完整私有历史/资源 GC 3.5 仍未完成。
- [ ] 3.4 实现用户资源原件和实际受管版本位置持久化；写回前保留旧字节与替换意图，提交才切换引用，验证替换后标记前崩溃可恢复旧关联或完成相同操作，不丢已提交旧版本（LD-02/08）。
- [ ] 3.5 实现安全 checkpoint/压缩/GC，必须先刷盘完整覆盖清单再回收日志，引用图保护候选、快照、冲突、删除意图和未完成发布资源；checkpoint 不得随历史无界增长，链长度应与活跃窗口而非总历史成正比，超过安全上限时拒绝写入并报告；故障注入验证 GC 任一步中断后均可恢复且无唯一字节被删（LD-02/08）。
- [ ] 3.7 为扫描与批量导入引入有界批量提交（同一逻辑集合合并，而不是每首歌一次链提交），并消除单次编辑触发的多次全链扫描/全量摘要校验；以 10k 量级合成库测量提交次数与 IO，验证不因性能优化延后唯一权威落盘（LD-01/02/10）。
- [x] 3.6 增加平台 FS 适配说明与协议恢复文档（可放 `docs/lan-sync-recovery.md` 的本地资料库章节），运行 `cargo test -p echo-core library_durability`，记录 POSIX/Windows 目录同步/替换能力及不支持时的明确失败语义（LD-09/10）。

  验证：2026-10-24 `library_durability --lib` 52/52 通过；`docs/lan-sync-recovery.md` 的“文件系统适配与失败语义”核对了现有 POSIX/Windows 实现，明确 Windows/Linux 尚待原生验收。此项不替代 10.3。

## 4. SQLite 可重建投影

- [x] 4.1 新增顺序 migration 保存 applied operation、本库投影水位和必要 typed state，不改已发布迁移；验证旧库升级、重复打开、migration 回滚失败与错误库 ID，执行 `cargo test -p echo-core migration`（LD-03/04/10）。

  验证：2026-10-24 migration 20/20 通过，旧 v27 升级/失败回滚/重开保留证据及独立审查见 [SQLite 投影迁移证据](evidence/projection-migration.md)；完整历史救援与 v3 目录升级仍属于第 8 组。
- [ ] 4.2 实现 Application 提交协调器，投影 MUST 从 SQLite 已记录的投影水位出发按目录链顺序幂等追赶，per-operation 闭包只是触发点而不得各自直写业务表；普通成功发生于两阶段完成，DB 失败返回已保存待投影；用可控次序 fake 验证绝无用户 DB-first 写入，且 C1 投影失败、C2 成功后的重启/追赶使 C1 事实也进入投影（LD-01/02）。
- [ ] 4.3 实现从目录重建临时 SQLite、完整性比对后替换活动投影，隔离损坏/落后/领先或其他库数据库；投影 MUST 容忍悬空父引用（可解析子集入正表，未解析项入 typed `library_recovery_records` 并计 partial），不得因单条悬空引用使重建整体失败；验证不依赖旧数据库、重建无上传回声、失败保持旧活动库（LD-03/06/07）。
- [ ] 4.5 把 `library_recovery_records` 的键改为稳定 typed key（不得使用 `Debug` 格式），并提供读回路径使 confirmed/pending/悬空引用在重建时可被重新尝试；验证补齐依赖后重试自动关联，无需重新导入（LD-06/07）。
- [ ] 4.4 为投影边界及 SQLite 损坏/只读/commit 失败补集成测试并更新数据库恢复说明，运行 `cargo test -p echo-core library_durability` 和 `cargo test -p echo-core continuation`（LD-01/02/03/10）。

## 5. 本地导入、扫描与监听接入

- [ ] 5.1 导入改为资源校验安全发布→目录提交→SQLite 投影，预留 UUID/added_at/claim 在目录可恢复，去除 DB commit 后拼 SongRecord；明确“预留 claim”与“提交 Song”两段操作身份的自识别与互引用键，重试不得重新分配 UUID，界面只报一个导入结果；以导入边界故障、失去 SQLite 后重建测试验证，运行 `cargo test -p echo-core import`（LD-01/02/03；safe-file-ingestion）。
- [ ] 5.2 改造扫描 `persist_parsed_index` 及刷新路径，新增/改名/标签变化先提交 Song；无变化不生候选，missing/卷卸载仅本机可用性；以扫描后 wipe/改名/标签/只读/重复事件测试验证，运行 `cargo test -p echo-core scan`（LD-01/03）。
- [ ] 5.3 打通导入、扫描和同步发布的 claim/UUID 复用，保留有界重处理并发；用双 worker 屏障、watcher 抢先、取消、同 hash/不同内容目标冲突测试验证不重复、不覆盖（LD-02/05）。
- [ ] 5.4 更新导入/重扫的恢复与逐项结果说明，运行 `cargo test -p echo-core library_durability` 加上述入口测试，记录歌词失败与已保存待投影如何分别返回，不用整体 toast 掩盖部分失败（LD-01/06/10）。

## 6. 喜欢、歌单、统计和资源编辑接入

- [ ] 6.1 Core 收藏/歌单/成员用例统一目录提交，桌面服务仅转发；补 true→false、空歌单、重命名、多目标重复成员、时间/顺序、级联删除的 wipe round-trip 测试，运行 `cargo test -p echo-core playlist`、`cargo test -p echo-desktop favorite`（LD-01/02/03）。
- [ ] 6.2 歌单封面和既有覆盖资源迁入内容寻址原件，读模型 cache key 派生；验证清空全部应用缓存后离线恢复、丢原缓存的历史缺口报告和相同资源复用，运行 `cargo test -p echo-desktop cover` 及 `cargo test -p echo-core library_durability`（LD-06/08）。
- [ ] 6.3 播放完成统计以目录稳定事件先提交设备桶再投影，失败不打断播放但可追踪重试；幂等事件身份存 `echo/state/playback-events/`，共享 `PlayStatsRecord` 只保留设备桶，验证重复 session/重放/ACK/重装不增计、同步导出不含会话标识、资源不可写不留 DB-only 增量，运行 `cargo test -p echo-desktop recorder` 和逐设备统计测试（LD-01/02/03/07/08）。
- [ ] 6.4 删除/撤销迁入目录本机意图及最终集合，保留 10 秒唯一终态、物理回收独立恢复与候选引用保护；验证窗口内重启、终结/撤销竞争、远端墓碑、未确认导入后删除，运行 `cargo test -p echo-core delete`、`cargo test -p echo-core trash`（LD-02/07/08）。
- [ ] 6.5 现有写回/移动及覆盖字段通过同一提交协调器，旧资源先保留、用户显式写回才替换；验证标签写回前后崩溃、旧/新哈希和原 UUID、覆盖字段清理、缺少媒体不抛关系（LD-01/02/08）。
- [ ] 6.6 删除旧桌面 DB-first materializer 路径和无消费者导出/fake，保留只读 builder 或测试辅助的理由；点名清理 `SqliteDatabase::record_playback` 这一公开 DB-first 入口与无调用者的孤儿模块，用 CodeGraph 调用链及 `rg -n 'write_record_guarded|materialize_playlist|record_play_stats|record_playback' crates apps/desktop/src-tauri` 复核所有剩余生产写入口均受协调器控制，运行 `cargo test -p echo-desktop --lib` 和架构测试（LD-10）。
- [ ] 6.7 同步更新各入口持久化/错误说明，记录与现有歌单 UI change 的封面字段交汇，验证接口不存在平台绕过 Core 的用户变更，并运行本组 round-trip 集成测试（LD-03/08/10）。

## 7. LAN 首次快照、增量、ACK 与主确认接入

- [ ] 7.1 改造远端快照/增量与来源 ACK 为 Core 目录提交用例，先校验全快照再发布可见集合；保留本地资料/候选，SQLite 对象与已应用水位在目录提交后同事务投影；验证快照分页失败/取消、重复 page、metadata-only 喜欢/空歌单、所有提交点崩溃（LD-01/02/03；LAN spec）。
- [ ] 7.2 持久化真实 confirmed payload、receipt、alias/tombstone 和 pending/前驱，修复旧 sync metadata 附到新值的误确认语义；验证离线 A→B 多候选、迟到 ACK、已发送正文不变、重复成员 alias 与旧删除不影响新成员，运行 `cargo test -p echo-core sync`（LD-05/07）。
- [ ] 7.3 主机确认改为规划/序列预留→目录权威提交→SQLite 账本→资源/安全检查点→连续 published→ACK；同一“规划-提交-投影”序列在多个确认分支重复出现时必须抽成一个 helper，不得复制粘贴；验证重复 mutation 同结果、相同 ID 异正文拒绝、每个 publication 窗口断电、ACK 后丢 SQLite 可恢复业务资料，运行 `cargo test -p echo-sync`（LD-01/02/07）。
- [ ] 7.4 远端资源 publication 保存目录位置声明与完成凭据，对 `media/` 和受管 `echo/media/` 均验证 hash/length/owner/version；验证媒体先后、中途替换失败、Windows 文件占用、旧文件冲突、丢 DB 后关联原 UUID，运行 `cargo test -p echo-core remote_resources`（LD-02/08）。
- [ ] 7.5 把候选终结、冲突原件和资源引用纳入目录重放/压缩，禁止重建制造 outbox 回声；验证启停/换库/epoch 清理不丢引用，运行 `cargo test -p echo-core resource_retention` 及多设备契约（LD-02/07/08）。
- [ ] 7.6 更新 `docs/lan-sync-recovery.md` 的 ACK、媒体与已同步边界和重装安全恢复；验证无钥匙串可离线恢复但不自动联网，可信账本缺失阻止旧 epoch，网络协议如需新增资源字段则先补版本协商规格与契约，不能默丢字段（LD-03/07/09/10）。
- [ ] 7.7 实现可信原会话重放与新 binding/作者的显式意图接纳转换，先核对旧 receipt/主规范状态，再持久保存旧→新 mutation 映射，保留原冻结时间/UUID/成员顺序/统计桶；验证旧 ACK 丢失、身份改变、重新配对与转换任一步中断均不重复应用，无法判定只保留待处理；加入旧 A 桶本地 7/主 5、新作者 B 无权抬高 A 或挪差额的场景，旧计数本地保留且 B 只增自己的桶，运行 `cargo test -p echo-core library_durability` 和 `cargo test -p echo-core sync`（LD-07）。
- [ ] 7.8 建立本地 manifest、record envelope、LAN wire/capability 的独立新旧兼容矩阵；验证 v1/v2 新读旧、旧拒写新、混版本资源字段协商与拒绝、无静默丢字段，并将结果写入协议/恢复文档（LD-09/10）。

## 8. 历史迁移、身份修复与接续

- [ ] 8.1 在 continuation/scan 前锁定并捕获一致 SQLite 救援：原 DB 物理快照留应用私有区，目标库脱敏 typed 逻辑导出及资源原件进入目录；验证逻辑导出不含其他库/绝对路径/凭据且原 DB 消失仍可继续（LD-04/09）。
- [ ] 8.2 实现 v1/v2→新协议的持久升级围栏、journal、原件备份与版本检查；恢复既有 v1→v2 journal 后再升级，验证旧程序拒写、未知未来格式只读、重复迁移、各围栏/fsync 边界失败及整套备份回滚（LD-04/09）。
- [ ] 8.3 实现歌曲/喜欢/歌单定义/成员/资源/统计缺口回填，保留原 UUID、入库/创建/加入时间和历史 absent；验证空歌单、数据库独有资料、缺媒体、丢缓存封面、issue #72 两种合成 fixture（LD-03/04/06/08）。
- [ ] 8.4 实现证据驱动身份映射与全引用迁移，复用现有严格身份修复能力，区别本地映射与主确认 alias；验证同路径不同内容、同 hash 多历史身份/墓碑、不同 binding、已发送事件、孤立喜欢均不被静默合并或伪造（LD-05/07）。
- [ ] 8.5 接续改读 typed envelope 与目录 confirmed/pending/alias/墓碑/资源图，先建立全部身份再解析关系；验证原先 superseded/invalid 案例被正确修复或进入保留的 partial 报告，不在后续扫描中遗忘（LD-03/05/06/07/08）。
- [ ] 8.6 回填完成前用独立空 SQLite 重建并比对规范快照，输出发现/补齐/缺失/歧义/未关联/资源错误摘要，完整才刷完成标记，partial 维持只读；运行 `cargo test -p echo-core library_durability`、`cargo test -p echo-core continuation` 并更新恢复文档的“可救援/不可推断”实例（LD-04/06/09）。

## 9. 类型化 IPC 与用户可见恢复状态

- [ ] 9.1 Core→desktop→IPC 提供未保存、已保存待恢复、已保存待投影、已完成、部分恢复及稳定操作身份/分类原因；运行 `cargo run -p echo-desktop --bin echo-generate-ipc` 并核对生成物，序列化测试覆盖各态，不能手改生成类型（LD-01/06/10）。
- [x] 9.5 为每个新增错误码补齐前端 message key 映射（不得把 key 字符串直接当用户文案显示），并把新码显式列入 `ErrorPolicy` 重试判定表，以断言钉住；验证 `pnpm --filter @echo/desktop typecheck` 与 IPC 漂移检查通过（LD-01/06/10）。
- [ ] 9.2 在现有资料库设置/错误详情中呈现回填进度、只读、未关联记录、待投影与同操作重试；喜欢/歌单乐观状态按提交结果处理，统计错误不中断播放但可查看；通过 React 测试验证焦点、键盘、错误、重新打开与批量逐项反馈（LD-01/06）。
- [x] 9.3 更新界面术语和恢复说明：完整音乐库包含 `media/`+`echo/`，只保留音频无法推断喜欢/歌单；同步凭据需重新配置，不能把缺历史资料显示成恢复成功；验证文案不泄露底层 SQL 或凭据（LD-03/06/09）。
- [x] 9.4 执行 `pnpm --filter @echo/desktop format:check`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop test`、`pnpm --filter @echo/desktop build`，并验证 UI 消费全部新增类型且无未处理异步错误（LD-10）。

  验证：2026-10-24 上述五项与 Chromium e2e 实际通过，42 文件 / 385 条前端测试；术语、类型消费与未完成范围见 [前端验收证据](evidence/frontend-validation-20261024.md)。后续 IPC/UI 修改仍须重跑受影响检查，9.1/9.2 未因此完成。

## 10. 整体恢复、故障与交付验证

- [ ] 10.1 完成主机加两从综合 fixture：本地导入/扫描、喜欢取消、空/非空歌单、封面/覆盖、统计、alias、删除、离线候选与 LAN 传播混合，停写并稳定备份后移走各机全部应用数据，逐一断网恢复并比对规范快照，重复恢复不重复/不回声（LD-01…LD-09）。
- [ ] 10.2 对复制/校验、文件/目录刷盘、commit marker、顺序链/材料化 checkpoint、SQLite commit、cursor、独立安全检查点、ACK、GC 逐项强制中断，覆盖本地和同步；验证未提交无普通成功、已提交可仅凭目录恢复、部分损坏报告，形成可重复证据（LD-01/02/06/07）。
- [ ] 10.3 macOS/Windows/Linux 原生临时库验证重装式恢复、只读/磁盘满、Unicode/大小写/符号链接、卷断开、文件占用与双进程锁，记录安装版本/步骤/结果/未知项；不得对真实用户库执行清空数据库测试（LD-08/09/10）。
- [ ] 10.4 测量大库扫描/受控批量导入及同步收敛回归，保持既有无变化同步额外 CPU 与约 1 秒发现/取消约束，记录新增持久 IO 成本；验证不以延迟唯一权威落盘换性能（LD-01/10）。
- [ ] 10.5 最终执行 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`pnpm verify:governance`、`pnpm --filter @echo/desktop test:e2e` 与第 9 组前端命令，确认每条测试确有执行且证据与 LD/入口规格对应（LD-10）。
- [ ] 10.6 使用 CodeGraph 与引用搜索复核无 DB-first/DB-only 生产替代入口、旧 materializer/fake/导出已清理，独立审查事务/迁移/身份/API/权限边界，修正后仅重跑受影响验证；交付审查与清理证据（LD-10）。
- [ ] 10.7 汇总 LD-01…LD-10 的自动化与原生证据，逐条对应 acceptance-matrix 的“Required acceptance evidence”，确认无“规划命令当通过”的伪证据；交付证据索引（LD-10）。

## 11. 场景登记、三方对账与归档门禁

> 本组是交付前置条件，不是收尾工作。delta 同步会向主规格新增 requirement 与 scenario；不登记就无法通过 `pnpm verify:governance`，而现状的“440 = 440 = 440”会被打破。

- [ ] 11.1 确认并记录编号纪律：新增 requirement 一律追加到目标主规格末尾，新增 scenario 一律追加到所属 requirement 末尾。本 change 的 `portable-library-layout` 已按此修正（原先插入会使 `PLL-R07-S05` 指向新场景）；同步时用 `node scripts/verify/reconcile-scenarios.mjs` 验证既有稳定 ID 无位移（LD-10）。
- [ ] 11.2 为 `library-durability` 注册新 area 前缀并加入 `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX`，在 `docs/traceability.md` 新增对应段；验证 `node scripts/verify/reconcile-scenarios.mjs` 的 spec/trace/manifest 三方计数一致（LD-10）。
- [ ] 11.3 为全部新增场景（`library-durability` 24 条 + 已注册 area 新增 32 条；其中 `library-experience` 4 条为本 change 新增的收藏持久反馈）在 `scripts/verify/scenario-commands.mjs` 登记可执行命令，运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 生成 `manifest.json.scenarios[]` 与 `tests/scenarios/*.yaml`；验证 `node scripts/verify/validate-scenario-commands.mjs` 无“matches 0 tests”，不得用过滤后零测试的命令占位（LD-10）。
- [ ] 11.4 重跑 `node scripts/verify/check-scenario-churn.mjs`，确认重复度与单命令簇未超出冻结上限；若新增场景使比例上升，按模块拆分为独立命令而不是抬高上限（LD-10）。
- [ ] 11.5 同步 delta 到主规格并复核既有场景未丢（逐 requirement 对比 scenario 标题集合），执行 `openspec validate --all --strict` 与 `pnpm verify:governance`；确认 tasks 与必要原生证据全部完成后归档本 change。PR 必须在同步及归档之后创建，规划阶段不勾选任务、不归档、不声称修复已生效（LD-10）。

## 12. 分支与交付流程

- [ ] 12.1 本 change 的全部实现与产物改动必须在 `codex/issue-72-library-first-durability` 分支上完成，通过关联 issue #72 的 PR 合入 `main`；禁止直接向 `main` 提交（AGENTS 约束）。验证：`git log --oneline main..HEAD` 只包含本 issue 的提交，且 `git status` 在 `main` 上干净。
- [ ] 12.2 任何阶段提交前 `main` 必须保持可编译且门禁可跑：`cargo check -p echo-core`、`cargo check -p echo-desktop`、`cargo fmt --all --check` 与 `pnpm verify:governance` 不得因本 change 的开发中状态而红；验证时用干净工作区复现一次（LD-10）。
- [x] 12.3 若开发分支上已有未完成的实现，先把它整理成可编译状态（或暂存/移除）再继续规划修订，不得让半成品实现与“仅规划”的 change 产物同时存在，以免证据基准被污染（LD-10）。
