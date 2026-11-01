# 架构/规范治理 — 当前状态与剩余工作

> 基线：`main` 的 `c21af22`（本文件所有数字都在基于它的 issue #110 阶段 1 分支上重跑过；引用时请带 HEAD）。
> 本文件由 issue #110 阶段 1（P1-8）重写：旧版把 `6866700` 时代的数字当成"当前基线"，
> 而规模豁免、场景数、检查数、注入证明数此后全部变过，读起来像另一套仓库。旧数字一律不再保留。
> OpenSpec change：`openspec/changes/archive/2026-09-18-enforce-architecture-and-code-standards/`（已归档，47/47 已勾选）

本文只写**现在是什么样**、**还差什么**、**怎么判断做完**。

---

## 0. 当前实测基线（基于 `c21af22` 的阶段 1 分支上重跑）

| 项 | 实测值 | 命令 |
|---|---|---|
| Rust 测试 | **1716 passed / 0 failed / 8 ignored**（32 个 test target） | `cargo test --workspace --all-features` |
| clippy | **exit 0**（`-D warnings`） | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| 覆盖率门禁 | **exit 0**（≥90% 行，排除 `application/testing`） | `node scripts/verify/checks/task-12.8.mjs` |
| 前端测试 | **450 passed / 50 files** | `pnpm --dir apps/desktop test -- --run` |
| 前端 lint | **0 error / 0 warning** | `pnpm --dir apps/desktop lint` |
| 前端 format / typecheck | 全过 | `pnpm --dir apps/desktop format:check` / `typecheck` |
| 规模豁免 | **7 文件 / 4 trait**（棘轮，只能减） | `node scripts/verify/check-scale.mjs` |
| 场景对账 | **spec 523 = trace 523 = manifest 523** | `node scripts/verify/reconcile-scenarios.mjs` |
| 场景命令重复度 | **523 场景 / 338 命令 = 1.55x**，最差簇 16 | `node scripts/verify/check-scenario-churn.mjs` |
| 已登记检查数 | **97** | `node scripts/verify/check-verification-validity.mjs` |
| 注入证明 | **35/35 通过，工作树逐字节还原** | `ECHO_INJECTION_REQUIRE_ALL=1 node scripts/verify/injection-suite.mjs` |
| 验证器自检 | **10 条断言全过** | `pnpm verify:self-test` |
| manifest 任务数 | **141**（任务）/ 523（场景） | `node -e '…manifest.json…'` |

---

## 1. 先跑通这些命令（照抄，不要凭印象）

```bash
# 前端（format:check 不能漏，它曾经红着没人知道）
cd apps/desktop && pnpm format:check && pnpm typecheck && pnpm lint && pnpm test --run && pnpm build

# Rust
cargo fmt --all --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 治理门禁（聚合入口，等价于 CI 的 governance job）
pnpm verify:governance

# 注入证明（35 条；单独跑更快，其中若干条会真跑 cargo/pnpm 做正向对照）
ECHO_INJECTION_REQUIRE_ALL=1 node scripts/verify/injection-suite.mjs

# 构建纯净性（含产物级证明，会真跑一次 cargo check）
ECHO_PURITY_REQUIRE_BUILD=1 node scripts/verify/check-build-purity.mjs

# 嵌入前端新鲜度（二进制不存在时打印 SKIP，不静默通过）
node scripts/verify/checks/check-embedded-frontend.mjs

# OpenSpec（CLI 是根 devDependency，走 pnpm exec；CI 同一条命令）
pnpm exec openspec validate --archived
```

注意：`cargo` 子命令的参数分隔符 `--` 不能省；`src-tauri` 的包名是 **`echo-app`**（不是 `echo-desktop`），
所以要单独查它用 `cargo check -p echo-app`。

**治理门禁必须独占运行**：`pnpm verify:governance` 里有 5 秒墙钟断言，并行跑别的 cargo / rg / node 扫描会假红。

---

## 2. 已关闭：clippy 存量告警（任务 2.3 / 8.1）

`cargo clippy --workspace --all-targets --all-features -- -D warnings` 退出码 0。

74 条唯一告警分四组清掉（每组一个提交 + 一次全量回归，提交链 `14e1cf4` → `a8c3c82` → `3a93460` → `70ce9cb`）：
机械类 17 / 风格类 23 / 语义类 24 / 文档类 10。

两处值得记住：

- **语义组有一处是真缺陷，不是误报。** `runtime/player/metadata.rs` 把 metadata 缓存锁跨两次仓库读
  （`songs.by_id` + `covers.cover_of`）持有，等于把每个未命中 id 的两次 I/O 串行化在一次队列变更后面。
  已改为按插入取锁（插入幂等，收窄无损失）。这类 lint **不要批量 `#[allow]`**。
- **清零时踩到两个门禁打架。** 清 `redundant_pub_crate` 把 private module 里的 `pub(crate)` 翻成 `pub`，
  顺带把 `LegacyLibraryFileSystem`（17 方法）变成了公开 trait，新触发规模门禁（公开 trait ≤6 方法）。
  豁免清单**不能**吸收它（清单是只减不增的棘轮，扩容本身就是失败）。
  该 trait 本就是"适配器完整性内部契约"，正解是解回 `pub(crate)` 并就地写明原因（提交 `44fe603`）。
  **教训：`pub(crate)` → `pub` 不是纯机械改动，它会改变其它门禁的输入。**

---

## 3. 规模棘轮：两个文件，一个目的（任务 5.5 / 5.8）

`scripts/verify/check-scale.mjs` 是棘轮门禁，数据分成两份：

| 文件 | 作用 | 能不能改 |
|---|---|---|
| `scripts/verify/scale-baseline.json` | **冻结上限集**：采纳限制时哪些超限文件/宽 trait 存在 | 加条目 = 扩容 = 门禁失败 |
| `scripts/verify/scale-allowlist.json` | **当前生效豁免**：拆分完成后逐条摘除 | 只能减 |

脚本断言 `allowlist ⊆ baseline`（子集方向，不是相等）。这个子集比较就是"清单有没有变大"的**唯一判据**——
拆成两个文件的意义是：豁免清单与冻结上限不再是同一份数据的两个副本。

> 本段旧版写的是"与 `check-scale.mjs` 里的 `BASELINE` 常量逐字比对"。那个内联常量已在 issue #110 P1-3 中
> 移出为 `scale-baseline.json`，逻辑不变（仍是子集比较），只是数据不再藏在脚本里。

**当前豁免 7 文件 / 4 trait**。⚠️ **其中 5 个文件条目已经失效**——它们指向的 `.rs` 文件早已被拆成目录：

| 豁免条目（已不存在） | 现在的模块根 | 行数 |
|---|---|---|
| `application/recover.rs` | `application/recover/mod.rs` | 999 |
| `application/scan.rs` | `application/scan/mod.rs` | 732 |
| `domain/entities.rs` | `domain/entities/mod.rs` | 44 |
| `domain/library.rs` | `domain/library/mod.rs` | 307 |
| `player/actor.rs` | `player/actor/mod.rs` | 959 |

仍在豁免、且**确实超限**的只有 2 个：`infrastructure/filesystem/adapter.rs`（700 行——
注意它也不超 1000，见下）、`infrastructure/sqlite/mod.rs`（196 行）。也就是说，
**清单里 7 个文件条目中没有任何一个还在拦真实违规**：5 个路径不存在，2 个行数已低于阈值。

这不影响门禁正确性（子集比较不会因此假绿——真的超限文件会被 1000 行断言抓住），
但它是**残留卫生问题**：清单看起来还在豁免 7 个文件，实际零作用。按棘轮规则只能减，
所以可以直接把这 7 个条目全部删掉（连同 `scale-baseline.json` 的对应条目）。
**已验证**：把 `allowlist.files` 清空后 `node scripts/verify/check-scale.mjs` 仍是 exit 0
（当前仓库没有任何文件 >1000 行）。本次只记录结论、**没有改动数据文件**——
它不属于 issue #110 阶段 1 的 P1-3 范围，留作可选后续。

**拆分手法（两种，按"超限在哪"选）**：

1. **超限来自内联测试** → 把 `#[cfg(test)] mod tests { … }` 整块搬到 `<module>/tests.rs`。
   `coordinator.rs` 1377→663、`local_state.rs` 1131→671、`queue.rs` 1114→756 都是这一类，
   `super` 仍解析到被测模块，零可见性改动。
2. **超限来自生产代码** → 按 port/能力分组拆成 `<module>/` 子目录 + 根部薄壳。
   `memory_database.rs` 1028→192 是这一类（只搬测试只能到 986 行，必须真拆）。
   子模块用 `use super::*` 是**该模块既有政策**：`application/testing.rs` 对整个 testkit
   用带理由的 `#![allow(pedantic/nursery)]` 明确豁免（脚手架，非出货业务代码），不是被压掉的告警。

**判据**：文件 ≤1000 行后从 `scale-allowlist.json` **和** `scale-baseline.json` 里同时删条目，
`check-scale.mjs` 仍退出 0。删条目后报红 → 说明没真降下来，**不许把条目加回去**。

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

**35 条证明全绿**（22 条在本文件内，10 条在 `injection-governance.mjs`，3 条在 `injection-object-kinds.mjs`）；
每条还断言门禁在**未污染的工作树上是通过的**（正向对照），所以"恒红"的门禁混不进来。
少数无法在任意机器上为绿的门禁（dev 构建的二进制、本就"尚无举证"的真机行）显式标注 `baseline: false`
并附一个明确的通过用例。

**完整性是机械的，不是文档承诺**：套件自己读 manifest，把 **97** 个已登记检查分两类——
**自包含类**（断言在检查内部）：**全部有注入证明**（21 个已登记路径命中 `ENTRIES`），
新增一个没有证明的会让套件退出 1；
**委托类**（断言在它调用的子命令里）**76 个**，由结构规则覆盖：`check-verification-validity.mjs` 要求
"凡调用子进程的检查必须比较 `.status`"，并由 `delegation/*` 三条样例实跑演示（cargo、pnpm、runner）。
套件每次运行都会打印这个数（`note injection-suite: 76 delegating check(s) …`）。

**要加一条新证明**：在 `ENTRIES` 里加一条 `{ id, guard, check, expect, baseline, inject }`，
`expect` 必须写清**失败原因的原文片段**，不要只写 `process.exit(1)`。

---

## 5. 场景命令重复治理的完整形态（任务 7.7，未完成）

已建棘轮门禁 `check-scenario-churn.mjs`（`14.7`）：重复度 ≤1.8x、单命令 ≤16 个场景，**只能降不能升**。
实测 **523 场景 / 338 命令 = 1.55x**、最差簇 16（旧版记的 1.76x 已随场景扩充降到 1.55x）。

**还没做**：完整的"按模块聚合并保留到测试名追溯"。这需要改写 **523 条验收行**，风险高
（会同时牵动 `scenario-commands.mjs`、`gen-scenario-manifests.mjs`、`docs/traceability.md`、以及
`reconcile-scenarios.mjs` 的对账），**建议单开一个 change**，不要塞进别的。

**先决条件**：动它之前先读 `scripts/verify/reconcile-scenarios.mjs` 里的非空断言 —— 它对每个登记命令做
**非空断言**，且要求 `manifest.json.scenarios[]` 与 `scenario-commands.mjs` 逐条一致
（`gen-scenario-manifests.mjs` 默认模式即此漂移门）。**改任何场景命令之前先改 registry 再 `--write`。**

---

## 6. 场景全量 / 真机 / 归档（任务 8.3 / 8.4 / 8.5）

**8.3 已完成的部分**（阶段 1 分支上实测）：
- `pnpm verify:self-test` → 10 条断言全过，退出码 0；
- 三方对账 `spec 523 = trace 523 = manifest 523`，退出码 0；
- `cargo test -p echo-desktop --test player_smoke` 4 条真实 libmpv 冒烟全过（单条复跑亦为绿）。

**8.3 未完成的部分**：`node scripts/verify/run-scenario.mjs --all` 仍非全绿，红的两类性质完全不同：

| 类 | 形态 | 结论 |
|---|---|---|
| 缺人工举证 | 命令是 `check-native-attestation.mjs <ID>`，缺 `artifacts/native-attestations/<ID>.log` | 未完成的工作，需操作者在真机执行 |
| 真缺陷 | 历史上有过 `task-9.5.mjs` 读已拆分的 `runtime/services.rs`（ENOENT）、`LE-R07-S01` 漏 `-- --ignored`（跑了 0 个测试） | 均已修复；它们是"路径/参数随重构漂移"的样板，别再让同类复发 |

**当前 attestation 账本（2026-11 生成树退役后实测）**：走 `check-native-attestation.mjs` 的场景 **59** 条
（分类只由命令判定，不存在第二来源）；已落盘的举证只有 **8** 份（`DAS-R18-S01`…`S08`），**51 条缺 log**。
退役前那套「17 条有 yaml / 42 条有 `.md`、两套互斥」的计数已随生成树删除，不再适用。

复现：
```bash
node --input-type=module -e '
import { scenarioCommands } from "./scripts/verify/scenario-commands.mjs";
const rows = Object.entries(scenarioCommands()).filter(([,c]) => /check-native-attestation\.mjs/.test(c));
console.log("attestation:", rows.length);'   # → 59
node scripts/verify/run-scenario.mjs --all          # 非零即仍有红
node scripts/verify/run-scenario.mjs LE-R05-S04     # 单条：修后 ok
node scripts/verify/run-scenario.mjs LE-R07-S01     # 单条：修后 ok
```
另注：运行器的失败行前缀是 **`error:`**，不是 `FAIL`。用 `grep '^FAIL'` 统计会得到 0，
从而误判成"零失败"——这轮真的这样误判过一次。

**⚠️ 我在这件事上连续给过两次相反的错误结论，错法都记在这里**
（当时 native md 与 scenario yaml 两棵生成树尚在；2026-11 已退役，
下列计数是历史记录，现在的分类只看命令）：

1. 第一版文档写"44 个 native 场景行（每场景一份 md）"的命令被映射成
   `check-native-attestation.mjs`"——**数字和对象都错**：真正走 attestation 的场景数与
   native md 的文件数并不对应，且两套命名空间是**互斥**的（当时实测 42 vs 17）。
2. 随后"订正"成"**0 行走 attestation，不需要任何 log**"——**这才是全错的**。
   错因：拿 native md 的**文件名**当集合去筛 manifest，得出 0，
   再把 0 当成"不存在"。真相是那 42 个 md 对应的是**另一批**场景（它们全部有自动化命令、
   且**没有**对应的 scenario yaml）；attestation 的另外 17 个场景各有 yaml、却不在 native 树里。
3. 同一个病还犯了第二次：用 `grep '^FAIL'` 统计失败 → 0 → 报"零失败"（前缀其实是 `error:`）。

**教训（可直接复用）**：算任何"有多少个 X"之前，先确认**集合本身是同一个集合**。
用文件名去 join 场景清单会静默得到 0，而 0 看起来和"没有"一模一样。
**先打印分母，再相信分子。**（两棵生成树退役后，"文件名集合 vs 命令集合"这类错源消失：
分类现在只由命令判定。）

**17 个带 yaml 的 attestation 场景登记曾自相矛盾**（这是 8.4 真正要处理的东西，也是退役的直接证据）：
它们的场景 yaml 里写着 `layer: automated`，而同一行的 `command` 却是
`check-native-attestation.mjs`（当时实测这 17 条 `layer` 分布 `{automated: 17}`）。看标题
（关闭窗口后继续后台播放、托盘控制、历史跨重启…）这些**确实是** headless 无法自动化的 OS 级行为，
所以更像**生成器把 layer 统一写成 `automated`**，而不是"本该自动化却漏了命令"。
yaml 已删除，这种"自相矛盾"在生成物里不可能再出现；剩下的问题只是这 17 条要不要补举证
—— 不要靠刷 log 把红刷绿，先定案。

**8.4 真机冒烟**：8.4 列举的七类行为在 manifest 里都有对应场景，但其中若干条正落在上面那批
attestation 场景里（如 `DP-R02-S05` 连续下一首播放、`DP-R04-S04` 单曲循环、`DP-R08-S03` 恢复、
`PHA-R01-S01` 歌单与队列日常流程）。**所以 8.4 不是纯自动化可收口的**：它就是要产出这些人工举证。
"真机"那一半里可自动化的部分由 `cargo test -p echo-desktop --test player_smoke`
（真 vendored libmpv）承担，目前 4 条全绿。

**8.5 归档**：依赖上面全部完成。`openspec archive` 前务必跑 `openspec validate --archived`
（它会检查未勾选任务并**退出码 1**）。当前 `--archived` 的结果是 **52 passed / 5 failed**，
5 个失败全是**未勾选任务**（不是 spec 格式问题），见 §9。

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
| 14.2 | `check-scale.mjs` + `scale-baseline.json` / `scale-allowlist.json` | ≤1000 行 / 公开 trait ≤6 方法；清单只减不增（`allowlist ⊆ baseline`） |
| 14.3 | `check-toolchain.mjs` | 生效 rustc 与 `rust-toolchain.toml` 完全一致 |
| 14.4 | — | **已退役（2026-11）**：生成树（`tests/scenarios` 与 `tests/native`）删除后由 `gen-scenario-manifests.mjs` 的默认模式（清单 vs 注册表漂移门）接替，接线在 `ci-governance.mjs` 与 `task-13.9.mjs` |
| 14.5 | `check-verification-validity.mjs` | 失败路径 + 观测点；**调用子进程必须比较退出码** |
| 14.6 | `check-build-purity.mjs` | 测试专用模块 cfg 门控 + 默认构建的 `.d` 里不出现 |
| 14.7 | `check-scenario-churn.mjs` | 场景命令重复度与最差簇只降不升 |
| 14.8 | `injection-suite.mjs` | **35** 条"注入违规即失败"证明 + 自包含检查必须带证明 |
| 14.9 | `checks/check-embedded-frontend.mjs` | 二进制里嵌的是当前 `dist`（此前只有手工入口） |

另有 `task-1.6`（fixture 确定性 + 许可清单）在 issue #110 阶段 0 后接入 governance job
（它此前登记在 manifest 却**没接任何 CI job、也没接任何场景**，可以连续红几个月没人知道）。

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
   产物，把已缩小的模块仍按旧的数百行统计，报出假红；`cargo llvm-cov clean --workspace` 后同一命令
   恢复正常。看到覆盖率突然掉十几个点，**先 `clean` 再下结论**。CI 是干净 runner，不受影响。
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
    `check-build-purity.mjs` 的 `rmSync` 可能抛
    `[safe-delete][SAFE_DELETE_BULK_CONFIRM_REQUIRED]`（`count` 累计超阈值 50、`scope=turn`）。
    **单独跑该门禁是 exit 0**，CI 的干净 runner 也没有这个守卫。
    辨认方法：报错里出现 `node-safe-delete-shim.cjs` 就是它，别去改仓库。

---

## 9. 归档校验现状（阶段 1 分支上实测）

`pnpm exec openspec validate --archived` → **52 passed / 5 failed**。5 个失败全部是"有未勾选任务"，
不是格式问题：

| change | 未完成 |
|---|---|
| `2026-10-13-introduce-audio-effects-equalizer` | 20（42/62） |
| `2026-10-20-clarify-sync-module-ownership` | 1（14/15） |
| `2026-10-20-introduce-lan-library-sync` | 4（58/62） |
| `2026-10-24-fix-lyrics-offset-stale-measurement` | 1（19/20） |
| `2026-10-31-enforce-library-first-durability` | 11（59/70） |

归档一个 change 前，必须先把它的任务勾选或书面关闭——校验器会把未勾选任务当成失败。
（issue #110 阶段 2/3/4 的 change 归档时同样受这条约束。）
