# 2026-10-24 scoped state / frozen mutation 依赖子步

此步为任务 4.2 提供 2.1 所需的部分领域契约；**不完成、不勾选 2.1 / 4.2 / 4.5 / 7.7**，不替代统一顺序协调器，也不授权联网或实施 adoption。未改已发布 SQLite migration。遵守 D2 / D3 / D5 与 CODE_STANDARDS §2.1、§3.1–3.2、§5、§8。

## 已落盘

- [x] `DurableStateChange` 新增可选 `StateKey`。完整 namespace/typed target 同时保存在 envelope 与 payload；存储 UUID 从带域分隔的完整规范 key 的 BLAKE3 派生，绝不把该 UUID 当成原 LAN mutation ID。
- [x] Namespace 明确区分真实 `Bound { binding }`、真实 `LocalUnbound { local LibraryId, author }` 与 `LegacyEvidence { source_root }`。本地 LibraryId 与 authority-owned SyncBinding.library_id 仍独立；仅 LocalUnbound 的同类型本地 LibraryId 须匹配 batch/manifest。
- [x] Typed target 覆盖 confirmed object、mutation/version、mutation disposition/version、receipt、ledger、song/member alias、origin/local-origin/peer cursor、conflict、publication。错误 key / storage UUID / payload key 或身份互相不匹配时拒绝。
- [x] `FrozenMutation` 复用 `MutationCandidate`、`ObjectKey`、`ObjectVersion`、`Revision`、`SharedResourceManifest`。保留原 mutation ID、作者、序列、前驱、对象/base、本地 revision、冻结毫秒、原 payload JSON 字节、原 resource JSON 字节、原 payload digest 及 typed 资源；没有 SQL/FS/网络 DTO 进入 Domain。
- [x] Immutable frozen 正文与 `MutationDisposition` 分离。完整 frozen 正文的规范 digest 是版本 key 的一部分，disposition 显式引用该版本；pending→confirmed/rejected 不改 frozen 存储 ID 或正文。
- [x] 现有合法 `rebase_local_chain` 不被新保护阻断：同 mutation ID 的重新规划产生新的 immutable 事实版本，旧版本保留。延迟到达的旧版本 disposition 仍指向旧版本，不覆盖新版本。FS 提交/重放与 SQLite 投影检查同完整版本 key 的 immutable 正文不被改写。
- [x] 新导出的 confirmed/alias/receipt/context cursor/conflict 从**目标 root** 的 active/archived context binding 建 key；ledger 使用该行自己的完整 binding。不能证明原 binding 的队列与 publication/local-origin 仅标记 LegacyEvidence，不从当前 binding 追认历史。
- [x] FS checkpoint 归并、retained SQLite 投影 key 与规范测试比较器使用完整 typed key。物化文件仍使用派生 storage UUID；原协议事件/operation ID 保留在 typed target/正文。
- [x] 旧 `key` 缺失用 `None` 读取，序列化继续省略。固定旧 JSON 串保持字节形状，旧提交 digest `68910618ffce0826286842645951086c2810523073341a951e00536b0b8be84c` 验证通过；没有重写旧 journal。

## 明确尚缺

1. 现有 `sync_mutations` 和 publication schema 没有 freeze-time binding 或 sent seal。此步**不能**从当前 binding、`pending`/`confirmed` 状态推定原授权或是否已经发送。4.2 必须在实际规划/冻结入口捕获真实 binding / LocalUnbound 身份和依赖；发送前的版本/授权验证仍未完成。版本化证据保留不是“允许改写已发送正文”。
2. 大部分 private body 仍是既有版本化 JSON schema：confirmed、alias、receipt、cursor、ledger、conflict、publication 目前有 typed identity key 和对应身份校验，**没有完成全部语义字段的 typed domain body**。只有本步 FrozenMutation / MutationDisposition 得到完整新结构；outcome 的内部结构也仍是保留的 JSON。delete intent / adoption / claim / playback 等旧生产者没有全部转为新 namespace。
3. LegacyEvidence 的 source root 只表示可证明的历史来源，不能补出缺失的原 binding / epoch。旧的无 key 历史事实仍保留原格式；本步不声称能消除已经发生的旧覆盖或恢复不存在的证据。
4. 未接入 active sync 恢复，未恢复 credentials/trust，未改变 rebase 业务算法或 adoption；不按新作者重发旧事件。统一协调器、完整 frozen write set / dependency closure / strict delta、引用图 GC 与跨平台完整验收仍属未完成任务。

## 实际验证

- `cargo check -p echo-core`：通过。
- `cargo test -p echo-core library_durability_ --lib`：70 通过、0 失败、676 filtered，28.32 秒；包含跨 binding/epoch 同对象共存、FS 真实 compaction 保留 frozen 新旧版本及迟到 disposition、SQLite retained scoped key、目标 active/archived root、真实 `rebase_local_chain` 后导出、旧固定 digest、错误身份拒绝。
- `cargo test -p echo-core --test arch`：7 通过；扫描实际新增 Domain 文件，验证无反向依赖/FS/SQL/网络依赖。
- `cargo clippy -p echo-core --all-targets --all-features -- -D warnings`：最终轮通过。
- 随后补充 LocalUnbound 的本地库归属校验，`cargo test -p echo-core domain::library::durability::tests --lib`：5 通过。
- `cargo fmt --all --check`、`node scripts/verify/check-scale.mjs`、`git diff --check`：最终轮通过；规模白名单未增加。

此子步未运行完整 workspace/all-features 测试、原生平台矩阵、前端或端到端验证；主 Agent 正独立运行的原样 513/governance 不能算成本子步已经通过的证据。

## 独立审查 P1 修复：非空 frozen resource 格式

初版只用空资源测试，遗漏了真实队列格式。`enqueue_sync_in_root_with_resources` 直接序列化 `SharedResourceManifest`，含 `digest: [u8; 32]` 与领域相对路径；它不是 LAN commit 的 `WireResource`（`content_hash` / 字符串路径）。原导出误用 wire 解析会使带音频/歌词/封面声明的 mutation 无法形成目录事实。

- [x] `facts::mutation` 改为读取 `Vec<SharedResourceManifest>`，复用现有 manifest 边界校验；不改入队算法或 wire 协议。
- [x] `FrozenMutation::validate` 将原 `original_resources_body` 按真实领域持久格式解析，并与 `candidate.resource_refs` 完整相等比较；不再只检查“它是数组”。原资源 JSON 字符串仍逐字节保留。
- [x] 新增真实 `set_local_sync_resources(audio + lyrics)` → `SongRepository::upsert` 入队 → `pending_local_mutations` → `export_sync_state` / `enrich_recovery_batch` → 原生临时目录 commit/recover 回归。修复前实际失败 `invalid type: map, expected a string`，修复后通过，且比较实际 SQL 原字符串与 frozen 原资源字符串。
- [x] 新增独立于 version digest 的矛盾拒绝测试：重新计算 version key 后，kind、digest、length、media_type、relative_path 任一不符、typed refs 被清空或原资源数据损坏仍被拒绝；合法非紧凑 JSON 原字节可以保留。

专项两项及 Core 架构 7 项均通过；最终 full Core / Clippy 结果见本轮交接。此修复不改变上列未完成任务和授权限制。
