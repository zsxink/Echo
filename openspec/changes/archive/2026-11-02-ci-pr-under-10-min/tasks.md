# Tasks

## 1. 基线与脚本契约（不改变 CI 结构）

- [x] 1.1 记录基线到 `artifacts/ci-pr-under-10-min-baseline.txt`（`artifacts/` 已被忽略）：`/usr/bin/time -p pnpm verify:governance -- --without-injection`、`/usr/bin/time -p pnpm verify:task -- --all`、`node scripts/verify/injection-suite.mjs --list | wc -l`、`gh run view 41323932057 --json jobs`；验证：`test -s artifacts/ci-pr-under-10-min-baseline.txt`，且文件中包含 governance / task / injection 三段输出与 run 的 job 列表。
- [x] 1.2 把 `scripts/verify/validate-scenario-commands.mjs` 与 `scripts/verify/check-scenario-evidence.mjs` 的 `cargo test -p <pkg> --all-features -- --list` 改为 `cargo test -p <pkg> --all-targets --all-features -- --list`；若某包的 bench/example 不支持 `--list`，退回 `--lib --tests --all-features`；验证：先执行 `cargo test --workspace --all-targets --all-features --no-run`，再分别执行两个脚本，均退出 0，且 `/usr/bin/time -p` 显示各自 < 60s（热构建应约 1–5s）。
- [x] 1.3 给 `scripts/verify/ci-governance.mjs` 的检查表增加 `section` 字段，新增 `--sections=a,b` 与 `--list-sections`；未知 section、空选择、非法语法都必须非零退出；`--without-injection` 兼容保留；验证：`node scripts/verify/ci-governance.mjs --list-sections` 输出 `quality registry rust-static static coverage purity embedded injection` 八项；`node scripts/verify/ci-governance.mjs --sections=registry` 退出 0 且只运行 registry 段；`node scripts/verify/ci-governance.mjs --sections=no-such` 非零退出。
- [x] 1.4 在 `scripts/verify/scenario-commands.mjs` 给 `SFI-R05-S03`、`LE-R21-S02`、`PM-R13-S05` 加 `delegatedTo: "perf"`，用 `node scripts/verify/gen-scenario-manifests.mjs --write` 同步 `manifest.json`，在 `package.json` 新增 `"verify:perf": "node scripts/verify/run-scenario.mjs SFI-R05-S03 LE-R21-S02 PM-R13-S05"`，并让 `check-scenario-evidence.mjs` 的 `DELEGATED_GATES` 接受 `perf` 且要求 `pnpm verify:perf` 出现在 `ci.yml`；验证：`node -e "const m=require('./scripts/verify/manifest.json');console.log(m.scenarios.filter(s=>s.delegatedTo==='perf').map(s=>s.id).join(' '))"` 输出三条 ID，`node scripts/verify/gen-scenario-manifests.mjs` 退出 0（drift 0），`node scripts/verify/check-scenario-evidence.mjs` 退出 0。
- [x] 1.5 改 `scripts/verify/run-scenario.mjs`：`--delegated` 支持逗号分隔多 gate（保留单值兼容），只跳过显式声明对应 gate 的场景；验证：`node scripts/verify/run-scenario.mjs --delegated=governance,perf LD-R10-S01 PHA-R01-S02 SFI-R05-S03` 退出 0 并显示三条 covered；`node scripts/verify/run-scenario.mjs --delegated=perf SFI-R05-S03` 显示 covered，且 `--delegated=unknown` 非零退出。
- [x] 1.6 改 `scripts/verify/checks/task-13.9.mjs`：新增 `--skip-static`，跳过 reconcile / validate / gen；`--delegated=` 继续透传；最终信息不得声称已完成静态全集校验；验证：`node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf` 退出 0，输出包含 static 已跳过的说明；不带 `--skip-static` 时仍执行静态校验。
- [x] 1.7 改 `scripts/verify/self-test.mjs`：最后一条 lockfile 不变性用例传入 `taskEnv`（fake cargo / fake pnpm 的 PATH），并为多 gate 委托增加 fixture；验证：`pnpm verify:self-test` 退出 0，`/usr/bin/time -p node scripts/verify/checks/task-1.3.mjs` 退出 0 且 < 30s，lockfile SHA-256 断言仍存在。
- [x] 1.8 在 `docs/arch-standards-remaining.md` 与 `docs/native-attestation-playbook.md` 记录新的脚本契约（`--sections` / `--list-sections`、`--skip-static`、多 gate `--delegated`、`verify:perf`）；验证：`rg -n "verify:perf|--skip-static|--list-sections|--delegated" docs/arch-standards-remaining.md docs/native-attestation-playbook.md` 命中，文档中的示例命令逐字执行退出 0。

## 2. Task gates 分片与结构门禁

- [x] 2.1 把 `.github/workflows/ci.yml` 的 `task-gates` 改为 4 个显式 shard（`core` / `desktop` / `frontend-a` / `frontend-b`），task 列表按 design D2 固定；删除 `16.1`；task-gates 不再安装 webkit、不再构建前端；12.5 的 benchmark artifact 只在 `core` 上传；更新 job 注释；验证：`node --input-type=module -e "import {readFileSync} from 'node:fs';import yaml from 'js-yaml';const w=yaml.load(readFileSync('.github/workflows/ci.yml','utf8'));const inc=w.jobs['task-gates'].strategy.matrix.include;const all=inc.flatMap(e=>e.tasks);console.log(inc.map(e=>[e.shard,e.tasks.length]));if(inc.length!==4||all.length!==42||new Set(all).size!==42||all.includes('16.1'))process.exit(1)"` 退出 0，且输出四个 shard 的 task 数 10 / 17 / 8 / 7。
- [x] 2.2 扩展 `scripts/verify/checks/task-1.5.mjs`：对 `Test`、`Rust static gates`、`Task 16.1`、`Task 9.7 9.8` 的 `run` / `if` / `env` 做步骤级断言；断言 `governance-static`、`perf`、`release-gate`、injection 两片与 task 四片的接线存在；维护固定 43 项 task 常量，断言 4 片并集为 42 项且与 Rust Linux 的 `16.1` 合起来覆盖 43 项、无重复；用 `ci-governance.mjs --list-sections` 的权威列表断言 section 全覆盖（`quality` → rust 矩阵三步，`embedded` → `verify:frontend-fresh`，`injection` → `injection-suite.mjs --shard`，其余 → `--sections=` 列表）；验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；临时从某个 shard 删除一个 task 后必须非零退出并指出缺失 task，恢复后再次退出 0。
- [x] 2.3 在 `docs/arch-standards-remaining.md` 记录 task 分片表、42+1 的覆盖分解与 `task-1.5` 的新断言；验证：`rg -n "task-gates.*shard|42.*16.1|task-1.5" docs/arch-standards-remaining.md` 命中，且文档中的分片列表与 `ci.yml` 逐项一致（用 2.1 的命令输出对照）。

## 3. Rust 矩阵分层与 Linux 专属步骤

- [x] 3.1 改 `.github/workflows/ci.yml` 的 `Test` 步骤：`if: github.event_name != 'pull_request' || matrix.platform == 'Linux'`，`run: cargo test --workspace --all-targets --all-features`；Linux/macOS 的 checkout 去掉 `lfs: true`，Windows rust job 与 Windows platform Gate 保留；验证：`rg -n "github.event_name != 'pull_request' \\|\\| matrix.platform == 'Linux'" .github/workflows/ci.yml` 命中，`rg -n "lfs: true" .github/workflows/ci.yml` 只命中 Windows job，`node scripts/verify/checks/task-1.5.mjs` 退出 0。
- [x] 3.2 在 Rust (Linux) 增加 `Rust static gates`（`pnpm verify:governance -- --sections=rust-static,purity`，`ECHO_PURITY_REQUIRE_BUILD: "1"`）、`Task 16.1`（`pnpm verify:task -- 16.1`）、`Task 9.7 9.8`（`pnpm verify:task -- 9.7 9.8`）三个 Linux 专属步骤；删除独立的 `linux-platform-gate` job；步骤顺序固定为 `fmt → clippy → Test → Rust static gates → Task 16.1 → Task 9.7 9.8`；验证：`rg -n "linux-platform-gate" .github/workflows/ci.yml` 无输出，`rg -n "Rust static gates|Task 16.1|Task 9.7 9.8" .github/workflows/ci.yml` 命中，`node scripts/verify/checks/task-1.5.mjs` 退出 0；Linux 专属任务的实际执行由 PR 的 Rust (Linux) job 验证。
- [x] 3.3 在 `docs/arch-standards-remaining.md` 与 `docs/ci-pipeline-review-2026-10-28.md`（只追加「后续」小节，不改历史结论）记录 PR/main 覆盖分层与 Linux 步骤顺序；验证：`rg -n "PR.*main|Windows.*macOS.*main|Rust static gates" docs/arch-standards-remaining.md docs/ci-pipeline-review-2026-10-28.md` 命中。

## 4. Governance 分段、Release gate 与 Performance

- [x] 4.1 把 `.github/workflows/ci.yml` 的 `governance` job 改为 `governance-static`，运行 `pnpm verify:governance -- --sections=registry,static,coverage`；删除 webkit 与前端构建，保留 ffmpeg 与 coverage 工具；验证：`rg -n "governance-static|--sections=registry,static,coverage" .github/workflows/ci.yml` 命中，`node scripts/verify/ci-governance.mjs --sections=registry,static,coverage` 退出 0，`rg -n -A25 "governance-static:" .github/workflows/ci.yml` 中不含 `webkit` / `pnpm build`。
- [x] 4.2 改 `release-gate`：命令改为 `node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf`；新增 `perf` job（Rust + rust-cache + setup-node + pnpm，不装 webkit、不装 node 依赖、不构建前端），运行 `pnpm verify:perf`；验证：`rg -n "verify:perf|--skip-static|delegated=governance,perf" .github/workflows/ci.yml package.json` 命中，`node scripts/verify/check-scenario-evidence.mjs` 退出 0。
- [x] 4.3 在 `docs/arch-standards-remaining.md` 记录 governance 八段与执行位置、release gate 的新参数、performance job；验证：`rg -n "governance-static|performance|verify:perf|skip-static" docs/arch-standards-remaining.md` 命中，文档中的 `--sections` 列表与 `ci-governance.mjs --list-sections` 一致。

## 5. 注入证明分片

- [x] 5.1 新增 `scripts/verify/injection-shard.mjs`，导出 `selectEntries(entries, { only, shard })`：按 `entry.shard ?? (index % total) + 1` 分配，非法 spec / 越界 / 空片失败，每次调用断言各片并集等于全集且互斥；`injection-suite.mjs` 解析 `--shard=<i>/<n>` 并调用 helper，`--list` 仍列出全部条目；嵌入前端条目加 `shard: 1`；验证：`node scripts/verify/injection-suite.mjs --list | wc -l` 与改动前一致，`node scripts/verify/injection-suite.mjs --shard=1/2` 与 `--shard=2/2` 均退出 0，`node scripts/verify/injection-suite.mjs --shard=3/2` 非零退出。
- [x] 5.2 把 `.github/workflows/ci.yml` 的 injection job 改为 2 个 shard：两片都安装 Rust / rust-cache / webkit / pnpm 依赖；shard 1 构建前端与 shell，运行 `--shard=1/2` 与正向 `pnpm verify:frontend-fresh`；shard 2 只构建前端，运行 `--shard=2/2`；验证：`rg -n "shard: \\[1, 2\\]|--shard=" .github/workflows/ci.yml` 命中，`node scripts/verify/checks/task-1.5.mjs` 退出 0。
- [x] 5.3 在 `scripts/verify/injection-governance.mjs` 新增三条证明：把 `Test` 条件改成 `matrix.platform == 'Windows'`、删除 `Rust static gates` 步骤、从 `governance-static` 的 `--sections=` 删除 `coverage`；三者都必须让 `task-1.5.mjs` 失败；验证：`node scripts/verify/injection-suite.mjs --only ci-structure` 三条全部通过，`node scripts/verify/injection-suite.mjs --list | wc -l` 比 1.1 基线增加 3。
- [x] 5.4 在 `docs/arch-standards-remaining.md` 记录注入分片规则（轮转 + 显式 `shard`、每片全集完整性检查、两片依赖）；验证：`rg -n "injection.*shard|--shard" docs/arch-standards-remaining.md` 命中，且 `node scripts/verify/check-scale.mjs` 退出 0（新 helper 与 suite 均不超 1000 行）。

## 6. 本地集成验证

- [x] 6.1 独占运行全量治理门（不并行跑 cargo / rg / node 扫描）：`/usr/bin/time -p pnpm verify:governance`，退出 0；记录墙钟并与 1.1 基线比较，确认没有因为分段而漏跑任何 section。
- [x] 6.2 运行 `pnpm verify:task -- --all`，退出 0；确认 43 个 task 全部被执行（16.1 在 Rust Linux 的步骤中，其余 42 个在四个 shard 中），且没有 archived task 被误跑。
- [x] 6.3 运行 `node scripts/verify/run-scenario.mjs --automated`，退出 0；`ok: scenario` 行数等于 `manifest.scenarios - attestation 场景数 - governance/perf 委托场景数`；再按域各抽一条（core / desktop / vitest / node / attestation）执行并全部 ok。
- [x] 6.4 运行 `pnpm verify:self-test`、`node scripts/verify/checks/task-1.5.mjs`、`node scripts/verify/check-scenario-evidence.mjs`、`node scripts/verify/validate-scenario-commands.mjs`、`node scripts/verify/check-scale.mjs`，全部退出 0；`git status --porcelain` 为空（生成物已提交，无未预期改动）。

## 7. 归档与 PR

- [ ] 7.1 同步 delta spec 到 `openspec/specs/engineering-governance/spec.md`，运行 `openspec validate ci-pr-under-10-min --strict` 与 `openspec validate --all --strict` 通过后归档 change；验证：`openspec list` 中不再有活动 change，`git status --porcelain` 为空。
- [ ] 7.2 推送分支并创建中文 PR，正文关联 Issue #124，包含设计文档链接、基线 / 目标墙钟、覆盖映射与回滚项；验证：`gh pr view --json number,body,url` 显示 PR 已关联 #124，且 `gh pr checks` 已触发。
- [ ] 7.3 PR CI 实测：记录 run 总墙钟与各 job / step 墙钟，重点记录 Rust (Linux) 的 `Test` / `rust-static` / 16.1 / purity、Governance static、Injection 1/2、Release gate 与 Performance；验收主口径为 rust-cache 命中的 run < 10 分钟，冷缓存 run 单独记录；把结果写入 `docs/arch-standards-remaining.md` 或 `docs/ci-pipeline-review-2026-10-28.md` 并提交。
- [ ] 7.4 PR 全绿后合并；在 `main` run 上确认 Windows / macOS 的 `Test` 步骤实际执行（非 skipped），并记录 main 墙钟；验证：`gh run view <id> --json jobs` 中 Windows / macOS 的 Test 为 success。
