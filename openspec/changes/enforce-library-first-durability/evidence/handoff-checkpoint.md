# 2026-10-24 本次继续实施（更新中，优先于后文）

继续遵守 sub-agent 严格串行。任务 **2.4 已完成并勾选，当前 15/67**。

- write permit 在取得 OS 锁后立即进入 guard，修复 migration/manifest 失败早退
  未显式 unlock 的窗口；文件先于线程 mutex 析构。成功与两类失败路径均保留重复
  文件句柄验证释放，独立审查通过。完整 Core **749 passed / 0 failed / 1 ignored**
  （47.10 秒）；`sync_cursor` 真实目录装配后 1/1 通过。主 Agent 独立复跑锁失败路径
  与 `sync_cursor` 均通过。见 [检查点修复证据](checkpoint-validation.md)。3.1 仍缺
  真实 permit 持有期间的取消/切库/退出及长路径专项，未整项勾选。
- 513 连接失败找到短复现：同一 current-thread runtime 第一次 pull 成功后同步阻塞
  16 秒，第二次 pull 发生 request 类 transport failure；并非客户端 timeout。
  `pull_host_changes` 改为逐页等待 owned `spawn_blocking` 投影，不改 timeout 或
  fixture。慢目录心跳回归修前失败、修后通过；短 native **108 passed / 1 ignored /
  1 明确过滤**，app Clippy 通过；独立审查及主 Agent 0.65 秒专项复跑通过。
  见 [原生分页诊断](native-513.md)。用户随后要求当前子任务完成后收尾，主 Agent
  已停止原样 513 fixed 跑次（约 14 分钟仍在 host 准备；cargo PID 37657、Echo PID
  37819 均已确认退出），没有分页断言的最终结果，不能记通过或断言失败。
  日志 `/tmp/echo-library-first-20261024-native-513-fixed.log`。此前旧 governance 和
  中间诊断跑次也已停止，均不计通过。下次仍须完整原样重跑。
- 2.4 新增 [协议字段说明](../../../../docs/library-durability-protocol.md)，明确
  当前 manifest/envelope 2、marker/checkpoint 1 与未完成目标 manifest 3，typed
  tombstone/alias/scoped state/frozen/disposition 和历史缺省规则。主 Agent 协议组
  74/74、随后新增 absent 回归 1/1、portable v1/v2 fixture 4/4、Core Clippy/fmt/
  scale/strict 通过；独立审查通过。见 [协议契约证据](protocol-contract.md)。

当前唯一活动子代理 `trusted_sync_projector` 正实现可信当前会话内的纯 SQL
sync-state projector 与显式 Core 重试入口；尚未最终验证/独立审查，不勾
4.2/4.5。其后仍需真实 freeze scope/active-version 捕获、显式 FrozenWriteSet、
actor 内 rollback 规划→目录提交→按水位纯投影，以及剩余本地/LAN/迁移/UI/验收。
未归档、未创建 PR；三平台和完整治理门禁仍未通过。

---

# 2026-10-24 继续实施检查点（上一轮）

用户要求继续剩余任务，sub-agent 严格串行；本节优先于下面历史记录。仍在
`codex/issue-72-library-first-durability`，未提交、未归档、未创建 PR。

## 本轮已验证并勾选

- 2.5：共享播放桶与目录私有事件分开；实际私有文件、producer/host 共享 payload
  及 round-trip 验证见 [播放隐私证据](playback-state-privacy.md)。
- 3.2 / 3.3：独立 review 找到并修复两个既有 P1：marker 和 coverage/receipt
  在 rename 后父目录未 sync 的连续故障窗口。所有未证明 fence 先 flush 才物化，
  全部 GC 证明先 flush 才开始删除，7 项连续故障测试及复查通过。
  见 [刷盘与 GC 证据](retry-fences.md)。
- 3.6：恢复文档新增 POSIX/Windows 文件/目录 sync、替换和不支持时的失败语义；
  Windows/Linux 原生验证仍未完成。
- 9.3 / 9.4：当前界面完整备份/重新配对/不完整恢复文案核对，前端五项命令及
  Chromium mock-bridge e2e 通过，见 [前端证据](frontend-validation-20261024.md)。

当前合计 14/67 完成。4.5 不能勾：虽然 typed key、完整读回和缺父补齐已实现，
confirmed/pending 仍仅保留为 RequiresStateRecovery，生产重试消费方尚未实现。

- 4.1 已于 2026-10-24 独立复查后勾选；released v27→v28 migration、事务回滚、
  重开与 typed state 持久化验证见 [迁移证据](projection-migration.md)。
- scoped namespace / FrozenMutation 子步已完成并记录于
  [scoped state 证据](scoped-frozen-state-contract.md)，但只提供后续 4.2 的部分依赖；
  2.1/4.2/4.5/7.7 仍未完成，不能把子步等同于整项任务。

当前 tasks.md 复选框合计 **14/67**。本轮没有活动 sub-agent；后续如派代理，继续
严格串行，完成并审阅一个后才派下一个。

## 性能与当前验证

已实现 A0：RootControlPlane 持有固定 128 条成功 intrinsic 校验证书缓存，key
包含注册 root UUID、document kind 和完整实际 raw bytes 的 BLAKE3。每次仍约束
读取/解析并实时校验归属与链；锁不可用/逐出走完整校验。10 项对抗测试与独立
review 通过。1 MiB 8 次读观察为 warm 138.302ms / cold 332.729ms，不能当作 10k
或 3.7/10.4 验收；见 [缓存证据](validation-cache.md)。

- 主 Agent 在 fence/隐私补测树上跑 `cargo test -p echo-core library_durability --lib`：
  **60 passed、0 failed**，29.04 秒（在 scoped-state/resource 修复之前）。
- producer/host 播放 payload 专项：1 passed，0.07 秒。
- `cargo test -p echo-core migration --lib`：**20 passed**，1.20 秒；新测试实际升级
  released v27 schema，注入 v28 中途 SQL 失败并确认表/迁移记录回滚，重跑/重开保留
  原迁移 checksum、根时间和 typed 证据。独立审查通过，4.1 已勾选。
- scoped-state 子步最终 `cargo clippy -p echo-core --all-targets --all-features -- -D warnings`
  与 `cargo fmt --all --check` 通过，见其证据文件；这不等于全 workspace 最终验证。
- 后续全 Core 跑次结果 **744 passed、1 failed、1 ignored**（56.11 秒）。唯一失败为
  `typed_tombstones_with_the_same_uuid_coexist_and_delete_independently` 第二次连续写入
  取得 `write.lock` 时 WouldBlock；隔离重跑曾通过。主 Agent 增加显式 unlock 与“重复
  文件句柄仍存活时 permit 必须释放锁”回归，修后该回归用例通过。**尚未在此修复后
  重跑完整 Core**，所以下一次先复查 Drop/unlock 生命周期并跑相关用例与全 Core。
- 新 fence/隐私补测之前全 Core：726 passed、1 ignored，33.08 秒；不能替代之后
  最终树全量验证。
- 前端 format/lint/typecheck/test/build：全部通过（42 文件、385 测试）；e2e 通过。
- scale、diff whitespace、OpenSpec change strict：通过。

**已结束但失败，不能记通过：**

- 原样 513：`cargo test -p echo-app --bin Echo native_pull_drains_more_than_one_commit_page_without_losing_local_work -- --nocapture`，
  日志 `/tmp/echo-library-first-20261024-native-513.log`。完整 513 条 fixture 未缩减；
  运行 3090.61 秒后在 `merge_cycle.rs:209` 以 `SyncConnectionFailed` 失败，未证明分页
  恢复验收。该构建包含 A0，但早于后续 fence/scoped-state/lock 修改。下一会话需定位
  连接失败阶段与慢路径，修复后仍原样重跑该用例。
- `pnpm verify:governance`：日志 `/tmp/echo-library-first-20261024-governance.log`；
  静态场景/build-purity 检查通过；全量 app binary 测试 107 passed、1 ignored，Core
  lib 733 passed、1 ignored，其他多个集成组通过；最终在 `cargo test -p echo-core --test
  sync_cursor` 的 `empty_response_cannot_hide_a_fork_or_roll_back_the_anchor` 失败，原因是
  测试装配未配置 directory commit adapter。governance 退出 101。先修测试装配使用真实或
  合适 fake adapter，不得放松生产 directory-first 约束，再重跑 governance。440/440/440
  只是已登记场景对账，不代表本 change 新场景已经登记。

## 4.2 实施依赖（已完成只读设计分析）

最小安全路线是 actor 内一个写请求：目录恢复与水位追赶 → 业务 SQL **回滚规划**
并冻结真实写集 → 回滚 → 目录提交 → 唯一纯投影按水位追赶 → 返回普通成功。
不再 commit 原规划 tx；不能在 actor 内调用 writer.run（会自锁），不能将 raw SQL
纳入 Domain 协议，也不能仅按 mutation.object 筛全量导出。

顺序依赖为：typed binding 命名空间/冻结事实 → 当前可信会话的纯 sync-state projector
（离线仅保留证据，不能授权网络）→ 显式 FrozenWriteSet collector → Application 协调器
与本地入口 → LAN 全入口（含 queue reject/advance/rebase）→ 清理/差异完整性/性能验证。
同 kind/id 不同 binding 的证据必须共存，原 mutation 正文与终结分开；legacy journal
摘要不能改写，无法证明 scope 的旧 state 保留证据。新 binding adoption 7.7 独立。

scoped-state 依赖和迁移测试已完成并复查；审查还发现并修复非空 frozen resource 格式
错误，真实 audio+lyrics 入队→export/enrich→目录 commit 回归通过。边界和剩余风险列在
专属证据中。下一步先处理本节列出的失败验证及 lock 生命周期复查，然后进入可信
sync-fact projector、显式 FrozenWriteSet collector 与 actor 内统一协调器。完整 tasks、
平台矩阵和归档/PR 顺序仍按后面的历史约束执行。

---

# 2026-10-23 最新收尾检查点（历史）

用户本轮要求“快没有额度了，先收尾”：先验收 macOS，Windows 后续由用户手动验证，Linux 当前无资源。已停止新增实现；所有 sub-agent 均串行执行并已结束。代码保留在 `codex/issue-72-library-first-durability`，未提交、未归档、未创建 PR，不宣称整个 change 或 issue #72 已完成。

**以下最新记录优先于后面的历史检查点。**

## 本轮新增修复

- 原生 LAN fixture 使用真实 runtime/control-plane 装配并注册目录；生产 enrollment 的两处 `{}` manifest 改为合法 `LibraryManifest`，重复初始化保留原身份和媒体。
- 区分本地 `domain::library::LibraryId` 与主机 `domain::sync::LibraryId`：移除错误的 UUID 相等和通用 JSON 字段递归比较；目录 manifest/batch/chain/checkpoint/SQLite watermark 归属与实时完整 binding 检查仍保留。旧 binding 资料仅作为恢复证据，不授权联网。
- 扫描和 SQLite typed 导出从 completed publication 的当前关联取歌曲逻辑路径，避免把实际 `echo/media/` 版本位置写入逻辑 `SongRecord`。相同已发布字节的重扫只刷新派生索引，保留原 UUID、共享标题、envelope 和候选。
- Commit 继续持有线程/跨进程写锁、migration 和格式检查，消除一次重复前置恢复。已持久 checkpoint 证明且当前受约束读取内容相同的材料化文件可跳过重复发布；缺失、篡改、符号链接仍拒绝，rename 后尚未完成同步的字节仍重新发布/刷盘。
- 清理全 workspace Clippy，包括 Errors 文档、私有导入规划错误装箱、逻辑导出 helper、tests 顺序和 FS 内部 transaction 模块拆分；未扩大规模白名单或公开 crate 外 API。
- 两轮独立 reviewer 审查上述身份/路径、IO 优化及 Clippy 清理，未发现新增可操作缺陷；不代表完整 change 的事务/迁移/权限审查已完成。

详见 [身份与已发布路径修复](identity-and-published-path-fix.md)、[目录发布 IO 测量](durable-publication-io.md)。

## 主 Agent 最终实际验证（macOS）

| 命令 | 结果 |
| --- | --- |
| `cargo test -p echo-core --lib` | 715 通过、0 失败、1 忽略，32.48 秒 |
| `cargo test -p echo-desktop --lib` | 422 通过、0 失败，2.39 秒 |
| `cargo test -p echo-app --bin Echo -- --skip native_pull_drains_more_than_one_commit_page_without_losing_local_work` | 106 通过、0 失败、1 忽略、1 条明确过滤，13.59 秒；不是完整原生套件通过 |
| `cargo test -p echo-core library_durability --lib` | 独立复核 41 通过、0 失败，22.04 秒 |
| `cargo test -p echo-app --bin Echo sync_runtime::tests::host_resources` | 独立复核 2 通过、0 失败，6.25 秒 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 通过 |
| `cargo check -p echo-core -p echo-desktop -p echo-app` | 通过 |
| `cargo fmt --all --check`、`node scripts/verify/check-scale.mjs`、`git diff --check` | 通过，白名单未增加 |
| `openspec validate enforce-library-first-durability --strict` | 通过 |

原生完整套件和独立 513 条分页测试均已真实尝试但因性能退化停止，不能计通过。优化前 77/513 主机提交准备耗时 8 分 47 秒；优化后仍在 3 分 20 秒仅准备到 101/513，尚未执行拉取分页断言。没有减少 fixture 数量、添加 ignore 或放宽生产 directory-first 要求。最终选择明确过滤该单项以检查其余原生回归；后续必须原样完成它。

受控 128 次提交 IO 基准从 106.569 秒降至 14.694 秒，file sync 从 8900 降至 645；prepared/marker/checkpoint/receipt/coverage/GC 的必要同步计数保持。此结果不是 10k 验收，任务 3.7 / 10.4 未完成。

`pnpm verify:governance` 本轮未重跑，仍不能报告通过；完整 workspace/all-features 测试、前端/e2e 及三平台原生矩阵未在本轮完成。前端和 IPC 本轮没有修改，其上一次实际结果保留于历史记录，不能充当新入口验收。

## 下一次继续顺序

1. **先解除 513 分页测试的 CPU 瓶颈。** 原生 sample 热点是完整 SQL after-images 造成的 `chain → LibraryCommit::validate/calculate_digest → canonical JSON` 重复工作。已形成 A0 最小分析方案，但按用户收尾要求没有任何缓存代码落盘：由 `RootControlPlane` 显式持有固定容量成功校验证书缓存，key 为 document kind/root UUID/完整真实 raw bytes 的 BLAKE3；每次仍受约束读取、解析，并实时核对 manifest/marker/operation ID/前驱与水位；仅可跳过已证明正文的 intrinsic 校验。不得缓存整份 replay、恢复成功、错误，或只信 mtime/长度。缓存为空/逐出/不可用走完整校验。实施前补冷/热读、等长篡改、换根、改 manifest/marker、删除、第二 adapter 写入与容量逐出测试，再实测原 513 场景。A0 是待实施建议，效果未知。
2. 完成完整 `echo-app` 和 governance 后，接任务 4.2 的统一顺序协调器与真实冻结 pending/前驱捕获。当前 per-operation 闭包仍直接投影；A0 不会解决该语义缺口。严格 delta 应来自有完整依赖/终结事实的冻结写集，不能仅按当前 mutation object 筛全量导出。
3. 继续下方原检查点列出的 typed state、私有历史有界归档/引用图 GC、v3 升级围栏与一致救援、独立临时 SQLite 验证/替换、全部本地/LAN/播放/删除/写回入口、UI 和场景登记等任务。历史 state `(kind,id)` 仍可能覆盖不同 binding 的证据，adoption 未实现，不勾 2.1/3.5/4.2/7.7。
4. macOS 本轮是自动化与原生临时目录验证，不是磁盘满、真实断卷、系统重装或所有权限场景的完整认证。Windows 待用户人工验证，Linux 无资源，10.3 保持未完成；不得把交付范围悄悄改成三平台已通过。
5. 所有规定验证与任务完成后才同步/归档并创建关联 #72 PR。继续遵守一个 sub-agent 完成后再派下一个。

## 上一次检查点（历史记录）

用户要求串行 sub-agent，随后因额度不足要求收尾。目录协议 agent 已完成交接；投影 agent 触及额度上限后由主 Agent 接手，修复未完成的 trait 实现、迭代器可变性与界面回归。此检查点未完成整个 change，不归档、不创建 PR，也不宣称 issue #72 已全面修复。

## 本次落盘

- 目录协议：权威 commit、独立 seq/前驱、无锁读取、跨进程写锁、覆盖快照、按 ID 原正文收据和活跃链压缩，详见 `directory-protocol.md`。
- issue #72 的可证明/孤立两类合成 fixture、完整 typed 状态比较器，详见 `fixture-and-ipc.md`。
- SQLite 先验证完整 replay，再在同一事务投影；悬空歌曲/歌单关系保留原 envelope 与 typed reason，补齐父对象后重试关联；投影不会制造上传事件。
- 新增 projection status / retained evidence 读取端口；专用同步私有状态只保留为 `RequiresStateRecovery`，不会自动恢复联网信任。
- Core/Desktop/IPC 提供已保存待恢复、待投影和原操作 ID；批量导入可恢复原操作，不重复导入。设置页显示待更新、未关联资料和历史完整性尚未验证，可刷新既有保存前缀。
- 修复既有导入 crash fake 的新能力转发、零版本记录、同步投影回滚反馈，以及未知未来格式的只读打开回归。

## 实际验证

| 检查 | 结果 |
| --- | --- |
| `cargo test -p echo-core --lib` | 703 通过、0 失败、1 忽略（32.26 秒；随后新增投影测试单独运行） |
| `cargo test -p echo-core library_durability_projection --lib` | 新增 2 条通过：完整 retained after-images 对比、缺父补齐无回声；坏后缀在任何 DB 写之前拒绝 |
| `cargo test -p echo-desktop --lib` | 422 通过、0 失败 |
| `pnpm --filter @echo/desktop test` | 42 文件、385 条通过；现有 React act 警告仍有输出 |
| `pnpm --filter @echo/desktop typecheck` | 通过 |
| `pnpm --filter @echo/desktop format:check` | 通过 |
| `pnpm --filter @echo/desktop lint` | 全量 0 error；新组件出现的 1 条 effect-ref warning 已修复并单文件复查 |
| `pnpm --filter @echo/desktop build` | 通过 |
| IPC generator | 已实际重新生成，Desktop 全量测试包含生成物漂移检查 |
| scale / whitespace / OpenSpec strict | 通过，未增加规模白名单 |

`cargo check -p echo-core`、`cargo check -p echo-desktop`、前端构建后串行执行的 `cargo check -p echo-app`、`cargo fmt --all --check` 均通过。新增 change 场景尚未同步到主规格，所以现有 440 场景的一致性不代表本 change 的新场景登记已完成。

**`pnpm verify:governance` 未通过（退出 101）。** 场景对齐（主规格/trace/manifest 均 440）、命令校验及 build-purity 已通过；其 workspace 测试阶段在 `echo-app` 得到 86 通过、20 失败、1 忽略。多数失败发生于原生 LAN fixture 的裸数据库没有配置新的 directory adapter（`library durability: directory commit adapter is not configured`）；另有 host-publication fixture 的 manifest `{}` 无 `format_version`，以及 host-runtime `LibraryUnavailable`。不能将这 20 个失败当成已修复，也不能放宽生产持久化要求让测试假通过。复现：`cargo test -p echo-app --bin Echo` 或重跑 governance。此次未运行完整 workspace Clippy/all-features/e2e 与三平台验收。

## 后续必须继续，未勾完整任务

1. Application 统一顺序提交协调器：目前部分 per-operation 闭包仍直接写业务投影；真实冻结 pending/前驱捕获仍不完整，不能保证所有本地和 LAN 路径已消除 DB-first 窗口。
2. 私有历史状态的有界归档和完整引用图 GC：活跃链有界，但覆盖快照仍累积历史 playback/pending/receipt/ledger；资产保守保留，3.5 不完成。
3. 完整 v1/v2→v3 升级围栏、SQLite 一致救援、typed 逻辑导出、身份映射和独立临时 SQLite 完整性比对/替换。当前原件备份与基本导出不是完整迁移验收。
4. 生产播放 recorder、删除/撤销、写回/移动、封面原件，以及 LAN 快照/ACK/主确认/新 binding 意图接纳的全部入口收口。
5. 喜欢/歌单全部乐观状态和播放统计可见反馈，真实完整回填进度；新设置报告不等于迁移已经完成。
6. 新场景 registry/trace/manifest、主加两从断网重建、10k IO/收敛测量、全 workspace Clippy/all-features/e2e 与 macOS/Windows/Linux 原生证据；随后规格同步/归档，再创建关联 #72 PR。

当前继续工作的第一项应是修复上述 20 项原生 LAN 集成失败并做独立审查，再接统一提交协调器和真实候选捕获。遵守用户要求继续逐个调度，避免同时启动多个写入 agent。
