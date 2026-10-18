# Tasks

## 1. 范围、依赖与契约基线

- [x] 1.1 将 PRODUCT/DESIGN/CODE_STANDARDS 的二期连接器条款统一到局域网优先，记录单主、全库、确认序LWW、非空目录与恢复边界；验证文档与 `lan-library-sync` 所有 Requirement 一致，执行 `openspec validate introduce-lan-library-sync --strict`。
- [x] 1.2 盘点每种共享对象和资源的本地写入口、portable schema、outbox 与 journal 覆盖，包括播放统计/封面/侧车、成员追加顺序和删除/撤销；区分本机展示排序偏好与共享顺序，登记 PendingDelete 临时墓碑的迁移策略；输出证据清单与 fixture，验证“共享内容边界”“历史库启用同步”没有仅存于SQLite的遗漏。
- [x] 1.3 建立 echo-sync crate 和 client/server/discovery feature 边界，验证候选依赖 MSRV、许可证及三平台兼容；锁定依赖并执行 `cargo check --workspace --all-targets`、`cargo deny check`，记录具体版本与选型结果。RustSec 修复后 workspace MSRV 为 1.88。macOS/Windows/Linux Release workflow 均从提交 `6a1bbe9` 构建成功，应用版本为 0.3.0。
- [x] 1.4 固定 mutation/commit/snapshot/resource DTO、typed song/member alias、检查点/日志摘要规范编码、限制值、身份和协议版本，生成兼容fixture和协议文档；测试摘要跨端一致、未知可选字段、未知必需种类、超限载荷及不兼容拒写，执行 `cargo test -p echo-core`。

## 2. 领域裁决与状态机

- [x] 2.1 实现库/authority/epoch、typed object key、本地版本/主确认版本和稳定事件身份值对象；测试错库、跨种类UUID、序列比较和墙钟偏差，执行 `cargo test -p echo-core`，覆盖“对象种类区分”“旧客户端或错误主身份”。
- [x] 2.2 实现存活对象主确认序LWW、base冲突检测与候选保留、同设备前驱链和不同对象独立更新；测试A/B并发和连续离线编辑结果，覆盖“同对象并发修改”“本地连续编辑与回拉”，更新裁决文档并运行 Core 测试。
- [x] 2.3 实现typed最终墓碑、歌曲/歌单级联、陈旧更新拒绝与显式新UUID重新导入；定义10秒可撤销意图→撤销/逻辑终结状态机和唯一终态，终结后才生成稳定删除事件；测试1秒撤销、重启/到期/撤销竞争、远端墓碑与本机意图竞争及旧更新不得复活，覆盖“删除后旧设备重连”“对象种类区分”“撤销窗口内删除并撤销”“删除窗口内重启与终结竞争”“删除意图遇到远端最终删除”。领域状态机、持久化删除恢复、远端墓碑与重开后稳定事件身份测试通过。
- [x] 2.4 实现播放统计逐设备max/作者授权、成员追加确定次序、不重复歌曲约束与typed成员alias；规范化旧身份的编辑/删除，历史alias不重定向到重新添加的成员；测试同时收藏/改标签/计数、并发追加后立即删除、添加ACK前排队删除、重新添加后旧删除重试，覆盖“收藏标签统计互不覆盖”“重复成员去重后删除”“成员重新添加遇到旧删除”。
- [x] 2.5 实现并发同哈希导入 canonical UUID 与别名重映射、同路径不同内容安全命名；测试收藏/成员/覆盖层/统计引用与入库时刻一致，覆盖“重复内容与文件名冲突”。
- [x] 2.6 定义关闭/配对/首次/增量/暂停/离线/错误状态与取消边界；测试不可达、恢复、取消及错误状态转换，覆盖“暂停与重试”，运行 Core 测试并记录状态转换表。

## 3. SQLite、控制面与升级恢复

- [x] 3.1 新增顺序迁移保存绑定、设备授权、mutation identity/pending、inbox、带摘要链的commit log、object versions、typed song/member aliases/conflicts、带摘要peer cursors、local delete intents及传输状态；测试旧库升级/重复升级/失败回滚/UUID不变，执行 `cargo test -p echo-core`。
- [x] 3.2 用独立持久化高水位替代依赖outbox MAX生成版本，补齐全部共享对象事务内变更捕获；将PendingDelete/临时墓碑隔离为本机意图，逻辑终结与最终delete mutation同事务，物理清理独立恢复；测试清队列后revision不回退、统计/资源可恢复、撤销不上传墓碑及终结只一次，覆盖 sync-foundation 版本/预写和“可撤销意图不成为同步墓碑”。
- [x] 3.3 实现主确认事务，原子保存inbox摘要/结果、canonical对象、主序列/日志摘要链及pending-publication；按journal→独立检查点→published连续水位顺序发布，检查点经平台端口注入并提供故障fake；测试同mutation重复/不同内容、ACK丢失、各发布边界崩溃/存储失败及未发布结果不可提前读取，覆盖“确认响应丢失”“网络重复提交”“控制面发布未完成”“发布检查点崩溃窗口”。
- [x] 3.4 实现 RemoteApply 专用入口、已确认状态与pending overlay、typed成员alias重映射、批次应用及游标/摘要同事务；测试回拉自身规范结果、旧ACK不回退、拒绝前驱不永久阻塞、歌曲/成员别名后续编辑/墓碑/重导入、已发送正文不变、进程重启与重复应用无outbox增长，覆盖“远端应用及游标恢复”“接收不制造回环”“迟到确认与拒绝前驱”“别名后续编辑与重导入”“重复成员去重后删除”“成员重新添加遇到旧删除”。
- [x] 3.5 实现portable v1读取与v2可恢复升级、typed墓碑/alias投影、authority/object_version材料化及控制面journal修复；旧PendingDelete临时墓碑先恢复意图，不误判最终删除；测试部分写出失败、成员映射重建、历史字段/UUID/入库时刻保持及旧版本拒写，更新存储格式文档。恢复/部分写入故障测试见 `portable_migration.rs`，格式说明见 `docs/DESIGN.md`。
- [x] 3.6 建立历史初始基线和固定快照水位/摘要/分页生命周期，快照纳入typed aliases并保留删除窗口内最后有效对象与受控暂存资源；冻结切换窗口后新事件正常排队；测试快照并发导入、窗口内快照/撤销、分页过期、历史outbox不重放为新编辑，覆盖“首次同步期间主库新增歌曲”“撤销窗口内删除并撤销”。
- [x] 3.7 实现主账本一致性备份及显式恢复入口强制新epoch、独立发布检查点启动核对、账本丢失/回退/分叉fencing；保留pending与冲突副本、墓碑/inbox不自动GC；测试对象与账本一起回退、检查点领先/缺失/不可读、同序列不同摘要、正常发布崩溃可补齐，覆盖“主设备旧备份恢复”“对象与账本共同回退”“发布检查点崩溃窗口”“同序列日志分叉”；写出包含整机共同回退检测限制的恢复步骤。恢复 IPC/UI 已连通；步骤见 `docs/lan-sync-recovery.md`。
- [x] 3.8 将同步绑定及所有协议账本、变更捕获、提交序列、设备游标和恢复检查点按本地资料库 root 隔离；把现有单一账本迁移归属到原 binding 的 root；测试新库独立建主/加入、A/B 切换后各自 pending/peer/commit chain 连续且互不读取、未绑定库离线编辑后启动同步。覆盖测试见 `sync_root_contexts.rs`。

## 4. 媒体资源与文件恢复

- [x] 4.1 为音频、侧车、用户封面与覆盖资源建立库授权的哈希/长度/类型清单和本机路径映射，缓存可重建；测试全库资源完整与无绝对路径/凭据泄露，覆盖“共享内容边界”。
- [x] 4.2 实现 staging、块校验/完整BLAKE3校验、续传进度和journal安全发布；测试断线重启续传、错hash、块重试和文件未完整不可用，覆盖“大文件断线续传”“对象先于媒体到达”。
- [x] 4.3 实现预期旧哈希检查、外部改动冲突隔离、跨平台路径/大小写/Unicode/保留名/符号链接防护；测试已有内容不覆写、磁盘满/只读根、目录逃逸，覆盖“文件变化或校验失败”“空间权限与路径攻击”。
- [x] 4.4 实现歌曲资源版本原子关联、重复哈希复用、撤销窗口内受控暂存资源可读取、远端最终删除媒体可恢复隔离与operation来源抑制；持久化pending/已发送事件、快照、冲突和日志的资源引用，先保留不可变副本再物理清理，重启恢复引用后清理；测试song+sidecar一致、窗口内快照下载、离线导入后删除重连/导入ACK丢失后删除、废纸篓清理失败、删除重复无副作用、watcher不回声及单机缺文件，覆盖“接收不制造回环”“单机媒体缺失”“撤销窗口内删除并撤销”“未确认导入后最终删除”。覆盖见 `sync_import_resources.rs`、`sync_snapshot.rs`、`sync_resource_publication*.rs`、`remote_tombstones.rs` 与 `resource_retention.rs`。
- [x] 4.5 实现Windows文件占用的挂起发布、对象与媒体水位分离、孤立暂存安全清理；测试播放中替换不打断/不假完成和冲突引用不被清理，覆盖“正在播放的文件不能替换”“对象水位与媒体完成分离”；更新文件恢复文档。完成发布只清理日志可追溯且内容哈希/长度吻合的冗余暂存；未知来源暂存保留供诊断，见 `sync_resource_publication_recovery.rs` 与 `docs/lan-sync-recovery.md`。

## 5. HTTPS、发现与设备信任

- [x] 5.1 实现库隔离HTTPS路由、认证、续传/ACK序列摘要核对和有界请求/分页/分块/Range，接入Core用例；运行 `cargo test -p echo-sync` 契约测试验证正确响应、分叉拒绝不报空增量成功、部分成功重试、非法载荷/JSON深度/限流/重定向拒绝。Router 契约测试覆盖认证/实时撤销、错误 anchor 409 和损坏日志、mutation 确认与重试、库隔离、请求限制、Range/If-Match；真实 TLS 客户端测试覆盖拒绝重定向且不连接目标、分块 ACK 丢失后续传与重复完成。
- [x] 5.2 实现TLS1.3后续公钥pin、六位单次配对码、10分钟有效期与自动刷新、初次配对证书交换、自动登记每设备独立凭据与撤销；测试错误/过期/重放验证码、伪主后续连接、跨库访问和撤销在途会话，覆盖“错误、过期或已使用验证码”“撤销与跨库访问”。用例见 `pairing_tests.rs`、`client_tests.rs`、`server_tests.rs` 和 `sync_peer_lifecycle.rs`。
- [x] 5.3 装配平台安全存储、独立发布检查点端口与库级凭据句柄，检查点不纳入库/账本备份；绑定认证身份与origin并拒绝伪作者/统计桶；测试安全存储失败不落明文且阻止发布、检查点耐久更新/正常重启、日志与IPC状态不泄露secret、独立安装重配对，执行桌面/网络crate测试。平台适配按 design 使用 Unix 私有权限与 Windows 当前用户 ACL 的应用数据 vault，不触发系统钥匙串授权弹窗；跨平台签名安装验证留在 7.6/8.4。
- [x] 5.4 实现选定LAN接口监听、固定端口50826、mDNS标准公告/发现、手动IP与IP变化后的身份校验；过滤点对点虚拟接口并优先常见IPv4候选；测试默认关闭无通信、发现故障的手动连接、监听冲突与网卡切换，覆盖“同步关闭时离线运行”“手动输入地址和验证码加入”。真实多网卡/mDNS环境验收留在 8.4。
- [x] 5.5 文档化连接协议、配对/撤销/恢复与网络诊断，运行示例契约测试证明无需账号/公网连接；保持对外端口不含HTTP/数据库实现类型，执行架构测试。协议见 `protocol.md`，操作与诊断见 `docs/lan-sync-recovery.md`。

## 6. 首次与增量会话编排

- [x] 6.1 实现首次 enrollment journal 与主 library_id 绑定；配对时保留已有歌曲、歌单、收藏、播放统计、portable 记录及媒体文件，将本地对象先写入 durable outbox，再把主机快照合并进本地目录。身份变更、退出或重新加入不得自动清空本地资料库或移动媒体。保留首次编辑禁用与浏览/部分播放；验证空库、已有歌曲、失败/取消和断点恢复，覆盖“新设备完整恢复”“已有歌曲的本机资料库加入主机”“首次快照失败或取消”。Native integration tests in `sync_provisioner/tests.rs` and Core `sync_snapshot_apply.rs`.
- [x] 6.2 实现 snapshot H → 资源下载 → >H增量 → 追平目标流程；测试主在首次同步中新增/删除、快照失效后重试和不重复UUID，覆盖首次全部场景并更新流程文档。流程和冻结水位/资源先后顺序见同步设计及 `sync_provisioner/tests.rs`。
- [x] 6.3 实现push/pull循环、提交前检查主水位/本机已见摘要、批次前驱顺序、每设备对象/媒体游标、摘要链验证、退避抖动与本地提交唤醒；顺序正确但因对象/作者校验失败的终态拒绝必须原子推进主端来源游标，兼容旧账本时仅依据同设备、紧邻序号的已持久化终态拒绝补齐单步游标缺口，缺失前驱拒绝保持可重试；在主+两从模拟测试离线修改、多从传播、丢包、乱序、ACK丢失和分叉立即停会话，覆盖“从设备之间经主设备传播”“主设备离线”“同序列日志分叉”“拒绝前驱后继续同步”。确认/拒绝/游标覆盖见 `sync_payloads/lan_ledger.rs`、`sync_queue.rs`、`replica_tests.rs` 与 `sync_provisioner/tests.rs`。
- [x] 6.4 实现暂停/取消/恢复、服务停止/库切换/睡眠/退出的可追踪任务关闭，不跨await持锁；测试任务无泄漏、已提交事实不回滚、关闭后台行为与同步一致，覆盖“后台与退出”。取消、停止、watch 关闭与运行时唤醒测试见 `sync_runtime/tests`。
- [x] 6.5 装配主自身变更的同一确认入口，验证主及所有从在空闲收敛后对象摘要、成员引用、墓碑及存活资源一致，无重复上传；覆盖“主设备自身修改”“接收不制造回环”，运行Core/网络/桌面集成测试。主机资源发布、同一增量合并及不回声的集成覆盖见 `sync_provisioner/tests.rs` 与 `sync_payloads/lan_ledger.rs`。

- [x] 6.6 修复 Issue #52 同步自唤醒高 CPU：分离普通写通知和同步工作通知，空事务、内部游标/快照和重复发布不触发同步，新本地 outbox、首次远端发布和并发编辑仍及时处理；覆盖“接收不制造回环”。执行 `cargo test -p echo-core write_wakeup`、`cargo test -p echo-app sync_runtime`、相关发布回归、`cargo fmt --all --check`、workspace Clippy 与构建；固定播放/UI 状态对比同步开关各 60 秒 CPU，验证空闲同步额外平均 CPU <1%，记录采样证据。验证结果见 `evidence/task-6.6-sync-idle-cpu-2026-10-17.md`：额外平均 CPU 约 0.02 个百分点；全仓现有测试/format/Clippy 阻塞已单独记录。

## 7. 类型化IPC与桌面界面

- [x] 7.1 增加逻辑句柄式目录/备份选择、角色/配对/设备/状态/冲突/备份恢复命令与带库/会话序号事件；歌曲导入/删除、收藏/取消、歌单成员增删及歌单创建/删除须进入持久同步队列并唤醒同步；远端有目录变化时刷新歌曲、歌单及成员、导航计数、当前歌曲详情与封面视图，无变化时不刷新。通过 `pnpm --filter @echo/desktop generate:ipc` 生成DTO，测试序列化、过期事件丢弃、secret不进入状态及生成漂移。目录与恢复选择使用受约束的资料库 root 句柄；冲突查询/处理 IPC 已注册。
- [x] 7.2 实现设置“局域网同步”主/从流程、六位配对码和IP直接加入、设备撤销、首次进度/空间提示；移除邀请复制、待审批列表和领取凭据步骤；补React成功/验证码错误/取消/离线/不兼容/权限错误测试，覆盖配对与首次加入场景，更新界面术语与原型。设置、容量提示与根切换/旧状态隔离测试已通过。
- [x] 7.3 实现应用工具区状态/手动同步、待确认/上传下载进度、暂停重试和“已同步”准确判定；测试元数据完成但媒体未就绪不假成功及队列变化，运行前端测试。Rust 同步周期中发出节流字节进度，媒体水位未就绪时不显示完成。
- [x] 7.4 实现冲突列表双方内容/确认次序/普通值恢复、10秒撤销与最终删除新UUID恢复提示、显式主备份恢复/新epoch流程及解除绑定保留本地库；测试撤销终结竞争文案、恢复形成新变更、设备撤销不擦除本地副本，覆盖冲突、删除撤销和“主设备旧备份恢复”。主机冲突面板只允许普通字段作为新本机变更恢复；最终删除提示重新导入生成新 UUID。删除撤销、主机恢复与设备撤销不清本地库已有 Core/UI 覆盖。
- [x] 7.5 验证键盘焦点/Escape/窄屏、导航和播放不被同步打断、品牌导入保持；将一期禁止同步字符串门禁改为默认关闭/无WebView直连断言，覆盖 desktop-app-shell 修改及全部本地回归场景。工具栏焦点/Escape、设置 UI 和本地回归见前端测试；门禁见 `task-13.8.mjs`。
- [ ] 7.6 完成本组静态验证：`pnpm --filter @echo/desktop format:check`、`lint`、`typecheck`、`test`、`build`；新增macOS权限配置、确保最终 `.app` bundle 签名 identifier 与 Bundle ID 一致，并通过实际签名安装包验证权限；Windows/Linux错误文案与本地流程同步可用。macOS 签名/权限门禁、DMG 中的播放器启动和安装后 LaunchServices 启动已验证；同步发现/配对仍待真机复测，其他静态验证未完成。

## 8. 故障集成、审查与发布门槛

- [x] 8.1 建立并登记所有新/修改Scenario的验证映射及CI门禁，更新crate架构覆盖、模块规模/端口限制与IPC漂移检查；执行 `pnpm verify:self-test` 和 `pnpm verify:governance`，没有未登记或伪通过场景。场景对账 428/428，门禁注入证明 31/31。
- [x] 8.2 执行 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo build --workspace --all-features`、`cargo deny check` 与前端完整format/lint/typecheck/test/build；记录结果，独立审查事务、凭据、文件与公共API兼容边界。验证记录见 `evidence/local-implementation-2026-10-18.md`。
- [ ] 8.3 使用两台真实设备（macOS 主机 + Windows 从机，或反向）验证双向导入/修改/收藏/歌单/统计、断网编辑、并发冲突、删除撤销/重启/到期 ACK 丢失、重复成员去重后删除/重加、旧备份回退/摘要分叉、进程 kill/journal 恢复；检查 UUID、最终对象/资源摘要与无回声，留存可重复步骤与证据。当前只有两台设备，不要求第三台从机。
- [ ] 8.4 使用 macOS 和 Windows 实际安装包验证发现/手动连接/配对/撤销、局域网权限/防火墙、睡眠与网卡切换、大文件中断/磁盘满、占用文件和后台退出；测大库内存/响应与传输速度并记录，不以模拟测试替代原生证据。Linux 设备/安装包当前不可用，记录为未覆盖平台，不阻塞 macOS/Windows 验收。
- [x] 8.5 回归本地库接续、导入去重、播放、歌单、多选、音效与无网启动；验证同步关闭不产生网络流量、WebView CSP未放宽，更新用户/开发/故障恢复文档并运行 `openspec validate introduce-lan-library-sync --strict`。本地回归和门禁见 `evidence/local-implementation-2026-10-18.md`。
- [ ] 8.6 若准备PR，先完成全部验收与场景对账，再同步delta specs并验证、归档change；最终报告列出真实验证范围与任何未完成发布门槛，执行 `openspec validate --all --strict`，按工程规范交付。

## 9. 在线传播加速：长轮询 watch

- [x] 9.1 本条目原为独立 change `2026-10-17-accelerate-lan-sync-propagation`，因与局域网同步同属一个需求变更，合并入本 change（spec delta、design 决策 10、任务第 9 节一并内联）。执行 `pnpm exec openspec validate introduce-lan-library-sync --strict`。
- [x] 9.2 echo-sync 协议与服务端保持：protocol 增加 `MAX_COMMIT_WAIT_MS`（30_000）限制常量；server `pull_commits` 支持可选 `wait_ms`（缺省/0 保持原语义，>上限返回 400 `invalid_wait`，空页且 `wait_ms>0` 时以 500ms 间隔复查已发布水位直到出现条目、预算耗尽或主机 `active=false`，保持期间认证/授权/限流只计一次）。测试三态（立即有数据 / 保持后到达 / 预算耗尽空返）+ 上限拒绝 + 停止时保持退出，每条断言做反向验证（拆掉保持逻辑必红）；执行 `cargo test -p echo-sync`。
- [x] 9.3 echo-sync 客户端等待：`pull_commits` 增加 `wait: Duration` 参数，设置 `wait_ms` 查询参数，按 `wait + 5s` 覆盖请求超时（`reqwest` 请求级 timeout 覆盖客户端默认 15s，0.12.28 源码已验证），`wait` 超上限返回参数错误；`replica::synchronize_once` 传 `Duration::ZERO` 保持原行为。测试查询参数与超时覆盖；执行 `cargo test -p echo-sync`。
- [x] 9.4 echo-app 同步循环 watch：`SyncProvisioner` 增加 `watch_host_commits(cancelled, wait)`，默认返回「不支持」（测试替身与主机角色行为不变）；`sync_runtime` 增加 `WatchOutcome` 与从设备分支：轮次间 watch，本地写入/取消/停止即时响应，`Unsupported` 或重试中退回 `wait_for_sync_wakeup`，「本地写入」结果后清空循环自有通道积压；循环顶部先消费待处理唤醒（含初始 wake，`wake.send(())` 移到线程 spawn 之前），修掉初始 wake 被保持中的 watch 吞掉的集成 bug；执行 `cargo test -p echo-app --bins sync_runtime`。测试（fake 脚本化 provisioner）：无本地写入时 watch 结果驱动轮次、本地写中断 watch 且不重放积压 hint、停止在约 1 秒内退出、主机角色不 watch、旧主机降级不成忙循环、初始 wake 立即跑首轮；8 种破坏（不 watch / Advanced 不驱动 / LocalWrite 不驱动 / 无顶部 drain / 主机也 watch / Unsupported 忙循环 / 错传 cancel flag / 不 drain 通道）逐一实测转红。
- [x] 9.5 echo-app provisioner 实现：`NativeSyncProvisioner` 以自注册本地唤醒监听（writer actor 多监听者，`register_sync_waker` 不动 API）+ current-thread 执行器实现 watch：select「取消 / 250ms tick（本地唤醒）/ HTTP watch」，以 replica checkpoint 发起 limit=1 的等待拉取；空页且耗时 `<wait/2` 判定主机不保持并降级。测试（真实 DB + 真实 TLS host，扩展 `LanMergeFixture`）：主机发布结束保持 watch（记录发布时刻断言传播 <2s，主机 500ms 复查即约 1 秒，旧兜底 5s 地板，8/8 稳定）、本地写结束保持 watch、立即应答主机报 `Unsupported`（为此给 echo-app 增加 axum dev-dep 搭最小「旧主机」）；4 种破坏（server 不保持 / watch 不理本地唤醒 / 无能力探测 / Advanced 映射坏）逐一实测转红。
- [x] 9.6 验证与证据：运行 `cargo fmt --all`、clippy 与干净树逐条 diff（新增零预警）、`cargo test -p echo-sync --all-features`（45 通过）、`cargo test -p echo-app --bins`（50 通过，1 ignored 为既有慢测试）、`cargo check -p echo-app`；记录在线传播延迟证据（约 1 秒）与空闲成本证据（每 25 秒一次全轮次 + 每分钟 2 次请求，对比原 5 秒轮询）。细节见 `.workbuddy/memory/2026-10-17.md`。真机双实例验收随 8.3/8.4 设备矩阵一并落地。
  - 更正：本条「每分钟 2 次请求」的口径不成立（实测空轮次是 4 次 `/v1/commits` + 1 次 watch + 3 次 cursor 写事务）。经 9.7 修复后为 3 次拉取 + 1 次 watch + 0 写事务。
- [x] 9.7 性能专项复核后的修复（2026-10-18，分支 `codex/lan-library-sync-0.3.0`）：复核报告与逐条证据见 `artifacts/perf-review-lan-sync-20261018.md`。修复六处：
  - **增量应用改为「先对象后媒体」**：`pull_host_changes` 不再预下载本页资源，`PreparedRemoteApply` 去掉「资源必须 complete」的前置门；提交页在一个事务里落对象 + manifest + cursor，歌曲先以不可用态出现，`publish_remote_resources` 再从持久 manifest 补齐并恢复可用（与 `enroll_peer_snapshot` 同序，即 spec「对象先于媒体到达」要求的顺序）。顺带删除 `prepared_incremental_cycle` 的重复预取页，每次增量少一次 `/v1/commits`。
  - **上传资源每会话只 `prepare` 一次**：`NativeSyncProvisioner.uploaded_resources` 按 `(binding, content_hash)` 登记，`prepare` 返回 complete 或 `complete_resource` 成功后写入；每轮不再对 512 深度待确认队列整队重发（原来单轮最多 512 次请求 ⇒ 必撞 120/60s 并 `sleep(60s)`）。同时把整文件 BLAKE3 从「每资源每轮两次」降到「每会话每资源一次」。
  - **本地 blob 缺失不再让整轮失败**：`verify_complete` 出错改为跳过该资源并记 warning，其余队列继续排空。
  - **空页应用走读连接**：`apply_remote_batch` 对空页改用 `with_reader` + Deferred，不写 cursor、不进单写队列、不唤醒 writer 监听者（原来空轮次有 3–4 次 WAL 写 + 全量 UI 订阅唤醒）。
  - **`serve_tls` 的连接超时改为连接生命期**：300s → 3300s，并新增 15s `header_read_timeout` 承担 slowloris 边界；健康从设备不再每 5 分钟被掐一次连接。
  - **watch 单次传输错误不再立即报 `Failed`**：连续两次失败才进入 `retrying`，第一次静默重试，连接回收不再表现为同步故障。
  - 测试：新增 `native_incremental_applies_objects_before_audio_arrives`、`native_upload_prepares_a_resource_once_until_the_host_acks_it`、`a_single_watch_transport_failure_is_retried_before_it_is_reported`、`empty_replica_apply_persists_nothing`；逐条做「拆掉修复必红」的反向验证（分别得到：apply 被媒体 gate 拒绝 / 第二轮 prepare 计数 2 / 2 秒内进不了第二次 watch / 空页仍写 cursor）。`cargo test -p echo-app --all-features`（53 通过，1 ignored）、`cargo test -p echo-sync --all-features` 全通过；`cargo test -p echo-core` 的唯一失败 `different_hash_same_path_gets_a_stable_safe_destination` 在 `e9e81f2` 上同样失败，属既有问题，与本轮无关。
  - 仍未落地：8.3/8.4 设备矩阵、端到端 1 秒传播的真机测时、N 从设备并发 ACK 延迟压测。
- [x] 9.8 性能复核的收敛轮（2026-10-18，同一分支）：把 9.7 末尾留的四个口子补掉，并把工作树的红全部转绿。
  - **长轮询复查改走读连接**：`published_commits_after` 从 writer actor 改到 `with_reader` + `unchecked_transaction`。持有的 watch 每 500ms 复查一次，原先排在单写队列里与主确认事务（fsync + checkpoint advance）互相等；现在由 WAL 上的只读连接满足，不占写锁也不占 actor。测试 `a_held_poll_recheck_reads_without_taking_the_writer` 断言复查不产生 writer 通知（改回 writer 路径即红）。
  - **限流按面拆窗**：控制面（commits / mutations / ack / snapshot / pairing）仍 120 req/60s，资源面（prepare / chunk / complete / 下载）单独 900 req/60s（≈60 MiB/s，高于目标 Wi-Fi 链路）。原来两类共用一个窗口，一次大上传会把从设备的长期轮询打成 429 一整分钟。测试 `resource_chunks_do_not_spend_the_control_budget` 双向断言。
  - **资源双流**：`publish_remote_resources` 先按页把全部资源以两条并发流 stage 完再按序发布；`upload_pending_resources` 同样两条流（每个资源内部块序不变）。对齐 `design.md:135`「每会话两条资源并发流」。`verify_complete` 的整文件 BLAKE3 改到 `spawn_blocking`，不再占用 current-thread 执行器。
  - **空轮次降到 1 次拉取**：`merge_peer_library` 只在有待上传本地变更时才做首轮合并拉取，尾部拉取只在「本轮推送过东西或页未取尽」时执行。实测（`an_idle_cycle_reaches_the_host_exactly_once`）从 3 次 `/v1/commits` 降到 1 次，改回旧条件即得 `left: 3, right: 1`。
  - **兜底轮询 5s → 10s**：`FALLBACK_POLL_INTERVAL` / `RETRY_BASE_DELAY` 拆成两个常量（原来重试基数与轮询间隔共用 5s）。旧主机 fallback 从 ≈48 req/min 降到 ≈12 req/min。
  - **工作树转绿**：修掉 `different_hash_same_path_gets_a_stable_safe_destination`（断言的是旧的 `Same (n).flac` 命名；现契约是 `echo/media/pending/<uuid>-<n>` 预留位，本地路径仍归本地歌曲）；`cargo clippy --workspace --all-targets --all-features -- -D warnings` 从「echo-core 6 + echo-sync 6 + echo-desktop 2 + echo-app 10」降到 0（超长函数按职责拆分，`stage_confirmation` 的重复 inbox 写入与墓碑拒绝分支各提一个函数）。
  - 验证：`cargo fmt --all -- --check` 干净；`cargo test --workspace --all-features` 全绿（echo-core 599 / echo-desktop 413 / echo-app 54 / echo-sync 43，各 1 个既有 ignored）；clippy 严格档 0 告警。
- [x] 9.9 修复收藏/播放统计的无变化冲突：过期基线的确认仍推进主版本，但仅当收藏布尔值或合并后统计桶变化时创建冲突；过滤旧历史中的无变化记录，并显示这两类冲突的真实业务值。验证：`cargo fmt --all --check`、`cargo test -p echo-core --test sync_domain`、`cargo test -p echo-core sync_conflicts`。
