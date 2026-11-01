## Context

动机见 `proposal.md — Why`。这里的现状事实决定了实现方式：

- **执行入口读的是生成物**：`scripts/verify/run-scenario.mjs` 从
  `manifest.json.scenarios[]` 取 `id / command` 并执行；`scenario-commands.mjs`
  只是它的生成源。
- **两个幸存的静态门各管一半**：`reconcile-scenarios.mjs` 管「spec = traceability =
  manifest 的 ID 集合」与「每条有非空命令」；`validate-scenario-commands.mjs`
  管「registry 里的命令指向真实测试/脚本」。**没有任何门禁比较
  `manifest.json` 与 `scenario-commands.mjs`**——`validate-scenario-manifests.mjs`
  在 `entry = byId.get(id) || commands.get(id)` 处优先取 manifest 行，因此连它也没在
  比较两者。
- `gen-scenario-manifests.mjs` 的头注释写着「Without `--write` … exits nonzero if
  the tree would drift from traceability」，实现却是 `process.exit(0)`；默认模式只
  数「哪些 yaml/md 会新建」。
- `check-native-attestation.mjs` 是唯一的人工举证执行契约，只读
  `artifacts/native-attestations/<ID>.log` 的 7 个字段；不读 `tests/native/*.md`。
- `injection-suite.mjs` 当前 35 条证明，14.4 是其中一条；`ci-governance.mjs`
  无条件设 `ECHO_INJECTION_REQUIRE_ALL=1`，所以任何一条被跳过都算失败。

## Goals / Non-Goals

**Goals:**

- 删除 523 个纯镜像文件（420 yaml + 103 md），只留一份不被解析的模板。
- 让「生成物 vs 权威源」这条约束从自证循环变成真门：`manifest.json.scenarios[]`
  与 `scenario-commands.mjs` 逐条一致，否则非零退出并被 CI 阻断。
- 保持注入证明条数不减少（≥ 35），且新证明针对**真实违规**而非脚本自身输入。
- 分类唯一化：人工/自动只由命令判定（`check-native-attestation.mjs` 是唯一人工标记）。

**Non-Goals:**

- 不重构 `manifest.json.scenarios[]` 这个中间产物（让消费者直接 import
  `scenario-commands.mjs` 是更大的注册表收敛，留给后续阶段）。
- 不重新分类 `traceability.md` 的「测试层」列（见 proposal「不在范围内」）。
- 不改变 `check-native-attestation.mjs` 的字段、路径或 `pass|ok` 判定。

## Decisions

### D1 — 删除镜像，权威源保留三处，中间产物只留一份

`scenario-commands.mjs`（命令）、spec 标题（ID/标题）、`traceability.md`（审计行）
保留；`manifest.json.scenarios[]` 继续生成，但条目收缩为 `{id, title, command}`
（删掉 `manifest` 字段）。

- 备选：保留 yaml 但删 md。否——两者都是零人工内容的镜像，只删一半仍要维护
  生成器分支、校验器与逐行 manifest 字段。
- 备选：换成 TSV/JSON 单文件镜像。否——单文件仍是镜像，问题不在格式而在
  「多一份会漂移的副本」；`manifest.json` 已经承担这一角色，且执行器已经读它。

### D2 — 生成器默认模式升级为漂移门，接替被删的 validator

`gen-scenario-manifests.mjs` 无参数时：

1. 从 `allScenarioIds()` + `scenarioCommands()` 构造期望条目（`{id, title, command}`）；
2. 与 `manifest.json.scenarios[]` 按 ID 建映射比较——缺项、多项、标题漂移、
   命令漂移分别报错，任一存在则 exit 1；
3. `--write` 仍是唯一的写入入口。

`ci-governance.mjs` 第 12 行的命令从 `validate-scenario-manifests.mjs` 改为
`gen-scenario-manifests.mjs`（标题改为 `scenario registry drift`）；
`task-13.9.mjs` 第 94 行同样替换。

理由：`run-scenario.mjs` 执行的是 manifest 里的命令，而现存门禁都不比较它与
registry。删掉 yaml 校验器后如果不补这一条，漂移将完全无人发现——那是**降低**严格度。
把生成器自己的 dry-run 变成门，比再写一个只做比较的脚本更少重复：生成逻辑只存在一处。

- 备选：保留 `validate-scenario-manifests.mjs`，去掉 yaml 解析、只比较 registry 与
  manifest。否——那正是生成器已持有的逻辑，两份比较实现会以不同方式漂移；
  而且文件名为「manifest validation」却没有 manifest 可校验，会误导后来者。
- 备选：让 `run-scenario.mjs` 直接读 `scenario-commands.mjs`，彻底删掉
  `manifest.json.scenarios[]`。否——`manifest.json` 同时承载 tasks 与 scenarios，
  `task-13.9`、`run-task`、PRD 矩阵等多个消费者依赖它；这是阶段 3/4 的注册表
  收敛议题，不在本 change。

### D3 — 注入证明 14.4 换成「registry 漂移」

新条目：`scenario-registry/command-drift`，`check` 指向 `gen-scenario-manifests.mjs`，
用 `replaceIn` 把 `scenario-commands.mjs` 里 `CRP-R01-S01` 的命令追加一个
`--drift-probe` 参数，断言门以 `manifest drift … CRP-R01-S01` 失败。`baseline: true`。

- 这证明的是真实违规面：权威源改了而生成物没跟上——执行器随后会跑旧命令。
- 备选：证明 `validate-scenario-commands.mjs` 的 missing-script 路由。否——那条路由
  不是本 change 删除的东西，且新门覆盖的是本 change 唯一新增的约束；
  证明应与新增/保留的契约一一对应。
- 条目数：35 - 1 + 1 = 35，满足 closeout ≥ 35。

### D4 — 人工/自动分类以命令为准，删除 traceability manifest 列

`isNative := /check-native-attestation\.mjs/.test(command)`。这不是新约定：
`reconcile-scenarios.mjs` 规则 6 与 `task-13.9.mjs` 的
`expectedAutomatedCount` 已经这样判定；删掉 manifest 列后不再有第二处分类源。

- 备选：保留 manifest 列作为「步骤文档指针」。否——列已漂移 78/523，且没有任何
  门禁校验它；保留等于保留一个会继续腐烂的第二权威源。

### D5 — 模板只保留一份，契约主体在 playbook

`tests/native/_TEMPLATE.md` 只含：7 个必备字段的日志骨架、`evidence-path` 的固定
路径规则、指向 `docs/native-attestation-playbook.md` §6 的链接，以及一句
「本文件不被任何门禁解析；门禁只读 `artifacts/native-attestations/<ID>.log`」。
playbook §6 是权威说明，并在其中注明字段清单的唯一实现是
`check-native-attestation.mjs` 的 `REQUIRED_FIELDS`。

- 备选：让模板携带完整人工步骤模板（目标平台/步骤/预期/证据四段）。否——那正是
  103 个 md 被填充前的形状，全部是 `(填写)`；真正的步骤已在 playbook §5。

### D6 — traceability 删列用脚本按列切分

523 行数据 + 每个 area 的表头/分隔行，第 6 列（`测试/步骤 manifest`）整体删除。
用 `split("|")` 去掉索引 5 的单元、重新拼接，**不用正则猜内容**；删除后断言每行
`|` 数量比原来少 1 且列数恒为 6，再跑 `reconcile-scenarios.mjs` 与
`prd-matrix.mjs` 复算。

- 备选：手工编辑。否——523 行必错，且错误只会在对账时暴露。

### D7 — 不产出 spec delta

见 `proposal.md — Capabilities`。`.openspec.yaml` 置 `skip_specs: true`；
本 change 不触碰任何 WHEN/THEN。

## Risks / Trade-offs

- **[漏掉一个消费者，治理门当场红]** → apply 前以 `rg -l "tests/scenarios|tests/native"`
  列出全部 14 个引用点（不含生成树与 `openspec/changes/**`），逐项在 tasks 中落位；
  closeout 跑整门验证。
- **[改 manifest.json 产生全文件重排]** → 只做两类文本删除（523 行 `"manifest": …`
  与 14.4 任务块），diff 断言「除删除行外无其他 hunk」；禁止 `JSON.stringify` 整体写回。
- **[漂移门与 `--write` 的输出不一致导致假红]** → 先改生成器、`--write` 一次落盘，
  再跑默认模式断言 0 漂移；两者不一致时以默认模式的报错为准修生成器。
- **[删除 103 个 md 丢失人工步骤]** → 这 103 个文件全部是 `(填写)` 占位；真实步骤在
  playbook §5（DAS-R12/R13、DP-R03/R08/R10、PHA-R01、DAS-R18 等）。tasks 中把
  「复核 §5 覆盖了所有以 attestation 承担验收的家族」列为显式检查项。
- **[注入证明替换后条目数掉到 34]** → closeout 断言
  `ECHO_INJECTION_REQUIRE_ALL=1 node scripts/verify/injection-suite.mjs` 输出
  `35/35`；不足即视为未完成。
- **[traceability 表格被脚本改坏]** → 删除后逐行断言列数；`reconcile-scenarios.mjs`
  的 ID 集合对账是第二道保险。
- **[playbook 改写误伤 §5 的人工步骤]** → 只改 §6、§7 与提到生成树/validator 的
  行；§5 全文不动，diff 人工复核。

## Migration Plan

单分支顺序提交（每步都可独立验证）：

1. 生成器：新 schema + 默认漂移门；`--write` 落盘新 `manifest.json`；默认模式 0 漂移。
2. 删除两棵生成树 + `validate-scenario-manifests.mjs`；新增 `tests/native/_TEMPLATE.md`。
3. 接线：`ci-governance.mjs`、`task-13.9.mjs`、`injection-suite.mjs`（替换条目）、
   `manifest.json` 任务 14.4 删除、`reconcile-scenarios.mjs` 规则 5 与头注释删除。
4. `docs/traceability.md` 删列 + 完成规则改写。
5. 文档同步：playbook 契约、`arch-standards-remaining.md`、`legacy-compat-removal-plan.md`、
   `ROADMAP.md`、`acceptance/platform-gate-handoff.md`、`scenario-commands.mjs` 头注释。
6. closeout：独占 `pnpm verify:governance`；注入套件 ≥ 35；`openspec validate --strict`；
   归档后 `--archived`。

**回滚**：本 change 删除了生成器的 yaml/native 分支，因此不能用
`gen-scenario-manifests.mjs --write` 重放被删文件；回滚只能 revert 本分支的提交。
这是有意的——保留重放能力就等于保留镜像的再生路径。
