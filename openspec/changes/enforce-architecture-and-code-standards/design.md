## Context

动机见 `proposal.md`。以下是影响方案选择的当前状态与约束，均在提交 `51c8f2f` 上逐条复验过。

**规范侧。** `openspec/CODE_STANDARDS.md` 已给出全部所需阈值：§2.1 单文件一般 ≤500 行、>1000 行必须拆分；§3.2 接口须小而聚焦、不得造万能 Service；§4.1 `echo-core` 默认不用 `unsafe`，确需时应隔离在最小模块并记录安全前提；§6 Tauri commands/events 使用集中定义的类型化接口。缺的是把这些阈值变成会失败的检查。

**偏离侧（实测）。**

| 项 | 事实 |
|---|---|
| lint 继承 | `crates/echo-desktop/Cargo.toml:33-34` 声明 `[lints.rust] unsafe_code = "allow"` 且无 `lints.workspace = true` → 整包丢失 workspace 的 `clippy::all/pedantic/nursery`；`echo-core:44-45` 与 `src-tauri:33-34` 均正确继承 |
| 分层漂移 | 播放上下文解析在 `echo-desktop/src/runtime/services.rs:94-155` 与 `:165-184`；恢复裁决在 `:189-227`；歌曲绝对路径重建在 `services.rs:931` 与 `player.rs:222`。以上均为纯领域规则，无 DB/mpv/Tauri 类型 |
| 死代码 | `echo-core/src/domain/catalog.rs:327/339/351` 已定义 `PlaybackContextRequest`/`ViewRef`/`PlaybackContextResolved`，但全仓无使用点；平台层另用 `view: &str` 自建 |
| 守卫不对称 | `crates/echo-core/tests/arch.rs`（277 行）只约束"Core 不得依赖平台"；无 `crates/echo-desktop/tests/arch.rs`。「领域规则漏进平台层」无守卫 |
| 规模 | 按总行数口径有 15 个 Rust 源文件 >1000 行（最大 `application/import.rs` 3,684 行，其中约 2,610 行为内联测试；`player/actor.rs` 2,670；`runtime/services.rs` 1,591）；`application/ports.rs` 956 行 26 个 trait，`TxAccess` 与 `LibraryFileSystem` 各 17 方法 |
| 契约 | `apps/desktop/src/bridge/index.ts` 的 `BridgeCommandMap` 有 39 个命令条目，其中 38 个返回 `unknown`，仅 `get_close_behavior` 具有具体返回类型；生成器 `crates/echo-desktop/src/ipc/generate.rs` 只产出 DTO，不产出命令映射；42 处 `bridge.call(` 无一使用泛型 |
| 门禁 | 场景唯一执行入口 `scripts/verify/run-scenario.mjs:42-50` 只判退出码，85 条场景命令为 cargo 过滤器且无"至少一个测试"断言；`validate-scenario-commands.mjs:24-25` 依赖仓库外 `/tmp/*-lib-tests.txt`，实测不存在 → 运行即 ENOENT；`spec-scenarios.mjs:35-44` 只登记 8 个领域，`openspec/specs/` 有 10 个，`phase-one-acceptance` 与 `portable-library-layout` 的 9 个场景完全脱离追溯；CI 只执行 79 个检查中的 2 个 |
| 对账现状 | `reconcile-scenarios.mjs` **当前为红**（退出码 1）：定稿时实测 `spec 209` / `trace 179` / `manifest 181`，**96 处失配**。三个数字随每次规格变更漂移，门禁必须自行派生 |
| 契约基线 | `BridgeCommandMap` **39** 个命令条目中 **38** 个返回 `unknown`，仅 `get_close_behavior` 具真实返回类型（该条由 `59aeee1` 引入，审计时还是 38/38，已过期） |
| 归档 | `openspec validate --archived` **已能**检出未完成任务并以退出码 1 失败，实测 4 个归档变更共 12 个未勾选任务；但仓库无任何 git 钩子（`.git/hooks/` 仅有 sample、无 husky），该校验从未被执行 |
| 前端状态 | 5 个自研 store、3 种写法、无共享原语（`playerStore.ts` 的 `class`+`useSyncExternalStore` 质量高，`coverArt.ts`/`songUpdates.ts`/`coverPalette.ts` 为模块级可变状态 + `Set`）；`features/*/index.ts` 一个都不存在，跨 feature 直引内部文件约 18 处；`test/setup.ts:82` 反向依赖 feature 内部 |
| 前端错误处理 | 生产路径 **19 处** `void bridge.call(...)`，`invoke` 对未注册命令会抛错，裸 `void` 静默吞掉 |
| Core 内聚 | `error.rs` 823 行单一巨型枚举；`application/testing/` 约 **3,931** 行（`memory_database.rs` 1,013 是完整内存数据库，不是"小 fake"） |
| 工具链 | `rust-toolchain.toml` 钉 `1.96.0`，而 `.github/workflows/ci.yml:27,94` 用 `dtolnay/rust-toolchain@stable` → 本地钉版本、CI 跑浮动 stable |
| 其余资产 | `tests/` 下 **136 个 yaml** 被读取的方式仅为"非空断言"，其字段未被任何脚本解析消费；209 条场景命令仅有 **115 条不同命令（1.82×）**，最高一条被引用 16 次；79 个检查中本轮仅审计少数，其余断言强度为未知有效性 |

**规范的两条既有约束直接限定方案。** §3.3 末句「不得为了'使用模式'增加无业务价值的层级、trait 或样板代码」——因此本变更不引入新的抽象层，只做归位与收窄；§1「当文档、OpenSpec 与实现冲突时，先确认预期并更新规格，再修改代码」——因此本变更先落规格再动代码。

**并行约束。** 审计与规划期间持续有并行会话在改 `player/*`、`runtime/services.rs`、`apps/desktop/src-tauri/*` 与前端。方案必须可分批、可独立回滚。

**复审修正（审计结论在写入规格前逐条复验的结果）。** 审计快照为 `3cb4e5b`（约 03:00），本 change 定稿时为 `51c8f2f`（约 16:00），其间落了 6 个提交；随后又在 `7b972cf`（复核时 HEAD）对**本 change 自身**做了一轮独立对抗式复核，下表数值均为该 HEAD 下的实测。逐条复验后，下列审计结论**不再成立**，因此**未写入规格**；记录于此以便日后审查时不会被重新提出：

| 审计原结论 | 复验结果 |
|---|---|
| `task-13.9` 的三方对账正则不比较数字，故 29 处不一致也能放行 | **半错**。正则确实不比较，但真正的闸门是 `reconcile-scenarios.mjs` 的**退出码**（`process.exit(errors ? 1 : 0)`），而 `run()` 会检查退出码 → 该正则是死代码而非漏洞。仍成立的部分：对账当前为红（定稿时 209/179/181、96 处失配）、13.9 硬编码了过期的场景总数 166（`:74/:75/:85`） |
| 发布门禁 13.9 依赖 `/tmp` 下的 native report / attestation / notarization / signing identity 四个文件 | **推翻**。`scripts/verify/` 内**不存在**这些引用；13.9 全文只有 4 个步骤，唯一的仓库外输入是 `:66-67` 传给场景执行的两个环境变量（`CCORE_TESTS` / `CDESK_TESTS`，指向不存在的 `/tmp/*-lib-tests.txt`）。真实缺陷是"依赖仓库外输入"，不是"依赖四个产物文件" |
| `task-12.1.mjs:29` 与 `task-12.7.mjs:23` 存在 cargo filter 笔误 | **已不存在**。`task-12.1.mjs` 现仅 35 行且已无 `cargo test` 调用；审计所称的测试名在全仓已无对应 |
| 229 处 `cargo test <filter>` 只查退出码 | **范围与数量都需修正**。`scripts/verify/checks/` 中带过滤器的调用**逐条用 `stdout.includes(测试名)` 断言**，无此漏洞；真正的漏洞在**场景执行路径**：`scripts/verify/run-scenario.mjs` 只判退出码，覆盖 85 条 cargo filter 场景 |
| 45 个 `tests/native/*.md` 含全角占位符 `（填写` | 占位符是**半角** `(填写)`。用全角检索得 0，属假阴性 |
| `validate-scenario-commands.mjs` 缺 `artifacts/verify/latest/parent-pid.txt` 等 | 缺的是另外的文件：`/tmp/core-lib-tests.txt` 与 `/tmp/desk-lib-tests.txt`（`readFileSync` 无保护 → ENOENT）。结论（脚本跑不起来）成立，原因需修正 |
| `coverPalette.ts` 是名不副实的"调色板"单例，与 `artworkPalette.ts` 重复 | **部分错**。它是 `coverClass()` 纯函数 + 一个模块级可变**计数 store**，确实职责混杂；但 `artworkPalette.ts` 是独立的像素主色提取（输入、输出、消费者三者皆不同），**不是重复实现**，不应合并 |
| Core 错误类型直接成为跨 IPC 边界的错误载体，与 §3.1 的 DTO 分离有张力 | **推翻**。`crates/echo-desktop/src/ipc/error.rs:52-81` 已把 `CoreError` 显式映射为 `IpcErrorDto`，并有钉住 code 表与 `retryable` 策略的测试，文档注释明确"绝对路径与调试串不跨越该边界"。**边界映射是正确的**，只剩 `error.rs` 自身的 823 行内聚问题 |
| 5 个任务复用同一条宽泛命令（traceability 颗粒度不足） | **基本不再成立**。`scripts/verify/manifest.json` 现有 307 条命令条目、**304 条互不相同**（唯一重复是 `task-3.10.mjs` 被任务 3.10/3.11/3.12/3.13 共用），且细到具体测试名。不需按"宽泛命令"处理 |
| `tests/` 下 136 个 yaml「内容未被任何脚本解析」 | **表述不准确**。它们确实被读取——`reconcile-scenarios.mjs` 会对每个登记路径做"非空"断言；未被消费的是其**字段**。按"无人读取"处理会误删对账门禁的活输入 |
| `PlaybackContextRequest` / `ViewRef` / `PlaybackContextResolved` 全仓无使用点 | **措辞过强**。同文件内有 `impl PlaybackContextRequest` 与自测；准确说法是"无定义模块之外的消费者"。归位后它应成为真实消费者的入口，而不是被当作可随便删除的死代码 |
| `features/library/SongList.tsx` 没有测试 | **不再成立**。`SongList.test.tsx` 已存在 |
| `application/ports.rs` 有 22 个 trait | 现为 **26** 个 |
| `player/actor.rs`（2,616 行）是上帝文件 | **审计自身已更正**：生产仅约 420 行、测试占 84%，不是负债。真正需要拆的是 `queue.rs` 与 `local_state.rs` |

一个元结论也写进 design：**审计结论在写入规格前必须逐条复验**。本次 12 条中有 4 条已过期或错误；若照抄进 spec，会被归档成长期误导的主规格。

同一个教训对本 change 自身同样适用——**方案文本也是会漂移的结论**。本 change 在 `7b972cf` 上被独立复核，逐条比对了 tasks/design 中的每处实测数值，纠正了 8 处（其中 3 处若不纠正会直接导致错误施工）：

| 本 change 原写法 | 复核结果（`7b972cf` 实测） | 若不纠正的后果 |
|---|---|---|
| 「`tests/` 下 136 个 yaml 未被任何脚本读取」 | 被**读取**（`reconcile-scenarios.mjs:86` 对每个路径做非空断言），未被消费的是**字段** | 会删掉对账门禁的活输入，把"红"变成"输入缺失"而崩坏 |
| 对账失配「69 处」 | **96 处**（`spec 209 / trace 179 / manifest 181`） | 低估收敛工作量约 40% |
| `BridgeCommandMap`「38 个命令、全部 `unknown`」 | **39 个条目、38 个 `unknown`、1 个具真实返回类型**（`get_close_behavior`，`59aeee1` 加入） | 契约生成会漏掉第 39 个命令 |
| `task-13.9`「5 个步骤、依赖 4 个 `/tmp` 产物」 | **4 个步骤**，仓库外输入只有 `:66-67` 的 2 个环境变量 | 按不存在的前提改动脚本 |
| `manifest.json` 命令「全部唯一」 | 307 条中 **304 条互不相同**（`task-3.10.mjs` 被 3.10–3.13 共用） | 无（但会重复提出一个已不需处理的议题） |
| `PlaybackContextRequest` 等「全仓无使用点」 | 同模块内有 `impl` 与自测，准确说法是"**无定义模块之外的消费者**" | 归位时被当成可随意删除的死代码 |
| 架构守卫「只扫 `use` 行」 | 只有 `layering_violations` / `crate_use_violations` 如此；`platform_cfg_violations` 是全行 `contains` | 给不受影响的探测器补无用用例，真正盲区被漏掉 |
| `import.rs`「唯一超 1000 行」 | 仅按**非测试行数**成立；按当前 HEAD 的总行数口径有 15 个 Rust 源文件越线（多为内联测试） | 口径混用，优先级判断失真 |

结论：**门禁要自证，方案也要自证**。任何写进 spec 的数值都必须带上"在哪个 HEAD 上、用什么命令"都能重跑出来的来源。

## Goals / Non-Goals

### 发布门禁与真机证据

发布门禁在干净检出或 CI 中只运行具备自动化命令的场景；原生矩阵场景仍由 `pnpm verify:scenario -- --all` 严格要求真实目标机器上的 attestation。两者不可互相替代：前者保证可重复的代码门禁，后者由 8.4 的真机冒烟提供实际平台证据。

**Goals:**

- 让每一项已写入 `CODE_STANDARDS.md` 的规范阈值都有对应的、会在违反时失败的检查。
- 让"领域规则归 Core"与"Core 不依赖平台"一样可被自动拒绝，而不依赖人工审查。
- 让验证门禁自身可信：任何"全绿"都必须意味着真的验证了东西。
- 让已有的越界与超限可收敛，且收敛过程行为不变、可分批回滚。

**Non-Goals:**

- 不改变任何产品可见行为。本变更不含功能开发，播放、资料库、歌单、同步语义均不变。
- 不改动 `PlayerPort`（3 方法，全仓设计最好的抽象）、不改动 SQLite schema 与迁移、不改动远端同步协议、不改动 Tauri command/event 的名称与参数、不改动 IPC 错误 DTO 的形状与错误 code 稳定性（`ipc/error.rs` 的映射已正确）。
- 不引入第三方状态管理库；前端状态统一是把已有的正确实现抽成原语，不是新增依赖。
- 不合并 `features/library/coverPalette.ts` 与 `features/player/artworkPalette.ts`（两者职责不重叠，见上文「复审修正」）。
- 不在本变更引入 AST 级静态分析框架，不新增运行时依赖。
- 不为"使用设计模式"新增层级或 trait；拆分只按既有职责边界进行。
- 不追求一次清空存量债务。lint 恢复后暴露的告警与超限文件按批次收敛，白名单机制保证只减不增。

## Decisions

### D1. 规范落为新的 `engineering-governance` 规格领域，而不是只更新 `CODE_STANDARDS.md`

`CODE_STANDARDS.md` 是叙述性文档：它无法被 `openspec validate` 校验，没有 scenario，也不会在归档时被检查。把它同时写成可验证的需求与场景，规范才具备"会失败"的形态。

**取舍**：会出现"阈值写在文档、契约写在 spec"的两处文本。规定职责分工——`CODE_STANDARDS.md` 保持叙述与阈值的唯一真相，`engineering-governance` 持有可验证的行为契约并引用它；两者不一致时以同一变更同步更新，spec 中不重复铺陈规范全文。

**备选与否决**：只更新 `CODE_STANDARDS.md`（不可验证，等于维持现状）；把本变更标记为 `skip_specs: true` 的纯工具类变更（规范本身是行为契约，且提案已需要一个新领域来承载它）。

### D2. 先让门禁可信，再做重构

`run-scenario.mjs` 只判退出码意味着"某个过滤器已永久失效"与"全部通过"在观测上不可区分。在这种状态下做分层归位，任何"测试全绿"都不能作为未退化的证据，归位本身也就不可验收。因此门禁通电必须先于一切代码移动。

**备选与否决**：先做 lint 止血（收益最大但会让 16k 行首次暴露告警，且仍无可靠的回归信号）；先做分层归位（无法证明行为未变）。

### D3. 恢复 lint 继承采用"模块级收窄"，而非保留 crate 级放宽

cargo 不允许 `lints.workspace = true` 与本地 `[lints.rust]` 覆盖共存——实测报错为 `cannot override 'workspace.lints' in 'lints', either remove the overrides or 'lints.workspace = true' and manually specify the lints`。因此可行方案只有：恢复 `workspace = true`，把 `unsafe_code` 的放宽从 crate 级下移到真正需要 FFI 的模块（`echo-desktop/src/player/ffi.rs`，必要时 `player/actor.rs`），并按 §4.1 补安全前提注释。

**取舍**：`echo-desktop` 首次全面接受 `clippy::all/pedantic/nursery` 会暴露一批存量告警。这是被掩盖 9,958 行生产代码的真实债务，不是新问题。

**备选与否决**：crate 级 `allow` 与 `workspace = true` 共存（cargo 直接拒绝）；把 `echo-desktop` 排除在 workspace lint 之外（即现状，正是要修的问题）。

### D4. "领域规则不得在平台层重复实现"用结构不变量代理，不引入语义分析

这是本变更最难自动化的判定。方案不尝试理解语义，而是把可判定的结构信号固定下来：

1. 平台层不得把"资料库根 + 相对路径"组合成绝对位置——该组合在 Core 之外出现即失败（Core 提供唯一入口）。
2. 平台层不得出现对播放上下文的分页遍历、顺序反转或可用性裁决的等价实现；对 Core 用例的调用只允许出现在命令编排模块。
3. 允许清单显式登记，且只减不增（与规模白名单同一机制）。

理由：与 `echo-core/tests/arch.rs` 现有的"逐行匹配 + 清单解析，不做 AST"风格一致，守卫可解释、无假阳性、无新依赖；而 §3.3 明确反对为模式加层，引入分析框架属于此类。

**取舍**：结构性规则无法覆盖全部语义越界，仍可能有绕过方式。这是有意识的取舍——守卫的作用是把"新增越界"从默认通过变为默认失败，剩余判断留给审查（`engineering-governance` 中"平台层只做适配"的场景即为审查依据）。

**备选与否决**：引入 `syn` 做 AST 级分析（为守卫增加重依赖，且无法可靠判定"等价实现"）；纯人工审查（本变更的前提正是文档约束会衰减）。

### D5. 播放上下文归位复用既有类型，并清理死代码

`echo-core` 已定义 `PlaybackContextRequest`/`ViewRef`/`PlaybackContextResolved`，说明该用例本就设计归 Core，只是实现落在了平台层。归位时复用这些类型而非另造一套；`ViewRef` 同时承担"视图取值集合"的校验，把平台层 `&str` 的未知视图判断收回 Core。新增用例落在 `application/` 层（用例编排），顺序与过滤规则落在其依赖的 `domain` 规则上，符合 §3.1 依赖方向。

**兼容影响**：Tauri command 的名称与参数不变；`view` 字符串的合法取值由 Core 校验，非法取值由"平台层返回 validation 错误"变为"Core 用例返回等价校验错误"，语义等价。

**设计模式对应的真实问题**：这里用的是 Ports and Adapters 的**归位**，不是新增抽象——把已有业务规则放回它本该属于的适配器内部。

### D6. 歌曲绝对位置解析收进 Core 的单一入口

桌面现有 2 处 `绝对根.join(相对路径)`。方案在 Core 提供单一解析入口（`Song` + `LibraryRoot` → 绝对位置），平台层两处调用点改为调用它。放置位置遵循 §3.1：相对路径与根的关系属领域不变式，不属平台能力。

**备选与否决**：在平台层抽一个私有 helper（把重复从 2 处收敛到 1 处，但不变式仍留在平台层，移动端会再复制一次）。

### D7. 跨边界契约由生成器产出，`BridgeCommandMap` 从手工文件变为生成物

现状是生成器只产 DTO，命令映射手写且 39 个条目中 38 个为 `unknown`——与 `bridge/index.ts` 自身注释「每个命令都有类型化的、生成的签名」矛盾。方案扩展生成器输出「命令 → 返回 DTO」映射，`BridgeCommandMap` 改为生成物，42 处 `bridge.call(` 逐步改用泛型以获得类型。

**BREAKING（仅限仓库内部 TS 契约）**：`BridgeCommandMap` 由手工维护变为生成；调用点会因类型收紧而暴露既有 `as` 断言。这是预期收益，但必须与 Rust 侧同批次提交，否则前端类型检查会中途失败。漂移守卫沿用既有做法（生成后 `git diff --exit-code`）。

### D8. 规模收敛先移测试、后拆实现，且白名单只减不增

超限文件里多数体积来自内联测试（`import.rs` 3,684 行中约 2,610 行为测试；`sqlite/tests.rs` 3,276 行为纯测试文件）。§2.1 要求"相关测试就近放在被测模块旁，不集中成巨型测试文件"，因此顺序是先按被测模块就近分散测试，再拆生产逻辑——先移测试能立刻降低文件行数且行为风险最低。

**备选与否决**：按行数机械切分（违反 §2.1"不以行数为唯一依据，按职责与边界拆分"）；一次性大拆（不可审查，且与并行会话冲突概率高）。

### D9. 测试替身以条件编译门控

`crates/echo-desktop/src/player/fake.rs`（723 行）是测试替身但未被门控，始终编译进生产构建。加 `#[cfg(test)]` 或 `feature = "testkit"`，并把它写进 `engineering-governance` 的构建纯净性需求。仓库已有同类先例（`application/testing/*` 用 `cfg(any(test, feature = "testkit"))`），因此沿用既有惯例而非新造开关。

### D10. CI 只接入核心门禁集，不追求接入全部 79 个检查

把对账、场景命令有效性、架构守卫、规模门禁、生成物漂移、覆盖率纳入 CI；发布门禁去除对仓库外 `/tmp` 手工产物的依赖。不接入全部 79 个检查的理由是其中多数需要重编译或平台特定产物，全量接入会让 CI 时长失控；核心集覆盖了本变更关心的全部失败模式。

### D11. 优先"启用已有校验"，而不是新写校验

复核发现若干能力**已经存在、只是没被执行**：`openspec validate --archived` 早已能检出未完成任务并以退出码 1 失败，但仓库没有任何 git 钩子；覆盖率门禁已写在 `scripts/verify/checks/task-12.8.mjs` 里，却不在 CI；架构守卫只差平台层那一半。因此方案的默认动作是**把已有校验接上电**，只有在确实不存在时才新写。

这既是成本考量，也是信号质量考量：本仓库门禁的主要失效模式不是"缺少检查"，而是"检查不在执行路径上"。新写检查却不接入执行路径，只会重演同一问题。

**取舍**：需要先核实每个问题是否已有对应能力，规划成本略高；但避免了重复实现与新的死门禁。

### D12. 前端状态用"抽出既有正确实现"而不是"引入状态库"

仓库已有 5 个自研 store、3 种写法。其中 `apps/desktop/src/player/playerStore.ts` 是写得最好的一份（`class` + `useSyncExternalStore`、快照权威、明确注释"乐观值不伪造快照未确认的终值"），而 `coverArt.ts` / `songUpdates.ts` / `coverPalette.ts` 走的是模块级可变状态 + `Set` 的路子。方案是**把 `playerStore` 那套抽成共享原语并让其余 4 处复用**，同时为每个 feature 建立只导出对外契约的 `index.ts`，用 ESLint `no-restricted-imports` 的 patterns 钉住跨模块引用只能走公开入口。

**理由**：这直接对应 `CODE_STANDARDS.md` §3.2「不得使用隐藏依赖的全局可变状态、Service Locator 或随处可取的单例」，且是"抽出已有正确实现"，不新增抽象层。

**备选与否决**：引入第三方状态库（新增依赖，且与 §3.3 末句「不得为了'使用模式'增加无业务价值的层级」相悖，而仓库已有一份正确实现可复制）；只靠代码审查（无门禁，按本 change 的前提必然衰减）。

### D13. "不等待结果"的跨边界调用必须显式表达意图

`void bridge.call(...)` 在 `invoke` 对未注册命令抛错时会静默吞掉失败。方案让 bridge 明确区分两种意图——需要结果的调用（必须处理返回值与失败），以及不等待结果的调用（显式吞错并上报到可观测通道）——并用 lint 规则禁止裸丢弃。

**理由**：§2 要求"不在业务路径使用未处理的…静默失败"。且这不是理论风险：本项目已因同一形态踩过坑（e2e mock bridge 命令表漂移时表现为"点击毫无反应，红的是几条毫不相干的断言"）。

**备选与否决**：要求全部调用都 `await` 并处理（改动面过大，且部分调用确实不需要结果）；只在文档中约定（无门禁）。

### D14. 门禁的有效性必须被证明，而不是被假定

本轮只对 79 个检查中的少数做了断言强度审计，其余为未知有效性。方案把"注入违规即失败"的可复现证明作为检查的准入条件：新增检查必须随附证明，存量检查按"未知有效性"登记并排序处理，优先处理断言为空、只查退出码、或依赖仓库外产物的那些。

**理由**：本仓库的主要失效模式正是"检查存在但不在执行路径上"（见 D11）。恒绿的门禁比恒红的更危险——恒红会有人去看，恒绿不会。

**备选与否决**：一次性审计全部 79 个检查（成本高，且解决不了将来新增检查的退化问题；建立持续机制更划算）。

## Risks / Trade-offs

- **并行会话冲突（最高）** → 动手前 `git status` + 复查目标文件；按 D2 顺序分批提交，每批独立可回滚；先做不触碰 `player/*` 的批次。规划期间实测并行会话正在改 `apps/desktop/src-tauri/src/commands.rs`、`main.rs`、`apps/desktop/src/bridge/index.ts`、`features/player/QueuePanel.tsx`、`features/settings/SettingsView.tsx`、`styles/player.css`、`crates/echo-desktop/src/player/coordinator.rs`、`runtime/player.rs`——**第 4 组（契约生成）与第 6 组（前端）正好压在这些文件上**，必须等对应改动落地后再动，或先与并行会话协调。发现改动与规划不一致时先读当天工作日志再继续。
- **恢复 lint 继承首次会报警一批** → 先以警告模式观察一轮，统计分布后按模块分批处理；本批次只清 `echo-desktop` 自身引入的告警，不改逻辑。
- **对账当前为红（69 处失配），新门禁一上线即失败** → 新增阻断前先修正追溯数据与领域登记；把"修复对账"排在"接入 CI"之前，避免门禁长期红导致被忽略（恒红门禁与恒绿门禁同样失去信号价值）。
- **结构不变量守卫可能误伤正常改动** → 允许清单机制 + 守卫自带"能拒绝"自证用例；若某条规则反复误伤，应在 design 中记录并调整规则，而不是放宽为永真。
- **生成器改造使前端类型检查中途失败** → `BridgeCommandMap` 生成物、Rust 生成器与调用点泛型化必须同批次提交。
- **白名单机制可能被滥用为"合法豁免"** → 加"只减不增"门禁：向白名单新增条目本身即失败。
- **规模拆分可能丢失测试资产** → 拆分只做移动与就近安置，禁止删除或跳过既有测试（`import.rs` 的 2,610 行测试是资产）。

## Migration Plan

按 D2 的论证顺序，每组独立提交、独立门禁、独立可回滚：

1. **门禁通电**（`run-scenario.mjs` 的"至少一个测试"断言、`validate-scenario-commands.mjs` 自举、对账数据修正与领域登记）——此步不得改变任何产品行为。
2. **静态检查**（恢复 `echo-desktop` lint 继承、模块级收窄 `unsafe`、分批清告警）。
3. **架构守卫对称**（平台层守卫 + 共用探测器 + 自证用例）。
4. **领域归位**（播放上下文、绝对位置解析、恢复裁决下沉 Core；复用并清理 `domain/catalog.rs` 的死类型）。
5. **契约生成**（生成器输出命令映射、`BridgeCommandMap` 转生成物、调用点泛型化）——同批次提交。
6. **规模与内聚收敛**（测试就近分散 → 拆实现；端口按调用方能力拆分并提升 `testkit`；`error.rs` 按层拆但不改边界映射；白名单只减不增）。
7. **前端架构收敛**（状态原语统一、公开入口与跨模块边界、拆开 `coverPalette`、解开测试基建的反向依赖、显式化"不等待结果"的调用、拆过大组件）。
8. **CI 与治理**（核心门禁接入 CI、发布门禁去 `/tmp` 依赖、工具链对齐、归档校验接入执行、处置未被解析的 136 个 yaml、证明其余检查的有效性、`CODE_STANDARDS.md` 与 `docs/DESIGN.md` 同步）。

**回滚**：每一步都是独立提交组，回滚某一步不影响其他步骤；第 4、5、6 步为行为保持型重构，回滚只影响结构。若第 3 步的守卫暴露了未预期的越界，先登记允许清单保住门禁在线，再单独补归位批次。

## Open Questions

- 规模与允许清单的**初始条目**需要在实施时按实测确定（超限文件与超限 trait 的完整列表、`echo-desktop` 首次 lint 的告警分布）。这不影响规格、方案与任务划分，仅影响白名单的初始内容。
- 端口拆分的最终粒度（`TxAccess` 与 `LibraryFileSystem` 各 17 方法，按调用方能力可拆成 2~3 个）留待实施时按真实调用点决定，阈值与拆分原则已由规格固定。
