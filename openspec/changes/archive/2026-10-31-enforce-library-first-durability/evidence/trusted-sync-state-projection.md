# 2026-10-24 可信会话内的 sync fact 纯投影子步

> 本文保留当次实现范围与结果。2026-10-25 已核对 4.5 本身的 typed key/读回/依赖
> 重试并完成专项验证；native 也已有凭据验证后的 saved_state 接线。4.5 已勾选，
> 4.2/collector/adoption 仍未完成，见 [开发进度复核](progress-closeout-20261025.md)。

本步是 **4.2 / 4.5 的部分依赖**，不完成、不勾选这两项；不实现 collector、统一顺序协调器或 adoption。遵守 LD-06/LD-07、D2/D3/D5 以及 CODE_STANDARDS §2.1、§3.1–3.2、§5、§8。未提交、未归档，未改已发布 migration。

## 真实调用边界

已提供可调用的 Core/Application 显式重试入口：

`RetryLibraryOperation::catch_up_sync(context, sync_port)` → 普通目录 `catch_up` → `LibrarySyncRecoveryPort::retry_library_sync_state` → SQLite writer 的单事务纯事实投影。

这不是只有单元测试引用的 SQL helper，但**目前没有 native runtime 调用方**。其调用者必须已经持有实际会话凭据并验证独立 checkpoint；本步没有从便携 binding 生成这种信任，也没有把“有当前 SQL binding”当成凭据验证。后续 runtime 接线必须在原有真实 credentials/checkpoint 验证之后构造 context，不能在普通离线打开时调用它。

`SyncRecoveryContext` 不可序列化，显式包含本地 root、本地 directory LibraryId、实际本地 author、host/replica role、完整 binding 与已验证 checkpoint。SQL 在同一投影事务中再次核对 active root、实际 device_state author、完整 sync_binding/role、目录投影水位的本地 LibraryId；不建立 binding、peer、凭据、监听器或网络连接。

普通 `project_library_commits` / `RetryLibraryOperation::catch_up` 仍只恢复用户值、播放私有事件及保留证据。缺 scope、LocalUnbound、LegacyEvidence、不同 binding/epoch、不同作者的原 mutation 不会因为当前已配对而变成上传事件。LocalUnbound 将来需要显式 adoption，不能借本入口改作者或绑定。

## 已实现的闭包范围

- `sync_state` 按 ledger、confirmed、相关证明、origin sequence 排序的 frozen event、disposition 阶段投影，不调用 enqueue，不生成 UUID、冻结时间或 outbox。匹配会话内重复重试不新增事件。
- Ledger 原正文按既有 LAN digest 算法逐条验证；要求 genesis 起完整连续前缀最终恰好匹配调用者已验证 checkpoint。可用同 binding 的当前 SQL ledger 补齐前驱，但同 sequence 的不同正文/digest 是冲突。缺前驱、缺原字符串或无法连接 anchor 保留 typed unresolved。
- 新导出补充 ledger `original_body` / `created_at`、receipt `original_outcome` / `received_at`。这些是实际原字段，绝不重编码猜回旧字节或补新时钟。旧 journal 正文/digest 不改；旧导出缺字段时保留待恢复。远端协议与 IPC 未变化，SQLite schema 未变化。
- Confirmed baseline 的 object/version/sequence/payload/tombstone 必须与已验证 ledger 逐一相符。仅写 `sync_object_versions` 和可证明的资源声明，不把已确认 A 覆盖到当前用户值 B；父对象缺失、跨 root 身份或墓碑依赖保持 typed unresolved。
- Frozen 正文与 disposition 独立引用完整版本 digest。恢复原 mutation ID、作者、sequence/predecessor、base、revision、原 payload/resource JSON 字符串与冻结毫秒；更新实际 local highwater / local origin cursor，防止后续分配回退。缺 disposition、base 或前驱保持 unresolved。
- 一个 event 有多个 frozen 版本但 SQL 没有精确匹配版本时，保留 `AmbiguousMutationVersion`，不按遍历顺序、墙钟、revision 或字典序选 sender。现存精确版本可保持；迟到旧 disposition 不终结真实 rebase 后的新版本。
- Accepted receipt 必须引用同 author/mutation 的已验证 ledger 与一致 StagedConfirmation checkpoint，并保留实际接收时间/原 outcome 字符串。**Rejected receipt 没有 accepted ledger proof，本入口不恢复它为 host inbox 权威**；无独立拒绝证明时明确保留 RequiresStateRecovery。
- Song/member alias 必须出现在同 sequence 的 ledger canonical aliases 中，并有同 root 的规范父对象；只恢复 alias fact，不制造新 alias 或重跑业务合并。
- Origin cursor 必须有匹配 author/mutation/origin_seq 的已验证 ledger。Conflict 仅在 replaced/accepted 两个版本及正文都由完整 anchored ledger 证明时恢复；未被接纳的 losing sketch 不能冒充历史 confirmed，缺证明仍保留原冲突资料。
- 新恢复的**非空 resource candidate** 在缺完整 upload bytes/publication/location 依赖消费者时保留 RequiresStateRecovery，不能只凭 frozen 声明加入可发队列。已有精确匹配、属于当前会话的 pending queue 可保留其资源；最终有效 manifests 跟随最新 pending，防止 confirmed A 的旧声明覆盖 pending B。
- 事务中的任意 SQL 错误回滚所有新 protocol facts 与状态报告更新；已经完成的普通离线用户值投影和目录提交不丢失。

新增 `MissingSyncDependency`、`AmbiguousMutationVersion`、`InvalidStateFact` typed 原因；所有未知或不支持的 fact 保留原件和 RequiresStateRecovery，不静默丢弃。

## 后续最小依赖，不能算本步已完成

1. 实际冻结入口的 `FrozenWriteSet` collector 必须捕获 freeze-time Bound/LocalUnbound scope、精确事件原字符串、当前 active frozen version、完整前驱/base/终结及资源依赖；现有 `sync_mutations` 无 freeze binding / sent seal，真实历史导出继续 LegacyEvidence。测试中的显式 Bound frozen 只模拟未来 collector 已经给出的合法事实，不声称现有 exporter 会做此转换。
2. 已发送正文版本与重规划版本的 active-version / sent proof、拒绝 outcome 的独立可信证明与 origin 消费链仍需明确持久契约，不能从今天 binding 或 pending/confirmed 字段猜回。
3. Publications 当前缺原 freeze scope、完整 declaration/claim/location/completion 事实；peer cursor 缺当前认证远端身份和完整资源完成状态；snapshot baseline 可能没有 genesis 起的全部 ledger；相应事实继续 RequiresStateRecovery。当前前缀验证偏保守，不能代替可信 snapshot 的专门恢复协议。
4. 本步只消费当前协议已经足以证明的 alias/conflict；缺失 accepted/replaced 版本、只有未接纳 losing sketch、缺父/规范 alias 的事实仍 partial。没有实现完整引用图 GC、strict delta、host epoch 恢复或重新配对 adoption。
5. `sync/durability::commit_projection` 仍提交原规划 tx；4.2 所需“规划回滚 → 目录 commit → 唯一按水位纯投影”尚未替换。actor 内不能套 writer.run；下一步必须在原 actor 请求内协调。所有本地/LAN 生产入口也尚未接此新 API，因此 4.2/4.5 仍不能整项勾选。

## 实际验证

- `cargo test -p echo-core library_durability_trusted_sync_ --lib`：最终资源收紧后 **13 passed、0 failed**，3.18 秒。专项覆盖真实 SQLite + 原生临时目录提交/重建，A baseline/B 用户值、原 pending 字符串/时间/UUID/前驱、重复无回声、binding/author/role/本地目录/缺会话拒绝、Legacy 与新作者不重发、坏 anchor、缺父后目录追加、事务触发器失败全回滚、多版本保留、真实 rebase 后迟到 disposition、拒绝 receipt 保留、可证明 conflict、资源候选缺依赖与已有 pending 资源保持。
- 资源收紧前：`cargo test -p echo-core library_durability_ --lib` **86 passed、0 failed**，35.96 秒；不能冒充资源收紧后的完整跑次。
- `cargo test -p echo-core --test arch`：**7 passed、0 failed**，0.22 秒（资源收紧前；之后只修改同层 projector 与就近测试，未增依赖边）。
- 最终资源收紧后 `cargo clippy -p echo-core --all-targets --all-features -- -D warnings`：通过，6.27 秒。
- 最终 `cargo fmt --all --check`、`node scripts/verify/check-scale.mjs`、`git diff --check`：通过，规模白名单未增加。

本步未跑完整 Core/workspace/all-features、前端/e2e、governance、原生分页 513 或三平台矩阵；主 Agent 后续独立复核与结果另记。没有重新启动主 Agent 已停止的长测。
