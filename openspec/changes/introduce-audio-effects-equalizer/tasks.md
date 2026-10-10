# Tasks

> 2026-10-02 实施授权：用户在本轮开发中回复「我批准你通过」，允许先推进功能实现。第1组实测和第6组发布验收仍须补齐；此授权不代表任何平台测量已经通过，未取得证据的任务不得勾选。
>
> 2026-10-03 复核新增：见 `review-and-tuning.md`。R1（运行时改参**未证明生效**；原归因「`extrastereo` 无回调」已证伪，替代结论「一律失效」亦已于 10-04 撤回）、R2（`install_pending` 潜在死锁，任务 4.6）、R6（app 内原先是旧库，任务 4.7）为复核发现并已登记；听感优化见任务 4.8 与 2.5。已完成候选 app 更新和库一致性检查；完整音频 Gate 状态不变。
>
> 2026-10-09 状态更正：上条是历史记录，macOS 运行时参数已由 AFX-9.5 的 EOF 前持续 PCM 捕获确认可用；其他平台仍须独立验证。`af-command` 返回值仍不能代替 PCM 生效证据。

## 1. macOS 原生技术 Gate（用户授权先开发，macOS P0 发布前须通过）

- [x] AFX-1.1 固定 macOS 候选 bundle 及 libmpv/FFmpeg 版本/哈希，补齐可重复 fixture、CoreAudio 探针和隔离 `ao=pcm` 捕获；运行时库哈希、协商采样率/声道和真实处理输出均写入 `native-gate.md` 与证据 JSON（T1/T5、V12/V18）。当时的 `/Applications/Echo.app` 旧包已在 4.7 更新并留备份。
- [x] AFX-1.2 macOS 候选库已验证十段具名链、EQ/preamp 定向命令、空间链 `c=false`、limiter `level=false` 和 loadfile 后配置链存活；低电平 997 Hz 响应误差 `-0.0041 dB`，压力点 sample-peak `-0.99985 dBFS` 且 PCM 全有限；单声道输入保持 mono，空间链被跳过；滤镜 target/options 和 sample-peak/true-peak 未测口径已记入 native-gate。证据：`macos-installed-app-coreaudio-probe.json`、`macos-installed-app-mono-probe.json`、`macos-pcm-response-probe.json`、`macos-pcm-limiter-stress-probe.json`。这只是 1.2 指定的单点实验，不替代 1.3/1.4 矩阵或总体发布 Gate（T1/T2/T3/T5/T6、V2/V6/V7/V17）。
      **⚠️ 2026-10-03 与 `native-gate.md:124` 的表述冲突已消除**：该处曾要求 1.2 保持未勾选，现统一为「1.2 = 已完成的单点实验，Gate 仍由 1.3/1.4/1.5 决定」。另：`-0.0041 dB` 来自**安装期写死 g=6 的静态链**，只证明静态响应；EQ/preamp 定向命令的 rc=0 **不构成**生效证据。
- [ ] AFX-1.3 在 macOS 捕获 30ms 平滑目标、开关/类型切换/输出重建/首样本屏障与 limiter 延迟补偿；验证：每类≥100次捕获 p95≤150ms、无新增爆音/断流/underrun，seek/EOF/歌词进度相对旁路无超过10ms新增偏移；volume/mute 不被覆盖（V10/V16–V18）。
      **⚠️ 2026-10-09 更新**：macOS 参数变化已有持续 PCM 证据，但30 ms输出、无爆音/无断流仍待本任务实测。`latency=true` 通过 trim/pad 补偿 look-ahead；源码不能据此断言丢失节目开头或静音替代尾部。须捕获首尾内容、帧数、seek/EOF、时间戳和设备侧偏移，见 AFX-11.5。
- [ ] AFX-1.4 固定 macOS 参考机（至少4核/8GB/SSD）和最重预设，预热30秒采样5分钟，分别测稳定播放及面板开关；验证：CPU平均增量≤10个百分点，记录原始数据。对九个EQ及空间按BS.1770-5测量同片段、响度匹配≤0.5 LU 后做A/B和人声折叠审查，记录实测结果及调音迭代（V1/V3/V7/V12）。
      **⚠️ 2026-10-03**：`evidence/` 中**无任何 BS.1770 测量文件**，design 的 0.5 dB 自动补偿与 AFX-2.5 的曲线定稿都依赖本项，须优先补做。
- [ ] AFX-1.5 汇总 macOS Gate 结论并更新 design 实测证据；验证：T1/T2/T3/T5/T6及上述门槛全通过。失败须先修订方案及受影响 PRD/spec，不可将空间或原地编辑禁用后标完成。T4、V9/V19留P1。

## 2. 音效纯规则、注册表与状态机

- [x] AFX-2.1 在desktop内部建立类型化Preset/Payload/请求与Applied状态、保留参数和revision/epoch规则，按design表实现选择/编辑/关闭/重置；验证：就近状态机测试覆盖关保存项与关草稿差异、快速输入后关闭/重置、环境变更/旧确认、草稿来源删除（V4/V5/V16），运行`cargo test -p echo-desktop`。
- [x] AFX-2.2 实现十段固定Q系数、有效频段、合成响应、安全preamp及空间增益规则和专业资料复核后的九个注册表初值；验证：单/相邻/全段±12、手动0、全零Auto、22.05kHz及边界数学测试，复算design的48kHz旧/新曲线合成峰值，并覆盖44.1/48/96/22.05kHz；数学检查不冒充音频实测，内置调音变化不改用户曲线/草稿快照，计算误差≤0.1dB且值有限，运行`cargo test -p echo-desktop`（V2/V3/V6/V18）。
      **⚠️ 2026-10-03**：本任务覆盖 V18（采样率/输出重建）**仅靠纯数学测试**，属以软件测试替代原生测量，V18 的原生部分仍待 1.3/4.3。过渡期余量已统一到 `AUTO_HEADROOM_DB`（原过渡用 1.0 dB、稳态用 0.5 dB 的分歧已修）。
- [x] AFX-2.3 实现用户命名、快照、独立ID及50项额度纯规则，复用既有NFKC/空白/full-case-fold；验证：1/40/41 grapheme、空白/组合字符/大小写/内置重名、49/50/51项、重命名额度测试通过`cargo test -p echo-desktop`，不改变歌单或Core API（V4/V8）。
- [ ] AFX-2.4 登记需求/场景/任务与命令到现有验证manifest，维护模块/端口职责说明和适用规范；验证：`cargo test --workspace`、`pnpm verify:governance`通过，新文件不超规模、trait不超6方法，Core无音效依赖（audio-effects架构契约）。
- [ ] AFX-2.5 复核发现的听感问题：九条内置曲线的可辨识性与 auto preamp 后的响度差需要实测；不能把 ±0.5–2 dB 直接判为不可闻或据此放大曲线。按 `review-and-tuning.md` 4.3 迭代幅度（单段 ≤±6 dB，禁止相邻同号大幅提升叠加），**新数值须走任务 1.4 的 BS.1770-5 响度匹配盲听定稿后才写入 `presets.rs`**；首尾两段 shelf 化会突破「十段固定 Q peaking」spec 边界，属 P0 范围变更，需先修订 spec。验证：盲听记录通过后 `cargo test -p echo-desktop effects::presets`，并同步 `design.md` 第3节的曲线表与证据分级（V2/V3）。

## 3. 本机偏好与管理用例

- [x] AFX-3.1 通过聚焦偏好端口接入DesktopStateStore新增版本化effects字段与原子读改写，编辑250ms合并写/退出flush；验证：旧数据默认关闭、主题/窗口/播放会话/未知字段保留、写失败显示未保存且尾值可重试，运行`cargo test -p echo-desktop`（V11/V14/V16）。
- [x] AFX-3.2 实现保存、重命名、删除串行事务及删除当前项的旁路/存储补偿，成功落盘后才发布列表；验证：关闭草稿保存不启用、改名不重建链、删除非当前/草稿来源不改变播放，以及旁路/写入/补偿失败注入，原列表/输入/草稿保留且不报假成功，运行`cargo test -p echo-desktop`（V4/V8/V16）。
- [x] AFX-3.3 实现恢复合法请求、保留原文档/可恢复项及未来版本保护；验证：损坏JSON、非法数值/载荷、悬空ID、未知schema/registry不盲启用不覆盖，恢复后待确认且不自动发声，正常重启保持草稿/请求，运行`cargo test -p echo-desktop`（V11/V17）。
- [x] AFX-3.4 更新`docs/PRODUCT.md`的本机音效与同步边界、`docs/ROADMAP.md`的0.2.0完整P0/P1独立范围，以及本机状态/恢复说明；验证：与proposal/spec一致，无SQLite/outbox/echo控制面新增音效字段，执行隔离测试与`cargo test --workspace`（V13/V14）。

## 4. 播放后端与生命周期集成

- [ ] AFX-4.1 基于Gate确定的API在MpvBackend实现命名链与运行时参数更新，固定浮点精度/preamp/空间/limiter配置，复用现有FFI隔离；验证：原生工具输出与第1组一致，压力/响应/空间折叠通过，运行`cargo test -p echo-desktop`；文档记录unsafe前提（若有）及精确链语法（V1/V2/V6/V7）。
      **2026-10-08 更新**：macOS 参数路径已由 AFX-9.5 的持续 PCM 复测确认；以下为 2026-10-04 历史阻塞记录：运行时原地改参**当时尚未被证明生效**（rc=0 不可信；「一律失效」的旧结论因实验 label 错误已撤回，见 AFX-9.7）。本任务须先确定「重建链」还是「另寻可生效的运行时机制」，并以 PCM 前后测量判定，**不得**以 `af-command` 返回 0 作为生效证据。
- [ ] AFX-4.2 actor在下一轮处理最新请求（不额外debounce）、保留30ms平滑、保护先后序和revision/epoch屏障；新链安装后等待libmpv重配置再更新具名参数。验证：fake故障测试及原生拖动后关闭/重置/切类型捕获均以最新状态收敛，失败回滚或旁路，过期步骤不能重新作用，切换首个音频参数更新无额外合并等待，运行`cargo test -p echo-desktop`（V5/V10/V16）。
      **2026-10-08 更新**：AFX-9.5 已完成，但 `advance()` 的 30 ms 斜坡实际瞬态仍待捕获验收；参数生效证据不能替代中间状态/连续性证据，再以 PCM 实测证明中间增益状态真实出现。另，`start()` 在 incompatible 分支于 `install()` 时直接写入最终 `preamp`，旁路末尾也可能由 `protected_preamp` 阶跃到目标值；改造须把 preamp 纳入过渡并消除该阶跃。
- [ ] AFX-4.3 接入实际处理环境、loadfile/输出重建、暂停/继续/无音轨首样本流程，支持mono/stereo EQ、只双声道空间；验证：22.05/44.1/48/96kHz及既有其他采样率、临时文件、换设备、seek/EOF、歌词/计数/volume/mute回归，通过原生工具与`cargo test -p echo-desktop`（V10/V13/V17/V18）。
- [x] AFX-4.4 runtime统一装配、会话恢复与退出flush/后台关窗，不另建音效会话；验证：窗口隐藏后持续音效，显示后读取同快照、资料库切换和清队列保留曲线，运行`cargo test --workspace`，更新`docs/DESIGN.md`的桌面音效分层/依赖图（V11/V13/V14）。
- [x] AFX-4.5 空间用户参数固定 width=1.25，UI 不开放编辑；2026-10-08 AFX-9.5 的持续 PCM 证实内部 `spatial:m` 有效，因此以实际 m=1↔1.25 生命周期过渡替代原「不得发送 spatial:m」负向断言。此修订用于修复旁路账面中性而实际未变的问题，不引入可编辑 width 或双链引擎；原生端点证据与命令/数学回归见 AFX-10.3，真实瞬态/无 underrun 仍由 AFX-1.3 验收（V6/V7）。
- [x] AFX-4.6 修复 `install_pending` 超时被误当作滤镜就绪的问题：安装超时不再伪装真实重配置；只有配置节点查询确认成功后才能发布 Applied、释放首样本屏障，配置失败不得由超时重确认伪装成功。验证：actor 回归覆盖 GET_META 显示已配置、缺失节点及坏 graph，超时/错误结果不放行；完整设备 PCM Gate 仍由 AFX-1.3 等项验收（V5/V16）。
- [x] AFX-4.7 复核发现的真机验证阻断（R6）：`/Applications/Echo.app` 原先的 libavfilter 为旧库；任务1.1 增加库一致性硬前置。已将签名有效的 macOS 候选 bundle 安装到 `/Applications/Echo.app`，保留旧包备份；`shasum -a 256` 确认 app 内库与 vendor manifest 均为 `2591332d9a3ccc0f6a7313b98fc2566ef4193dd40271a6908dec7243085df6bc`，结构检查确认四个必需滤镜可用（V12）。
      **⚠️ 2026-10-03**：该任务以手工 `shasum` + 手工结构检查替代了 V12 的原生/性能验收，属替代而非等价覆盖；且当时的结构检查用的是字段布局错误的脚本（见 `native-gate.md`）。哈希结论不受影响，但「四个滤镜可用」应改由修复后的 `inspect-filter-runtime.py`（会校验布局并在不通过时拒绝输出）复现。
- [x] AFX-4.8 auto preamp 保守策略（review-and-tuning.md §4.2）：峰值 headroom 从 1 dB 调整为 0 dB，并在 Auto 策略加 0.5 dB 输出补偿；保留现有 `PreampMode`，不在缺少听感证据时引入响度匹配模式。跨采样率数学压力测试通过；候选 bundle 使用 997 Hz/-0.1 dBFS 输入、1 kHz/+12 dB EQ、-11.5 dB Auto preamp 模拟输出，在 PCM 捕获中测得 sample-peak -0.99985 dBFS、integer PCM（有限值），报告在 native-gate（V2/V3/V6）。
      **⚠️ 2026-10-03 三点限定**：① 该 0.5 dB **无 BS.1770 实测 A/B**，属未验证的调音偏好；② `-0.99985 dBFS` 距 0 dBFS 仅 0.00015 dB，**只证明未削波，不能证明 limiter 保护介入可见**，故不足以关闭 V6 的「保护介入可见」要求；③ 该点仅覆盖**单段** +12 dB，未覆盖 V6 要求的多相邻段/全部 +12/手动 0/空间组合。

## 5. 类型化 IPC 与 UI

- [x] AFX-5.1 集中声明音效commands/DTO和player快照可选投影，生成IPC类型，区分接受请求/持久化/实际生效；验证：`pnpm --dir apps/desktop generate:ipc`、`cargo test -p echo-desktop`、`pnpm --dir apps/desktop typecheck`，序列化/旧字段缺失/旧消费者及未知值失败测试通过，不手改生成物（V16）。
- [x] AFX-5.2 player store接入唯一请求/应用状态，两标签页和普通/沉浸播放栏共用，只有确认生效显示名称；验证：Pending/Failed/Unavailable/恢复/旧回执测试通过`pnpm --dir apps/desktop test`，暂停编辑不触发播放，空间预设无可保存曲线入口（V1/V4/V7/V17）。
- [x] AFX-5.3 构建共宽度音效/均衡器浮层、固定顶部开关、十段推子/真实响应/preamp请求与实际提示及CRUD对话框；验证：`pnpm --dir apps/desktop test`覆盖V2/V4/V5/V8，桌面/窄屏截图确认独立控件占位、无重叠、内部可滚动，不显示原声或P1操作入口；预设文案不宣称行业标准曲线、动态去齿音或高频信息恢复。
- [x] AFX-5.4 在既有浮层栈接入音效/队列互斥、重复点击/外部关闭、Escape单层、非模态Tab离开与对话框焦点；实现tab左右键、推子上下/Home/End、读屏语义/禁用原因及礼貌/合并播报，更新`docs/interface-terminology.md`；验证：`pnpm --dir apps/desktop test`与`pnpm --dir apps/desktop test:e2e`覆盖V15和shell delta，外部点击与Tab离开保留目标焦点，焦点不误锁/误拉回，内部操作不误关。
- [x] AFX-5.5 完成UI失败重试/未保存反馈和正常退出草稿回归，同步原型与实现差异说明；验证：写入失败输入/列表/草稿保留、快速输入尾值/改名/删除正确，执行`pnpm --dir apps/desktop format:check`、`lint`、`typecheck`、`test`、`build`（V8/V11/V16）。

## 6. 集成验收与交付（不替代前述就近验证）

- [ ] AFX-6.1 在 macOS 候选安装包运行已登记原生与E2E命令，完成 V1–V8/V10–V18 逐场景证据和离线/临时/后台/故障矩阵；验证：复用 native-gate.md 捕获命令并记录包/机器/素材和结果，100次延迟、5分钟 CPU/underrun 与听感复核全达标；V9/V19不标P0完成。
- [ ] AFX-6.2 执行Rust全量`cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace`及UI第5.5命令、`pnpm --dir apps/desktop test:e2e`、`pnpm verify:governance`；验证：登记后运行对应`pnpm verify:task -- <实际ID>`/`pnpm verify:scenario -- <实际ID>`，IPC/架构/场景覆盖均通过。
- [ ] AFX-6.3 复核PRD决策、专业来源与参数证据分级及产物，更新验收证据及0.2.0发布文档；验证：所有P0任务已完成且`openspec validate introduce-audio-effects-equalizer --strict`通过，再同步delta并执行`openspec validate --specs --strict`，归档本change后才创建PR，不将规划状态等同功能交付。

## 7. 最后执行：Windows/Linux 跨平台 Gate

- [ ] AFX-7.1 为 Windows 与 Linux 各固定候选安装包、libmpv/FFmpeg 版本/哈希、参考机和捕获设备；按任务 1.1 建立同等工具记录。
- [ ] AFX-7.2 在 Windows 与 Linux 候选库补跑任务 1.2 的滤镜链/原地命令/单声道/峰值/loadfile 实验，逐平台附带原始报告。
- [ ] AFX-7.3 逐平台补跑任务 1.3 的 100 次延迟捕获及任务 1.4 的性能、underrun 和听感验收；随后回填跨平台差异与 6.1 的总 Gate 结论。未经这些证据不得宣称三平台完成。

## 8. 实现复核修复（R7–R11；不替代原生 Gate）
- [x] AFX-8.1 修复不支持环境中的旁路过渡提前完成和安全状态残留（R7）；验证多轮 Pending→Unavailable、旁路失败持续阻止播放、旧失败状态恢复以及 mono→stereo，运行 `cargo test -p echo-desktop player::actor_effects`。
- [x] AFX-8.2 音效首样本屏障期间不发布 Playing，确认放行后才进入播放状态（R8）；验证 FileLoaded/Play/Toggle/输出重建，以及迟到重配置不复活已结束曲目，运行 `cargo test -p echo-desktop player::actor`。
- [x] AFX-8.3 读取快照失败后的重试仅重复读取，不提交音效或修复偏好（R9）；连续两次失败再成功，断言全为 GET，运行 `pnpm --dir apps/desktop test -- src/features/player/EffectsPanel.test.tsx`。
- [x] AFX-8.4 增加独立非持久化快照序号，拒绝同 revision/epoch 旧快照覆盖 CRUD 结果（R10）；不增加音频 revision/后端调用；运行 `cargo test -p echo-desktop effects_service`、`pnpm --dir apps/desktop test -- src/player/playerStore.test.ts` 与 IPC 生成漂移检查。
- [x] AFX-8.5 独立投影恢复失败原因并显示明确恢复失败提示，保留原数据直到显式修复成功（R11）；运行 service/DTO/Panel 回归，读取和普通写入失败不混淆。
- [x] AFX-8.6 完成修复后 Rust/前端全量回归及 OpenSpec 严格校验，记录现有治理失败与未完成原生 Gate，更新 `review-and-fixes.md`。Rust workspace/严格 Clippy/格式、前端 327 项测试/typecheck/lint/format/build/E2E、规模/IPC 漂移通过；治理对账仍有既有 45 项错误，原 2.4/6.2 不标完成。

## 9. 运行时改参失效（2026-10-03 实测发现，阻塞 P0）

- [x] AFX-9.1 修正判据工具并给出可信结论：`inspect-filter-runtime.py` 按 FFmpeg 6.0 真实字段布局读取（`inputs`/`outputs` 在 `priv_class` 之前、pad 计数为 `uint8`、`priv_size` 在 offset 80），并用导出的 `avfilter_filter_pad_count()` 交叉校验，断言不通过即拒绝输出逐滤镜结论。**更正**：随包库 `equalizer`/`volume`/`alimiter`/`extrastereo` 均有 `process_command`，此前「`extrastereo` 无回调」为布局错位造成的假 False。
- [x] AFX-9.2 建立运行时改参探针：`probe.py` 新增 `--runtime-parameter-proof`（PCM 前后对比）与 `--runtime-capture-seconds`（固定捕获窗），并新增 `static_gain_reference` 自检。**原结论已撤回**：该轮命令使用错误的 mpv label，不能判定运行时改参是否有效。探针仍可用于后续正确 label 的 PCM 判别；具体状态见 AFX-9.5、AFX-9.7 与 `native-gate.md`。
- [x] AFX-9.3 同步规格与实现：spec「空间感预设」Requirement 改为「width 是安装期常量 + 不得以 rc=0 报告生效」并补第 4 个 Scenario；`design.md` 第 4 节与 `native-gate.md` 更正 `latency`/`process_command`/证据作用域表述；`native_effects.rs` 模块文档记录该限制与待验证状态；任务 AFX-4.1/4.2/4.5/1.3 加阻塞或重新定性说明。`openspec validate --strict` 通过。
- [x] AFX-9.4 消除任务号撞名并加门禁：本 change 的 39 个任务号加 `AFX-` 前缀（原先 1.1/1.2/2.2/4.5/4.7/4.8/6.1/6.2 等与 `scripts/verify/manifest.json` 现有条目**撞名**，`pnpm verify:task -- 4.8` 会跑到 BLAKE3 测试并报 ok）；`validate-scenario-commands.mjs` 白名单接纳 `scripts/audio-effects/*.py`（路径锁定）；新增 `scripts/verify/checks/task-audio-effects-evidence.mjs` 并登记为 `AFX-1`，覆盖撞名、失效结论回流、探针能力退化。**判别力已验证**：去掉前缀即 rc=1。
- [x] AFX-9.5 定位运行时改参未改变 PCM 捕获的根因：2026-10-08 持续捕获确认旧短输入在发命令前已高速解码 EOF，并非随包滤镜失效。修复 `probe.py --runtime-parameter-proof` 后，正确 label 的 EQ +6/preamp −6/空间 1.25→1 均符合理论（误差≤0.25 dB），主 Agent 独立复测记录于 `evidence/macos-runtime-parameter-recheck-20261008.json`；静态对照自检通过。本项解除参数机制阻塞，不替代 AFX-4.2/1.3 的 30 ms 输出瞬态及三平台 Gate。
- [x] AFX-9.6 完成 design §3 承诺的 ≤0.25 dB 静态响应验收：使用独立逐频正弦工具完成四率/52 报告/7,774 频点比较，最大实测差 0.000006809483 dB、保守舍入界 0.000134970 dB。旧 `probe.py --sweep-response` 仍为 `usable_for_tolerance_verdict: false`，不作为证据；详见 AFX-11.1 与 `native-gate.md`。此项不替代 limiter、实时或三平台 Gate。
- [x] AFX-9.7 撤回 AFX-9.2 的「运行时改参整体失效」结论（2026-10-04）。该结论所依据的实验使用了**错误的 mpv label**（把 `af` 属性显示的滤镜名当成 label），因此整个判别链无效。撤回内容：`native-gate.md`「运行时改参一律无效」、`review-and-tuning.md` R1 的「实测不改变音频」表述、以及 `native_effects.rs` 模块文档中的实测数据表。**保留的事实**：随包库四个滤镜均有 `process_command`（结构层，判据已修正）；`af-command` 的 rc 不能作为生效证据；用 PCM 前后对比才是唯一可接受判据。当时根因仍未定位；macOS 后由 AFX-9.5 确认为短素材在命令前到达 EOF。
- [x] AFX-9.8 记录 mpv `af-command` 的 label 语义（2026-10-04，供后续排障复用）：`af add/set` 的 `@label:NAME` 中 `label` 是 `@` 与 `:` 之间那段，`NAME` 是滤镜名（`options/m_option.c:3177-3196`）；`af` 属性返回的 `name` 是**滤镜名而非 label**（因此 `@x:lavfi=[...]` 显示为 `lavfi` 属正常，**不能**据此推断 label 不可用——这是本轮踩过的坑）；`af-command <label> …` 经 `find_by_label()` 在 user_filters 中匹配（`filters/f_output_chain.c:421-451`），找不到即 rc=-12；`target=all` 走广播分支（`f_lavfi.c:439`），会无条件返回 true，属另一处假成功来源。

## 10. 2026-10-08 专业音频复核修复

- [x] AFX-10.1 修复运行时 EQ/preamp 参数生效判据：使用正确具名 target、持续 PCM 捕获及按实际样本窗口测量，记录静态对照与运行中 0→+6 dB / preamp −6 dB 的变化，不以 rc=0 替代生效。验证：`python3 -m unittest discover -s scripts/audio-effects -p 'test_*.py' -v` 与 `probe.py --runtime-parameter-proof`，原生低电平 997 Hz 增益变化 ≥5.5 dB；具体可复现命令及候选库哈希记录于 native-gate.md。
- [x] AFX-10.2 修复兼容 EQ 编辑的 preamp 阶跃及过度保护：从实际中间状态连续过渡，预算只计算安装的有效频段，处理增益先降风险再升风险，最终收敛至最新目标；回归覆盖摇滚曲线不额外下降 12 dB、22.05 kHz 停用 16 kHz 不参与预算、连续编辑后关闭/重置及故障。验证：`cargo test -p echo-desktop`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- [x] AFX-10.3 修复空间启用/关闭的实际宽度过渡：保持用户 width 固定 1.25 且不开放编辑，原生 PCM 证明内部过渡机制有效后才使用；旁路必须实际回到 m=1 并恢复 preamp 后移除链，不能仅更改 Rust 内部 width。验证：原生同相/反相/单侧输入 PCM 与 `cargo test -p echo-desktop`，保护持续至过渡尾部。
- [x] AFX-10.4 更正限幅预算与证据口径：+0.5 dBFS 输入相对 −1 dBFS limiter 阈值需约 1.5 dB 稳态衰减，增加前置增益不等于减少限幅或固定提高节目响度；同步设计、规格及当前复核记录。验证：`openspec validate introduce-audio-effects-equalizer --strict`、`pnpm verify:task -- AFX-1`；三平台设备 Gate 仍独立记录。

## 11. DSP 专业复核待补项（2026-10-09 只读复核，阻断项均未闭合）

> 本轮以 FFmpeg n6.0 上游源码 + `math.rs` 同式独立复算，确认 RBJ 系数、`G` 的乘积计算、`extrastereo` 的 M/S 语义、`level=false` 必需性、`attack/release` 默认值与 `-0.99985 dBFS ↔ limit=0.891250938` 的自洽性；同时修正了若干文档语义：`extrastereo` 的 `c` 是削波开关而非 center coefficient、`latency=true` trim/pad 机制、脉冲峰值的线性模型解释、30 ms 措辞、`spatial:m` 历史结论和 0.5 dB 标注位置。**下列原生测量仍未完成，是 0.2.0 的实际阻断点。**

- [x] AFX-11.1 **逐频正弦扫频响应**。独立工具 `scripts/audio-effects/sine_sweep.py` 捕获四率×13 条曲线（9 个内置预设 + 4 个压力曲线），总计 52 份报告、7,774 个频点比较；各率采样点数为 146/150/151/151，观察上限 `min(20 kHz,0.499Fs)`，有效中心频率截止仍为 0.45Fs。最大原生与复合 RBJ 响应差为 0.000006809483 dB，最大保守舍入误差界为 0.000134970 dB，均低于 0.25 dB。证据：`evidence/macos-stepped-sine-20261009/summary.json`；报告绑定同一工具源 hash。该项仅验收 macOS 低电平静态响应，不代表 limiter 高压力、实时过渡或三平台 Gate。
- [ ] AFX-11.2 **源 Fs ≠ 滤镜图实际 Fs**（`fc<=0.45*Fs` 判据用哪个率）。现有矩阵没有图内 link 采样率观测。分别记录源/decoder 率、AO协商率和 equalizer link 实际 `sample_rate`；判据是预算、有效段判断、系数及响应采用的模型 Fs 与 equalizer 实际 link Fs 相同。源率与 AO 率可以不同；若模型率不匹配图内率，修复取值并重测停用段行为。
- [ ] AFX-11.3 **BS.1770-5 等响度 A/B**（`AUTO_HEADROOM_DB=0.5` 定稿）。`evidence/` 目录**无任何** BS.1770/响度/LU 字段，该 0.5 dB 是唯一改变全链路电平的决策且方向为「让 limiter 多干活」，目前是纯推算。目的：把 0.5 dB 从推算变实测；命令：对 9 预设 × {中性, safe, safe+0.5} 测积分响度并制作差 ≤0.5 LU 的匹配副本，≥2 名审查者盲听；判据：记录三态响度差与盲听能否分辨；通过阈值：匹配响度下不能稳定分辨 ⇒ 0.5 dB 无可闻收益，退回 `AUTO_HEADROOM_DB=0.0`（需产品决策，见 AFX-11.6）。
- [ ] AFX-11.4 **脉冲线性解释复核**。四档采样率脉冲首样本峰值与无 limiter 线性模型的差均小于 0.00044 dB，低峰由 `b0` 与 preamp 的线性衰减解释；这不是 attack 保护失败的证据。保留该数值为既有计算观察，不把其写成新的原生完整验收。若后续捕获 limiter 行为，比较 limiter 前后浮点样本、增益变化及限幅介入；不得设输出峰值下限，反相用例应独立验证空间矩阵最坏放大与 EQ 压力，不把互斥 EQ/空间模式组合成单一验收场景。
- [ ] AFX-11.5 **`latency=true` 首尾样本与时序实测**。FFmpeg n6.0 的 trim 裁切 look-ahead 延迟输出，EOF 以输入零样本排空缓冲；源码参数不能直接证明节目内容丢失。比较同素材旁路与 limiter 输出的帧数、首尾标记/波形、seek/EOF 内容和时间戳；通过阈值：记录真实内容差异，且相对旁路的输出、seek/EOF、进度新增偏移≤10 ms。不要把源代码机制预设成丢失开头节目样本或尾部静音替换。
- [ ] AFX-11.6 **待产品决策**：`AUTO_HEADROOM_DB` 当前 0.5 dB。选项 (A) 维持 0.5 并补 AFX-11.3 证据；(B) 退回 0.0（=纯 `safe`）先行发布，0.5 留待 A/B 定稿。**此项不由技术复核单方决定**，须用户裁决后执行，并同步 `design.md` 第3/4 节、`math.rs::AUTO_HEADROOM_DB` 与相关单测期望值。

### 已由本轮复核确认正确、整改时不得改动的结论

- RBJ peaking 系数推导三处自洽（`design.md` / `math.rs` / FFmpeg `af_biquads.c:842-847`）；`t=q` + `w=√2` 的 `alpha=sin(w0)/(2√2)` 与 RBJ 一致。
- `mix=1, normalize=false, precision=f64, block_size=0` 无电平/精度副作用（`normalize` 默认已是 0；本实现必须为 false，否则 `:994-1000` 会额外缩放分子）。
- 中心频率严格 ×2 排布与「不宣传每段严格一倍频程」**不矛盾**（排布是倍频程、带宽不是）。+12 dB 时 peaking −3 dB 带宽随频率压缩的当前计算为：1 kHz 约 0.542 octave、16 kHz 约 0.225 octave。历史文本中的 cut 谷宽 3.32 octave 来自不适用的阈值/搜索括区，应删除该结论；RBJ +g/−g 曲线互为逆响应。
- `G` 用 `product(H_i)` 逐频累加，正确覆盖级联叠加。本轮复算：单段 +12 为 +12.000 dB；相邻 1/2 kHz 两段 +12 的峰为 **+14.564616 dB @约1025.35 Hz**；全十段 +12 为 **+18.402800 dB @约499.90 Hz**。这表示级联提升超过单段，但低于推子数值的简单相加；不是滤镜之间的抑制机制。
- 九条曲线 Auto 值按标准四舍五入到两位为 classical −0.11、electronic −3.18、vocal −1.82、bass −4.35 dB。
- **密栅格复核状态已更新**：旧 review 计算的九预设单条件结果约 3.165e−7 dB 已被当前 48 组门禁覆盖取代。现代码每组遍历 1,048,577 个频点；与独立极值和左右邻点检查相比，最大观测高估为 0.00000547 dB，未观察到低估。该实测不构成严格全局误差上界。
- 「频域估计 ⇒ 任意瞬态安全」的漏洞已在 `design.md` 第4 节封住（明确 G 为稳态估计、振铃可超限、limiter 充分性不由 G 保证）。
- `level=false` 必需（`level` 默认 1 会归一化回 0 dB、破坏 preamp 预算）；`asc=false` 关闭的是自动释放控制；`level_in/out=1` 直通；`attack=5`/`release=50` 确为 FFmpeg 默认（`af_alimiter.c:82-83`）。
- `latency=true` 以 trim/pad 补偿 look-ahead；EOF 补入的是输入零样本以排空缓冲。是否影响首尾节目内容须捕获确定，不由源码 trim/pad 数字直接推断。
- `extrastereo` 的 `c=false` 关闭内部削波是正确决策（符合「不可先硬削波再衰减」）；M/S 语义与实现一致；L=R 时任意 m 增益恒为 1，**中央信号不下沉**；最坏放大 1.25/1.9382 dB 仅在反相时取到，单侧为 +1.0231 dB。
- `fc<=0.45*Fs` 段退化清单：22050 → 仅 16000 Hz；44100/48000/96000 → 无退化。实现（`math.rs:30`、`native_effects.rs:413`）与响应图绘制三处一致。
- `-0.99985 dBFS` 与 `limit=0.891250938` 自洽（`20log10(29205/32768)`），说明限幅器**在动作**。
- R10 已真正闭合（`effects_service/mod.rs:75-83` 服务锁内单调分配，`tests.rs:250-255` 断言 CRUD 前后序号单调），不只是前端拒绝旧序号；R11 亦已闭合。
- AFX-1 门禁已登记进 `scripts/verify/manifest.json`（非死门禁），但**只覆盖证据纪律，不覆盖任何 DSP 结论** —— 见 AFX-2。

## 12. 2026-10-09 复核修复与待验证回归

> 本节按实际验证结果记录状态。软件回归通过不代表完整原生 Gate 通过；设备捕获、高压力 limiter 和三平台发布任务仍保持未完成。

- [x] AFX-12.1 修复并回归首样本屏障确认：滤镜安装超时不能模拟真实重配置或配置成功；节点未就绪/配置失败时不得发布 Applied 或放行音频。actor 回归覆盖 native GET_META 确认已配置/缺失/坏 graph，以及暂停失败后重试；native agent 与独立 reviewer 未发现剩余阻断项。workspace 测试通过（1 项既有 ignored），严格全目标/全功能 Clippy 通过。此项软件回归不替代设备输出 PCM 和时延 Gate。
- [x] AFX-12.2 修复 EQ 编辑器的环境有效段和状态呈现：基于目标 EQ 的 `editableBands` 判断编辑能力，不复用空间链 `activeBands`；分别呈现手动请求 preamp、确认实际值及限制原因；提供 Auto/Manual 切换且保留 EQ gains 与手动请求值；无音轨/参考图/空间预设/待应用环境分别验证。旧快照缺少新增可选字段时仍兼容。修复编辑提交失败后的重试：重新提交保留的曲线；恢复保护先修复后重提，读取失败仍只读取。最终前端 37 个测试文件、338 项测试及 typecheck/build/lint（0 errors）/format 通过，独立审查未发现剩余阻断项。
- [x] AFX-12.3 增加独立 DSP 数学回归：真实遍历 2²⁰ 间隔密栅格，覆盖九条预设、全段 +12、相邻 +12、交替增益及四种处理率；对局部极值搜索做独立求解/左右邻点检查；校验相邻两段 +12 峰约 14.564616 dB、全段 +12 约 18.402800 dB。报告为 12 条曲线 × 四率、每组 1,048,577 个频点；最大观测高估 0.00000547 dB，未观察到低估。验证：math gate 与 mutation/extrema 自检覆盖 48 组通过；Rust `effects::math` 10 项数值测试通过。此对照不构成严格全局误差上界。
- [ ] AFX-12.4 建立 limiter 与完整响应的原生诊断证据：低电平 stepped-sine 四率 52 份报告已完成，最大响应差 0.000006809483 dB、保守舍入界 0.000134970 dB（AFX-11.1）；证据仅覆盖静态低电平响应。limiter 前后浮点 PCM、增益变化及高压力链诊断尚未完成。后续以明确工具/算法记录 dBTP，覆盖空间与 EQ 互斥场景、噪声/脉冲/扫频、首尾内容及 seek/EOF 对齐，不设输出峰值下限。该任务由 AFX-11.1/11.5 与 AFX-1.3 的正式 Gate 共同验收，不能替代三平台验证。
