# Echo 代码规范

> 状态：初始规范。适用于后续 OpenSpec change 与代码实现；若规范需要调整，应在对应 change 的 design 中说明原因，并同步更新本文档。

## 1. 适用范围与优先级

- 本规范覆盖 Rust Core、Tauri 桌面适配层、React/TypeScript 桌面 UI，以及后续 Flutter/Dart 移动端。
- 产品范围以 `docs/PRODUCT.md` 和 `docs/ROADMAP.md` 为准，架构边界以 `docs/DESIGN.md` 为准。
- OpenSpec change 负责记录单次变更的具体行为、设计和验收；不得用局部实现绕过项目级约束。
- 当文档、OpenSpec 与实现冲突时，先确认预期并更新规格，再修改代码。

## 2. 通用规范

- 代码标识符使用英文；产品术语遵循 `docs/interface-terminology.md`，同一概念不得在不同端使用不同名称。
- 模块保持单一职责，公共接口最小化；跨层通信使用明确类型，不传递无结构的 map 或动态对象。
- 注释解释约束、原因和取舍，不复述代码。公开 API、协议字段和恢复逻辑应有必要文档。
- 不在业务路径使用未处理的 panic、强制断言、静默失败或空 catch；错误必须保留上下文并在合适边界转换。
- 日志采用结构化字段，不记录凭据、访问令牌、完整连接串或不必要的用户文件信息。
- 新增依赖必须有明确用途，优先复用已有能力；锁文件应提交并保持可复现构建。
- 生成代码只通过对应生成器更新，不手工修改。

## 3. 架构与依赖方向

```text
React UI ── Tauri command/event ── echo-desktop ── echo-core
Flutter UI ── flutter_rust_bridge ──────────────── echo-core

平台播放器：desktop → mpv；mobile → media_kit
同步连接器：S3/WebDAV → RemoteConnector 抽象
```

- `echo-core` 只包含模型、资料库、扫描、索引、导入、同步和冲突裁决，不依赖 Tauri、mpv、Flutter 或 UI 类型。
- 播放器实例、播放队列和设备播放状态属于平台层，不得进入 Core。
- `echo-desktop` 负责 Tauri command/event、系统集成和 mpv 适配；UI 不直接访问 SQLite、文件系统或播放器原生接口。
- Flutter 通过 `flutter_rust_bridge` 使用 Core；不得在 Dart 中复制同步、去重或冲突裁决规则。
- S3 与 WebDAV 必须实现统一的 `RemoteConnector` 契约；业务逻辑不得依赖具体连接器。
- 跨边界 DTO 与领域模型分离。IPC、Binding 或远端协议变化必须在 OpenSpec design 中说明兼容影响。

### 3.1 分层规则

代码必须按职责分层，依赖只能由外向内：

```text
Presentation / Platform
          ↓
Application / Use Cases
          ↓
Domain
          ↑
Infrastructure（通过 Port 接入）
```

- **Domain**：实体、值对象、领域规则和领域服务；不得依赖数据库、文件系统、网络、UI 或具体框架。
- **Application**：组织用例、权限与流程、事务边界及 Port；不得包含视图逻辑或具体基础设施实现。
- **Infrastructure**：实现 SQLite、文件系统、标签解析、S3/WebDAV 等 Adapter；不得反向定义业务规则。
- **Presentation / Platform**：React、Flutter、Tauri command/event 和播放器适配；只负责输入输出、状态呈现与平台能力。
- 上层通过明确接口调用下层，不得跨层直接访问实现。例如 UI 不得直接执行 SQL，连接器不得直接操作视图状态。
- 数据跨层时应显式转换；数据库记录、远端 DTO、IPC DTO 与领域模型不得混为同一类型。
- 事务边界由 Application 用例协调，具体事务能力由 Infrastructure 提供；领域逻辑不得自行打开数据库连接。

### 3.2 抽象原则

- 遵循 SOLID、依赖倒置和组合优于继承；高层业务依赖 Port/trait，而不是依赖具体数据库、协议或框架。
- 抽象应围绕稳定的业务边界或真实变化点建立，例如 `RemoteConnector`、资料库 Repository、文件操作 Port、播放器 Adapter。
- 接口保持小而聚焦，按调用方能力拆分；不得创建包含大量无关方法的万能 Service、Manager 或 Repository。
- 依赖通过构造参数或明确装配层注入；不得使用隐藏依赖的全局可变状态、Service Locator 或随处可取的单例。
- 公共抽象至少应有两个合理实现、测试替身需求或明确的架构边界；不要只为包裹一次函数调用而增加一层接口。
- 优先复用领域语言形成类型和模块，不以技术名词替代业务概念。

### 3.3 设计模式

应根据问题选择设计模式，并在 OpenSpec design 中说明关键模式及其解决的问题：

- **Ports and Adapters / Hexagonal Architecture**：隔离 Core 与 UI、数据库、文件系统、远端协议及播放器。
- **Repository**：封装领域对象的 SQLite 持久化与查询，避免 SQL 泄漏到用例和 UI。
- **Strategy + Adapter**：统一 S3/WebDAV `RemoteConnector`，以及桌面与移动播放器的不同实现。
- **Command / Event**：用于 Tauri 边界和跨层通知；command 表达请求，event 表达已经发生的状态变化。
- **State Machine**：用于同步、扫描、导入恢复和播放器等具有明确状态与转换约束的流程，禁止用散落布尔值组合隐式状态。
- **Unit of Work / Transaction Script**：协调单个用例内的 SQLite 原子提交；跨文件系统操作配合 `operation_journal`，不得伪装成单一事务。
- **Factory / Builder**：仅用于具有多种实现或复杂校验的对象构造，简单结构体不需要模式包装。

设计模式服务于边界、变化和可测试性，不得为了“使用模式”增加无业务价值的层级、trait 或样板代码。

## 4. Rust

### 4.1 格式与静态检查

- 使用稳定版 Rust 工具链和 workspace 统一配置；提交前通过 `cargo fmt --check`。
- 提交前通过 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- `echo-core` 默认不使用 `unsafe`；FFI 或播放器适配确需 `unsafe` 时，应隔离在最小模块并记录安全前提。

### 4.2 API 与错误

- 库和领域边界使用 `thiserror` 定义可匹配的错误类型；应用装配层可用 `anyhow` 附加上下文。
- 运行时业务路径不得使用 `unwrap()` 或无说明的 `expect()`；测试和已由类型证明不可能失败的位置除外。
- 公共类型和函数应表达领域含义，避免布尔参数堆叠；必要时使用 enum、新类型或配置结构体。
- Core 对外接口不得暴露 Tauri、mpv、Flutter、S3 或 WebDAV 的具体类型。

### 4.3 异步、阻塞与并发

- `tokio` 异步任务中不得直接执行长时间文件扫描、哈希、标签解析或其他阻塞工作；使用受控的阻塞任务或专用执行层。
- 不跨 `.await` 持有数据库事务、锁或文件句柄；共享状态应限制作用域并明确所有权。
- 后台任务必须支持错误回传和可控结束；不得创建无法追踪生命周期的任务。
- 同步、扫描和导入操作应可重试或幂等，并以稳定 UUID、版本和操作日志避免重复副作用。

## 5. SQLite、文件与同步

- SQLite 访问集中在 Core 的存储层，SQL 不散落在 UI、Tauri command 或连接器中。
- 关联数据在单个 SQLite 事务中提交；文件系统与数据库的跨资源操作使用 `operation_journal` 恢复，不假设二者原子提交。
- 数据库结构通过顺序迁移演进；已发布迁移不得直接改写。schema 变化必须包含升级、失败恢复和兼容测试。
- 业务关联使用稳定 UUID；本机绝对路径不得进入远端协议，歌单和队列不得以文件路径作为身份。
- 远端只保存版本化 JSON 变更记录和媒体文件，不上传或合并 SQLite 数据库。
- 一份资料库只绑定 S3 或 WebDAV 中的一个远端；更换连接器按一次性全量迁移处理，不同时双写。
- 同步首次全量、之后增量，播放前保证媒体已落盘，不实现边播边传的流式读取。
- 远端协议字段必须可序列化、可版本化并有兼容策略；未知字段应尽可能向前兼容。
- 冲突采用最后写入者胜，以单调递增版本号为主、本地时间戳兜底，不以墙钟直接裁决。
- 墓碑、版本裁决、outbox 与连接器行为必须支持重复执行；连接器实现应通过相同的契约测试。
- 导入和写回不得覆盖已有文件；临时写入完成解析与校验后再原子替换或移动。

## 6. React 与 TypeScript

- TypeScript 启用 `strict`；不得以 `any`、非空断言或类型强转常态化绕过类型检查。边界输入必须验证后再进入应用状态。
- 使用 ESLint 和 Prettier 统一检查与格式；具体脚本由桌面工程初始化时写入 workspace 配置。
- React 组件负责展示和交互编排；资料库、同步、导入和冲突规则留在 Core。
- Tauri commands/events 使用集中定义的类型化接口。command 名称、参数、返回值和错误结构变化视为接口变更。
- effect 必须声明完整依赖并清理订阅、计时器和事件监听；异步结果写入状态前处理取消或过期响应。
- 组件保持可测试，避免把数据访问、事件订阅和复杂视图全部放在单个组件中。
- 用户可见状态必须覆盖加载、空数据、失败、离线或资料库不可用等路径，不能只实现成功态。

## 7. Flutter 与 Dart

- 使用 Dart null safety、`dart format` 和 `flutter analyze`；不得通过动态类型复制 TypeScript 或 Rust 的领域对象。
- `flutter_rust_bridge` 生成层只负责传输；业务规则留在 Rust Core，Dart 层负责移动端 UI、生命周期和 media_kit 播放适配。
- Binding DTO 变化必须与 Rust API 同步生成并测试；不得手工编辑生成文件。
- 平台权限、后台播放和文件访问封装在平台适配层，不渗入共享领域模型。

## 8. 测试与交付门槛

- Rust Core 单元测试覆盖率目标不低于 90%；领域规则修复必须包含能复现问题的测试。
- Core 重点覆盖：扫描与去重、路径与 UUID 稳定性、SQLite 事务和迁移、operation journal 恢复、同步幂等、版本冲突与墓碑。
- 文件测试使用临时目录和固定样本，不依赖开发机绝对路径、真实音乐库或真实远端账户。
- S3/WebDAV 通过共享连接器契约测试；网络失败、超时、重复请求和部分成功必须有测试。
- Tauri command、Rust Binding 和远端 JSON 属于边界接口，应包含序列化与集成测试。
- React/Flutter 至少覆盖关键状态与用户路径；核心流程按 change 的验收场景补充端到端验证。
- 涉及路径、文件监听或媒体解析的变更，应考虑 macOS、Windows、Linux 的路径分隔符、大小写、Unicode 和权限差异。
- OpenSpec tasks 必须列出实际验证命令。工程建立后，最低检查包括对应栈的 format、lint/analyze、type-check、test 和 build。

## 9. OpenSpec 对代码变更的要求

以下变更必须在 design 中明确兼容和迁移方案：

- Core 公共 API、Tauri command/event 或 Flutter Binding 变化；
- SQLite schema、迁移、索引或事务边界变化；
- 远端 JSON 格式、版本规则、冲突策略或连接器契约变化；
- UUID、哈希、文件路径、导入恢复或墓碑语义变化；
- 新增跨平台依赖、FFI、`unsafe` 或长期后台任务。

每个实现任务应能追溯到 requirement/scenario，并包含正常路径、关键失败路径和回归验证。

design 必须说明变更落在哪一层、依赖方向、复用的抽象及选用的设计模式；新增跨层依赖或绕过既有抽象必须给出理由。代码审查应拒绝职责混杂、反向依赖、基础设施泄漏和无依据的重复实现。
