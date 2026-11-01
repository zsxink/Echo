## Why

Issue #110 阶段 2（P1-1/P1-2）：`tests/scenarios/*.yaml`（420 个，约 292 KB）与
`tests/native/*.md`（103 个，约 65 KB）全部由 `gen-scenario-manifests.mjs` 从
`scenario-commands.mjs` 生成，**零人工内容**——103/103 个 native md 至今仍是未填写的
`(填写)` 占位模板。420 + 103 = 523，恰好是场景登记表的一行一个文件。

这层镜像既没有被执行路径读取，也已经开始撒谎：

- **不被读取**：`run-scenario.mjs` 直接执行 `scenario-commands.mjs` 里的命令；
  `check-native-attestation.mjs` 只读 `artifacts/native-attestations/<ID>.log`。
  唯一读 yaml 的 `validate-scenario-manifests.mjs` 只校验「生成物 = 登记表」，
  是自证循环，而不是对行为的验证。
- **已经漂移**（本分支实测）：523 行登记中有 **78 行**的 manifest 指向与命令类别不符
  ——17 行原生命令指向 `.yaml`，61 行自动命令指向 `.md`。`docs/traceability.md`
  的 manifest 列把 103 行写成人工步骤（生成器据此产出 `.md`），而真正以 attestation
  承担验收的只有 **59** 条；其余 61 行早已接上自动命令，`.md` 里的 `(填写)`
  永远不会被填写。
- **只增不删**：`gen-scenario-manifests.mjs --write` 从不清理不再登记的 id，
  孤儿文件会永久留存（`docs/native-attestation-playbook.md` §7.3 已记录过一例）。

`engineering-governance` 的「已登记的验收资产从未被解析」要求把这类资产判定为未生效
并列入处置清单。本 change 就是该处置：退役镜像，把契约留在唯一权威源与举证手册里，
而不是继续维护一份会与事实分叉的副本。

## What Changes

- **BREAKING**（对仓库内工具链）：删除 `tests/scenarios/`（420 个 yaml）与
  `tests/native/*.md`（103 个）；只新增一份 `tests/native/_TEMPLATE.md`
  作为人工举证模板。
- 删除 `scripts/verify/validate-scenario-manifests.mjs`，并拆除它的全部接线：
  - `scripts/verify/ci-governance.mjs` 的 `scenario manifest validity` 检查；
  - `scripts/verify/checks/task-13.9.mjs` 中的调用；
  - `scripts/verify/manifest.json` 任务 `14.4`；
  - `scripts/verify/injection-suite.mjs` 的 14.4 注入条目。
- **注入证明不减少**：以一条面向幸存契约的证明替换 14.4 —— 当某场景命令指向不存在
  的检查脚本时，`validate-scenario-commands.mjs` 必须报错退出。条目总数保持 35。
- `scripts/verify/manifest.json`：删除全部 523 条 scenario 的 `manifest` 字段
  （以及任务 14.4）。按 1 空格缩进**文本编辑**，禁止整体重排。
- `scripts/verify/gen-scenario-manifests.mjs`：只保留 `manifest.json` 生成；删除
  yaml/native 写入分支、`nativeIds()`、`manifest` 字段与 traceability manifest 列解析。
  同时把默认模式改成**真正的漂移门**：`manifest.json.scenarios[]` 与
  `scenario-commands.mjs` 不一致时非零退出（现在注释声称 nonzero，实际恒 exit 0）。
  这一条接替被删的 validator，成为 `ci-governance.mjs` 的第 12 行与 `task-13.9.mjs`
  的第 94 行。
- `scripts/verify/reconcile-scenarios.mjs`：删除规则 5（manifest 路径存在性）；
  「是否人工举证」继续由命令本身（含 `check-native-attestation.mjs`）判定，
  规则 6 的 P0 自动化断言不变。
- `docs/traceability.md`：删除「测试/步骤 manifest」列；三方对账（spec = traceability
  = 场景清单）与「每个场景必须有可执行命令」不变。
- `docs/native-attestation-playbook.md`：写入退役后的完整契约——唯一权威源、
  人工/自动分类由命令决定、举证日志路径与 7 个必备字段、`_TEMPLATE.md` 用法、
  自检命令；删除指向 `validate-scenario-manifests.mjs` 与生成树的段落。
- 同步其余文档引用：`docs/arch-standards-remaining.md`、`docs/legacy-compat-removal-plan.md`、
  `docs/ROADMAP.md`、`docs/acceptance/platform-gate-handoff.md`，以及
  `scripts/verify/scenario-commands.mjs` 头注释里「会生成 tests/ 两棵树」的说明。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

（无）

**为什么没有规格 delta**：本 change 不改变任何 requirement 的 WHEN/THEN，只是把
未被解析的登记资产移除，并把原本由生成物承载的说明挪进文档。`engineering-governance`
的三条相关要求仍由幸存机制满足：

- 「门禁不得静默通过」：登记命令继续由 `validate-scenario-commands.mjs` 校验目标存在
  与零匹配；
- 「规格追溯的完整性与阻断」：`reconcile-scenarios.mjs` 继续做三方数量与标识对账；
- 「已登记的验收资产从未被解析」：本 change 即其处置动作。

因此 `.openspec.yaml` 置 `skip_specs: true`。

## Impact

**受影响代码**：`scripts/verify/{ci-governance.mjs, checks/task-13.9.mjs,
injection-suite.mjs, manifest.json, gen-scenario-manifests.mjs, reconcile-scenarios.mjs,
scenario-commands.mjs}`；删除 `scripts/verify/validate-scenario-manifests.mjs`。

**受影响资产**：删除 `tests/scenarios/**`（420）、`tests/native/**`（103），
新增 `tests/native/_TEMPLATE.md`（1）。

**受影响文档**：`docs/traceability.md`、`docs/native-attestation-playbook.md`、
`docs/arch-standards-remaining.md`、`docs/legacy-compat-removal-plan.md`、
`docs/ROADMAP.md`、`docs/acceptance/platform-gate-handoff.md`。

**不变式（不得在本 change 中放宽）**：

- 真契约不动：`check-native-attestation.mjs` 仍要求
  `artifacts/native-attestations/<ID>.log`，`REQUIRED_FIELDS`（os、versions、
  desktop-env、operator、result、evidence-path、date）与 `result: pass|ok` 判定一字不改。
- 523 条场景登记、它们的命令、`-D warnings`、coverage 阈值、scale 白名单、
  注入证明条目数（≥ 35）全部不变；对账仍以非零退出阻断。
- 不修改任何 spec 的 WHEN/THEN。

**明确不在范围内**：

1. `scripts/audio-effects/` 的 Python 工具链（`inspect-filter-runtime.py`、
   `sine_sweep.py`、`task-audio-effects-evidence.mjs`，以及
   `validate-scenario-commands.mjs` 中对它们的路径钉死）。
2. `.github/workflows/release.yml` 的平台门（macOS / Windows / Linux 构建与签名矩阵）。
3. `docs/traceability.md`「测试层」列的重新分类。本 change 只删除 manifest 列并记录
   度量（78 行 manifest 指向与命令类别不符）；其中 17 行的命令已退回 attestation，
   需要先补真实命令或正式降级为人工举证，不能只改表格，留作后续 change。
