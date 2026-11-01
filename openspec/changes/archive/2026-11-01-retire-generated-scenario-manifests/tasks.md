## 1. 生成器与漂移门

- [x] 1.1 改 `scripts/verify/gen-scenario-manifests.mjs`：条目收缩为 `{id, title, command}`；删除 `nativeIds()`、`yamlEscape()`、`scenarioYaml()`、`nativeMd()`、`tests/scenarios|native` 两个目录常量与写入分支、`TRACE` 依赖；导入只剩 `allScenarioIds()` + `scenarioCommands()`。验证：`rg -n "nativeIds|scenarioYaml|nativeMd|tests/scenarios|tests/native|traceability" scripts/verify/gen-scenario-manifests.mjs` 无命中。
- [x] 1.2 把默认模式改成真漂移门：按 ID 建立 `manifest.json.scenarios[]` 与期望条目的映射，缺项 / 多项 / 标题漂移 / 命令漂移逐条报错并 `process.exit(1)`；`--write` 仍是唯一写入口，且写入顺序与内容可由默认模式复现。验证：`node scripts/verify/gen-scenario-manifests.mjs --write` 落盘后 `node scripts/verify/gen-scenario-manifests.mjs` exit 0 且输出 `manifest drift: 0`；把某条命令临时改一个字符再跑，必须非零并指出该 ID（验证后还原）。
- [x] 1.3 复核 1.2 的 `--write` diff：`scripts/verify/manifest.json` 的 `scenarios[]` 只允许删除 `"manifest": …` 行，不得出现字段重排、键序变化或 tasks 段改动。验证：`git diff -U0 scripts/verify/manifest.json` 的 523 个 hunk 全部起始于 `scenarios[]`（首个 hunk ≥ 2241 行），新增行仅为对应 `"command"` 行去掉尾逗号（`"manifest"` 原本是对象最后一个键，删除它必然改动前一行的逗号，因此「纯删除行」在 JSON 语法上不可达），删除行为原 `"command",` 与 `"manifest": …`；无字段重排、无键序变化、tasks 段零改动，且 `JSON.parse` 通过。

## 2. 退役生成物

- [x] 2.1 删除 `tests/scenarios/`（420 个 yaml）与 `tests/native/*.md`（103 个），新增 `tests/native/_TEMPLATE.md`：7 个必备字段的日志骨架、`evidence-path: artifacts/native-attestations/<ID>.log` 规则、指向 `docs/native-attestation-playbook.md` §6 的链接，并注明本文件不被任何门禁解析。验证：`find tests/scenarios -name '*.yaml' | wc -l` 为 0（目录可一并删除）；`ls tests/native` 只有 `_TEMPLATE.md`。
- [x] 2.2 删除 `scripts/verify/validate-scenario-manifests.mjs`。验证：`test ! -e scripts/verify/validate-scenario-manifests.mjs`；`rg -n "validate-scenario-manifests" --glob '!openspec/changes/**' --glob '!node_modules'` 只剩 tasks 2.3 待处理的接线。
- [x] 2.3 接线替换：`scripts/verify/ci-governance.mjs` 第 12 行改为 `["scenario registry drift", "node", ["scripts/verify/gen-scenario-manifests.mjs"]]`；`scripts/verify/checks/task-13.9.mjs` 第 94 行改为调用同一脚本。验证：`rg -n "scenario registry drift|gen-scenario-manifests" scripts/verify/ci-governance.mjs scripts/verify/checks/task-13.9.mjs` 各命中 1 处，且 `node scripts/verify/check-verification-validity.mjs` exit 0。
- [x] 2.4 删除 `scripts/verify/manifest.json` 任务 `14.4` 整块（**文本编辑**：1 空格缩进、数组 `],` 前保留空行；禁止 `JSON.stringify` 整体写回）。验证：`node -e 'const m=require("./scripts/verify/manifest.json");console.log(m.tasks.some(t=>t.id==="14.4"))'` 输出 `false`；`git diff scripts/verify/manifest.json` 的该 hunk 只有删除。
- [x] 2.5 删除 `scripts/verify/reconcile-scenarios.mjs` 规则 5（manifest 路径存在性，第 80–89 行）与头注释第 15–16 行的对应说明。验证：`node scripts/verify/reconcile-scenarios.mjs` 输出 `reconcile: spec 523 = trace 523 = manifest 523 scenarios` 且 exit 0。
- [x] 2.6 替换 `scripts/verify/injection-suite.mjs` 的 14.4 条目为 `scenario-registry/command-drift`：`check` 指向 `gen-scenario-manifests.mjs`，`inject()` 用 `replaceIn` 把 `scenario-commands.mjs` 中 `"CRP-R01-S01": "node scripts/verify/checks/task-crp.mjs"` 追加 ` --drift-probe`，`expect` 匹配 `manifest drift:[\s\S]*CRP-R01-S01`，`baseline: true`。验证：`node scripts/verify/injection-suite.mjs --only scenario-registry` 输出 1/1 通过、工作区恢复；`node scripts/verify/injection-suite.mjs --list | rg -c '^'` 仍为 35。

## 3. 文档与契约

- [x] 3.1 用脚本删除 `docs/traceability.md` 的「测试/步骤 manifest」列（第 6 列）：每个 area 的表头与分隔行同步；删除后逐行断言列数由 7 变 6。验证：`node -e` 检查所有表格行列数一致；`node scripts/verify/reconcile-scenarios.mjs` 仍 exit 0；`node scripts/verify/prd-matrix.mjs` exit 0。
- [x] 3.2 改写 `docs/traceability.md`「完成规则」第 2、3 条：删除「每行指定的 manifest 必须包含…」与「测试 manifest 的 ID 集合」，改成「场景 ID 由 spec 标题派生；每行必须有可执行验收命令；人工举证由 `docs/native-attestation-playbook.md` 与 `artifacts/native-attestations/<ID>.log` 承担」。验证：`rg -n "manifest" docs/traceability.md` 无残留生成物引用（仅允许「manifest.json」若确有必要）。
- [x] 3.3 改写 `docs/native-attestation-playbook.md`：§6 写清退役后的完整契约（唯一权威源、分类由命令决定、日志 7 字段、`evidence-path` 规则、`_TEMPLATE.md` 用法、自检命令）；删除 §7 对 `validate-scenario-manifests.mjs` 的引用与 §7.3 关于生成树孤儿文件的整段；§5 的人工步骤**原文保留**。验证：`rg -n "validate-scenario-manifests|tests/scenarios|tests/native/<ID>\.md" docs/native-attestation-playbook.md` 无命中（`tests/native/_TEMPLATE.md` 除外）；`git diff --stat docs/native-attestation-playbook.md` 的改动不落在 §5 行段。
- [x] 3.4 改 `scripts/verify/checks/check-native-attestation.mjs` 的失败提示：把「run the tests/native/<ID>.md human steps」改为指向 `docs/native-attestation-playbook.md` §5/§6；`REQUIRED_FIELDS`、`artifacts/native-attestations/<ID>.log` 与 `pass|ok` 判定一字不改。验证：对一个无日志的场景运行 `node scripts/verify/checks/check-native-attestation.mjs DAS-R12-S01`，非零退出且提示指向 playbook；`git diff` 只含提示字符串。
- [x] 3.5 同步其余引用：`docs/arch-standards-remaining.md`（删除 14.4 行与生成树计数，按复跑结果刷新基线数字）、`docs/legacy-compat-removal-plan.md`、`docs/ROADMAP.md`、`docs/acceptance/platform-gate-handoff.md`、`scripts/verify/scenario-commands.mjs` 头注释（删除「manifest.json.scenarios[] 与两棵树由它生成」）。验证：`rg -n "tests/scenarios|tests/native/\*|gen-scenario-manifests" docs scripts --glob '!openspec/changes/**'` 只剩 `_TEMPLATE.md` 与生成器自身的正当引用。

## 4. 收口

- [x] 4.1 场景链路抽查：`node scripts/verify/run-scenario.mjs CRP-R01-S01` exit 0（执行器从新 schema 的 `manifest.json` 取命令）；`node scripts/verify/validate-scenario-commands.mjs` exit 0；`node scripts/verify/check-scenario-command-proof.mjs` exit 0。
- [x] 4.2 治理门**独占**运行 `pnpm verify:governance`（不并行跑任何 cargo / rg / node 扫描），exit 0，且日志含 `injection-suite: 35/35`。
- [x] 4.3 `pnpm exec openspec validate retire-generated-scenario-manifests --strict` 通过；同步并归档 change 后 `pnpm exec openspec validate --archived` 通过。
- [x] 4.4 提交（中文信息，含每项「证据 → 改动」与真实命令输出）并开 PR，正文注明「关联 #110」；`git status --porcelain` 为空。
