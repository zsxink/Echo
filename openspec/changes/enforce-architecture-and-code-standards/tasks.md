> 纯规划：本 change 尚处于 propose 阶段，未开始实现。
>
> 顺序依据 design.md 的 D2：**先让门禁可信（第 1 组），再做一切代码移动**。
> 第 1 组不得改变任何产品行为；第 3/4/5/6 组均为行为保持型重构，须独立提交、独立回滚。
> 动手前先 `git status` + 复查目标文件——本仓库存在并行会话，已发生过改动被静默回退的事故。
> 行号与规模数字以本 change 定稿时（`51c8f2f`）复验为准；落地前需再核一次。

## 1. 门禁通电（必须先完成）

- [x] 1.1 让场景执行证明"至少跑了一个测试"：改造 `scripts/verify/run-scenario.mjs`，在以过滤器或名称选择测试的命令上解析执行结果中的已执行测试计数，零匹配即判定失败而非成功。验证：临时把某条场景命令的过滤器改成不可能匹配的值，`pnpm verify:scenario` 必须失败；恢复原值后通过。
- [x] 1.2 让零测试防护脚本可在干净检出上运行：改写 `scripts/verify/validate-scenario-commands.mjs`，不再无条件读取仓库外的 `/tmp/core-lib-tests.txt` 与 `/tmp/desk-lib-tests.txt`（实测两者不存在，脚本运行即 ENOENT），改为在运行期自行获取真实测试清单或由可重复的构建步骤产出。验证：在一个无任何 `/tmp` 手工产物的检出上运行 `node scripts/verify/validate-scenario-commands.mjs`，能完整执行并给出结论。
- [x] 1.3 修正三方对账数据与规格领域登记：把 `phase-one-acceptance` 与 `portable-library-layout` 加入 `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX`（当前登记 8 个领域、`openspec/specs/` 有 10 个，这两个领域的 9 个场景完全脱离追溯）；修复当前失配——复验时实测 `spec 209 / trace 179 / manifest 181`、**96 处**失配（含 `PM-R07-S01`、`PM-R07-S02` 等无命令场景）。**注意：这三个数字随每次规格变更漂移，文中数值只是定稿时的快照，门禁必须自行派生而不是硬编码。** 验证：`node scripts/verify/reconcile-scenarios.mjs` 退出码为 0 且末行三方数量相等；把任一数字人为改错，门禁必须失败。
- [ ] 1.4 让发布门禁自洽：改写 `scripts/verify/checks/task-13.9.mjs`（全文只有 4 个步骤），把第 1 步的对账正则（`:51`，当前 `/spec \d+ = trace \d+ = manifest \d+ scenarios/` 不比较数字，是靠 `reconcile-scenarios.mjs` 的退出码兜住）改为捕获数量并断言相等；把硬编码的场景总数（`:74`、`:75`、`:85` 三处的 `166`）改为按规格动态派生；把它传给场景执行的仓库外环境变量（`:66-67` 的 `CCORE_TESTS=/tmp/core-lib-tests.txt` 与 `CDESK_TESTS=/tmp/desk-lib-tests.txt`，实测两个文件都不存在）改为可本地生成的输入，或在缺失时给出可执行指引。**注意**：审计曾称 13.9 依赖 `/tmp` 下的 native report / attestation / notarization / signing identity 四个文件——复验证明 `scripts/verify/` 内**不存在**这些引用，该结论不成立。验证：人为改动一个对账数字，13.9 必须失败；干净检出上 13.9 能完整执行。
- [x] 1.5 修复场景清单生成器：`scripts/verify/gen-scenario-manifests.mjs` 把字面 `declare()` 写进了模板字符串（应为函数调用），导致 45 个 `tests/native/*.md` 的 evidence-path 全部是字面 `declare()` 且内容为未填占位（占位符是**半角** `(填写)`，用全角检索会得 0 造成假阴性）。修正后重新生成。验证：重新生成后 `tests/native/*.md` 的 evidence-path 不再含字面 `declare()`；`check-native-attestation.mjs` 仍按预期拒绝未填写项。

## 2. 静态检查继承与架构守卫对称

- [ ] 2.1 恢复 `echo-desktop` 的 workspace lint 继承：`crates/echo-desktop/Cargo.toml:33-34` 当前声明 `[lints.rust] unsafe_code = "allow"` 且无 `lints.workspace = true`，整包丢失 workspace 的 `clippy::all/pedantic/nursery`（实测：同构最小 workspace 中此写法得到 **0 warnings**）。cargo 不允许两者共存（实测报错 `cannot override 'workspace.lints' in 'lints'`），因此改为 `workspace = true`，并把 `unsafe_code` 放宽下移到 `crates/echo-desktop/src/player/ffi.rs`（60 处 unsafe，必要时 `player/actor.rs` 的 28 处）的模块级，按 `openspec/CODE_STANDARDS.md` §4.1 补安全前提注释。先以警告模式观察一轮并统计告警分布。验证：`cargo clippy --workspace --all-targets --all-features -- -D warnings` 与 `cargo fmt --all --check` 通过。
- [x] 2.2 新增 lint 继承门禁：检查每个 crate 的 `[lints]` 段是否继承 workspace；任一 crate 声明 crate 级覆盖即失败。验证：临时给某 crate 加 crate 级 `[lints.rust]`，门禁必须失败；移除后通过。
- [ ] 2.3 按模块分批清理 2.1 暴露的存量告警，只修静态检查问题、不改逻辑。验证：每批处理后 `cargo clippy --workspace --all-targets --all-features -- -D warnings` 与 `cargo test --workspace` 均通过。
- [ ] 2.4 新增平台层架构守卫 `crates/echo-desktop/tests/arch.rs`：把 `crates/echo-core/tests/arch.rs` 的探测器实现抽为共享的 dev-only 测试支持并复用；覆盖 (a) 平台层不得把"资料库根 + 相对路径"组合为绝对位置，(b) 平台层不得出现与 Core 用例等价的分页遍历、顺序反转或可用性裁决，(c) 允许清单显式登记且只减不增；包含能拒绝合成违规的自证用例。验证：`cargo test -p echo-desktop --test arch` 通过；临时引入一处违规，守卫必须失败并指出文件。
- [ ] 2.5 让守卫覆盖全部 crate：校验 workspace 每个 crate 都有架构守卫测试并被执行。验证：临时移走或跳过某个 crate 的守卫，检查必须失败。
- [ ] 2.6 补齐守卫的已知盲区并显式记录：现有 `echo-core/tests/arch.rs` 中，**分层方向探测器与 crate 依赖探测器**只匹配 `use` / `pub use` / `extern crate` 前缀行（实测 `layering_violations`、`crate_use_violations`），因此行内全限定路径（如 `crate::infrastructure::…`）会逃逸；而 `platform_cfg_violations` 是全行 `contains`，不受此限——**不要笼统说"所有探测器都只扫 use 行"**。为受影响的探测器补一个"以行内全限定路径写成的合成违规"自证用例；确实无法覆盖的写法在守卫内以已知盲区注释登记，并在 design 中同步。验证：合成违规用例能拒绝该写法；守卫注释中列出的盲区与实际一致。

## 3. 领域规则归位（行为不变，需在第 1 组之后、第 2.4 组的守卫下进行）

- [ ] 3.1 把「资料库视图 / 歌单 → 有序播放上下文」的解析下沉到 `echo-core`：复用 `crates/echo-core/src/domain/catalog.rs` 中已定义但**无定义模块之外消费者**的 `PlaybackContextRequest` / `ViewRef` / `PlaybackContextResolved`（同文件内有 `impl` 与自测，**不是**可随手删除的死代码——归位后它们应成为真实消费者的入口），把 `crates/echo-desktop/src/runtime/services.rs:94-184` 的"最近"过滤（对标题、艺人、专辑的不区分大小写子串匹配）、分页遍历（PAGE_SIZE=500，遍历至末页）、排序、歌单顺序（最后加入者首位）与选中项校验搬入 Core 用例，未知视图的校验一并收回 Core。桌面侧改为调用并只做参数传递与错误映射。验证：`cargo test -p echo-core --lib application::playback_context`、`cargo test -p echo-desktop`、以及对应场景的 `pnpm verify:scenario` 通过。
- [ ] 3.2 在 Core 提供歌曲绝对位置的单一解析入口（资料库根 + 相对路径 → 绝对位置，作为领域不变式），桌面 `crates/echo-desktop/src/runtime/services.rs:931` 与 `crates/echo-desktop/src/runtime/player.rs` 的解析闭包改为调用它。验证：2.4 的规则 (a) 通过；`cargo test -p echo-desktop` 通过；桌面侧绝对位置拼接归零。
- [ ] 3.3 把播放会话恢复的裁决规则下沉为 Core 领域规则：当前 `crates/echo-desktop/src/runtime/services.rs:189-243` 自行裁决（可用→Restore / 缺失→Blocked / 待删除或异根或查询失败→Drop）以及"活动根内且可播放"的重试判定。规则搬入 Core，桌面侧只做调用与向既有会话类型的映射；不得改动 `desktop-playback` 已规定的对外语义。验证：Core 侧单测覆盖四种裁决与重试判定；`cargo test -p echo-desktop player::session` 通过；启动恢复场景通过。
- [ ] 3.4 清理归位后的残留：确认 3.1 复用的 Core 类型已成为真实使用点（消除死代码），删除被归位替代的旧实现与不再需要的导出。验证：`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过（含 dead_code）；全仓检索确认无未使用的对应类型或函数。

## 4. 跨边界契约生成

- [ ] 4.1 扩展 `crates/echo-desktop/src/ipc/generate.rs` 输出「命令 → 返回 DTO」映射，使 `apps/desktop/src/ipc/ipc-types.generated.ts` 含命令契约；`apps/desktop/src/bridge/index.ts` 的 `BridgeCommandMap`（实测 **39** 个命令条目中 **38** 个返回 `unknown`，仅 `get_close_behavior: () => string` 具真实返回类型）改为引用生成物。按 design.md 的 BREAKING 说明，Rust 生成器、生成物与调用点必须同批次提交。验证：运行生成器后 `git diff --exit-code` 无输出；`pnpm --dir apps/desktop typecheck` 通过。
- [ ] 4.2 消除未知返回值：逐命令补具体返回 DTO，并把 `apps/desktop/src/bridge/index.ts` 的泛型调用能力用到调用点（实测 `bridge.call(` 的调用点无一使用泛型，存在 `as` 手工断言）。验证：`BridgeCommandMap` 中未知返回值计数为 0；`pnpm --dir apps/desktop typecheck && pnpm --dir apps/desktop lint && pnpm --dir apps/desktop test` 通过。
- [ ] 4.3 新增契约与生成物漂移门禁：手工契约与生成物不一致即失败；未知返回值计数不为 0 即失败。验证：手工改动生成物，门禁失败；还原后通过。

## 5. 规模与内聚收敛（白名单只减不增）

- [ ] 5.1 新增规模门禁：单个源文件总行数超过 1000 行即失败（生成代码、产品原型、编译产物除外）；任一 `pub trait` 方法数超过 6 即失败；允许清单显式登记且向清单新增条目本身即失败。初始清单按实测确定。验证：临时新增一个超限文件，门禁失败；临时向允许清单加条目，门禁失败。
- [ ] 5.2 先把测试就近分散：把 `crates/echo-core/src/application/import.rs` 的内联测试（该文件约 3,684 行中约 2,610 行为测试）与 `crates/echo-core/src/infrastructure/sqlite/tests.rs`（约 3,276 行纯测试）按被测模块就近拆分，符合 `CODE_STANDARDS.md` §2.1「相关测试就近放在被测模块旁，不集中成巨型测试文件」。禁止删除或跳过任何既有测试。验证：文件行数显著下降；`cargo test -p echo-core` 通过且测试总数不减少。
- [ ] 5.3 拆分 `crates/echo-core/src/application/import.rs` 的生产逻辑（总 3,684 行，其中生产约 1,073 行）为按职责的子模块（计划 / 执行 / 报告 / 歌词），并把其中的去重与冲突判定下沉为 Core 领域服务，使移动端可复用。**口径说明**：按"非测试行数"计，它是全仓库**唯一**生产代码越过 1000 行的文件；按当前 HEAD 的"文件总行数"计，有 15 个 Rust 源文件越线（含本文件），其中多数体积来自内联测试（如 `player/actor.rs` 总 2,670 行而生产仅约 420 行），两者的处置优先级不同，不要混为一谈。验证：每个文件 ≤500 行；`cargo test -p echo-core` 通过；导入相关场景通过。
- [ ] 5.4 拆分 `crates/echo-core/src/application/ports.rs`（956 行、**26 个 `pub trait`**、全在一个文件）：按 repository / filesystem / media / system / sync 分文件，根部 re-export 保持公共入口稳定；拆分 `TxAccess`（17 方法）与 `LibraryFileSystem`（17 方法，实测一个 trait 混了枚举与元数据、暂存与发布、废纸篓、写能力探测四类关注点）。注意：小端口（2–4 方法）已符合 §3.2，不要为统一而改造它们。验证：既有 `use echo_core::application::ports::X` 的调用方无需改动导入路径；每个 trait ≤6 方法；`cargo test -p echo-core` 通过。
- [ ] 5.5 拆分 `crates/echo-desktop/src/runtime/services.rs`（约 1,591 行、约 30+ 个 `pub fn`，覆盖启动引导、曲库查询、收藏、歌单、删除撤销、导入、扫描、根目录切换、主题、播放上下文、会话裁决、揭示文件）为按能力划分的命令编排模块（读 / 写+gate / 工作区 / 播放），共享同一 `Deps` 值对象；不得改动 Tauri command 的名称与参数。同时按行数收敛其余超限文件：`player/queue.rs`（约 1,110）、`platform/local_state.rs`（约 1,124）、`player/coordinator.rs`（约 1,377）、`runtime/player.rs`（约 1,767）。**不要动 `PlayerPort`（3 方法，全仓最好的抽象）。** 验证：`cargo test -p echo-desktop` 通过；命令清单与 `crates/echo-desktop/src/ipc/` 的契约、`apps/desktop/src-tauri` 的权限配置保持一致（`main.json` 的 permissions 与 `security.rs` 的常量必须完全一致）。
- [ ] 5.6 门控测试替身：`crates/echo-desktop/src/player/fake.rs`（约 723 行）是测试替身但未门控、始终进入默认构建；按仓库既有惯例（`application/testing/*` 用 `cfg(any(test, feature = "testkit"))`）加门控，并新增构建纯净性门禁。验证：默认构建产物中不含该适配器；`cargo build -p echo-desktop` 与 `cargo test -p echo-desktop` 均通过。
- [ ] 5.7 拆分 `crates/echo-core/src/error.rs`（823 行单一巨型错误枚举）：按层（domain / application / infrastructure）拆分为子模块，保留 `thiserror` 的可匹配错误类型。**注意**：跨 IPC 边界的错误映射**已经正确**——`crates/echo-desktop/src/ipc/error.rs:52-81` 已把 `CoreError` 显式映射为 `IpcErrorDto`，并有钉住 code 表与 `retryable` 策略的测试（审计曾判定此处越界，复验已推翻）。因此本项**只做内聚拆分，不改动边界映射**，也不得破坏既有 DTO 形状与 code 稳定性。验证：`cargo test -p echo-core` 与 `cargo test -p echo-desktop ipc::error` 通过；既有错误 code 集合不变。
- [ ] 5.8 把测试替身目录提升为一等公民：`crates/echo-core/src/application/testing/` 现约 **3,931 行**（`memory_database.rs` 1,013、`small_fakes.rs` 747、`filesystem.rs` 706、`repositories.rs` 650…），其中 `memory_database.rs` 是完整的内存数据库实现，而非 §2.1 所说的"小 fake"。把它提升为 `testkit` 模块（沿用既有 `testkit` feature），并按 port 分组拆分文件。验证：`cargo test -p echo-core` 通过；`cargo test -p echo-core --features testkit` 通过；`cargo build -p echo-core`（默认特性）不编译该目录。

## 6. 前端架构收敛

- [ ] 6.1 禁止静默丢弃跨边界调用失败：实测生产路径有 **19 处** `void bridge.call(...)`（`player/useGlobalPlayerHotkeys.ts` 6、`features/player/PlayerBar.tsx` 5、`features/player/QueuePanel.tsx` 2、`features/playlists/PlaylistsView.tsx` 2、`features/library/LibraryWorkspace.tsx` 2、`features/player/ImmersivePlayer.tsx` 1、`features/settings/useTheme.ts` 1）。Tauri `invoke` 对未注册命令会抛错，裸 `void` 会静默吞掉；本项目已因同类形态踩过坑（e2e mock bridge 漂移时"点击毫无反应，红的是不相干断言"）。在 bridge 上区分"需要结果"的调用与显式吞错并上报的调用，并用 ESLint 规则拒绝裸丢弃。验证：`pnpm --dir apps/desktop lint` 在存在裸丢弃时失败；临时把一处改成裸丢弃，门禁必须失败。
- [ ] 6.2 统一状态原语：实测前端有 **5 个自研 store、3 种写法、无共享原语**——`player/playerStore.ts`（`class` + `useSyncExternalStore`，质量高，作为范本）、`app/toast.ts`（`class` + hook）、`app/coverArt.ts`（3 个模块级 `Set`）、`features/library/songUpdates.ts`（模块级 `Set` + 发布订阅）、`features/library/coverPalette.ts`（模块级 `let` + `Set`）。抽出统一的共享原语并让 5 处复用；新增状态模块不得引入第二种订阅契约。验证：`pnpm --dir apps/desktop typecheck && pnpm --dir apps/desktop lint && pnpm --dir apps/desktop test`；新增状态模块若不复用原语，门禁失败。
- [ ] 6.3 拆开职责混杂的文件：`apps/desktop/src/features/library/coverPalette.ts`（约 189 行）同时包含纯函数 `coverClass(seed)`（确定性封面占位色）与一个模块级可变的资料库计数 store，文件名与近一半内容不符（违反 §2.1「一个文件承载一种抽象」）。拆为 `coverClass.ts`（纯函数）与 `libraryCounts.ts`（改用 6.2 的统一原语）。**注意**：审计曾判断它与 `features/player/artworkPalette.ts` 重复——复验证明**二者不重复**（输入 `string id` vs `Uint8ClampedArray`、输出 CSS 类 vs 像素主色 hex、消费者 列表行 vs 沉浸播放器，三者皆不同），不要合并。验证：`coverPalette` 从各 feature 的 import 列表消失；`pnpm --dir apps/desktop test` 通过。
- [ ] 6.4 建立模块公开入口与跨模块边界：实测 `apps/desktop/src/features/*/index.ts` **一个都不存在**，跨 feature 直接引用内部文件约 18 处（`features/playlists/PlaylistsView.tsx` 7 处最多，其次 `features/player/PlayerBar.tsx` 3 处）。为每个 feature 增加只导出对外契约的 `index.ts`，并用 ESLint `no-restricted-imports` 的 patterns 钉住"跨 feature 只能引用公开入口"。验证：`pnpm --dir apps/desktop lint` 在跨 feature 引内部文件时失败；临时引入一处，门禁必须失败。
- [ ] 6.5 解开测试基建对功能模块内部的反向依赖：`apps/desktop/src/test/setup.ts:82` 直接动态 import `../features/library/coverPalette` 的内部函数做重置。改由公开入口或统一 store 原语提供统一的 `resetAll()`。验证：`test/` 下不再出现指向 `features/*/` 内部文件的引用；门禁覆盖该模式；`pnpm --dir apps/desktop test` 通过。
- [ ] 6.6 拆分过大的视图组件：`apps/desktop/src/features/player/ImmersivePlayer.tsx`（约 671 行）同时负责取详情与歌词、封面取色、键盘与浮层栈、滚动跟随定时器、布局测量与 body 副作用；`features/library/SongMenu.tsx`（约 448 行）、`app/App.tsx`（约 330 行、多个 hook 编排）同类。拆为"数据 hooks 层 / 布局组件 / 交互与浮层行为"三块。**注意**：这不违反 1000 行硬限（属 500 行软线），是 §6「组件保持可测试，避免把数据访问、事件订阅和复杂视图全部放在单个组件中」的可维护性改进，**不做成门禁**。验证：`pnpm --dir apps/desktop typecheck && pnpm --dir apps/desktop lint && pnpm --dir apps/desktop test`；沉浸播放器相关场景不回归。

## 7. CI 接入与治理补强

- [ ] 7.1 把核心门禁集接入 CI：三方对账、场景命令有效性、架构守卫（全部 crate）、规模门禁、契约与生成物漂移、覆盖率、lint 继承。当前 CI 只执行 79 个检查中的 2 个（`task-1.9`、`task-1.10`），对账与场景门禁从未在 CI 运行，覆盖率门禁（`task-12.8`）也不在 CI。验证：本地按 CI 配置执行一遍全部接入项；故意破坏其中一项，CI 必须失败。
- [ ] 7.2 发布门禁去除仓库外依赖：承接 1.4，确认发布门禁在 CI 与干净检出上均可完整执行。验证：在无手工 `/tmp` 产物的检出上运行发布门禁并记录结果。
- [ ] 7.3 对齐工具链：`.github/workflows/ci.yml` 两处（第 27、94 行）使用 `dtolnay/rust-toolchain@stable`，而 `rust-toolchain.toml` 钉的是 `channel = "1.96.0"`——CI 会安装并激活浮动 stable，使"本地钉版本"失去意义。改为按仓库钉住的版本安装，并在 CI 校验实际生效版本与 `rust-toolchain.toml` 一致。验证：CI 日志中的编译器版本与 `rust-toolchain.toml` 一致；把两者改成不同值，校验必须失败。
- [ ] 7.4 归档一致性校验接入执行：`openspec validate --archived` 已具备该能力并且以退出码 1 失败（实测 4 个归档变更失败、共 12 个未勾选任务），缺的是执行——仓库没有任何 git 钩子（`.git/hooks/` 仅剩 sample，无 husky）。把它接入 pre-commit 或 CI，使未完成任务无法静默归档。验证：在 7.8 完成前运行 `openspec validate --archived`，退出码为 1 且被 CI 判为失败；7.8 完成后退出码为 0。
- [ ] 7.5 处置未被真正解析的登记资产：`tests/` 下有 **136 个 yaml**。它们**确实被读取**——`reconcile-scenarios.mjs` 会对每个登记路径做"非空"断言——但其字段（command / requirement / automated filter 等）**没有任何脚本解析消费**，等于只校验了"文件不为空"。逐个判定为"接入门禁（解析字段并断言）"或"删除"。**不要按"无人读取"处理**：直接删除会让对账门禁从"红"变成"输入缺失"而崩坏。验证：`tests/` 下每个 yaml 都能指出解析其字段的脚本与断言；无法指出者已删除或已接入。
- [ ] 7.6 证明其余检查的有效性：本轮只对 79 个检查中的少数做了断言强度审计，其余为**未知有效性**。对每个检查建立"注入违规即失败"的可复现证明（或标记为未知并排序处理），优先处理断言为空、只查退出码、或依赖仓库外产物的检查。验证：检查清单中每一项都有"会失败"的证据链；新增检查必须随附该证明。
- [ ] 7.7 治理场景命令的重复执行：实测 209 条场景命令只对应 **115 条不同命令（1.82×）**，最高一条被引用 16 次（`pnpm --filter @echo/desktop test -- --run src/features/player/ImmersivePlayer.test.tsx`），单条命令失效会同时影响多个场景。按模块聚合场景到更少的命令批次，并保留每个场景到具体测试名的追溯。验证：重复度下降；`pnpm verify:scenario` 的通过/失败集合与改造前一致。
- [ ] 7.8 处置历史遗留：`openspec/changes/archive/` 下 4 个 change 共 12 个未勾选任务被静默归档（`library-nav-counts` 1、`release-0-1-0-desktop-player` 8、`wire-desktop-system-dialogs` 2、`refine-playback-queue-semantics` 1），逐个显式延期或从范围移除。验证：`openspec/changes/archive/` 下不再有未勾选任务。
- [ ] 7.9 治理文档同步：`docs/DESIGN.md` 补上 `apps/desktop/src-tauri` 层级（当前仓库结构章节缺失该层）；`apps/desktop/src/player/playerStore.ts:11` 的注释指向不存在的 `app/store` 模块（全仓仅此一处引用，且全仓无 `@tanstack` 依赖），修正注释或补齐模块；消除 `AGENT.md` 与 `CODE_STANDARDS.md` 各自画架构图的重复真相（指定唯一真相来源）；在 `CODE_STANDARDS.md` 增加「规范条款 → 门禁」映射表。验证：文档中引用的每个路径逐一做存在性检查；`docs/DESIGN.md` 覆盖 workspace 全部 crate。
- [ ] 7.10 固化后续代码变更的规格复核与残留清理：把本 change 的 `engineering-governance` delta 同步为主规格后，在 `openspec/CODE_STANDARDS.md` 明确要求涉及生产代码的 change 在 design/spec 中记录适用的架构层级、依赖方向、规范条款、实现前的现状核对及可执行验证；重构任务必须列出待删除的替代实现、无消费者导出、失效测试替身或登记资产，并以引用检查、静态检查和测试证明清理完成。验证：创建一份含代码变更的样例 change 时，缺少上述任一项即不能通过规格审查；`openspec validate "enforce-architecture-and-code-standards" --strict` 通过。

## 8. 集成验证

- [ ] 8.1 Rust 侧全量检查：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
- [ ] 8.2 前端侧全量检查：`pnpm --dir apps/desktop typecheck`、`pnpm --dir apps/desktop lint`、`pnpm --dir apps/desktop test`、`pnpm --dir apps/desktop build`。
- [ ] 8.3 门禁自检与场景全量：`pnpm verify:self-test`、`pnpm verify:scenario`；并确认 1.3 之后三方数量相等、`openspec validate --archived` 退出码为 0。
- [ ] 8.4 真机冒烟：播放/暂停、切歌、队列顺序与随机/单曲循环、退出后恢复位置与队列、歌单播放上下文顺序、资料库"最近"视图播放、播放控制失败时的用户反馈（对应 6.1）。若并行会话已改动播放相关文件，先复查改动再验证。
- [ ] 8.5 归档前确认：全部任务完成、`engineering-governance` 与 `CODE_STANDARDS.md` 表述一致、`desktop-playback` 的新增需求已在实现中兑现。**并逐条对照本轮审计清单**，确认每一项要么已落地、要么以"已复核为不再成立"的书面结论关闭（见 design.md 的「复审修正」）。
