# Spec Delta

## MODIFIED Requirements

### Requirement: CI status checks use stable platform names

持续集成与发布构建矩阵 MUST 为每个受支持平台提供明确的平台语义标识，并 MUST 使用该标识构造面向维护者的 job 显示名。显示名 MUST NOT 包含 runner label；runner label 仅用于选择执行环境。更新 runner label 时，相同 job 的平台语义显示名 MUST 保持不变。矩阵校验 MUST 验证平台标识与其 runner 配置的对应关系，并能在该映射缺失或错误时失败。

每个 job 的平台覆盖 MUST 由其实际执行的检查决定，MUST NOT 为「与其他平台对称」而重复执行与平台无关的检查。前端质量检查（`format:check` / `lint` / `typecheck` / `vitest` / `build`）MUST 在 Linux 上执行一次；它们运行在 jsdom 与 Node 工具链上，结果与宿主平台无关，因此 MUST NOT 为 macOS / Windows 各起一条腿。前端在 macOS / Windows 上的**构建**覆盖由 Rust 矩阵分片的 Tauri 上下文构建保留，MUST NOT 随前端腿一起删除。矩阵收敛后，前端 job 的显示名 MUST 仍携带平台标识（`Frontend (Linux)`），MUST NOT 退化为不含平台的无标识名字。

#### Scenario: CI 检查名不泄露 runner label

- **WHEN** Rust 的三平台 CI 矩阵与 Frontend 的 Linux CI 矩阵运行
- **THEN** 检查名分别按 job 类型与平台显示（例如 `Rust (macOS)`、`Frontend (Linux)`），且不显示 `xcode-27`、`ubuntu-latest` 或 `windows-latest`

#### Scenario: 前端矩阵被收敛为单平台

- **WHEN** 一次改动把前端质量检查收敛为只在 Linux 上执行，且 macOS / Windows 的前端构建仍由 Rust 矩阵分片执行
- **THEN** 结构门禁通过；若收敛后前端 job 的显示名丢失平台标识，或 Rust 矩阵分片不再构建前端，结构门禁失败并指出缺口

#### Scenario: 发布构建检查名不泄露 runner label

- **WHEN** macOS、Windows、Linux 发布构建矩阵运行
- **THEN** 检查名分别显示 `Build (macOS)`、`Build (Windows)`、`Build (Linux)`，而 runner label 只用于执行环境选择

#### Scenario: Runner label 更新不改变平台检查名

- **WHEN** 某平台的 runner label 被替换，但该矩阵的平台映射保持正确
- **THEN** 该平台面向维护者的检查名保持不变

#### Scenario: 缺失或错误的平台映射被门禁拒绝

- **WHEN** CI 或发布矩阵缺少某受支持平台条目，或平台标识映射到错误的 runner
- **THEN** 对应 workflow 结构门禁以非零状态失败并指出不满足的平台映射

### Requirement: CI 执行分层与结构完整性

持续集成 MUST 按事件类型分层执行验证，且分层 MUST NOT 使任何检查从 Pull Request 与 `main` 同时消失。Pull Request MUST 执行三平台 `fmt` / `clippy`、Linux 全量 workspace 测试与各平台 Gate。Windows / macOS 的全量 workspace 测试 MUST 在推送到 `main` 时执行；在 Pull Request 上 MAY 被跳过。任何在 Pull Request 上被跳过的检查 MUST 在 `main` 上真实执行，并由结构门禁断言。

同一检查 MUST NOT 在多个 job 中重复执行，除非第二次执行覆盖了第一次无法覆盖的条件（不同平台、不同 feature 形态、不同运行环境）。重复执行 MUST 由结构门禁反向锁定：把一个已被其他 job 覆盖的步骤重新加回时，结构门禁 MUST 失败并指出该重复。

Windows / macOS 的全量 workspace 测试 MAY 按包分片执行；各分片所覆盖包的并集 MUST 等于全部 workspace 包，MUST NOT 因分片遗漏或删除任何包。分片引入的跳过 MUST 遵守与整段测试相同的分层规则：分片 MAY 仅在 Pull Request 上跳过，在 `main` 上 MUST 全部真实执行，且结构门禁 MUST 断言分片并集与各分片在 `main` 上的执行条件。

`clippy` MUST 在三平台以 workspace 形态执行，MUST NOT 收窄到单个 crate 或包子集。写入共享缓存的 rust 矩阵分片（`rust` 与 `rust-desktop` 两个 job 的分片），其最后一条构建型 cargo 调用 MUST 是该缓存 key 对应的固定 cargo 形态：Linux `workspace` 分片为 `cargo test --workspace --all-targets --all-features`，Windows / macOS 的 `core` 与 `desktop` 分片为 `cargo test <该分片包列表> --all-targets --all-features`；其后 MAY 执行同形态的 `-- --list`，MUST NOT 在缓存保存之前执行不同 feature、不同 target 选择或 check 形态的 cargo 调用，使缓存内容与后续读取者的构建形态不符。`governance-static` 的缓存 key 是 job 私有的：只有该 job 与定时依赖审计 workflow 读取它，其 coverage 插桩形态不会污染任何其他读者，因此不受上述 cargo 形态约束。缓存写入 MUST 只发生在 `main`；Pull Request 上的任何 job MUST NOT 保存缓存。

性能场景 MUST 在 Pull Request 与 `main` 上真实执行，且 MUST 由结构门禁断言其执行位置：承载它的 job MUST 在两种事件上都运行，其步骤 MUST 执行 `pnpm verify:perf`。性能场景 MAY 与 task gate 共享一个 job；共享时该 job 的缓存步骤 MUST NOT 保存缓存，避免 `--release` 产物污染其他读者读取的缓存 key。

CI 的验证结构 MUST 可被静态校验：task 分片的并集 MUST 覆盖全部已登记 task 且无重复；governance 的每个 section MUST 在 `ci.yml` 中有执行位置；场景委托 MUST 指向真实执行的 gate；删除、重命名或移动 job / step 导致上述结构不成立时，结构门禁 MUST 失败。

#### Scenario: PR 与 main 的分层覆盖

- **WHEN** 一次 Pull Request 运行持续集成
- **THEN** 三平台 `fmt` / `clippy`、Linux 全量 workspace 测试与各平台 Gate 实际执行；Windows / macOS 的全量 workspace 测试可以跳过
- **WHEN** 同一变更推送到 `main` 后运行持续集成
- **THEN** Windows / macOS 的全量 workspace 测试实际执行，而不是保持 skipped

#### Scenario: 检查从 PR 与 main 同时消失

- **WHEN** 某个检查在 Pull Request 上被跳过，且 `main` 上也没有执行它的 job 或 step
- **THEN** 结构门禁失败并指出该检查，而不是把分层当作删除检查的通道

#### Scenario: 已被其他 job 覆盖的检查被重复执行

- **WHEN** 某个 job 重新加入一条已被其他 job 在相同平台、相同 checkout 上执行的检查（例如在 `rust-desktop` 重新加入 `cargo fmt --all -- --check`）
- **THEN** 结构门禁失败并指出该重复步骤，而不是让配额继续被同一份检查消耗两次

#### Scenario: 同一 job 内重复执行同一测试套件

- **WHEN** 一个 job 内的两个 task gate 以同一形态运行同一个测试套件，且其中一份缺少另一份的 skip 防护或运行环境准备
- **THEN** 较弱的一份 MUST 被删除；结构门禁 MUST 断言该套件在该 job 内只被一份 gate 运行

#### Scenario: task 分片遗漏或重复

- **WHEN** 各 task 分片的 task 列表并集不等于全部已登记 task，或某个 task 同时出现在两个分片
- **THEN** 结构门禁失败并指出缺失或重复的 task

#### Scenario: governance section 无执行位置

- **WHEN** `ci-governance.mjs` 声明的某个 section 在 `ci.yml` 中没有执行位置
- **THEN** 结构门禁失败并指出该 section，而不是让该段检查静默不执行

#### Scenario: CI 结构被重命名或移动

- **WHEN** 承载 task 分片、governance section 或平台 Gate 的 job / step 被删除、重命名或移动，导致结构断言无法定位它
- **THEN** 结构门禁失败并指出缺失的 job / step

#### Scenario: main 上的 workspace 测试分片覆盖

- **WHEN** 一次推送到 `main` 的持续集成运行 Rust 矩阵
- **THEN** Windows / macOS 的每个测试分片都实际执行，且各分片包列表的并集等于全部 workspace 包

#### Scenario: 分片遗漏 workspace 包或在 main 上被跳过

- **WHEN** 某个测试分片的包列表被删减，或某个分片在 `main` 上被跳过，导致分片并集不再覆盖全部 workspace 包
- **THEN** 结构门禁失败并指出未覆盖的包或未执行的分片

#### Scenario: clippy 收窄到单个包

- **WHEN** 某平台的 `clippy` 以单包选择或非 workspace 形态执行
- **THEN** 结构门禁失败并指出该平台，而不是把覆盖收缩当作分片优化

#### Scenario: rust 矩阵缓存写者形态与缓存不一致

- **WHEN** 某个写入共享缓存的 `rust` 矩阵分片在保存缓存之前执行了与缓存 key 固定形态不一致的 cargo 调用（例如 `clippy` 位于 `Test` 之后，或出现 `--lib` 等不同 target 选择）
- **THEN** 结构门禁失败并指出该分片与不一致的 cargo 形态

#### Scenario: governance-static 的 job 私有缓存不受形态约束

- **WHEN** `governance-static` 以 coverage 插桩形态作为其最后一次**构建型** cargo 调用并保存到 `governance-static` 缓存 key
- **THEN** 该 job 不因形态与 `rust` 矩阵分片不同而失败；结构门禁确认该 key 只被 `governance-static` 与定时依赖审计 workflow 读取，没有任何 `rust` 矩阵分片或其他 job 以它为读者

#### Scenario: Pull Request 写入缓存

- **WHEN** 非 `main` 的 run 中任一 job 的缓存步骤允许保存缓存
- **THEN** 结构门禁失败并指出该 job，而不是让 PR 缓存继续占用配额

#### Scenario: 性能场景从 PR 与 main 同时消失

- **WHEN** 承载性能场景的步骤被删除、改写成不执行 `pnpm verify:perf`，或落到一个在 Pull Request 上不运行的 job
- **THEN** 结构门禁失败并指出该缺口，而不是让 3 个委托场景失去执行位置

#### Scenario: 性能场景与 task gate 共享 job 时保存缓存

- **WHEN** 承载性能场景的 job 的缓存步骤允许保存缓存，使 `--release` 产物写入其他读者读取的 key
- **THEN** 结构门禁失败并指出该 job
