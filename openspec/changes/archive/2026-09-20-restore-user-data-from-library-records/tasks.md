> 施工前先读 `design.md` 的「实测证据」与「兼容基线」，并复跑 E4/E6/E7 三个关键项。
> 实现于 2026-09-19/20 完成并归档；下列勾选为**完成后回填**，逐条对应实际执行过的命令。
> `scripts/verify/manifest.json` 现有任务 id 最大为 `14.9`，本 change 的可执行命令登记为新的 **`15.x`** 组（`pnpm verify:task 15.x` 依赖它）。

## 1. 兼容基线与前置复核

- [x] 1.1 为「目录不含可携带控制面」的资料库保留纯扫描路径：接续只在 `echo/` 已具备控制面时启用，新增回归测试固定该分支（无控制面 → 不调用接续、不写记录、行为与当前一致）；运行 `cargo test -p echo-core --all-features root_switch` 与 `cargo test -p echo-core --all-features application::import`。
- [x] 1.2 以真机实测形态（缺 `manifest.json`、`echo/records/songs` 与 SQLite UUID 完全脱节、favorites 为 songs 子集）建立测试 fixture：在 `crates/echo-core/src/application/testing/` 下按既有 fixture 模式添加构造器，运行 `cargo test -p echo-core --all-features testing` 通过。
- [x] 1.3 施工前复跑 design 的 E4/E6/E7（records ∩ DB = 0/87、favorites ⊆ songs、87↔87 双射）：先把 `echo.sqlite`、`-wal`、`-shm` 三件套复制到临时目录再打开（只读打开带 WAL 的库会读到陈旧快照），并**以当时的活动根为准**重新取值；与 design 表格不一致时先更新 design 再动代码。　**复跑记录（2026-09-20 00:59:14 快照，活动根 `/Users/xian/Music/Echo`）**：`records/songs` 87 条、`favorites` 11 条、`playlists` 0 条、`media/` 87 个文件；records UUID ∩ DB UUID = **0/87**（E4 一致）；favorites 为 songs 子集且 **3 true / 8 false**（E6 一致）；`echo/` 内**无 `manifest.json`**。⚠️ DB 侧 `sum(is_favorite)` 已为 **0**（design 记录时为 3）——库在核对期间又被重建过，再次印证「结论随 HEAD/运行中的库漂移」。
- [x] 1.4 在 `scripts/verify/manifest.json` 注册 `15.x` 任务组：自定义门禁写成 `scripts/verify/checks/task-15.x.mjs` 并登记，纯命令类任务直接登记 `commands`（现状为 129 个任务 id、其中 72 个有对应 checks 文件，两种登记形状都存在）；运行 `pnpm verify:task 15.1` 与 `pnpm verify:governance` 确认登记生效（脚本写完不登记就是死门禁）。

## 2. 控制面存在性与自愈

- [x] 2.1 在资料库启用链路接入 `InitPortableLibrary`：首次启用可写目录（无 manifest、无 `records/`）时建立 `echo/manifest.json`，记录稳定逻辑 ID 与格式版本；运行 `cargo test -p echo-core --all-features application::portable` 并新增断言「导入第一首歌后目录内出现 manifest」的用例。
- [x] 2.2 实现 `manifest` 缺失但 `echo/records/` 存在的**自愈**分支（D3 情形 2）：以既有对象资料为准建立 manifest，保留全部对象与 UUID，并产出一次自愈事件；新增测试断言自愈后对象数量与 UUID 不变。
- [x] 2.3 区分「manifest 不存在」与「格式版本不兼容」（D3 情形 3）：高版本 manifest 必须走拒绝接续 + 只读降级，且**不得**被自愈逻辑覆盖；新增测试断言高版本 manifest 文件字节不变。运行 `cargo test -p echo-core --all-features control_plane`。
- [x] 2.4 控制面不可写时保持既有语义并明确报告（不静默把本机数据库当唯一存储）：复用 `ensure_control_plane_writable` 的判定，新增测试断言逻辑变更被拒绝且既有控制面文件未被改写；运行 `cargo test -p echo-desktop --all-features services`。

## 3. 对象资料接续与对账

- [x] 3.1 把 `RestoreLibrary` 的投影范围从 `RecordKind::Song` 扩到全部对象种类（歌曲、歌单、歌单成员、收藏、覆盖层、播放统计）：投影顺序必须先父后子（歌单 → 成员），成员顺序按对象资料中的顺序键恢复；新增单元测试断言「投影后歌单数量、成员顺序、收藏集合与对象资料一致」，运行 `cargo test -p echo-core --all-features application::restore`。
- [x] 3.2 把接续接入 `PrepareLibraryCandidate::prepare`（D2）：在既有 `StartScan` **之前**执行「读 manifest → 投影对象资料」，失败时保持「旧活动根不受影响、候选可重试」的既有语义；新增测试断言接续失败不推进 root epoch，运行 `cargo test -p echo-core --all-features root_switch`。
- [x] 3.3 实现 D5 的三条有效性规则（墓碑优先 → 记录采纳 → 无法成立则不进入有效视图且不删除）：新增测试覆盖「墓碑高于记录 → 不复活」「无墓碑 → 采纳（含 `is_favorite:false` 不误恢复）」「引用悬空 → 只计数不呈现」，运行 `cargo test -p echo-core --all-features`.
- [x] 3.4 实现接续结果的幂等（重复打开不产生第二份对象、不改变 UUID）与对账报告（自愈/接续/无效计数各若干）：新增测试连续执行两次接续并断言对象数量、UUID、成员顺序、播放统计完全一致，运行 `cargo test -p echo-core --all-features application::restore`.
- [x] 3.5 接线 `choose_library_root` 并选择呈现方式（D7：优先复用 `LibraryRootStatusDto`；仅当必须新增命令时才动 `main.json` + `security.rs` + `cargo check -p echo-app`）：运行 `cargo test -p echo-desktop --all-features services` 与 `cd apps/desktop && pnpm typecheck`。

## 4. 逻辑对象的材料化写入点

- [x] 4.1 歌单与歌单成员的每次已提交变更（创建、重命名、删除、添加成员、移除成员）在事务提交后写对象资料，沿用 `ImportExecution`/`favorites.rs` 的「提交后写、写失败即失败」模式（D6）：运行 `cargo test -p echo-core --all-features application::playlist` 与 `cargo test -p echo-desktop --all-features services::playlists`。
- [x] 4.2 **范围决策 A（2026-09-20）——本 change 不做**：核查结论是 `LyricsSource::Override` 只是歌词来源**优先级**，0.1.0 没有任何用户可触发的覆盖层**变更入口**可供材料化，`OVERRIDE_OBJECT_TYPE` 因此没有生产调用点（全仓 `grep` 只有常量定义与接续侧的读取）。硬写一个写入点等于凭空虚造功能。处置：接续仍读取并校验 `overrides/` 记录，`15.1` 门禁仍保证该种类与目录映射一一对应，**不登记**任何自动化命令（避免留下永远绿不了的假门禁）；覆盖层编辑真正落地时另开 change。
- [x] 4.3 删除路径写墓碑而不是删除记录（歌单、成员、歌曲），使 D5 规则 1 有实际数据可依：运行 `cargo test -p echo-core --all-features delete` 与 `cargo test -p echo-core --all-features tombstone`。
- [x] 4.4 新增对象种类 `play-stats`（D4）：在 `domain/library.rs` 扩展种类、目录名与载荷类型（`by_device` 加性计数），在 `crates/echo-desktop/src/runtime/player/recorder.rs` 的播放记录路径写入，并在接续投影时按 `Σ by_device` 还原 `play_count`；新增测试断言「两设备各播一次 → 合并后计数为 2」与「同一设备重复写不翻倍」，运行 `cargo test -p echo-core --all-features play` 与 `cargo test -p echo-desktop --all-features recorder`。
- [x] 4.5 为 `RecordKind` 到目录名的映射与 `受管理资料库目录布局` 的一致性加一条自包含检查（新种类新增而规格未更新时必须失败），并按 manifest `14.8` 的要求在 `scripts/verify/injection-suite.mjs` 增加一条可证伪条目；运行 `node scripts/verify/injection-suite.mjs`（本机若被删除护栏拦截，按签名 `SAFE_DELETE_BULK_CONFIRM_REQUIRED` 归因后在 CI 用 `ECHO_INJECTION_REQUIRE_ALL=1` 复跑）。

## 5. 门禁诚实化与场景补齐

- [x] 5.1 在 `scripts/verify/scenario-commands.mjs`（唯一权威源）中把 `PLL-R03-S01/S02` 换成**真正断言歌单/成员顺序/收藏按 UUID 恢复**的命令，并新增覆盖「本机数据库丢失后重开」「重复打开幂等」「歌单与成员被材料化」的场景条目；改完运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重新生成 `manifest.json.scenarios[]` 与 `tests/scenarios/*.yaml`（直接改生成物会被静默抹掉）。
- [x] 5.2 逐条运行新增/修改的场景命令确认可执行且会失败：`pnpm verify:scenario -- PLL-R03-S01`（及新增条目 ID），并运行 `pnpm verify:scenario -- --all` 与 `node scripts/verify/check-scenario-churn.mjs` 确认 1.8x 棘轮未被突破。
- [x] 5.3 更正 `docs/native-attestation-playbook.md` 中 `PLL-R03-S01` 的覆盖描述（现在写「喜欢/歌单/成员顺序」，实际测试只断言歌曲投影），并运行 `node scripts/verify/reconcile-scenarios.mjs` 与 `node scripts/verify/validate-scenario-manifests.mjs`。
- [x] 5.4 同步 `docs/traceability.md` 的 PLL 场景行（新增/变更场景 ID 与验收命令），运行 `pnpm verify:scenario -- --all` 确认 spec=表=manifest 三方 ID 集合相等。
- [x] 5.5 确认新场景命令被登记后，`pnpm verify:governance`（含 14.4/14.5/14.7/14.8/14.9）与 `node scripts/verify/check-verification-validity.mjs` 通过；若新增自包含检查，必须有对应的注入证明条目，否则套件 exit 1。

## 6. 文档与规格一致性

- [x] 6.1 改写 `docs/DESIGN.md:139` 与 §5.1 的真相源措辞：SQLite 是本机运行时投影，资料库目录的 `echo/` 是持久真相源、SQLite 必须可从中重建；同步补充 §4「曲库与扫描」中「打开资料库先接续再扫描」的一句描述。运行 `pnpm verify:governance` 确认文档门禁无回归；文档引用核对走人工（仓库无独立的文档引用检查脚本，勿在任务里引用不存在的脚本）。
- [x] 6.2 核对 `docs/ROADMAP.md` 与 `docs/acceptance/` 中涉及「重新打开资料库」的验收口径，若存在「歌单会恢复」的承诺而本机数据已不可恢复（见 design 的 E2 说明），按「从此不再丢失」改写；人工核对后在归档前记录结论。

## 7. 全量验证与真机复验

- [x] 7.1 跑完工作区门禁：`cargo fmt --all`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`（`--` 不能省）、`cargo test --workspace`、`cd apps/desktop && pnpm typecheck && pnpm lint && pnpm test && pnpm build`，全部 exit 0。
- [x] 7.2 真机复验（**自动化等价复验已执行；GUI 复验未执行**）：以 7.1 快照的同形数据（87 首 / 11 条 favorite、3 true / 0 歌单 / 无 manifest）在 `application::continuation` 新增 `a_wiped_database_is_rebuilt_from_the_records_at_real_scale`，断言清空数据库后恢复 **87 首**、UUID **全部来自记录（无一新铸）**、**3 首**为「我的喜欢」、**0 个歌单**、重复打开结果不变。**未执行部分**：启动 GUI 打开 `/Users/xian/Music/Echo` 的副本并清除应用数据 —— 属破坏性操作（会清掉本机`~/Library/Application Support/com.zsxink.echo` 的真实数据），需用户明确确认后单独执行。
- [x] 7.3 运行 `pnpm exec openspec validate restore-user-data-from-library-records --strict` 与 `openspec validate --all`，确认无结构错误（Scenario 恰好 4 个 `#`、新增需求均带至少 1 个场景）。
- [x] 7.4 归档前确认：`openspec/changes/restore-user-data-from-library-records/tasks.md` 无未勾选任务，`docs/DESIGN.md` 与 `openspec/specs/` 已同步，改写的门禁描述与实际执行的命令一致。
