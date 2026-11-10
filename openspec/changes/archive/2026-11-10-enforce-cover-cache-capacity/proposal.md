# Proposal

## Why

#111 让列表类视图改读 128px / 512px 缩略图，缩略图按需生成后落盘在既有封面缓存条目下（`thumb-128.bin`、`thumb-512.bin`），于是 `DiskCoverCache::stored_bytes()` 会随浏览范围持续增长；而 `gc()` 的 pass 2 在超限时**只写一条 `tracing::debug!`**、不删除任何文件，容量上限在缩略图场景下退化成日志（issue #126）。现在处理，是因为缩略图已进入常规读取路径，缓存增长从一次性变成持续；且 GC 目前只在应用启动装配时执行一次，越早定义回收语义，越能避免「每次启动清空缩略图却永远达不到上限」的首屏抖动。

## What Changes

- 把 `gc()` 的 pass 2 从「只记日志」改为可执行的容量回收：在**被引用条目内部**区分不可重建资产（`original.bin`、`mime.txt`）与可重建资产（128 / 512 两档缩略图），容量回收只作用于后者。
- 定义确定性的可重建优先级：详情档（512px）先于列表档（128px）；同档按文件占用降序、内容哈希升序排序；逐个淘汰直到总字节数回到上限内即停止，不删多余文件。
- 保留引用完整性：pass 1 仍整目录删除未引用条目；任何情况下都不得删除被引用条目的原图与 MIME 记录，容量规则不得凌驾于引用关系之上。
- 定义不可达标的退化路径：若删光全部缩略图仍超上限（说明被引用原图自身已超限），GC MUST 保留全部缩略图、成功返回并记录结构化日志，不做无效且反复的删除。
- 被淘汰的缩略图继续沿用既有按需生成、原子落盘、按 `(key, 档位)` 单飞复用的语义；无新元数据、无访问时间依赖、无磁盘布局变化。
- 先补能复现「缩略图把缓存推过容量上限」的回归测试（红），再实现回收策略（绿），并覆盖淘汰顺序、原图超限退化、重建与并发。
- 同步更新 `cover.rs` 模块头注释中「超容量只碰未引用条目」的过时表述，并在 `docs/DESIGN.md` §2.4 补一句容量回收职责。

**BREAKING**：无。`CoverCache` port 与所有公开方法签名不变。

## Non-Goals

- 不引入 LRU / 访问时间元数据，不新增元数据文件、不改动内容寻址与磁盘布局（延续 #111 的非目标）。
- 不改 `CoverCache` trait、`gc()` 调用时机、默认 256 MiB 上限、`stored_bytes()` 统计口径、`cover://` 档位与 CSP、缩略图缩放算法与 JPEG 质量。
- 不清理写入中断留下的 `*.tmp` 半成品（现有原子写会覆盖它，本次不动其语义），不处理除缩略图之外的容量来源。
- 不修改 `desktop-app-shell`、`library-durability`、`local-library` 的既有需求；不改前端、桌面壳、移动端、SQLite 与 IPC 契约，不新增依赖。
- 不处理 #111 之外的封面内存问题，不新建 issue / PR 之外的交付动作。

## Capabilities

### New Capabilities

- `cover-cache-capacity`: 封面缓存容量回收的对外行为契约——何时淘汰可重建缩略图、淘汰顺序与停止条件、引用完整性、原图超限时的退化行为、淘汰后的重建与兼容边界。

### Modified Capabilities

- 无。既有能力与本 change 无需求冲突：`desktop-app-shell` 的封面协议档位、`library-durability` 的「派生索引与缩略图 MUST 可重新生成」、`local-library` 的封面写入约束均保持不变；本 change 只新增独立能力，不改写既有 Requirement 与场景 ID（新能力不在 `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX` 中，按当前治理规则无需登记 0.1.0 场景清单）。

## Impact

- **代码**：`crates/echo-core/src/infrastructure/metadata/cover.rs`（`gc()` pass 2、模块头注释）、同目录新增 `cover_capacity_tests.rs`（容量回收场景测试，见 `design.md` D6 的规模拆分理由）；`docs/DESIGN.md` §2.4；本 change 的规划产物；归档时生成 `openspec/specs/cover-cache-capacity/spec.md`。
- **接口/协议**：`CoverCache::gc`、`new` / `with_capacity` / `put` / `get` / `thumbnail` / `stored_bytes` 全部不变；无 IPC 命令、Tauri command、生成物、数据库 schema、远端协议或依赖变化；trait 仍为 4 个方法（`CODE_STANDARDS §9` 上限 6）。
- **运行时**：GC 仍只在启动装配时执行一次（`crates/echo-desktop/src/runtime/app.rs:76`）；被淘汰档位在下次请求时多一次解码与缩放，之后复用落盘结果；可达标时 `stored_bytes()` 在 GC 后回落到上限内。
- **分层与规范**：仅 Core Infrastructure 的磁盘适配器内部变化，依赖方向 Platform → Application → Domain / Infrastructure 不变；复用 Ports and Adapters 与既有 `CoverCache` port，不新增跨层依赖或抽象。适用 `openspec/CODE_STANDARDS.md` §2（注释解释约束、不静默失败、结构化日志）、§2.1（当前 735 行，改动后仍远低于 1000 行硬上限）、§3.1 / §3.2、§4.1 / §4.2、§8（临时目录与固定样本）、§9（分层条款、兼容性影响与可执行验证命令）。
- **兼容**：旧缓存条目（只有原图）行为不变；已有缩略图在超限时被淘汰后按需重建；内容哈希寻址、未知 / 畸形 key 的校验与忽略行为不变；`cover://` 的 `s=list|detail|original` 与不可变缓存头不变。
- **事实基准**：`docs/PRODUCT.md` 的本地优先与离线可用不变；`docs/DESIGN.md` §2.4 的封面资产协议增加容量回收一句；`docs/ROADMAP.md` 当前阶段范围与验收不受影响。

## 事实校核（当前 HEAD）

| 观察 | 证据 |
|---|---|
| pass 2 超限只写 debug 日志，不删文件 | `crates/echo-core/src/infrastructure/metadata/cover.rs:278-291` |
| 默认容量 256 MiB；`stored_bytes()` 递归求和目录内全部文件 | 同文件 `:63`、`:223`、`:312-330` |
| 缩略图落盘为条目目录下的 `thumb-<px>.bin`，属于被引用条目的一部分 | 同文件 `:136-137`、`:185-218` |
| 超限时现有测试明确保留被引用资产（`get()` 读原图） | 同文件 `cover_cache_gc_over_capacity_retains_referenced_assets`：`:470-489` |
| 生产唯一 GC 调用点在启动装配，且只在此时执行 | `crates/echo-desktop/src/runtime/app.rs:70-79` |
| #111 的归档 change 明确把「`gc()` 容量 / 驱逐语义」列为非目标并另立 issue | `openspec/changes/archive/2026-11-02-serve-cover-thumbnails/proposal.md` Non-Goals |
| 缩略图属于可重建派生资产 | `openspec/specs/library-durability/spec.md` LD-01：「派生索引与缩略图 MUST 可重新生成」 |
| issue #126 为 OPEN | `gh issue view 126 --repo zsxink/Echo --json number,title,state` |
