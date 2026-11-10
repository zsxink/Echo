# Design

## Context

见 `proposal.md` 的 Why 与「事实校核」；这里只记录影响实现方式的现状与约束。

- `gc()` 位于 `crates/echo-core/src/infrastructure/metadata/cover.rs:260`，分两段：pass 1 按引用 hash 整目录删除未引用条目，并把 survivors 记为 `(entry_dir, directory_bytes(entry_dir))`；pass 2 求和 `total`，超过 `max_capacity` 时只写 `tracing::debug!`。
- `directory_bytes`（`:312-330`）递归求和目录内**文件**长度，不把目录节点自身 `len()` 计入。因此 `thumb-128.bin` / `thumb-512.bin` 与 `original.bin` / `mime.txt` 都会被算进 `stored_bytes()` 与 `total`。
- 缩略图由 `thumbnail_inner`（`:129-169`）按需生成：先读正式文件，未命中则按 `(hash, detail)` 取单飞 gate，再由 `generate_thumbnail` 从 `original.bin` 解码缩放，`write_atomically` 先写 `*.tmp` 再 `rename`。删除正式缩略图后，下一次读会自然走到生成分支，无需新增恢复逻辑。
- `max_capacity` 默认 256 MiB（`new`，`:63`），`with_capacity` 只用于测试与未来装配；生产唯一调用点 `crates/echo-desktop/src/runtime/app.rs:70-79` 在启动装配时按 `all_referenced_asset_keys()` 调用一次 GC，失败只 `tracing::warn!` 不阻断启动。
- 现有测试 `cover_cache_gc_over_capacity_retains_referenced_assets`（`:470-489`）断言超限时两个被引用条目的 `get()`（原图）都可读。新行为必须继续满足它——这是「容量不得删除被引用原图」的回归基线。
- `library-durability` LD-01 已声明「派生索引与缩略图 MUST 可重新生成」，把缩略图定义为可重建派生资产；本 change 不改动该 Requirement。
- `CoverCache` port（`crates/echo-core/src/application/ports/media.rs:69-83`）有 4 个方法，`CODE_STANDARDS §9` 的公开 trait 上限为 6；本次不新增方法。
- `cover.rs` 当前 735 行，`scripts/verify/check-scale.mjs` 的硬上限为 1000 行；`scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX` 只登记 12 个 0.1.0 领域，新能力 `cover-cache-capacity` 不在其中，按现行治理规则无需改动 traceability / manifest / PRD 矩阵（与 `macos-notch-player` 等后续能力一致）。

## Goals / Non-Goals

**Goals:**

- 让 `gc()` 的容量上限在缩略图场景下成为可执行、可验证的回收规则，而不是一条日志。
- 保持引用完整性：被引用原图、MIME 记录、条目目录永不被容量规则删除；未引用条目仍整目录清理。
- 回收行为确定、可测试；淘汰后缩略图能自愈重建，不引入新元数据或新跨层依赖。
- 明确「无法达标」时的退化语义，避免启动期反复清空缩略图。

**Non-Goals:**

- 不引入 LRU / atime / 元数据文件，不改磁盘布局与内容寻址，不改 `stored_bytes()` 统计口径。
- 不改 `CoverCache` trait、`gc()` 调用时机与频率、`cover://` 档位与 CSP、缩略图算法与 JPEG 质量。
- 不清理 `*.tmp` 孤儿文件（既有原子写会覆盖），不引入新的后台任务或定时 GC。
- 不修改 `desktop-app-shell` / `library-durability` / `local-library` 的既有 Requirement 与场景 ID，不改 UI、桌面壳、SQLite、IPC 与生成物。

## Decisions

### D1 用「可重建优先级」淘汰，不引入 LRU 元数据

pass 2 把 survivors 内的文件分为两类：不可重建（`original.bin`、`mime.txt`）与可重建（`thumb-512.bin`、`thumb-128.bin`），**只有**后者进入候选集。判定依据是文件名这一既有磁盘布局事实，不需要读取图片内容、不新增字段。

**理由**：`library-durability` 已把缩略图定为可重建派生资产；#111 的非目标明确「不改磁盘布局」；LRU 需要额外元数据文件（改变布局、需要原子更新与崩溃恢复）或依赖 `atime`（Windows / macOS / 各类挂载默认关闭或不可靠），还会让 `thumb-*.bin` 的每次读取都带上一次元数据写入，与「按需生成后直接复用磁盘文件」的既有快路径冲突。启动期一次性 GC 下，「冷热」信息也不足以抵消复杂度。

**被否决**：(a) 引入 `access.json` / mtime 排序；(b) 用 `std::fs::metadata().accessed()` 排序；(c) 直接删除超容量的被引用条目（违反现有测试与引用完整性）。

### D2 淘汰顺序：档位 → 占用降序 → 哈希升序，达标即停

候选按 `(tier, Reverse(len), hash)` 排序，其中 `tier` 令 512px 详情档排在 128px 列表档之前；逐个删除并累减 `total`，`total <= max_capacity` 立即 break。

**理由**：512px 文件通常更大且只用于歌单预览 / 详情，淘汰收益高、影响面小；同档按占用降序让每次删除收益最大、删除数量最少；哈希升序给出确定性 tie-breaker，保证测试可重复且跨平台一致。「达标即停」把重建抖动限制在最小集合，而不是整档清空。

**被否决**：(a) 先删光整个 512 档再考虑 128 档（会删掉大量本可保留的文件）；(b) 按 hash 字典序直接删（不确定收益，可能删很多小文件）；(c) 按 mtime 排序（D1 的跨平台问题）。

### D3 不可达标时不做无效淘汰

设 `reclaimable = Σ candidate.len`。若 `total - reclaimable > max_capacity`，说明**被引用原图 + MIME + 其它不可回收文件**本身已超限，删光缩略图也无法达标：此时 MUST 一个都不删、返回 `Ok(())`，并写结构化 `tracing::warn!`（`total` / `capacity` / `unreclaimable`，不含用户路径）。可达标时才进入 D2 的删除循环。

**理由**：GC 只在启动时跑一次，若每次都删光上一会话生成的缩略图、却永远无法达标，等于每次启动都强制首屏重新解码，属于纯粹的性能回退；保留缩略图至少让容量规则之外的读取路径仍然有效。这也把「容量规则不得凌驾于引用关系」从「不删原图」延伸到「不做无意义的破坏」。

**被否决**：删光缩略图后打日志（反复重建）；把超限当错误返回（启动装配会 warn，且用户无法处置被引用封面）。

### D4 pass 1 不变；pass 2 的数据流与错误语义

```
read_dir(cache_root)
  ├─ 未引用 hash → remove_dir_all（沿用现有 best-effort）
  └─ 引用 hash → survivors.push((entry_dir, directory_bytes(entry_dir)))
total = Σ survivors.size
if total <= capacity → Ok
candidates = 对每个 survivor 目录 read_dir，只收 is_file 且文件名属于两档的 (path, len, tier, hash)
if total - Σ len > capacity → warn!(不可达标) ; Ok        // D3
sort(candidates)                                        // D2
for c in candidates:
    remove_file(c.path)
      ├─ Ok / NotFound → total -= c.len ; if total <= capacity → break
      └─ 其它 I/O → warn!(结构化字段，含 hash 与错误) ; 继续下一个候选
debug!(total, capacity, evicted, reclaimed, "cover cache reclaimed thumbnails")
```

- 候选收集只在超限时发生，正常启动不增加额外目录扫描。
- 删除是 best-effort：单个文件失败（Windows 上被其它线程打开、权限、竞态）记 `warn!` 并继续，不让整次 GC 失败，也不静默吞掉；`NotFound` 视为已被并发删除并计入已回收字节。
- `original.bin` / `mime.txt` / `*.tmp` / 未知文件名一律不进候选，条目目录本身不删。
- 用 `#[cfg(test)]` 的 `fail_thumbnail_eviction: AtomicBool`（与既有 `fail_thumbnail_write` / `decode_count` 同形）注入一次删除失败，使「继续回收」路径可测；生产构建零开销。

### D5 并发、原子写与重建自愈

- `remove_file` 与 `read_thumbnail` 交错：读者要么读到完整旧文件，要么得到 `NotFound` 后走生成分支；两者都不会读到半成品。
- `remove_file` 与 `generate_thumbnail` 交错：生成方仍按 `*.tmp → rename` 落盘，删掉旧正式文件不影响 rename；最坏结果是 GC 白删一次，下次请求仍命中磁盘文件。
- 单飞 gate、原子写、JPEG 输出、尺寸上限、`cover://` 档位全部不改；不新增锁，也不跨 `.await` 持锁（GC 为同步函数，由启动装配调用）。
- 不处理 `*.tmp`：它不可被读取，且下一次 `write_atomically` 会覆盖同名临时文件；把它计入 `total` 但不计入 `reclaimable`，因此它的存在只会在 D3 中如实反映为「不可达标」，不会诱发对原图的删除。

### D6 文件规模与测试位置

实现（pass 2 辅助函数 + 候选结构）留在 `cover.rs` 的既有 `impl` 内；容量回收的场景测试放在**同目录** `cover_capacity_tests.rs`，以 `#[cfg(test)] #[path = "cover_capacity_tests.rs"] mod capacity_tests;` 接入，并复用 `mod tests` 中改为 `pub(super)` 的 `rgba_png` / `thumb_path` 与 `DiskCoverCache` 的既有 `#[cfg(test)]` 注入开关。

**施工期实测修正**：实现落地后 `cover.rs` 为 871 行（原 735 行 + 约 136 行实现），再把 12 条场景测试（约 320 行）堆进去会突破 `check-scale.mjs` 的 1000 行硬上限，触发 `CODE_STANDARDS §2.1`「文件增长至接近上限时应先拆分再扩展」。因此按「容量回收」边界把**测试**拆到兄弟模块：`cover.rs` 871 行、`cover_capacity_tests.rs` 319 行，两者都远低于上限。测试仍就近位于 `infrastructure::metadata` 下、同属 `cover` 模块树，不新增跨模块依赖；`pub(super)` 只把两个测试 helper 的可见性放宽到父模块，不扩大生产 API。

### D7 分层、依赖方向与 CODE_STANDARDS §9

| 项 | 结论 |
|---|---|
| 所属分层 | Core Infrastructure：`infrastructure/metadata/cover.rs` 的磁盘适配器内部；Application / Domain / Presentation 零改动 |
| 依赖方向 | Platform（`runtime/app.rs` 装配）→ Application（port 定义）→ Infrastructure 实现；不产生 Application → Infrastructure 的直接引用，`crates/echo-core/tests/arch.rs` 保持通过 |
| 复用抽象 / 设计模式 | 复用 Ports and Adapters：`CoverCache::gc` 契约不变，具体回收策略只属于 `DiskCoverCache`；复用既有的「按需生成 + 单飞 + 原子写」作为重建机制，不新增 trait、状态机、全局状态或 Service |
| 规范条款 | §2（注释解释约束、无未处理 panic / 静默失败、结构化日志）、§2.1 / §9.1（规模门禁）、§3.1 / §3.2（向内依赖、最小抽象）、§4.1 / §4.2（fmt、Clippy、类型化 `Error`）、§8（临时目录与固定样本、失败路径覆盖）、§9（分层、兼容性影响、可执行验证命令） |
| 当前实现核对 | 施工前重跑 proposal「事实校核」表中的 `rg` 与 `gh` 命令；任一不符先修正规格与 design 再编码 |
| 兼容性影响 | 无 Core 公共 API、Tauri command/event、Flutter Binding、SQLite schema、远端 JSON、UUID / 哈希 / 路径语义、磁盘布局变化；无需迁移或版本协商 |
| 待清理残留 | 无被替代实现、无无消费者导出、无失效测试替身；`cover.rs` 模块头注释中「above the capacity limit, only ever touches *unreferenced* entries」与 pass 2 旧注释属于**必须同步**的失效表述 |

### D8 兼容与迁移

无 schema / 目录格式 / 协议变化，无需数据迁移：老缓存（无缩略图）行为完全不变；有缩略图的缓存仅在超限时被淘汰部分档位，下次访问自动重建。回滚方式即回退提交：磁盘上已删除的缩略图由既有按需生成路径补回，原图从未被本 change 触碰。`docs/DESIGN.md` §2.4 增加一句容量回收职责，与「缩略图按需生成并落盘复用」「CSP 不变」并列，不改写既有条目。

## 复现证据（红，实施前）

在 `cover.rs` 当前 pass 2（只写 debug 日志）上先加入 `cover_cache_capacity_thumbnails_push_cache_over_limit`，`cargo test -p echo-core --all-features cover_cache_capacity -- --nocapture` 以 101 退出：

```text
gc must bring the cache back under the cap: 12309 <= 5928
```

即 600×400 纯色 PNG 原图 + `mime.txt` 共 5927 B，生成两档缩略图后缓存为 12309 B，GC 后仍是 12309 B——正是 issue #126 描述的「容量上限退化为日志」。实现后同一用例转绿（12 条容量场景测试全部通过），其余容量测试随实现一并落地。

## Risks / Trade-offs

- **[启动期淘汰导致首屏重新解码]** → D2 只删「刚好够」并按收益排序；D3 在不可达标时完全不删；被淘汰档位由单飞合并解码，最坏情况是一次 Lanczos3 缩放。
- **[Windows 上删除被占用的缩略图失败]** → D4 记录结构化 `warn!` 并继续，不使启动 GC 失败，不静默失败；下一次启动或下一次 GC 会再尝试。
- **[`*.tmp` 孤儿文件被计入 `total` 却不可回收]** → 明确登记为既有边界；它只会让 D3 的「不可达标」判定更保守，不会误删原图；清理孤儿临时文件另立范围。
- **[候选集依赖固定文件名，未来新增第三档缩略图会漏掉]** → 在代码注释中写明「新增档位必须同时登记到候选表」，并在 design 中登记为可演进点；当前只有两档，不引入枚举式配置以免过度设计。
- **[容量上限 256 MiB 对大曲库偏小]** → 本次不改默认值；D3 让「原图已超限」变成明确的诊断日志而非静默失效，是否调大上限属于独立产品决策。
- **[新能力不在 0.1.0 场景清单]** → 与 `macos-notch-player` 等后续能力一致，`spec-scenarios.mjs --check` / `reconcile-scenarios.mjs` / `prd-matrix.mjs` 三方集合不受影响；本 change 的场景证据由 Core 单元测试与 `openspec validate --strict` 承担。

## Migration Plan

1. 在独立分支 `fix/issue-126-cover-cache-capacity` 上先落地复现测试（红，证据见上），再实现回收策略（绿）；测试按 D6 放在同目录兄弟模块。
2. 通过 Core 与 workspace 的 format / Clippy / 测试门禁，以及 `check-scale.mjs`、`openspec validate --strict`、`pnpm verify:governance`（独占）。
3. early-sync：把 delta spec 逐字同步为 `openspec/specs/cover-cache-capacity/spec.md`，再执行 `openspec archive ... --yes`；归档后再复跑三方对账与治理门。
4. 提交、推送并创建关联 #126 的中文 PR（不直接向 `main` 推送）。
5. 回滚：回退该 PR 即可，无数据迁移；磁盘缩略图按需重建。

## 可执行验证命令

```bash
# 施工前事实校核
rg -n 'fn gc|stored_bytes|max_capacity|thumb-' crates/echo-core/src/infrastructure/metadata/cover.rs
rg -n 'cover_cache.gc' crates/echo-desktop/src/runtime/app.rs
gh issue view 126 --repo zsxink/Echo --json number,title,state

# 红 / 绿（复现与回归）
cargo test -p echo-core --all-features cover_cache_capacity
cargo test -p echo-core --all-features cover_cache_gc
cargo test -p echo-core --all-features cover_cache_thumbnail

# 静态与全量门禁
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
node scripts/verify/check-scale.mjs
node scripts/verify/spec-scenarios.mjs --check
node scripts/verify/reconcile-scenarios.mjs
node scripts/verify/prd-matrix.mjs
node scripts/verify/validate-scenario-commands.mjs
openspec validate enforce-cover-cache-capacity --strict
pnpm verify:governance   # 独占运行，不与其它扫描并行

# 归档后
node scripts/verify/spec-scenarios.mjs --check
node scripts/verify/reconcile-scenarios.mjs
pnpm verify:governance
```

## Open Questions

无。淘汰策略（优先级淘汰 vs LRU）已按 D1 定案并写入 spec；若产品侧后续要求「按访问冷热回收」，应另立 change 引入元数据与迁移方案，不在本 change 内以 `atime` 近似实现。
