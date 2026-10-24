# 2026-10-24 18/67：3.1 完成，性能优化实施中（最新）

用户随后明确要求“继续去完成任务”，停止状态已解除。当前 **18/67**，
3.1 已勾选；逐项结果见 [生命周期验收](lifecycle-acceptance-20261024.md)。
退出 handoff 回退重启 P1、恢复重试锁竞争 P2 均经有效 RED/GREEN 与串行 Sol 复审。
真实 SQLite 激活回滚、queued enter、跨进程许可、长 Unicode 根及恢复/重试均通过。
主线程 Core 814/0/1 ignored 及全部集成、Desktop 429/429；最新 native 短组
130/0/1 ignored/1 显式过滤 513；workspace all-targets/all-features check/Clippy、
fmt/scale/strict/diff 通过，测试整理后的专项再次通过。无提交/推送/归档/PR。
原样 513、完整 governance 和三平台原生认证仍未完成。

正在继续 3.7 性能工作：串行设计已完成，依据原交接及当前源码重新采样，
原样 513 基线构建 6.20 秒、120 秒上限内未完成，整组回收；
[性能验收清单](performance-20261024.md) 记录实际状态。design D2.1 先固化边界，
实现代理负责 GC 阶段目录去重与 enrich 索引，保留全部文件 sync。
**上述全量通过属于 3.1 检查点，性能 WIP 修改后必须重新验证，不能冒充当前树全绿。**
有界 scan/import（含 reservation）、10k 测量及完整 513 仍待完成，3.7 / 3.5 未勾选。

---

# 2026-10-24 用户要求停止：3.1 生命周期 WIP 交接（历史）

用户已明确要求停止任务并做交接。已停止新增实现/验证、结束实现代理，主线程与代理
均核实没有存活 Cargo/rustc/测试进程；不应继续等待任何历史 session。

详见 [生命周期完整交接](lifecycle-handoff-20261024.md)。当前分支
`codex/issue-72-library-first-durability`，改动未提交、未推送、未归档、未创建 PR；
tasks 仍为 **17/67**，2.3 / 3.1 整项未勾选。已验证子步骤保留在
[契约清单](contracts-20261024.md)，没有把部分实现勾成整项。

当前树已有实例 LocalActivity、generation、防嵌套死锁、activation 排空与重读、
扫描取消竞态、Desktop 各入口装配及 native 后台单次 shutdown 的 WIP；真实生命周期
组曾 **7 passed**，workspace all-targets check 曾通过。**这些结果早于最后扫描
RED/恢复及部分新增回归，不代表当前树最终全量通过。**

最后一次真实扫描 RED 为临时撤掉取消重查后复现 `cancellation during final commit
was lost`；脚本 finally 已恢复修复代码，主线程只读核实，**恢复后未再跑 GREEN**。
generation 的 Core 专项、新跨进程 permit 测试、native shutdown 两项单测尚未执行；
最后新增后未再 fmt/check/scale/Clippy。下次先完成这些、串行独立审查，再全量验证。
具体命令/日志和 producer 停止的审查边界见完整交接。

下面 Core 803/0/1、Desktop 423/0 与 workspace Clippy 是 **3.1 改动之前的契约组**
结果，不是当前生命周期树的最终验证。原未跟踪的性能交接文档已保留。原样 513、
governance、workspace/all-features 测试和三平台原生验收仍未完成。

---

# 2026-10-24 后续交接：资源确认与 publication.v2 契约补齐（历史）

**继续实施中的状态（优先于下面的契约组验证）：** 3.1 生命周期实现正在进行，
实例 LocalActivity 与各层既有调用点已有 WIP 改动，生产 activation/正常退出和
真实目录提交时序验收尚未收尾。下面的 803/423 及 workspace Clippy 是这一组改动
之前的契约组结果，不能视为当前生命周期 WIP 树已通过。当前仍为 17/67；3.1 未勾选。
设计决策已补入 design D3.1，后续必须先将工作树恢复可编译并完成真实 RED/GREEN、
串行独立审查和主线程验证，再更新此段状态。

- 分支仍为 `codex/issue-72-library-first-durability`；本轮代码/测试/证据尚未提交，
  未推送、未归档、未创建 PR。保留原未跟踪的 `性能优化交接文档-2026-10-24.md`。
- 子代理逐个派发，主线程编辑。已验证子步骤逐项勾选于
  [契约补充清单](contracts-20261024.md)；tasks 仍为 **17/67**，2.3 整项未勾选。
- 真实 host 带音频/歌词的确认此前误保留 v1：Domain validator 把 LAN `content_hash`
  资源当作 Domain `digest` 解码。已先复现失败，再显式转换，保留原始正文和摘要。
  补齐 scoped 同 seq、缺 ledger、真实 host typed alias、base/前驱/原文及严格负例。
- 新增严格 `echo.sync.resource-publication.v2`，校验原 journal 字符串、内外
  root/operation/completed、typed owner、资源声明集合版本、实际/旧/staging 位置、
  摘要/长度/MIME 和必填字段。真实 prepare/save/finish exporter 往返通过。历史
  v1/未知 schema 继续保留证据，不自动投影资源或授权联网，不补造主机 object_version。
- v1 显式 null 墓碑也复现恢复失败并修复：只有匹配独立验证的 ledger 删除事实时
  才转换为 SQL NULL；连续两次重试通过。v2 marker 墓碑回归继续保留。
- populated portable v1/v2/commit 字段与关系矩阵通过；目标 manifest v3 正向往返
  及完整围栏尚未实现，不能用“拒绝未知 v3”测试替代，2.3 / 8.2 继续未完成。
- 主线程最终实际验证：Core **803 passed / 0 failed / 1 ignored**（45.29 秒）、
  Desktop **423 passed / 0 failed**（2.41 秒）、workspace all-targets/all-features
  Clippy、fmt、scale（白名单未增长）、OpenSpec strict、diff 检查通过。
  日志分别为 `/tmp/echo-library-contracts-core-20261024.log` 和
  `/tmp/echo-library-contracts-desktop-20261024.log`。最终 Sol 独立审查无可操作问题。
  本轮未跑原样 513、workspace/all-features 测试、governance、三平台原生验收。
- 3.1 新核实的生产缺口：切库 cancel_all 后不等待本地扫描结束，正常退出未排空
  本地扫描/目录提交；现有 LOCAL_ACTIVITY 只覆盖部分共享租约。下次先设计并接入
  “关闭准入→取消→排空安全边界→activation/exit”，核对嵌套租约及 actor 锁顺序，
  不直接塞独占锁制造死锁。该部分本轮只分析，未修改；真实 permit 生命周期与长路径
  验收仍欠缺，3.1 未完成。4.2 仍是原规划 SQL 事务提交，未替换成唯一纯投影。

---

# 2026-10-24 收尾交接：confirmed.v2 与墓碑回归已闭环（历史）

## 当前状态

- 分支 `codex/issue-72-library-first-durability`；`tasks.md` 仍为 **17/67**，2.3 未勾选。本次提交只把已验证的 confirmed/receipt 接线与墓碑修复落盘，不归档、不创建 PR。
- 后续仍遵守子代理严格串行、主线程编辑、完成整项并验证后才勾选。

## 本轮已闭环的墓碑回归

上一轮记录的失败只是 fixture 装配错误（未授权随机作者 + origin 链冲突）。本次改为授权 peer 作为 origin，并补上 exporter 不声明、但投影必需的合成 pending 状态后，**先观察到了真实的 RED**：恢复后的 SQL baseline 是 `payload_json=Some("{\"tombstone\":true}")`，而 host 实际契约是 `payload_json=NULL`。

- 生产修复：`durability_projection/sync_state/objects.rs::confirmed` 对 typed tombstone 写入 SQL NULL（与 `host_ledger_apply` / `sync_snapshot_apply` 的既有契约一致），同版本幂等比较改为按原始 SQL 字符串比对，避免 tombstone 与普通正文被误判相等。
- 修复后该测试连续两次重试均断言 `(is_tombstone=true, payload_json=NULL, changed_at_ms=222)` 且无 live playlist。

## 当前工作树的实际验证

| 检查 | 结果 |
| --- | --- |
| `cargo test -p echo-core library_durability --lib` | 112 通过 / 0 失败 |
| `cargo test -p echo-core --lib` | 787 通过 / 0 失败 / 1 忽略（46.88 秒） |
| `cargo clippy -p echo-core --all-targets -- -D warnings` | 通过（期间修复新增测试触发的 `too_many_lines`） |
| `cargo test -p echo-desktop --lib` | 423 通过 / 0 失败 |
| `cargo fmt --all --check` | 通过 |
| `pnpm --filter @echo/desktop typecheck` | 通过 |

仍未运行：完整 workspace/all-features Clippy、`pnpm verify:governance`、原样 513 长测、三平台原生验收。这些不得视为已通过，2.3 与第 4 组仍未完成。


## 本轮 confirmed 子步骤

- 新增 `crates/echo-core/src/domain/library/durability/confirmed.rs`，通过 `state.rs` 分派校验 `echo.sync.confirmed.v2`：必须保留 author、mutation_id、version/sequence、payload/tombstone、changed_at_ms、原始 LAN commit 字符串及前后 digest；检查原始 binding、对象身份、协议版本与 commit digest。不据此授予联网权限。
- `sqlite/sync/durability.rs::complete_confirmations` 按完整 namespace + sequence 匹配本次导出的真实 ledger，只有完整正文通过校验才将导出候选升级为 v2；缺失或冲突保留 v1，不从当前身份、资源或时钟补造，不改写已提交的旧目录事实。
- `durability_projection/sync_state/objects.rs::confirmed` 接受 v2，并与独立验证 ledger 的 exact original_body/digest 对账；同版本幂等比较补上 changed_at_ms，不再忽略历史时间冲突。仍保留 pending 有效资源保护。
- 以下专项均已先观察预期失败再修复通过（历史跑次，不是当前树全量验证）：
  - `library_durability_trusted_sync_retry_rejects_same_version_with_changed_timestamp`：1/1。
  - `library_durability_trusted_sync_retry_restores_exact_baseline_and_pending_without_echo`：1/1，真实 exporter 产出 confirmed.v2，baseline A 与有效值/pending B 分离。
  - `library_durability_confirmed_v2_rejects_mismatched_original_confirmation`：1/1，缺字段/篡改正文身份等拒绝。
- 上述接线后跑过 `cargo fmt --all`；最后新增的墓碑测试尚未重新 fmt。完整 Core **784 passed / 1 ignored** 是此前 receipt 阶段结果，不能冒充 confirmed 修改后的全量结果。当前 confirmed 新模块尚未跑最终 Clippy/完整 Core。

## 最后失败：已修复（保留历史记录）

新增测试位于 `crates/echo-core/src/infrastructure/sqlite/durability_projection/sync_state/tests.rs`：
`library_durability_confirmed_tombstone_restores_sql_null_and_retries`。

- 意图：从真实 host 删除确认导出完整 tombstone，提交目录，恢复至新 SQLite，再重复重试；断言 SQL baseline 为 `(is_tombstone=true, payload_json=NULL, changed_at_ms=222)` 且无 live playlist。
- 最后执行：`cargo test -p echo-core --lib library_durability_confirmed_tombstone_restores_sql_null_and_retries`。
- 结果 **exit 101，0 passed / 1 failed / 787 filtered out**。失败发生在 `stage_confirmation_at(...).unwrap()`，错误为 `origin device is not authorized for this library`（当时 tests.rs:634）。测试用了新的随机 `DeviceId::new()`，不是已授权作者；这是 fixture 装配错误，**不是已观察到生产墓碑缺陷的有效 RED**。
- 测试改动已保留，未为交接继续修。下一步可沿用 source 已授权作者：删除候选 origin_seq=2、previous_mutation_id=原 seq1 mutation；同时避开 fixture 中 pending B 的 seq2 占位（将 pending frozen 改成 seq3 并指向删除 mutation），或另建隔离 fixture。不要直接把新随机作者改成当前作者而仍用 seq1，造成 origin 链冲突。
- 待正确装配并观察预期 RED 后，再修表示转换：真实 host 在 SQL 存 tombstone `payload_json=NULL`，commit 正文是 `{"tombstone":true}`；当前恢复比较、插入与同版本幂等仍未完整处理该差异。v1 null 兼容及 v2 marker → SQL NULL 均需明确测试，不能为了通过改变现有 host SQL 契约。

## 长验证状态纠正

下文“原样 513 正在执行”已过时：该测试与 workspace/all-features 长测被另一会话 SIGTERM，均 **exit 143**，没有最终通过结果；governance 跑次 **exit 1**，本线程尚未展开最终失败日志。三组均不得计为通过或描述为仍在运行。

- 跨会话报告三组长测持续高 CPU 后终止；暂不重启三组并发长测。尚无性能采样证明根因。
- 全量 records/state 导出、batch 解码/哈希、`enrich_recovery_batch` 逐条线性查找仅是候选热点，不能当成已证实根因；后续先做阶段进度/计时及采样，再单项有界验证。
- 原样 513 的完整验收仍欠缺，短 suite 不替代。相关旧日志路径保留在下文。

## 继续顺序与验收边界

1. 修墓碑测试的授权作者/origin 链装配，得到有效 RED，再实现 SQL NULL/领域 marker 转换并验证重复重试。
2. 补 confirmed 非空资源、aliases、base/predecessor、缺 ledger 保留 v1、active/archived 同 seq 不串 scope、旧/未知格式兼容覆盖。
3. 继续 2.3 的实际受管资源 publication 严格契约及 portable 字段/关系兼容矩阵；目前不能勾选 2.3。
4. 串行独立审查，跑受影响专项、fmt、Core Clippy/完整测试及规模检查，再更新协议/证据；长测按上述性能约束另行定位，不静默跳过最终验收。
5. 4.2 真实 FrozenWriteSet / 规划回滚 → 目录提交 → 唯一纯投影、4.5 生产 retained retry 等仍未完成。现有 `commit_projection` 仍提交原规划 SQL 事务，不能用此轮 confirmed 接线代替。

本次收尾仅核对分支、任务计数、工作树并更新交接；`git diff --check` 当次无输出。未重新运行测试，不宣称当前工作树全部通过。

---

# 2026-10-24 本轮继续实施（历史记录，最新状态以上文为准）

- 2.1 完成时 `tasks.md` 为 **16/67**（2.2 完成后的最新计数见下一条）：2.1 已完成并勾选；目录 change set 增加 typed 预期版本、完整依赖及资源原件校验，保留无关历史悬空证据。主线程 `library_durability --lib` **96/96**、Core Clippy、fmt 通过；代理全 Core 为 **771 passed / 1 ignored**，集成测试通过。生产协调器接入仍属于 4.2，不据此勾选。
- 2.2 已完成并勾选，当前 **17/67**。保留 `ControlPlanePort::durability()` 能力访问器，仅禁止其余五个重复转发方法；聚焦 `LibraryProjectionPort` 与历史救援声明分开，新增目录边界故障 fake、投影 sink 和 UoW 仅投影契约回归。独立复查无可操作缺陷；目录端口 6 方法、投影端口 3 方法、ControlPlane 15 方法不高于既有基线，规模白名单未增长。代理严格串行。
- 2.2 后主线程完整 Core **774 passed / 1 ignored**，架构 **8/8**，集成测试均通过；Desktop Clippy、fmt、diff 检查通过。原生短套件 **119 passed / 1 ignored / 1 明确过滤**，不替代仍在执行的原样 513 测试。`commands.rs` 原样拆出四个解析 helper 至 `commands/parsing.rs` 后 scale、App check/Clippy 通过；未扩大白名单或改变解析语义。
- 已核实下文资源审查窗口并补三类精准回归：缺 frozen、缺 disposition、版本不匹配均不得让 confirmed 覆盖有效 pending 资源；pending 终结后原子重选引用，覆盖跨 root、重复重试与 SQL 回滚。
- Desktop 原 E0004 已修复；随后实际生成 IPC 暴露生成器仍硬编码旧恢复原因，前端 typecheck 报 TS2353。新增真实生成输出/serde 契约回归，先观察 `missing missingSyncDependency` 失败，再修 generator 并实际重新生成，未以手改生成物代替。
- 修复后主线程验证：Desktop **423/423**；前端 **44 文件 / 402 测试**，typecheck、format:check、lint、build 均通过。测试仍输出 React act 和部分 mock 告警，不声称输出无告警。以上不等于 9.1/9.2 整项完成。
- 当前树 workspace all-targets/all-features Clippy 与 Chromium mock-bridge e2e 已通过；e2e 不替代原生或三平台验收。
- 原样 513 条原生分页测试已重新启动，日志 `/tmp/echo-library-first-20261024-native-513-current.log`；尚无最终结果，不计通过。完整 governance、workspace/all-features 和三平台原生验收仍未完成。
- 2.3 继续实施，尚未勾选：控制面替换前严格解析已有原件，坏 JSON/未知格式拒写且保留原字节；legacy 写入不得把旧确认继承给不同用户值；本地 manifest 与 sync library 身份独立。四类回归均先观察失败，控制面组 **17/17** 通过。
- 原件引用新增 MIME token 与小写 BLAKE3 校验；大写摘要回归先观察失败后复用既有 digest 校验修复，change-set 组 **7/7** 通过。旧 JSON 解码未被无差别收紧。
- 修复测试 redundant clone 后，Core all-targets/all-features Clippy、持久化组 **104/104**、fmt、diff 检查通过；该轮早于随后新增的小写摘要回归，不冒充最后树完整验证。严格 confirmed/receipt/publication 新正文与历史 v1 兼容接线仍在实施。
- 2.3 的 receipt.v2 已接真实 exporter 与可信恢复 projector：完整 host outcome 经严格校验后标为 v2；无 schema 的 SQL 历史/replica applied outcome 不补造 checkpoint，保留 v1 和原始字符串。既有已提交 v1 不改写。sequence/checkpoint 必须显式存在（允许拒绝态 null），缺字段回归均观察到红后修复；accepted binding/sequence/identity 及 round-trip、旧 Bound v1 保留测试通过。
- receipt 接线后持久化组 **108/108** 通过；随后新增 replica 原文保留测试，export 组 **3/3**、receipt 组 **3/3**、Core all-targets/all-features Clippy、diff 检查通过。随后完整 Core exit 0：单元 **784 passed / 1 ignored**、架构 **8/8**、其余集成 **7+1+2+19** 均通过，日志 `/tmp/echo-library-first-core-receipt-v2.log`；其后仅加强真实恢复测试对 digest/original_outcome/sequence/received_at 原样恢复的断言，该专项 **1/1** 通过。不据此勾选 2.3，confirmed/publication 完整契约仍待实施。
- 本轮工作树有未提交改动；未提交、未推送、未归档、未创建 PR。

---

# 2026-10-24 Codex 后续会话交接补记（历史记录）

本节根据 Codex 会话
`~/.codex/sessions/2026/10/24/rollout-2026-10-24T07-01-03-01a16aff-0fa3-7b81-9876-a5fbe499341a.jsonl`
补齐原检查点之后的进展。会话最后因 `usage_limit_exceeded` 中断，未正常完成收尾；
不能继续把下面的 `trusted_sync_projector` 视为仍在执行。此次仅整理交接文档，
未重新运行代码验证，也未继续实现。

本次继续已闭环两项阻塞：

- Desktop/App 的 exit 101 根因是 `ProjectionUnresolvedReason` 新增三个枚举项后，
  `echo-desktop` IPC 转换未穷尽匹配（E0004）。已补 Rust DTO 映射、IPC 类型和恢复界面文案；
  `cargo check -p echo-desktop --lib` 通过。该修复不等于 9.1 完成，完整生成/序列化/端到端验收仍待执行。
- confirmed 基线推进覆盖 pending 资源的风险已用缺失 frozen、缺失 disposition、版本不匹配
  三类回归复现并修复。confirmed 更新在同 root/对象仍有 pending 时保留有效资源；pending
  终结后在同一事务内重新选择剩余 pending 或 confirmed 资源；跨 root、重复重试及 SQL
  回滚均有回归。专项 `cargo test -p echo-core durability_projection` 22/22、
  `cargo fmt --all -- --check`、Core clippy、`sync_domain` 19/19 通过。该修复尚未使
  7.2/7.5 整项完成。

当前 `tasks.md` 仍为 **15/67**；**3.1 / 4.2 / 4.5 未勾选**。

本次随后修复了 Core 集成测试装配：`sync_cursor_enrollment` 的 host/replica/fork fixture
此前未配置 directory commit adapter，导致两个测试在生产 directory-first 约束处正确拒绝。
测试现在为各 fixture 注册真实 `RootControlPlane`、manifest，并在重启后从数据库恢复 active
root 再装配；生产代码未改。`cargo test -p echo-core --test sync_cursor_enrollment` 为 2/2
通过。该修复不代表 12.2 或 governance 全量门禁完成。
本次核对分支为 `codex/issue-72-library-first-durability`，更新本文前工作树干净，
HEAD 为 `4310156`。旧会话中的“未提交”是当时状态，不作为当前提交状态断言；
change 仍位于活动目录，未完成归档，不能宣称整个 change 已验收。

## 后续已落盘的子步骤与边界

- 可信当前会话内的 sync-state 纯 SQL projector 及 Core 显式重试入口已落盘，
  不再只是正在实施的计划。调用链为
  `RetryLibraryOperation::catch_up_sync` → 普通目录 `catch_up` →
  `LibrarySyncRecoveryPort::retry_library_sync_state` → SQLite writer 单事务投影。
  详见 [可信同步状态投影证据](trusted-sync-state-projection.md)。
- Context 显式携带 root、本地 LibraryId、author、role、完整 binding 和已验证的独立
  checkpoint；投影事务再次核对身份与水位。离线恢复只保留证据，不据便携 binding
  恢复联网信任，不生成新 mutation 身份或 outbox 回声。
- **尚无 native runtime 调用方**，真实 freeze scope / active-version 捕获、显式
  `FrozenWriteSet` collector 和统一提交协调器仍未完成。现有
  `sync/durability::commit_projection` 仍提交原规划事务，尚未替换成
  “规划回滚 → 目录提交 → 按水位唯一纯投影”；不能因此勾选 4.2 / 4.5。

## 最后审查与验证：不可遗漏的未解决项

1. **资源引用审查问题尚未闭环。** 会话末尾独立审查指出：恢复新的 confirmed 基线
   可能覆盖 SQLite 中仍待确认候选的资源引用。尽管子步证据已描述“已有精确 pending
   资源保持”并报告相关测试，最后审查仍提出遗漏；会话仅承诺修复与精准回归，
   没有后续修复及验证完成证据。继续前须核实该具体窗口，不能按已有 13 项专项通过
   认定风险已消除。
2. **最终 Desktop / App 跑次均退出 101。** 会话未展开具体错误，不能断言是测试断言、
   编译还是其他环境问题；优先检查以下日志，日志不可用时原样复跑取得错误：
   - `/tmp/echo-library-first-20261024-wrap-desktop.log`
   - `/tmp/echo-library-first-20261024-wrap-app.log`
   App 跑次明确跳过 513 条长测；之前短 native 的 108 passed 不能替代最终失败跑次。
3. 最后 Core 收尾跑次记录 **exit 0**，但未捕获最终测试数量；日志为
   `/tmp/echo-library-first-20261024-wrap-core.log`。不能把前一轮的
   **749 passed / 0 failed / 1 ignored** 数量套用到这一跑次。
4. 可信投影资源收紧后的专项记录 **13 passed / 0 failed**，Core Clippy、fmt、scale、
   diff 检查通过；资源收紧前的 durability 组为 **86 passed / 0 failed**、arch 为
   **7 passed / 0 failed**。这些是历史会话结果，且均不代表最后审查问题已解决、
   完整 workspace 或治理门禁通过。
5. 修复后的原样 513 条长测按收尾要求停止，仍未执行完分页断言；既不计通过，
   也不能据此断言修复失败。保留日志
   `/tmp/echo-library-first-20261024-native-513-fixed.log`，下次仍须原样完成：
   `cargo test -p echo-app --bin Echo native_pull_drains_more_than_one_commit_page_without_losing_local_work -- --nocapture`。

## 下一次继续顺序

1. 先核实并闭环 confirmed / pending 资源引用审查问题，补精准回归和独立复查；
   同时查明最终 Desktop / App 的 exit 101，修复后重跑受影响验证。
2. 完整原样重跑 513 条分页场景，再完成缺失的治理及全量验证；不得减少 fixture、
   放宽 directory-first 约束或用过滤后的短套件冒充完整通过。
3. 按依赖推进真实冻结写集捕获、actor 内统一顺序协调器、可信 runtime 接线及全部
   本地/LAN 入口；继续保持 3.1 / 4.2 / 4.5 未完成，直至整项行为与验收均满足。
4. 后续 sub-agent 仍严格串行。剩余迁移、资源/私有历史 GC、UI、场景登记与三平台
   证据仍按 tasks 执行；完成规定验证后才同步规格、归档并创建关联 #72 的 PR。

---

# 2026-10-24 本次继续实施（历史检查点）

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

本历史检查点写入时，唯一活动子代理 `trusted_sync_projector` 正实现可信当前会话内的纯 SQL
sync-state projector 与显式 Core 重试入口；该子步后续已落盘，最终审查与失败验证见本文顶部，
不再表示代理仍活动，4.2/4.5 仍不勾选。其后仍需真实 freeze scope/active-version 捕获、显式 FrozenWriteSet、
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
