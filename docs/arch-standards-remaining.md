# 架构/规范治理 — 剩余工作交接

> 基线：`main` 上 `6866700`（`test(verify): prove every gate fails when its rule is violated`）。
> 用 `git log --oneline -12` 可回看到本段全部提交。
> OpenSpec change：`openspec/changes/archive/2026-09-18-enforce-architecture-and-code-standards/`（已归档）
> 完成度：**43/47**，逐项原因见该 change 的 `tasks.md` 末尾「收尾状态」表。
>
> 本文只写**还没做完的事**、**怎么判断做完**，以及**本轮用血换的纪律**。
> 所有数值都在本文件所基于的 HEAD 上重跑过；引用时请带 HEAD。

---

## 0. 已实测基线（在 `6866700` 上重跑，可直接引用）

| 项 | 实测值 | 命令 |
|---|---|---|
| Rust 测试 | 732 passed / 0 failed | `cargo test --workspace --all-features` |
| clippy | **exit 0**（唯一的结构性红灯已灭） | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| 覆盖率门禁 | exit 0（≥90% 行，排除 `application/testing`） | `node scripts/verify/checks/task-12.8.mjs` |
| 前端测试 | 177 passed / 26 files | `pnpm --dir apps/desktop test` |
| 前端 lint | 0 error / 6 warning | `pnpm --dir apps/desktop lint` |
| 前端 format | 全过（本轮才修好，见 §2） | `pnpm --dir apps/desktop format:check` |
| 规模豁免 | 7 文件 / 4 trait（棘轮，只能减） | `node scripts/verify/check-scale.mjs` |
| 场景对账 | spec 218 = trace 218 = manifest 218 | `node scripts/verify/reconcile-scenarios.mjs` |
| 场景命令重复度 | 218 场景 / 124 命令（**1.76x**），最差簇 16 | `node scripts/verify/check-scenario-churn.mjs` |
| 已登记检查数 | **86** | `node scripts/verify/check-verification-validity.mjs` |
| 注入证明 | **22/22 通过，工作树逐字节还原** | `node scripts/verify/injection-suite.mjs` |

---

## 1. 先跑通这些命令（照抄，不要凭印象）

```bash
# 前端（format:check 不能漏，它曾经红着没人知道）
cd apps/desktop && pnpm format:check && pnpm typecheck && pnpm lint && pnpm test --run && pnpm build

# Rust
cargo fmt --all --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 治理门禁（聚合入口，等价于 CI 的 governance job，含 14.1–14.9）
pnpm verify:governance

# 注入证明（22 条；单独跑更快，其中 3 条会真跑 cargo/pnpm 做正向对照）
node scripts/verify/injection-suite.mjs

# 构建纯净性（含产物级证明，会真跑一次 cargo check）
ECHO_PURITY_REQUIRE_BUILD=1 node scripts/verify/check-build-purity.mjs

# 嵌入前端新鲜度（二进制不存在时打印 SKIP，不静默通过）
node scripts/verify/checks/check-embedded-frontend.mjs

# OpenSpec（CLI 是根 devDependency，走 pnpm exec；CI 同一条命令）
# 该 change 已归档，归档子项用 --archived 校验（未归档时才用 <name> --strict 定向校验）
pnpm exec openspec validate --archived
```

注意：`cargo` 子命令的参数分隔符 `--` 不能省；`src-tauri` 的包名是 **`echo-app`**（不是 `echo-desktop`），
所以要单独查它用 `cargo check -p echo-app`。

---

## 2. 已关闭：clippy 存量告警（任务 2.3 / 8.1）

**已完成。** `cargo clippy --workspace --all-targets --all-features -- -D warnings` 退出码 0。

74 条唯一告警分四组清掉（每组一个提交 + 一次全量回归，提交链 `14e1cf4` → `a8c3c82` → `3a93460` → `70ce9cb`）：
机械类 17 / 风格类 23 / 语义类 24 / 文档类 10。

两处值得记住：

- **语义组有一处是真缺陷，不是误报。** `runtime/player/metadata.rs` 把 metadata 缓存锁跨两次仓库读
  （`songs.by_id` + `covers.cover_of`）持有，等于把每个未命中 id 的两次 I/O 串行化在一次队列变更后面。
  已改为按插入取锁（插入幂等，收窄无损失）。这类 lint **不要批量 `#[allow]`**。
- **清零时踩到两个门禁打架。** 清 `redundant_pub_crate` 把 private module 里的 `pub(crate)` 翻成 `pub`，
  顺带把 `LegacyLibraryFileSystem`（17 方法）变成了公开 trait，新触发规模门禁（公开 trait ≤6 方法）。
  豁免清单**不能**吸收它（清单与不可增的 BASELINE 逐字比对，扩容本身就是失败）。
  该 trait 本就是"适配器完整性内部契约"，正解是解回 `pub(crate)` 并就地写明原因（提交 `44fe603`）。
  **教训：`pub(crate)` → `pub` 不是纯机械改动，它会改变其它门禁的输入。**

---

## 3. 已关闭：规模超标文件（任务 5.5 / 5.8）

`scripts/verify/check-scale.mjs` 是棘轮门禁：豁免清单 `scripts/verify/scale-allowlist.json`
**只能减不能增**，且与 `check-scale.mjs` 里的 `BASELINE` 常量逐字比对。当前豁免 **7 文件 / 4 trait**。

本轮从清单摘掉 4 个文件（`coordinator.rs`、`local_state.rs`、`queue.rs`、`testing/memory_database.rs`），
此前一段已摘掉 `runtime/services.rs`、`runtime/player.rs` 等。

**拆分手法（两种，按"超限在哪"选）**：

1. **超限来自内联测试** → 把 `#[cfg(test)] mod tests { … }` 整块搬到 `<module>/tests.rs`。
   `coordinator.rs` 1377→663、`local_state.rs` 1131→671、`queue.rs` 1114→756 都是这一类，
   `super` 仍解析到被测模块，零可见性改动。
2. **超限来自生产代码** → 按 port/能力分组拆成 `<module>/` 子目录 + 根部薄壳。
   `memory_database.rs` 1028→192 是这一类（只搬测试只能到 986 行，必须真拆）。
   子模块用 `use super::*` 是**该模块既有政策**：`application/testing.rs` 对整个 testkit
   用带理由的 `#![allow(pedantic/nursery)]` 明确豁免（脚手架，非出货业务代码），不是被压掉的告警。

**判据**：文件 ≤1000 行后从 `scale-allowlist.json` **和** `check-scale.mjs` 的 `BASELINE` 里同时删条目，
`check-scale.mjs` 仍退出 0。删条目后报红 → 说明没真降下来，**不许把条目加回去**。

**仍在豁免清单里的 7 个文件**（都是存量，尚未逐个复核是否真的拆不动）：
`application/recover.rs` 2548、`application/scan.rs` 1819、`domain/library.rs` 1311、
`infrastructure/filesystem/adapter.rs` 1241、`infrastructure/sqlite/mod.rs` 1150、`domain/entities.rs` 1054、
`player/actor.rs` 2675。

---

## 4. 已关闭：检查有效性证明（任务 7.6）

`check-verification-validity.mjs` 证明的是**静态**属性（每个检查有失败路径与观测点）。
"有 `process.exit(1)`"与"该退出码挂在它声称守的条件上"是两件事。

**`scripts/verify/injection-suite.mjs`（manifest `14.8`，已接 `verify:governance` 与 CI）** 补上后者：
真注入违规 → 跑门禁 → 断言**失败原因就是声明的那条** → 还原工作树。
三种注入方式，按侵入性从低到高：

- **fixture**：给门禁一个它本来就接受的可选根（`ECHO_SCALE_ROOT`、`ECHO_LINT_CHECK_ROOT`），**一个仓库文件都不碰**；
- **argument**：用参数点名一个违规（`--bin <垃圾文件>`、一个没有举证的场景 id）；
- **mutate**：改一个仓库文件，跑完还原。每个目标先做内存快照 + 磁盘备份，`finally` 还原，**按哈希校验**，
  并额外逐路径比对前后 git 状态。

**22 条证明全绿**；每条还断言门禁在**未污染的工作树上是通过的**（正向对照），所以"恒红"的门禁混不进来。
三条无法在任意机器上为绿的门禁（dev 构建的二进制、本就"尚无举证"的真机行）显式标注 `baseline: false`
并附一个明确的通过用例。

**完整性是机械的，不是文档承诺**：套件自己读 manifest，把 86 个已登记检查分两类——
**自包含类**（断言在检查内部）**7 个，全部有注入证明**，新增一个没有证明的会让套件退出 1；
**委托类**（断言在它调用的子命令里）**79 个**，其中 4 个（`toolchain` / `build-purity` / `task-2.1` / `task-1.2`）
另有动态证明，合计 **11 个有动态证明**；其余 **75 个**由结构规则覆盖（套件每次运行都会打印这个数）。
对委托类注入一个失败的"测试"只能证明子命令，真正会静默通过的是**忽略子命令退出码**，
因此证据是结构性的：`check-verification-validity.mjs` 新增规则"凡调用子进程的检查必须比较 `.status`"，
79 个全部成立，并由 `delegation/*` 三条样例实跑演示（cargo、pnpm、runner）。

**要加一条新证明**：在 `ENTRIES` 里加一条 `{ id, guard, check, expect, baseline, inject }`，
`expect` 必须写清**失败原因的原文片段**，不要只写 `process.exit(1)`。

---

## 5. 场景命令重复治理的完整形态（任务 7.7，未完成）

已建棘轮门禁 `check-scenario-churn.mjs`（`14.7`）：重复度 ≤1.8x、单命令 ≤16 个场景，**只能降不能升**。
实测 218 场景 / 124 命令 = **1.76x**、最差簇 16。

**还没做**：完整的"按模块聚合并保留到测试名追溯"。这需要改写 **218 条验收行**，风险高
（会同时牵动 `tests/*.yaml`、`gen-scenario-manifests.mjs`、`docs/traceability.md`、以及
`reconcile-scenarios.mjs` 的对账），**建议单开一个 change**，不要塞进这一个。

**先决条件**：动它之前先读 `scripts/verify/reconcile-scenarios.mjs:86` —— 它对每个登记路径做
**非空断言**。曾有人（包括我）把"136 个 YAML 无人读取"误读成"可以删"，实际删了会让门禁
从"报红"变成"输入缺失"而崩坏。**删任何场景 YAML 之前先确认它在对账表里。**

---

## 6. 场景全量 / 真机 / 归档（任务 8.3 / 8.4 / 8.5，未完成）

**8.3 已完成的部分**（在 `e7bf5b7` 上实测）：
- `pnpm verify:self-test` → 10 条断言全过，退出码 0；
- 三方对账 `spec 218 = trace 218 = manifest 218`，退出码 0；
- `pnpm exec openspec validate --archived` → `7 passed, 0 failed`，**退出码 0**。

**8.3 未完成的部分**：`pnpm verify:scenario -- --all` 实测 **218 个场景里 175 过 / 43 红，退出码 1**。
43 红分两类，性质完全不同：

| 类 | 数量 | 形态 | 结论 |
|---|---|---|---|
| 缺人工举证 | **41** | 命令是 `check-native-attestation.mjs <ID>`，缺 `artifacts/native-attestations/<ID>.log` | 未完成的工作，需操作者在真机执行 |
| 真缺陷 | **1** | `LE-R05-S04` → `task-9.5.mjs` 读 `runtime/services.rs`（已被拆成 `runtime/services/`）→ ENOENT 崩 | **本轮已修**（改读 `services/reveal.rs`） |
| 真缺陷 | **1** | `LE-R07-S01` → 命令漏了 `-- --ignored`，`#[ignore]` 的 50k bench 跑了 **0 个测试** | **本轮已修**（manifest 加 `-- --ignored`） |

复现：
```bash
pnpm verify:scenario -- --all          # 退出码 1
node scripts/verify/run-scenario.mjs LE-R05-S04   # 修后 ok
node scripts/verify/run-scenario.mjs LE-R07-S01   # 修后 ok
```
另注：运行器的失败行前缀是 **`error:`**，不是 `FAIL`。用 `grep '^FAIL'` 统计会得到 0，
从而误判成"零失败"——这轮真的这样误判过一次。

**⚠️ 我在这件事上连续给过两次相反的错误结论，错法都记在这里**：

1. 第一版文档写"44 个 native 场景行（`tests/native/*.md`）的命令被映射成
   `check-native-attestation.mjs`"——**数字和对象都错**：真正走 attestation 的是 **41 个**场景，
   且它们**没有一个**在 `tests/native/` 下有 `.md`。
2. 本轮我先"订正"成"**0 行走 attestation，不需要任何 log**"——**这才是全错的**。
   错因：拿 `tests/native/*.md` 的**文件名**当集合去筛 manifest（44 个），得出 0，
   再把 0 当成"不存在"。真相是那 44 个 `.md` 对应的是**另一批**场景（它们全部有自动化命令、
   且**没有** `tests/scenarios/*.yaml`）；attestation 那 41 个场景各有 yaml、却不在 `tests/native/` 里。
3. 同一个病还犯了第二次：用 `grep '^FAIL'` 统计失败 → 0 → 报"零失败"（前缀其实是 `error:`）。

**教训（可直接复用）**：算任何"有多少个 X"之前，先确认**集合本身是同一个集合**。
`tests/native/*.md` ↔ manifest 场景是**两套命名空间**，用文件名去 join 会静默得到 0，
而 0 看起来和"没有"一模一样。**先打印分母，再相信分子。**

**41 个 attestation 场景的登记是自相矛盾的**（这是 8.4 真正要处理的东西）：
它们的 `tests/scenarios/<ID>.yaml` 里写着 `layer: automated` 与
`automated_filter: the command above`，而同一行的 `command` 却是 `check-native-attestation.mjs`。
41 个全部如此（实测 `layer` 分布 `{automated: 41}`）。看标题（关闭窗口后继续后台播放、托盘控制、
历史跨重启…）这些**确实是** headless 无法自动化的 OS 级行为，所以更像
**生成器把 layer 统一写成 `automated`**，而不是"本该自动化却漏了命令"。
无论哪种解释，都需要先定案再动 —— 不要直接补 41 份 attestation 把红刷绿。

**8.4 真机冒烟**：8.4 列举的七类行为在 manifest 里都有对应场景（播放/暂停 24、切歌 4、
队列顺序与随机/单曲循环 16、退出后恢复 12、歌单播放上下文 6、资料库"最近"视图 2、控制失败反馈 13），
但其中若干条正落在上面那 41 个 attestation 场景里（如 `DP-R02-S05` 连续下一首播放、
`DP-R04-S04` 单曲循环、`DP-R08-S03` 恢复、`PHA-R01-S01` 歌单与队列日常流程）。
**所以 8.4 不是纯自动化可收口的**：它就是要产出这些人工举证。
"真机"那一半里可自动化的部分由 `cargo test -p echo-desktop --test player_smoke`
（真 vendored libmpv）承担。


**8.5 归档**：依赖上面全部完成。`openspec archive` 前务必跑 `openspec validate --archived`
（它会检查未勾选任务并**退出码 1**）。

**⚠️ 归档流程的两个静默坑**（本轮踩过）：
- 新增能力 spec 必须带 `## Purpose`（≥50 字符），否则归档后主 spec 留 `TBD` 占位。
- Scenario 必须**恰好 4 个 `#`**（`#### Scenario:`）。写成 3 个会**静默失败**，不报错。

---

## 7. 门禁全集（14.1–14.9，都已接电）

登记在 `scripts/verify/manifest.json`，接入 `pnpm verify:governance`（`scripts/verify/ci-governance.mjs`）
与 CI 的 `governance` job：

| 任务 | 脚本 | 它证明什么 |
|---|---|---|
| 14.1 | `check-lint-inheritance.mjs` | 每个 crate 只用 `lints.workspace = true`，无 crate 级覆盖 |
| 14.2 | `check-scale.mjs` + `scale-allowlist.json` | ≤1000 行 / 公开 trait ≤6 方法；清单只减不增 |
| 14.3 | `check-toolchain.mjs` | 生效 rustc 与 `rust-toolchain.toml` 完全一致 |
| 14.4 | `validate-scenario-manifests.mjs` | 每个 YAML 字段与登记表一致 |
| 14.5 | `check-verification-validity.mjs` | 失败路径 + 观测点；**调用子进程必须比较退出码** |
| 14.6 | `check-build-purity.mjs` | 测试专用模块 cfg 门控 + 默认构建的 `.d` 里不出现 |
| 14.7 | `check-scenario-churn.mjs` | 场景命令重复度与最差簇只降不升 |
| 14.8 | `injection-suite.mjs` | 22 条"注入违规即失败"证明 + 自包含检查必须带证明 |
| 14.9 | `checks/check-embedded-frontend.mjs` | 二进制里嵌的是当前 `dist`（此前只有手工入口） |

CI 的 governance job 在跑 `pnpm verify:governance` 之前会先 `pnpm --dir apps/desktop build`：
14.8 里"嵌入前端"那条证明需要一个可比的 `dist`，否则会退化成具名 skip。
`ci-governance.mjs` 设 `ECHO_INJECTION_REQUIRE_ALL=1`，把那种 skip 变成失败
（与 `check-build-purity.mjs` 的 `ECHO_PURITY_REQUIRE_BUILD=1` 同一契约）。

**教训值得重复一遍**：14.1–14.5 这些检查在本段之前**已经存在，但不在 `manifest.json` 里**，
所以从来没人执行过；本轮又发现两个同类——`pnpm --dir apps/desktop format:check` 早已红灯
（导致 `task-1.2` 是红的而没人知道），`check-embedded-frontend.mjs` 只有手工入口。
排查门禁类问题，**先问"这个检查有没有被执行"，再考虑写新检查**。新写但不接电 = 又一个死门禁。

---

## 8. 施工纪律（用血换的，别重蹈）

1. **动手前先备份工作树**。`tar czf /tmp/echo-wip-*.tar.gz` 这个动作救过场
   （一个子代理留下 525 个编译错误的半成品，靠它整份回滚）。
2. **高峰期不要把事摊给多个 sub-agent**。并行子代理共享同一速率池，会同时撞限流集体阵亡，
   留下半吊子工作树。
3. **"统计为 0" 极易被误读成"代码里没有"**。`grep -E '^\s+pub fn'` 在 BSD grep 下恒 0 命中
   （不支持 `\s`）。要换 `[[:space:]]` 或换工具复核。同族坑：`\|`、`\b`。
4. **结论会随 HEAD 漂移**。写进 spec 的每个数值都要能回答"在哪个 HEAD、用什么命令重跑出来"。
   一条定稿方案被独立复核纠正过 8 处，其中 3 处会导致错误施工。
5. **不要为了让门禁变绿而放宽门禁**。拆测试文件时架构守卫真抓到一条存量越界，
   正确做法是改目录让它合规。
6. **覆盖率数字在大重构后不可信，要先清插桩缓存**。`cargo llvm-cov` 会复用拆分前的 instrumented
   产物，把已缩到 8 行的 `application/import.rs` 仍按旧的 603 行统计，报出 **73.48%** 的假红；
   `cargo llvm-cov clean --workspace` 后同一命令得到 **91.77%**。看到覆盖率突然掉十几个点，
   **先 `clean` 再下结论**。CI 是干净 runner，不受影响。
7. **写"注入断言"时，替换串不能仍然包含被查的子串。** 把 `tauri-plugin-dialog` 改成
   `tauri-plugin-dialog-DISABLED`，对人是"删掉了"，对 `String.includes` 是**什么都没改**——
   门禁保持绿色，而证明会以"注入无效"的样子失败。要改成一个不含原串的形态（`…dial0g`）。
8. **改共享声明值时，`replace` 的第一个命中通常是别的规则。** `app-extras.css` 里
   `grid-area: workspace` 先出现在另一个选择器上，全局替换第一处等于没碰目标块。
   注入要**限定在目标块内**（`replaceWithinBlock`）。
9. **断言"失败原因"时先确认分隔符**。`check-scale` 打的是 `FAIL scale gate:\n- …`（冒号后是换行），
   写 `/FAIL scale gate: [\s\S]*/` 会永远不匹配——而"期望失败"的断言不匹配时表现为
   **"门禁失败了但原因不对"**，很容易被读成门禁有问题。本轮 5 条期望值就是这么修出来的。
10. **本机可能出现的假红：agent 运行时的批量删除守卫。** 在把 `verify:governance` 整条跑在一轮里时，
    `check-build-purity.mjs:94` 的 `rmSync` 可能抛
    `[safe-delete][SAFE_DELETE_BULK_CONFIRM_REQUIRED]`（`count` 累计超阈值 50、`scope=turn`）。
    **单独跑该门禁是 exit 0**，CI 的干净 runner 也没有这个守卫。
    辨认方法：报错里出现 `node-safe-delete-shim.cjs` 就是它，别去改仓库。
