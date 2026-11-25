## MODIFIED Requirements

### Requirement: CI 执行分层与结构完整性

持续集成 MUST 按事件类型分层执行验证，且分层 MUST NOT 使任何默认检查从 Pull Request 与 `main` 同时消失。唯一的按需例外为本规范明示的 task 12.5 及其产物；其手动入口 MUST 保留。Pull Request MUST 执行 Linux 的 `fmt` / `clippy`、Linux 全量 workspace 测试与各平台 Gate。Windows / macOS 的全量 workspace 测试、`fmt` 与 `clippy` MUST 在推送到 `main` 时执行；在 Pull Request 上 MAY 被跳过。除已明示为按需执行的 task 12.5 外，任何在 Pull Request 上被跳过的默认检查 MUST 在 `main` 上真实执行，并由结构门禁断言。平台 Gate 在 Pull Request 上 MUST 编译并测试该平台相关代码，因此 macOS / Windows 的 `fmt` / `clippy` 虽不在 PR 上执行，平台相关的编译覆盖 MUST NOT 从 PR 上消失。

同一检查 MUST NOT 在多个 job 中重复执行，除非第二次执行覆盖了第一次无法覆盖的条件（不同平台、不同 feature 形态、不同运行环境）。重复执行 MUST 由结构门禁反向锁定：把一个已被其他 job 覆盖的步骤重新加回时，结构门禁 MUST 失败并指出该重复。

作为其他检查的前置输入而执行的构建 MUST NOT 被视为重复门禁：它的失败会阻断该 job 的真实检查，而不是独立宣告某个质量维度（例如 Rust 分片为 `generate_context!` 构建的 `apps/desktop/dist`，与前端 job 的 `Build` 步骤目的不同）。

场景的验收命令 MAY 显式委托给另一个在持续集成中真实执行的 gate，但委托 MUST 由场景登记项声明，MUST NOT 由命令形态推断。默认执行的委托 gate MUST 在 `ci.yml` 中有一条可定位的执行命令：`workspace-test` 对应 Linux `rust` job 的 `cargo test --workspace --all-targets --all-features`，`frontend-test` 对应 `frontend` job 的完整 Vitest 套件。结构门禁 MUST 校验每个委托 gate 的执行命令真实存在，且该命令 MUST 与场景登记的域一致。

Windows / macOS 的全量 workspace 测试 MAY 按包分片执行；各分片所覆盖包的并集 MUST 等于全部 workspace 包，MUST NOT 因分片遗漏或删除任何包。分片引入的跳过 MUST 遵守与整段测试相同的分层规则：分片 MAY 仅在 Pull Request 上跳过，在 `main` 上 MUST 全部真实执行，且结构门禁 MUST 断言分片并集与各分片在 `main` 上的执行条件。将 macOS / Windows 的多个分片合并为一个 `--workspace` 分片 MUST NOT 改变包并集。

`clippy` MUST 在三平台以 workspace 形态执行，MUST NOT 收窄到单个 crate 或包子集：Linux 在 `rust` job 的 `workspace` 分片执行，macOS / Windows 在 main-only 的 `rust-desktop` job 执行。写入共享缓存的 rust 矩阵分片（`rust` 与 `rust-desktop` 两个 job 的分片），其最后一条构建型 cargo 调用 MUST 是该缓存 key 对应的固定 cargo 形态：Linux `workspace` 分片为 `cargo test --workspace --all-targets --all-features`，macOS / Windows 的 `desktop` 分片为 `cargo test --workspace --all-targets --all-features`；其后 MAY 执行同形态的 `-- --list`，MUST NOT 在缓存保存之前执行不同 feature、不同 target 选择或 check 形态的 cargo 调用，使缓存内容与后续读取者的构建形态不符。`governance-static` 的缓存 key 是 job 私有的：只有该 job 与定时依赖审计 workflow 读取它，其 coverage 插桩形态不会污染任何其他读者，因此不受上述 cargo 形态约束。缓存写入 MUST 只发生在 `main`；Pull Request 上的任何 job MUST NOT 保存缓存。macOS 完整打包的 release 产物 MUST 使用独立的缓存 key `macos-desktop-release`，其固定构建形态为 release profile 的 `tauri build --bundles app --target aarch64-apple-darwin`；该 key MUST NOT 与 debug 形态的 workspace 测试缓存 key 混用，MUST NOT 在同一 job 内由多个缓存步骤重复归档 `target/`，且 MUST 遵循与其它共享缓存相同的 main-only 写入规则。结构门禁 MUST 断言该 key 的写者、读者与形态。

Linux 的 `linux-workspace` 与 `governance-static` Rust 缓存组 MUST 使用只包含项目钉住工具链的专用 Rustup home。每组读写方 MUST 在安装工具链和恢复缓存之前使用同一配置；runner 预装工具链不参与缓存身份。缓存 MUST 继续区分真实的工具链、编译环境及依赖变化，且 MUST 仅由 `main` 写入。结构门禁 MUST 验证共享配置与步骤顺序。

导入可见性、持久化保存确认和歌单维护的快速正确性测试 MUST 在 Pull Request 与 main 的 workspace 测试执行。默认 CI MUST NOT 运行重型本地编辑计时。独立定时及手动入口 MUST 在 Release 下运行 1,000 与 10,000 首规模，分别记录收藏、创建歌单、添加及移除成员的命令与目录提交耗时，并区分收藏同步投影和歌单后台投影；无后台阶段的命令 MUST 明确记为不适用。四类命令 p95 在两种规模 MUST 严格小于 50 ms。默认 Release 报告 MUST 显式递延未执行的本地编辑计时且不得将其计为通过；结构门禁 MUST 验证执行入口与报告选项。独立的 task 12.5 benchmark 及其产物上传 MUST 保持既有按需入口。

coverage 门禁 MAY 在 Pull Request 上跳过，但 MUST 在推送到 `main` 时真实执行；结构门禁 MUST 断言其 main-only 执行条件与命令。Pull Request 上 MUST 继续执行 registry 与 static 检查。

治理门禁的注入证明套件（mutation-test 各门禁在违规时确实失败）MAY 作为本地命令维护，MUST NOT 作为 Pull Request 的 job 执行；结构门禁 MUST 断言 `ci.yml` 中没有任何 job 执行该套件，同时 MUST 保留对 proof 登记的静态校验。

CI 的验证结构 MUST 可被静态校验：task 分片的并集 MUST 覆盖全部未被标记 `archived: true` 且声明了 CI 执行位置的 task，且无重复；`archived: true` 的 task MUST NOT 被要求出现在 task 分片中，其断言 MUST 由真实执行的 workspace test、frontend test 或平台 Gate 承载，且 manifest 的归档理由 MUST 指出承载 gate。governance 的每个 section MUST 在 `ci.yml` 中有执行位置；场景委托或显式手动递延 MUST 指向真实执行的 gate，并区分默认自动执行与仅可手动执行；删除、重命名或移动 job / step 导致上述结构不成立时，结构门禁 MUST 失败。

纯文档变更 MAY 通过 `paths-ignore` 跳过 CI，但忽略清单 MUST NOT 包含任何门禁输入（例如 `docs/traceability.md`、`docs/acceptance/PRD-matrix.md`、`docs/LOGGING.md`、`docs/native-attestation-playbook.md`，以及被 gate 读取的 `README.md`、归档 OpenSpec 证据与 `artifacts/` 记录）。结构门禁 MUST 把忽略清单固定为已审计的集合，新增或删除条目 MUST 失败。

#### Scenario: PR 与 main 的分层覆盖

- **WHEN** 一次 Pull Request 运行持续集成
- **THEN** Linux 的 `fmt` / `clippy`、Linux 全量 workspace 测试与各平台 Gate 实际执行；Windows / macOS 的全量 workspace 测试、`fmt` 与 `clippy` 可以跳过
- **WHEN** 同一变更推送到 `main` 后运行持续集成
- **THEN** Windows / macOS 的全量 workspace 测试、`fmt` 与 `clippy` 实际执行，而不是保持 skipped

#### Scenario: 检查从 PR 与 main 同时消失

- **WHEN** 某个默认检查（不包括明示按需的 task 12.5）在 Pull Request 上被跳过，且 `main` 上也没有执行它的 job 或 step
- **THEN** 结构门禁失败并指出该检查，而不是把分层当作删除检查的通道

#### Scenario: 已被其他 job 覆盖的检查被重复执行

- **WHEN** 某个 job 重新加入一条已被其他 job 在相同平台、相同 checkout 上执行的检查（例如在 `release-gate` 重新加入完整 `cargo test --workspace --all-targets --all-features`）
- **THEN** 结构门禁失败并指出该重复步骤，而不是让配额继续被同一份检查消耗两次

#### Scenario: 同一 job 内重复执行同一测试套件

- **WHEN** 一个 job 内的两个 task gate 以同一形态运行同一个测试套件，且其中一份缺少另一份的 skip 防护或运行环境准备
- **THEN** 较弱的一份 MUST 被删除；结构门禁 MUST 断言该套件在该 job 内只被一份 gate 运行

#### Scenario: 场景委托指向不存在的 gate

- **WHEN** 某个场景声明 `delegatedTo: "workspace-test"` 或 `"frontend-test"`，但 `ci.yml` 中不存在对应的完整测试命令
- **THEN** 结构门禁失败并指出该委托 gate，而不是让场景以「已委托」之名跳过

#### Scenario: task 分片遗漏或重复

- **WHEN** 各 task 分片的 task 列表并集不等于全部未归档且有 CI 执行位置的 task，或某个 task 同时出现在两个分片
- **THEN** 结构门禁失败并指出缺失或重复的 task

#### Scenario: 归档 task 仍被要求执行

- **WHEN** 某个 task 被标记 `archived: true`，但其 manifest 归档理由没有指出承载其断言的 gate，或该 gate 在 `ci.yml` 中不存在
- **THEN** 结构门禁失败并指出该 task 与缺失的承载 gate

#### Scenario: governance section 无执行位置

- **WHEN** `ci-governance.mjs` 声明的某个 section 在 `ci.yml` 中没有执行位置
- **THEN** 结构门禁失败并指出该 section，而不是让该段检查静默不执行

#### Scenario: coverage 在 PR 上被跳过

- **WHEN** coverage 步骤被移到 main-only，但 `main` 上没有执行 `--sections=coverage` 的步骤，或其 PR 跳过条件缺失
- **THEN** 结构门禁失败并指出 coverage 的缺失执行位置或错误分层

#### Scenario: CI 结构被重命名或移动

- **WHEN** 承载 task 分片、governance section 或平台 Gate 的 job / step 被删除、重命名或移动，导致结构断言无法定位它
- **THEN** 结构门禁失败并指出缺失的 job / step

#### Scenario: main 上的 workspace 测试分片覆盖

- **WHEN** 一次推送到 `main` 的持续集成运行 Rust 矩阵
- **THEN** macOS / Windows 的每个测试分片都实际执行，且各分片包列表的并集等于全部 workspace 包

#### Scenario: 分片遗漏 workspace 包或在 main 上被跳过

- **WHEN** 某个测试分片的包列表被删减，或某个分片在 `main` 上被跳过，导致分片并集不再覆盖全部 workspace 包
- **THEN** 结构门禁失败并指出未覆盖的包或未执行的分片

#### Scenario: clippy 收窄到单个包

- **WHEN** 某平台的 `clippy` 以单包选择或非 workspace 形态执行，或 main 上的 macOS / Windows `clippy` 被删除
- **THEN** 结构门禁失败并指出该平台，而不是把覆盖收缩当作分片优化

#### Scenario: rust 矩阵缓存写者形态与缓存不一致

- **WHEN** 某个写入共享缓存的 `rust` 或 `rust-desktop` 分片在保存缓存之前执行了与缓存 key 固定形态不一致的 cargo 调用（例如 `clippy` 位于 `Test` 之后，或出现 `--lib` 等不同 target 选择）
- **THEN** 结构门禁失败并指出该分片与不一致的 cargo 形态

#### Scenario: governance-static 的 job 私有缓存不受形态约束

- **WHEN** `governance-static` 以 coverage 插桩形态作为其最后一次**构建型** cargo 调用并保存到 `governance-static` 缓存 key
- **THEN** 该 job 不因形态与 `rust` 矩阵分片不同而失败；结构门禁确认该 key 只被 `governance-static` 与定时依赖审计 workflow 读取，没有任何 `rust` 矩阵分片或其他 job 以它为读者

#### Scenario: Linux runner 更新预装工具链

- **WHEN** runner 更新预装的默认 Rust，而 Echo 项目钉住的工具链、编译环境与依赖均未改变
- **THEN** 两组 Linux Rust 缓存均不因预装版本变化更换缓存键；配置缺失或位置错误被结构门禁阻断，Pull Request 仍不得写入缓存

#### Scenario: Pull Request 写入缓存

- **WHEN** 非 `main` 的 run 中任一 job 的缓存步骤允许保存缓存
- **THEN** 结构门禁失败并指出该 job，而不是让 PR 缓存继续占用配额

#### Scenario: 性能场景从 PR 与 main 同时消失

- **WHEN** 默认 CI 执行，或有人删掉定时/手动入口的实际计时命令
- **THEN** 默认 workspace 保留导入、收藏与歌单行为测试；独立入口遗漏命令会被结构门禁阻断

#### Scenario: PR 仅修改已审核的样式或说明文档

- **WHEN** 一次已触发的默认 CI 处理只改样式的 PR
- **THEN** 仍运行 workspace 正确性；重型计时与 task 12.5 不进入默认 CI，文档路径仍按原规则处理

#### Scenario: PR 性能相关性无法排除或触发 main CI

- **WHEN** 默认 Release gate 运行而未执行独立计时
- **THEN** 两条本地编辑时延场景明确记为未运行，并从通过分母扣除；不得由默认性能 job 虚报通过

#### Scenario: 性能场景与 task gate 共享 job 时保存缓存

- **WHEN** 重型本地编辑计时被重新加入默认 job 或独立计时写入共享 Rust 缓存
- **THEN** 结构门禁失败并指出该配置

#### Scenario: 注入证明套件被重新加回 PR

- **WHEN** `ci.yml` 中任一 job 的步骤执行 `injection-suite.mjs`
- **THEN** 结构门禁失败并指出该 job，而不是让门禁自身的 mutation 测试重新占用 PR 配额

#### Scenario: 纯文档 PR 触发全量 CI

- **WHEN** `paths-ignore` 清单新增或删除了条目，或包含一个门禁输入文件
- **THEN** 结构门禁失败并指出该条目，而不是让文档提交继续消耗全量 CI 或让门禁输入被跳过

#### Scenario: 50k 基准仅按需执行

- **WHEN** 默认 PR 或 main CI 运行
- **THEN** 不执行 task 12.5 或同等 50k 基准、不上传其产物；release gate 将 LE-R07-S01 精确记为未运行的手动递延，不计入通过或由默认 job 覆盖

- **WHEN** 用户手动触发 50k benchmark 流程或显式运行本地 task 12.5
- **THEN** 运行原真实 50k 测量并检查原预算，生成同格式 JSON；手动 GitHub 流程在缺失产物时失败并在成功时上传

- **WHEN** 删除手动入口、增加非手动触发、删掉命令或上传，或将 benchmark 重新加入默认 CI
- **THEN** 治理结构门禁失败

## ADDED Requirements

### Requirement: macOS 完整打包使用独立的 release 形态缓存

macOS 完整打包 MUST 使用与 debug 形态隔离的 release 缓存 key 保存构建产物。该 key MUST 只由 macOS 平台 Gate 读写，MUST 只在 `main` 上写入；Pull Request MUST 只读取 `main` 写入的条目，MUST NOT 保存任何条目。缓存命中 MUST NOT 改变门禁语义：release 构建步骤、1.9、1.10、9.7 与严格嵌入前端校验 MUST 每次真实执行，MUST NOT 以「产物来自缓存」为由跳过。

仓库缓存总量 MUST 保持低于平台上限。缓存键中环境哈希已非当前环境的条目不可达，MUST 在写入新条目前清理，MUST NOT 依赖平台的自动淘汰来腾出空间：淘汰是全仓库最久未访问优先，可能连带删除仍被读取的条目。

#### Scenario: Pull Request 命中 release 缓存

- **WHEN** 一个 Pull Request 运行 macOS 平台 Gate，且 `main` 已写入对应的 release 缓存条目
- **THEN** Gate 恢复依赖产物并只重编工作区 crate 与最终链接，1.9、1.10、9.7 与严格嵌入前端校验仍然真实执行

#### Scenario: Pull Request 不写缓存

- **WHEN** 任意 Pull Request 运行 macOS 平台 Gate
- **THEN** 该 job 不保存任何缓存条目，只读取 `main` 写入的 release 缓存条目

#### Scenario: release 缓存被改回 debug 形态

- **WHEN** macOS 平台 Gate 的缓存步骤被改回 debug 形态的 workspace 测试 key，或该 key 被其它 job 读写，或同一 job 内出现第二个 Rust 缓存步骤
- **THEN** 结构门禁失败并指出该 job 或该 key，而不是让 release 构建继续每次从零开始

#### Scenario: 缓存配额逼近上限

- **WHEN** 仓库缓存条目总量逼近平台上限
- **THEN** 过期且不可达的条目被清理后再写入新条目，而不是让平台自动淘汰仍被读取的条目
