# Tasks

> 2026-10-02 实施授权：用户在本轮开发中回复「我批准你通过」，允许先推进功能实现。第1组实测和第6组发布验收仍须补齐；此授权不代表任何平台测量已经通过，未取得证据的任务不得勾选。
>
> 2026-10-03 复核新增：见 `review-and-tuning.md`。R1（**运行时改参整体失效**，原归因「`extrastereo` 无回调」已被 PCM 实测证伪）、R2（`install_pending` 潜在死锁，任务 4.6）、R6（app 内原先是旧库，任务 4.7）为复核发现并已登记；听感优化见任务 4.8 与 2.5。已完成候选 app 更新和库一致性检查；完整音频 Gate 状态不变。
>
> 2026-10-03 修复轮实测结论（**优先于上述复核记录**）：本随包库上 `af-command` **对任何滤镜都不改变音频**（EQ 增益 / preamp / width，rc 均 0 而 PCM 输出不变），mpv 因 `f_lavfi.c` 只判 `result >= 0` 报假成功。**因此任务 4.1/4.2 赖以成立的「运行时原地改参」当前不可依赖**，须改为「重建链 + 实测确认」。详见 `native-gate.md` 的判别性测量表。

## 1. macOS 原生技术 Gate（用户授权先开发，macOS P0 发布前须通过）

- [x] AFX-1.1 固定 macOS 候选 bundle 及 libmpv/FFmpeg 版本/哈希，补齐可重复 fixture、CoreAudio 探针和隔离 `ao=pcm` 捕获；运行时库哈希、协商采样率/声道和真实处理输出均写入 `native-gate.md` 与证据 JSON（T1/T5、V12/V18）。当时的 `/Applications/Echo.app` 旧包已在 4.7 更新并留备份。
- [x] AFX-1.2 macOS 候选库已验证十段具名链、EQ/preamp 定向命令、空间链 `c=false`、limiter `level=false` 和 loadfile 后配置链存活；低电平 997 Hz 响应误差 `-0.0041 dB`，压力点 sample-peak `-0.99985 dBFS` 且 PCM 全有限；单声道输入保持 mono，空间链被跳过；滤镜 target/options 和 sample-peak/true-peak 未测口径已记入 native-gate。证据：`macos-installed-app-coreaudio-probe.json`、`macos-installed-app-mono-probe.json`、`macos-pcm-response-probe.json`、`macos-pcm-limiter-stress-probe.json`。这只是 1.2 指定的单点实验，不替代 1.3/1.4 矩阵或总体发布 Gate（T1/T2/T3/T5/T6、V2/V6/V7/V17）。
      **⚠️ 2026-10-03 与 `native-gate.md:124` 的表述冲突已消除**：该处曾要求 1.2 保持未勾选，现统一为「1.2 = 已完成的单点实验，Gate 仍由 1.3/1.4/1.5 决定」。另：`-0.0041 dB` 来自**安装期写死 g=6 的静态链**，只证明静态响应；EQ/preamp 定向命令的 rc=0 **不构成**生效证据（实测不改变输出）。
- [ ] AFX-1.3 在 macOS 捕获 30ms 平滑目标、开关/类型切换/输出重建/首样本屏障与 limiter 延迟补偿；验证：每类≥100次捕获 p95≤150ms、无新增爆音/断流/underrun，seek/EOF/歌词进度相对旁路无超过10ms新增偏移；volume/mute 不被覆盖（V10/V16–V18）。
      **⚠️ 2026-10-03 阻塞前提**：30ms 平滑当前依赖逐帧 `af-command`，而该路径已实测**不改变音频**（见 `native-gate.md`）。本任务须先确定可生效的过渡机制（重建链或替代方案），并以 PCM 捕获证明中间增益状态真实出现，否则「无爆音/无断流」无从验证。另：`latency=true` 经实测**确实补偿**延迟（48 kHz/attack=5 ms 抵消 239 样本，与 `attack*Fs−1` 精确一致），故 limiter 自身的延迟补偿项已成立，本任务改为复核 CoreAudio 设备侧总偏移。
- [ ] AFX-1.4 固定 macOS 参考机（至少4核/8GB/SSD）和最重预设，预热30秒采样5分钟，分别测稳定播放及面板开关；验证：CPU平均增量≤10个百分点，记录原始数据。对九个EQ及空间按BS.1770-5测量同片段、响度匹配≤0.5 LU 后做A/B和人声折叠审查，记录实测结果及调音迭代（V1/V3/V7/V12）。
      **⚠️ 2026-10-03**：`evidence/` 中**无任何 BS.1770 测量文件**，design 的 0.5 dB 自动补偿与 AFX-2.5 的曲线定稿都依赖本项，须优先补做。
- [ ] AFX-1.5 汇总 macOS Gate 结论并更新 design 实测证据；验证：T1/T2/T3/T5/T6及上述门槛全通过。失败须先修订方案及受影响 PRD/spec，不可将空间或原地编辑禁用后标完成。T4、V9/V19留P1。

## 2. 音效纯规则、注册表与状态机

- [x] AFX-2.1 在desktop内部建立类型化Preset/Payload/请求与Applied状态、保留参数和revision/epoch规则，按design表实现选择/编辑/关闭/重置；验证：就近状态机测试覆盖关保存项与关草稿差异、快速输入后关闭/重置、环境变更/旧确认、草稿来源删除（V4/V5/V16），运行`cargo test -p echo-desktop`。
- [x] AFX-2.2 实现十段固定Q系数、有效频段、合成响应、安全preamp及空间增益规则和专业资料复核后的九个注册表初值；验证：单/相邻/全段±12、手动0、全零Auto、22.05kHz及边界数学测试，复算design的48kHz旧/新曲线合成峰值，并覆盖44.1/48/96/22.05kHz；数学检查不冒充音频实测，内置调音变化不改用户曲线/草稿快照，计算误差≤0.1dB且值有限，运行`cargo test -p echo-desktop`（V2/V3/V6/V18）。
      **⚠️ 2026-10-03**：本任务覆盖 V18（采样率/输出重建）**仅靠纯数学测试**，属以软件测试替代原生测量，V18 的原生部分仍待 1.3/4.3。过渡期余量已统一到 `AUTO_HEADROOM_DB`（原过渡用 1.0 dB、稳态用 0.5 dB 的分歧已修）。
- [x] AFX-2.3 实现用户命名、快照、独立ID及50项额度纯规则，复用既有NFKC/空白/full-case-fold；验证：1/40/41 grapheme、空白/组合字符/大小写/内置重名、49/50/51项、重命名额度测试通过`cargo test -p echo-desktop`，不改变歌单或Core API（V4/V8）。
- [ ] AFX-2.4 登记需求/场景/任务与命令到现有验证manifest，维护模块/端口职责说明和适用规范；验证：`cargo test --workspace`、`pnpm verify:governance`通过，新文件不超规模、trait不超6方法，Core无音效依赖（audio-effects架构契约）。
- [ ] AFX-2.5 复核发现的听感问题：九条内置曲线幅度过小（多数 ±0.5–2 dB，`classical` 仅 ±0.5 dB），且 auto preamp 按合成峰值扣除导致响度差盖过音色变化。按 `review-and-tuning.md` 4.3 迭代幅度（单段 ≤±6 dB，禁止相邻同号大幅提升叠加），**新数值须走任务 1.4 的 BS.1770-5 响度匹配盲听定稿后才写入 `presets.rs`**；首尾两段 shelf 化会突破「十段固定 Q peaking」spec 边界，属 P0 范围变更，需先修订 spec。验证：盲听记录通过后 `cargo test -p echo-desktop effects::presets`，并同步 `design.md` 第3节的曲线表与证据分级（V2/V3）。

## 3. 本机偏好与管理用例

- [x] AFX-3.1 通过聚焦偏好端口接入DesktopStateStore新增版本化effects字段与原子读改写，编辑250ms合并写/退出flush；验证：旧数据默认关闭、主题/窗口/播放会话/未知字段保留、写失败显示未保存且尾值可重试，运行`cargo test -p echo-desktop`（V11/V14/V16）。
- [x] AFX-3.2 实现保存、重命名、删除串行事务及删除当前项的旁路/存储补偿，成功落盘后才发布列表；验证：关闭草稿保存不启用、改名不重建链、删除非当前/草稿来源不改变播放，以及旁路/写入/补偿失败注入，原列表/输入/草稿保留且不报假成功，运行`cargo test -p echo-desktop`（V4/V8/V16）。
- [x] AFX-3.3 实现恢复合法请求、保留原文档/可恢复项及未来版本保护；验证：损坏JSON、非法数值/载荷、悬空ID、未知schema/registry不盲启用不覆盖，恢复后待确认且不自动发声，正常重启保持草稿/请求，运行`cargo test -p echo-desktop`（V11/V17）。
- [x] AFX-3.4 更新`docs/PRODUCT.md`的本机音效与同步边界、`docs/ROADMAP.md`的0.2.0完整P0/P1独立范围，以及本机状态/恢复说明；验证：与proposal/spec一致，无SQLite/outbox/echo控制面新增音效字段，执行隔离测试与`cargo test --workspace`（V13/V14）。

## 4. 播放后端与生命周期集成

- [ ] AFX-4.1 基于Gate确定的API在MpvBackend实现命名链与运行时参数更新，固定浮点精度/preamp/空间/limiter配置，复用现有FFI隔离；验证：原生工具输出与第1组一致，压力/响应/空间折叠通过，运行`cargo test -p echo-desktop`；文档记录unsafe前提（若有）及精确链语法（V1/V2/V6/V7）。
      **⚠️ 2026-10-03 阻塞前提**：`native-gate.md` 的 PCM 判别性测量显示本随包库上运行时 `af-command` 对任何滤镜都不改变输出（rc=0 是假成功）。本任务须先确定「重建链」还是「另寻可生效的运行时机制」，并以 PCM 前后测量判定，**不得**以 `af-command` 返回 0 作为生效证据。
- [ ] AFX-4.2 actor在下一轮处理最新请求（不额外debounce）、保留30ms平滑、保护先后序和revision/epoch屏障；新链安装后等待libmpv重配置再更新具名参数。验证：fake故障测试及原生拖动后关闭/重置/切类型捕获均以最新状态收敛，失败回滚或旁路，过期步骤不能重新作用，切换首个音频参数更新无额外合并等待，运行`cargo test -p echo-desktop`（V5/V10/V16）。
      **⚠️ 2026-10-03 阻塞前提**：`advance()` 的 30 ms 斜坡依赖逐帧 `af-command`，而该路径已实测不改变音频；`start()` 在 incompatible 分支还会于 `install()` 时直接写入最终 `preamp`、并在旁路末尾由 `protected_preamp` 阶跃到目标值。改造时须把 preamp 纳入斜坡并消除该阶跃，且以 PCM 实测证明中间增益状态真实出现。
- [ ] AFX-4.3 接入实际处理环境、loadfile/输出重建、暂停/继续/无音轨首样本流程，支持mono/stereo EQ、只双声道空间；验证：22.05/44.1/48/96kHz及既有其他采样率、临时文件、换设备、seek/EOF、歌词/计数/volume/mute回归，通过原生工具与`cargo test -p echo-desktop`（V10/V13/V17/V18）。
- [x] AFX-4.4 runtime统一装配、会话恢复与退出flush/后台关窗，不另建音效会话；验证：窗口隐藏后持续音效，显示后读取同快照、资料库切换和清队列保留曲线，运行`cargo test --workspace`，更新`docs/DESIGN.md`的桌面音效分层/依赖图（V11/V13/V14）。
- [x] AFX-4.5 复核发现的空间宽度假成功（R1）：采用 review 推荐方案 A；`extrastereo` width 只在安装时设置，移除无效 `spatial:m` 运行时命令，并以负向回归断言确保不再发送。随包库结构检查及应用包探针已检查该命令路径；不选择重建链方案 B，因其需要尚未取得的连续性捕获证据（V6/V7）。
      **⚠️ 2026-10-03 重新定性**：负向断言**保留**（仍不应发送 `spatial:m`），但**理由已变**——原依据「`extrastereo` 无 `process_command`」被证伪（该回调存在）；真实依据是「本随包库上运行时改参对所有滤镜都不生效」（PCM 实测：装 `m=1.25` 发 `spatial:m 1.4`，rc=0 而输出 Δ=+0.0000 dB，理论 +0.9844）。复核命令：`probe.py --capture-only --runtime-parameter-proof`。
- [x] AFX-4.6 复核发现的 `install_pending` 潜在死锁（R2）：若 AUDIO_RECONFIG 未到达，超过一个 `TRANSITION` 后自动重新确认，避免后续 `apply`/`bypass` 永久静默早退；新增不调用手工 `reconfirm()` 的超时恢复单测，验证无事件时下一次 `apply` 下发 `af-command`（V5/V16）。
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
- [x] AFX-9.2 建立判别性测量并纠正结论：`probe.py` 新增 `--runtime-parameter-proof`（命令值必须区别于安装值 + PCM 前后对比）与 `--runtime-capture-seconds`（固定捕获窗），并新增 `static_gain_reference` 自检。**实测结论**：本随包库上 EQ 增益 / preamp / width 的运行时改参**均不改变输出**（rc=0 而 Δ≈0.000 dB），而安装期静态增益实测 +5.9959 dB 正常。判定为**运行时改参整体失效**，非 ENOSYS 掩盖。证据与复现命令见 `native-gate.md`。
- [x] AFX-9.3 同步规格与实现：spec「空间感预设」Requirement 改为「width 是安装期常量 + 不得以 rc=0 报告生效」并补第 4 个 Scenario；`design.md` 第 4 节与 `native-gate.md` 更正 `latency`/`process_command`/证据作用域表述；`native_effects.rs` 模块文档记录该限制与实测数据；任务 AFX-4.1/4.2/4.5/1.3 加阻塞或重新定性说明。`openspec validate --strict` 通过。
- [x] AFX-9.4 消除任务号撞名并加门禁：本 change 的 39 个任务号加 `AFX-` 前缀（原先 1.1/1.2/2.2/4.5/4.7/4.8/6.1/6.2 等与 `scripts/verify/manifest.json` 现有条目**撞名**，`pnpm verify:task -- 4.8` 会跑到 BLAKE3 测试并报 ok）；`validate-scenario-commands.mjs` 白名单接纳 `scripts/audio-effects/*.py`（路径锁定）；新增 `scripts/verify/checks/task-audio-effects-evidence.mjs` 并登记为 `AFX-1`，覆盖撞名、失效结论回流、探针能力退化。**判别力已验证**：去掉前缀即 rc=1。
- [ ] AFX-9.5 定位运行时改参失效的根因（**未完成，阻塞 AFX-4.1/4.2**）。已排除：mpv 补丁不在（`otool` 确认 `strchr` 分割与 `"all"` 回退）、`command-list` 签名不符、目标名不匹配（`spatial`/`extrastereo`/`all` 与 `echo_*` 均试过）、命令早于滤镜实例化（loadfile 前后均试过）、limiter 饱和与位深误算。**待查**：`@label:lavfi=[...]` 语法下 `af` 里 label 全为 `lavfi`（裸滤镜名才给出各自 label），怀疑 mpv 的 `<target>:<option>` 补丁与该 label 语义交互异常。判据：`af-command <真实label> eq5:gain 6` 使 997 Hz 响应实测变化 ≥5.5 dB（`static_gain_reference` 同时保持 `harness_valid`）。
- [ ] AFX-9.6 补做 design §3 承诺的 ≤0.25 dB 响应验收（**未完成**）。已加 `probe.py --sweep-response`，但窗包络法测的是窗内频率内容而非滤波器响应（误差随窗长从 +10.2 dB 单调收敛到安装增益 +5.949 dB），故显式输出 `usable_for_tolerance_verdict: false`。需改为逐频正弦扫频或窄于滤波器带宽的 FFT 分析。
