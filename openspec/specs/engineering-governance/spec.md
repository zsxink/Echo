# engineering-governance Specification

## Purpose
定义 Echo 仓库的工程治理契约：把代码规范与架构规范转成可被自动门禁验证的需求，使分层越界、静态检查失效、门禁静默通过与规格追溯断裂在提交与发布前显式失败，而不是留待下一次人工审计发现。

## Requirements

### Requirement: main 分支变更必须经过 Issue 与 Pull Request

`main` MUST 配置为受保护分支。每项变更 MUST 有对应的 GitHub Issue，并在独立分支上实施；变更 MUST 通过关联该 Issue 的 Pull Request 合并到 `main`。任何人（包括仓库管理员）MUST NOT 直接向 `main` 推送、强推或删除该分支。

#### Scenario: 未关联 Issue 的变更请求合并

- **WHEN** 一个 Pull Request 未关联对应的 GitHub Issue
- **THEN** 它不符合仓库变更流程，MUST NOT 合并

#### Scenario: 变更直接推送到 main

- **WHEN** 任意用户（包括管理员）尝试直接推送、强推或删除 `main`
- **THEN** GitHub 分支保护拒绝该操作

#### Scenario: 变更通过 Pull Request 合并

- **WHEN** 变更在独立分支上完成，Pull Request 关联了对应 Issue，且满足仓库门禁
- **THEN** 变更可以通过 Pull Request 合并到 `main`

### Requirement: 分层边界与领域规则归属

系统 MUST 由共享 Core 单一提供与平台无关的领域规则，包括资料库视图或歌单到有序播放上下文的解析、歌曲到其资料库内绝对位置的解析，以及播放会话恢复时对各项可用性的裁决。平台层（桌面端、应用壳与后续移动端）MUST 只承担输入输出、平台能力与适配，不得为达成同一结果自行重新实现上述规则。

平台层为完成调用而做的类型转换、命令编排与错误映射不视为重复实现；判定重复实现的标准是"存在同一领域决策的第二处独立实现"。

#### Scenario: 领域规则在 Core 之外出现第二处实现

- **WHEN** 平台层新增了一段可独立改变同一领域决策结果的逻辑（例如自行计算视图顺序、自行拼接资料库内绝对位置、自行裁决恢复项的去留）
- **THEN** 架构守卫失败并指出该文件与判定理由，提交被拒绝

#### Scenario: 多端对同一输入得到一致结果

- **WHEN** 同一个资料库视图或歌单在不同端按同一输入建立播放上下文
- **THEN** 各端得到相同的条目顺序与相同的选中项解析结果，且该结果由 Core 的同一实现产生

#### Scenario: 平台层只做适配时的判定

- **WHEN** 平台层的改动仅涉及反序列化、参数校验、错误映射或调用 Core 用例
- **THEN** 架构守卫不将该改动判为重复实现，不予阻断

### Requirement: 架构守卫的对称性与自证

系统 MUST 为 workspace 内每个 crate 提供架构守卫测试，并对全部 crate 生效。守卫 MUST 同时约束两个方向：Core 不得依赖平台与播放器实现，平台层不得承载领域规则。守卫 MUST 包含能拒绝合成违规的自证用例——一个从未拒绝过任何输入的守卫不得被视为有效。

#### Scenario: 新增 crate 未纳入守卫

- **WHEN** workspace 新增一个 crate 而架构守卫未覆盖它
- **THEN** 守卫失败并列出未被覆盖的 crate

#### Scenario: 守卫被削弱为永真

- **WHEN** 守卫的检测逻辑被改为对任何输入都通过
- **THEN** 守卫自带的自证用例失败

#### Scenario: Core 引入平台依赖

- **WHEN** Core 的依赖清单或源码中出现对平台框架、播放器实现或界面框架的依赖
- **THEN** 守卫失败并指出具体依赖与位置

#### Scenario: 违规不以上述形式出现

- **WHEN** 违规没有写成被扫描的前缀形式，而是行内全限定路径或其他等价写法
- **THEN** 守卫能够拒绝它；守卫无法覆盖的写法 MUST 在守卫中以已知盲区显式记录，不得默认视为已覆盖

### Requirement: 静态检查配置的继承与收窄

系统 MUST 让每个 crate 继承 workspace 统一的静态检查配置；任一 crate MUST NOT 在 crate 级丢弃 workspace 的检查集合。确需放宽某项检查时，放宽 MUST 收窄到实际需要它的最小模块，并附安全或必要性说明。

#### Scenario: crate 级丢弃 workspace 检查

- **WHEN** 某个 crate 以 crate 级配置覆盖或绕开 workspace 的静态检查集合
- **THEN** 门禁失败并指出该 crate

#### Scenario: 需要放宽的模块被收窄

- **WHEN** 某个模块确需放宽某项检查并已在模块级声明与记录前提
- **THEN** 该模块通过，且同一 crate 内其余模块仍受 workspace 完整检查约束

### Requirement: 门禁不得静默通过

系统 MUST 保证任何验证命令在"实际未验证任何东西"时失败。以过滤器或名称选择的测试命令 MUST 断言至少执行了一个测试，零匹配 MUST 视为失败；场景或任务登记的验证命令 MUST 指向真实存在的测试或检查。

场景的验收命令 MAY 按域聚合——多个场景共享一次批量执行——但每个场景 MUST 显式声明它依赖的验收证据（一条或多条），且每一条证据 MUST 满足以下全部条件：目标在仓库中真实存在；目标在聚合命令的实际执行输出中被观测到执行过；目标与场景 THEN 子句所依赖的行为直接相关。仅凭"命令里带了过滤器"或"场景被列在某个批量命令下" MUST NOT 被计为已验证。聚合 MUST NOT 使任何场景的证据检查退化为恒真。

场景或任务 MAY 将其验收显式委托给另一个持续集成 gate，但委托 MUST 指向在持续集成中真实执行的 gate；委托 MUST NOT 成为跳过检查的通道。被委托的 gate MUST 在 `ci.yml` 的某个 job 中有执行位置，且委托关系 MUST 由静态门禁校验。

依赖审计的在线公告扫描 MAY 在 Pull Request 上跳过，但 MUST 在推送到 `main` 与定时任务上真实执行。跳过时 MUST 在日志中打印显式 note，说明在线半未执行；MUST NOT 以不输出任何说明的方式静默略过。该分层 MUST 由结构门禁断言：在线扫描的执行条件、它在 `main` 与定时任务中的真实执行位置，以及跳过 note 的存在性 MUST 被静态校验。

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

#### Scenario: PR 跳过依赖审计在线半

- **WHEN** 一次 Pull Request 运行依赖审计
- **THEN** 离线半真实执行，在线公告扫描跳过，且日志打印显式 note 说明在线半未执行

#### Scenario: 在线扫描从 main 与定时任务同时消失

- **WHEN** 依赖审计的在线半既不在 `main` 上执行，也没有定时任务承载它
- **THEN** 结构门禁失败并指出该缺口，而不是把 PR 跳过当作永久豁免

#### Scenario: 分层退化为永久跳过

- **WHEN** 在线扫描的执行条件被写成常量假值或被删除，使其在任何事件与任何定时任务上都不再执行
- **THEN** 结构门禁失败并指出该执行条件，而不是让跳过 note 掩盖未执行的事实

### Requirement: 作为场景验收命令的检查必须自带可执行的失败证明

`scripts/verify/scenario-commands.mjs` 中任何以自包含检查脚本（`node scripts/verify/checks/<…>.mjs`）承担的验收命令，其目标脚本 MUST 在注入证明套件中登记至少一条条目；该条目 MUST 通过**真实制造该场景 THEN 子句所依赖的条件被违反**使脚本退出非 0。

**豁免必须按断言判断，不得按文件分类。** 一个检查同时「委托子进程」与「在自身内部断言」时（例如既跑 cargo 又对源码做结构性判断），MUST 只为**自包含的那部分断言**登记注入证明；MUST NOT 因为文件里出现过子进程调用就整体豁免。委托部分（子进程退出码必须导致检查失败）继续由静态规则覆盖，两者不互相替代。

仅断言编译成功、或仅做结构性 token 存在性判断的检查 MUST NOT 单独承担一条场景的验收：这类检查对它所守卫的形态变化不敏感（存在性判断不随取值改变），恒绿。

这条要求与 `门禁不得静默通过` 的「检查无法被证明会失败」同源，但补上了它未覆盖的两种形态：**检查本身是可失败的，只是失败条件与该场景无关**；以及**检查的断言被豁免规则整体跳过**。二者都比「没有门禁」更危险——它们让对账通过、让覆盖看起来完整。

#### Scenario: 场景命令指向无注入条目的检查

- **WHEN** 某条场景的验收命令指向一个自包含检查脚本，而该脚本在注入证明套件中没有条目
- **THEN** 治理门禁失败并指出该场景标识与脚本路径，不允许「先登记命令、事后补证明」

#### Scenario: 混合形态的检查被整体豁免

- **WHEN** 某检查既调用子进程、又在自身内部做断言，而完整性判定因「文件里出现子进程调用」将其整体归入豁免桶
- **THEN** 该判定失败并列出那些内部断言未被证明的检查；豁免 MUST 只覆盖其委托部分

#### Scenario: 检查无法证伪其场景

- **WHEN** 某检查只断言编译成功或结构性 token 存在性，无法在其守卫的条件被真实违反时失败
- **THEN** 该检查不得作为该场景的验收命令；必须改用能证伪场景 THEN 子句的命令，或把该场景明确降级为人工举证并在举证手册中写出可执行步骤

#### Scenario: 注入条目只违反脚本自身的输入

- **WHEN** 注入证明条目的机制是让脚本收到格式非法的输入（而非让仓库真实违反该检查守卫的条件）
- **THEN** 该条目不计为有效证明，检查的有效性仍为未知

### Requirement: 规格追溯的完整性与阻断

系统 MUST 保证规格、追溯记录与场景清单三方一致：三方数量 MUST 相等，场景标识 MUST 逐一对应，且每个已登记场景 MUST 有可执行的验证命令。对账 MUST 以非零结果阻断后续流程。每个规格领域 MUST 纳入登记表——未被登记的领域必须使门禁失败，而不是被静默排除在追溯体系之外。

#### Scenario: 三方数量不一致

- **WHEN** 规格、追溯记录与场景清单的数量不相等
- **THEN** 对账失败、以非零结果退出，并使发布门禁失败

#### Scenario: 场景缺少验证命令

- **WHEN** 某个已登记场景没有对应的可执行命令
- **THEN** 对账失败并指出该场景标识

#### Scenario: 新增规格领域未登记

- **WHEN** 引入一个新的规格领域目录而未将其加入登记表
- **THEN** 门禁失败并指出该领域，而不是静默忽略其全部场景

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

### Requirement: 代码与抽象规模上限

单个源文件 MUST NOT 超过 1000 行；超过 MUST 拆分。生成代码、产品原型与编译产物除外。单一公开接口 MUST NOT 超过 6 个方法；超过 MUST 按调用方能力拆分。确需保留的超限项 MUST 显式登记在白名单中，白名单 MUST 只减不增。

#### Scenario: 新增超限文件或接口

- **WHEN** 新增或扩展出一个超过上限的源文件或公开接口，且未登记在白名单
- **THEN** 规模门禁失败并指出该文件或接口

#### Scenario: 白名单被扩大

- **WHEN** 一次改动向白名单新增条目
- **THEN** 门禁失败；收缩白名单则通过

#### Scenario: 拆分保持公共入口稳定

- **WHEN** 一个超限文件或接口被拆分
- **THEN** 既有调用方无需修改其导入路径或调用形式

### Requirement: 跨边界契约的生成与漂移守卫

跨边界调用的命令契约 MUST 由生成器产出并纳入版本控制，且 MUST 有漂移门禁保证手工声明与生成物一致。每个跨边界命令的返回值 MUST 是具体类型；MUST NOT 以未知类型或动态对象作为返回值契约。

#### Scenario: 手工契约与生成物不一致

- **WHEN** 手工维护的调用契约与生成物不一致
- **THEN** 漂移门禁失败

#### Scenario: 命令返回未知类型

- **WHEN** 某个跨边界命令的返回值契约仍是未知类型
- **THEN** 契约门禁失败并列出该命令

#### Scenario: 命令返回值变化

- **WHEN** 某个命令的参数或返回类型发生变更
- **THEN** 生成物随生成器更新，调用点因类型检查失败而必须同步调整

### Requirement: 构建纯净性、门禁可执行性与工具链一致

仅用于测试的替身、桩与测试数据库 MUST 以条件编译或特性开关门控，MUST NOT 进入默认生产构建。核心门禁集 MUST 在持续集成中执行，并 MUST 能在干净检出上直接运行；发布门禁 MUST NOT 依赖仓库之外的手工产物。持续集成与本地构建 MUST 使用仓库钉住的同一工具链版本，使构建可复现。

#### Scenario: 测试替身进入默认构建

- **WHEN** 默认构建的产物中包含仅用于测试的替身或桩
- **THEN** 构建纯净性门禁失败并指出该文件

#### Scenario: 干净检出上运行发布门禁

- **WHEN** 在一个刚克隆且未做过任何手工准备的检出上运行发布门禁
- **THEN** 门禁能完整执行，不因缺少仓库外文件而中止

#### Scenario: 核心门禁未接入持续集成

- **WHEN** 核心门禁集未被持续集成执行
- **THEN** 该缺口被视为交付阻断项，而不是可接受状态

#### Scenario: 持续集成使用与仓库钉住不同的工具链

- **WHEN** 持续集成安装并激活的编译器版本与仓库钉住的版本不一致
- **THEN** 该不一致被指出并对齐到仓库钉住的版本，避免出现"本地钉住、持续集成浮动"而削弱可复现构建

### Requirement: 跨边界调用的失败必须被处理

界面层 MUST 显式处理跨边界调用的失败：要么向用户呈现可理解的反馈，要么上报到可观测通道；MUST NOT 静默丢弃。调用点 MUST 能区分"需要结果"与"不等待结果"两种意图，后者也 MUST 以显式方式吞掉并记录错误，而不是以裸丢弃表达。

#### Scenario: 跨边界调用失败但无人知晓

- **WHEN** 一次跨边界调用失败，包括调用的命令或处理器不存在
- **THEN** 用户看到可理解的反馈，或该失败被记录；MUST NOT 出现"用户点了但毫无反应且没有任何记录"

#### Scenario: 以不等待结果的方式发起调用

- **WHEN** 调用点不需要该调用的结果
- **THEN** 写法必须显式表达已吞掉错误；裸丢弃失败的门禁拒绝该写法

### Requirement: 应用状态的单一原语与模块公开入口

应用状态 MUST 通过统一的共享原语暴露，保持一致的订阅与快照契约；MUST NOT 用模块级可变全局状态作为跨模块状态载体。每个功能模块 MUST 通过公开入口暴露对外契约，跨模块引用 MUST 只指向公开入口；测试基建 MUST NOT 依赖功能模块的内部文件。

#### Scenario: 新增状态模块引入第二种订阅契约

- **WHEN** 新增一个状态模块，而它具有与既有共享原语不同的订阅或快照契约
- **THEN** 门禁失败并指出现有原语；新增状态必须复用既有原语

#### Scenario: 跨模块引用内部文件

- **WHEN** 一个功能模块引用了另一功能模块的内部文件而非其公开入口
- **THEN** 门禁失败并指出该引用

#### Scenario: 测试基建依赖功能模块内部

- **WHEN** 测试基建或测试初始化代码直接引用功能模块的内部文件
- **THEN** 门禁失败；该能力必须由公开入口或共享原语提供

### Requirement: OpenSpec change 必须在创建 Pull Request 前同步并归档
每项通过 OpenSpec 管理的变更 MUST 在创建 Pull Request 前完成实现任务和规定验证。存在 delta spec 时 MUST 先将其同步到对应主规格并验证同步结果，然后归档 change；没有 delta spec 时 MUST 在创建 Pull Request 前归档 change。未归档的 change MUST NOT 进入 Pull Request 创建阶段。

#### Scenario: 存在 delta spec 的变更准备提交 Pull Request
- **WHEN** OpenSpec change 的实现任务和规定验证均已完成且存在 delta spec
- **THEN** 维护者先同步 delta 到主规格并验证，再归档 change，之后才创建 Pull Request

#### Scenario: 没有 delta spec 的变更准备提交 Pull Request
- **WHEN** OpenSpec change 的实现任务和规定验证均已完成且没有 delta spec
- **THEN** 维护者先归档 change，之后才创建 Pull Request

#### Scenario: change 尚未归档
- **WHEN** 维护者准备创建 Pull Request 但对应 OpenSpec change 仍在活动目录
- **THEN** 交付流程要求先完成必要的规格同步和归档，Pull Request 模板明确提示该状态

### Requirement: 归档与文档一致性

变更归档前 MUST 确认其任务已全部完成；确需遗留的任务 MUST 显式登记为延期或从范围中移除，MUST NOT 在未勾选状态下静默归档。面向维护者的架构与模块说明 MUST 只引用真实存在的模块、入口或检查；文档与实现不一致 MUST 在审查阶段被修正。

#### Scenario: 存在未完成任务时归档

- **WHEN** 一次变更仍含未勾选任务而尝试归档
- **THEN** 归档被拒绝，并指出未完成的任务项

#### Scenario: 文档引用不存在的模块

- **WHEN** 架构说明或模块注释引用了仓库中不存在的模块、目录或入口
- **THEN** 该不一致被指出并修正，使说明与实现一致

### Requirement: 代码变更的规格复核与残留清理

涉及生产代码、架构边界或跨边界契约的 OpenSpec change MUST 在 design 与受影响 capability spec 中记录适用的代码规范和架构规范、所属层级、依赖方向、兼容性影响及可执行验证命令。实现前 MUST 基于当前 HEAD 复核规格与实现；审计数值、模块路径和行为结论 MUST 可由命令重跑，不得把已失效的快照当作施工前提。重构或迁移完成后 MUST 清理被替代的实现、无定义模块之外消费者的导出、失效测试替身与不再被解析的登记资产；确有保留必要时 MUST 记录消费者或白名单理由。

#### Scenario: 新增或修改生产代码

- **WHEN** 一个 change 新增或修改生产代码、架构边界或跨边界契约
- **THEN** 其 design 与受影响 capability spec 明确列出适用规范、层级和依赖方向、兼容性影响、验证命令及待清理残留；缺少任一项不得进入实施

#### Scenario: 规格与当前实现不一致

- **WHEN** 实施前复核发现规格中的行为、路径、数量或架构结论与当前 HEAD 不一致
- **THEN** 先以可复跑证据修正规格和任务，再实施代码；不得按过期结论施工

#### Scenario: 重构后存在残留实现

- **WHEN** 新实现已替代旧实现，而旧模块、导出、测试替身或登记资产不再有有效消费者
- **THEN** 该残留被删除，或以明确消费者与保留理由登记；引用检查、静态检查和相关测试证明结果一致

### Requirement: CI 执行分层与结构完整性

持续集成 MUST 按事件类型分层执行验证，且分层 MUST NOT 使任何检查从 Pull Request 与 `main` 同时消失。Pull Request MUST 执行 Linux 的 `fmt` / `clippy`、Linux 全量 workspace 测试与各平台 Gate。Windows / macOS 的全量 workspace 测试、`fmt` 与 `clippy` MUST 在推送到 `main` 时执行；在 Pull Request 上 MAY 被跳过。任何在 Pull Request 上被跳过的检查 MUST 在 `main` 上真实执行，并由结构门禁断言。平台 Gate 在 Pull Request 上 MUST 编译并测试该平台相关代码，因此 macOS / Windows 的 `fmt` / `clippy` 虽不在 PR 上执行，平台相关的编译覆盖 MUST NOT 从 PR 上消失。

同一检查 MUST NOT 在多个 job 中重复执行，除非第二次执行覆盖了第一次无法覆盖的条件（不同平台、不同 feature 形态、不同运行环境）。重复执行 MUST 由结构门禁反向锁定：把一个已被其他 job 覆盖的步骤重新加回时，结构门禁 MUST 失败并指出该重复。

作为其他检查的前置输入而执行的构建 MUST NOT 被视为重复门禁：它的失败会阻断该 job 的真实检查，而不是独立宣告某个质量维度（例如 Rust 分片为 `generate_context!` 构建的 `apps/desktop/dist`，与前端 job 的 `Build` 步骤目的不同）。

场景的验收命令 MAY 显式委托给另一个在持续集成中真实执行的 gate，但委托 MUST 由场景登记项声明，MUST NOT 由命令形态推断。被委托的 gate MUST 在 `ci.yml` 中有一条可定位的执行命令：`workspace-test` 对应 Linux `rust` job 的 `cargo test --workspace --all-targets --all-features`，`frontend-test` 对应 `frontend` job 的完整 Vitest 套件。结构门禁 MUST 校验每个委托 gate 的执行命令真实存在，且该命令 MUST 与场景登记的域一致。

Windows / macOS 的全量 workspace 测试 MAY 按包分片执行；各分片所覆盖包的并集 MUST 等于全部 workspace 包，MUST NOT 因分片遗漏或删除任何包。分片引入的跳过 MUST 遵守与整段测试相同的分层规则：分片 MAY 仅在 Pull Request 上跳过，在 `main` 上 MUST 全部真实执行，且结构门禁 MUST 断言分片并集与各分片在 `main` 上的执行条件。将 macOS / Windows 的多个分片合并为一个 `--workspace` 分片 MUST NOT 改变包并集。

`clippy` MUST 在三平台以 workspace 形态执行，MUST NOT 收窄到单个 crate 或包子集：Linux 在 `rust` job 的 `workspace` 分片执行，macOS / Windows 在 main-only 的 `rust-desktop` job 执行。写入共享缓存的 rust 矩阵分片（`rust` 与 `rust-desktop` 两个 job 的分片），其最后一条构建型 cargo 调用 MUST 是该缓存 key 对应的固定 cargo 形态：Linux `workspace` 分片为 `cargo test --workspace --all-targets --all-features`，macOS / Windows 的 `desktop` 分片为 `cargo test --workspace --all-targets --all-features`；其后 MAY 执行同形态的 `-- --list`，MUST NOT 在缓存保存之前执行不同 feature、不同 target 选择或 check 形态的 cargo 调用，使缓存内容与后续读取者的构建形态不符。`governance-static` 的缓存 key 是 job 私有的：只有该 job 与定时依赖审计 workflow 读取它，其 coverage 插桩形态不会污染任何其他读者，因此不受上述 cargo 形态约束。缓存写入 MUST 只发生在 `main`；Pull Request 上的任何 job MUST NOT 保存缓存。

性能场景 MUST 在 Pull Request 与 `main` 上真实执行，且 MUST 由结构门禁断言其执行位置：承载它的 job MUST 在两种事件上都运行，其步骤 MUST 执行 `pnpm verify:perf`。性能场景 MAY 与 task gate 共享一个 job；共享时该 job 的缓存步骤 MUST NOT 保存缓存，避免 `--release` 产物污染其他读者读取的缓存 key。

coverage 门禁 MAY 在 Pull Request 上跳过，但 MUST 在推送到 `main` 时真实执行；结构门禁 MUST 断言其 main-only 执行条件与命令。Pull Request 上 MUST 继续执行 registry 与 static 检查。

治理门禁的注入证明套件（mutation-test 各门禁在违规时确实失败）MAY 作为本地命令维护，MUST NOT 作为 Pull Request 的 job 执行；结构门禁 MUST 断言 `ci.yml` 中没有任何 job 执行该套件，同时 MUST 保留对 proof 登记的静态校验。

CI 的验证结构 MUST 可被静态校验：task 分片的并集 MUST 覆盖全部未被标记 `archived: true` 且声明了 CI 执行位置的 task，且无重复；`archived: true` 的 task MUST NOT 被要求出现在 task 分片中，其断言 MUST 由真实执行的 workspace test、frontend test 或平台 Gate 承载，且 manifest 的归档理由 MUST 指出承载 gate。governance 的每个 section MUST 在 `ci.yml` 中有执行位置；场景委托 MUST 指向真实执行的 gate；删除、重命名或移动 job / step 导致上述结构不成立时，结构门禁 MUST 失败。

纯文档变更 MAY 通过 `paths-ignore` 跳过 CI，但忽略清单 MUST NOT 包含任何门禁输入（例如 `docs/traceability.md`、`docs/acceptance/PRD-matrix.md`、`docs/LOGGING.md`、`docs/native-attestation-playbook.md`，以及被 gate 读取的 `README.md`、归档 OpenSpec 证据与 `artifacts/` 记录）。结构门禁 MUST 把忽略清单固定为已审计的集合，新增或删除条目 MUST 失败。

#### Scenario: PR 与 main 的分层覆盖

- **WHEN** 一次 Pull Request 运行持续集成
- **THEN** Linux 的 `fmt` / `clippy`、Linux 全量 workspace 测试与各平台 Gate 实际执行；Windows / macOS 的全量 workspace 测试、`fmt` 与 `clippy` 可以跳过
- **WHEN** 同一变更推送到 `main` 后运行持续集成
- **THEN** Windows / macOS 的全量 workspace 测试、`fmt` 与 `clippy` 实际执行，而不是保持 skipped

#### Scenario: 检查从 PR 与 main 同时消失

- **WHEN** 某个检查在 Pull Request 上被跳过，且 `main` 上也没有执行它的 job 或 step
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

#### Scenario: Pull Request 写入缓存

- **WHEN** 非 `main` 的 run 中任一 job 的缓存步骤允许保存缓存
- **THEN** 结构门禁失败并指出该 job，而不是让 PR 缓存继续占用配额

#### Scenario: 性能场景从 PR 与 main 同时消失

- **WHEN** 承载性能场景的步骤被删除、改写成不执行 `pnpm verify:perf`，或落到一个在 Pull Request 上不运行的 job
- **THEN** 结构门禁失败并指出该缺口，而不是让 3 个委托场景失去执行位置

#### Scenario: 性能场景与 task gate 共享 job 时保存缓存

- **WHEN** 承载性能场景的 job 的缓存步骤允许保存缓存，使 `--release` 产物写入其他读者读取的 key
- **THEN** 结构门禁失败并指出该 job

#### Scenario: 注入证明套件被重新加回 PR

- **WHEN** `ci.yml` 中任一 job 的步骤执行 `injection-suite.mjs`
- **THEN** 结构门禁失败并指出该 job，而不是让门禁自身的 mutation 测试重新占用 PR 配额

#### Scenario: 纯文档 PR 触发全量 CI

- **WHEN** `paths-ignore` 清单新增或删除了条目，或包含一个门禁输入文件
- **THEN** 结构门禁失败并指出该条目，而不是让文档提交继续消耗全量 CI 或让门禁输入被跳过

### Requirement: 公开错误类型可进入错误链

从公开函数返回的错误类型 MUST 实现 `Display` 与 `std::error::Error`，使调用方能够以 `?` 传播、向用户呈现原因，并把它接入既有错误链。错误类型 MUST 保持可穷举匹配的既有形态（具体枚举而非不透明包装），MUST NOT 为了满足该要求而改为字符串或动态错误。

`Display` 文本 MUST 说明失败的语义类别，MUST NOT 只重复类型名，也 MUST NOT 包含敏感数据。该要求适用于共享 Core 中跨 crate 可见的领域错误；仅在同一模块内使用的内部错误不受此约束。

#### Scenario: 领域错误从公开函数返回

- **WHEN** 一个共享 Core 的公开函数以领域错误枚举失败，调用方希望用 `?` 把该失败并入自己的错误类型
- **THEN** 该领域错误实现 `std::error::Error` 与 `Display`，可直接被传播与呈现，无需调用方逐变体手写映射

#### Scenario: 新增变体时下游编译失败

- **WHEN** 一个已被调用方匹配的公开错误枚举新增变体
- **THEN** 下游必须显式处理新变体，MUST NOT 因错误类型被包装成不透明字符串而静默漏处理

#### Scenario: 错误文本不泄露敏感信息

- **WHEN** 领域错误被记录到日志或展示给用户
- **THEN** `Display` 输出只说明失败类别，不包含配对码、凭据、绝对路径或文件内容
