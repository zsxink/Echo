# Echo CI 流程分析与优化研究报告

- 分析对象：`.github/workflows/ci.yml`（391 行，8 个 job）、`.github/workflows/release.yml`（241 行，2 个 job）
- 分析日期：2026-10-28
- 数据来源：GitHub Actions 真实运行记录（`gh api`），主力样本 `run 40169490729`（2026-10-25 成功）、`40153509669`、`40149996475`，失败样本 `40160828943` / `40158689677` / `40150717461`，Release 样本 `39537049148`
- 性质：**只读分析 + 研究报告**，未修改任何代码或 workflow

---

## 0. 结论速览（TL;DR）

1. **单次 CI 墙钟 ≈ 7.5 分钟，机器时间 ≈ 44 分钟/次**（12 个 job 并行）。墙钟被两个「重型门禁」卡住：`Gate injection proofs`（450s）和 `Release gate`（413s）。
2. **本仓库是 public repo，标准托管 runner 的分钟数免费**。因此优化目标不应是「省账单」，而应是三条：**① 缩短反馈时间 ② 提高可靠性 ③ 降低维护面**。唯一可能的例外是 macOS 的 `xcode-27` runner（见 §9，需确认是否 larger runner，larger runner 即使 public 也计费）。
3. **重复度很高**：`pnpm install` 跑 8 次、`frontend build` 跑 5 次、`cargo test --workspace` 跑 4 次（+llvm-cov 再跑 1 次核心测试）、Linux webkit `apt` 装 4 次、rust-cache 恢复 7 次。每次 setup 开销约 25–35s。
4. **低风险可立即删/并**：Linux platform Gate 实为空转（27s，校验步 1s）；Windows platform Gate 里那条 native 测试是 Rust(Windows) 已覆盖的子集；Linux apt 步骤可合并。
5. **墙钟加速的最大杠杆**：把注入证明（当前分支 33 条 / origin/main 34 条）拆成 2–3 个并行 job（套件已支持 `--only` 过滤），再把 `Rust (Windows)` 的 271s 测试用 nextest / 分片压下去；两项都做完墙钟可到 ~5 分钟。
6. **最大隐性成本是「CI 形状被门禁锁定」**：`task-1.5`、`task-crp`、`injection-governance` 等脚本会解析 `ci.yml` / `release.yml` 并断言 job 名、三平台矩阵、精确命令字符串；**任何「删 job / 并矩阵 / 改命令」都必须同步改这些门禁并重跑 `pnpm verify:governance`**，否则 CI 自己会红（见 §8）。

---

## 1. 现状盘点

### 1.1 `ci.yml` 的 8 个 job（真实耗时，run 40169490729）

| Job | Runner | 墙钟 | 最耗时步骤（真实秒数） | 作用 |
|---|---|---|---|---|
| Rust (Linux) | ubuntu-latest | 207s | Test 75s · apt 44s · Cache 23s | fmt + clippy + `cargo test --workspace` |
| Rust (macOS) | xcode-27 | 278s | Test 161s · Cache 29s · Clippy 30s | 同上 |
| Rust (Windows) | windows-latest | 425s | **Test 271s** · Clippy 43s · Cache 34s · Checkout 22s | 同上（含 LFS 114MB dll） |
| Frontend (Linux) | ubuntu-latest | 43s | Test 17s | format/lint/typecheck/test/build |
| Frontend (macOS) | xcode-27 | 71s | Test 28s | 同上 |
| Frontend (Windows) | windows-latest | 107s | Test 43s · Lint 9s | 同上 |
| macOS platform Gate | xcode-27 | 261s | **verify:task 1.9/1.10/9.7 → tauri build 200s** | 真实打包 .app + libmpv/签名/文件关联 |
| Windows platform Gate | windows-latest | 133s | native 测试 43s · verify 8s | 平台专属 dll/SHA256/加载探针 |
| Linux platform Gate | ubuntu-latest | **27s** | **verify 仅 1s（空转）** | 9.7/9.8 在 Linux 上目前是「pending」哨兵 |
| Governance gates | ubuntu-latest | 232s | verify:governance 144s · apt 32s · Cache 31s | 14 项治理门禁 + 覆盖率 + 前端嵌入新鲜度 |
| Gate injection proofs | ubuntu-latest | **450s** | **注入套件 339s** · cargo build 24s · Cache 25s | 「注入违规即失败」证明 34 条（14.8；当前分支 33 条） |
| Release gate | ubuntu-latest | 413s | **task-13.9 场景 330s** · apt 26s | spec↔scenario 对账 + PRD A1–A14 |

**墙钟**：06:33:12 → 06:40:42 = **450s（7.5 分钟）**；**机器时间合计** ≈ **2647s（44.1 分钟）**。

> 说明：本 checkout 位于 `codex/issue-72-library-first-durability` 分支（HEAD `900187d`），其 `ci.yml` 与 `origin/main`（`eee3d3e`）唯一差异是：origin/main 的 Windows platform Gate 多跑 `17.1 18.1` 两个静态契约门禁（issue #64/#66，`Verify platform Gate (9.7 9.8 17.1 18.1)`，多几秒）。下文按当前工作树分析；该差异不影响任何结论。耗时数据取自 origin/main 上的真实运行，与当前分支的 job 结构一致。

### 1.2 `release.yml`（tag 触发，非常规 PR 路径）

| Job | Runner | 墙钟 | 说明 |
|---|---|---|---|
| Build (Linux) | ubuntu-latest | 628s | 含 22.04 容器内自建 libmpv |
| Build (Windows) | windows-latest | **847s（14min，关键路径）** | LFS + tauri + NSIS |
| Build (macOS) | xcode-27 | 558s | dmg 打包 |
| Publish | ubuntu-latest | 21s | 聚合产物 + Release |

Release 整体约 14.6 分钟，只在打 tag 时跑，不属于日常反馈路径，优先级低。

---

## 2. 关键基线：重复度矩阵

| 重复的动作 | ci.yml 中出现次数 | 说明 |
|---|---|---|
| `actions/checkout@v4` | 8 | 每 job 一次（不可避免） |
| `actions/setup-node@v4` | 8 | 可抽取 composite |
| `pnpm/action-setup@v4` | 8 | 可抽取 composite |
| `pnpm install --frozen-lockfile` | 8 | 4–11s/次，可用 pnpm store 缓存 |
| `Swatinem/rust-cache@v2` | 7 | 恢复 23–34s/次 |
| `dtolnay/rust-toolchain` | 7 | 7–13s/次 |
| `pnpm --dir apps/desktop build` | **5** | 同一份 `dist` 被重复构建 |
| Linux webkit `apt-get install` | **4** | 26–44s/次 |
| `cargo test --workspace` | **4**（Rust×3 + Governance） | + llvm-cov 再跑 1 次核心测试 = 核心测试实际跑 5 遍 |
| `cargo build -p echo-app` | 2（injection / 由场景间接） | |

**要点**：12 个 job 里有 5 个是 Linux，各自重复「checkout + rust + node + pnpm + apt + 前端 build」。这不是靠「删一个 job」能解决的，而是需要**可复用 workflow / composite action + 共享缓存**。

---

## 3. 可以删除（低风险，先做）

| # | 目标 | 证据 | 处理 |
|---|---|---|---|
| D1 | **Linux platform Gate 独立 job** | 整个 job 27s，其中 `verify:task 9.7 9.8` 仅 **1s**——Linux 侧 libmpv 仍是 `pending` 哨兵，没有真实断言 | 把这一条 `verify:task -- 9.7 9.8` **并入 Rust (Linux)** job；删掉独立 job |
| D2 | **Windows platform Gate 的 native 回归测试步** | `cargo test -p echo-desktop --lib native_confirmation_tests::` 是 `cargo test --workspace`（Rust(Windows) 已跑）的**子集**；`native_confirmation_tests` 是 echo-desktop lib 内的 `mod`，workspace 测试必然执行 | 删除该步（43s）；保留 `verify:task 9.7 9.8`（那是文件级/dll 级探针，别处不覆盖） |
| D3 | **重复的 `cargo install cargo-llvm-cov --locked`** | 观测中该步已 0s（缓存/预装），但冷机从源码编译需数分钟 | 换成 `taiki-e/install-action`（ci.yml 注释里已承认「会更快」）或依赖 runner 预装 |

> 注意：D1/D2 会改变 job 结构，务必先读 §8——`task-1.5` 等门禁会检查矩阵，删除后需同步调整。

---

## 4. 可以简化

| # | 目标 | 收益 | 代价/风险 |
|---|---|---|---|
| S1 | **抽取 composite action / 可复用 workflow**，收敛「checkout→node→pnpm→install→(rust+apt)」样板（8 份） | 维护面显著下降；改一处同步全仓 | 需改 8 个 job 结构，并核对门禁字符串 |
| S2 | **Frontend 矩阵 3 平台 → 1 平台（Linux）** | 无任何前端测试引用 `process.platform/win32/darwin`（50 个测试文件，0 命中），三平台跑的是同一套 JS 检查；省 2 个 runner、约 178s 机器时间 | `task-1.5` **强制** frontend 三平台矩阵，必须改门禁并写理由 |
| S3 | **Governance 内去重**：`cargo test --workspace --all-targets` 与 `cargo llvm-cov -p echo-core` 都跑 echo-core 测试（核心测试在治理 job 内就跑了 2 遍） | 减少 1 次核心测试编译+运行 | `--all-targets` 覆盖 bench/example 目标，需保留这部分或挪去 Rust 矩阵 |
| S4 | **release.yml 两个 Linux `apt-get` 合并** | 少一次 apt update | 低 |
| S5 | **把 `--all-targets` 交给 Rust 矩阵腿**，governance 只保留 `--all-features` 的核心测试 | 消除跨 job 语义重叠 | 需确认矩阵腿用 debug target 能覆盖 |

---

## 5. 可以加速（压缩墙钟，优先级最高）

当前墙钟 = `max(各 job)` = **Gate injection proofs 450s**。加速必须打在「关键路径 job」上。

| # | 目标 | 现状 | 做法 | 预期 |
|---|---|---|---|---|
| A1 | **拆分注入证明 job** | 单 job 450s，其中套件 339s、33–34 条 entry | 脚本已支持 `--only <substr>`；新增 `--shard i/n`，在 2–3 个独立 checkout 的 job 里并行（注入条目会改仓库文件，必须各自独立工作区）；静态「完整性」元断言每次照跑即可 | 450s → **~250s** |
| A2 | **Rust (Windows) 测试 271s** | 单一 `cargo test --workspace` | `cargo nextest` 的进程级并行；或把 workspace 测试按 crate（core/desktop/app）拆 2 个 job | 271s → **~150s** 量级 |
| A3 | **Release gate 场景 330s** | `run-scenario --automated`，去重后 ~147 条命令，`ECHO_VERIFY_TEST_CONCURRENCY` 默认 2 | 提高并发到 4–6；或按 scenario id 段拆成 2 个 job | 330s → **~180s** |
| A4 | **Tauri / verify build 编译** | macOS gate 200s 的 tauri build 是打包必需 | 保留；可与 Rust(macOS) 共享 rust-cache 的 release profile | 小幅 |
| A5 | **apt webkit 4 次 26–44s** | 每次 `apt-get update && install` | 用带缓存的 apt 镜像或预置容器；最少把 4 次合并到共享 setup | 每次省 20–40s |

> A1+A2+A3 全部落地后，墙钟瓶颈从 450s 降到约 **250–300s（4–5 分钟）**；机器时间从 44min 降到 ~32min。

---

## 6. 可以缓存

| # | 缓存对象 | 现状 | 建议 |
|---|---|---|---|
| C1 | **Rust target** | 已用 `Swatinem/rust-cache`，恢复 23–34s/次 | 加 `shared-key` 让相关 job 复用同一主键；`save-if` 只在主干保存，避免 PR 反复写 |
| C2 | **pnpm store / node_modules** | `pnpm install` 4–11s/次 | `actions/cache` 缓存 `~/.local/share/pnpm/store`，或 `pnpm/action-setup` 的 store 缓存 |
| C3 | **apt webkit 依赖** | 4 个 job 各装 26–44s | 缓存 `/var/cache/apt/archives`，或改用预装镜像 |
| C4 | **cargo-llvm-cov 二进制** | 冷机需从源码编译 | `taiki-e/install-action`（自带缓存） |
| C5 | **前端 `dist`** | 5 个 job 各 build 一次 | 由一个 job 构建后 `upload-artifact`，其余 `download-artifact`（注意 `check-embedded-frontend` 需要一个「可比」的 dist） |
| C6 | **Git-LFS 对象（114MB dll）** | Windows checkout 18–22s | 缓存 LFS 对象，减少重复拉取 |

---

## 7. 可以拆分 / 可以合并

### 可以拆分
- **注入证明 → 2–3 个 shard**（A1，最大收益）。
- **Release gate 场景 → 2 个 shard**（A3）。
- **Rust workspace 测试 → 按 crate 分片**（A2 的备选）。
- 前端 lint/typecheck/test 可拆并行，但当前 43–107s 不是瓶颈，**低优先**。

### 可以合并
| # | 合并方案 | 结论 |
|---|---|---|
| M1 | Windows platform Gate → **Rust (Windows)** 同一 job | ✅ 推荐。同一 runner、同一 rust-cache，省一个 job 的 setup + 一次重复编译 |
| M2 | Linux platform Gate → **Rust (Linux)** | ✅ 推荐（同 D1） |
| M3 | **Governance + Gate injection proofs 合并** | ❌ **不建议**。二者都在关键路径上，合并会把单 job 拉到 ~11min，墙钟反而变差。应改为**共享 Linux 构建 + 缓存**，而不是合并 |
| M4 | 8 份 setup 样板 → **composite action** | ✅ 推荐（S1），维护收益最大 |
| M5 | macOS platform Gate 保留独立 | ✅ 它是真实打包校验（200s tauri build），不能并入 debug 测试 job |

---

## 8. 硬约束：CI 的形状被「治理门禁」锁定（本仓库最大隐性成本）

这个仓库用「脚本反向断言 CI 文件内容」来保证质量纪律，**改 CI 前必须先看它们**：

| 门禁脚本 | 锁死什么 |
|---|---|
| `scripts/verify/checks/task-1.5.mjs` | 断言 `ci.yml` 的 `rust`/`frontend` job 必须是 **include-only 三平台矩阵**、`runs-on: ${{ matrix.os }}`、**精确 job 名**（`Rust (${{ matrix.platform }})` 等），并逐条匹配 9 个精确命令字符串；还会在本机实跑一遍 |
| `scripts/verify/checks/task-crp.mjs` | 断言 `release.yml` 的三平台矩阵、`Assert expected artifacts exist`、SHA256SUMS、NOTICE、publish 聚合等字符串/结构 |
| `scripts/verify/injection-governance.mjs` | 注入证明：删掉 `ci.yml` 的 Linux 矩阵项必须让 `task-1.5` 报 `ci.yml rust matrix is missing platform 'Linux'` |
| `scripts/verify/workflow-matrix.test.mjs` | 单测 `parseWorkflow` / `assertPlatformMatrix` 的解析契约 |
| `scripts/verify/check-scale.mjs` | 单文件 ≤1000 行（`injection-suite.mjs` 已逼近上限，注释说明拆文件是为了卡这个） |

**结论**：任何「删 job / 并矩阵 / 改命令文本」的方案，都是**改 workflow + 改门禁 + 重跑 `pnpm verify:governance`（含注入证明）**三件套。这不是坏事——它保证 CI 不会悄悄退化——但必须把成本算进去。**建议把门禁里的「精确字符串匹配」升级为「解析 YAML 后的结构断言」**（`workflow-matrix.mjs` 已经这么做了），这样合并 job 时不必大改字符串。

---

## 9. 风险与「不建议做」清单

- ⚠️ **`xcode-27` 归属需确认**：它是非标准 GitHub 托管 label。若为 **larger runner / 自托管**，即使 public repo 也计费——那么 Rust(macOS)+macOS gate+Frontend(macOS) 三个 job 的合并就有真实成本收益；若是普通托管则不用管。
- ❌ 不要合并 Governance 与 Gate injection proofs（M3）。
- ❌ 不要删除 macOS platform Gate（真实打包/lipv/签名校验，不可替代）。
- ❌ 不要为「变绿」而放宽门禁（仓库纪律明确要求改代码而非改门禁）。
- ⚠️ **先修 flake 再谈加速**：近 12 次运行里有 3 次失败——
  - `40160828943` / `40158689677`：**Gate injection proofs 失败**，原因是「某一自包含 check 没有对应注入证明」（`FAIL injection-suite: 1 self-contained check(s) have no injected-failure proof`）。这是**新增检查必须同步补证明**的维护负担，恰好落在关键路径 job 上。
  - `40150717461`：Rust (Linux) `explicit_restore_retry_clears_exhaustion_and_restarts_the_bounded_worker` **panic**（疑似时间/竞态 flaky）。
  这两类问题会直接放大会让「加速」的收益被重跑抵消。

---

## 10. 优先级路线图

| 优先级 | 动作 | 风险 | 预期收益 |
|---|---|---|---|
| **P0（当天，低风险）** | D2 删 Windows native 重复测试；D1 Linux gate 并入 Rust(Linux)；S4 合并 apt；D3 `taiki-e/install-action` | 低（仅需微调门禁字符串） | 机器时间 −~2min；少 1 个 job |
| **P1（中风险，需改门禁）** | M1/M2 平台 Gate 并入 Rust 矩阵；S2 Frontend 3→1 平台；S1 抽 composite action | 中（`task-1.5`/`task-crp` 同步改） | 维护面 8→5–6 job；机器时间 −~4min |
| **P2（收益大，改脚本+门禁）** | A1 注入证明分片；A2 nextest/分片；A3 场景分片+提并发；C1–C6 缓存 | 中高 | **墙钟 7.5→~5min；机器时间 44→~32min** |

---

## 11. 数据来源与复现命令

```bash
# 列出最近 CI 运行
gh run list --workflow=ci.yml --limit 12

# 某次运行的 job 级耗时
gh api repos/zsxink/Echo/actions/runs/40169490729/jobs?per_page=100 \
  --jq '.jobs[] | "\(.name)\t\(.started_at)\t\(.completed_at)"'

# step 级耗时（本次报告的主要数据来源）
gh api repos/zsxink/Echo/actions/runs/40169490729/jobs?per_page=100

# 失败原因
gh run view 40160828943 --log-failed
gh run view 40150717461 --log-failed

# 复现治理门禁（会实跑 Rust/前端命令）
pnpm verify:governance
```

**关键数字**：墙钟 450s（7.5min）· 机器时间 2647s（44.1min）· 12 个 job · 8 次 pnpm install · 5 次前端 build · 4 次 `cargo test --workspace` · 注入套件 339s（当前分支 33 条 / origin/main 34 条）· Release gate 场景 330s。

---

*本报告为只读分析产物，未修改任何 workflow、脚本或源代码。*
