# Spec Delta

## MODIFIED Requirements

### Requirement: 门禁不得静默通过

系统 MUST 保证任何验证命令在"实际未验证任何东西"时失败。以过滤器或名称选择的测试命令 MUST 断言至少执行了一个测试，零匹配 MUST 视为失败；场景或任务登记的验证命令 MUST 指向真实存在的测试或检查。

场景的验收命令 MAY 按域聚合——多个场景共享一次批量执行——但每个场景 MUST 显式声明它依赖的验收证据（一条或多条），且每一条证据 MUST 满足以下全部条件：目标在仓库中真实存在；目标在聚合命令的实际执行输出中被观测到执行过；目标与场景 THEN 子句所依赖的行为直接相关。仅凭"命令里带了过滤器"或"场景被列在某个批量命令下" MUST NOT 被计为已验证。聚合 MUST NOT 使任何场景的证据检查退化为恒真。

场景或任务 MAY 将其验收显式委托给另一个持续集成 gate，但委托 MUST 指向在持续集成中真实执行的 gate；委托 MUST NOT 成为跳过检查的通道。被委托的 gate MUST 在 `ci.yml` 的某个 job 中有执行位置，且委托关系 MUST 由静态门禁校验。

#### Scenario: 过滤器未命中任何测试

- **WHEN** 某条验证命令的过滤器因拼写错误或测试改名而未命中任何测试
- **THEN** 该命令失败并报告零匹配，而不是以成功退出

#### Scenario: 登记的命令指向不存在的目标

- **WHEN** 某场景或任务登记的验证命令指向不存在的测试、检查脚本或测试文件
- **THEN** 门禁失败并指出该登记项

#### Scenario: 检查无法被证明会失败

- **WHEN** 某个检查从未在注入违规时失败过，或其断言强度未经证明
- **THEN** 该检查被视为有效性未知并列入待证明清单，MUST NOT 被当作已生效的门禁计入覆盖；新增检查 MUST 同时提交"注入违规即失败"的可复现证明

#### Scenario: 已登记的验收资产从未被解析

- **WHEN** 仓库中存在被登记为验收资产、但没有任何脚本读取其内容的内容
- **THEN** 该资产被判定为未生效并列入处置清单，不计入门禁覆盖

#### Scenario: 聚合场景的证据目标不存在

- **WHEN** 某个场景声明的验收证据指向不存在的测试名、测试文件或检查脚本
- **THEN** 门禁失败并指出该场景标识与其缺失的证据目标，而不是因聚合命令本身成功而放行

#### Scenario: 聚合场景的证据未被实际执行

- **WHEN** 某场景声明的验收证据属于聚合命令的域，但该证据在本次执行的输出中一次也未出现
- **THEN** 门禁失败并指出该场景标识与未被观测到的证据，而不是以"批量命令已通过"结案

#### Scenario: 聚合掩盖了场景与证据的对应关系

- **WHEN** 一条聚合命令同时承载多个场景，而其中某个场景的证据被移除或改指向别的测试，且没有其他门禁能发现这一变化
- **THEN** 该场景的证据校验失败，使聚合不得成为稀释逐场景验收的通道

#### Scenario: 新增场景未声明证据

- **WHEN** 新增一个场景并登记了聚合域命令，但没有声明其验收证据
- **THEN** 门禁失败并指出该场景标识，不允许以"落在某个批量命令范围内"作为登记方式

#### Scenario: 委托的 gate 在持续集成中不存在

- **WHEN** 某场景或任务声明其验收委托给某个 gate
- **THEN** 静态门禁确认该 gate 在持续集成中有执行位置；若不存在，门禁失败并指出该声明

## ADDED Requirements

### Requirement: CI 执行分层与结构完整性

持续集成 MUST 按事件类型分层执行验证，且分层 MUST NOT 使任何检查从 Pull Request 与 `main` 同时消失。Pull Request MUST 执行三平台 `fmt` / `clippy`、Linux 全量 workspace 测试与各平台 Gate。Windows / macOS 的全量 workspace 测试 MUST 在推送到 `main` 时执行；在 Pull Request 上 MAY 被跳过。任何在 Pull Request 上被跳过的检查 MUST 在 `main` 上真实执行，并由结构门禁断言。

CI 的验证结构 MUST 可被静态校验：task 分片的并集 MUST 覆盖全部已登记 task 且无重复；governance 的每个 section MUST 在 `ci.yml` 中有执行位置；场景委托 MUST 指向真实执行的 gate；删除、重命名或移动 job / step 导致上述结构不成立时，结构门禁 MUST 失败。

#### Scenario: PR 与 main 的分层覆盖

- **WHEN** 一次 Pull Request 运行持续集成
- **THEN** 三平台 `fmt` / `clippy`、Linux 全量 workspace 测试与各平台 Gate 实际执行；Windows / macOS 的全量 workspace 测试可以跳过
- **WHEN** 同一变更推送到 `main` 后运行持续集成
- **THEN** Windows / macOS 的全量 workspace 测试实际执行，而不是保持 skipped

#### Scenario: 检查从 PR 与 main 同时消失

- **WHEN** 某个检查在 Pull Request 上被跳过，且 `main` 上也没有执行它的 job 或 step
- **THEN** 结构门禁失败并指出该检查，而不是把分层当作删除检查的通道

#### Scenario: task 分片遗漏或重复

- **WHEN** 各 task 分片的 task 列表并集不等于全部已登记 task，或某个 task 同时出现在两个分片
- **THEN** 结构门禁失败并指出缺失或重复的 task

#### Scenario: governance section 无执行位置

- **WHEN** `ci-governance.mjs` 声明的某个 section 在 `ci.yml` 中没有执行位置
- **THEN** 结构门禁失败并指出该 section，而不是让该段检查静默不执行

#### Scenario: CI 结构被重命名或移动

- **WHEN** 承载 task 分片、governance section 或平台 Gate 的 job / step 被删除、重命名或移动，导致结构断言无法定位它
- **THEN** 结构门禁失败并指出缺失的 job / step
