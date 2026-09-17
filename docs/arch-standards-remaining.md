# 架构/规范治理 — 剩余工作交接

> 基线：`main` 上 `089a50a`（`chore: enforce architecture and code standards`）之后的 5 个收尾提交，
> 用 `git log --oneline -6` 定位。下面所有数值都在这组提交的顶端重跑过。
> OpenSpec change：`openspec/changes/enforce-architecture-and-code-standards/`
> 完成度：**38/47**，逐项原因见该 change 的 `tasks.md` 末尾「收尾状态」表。
>
> 本文只写**还没做完的事**，以及**怎么判断做完**。所有数值都在本文件所基于的 HEAD 上重跑过。

---

## 0. 已实测基线（在当前 HEAD 重跑过，可直接引用）

| 项 | 实测值 | 命令 |
|---|---|---|
| Rust 测试 | 732 passed / 0 failed | `cargo test --workspace` |
| Core 单元覆盖率 | **91.77% 行**（91.19% 区域） | `cargo llvm-cov -p echo-core --all-features --ignore-filename-regex application/testing --fail-under-lines 90` |
| 前端测试 | 177 passed / 26 files | `pnpm test --run` |
| lint | 0 error / 6 warning | `pnpm lint` |
| 规模豁免 | 11 文件 / 4 trait（棘轮，只能减） | `node scripts/verify/check-scale.mjs` |
| 场景对账 | spec 218 = trace 218 = manifest 218 | `node scripts/verify/reconcile-scenarios.mjs` |
| 场景命令重复度 | 218 场景 / 124 命令（1.76x），最差簇 16 | `node scripts/verify/check-scenario-churn.mjs` |
| 注册检查数 | 84 个 | `node scripts/verify/check-verification-validity.mjs` |
| 聚合门禁 | **exit 0，12 道全绿** | `pnpm verify:governance` |
| clippy `-D warnings` | **❌ 93 条 warning** | `cargo clippy --workspace --all-targets -- -D warnings` |

**唯一结构性红灯是 clippy。** 覆盖率是绿的——但见下面第 8 节的坑，它是"看起来红过"的那一个。

---

## 1. 先跑通这些命令（照抄，不要凭印象）

```bash
# 前端
cd apps/desktop && pnpm typecheck && pnpm lint && pnpm test --run && pnpm build

# Rust
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings   # ← 当前唯一结构性红灯

# 治理门禁（聚合入口，等价于 CI 的 governance job）
pnpm verify:governance

# 构建纯净性（含产物级证明，会真跑一次 cargo check）
ECHO_PURITY_REQUIRE_BUILD=1 node scripts/verify/check-build-purity.mjs

# OpenSpec（CLI 是根 devDependency，走 pnpm exec；CI 同一条命令）
pnpm exec openspec validate enforce-architecture-and-code-standards --strict
pnpm exec openspec validate --archived
```

注意：`cargo` 子命令的参数分隔符 `--` 不能省；`src-tauri` 的包名是 **`echo-app`**（不是 `echo-desktop`），
所以要单独查它用 `cargo check -p echo-app`。

---

## 2. 唯一的结构性红灯：clippy 存量告警（任务 2.3 / 8.1）

`cargo clippy --workspace --all-targets -- -D warnings` **失败**。

当前实测（`cargo clippy --workspace --all-targets`，本 HEAD）：**93 条 warning**，分布：

| 条数 | lint |
|---:|---|
| 14 | `wildcard_imports`（wildcard import） |
| 12 | `default_trait_access`（`HashSet::default()` 更清晰） |
| 11 | `redundant_pub_crate`（private module 里的 `pub(crate)`） |
| 8 | `significant_drop_tightening` |
| 6 | `missing_panics_doc` |
| 12 | `cast_possible_truncation` / `cast_possible_wrap`（usize→i32，各 6） |
| 4 | `float_cmp` |
| 4 | `missing_errors_doc` |
| 3 | `filter_map_bool_then` |
| 2 | `needless_lifetimes` |
| 其余 | 各 1 条（underscore binding、too_many_lines、needless_pass_by_value、implicit_hasher、items_after_test_module、single_char_push_str、similar_names 等） |

**判据**：`cargo clippy --workspace --all-targets -- -D warnings` 退出码 0。

**建议做法**：不要一次性 `--fix`。按 **lint 分组**推进，每组一次提交 + 一次全量 `cargo test`：
1. 机械类（`redundant_pub_crate`、`needless_lifetimes`、`filter_map_bool_then`、`single_char_push_str`）——
   纯改写，风险最低，先清掉约 17 条。
2. 风格类（`wildcard_imports` 14、`default_trait_access` 12）——需要把通配导入展开成显式列表；
   **注意 `prelude` 类模块展开后容易漏 item**，改完必须编译。
3. 语义类（`significant_drop_tightening` 8、`cast_*` 12、`float_cmp` 4）——**必须逐个看**，
   每一处都可能是真实缺陷或误报，禁止批量 `#[allow]`。
4. 文档类（`missing_panics_doc` / `missing_errors_doc` 共 10）——补 `# Panics` / `# Errors` 段。

**同时要确认**：这些 lint 是从哪来的（workspace `[lints]`？`Cargo.toml`？CI？）。
如果它们**不在任何 lint 配置里、只是因为默认 `clippy::all` 升级才出现**，那"清零"这件事的性质就变了——
先确认门禁到底要求哪一档，别为了绿灯去 `allow`。

---

## 3. 剩余规模超标文件（任务 5.5 / 5.8）

`scripts/verify/check-scale.mjs` 是一条**棘轮门禁**：豁免清单 `scripts/verify/scale-allowlist.json`
**只能减不能增**。当前豁免 11 个文件 + 4 个 trait。

本轮已从清单里摘掉 4 个文件 / 2 个 trait（`import.rs`、`sqlite/tests.rs`、`runtime/services.rs`、
`runtime/player.rs`；`TxAccess`、`LibraryFileSystem`）。

仍在清单里、**还能继续拆的**（本 HEAD 实测）：

| 文件 | 行数 | 备注 |
|---|---:|---|
| `crates/echo-desktop/src/player/coordinator.rs` | 1377 | |
| `crates/echo-desktop/src/platform/local_state.rs` | 1131 | |
| `crates/echo-desktop/src/player/queue.rs` | 1114 | |
| `crates/echo-core/src/application/testing/memory_database.rs` | 1028 | 测试替身，任务 5.8 |

其余豁免项（`recover.rs`、`scan.rs`、`domain/entities.rs`、`domain/library.rs`、
`infrastructure/filesystem/adapter.rs`、`infrastructure/sqlite/mod.rs`、`player/actor.rs`）尚未逐个复核是否真的拆不动。

**拆分手法（本轮已用两次，推荐照搬）**：
`tests.rs` 这类文件本来就被 `include!` 进上级模块 → 按顶层 item 切成若干块再用 `include!` 拼回去，
**同一命名空间，交叉引用全部自动保持，不需要动任何可见性**。
- 普通 `#[cfg(test)] mod tests;` 要转成 `tests/mod.rs` + 子块，**不能同时留 `tests.rs`**
  （rustc 报 "file for module found at both"）。
- `#[test]` 属性不是独立 item，切分要**以 `fn`/`impl`/`const` 为锚点向上吸收紧邻属性与注释**。
- 块内 `include_str!("migrations/…")` 的相对基准跟着 include 后的目录走，要改成 `../migrations/`。
- **目标目录名会影响架构守卫的豁免判定**。拆出的块要放进真正的 `tests/` 目录
  （守卫豁免 `/tests/`），**不要为了让它过而放宽守卫**。

**判据**：文件降到 ≤1000 行后，从 `scale-allowlist.json` 里删掉该条目，`check-scale.mjs` 仍退出 0。
如果某个文件删条目后门禁报红 → 说明没真降下来，**不许把条目加回去**。

---

## 4. 检查有效性证明（任务 7.6）

已有 84 个检查的静态 evidence + failure 路由门禁（`check-verification-validity.mjs`），
并已用「注册一个空的 `console.log` 式检查」证明它会失败。

**还没做**：为每个检查建立**可执行的注入样例** —— 即"注入违规 → 该检查必须退出非 0"的自动化用例。
本轮只对新增的 7 条门禁手工做了这个证明（见下表），没有体系化。

已手工证明过的（可作模板）：

| 门禁 | 注入方式 | 结果 |
|---|---|---|
| `check-lint-inheritance` | 给某个 crate 加 `[lints.rust]`，不继承 workspace | 失败 ✅ |
| `check-scale` | 造一个 1100 行源文件 | 失败 ✅ |
| `check-verification-validity` | 注册一个只有 `console.log` 的空检查 | 失败 ✅ |
| `check-toolchain` | 改错位 `rust-toolchain.toml` | 失败 ✅ |
| `check-build-purity` | 去掉 `pub mod fake;` 的 `#[cfg(test)]` | 静态层 + 产物层**同时**抓到 ✅ |
| `check-scenario-churn` | 收紧阈值 | 失败 ✅ |

**建议做法**：新增 `scripts/verify/self-test.mjs` 的 `--inject` 模式，或单独一个
`scripts/verify/injection-suite.mjs`，把上表固化成可重复执行的用例。注意所有注入都要能**自动回滚**
（写临时备份 → 改 → 跑 → 还原 → 断言还原成功），否则失败的注入会污染工作树。

---

## 5. 场景命令重复治理的完整形态（任务 7.7）

已建棘轮门禁 `check-scenario-churn.mjs`：场景命令重复度 ≤1.8x、单命令 ≤16 个场景，**只能降不能升**。

**还没做**：完整的"按模块聚合并保留到测试名的追溯"。这需要改写 **218 条验收行**，
风险高（会同时牵动 `tests/*.yaml`、`gen-scenario-manifests.mjs`、`docs/traceability.md`、以及
`reconcile-scenarios.mjs` 的对账），**建议单开一个 change**，不要塞进这一个。

**先决条件**：动它之前先读 `scripts/verify/reconcile-scenarios.mjs:86` —— 它对每个登记路径做
**非空断言**。曾经有人（包括我）把"136 个 YAML 无人读取"误读成"可以删"，实际删了会让门禁
从"报红"变成"输入缺失"而崩坏。**删任何场景 YAML 之前先确认它在对账表里。**

---

## 6. 场景全量 / 真机 / 归档（任务 8.3 / 8.4 / 8.5）

- **8.3**：`pnpm verify:scenario -- --all` 没全跑过。其中含 native 举证行，需要**真机产物**（macOS 本机）。
- **8.4 真机冒烟**：没做。需要在真机上走一遍播放 / 恢复路径。可参考的排查手段见
  `.workbuddy/memory/MEMORY.md` 的「无 devtools 时的真机排查」小节
  （`eprintln!` / 临时 `#[tauri::command]` + `invoke` / `osascript` + `screencapture` 读像素 /
  改 `desktop-state.json` 的 `playbackSession.current`）。
- **8.5 归档**：依赖上面全部完成。`openspec archive` 前务必跑 `openspec validate --archived`
  （它会检查未勾选任务并**退出码 1**）。

**⚠️ 归档流程的两个静默坑**（本轮踩过）：
- 新增能力 spec 必须带 `## Purpose`（≥50 字符），否则归档后主 spec 留 `TBD` 占位。
- Scenario 必须**恰好 4 个 `#`**（`#### Scenario:`）。写成 3 个会**静默失败**，不报错。

---

## 7. 本轮新增并已「接电」的门禁（背景信息，勿重复造）

`scripts/verify/manifest.json` 新增任务 **14.1–14.7**，接入 `pnpm verify:governance`
（`scripts/verify/ci-governance.mjs`）与 CI 的 `governance` job：

1. `check-lint-inheritance.mjs` — lint 配置继承
2. `check-scale.mjs` + `scale-allowlist.json` — 规模棘轮
3. `check-toolchain.mjs` — 工具链钉版
4. `validate-scenario-manifests.mjs` — 场景 YAML 字段
5. `check-verification-validity.mjs` — 检查有效性（静态）
6. `check-build-purity.mjs` — 构建纯净性（静态 + 产物级）
7. `check-scenario-churn.mjs` — 场景命令重复棘轮

**教训值得重复一遍**：这 5 个检查（1/2/3/4/5）在本轮之前**已经存在，但不在 `manifest.json` 里**，
所以从来没人执行过 —— 排查门禁类问题时，**先问"这个检查有没有被执行"，再考虑写新检查**。
新写但不接电 = 又一个死门禁。

---

## 8. 施工纪律（本轮用血换的）

1. **动手前先备份工作树**。本轮 `main` 上有 88 个未提交改动且**编译是红的**；
   `tar czf /tmp/echo-wip-*.tar.gz` 这个动作后来真救了场（一个子代理留下 525 个编译错误的半成品，
   靠它整份回滚）。
2. **高峰期不要把事情摊给多个 sub-agent**。本轮 3 个并行子代理**同时撞模型速率限制集体阵亡**，
   留下半吊子工作树。它们共享同一速率池。
3. **"统计为 0" 极易被误读成"代码里没有"**。`grep -E '^\s+pub fn'` 在 BSD grep 下恒 0 命中（不支持 `\s`）。
   要换 `[[:space:]]` 或换工具复核。
4. **结论会随 HEAD 漂移**。写进 spec 的每个数值都要能回答"在哪个 HEAD、用什么命令重跑出来"。
   本轮定稿的方案被独立复核纠正了 8 处，其中 3 处会导致错误施工。
5. **不要为了让门禁变绿而放宽门禁**。本轮拆测试文件时架构守卫真抓到一条存量越界，
   正确做法是改目录让它合规。

6. **覆盖率数字在大重构后不可信，要先清插桩缓存**。本轮 `cargo llvm-cov` 复用了拆分前的
   instrumented 产物，把已缩到 8 行的 `application/import.rs` 仍按 603 行的旧版本统计，
   报出 **73.48%** 的假红；`cargo llvm-cov clean --workspace` 后同一命令得到 **91.77%**，
   门禁 exit 0。**看到覆盖率突然掉十几个点，先 `clean` 再下结论**——否则会去追一个不存在的
   覆盖率缺口。CI 上是干净 runner，不受影响；这个坑只在本机复现。
