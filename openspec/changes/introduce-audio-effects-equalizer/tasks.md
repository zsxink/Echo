# Tasks

## 1. 实施前原生技术 Gate（未通过不得进入第2组）

- [ ] 1.1 建立最小原生音效实验和离线生成素材工具，锁定三平台实际包/libmpv/FFmpeg版本、校验和与参考机；记录可重复的命令、捕获工具及素材许可/hash到本change的native-gate.md。验证：三平台都能运行工具并观察实际处理采样率/声道/输出重建，不能以源元数据或fake替代（T1/T5、V12/V18）。
- [ ] 1.2 在实际库测试十段具名链、gain/preamp原地命令、空间c=false、limiter关闭自动增益及loadfile存活语义；验证：低电平响应误差≤0.25 dB、压力峰值≤0 dBFS且全部有限、单声道空间不可用不自动下混，记录输出sample-peak/true-peak的不同口径及工具版本（true-peak仅作诊断，不是合规承诺），报告精确target语法与滤镜选项（T1/T2/T3/T5/T6、V2/V6/V7/V17）。
- [ ] 1.3 验证30ms平滑目标、开关/类型切换/输出重建/首样本屏障与limiter延迟补偿；验证：每类≥100次音频捕获p95≤150ms、无新增爆音/断流/underrun，seek/EOF/歌词进度相对旁路无超过10ms新增偏移；volume/mute不被覆盖（V10/V16–V18）。
- [ ] 1.4 固定每平台至少4核/8GB/SSD参考机和最重预设，预热30秒采样5分钟，分别测稳定播放及面板开关；验证：CPU平均增量≤10个百分点，记录原始数据。对九EQ及空间按BS.1770-5测量同片段并离线匹配响度（≤0.5 LU，替代度量需记录原因）做A/B和人声折叠审查，检查低音/电子的低频瞬态与限幅介入、人声对乐器的影响、古典近中性及差异，记录原理来源/工程初值/实测结果及调音迭代（V1/V3/V7/V12）。
- [ ] 1.5 汇总三平台Gate结论并更新design的实测证据；验证：T1/T2/T3/T5/T6及上述门槛全通过才开始功能实施；失败须先修订方案及受影响PRD/spec并重新评审，不可将P0空间或原地编辑禁用后标完成。T4、V9/V19留P1。

## 2. 音效纯规则、注册表与状态机

- [ ] 2.1 在desktop内部建立类型化Preset/Payload/请求与Applied状态、保留参数和revision/epoch规则，按design表实现选择/编辑/关闭/重置；验证：就近状态机测试覆盖关保存项与关草稿差异、快速输入后关闭/重置、环境变更/旧确认、草稿来源删除（V4/V5/V16），运行`cargo test -p echo-desktop`。
- [ ] 2.2 实现十段固定Q系数、有效频段、合成响应、安全preamp及空间增益规则和专业资料复核后的九个注册表初值；验证：单/相邻/全段±12、手动0、全零Auto、22.05kHz及边界数学测试，复算design的48kHz旧/新曲线合成峰值，并覆盖44.1/48/96/22.05kHz；数学检查不冒充音频实测，内置调音变化不改用户曲线/草稿快照，计算误差≤0.1dB且值有限，运行`cargo test -p echo-desktop`（V2/V3/V6/V18）。
- [ ] 2.3 实现用户命名、快照、独立ID及50项额度纯规则，复用既有NFKC/空白/full-case-fold；验证：1/40/41 grapheme、空白/组合字符/大小写/内置重名、49/50/51项、重命名额度测试通过`cargo test -p echo-desktop`，不改变歌单或Core API（V4/V8）。
- [ ] 2.4 登记需求/场景/任务与命令到现有验证manifest，维护模块/端口职责说明和适用规范；验证：`cargo test --workspace`、`pnpm verify:governance`通过，新文件不超规模、trait不超6方法，Core无音效依赖（audio-effects架构契约）。

## 3. 本机偏好与管理用例

- [ ] 3.1 通过聚焦偏好端口接入DesktopStateStore新增版本化effects字段与原子读改写，编辑250ms合并写/退出flush；验证：旧数据默认关闭、主题/窗口/播放会话/未知字段保留、写失败显示未保存且尾值可重试，运行`cargo test -p echo-desktop`（V11/V14/V16）。
- [ ] 3.2 实现保存、重命名、删除串行事务及删除当前项的旁路/存储补偿，成功落盘后才发布列表；验证：关闭草稿保存不启用、改名不重建链、删除非当前/草稿来源不改变播放，以及旁路/写入/补偿失败注入，原列表/输入/草稿保留且不报假成功，运行`cargo test -p echo-desktop`（V4/V8/V16）。
- [ ] 3.3 实现恢复合法请求、保留原文档/可恢复项及未来版本保护；验证：损坏JSON、非法数值/载荷、悬空ID、未知schema/registry不盲启用不覆盖，恢复后待确认且不自动发声，正常重启保持草稿/请求，运行`cargo test -p echo-desktop`（V11/V17）。
- [ ] 3.4 更新`docs/PRODUCT.md`的本机音效与同步边界、`docs/ROADMAP.md`的0.2.0完整P0/P1独立范围，以及本机状态/恢复说明；验证：与proposal/spec一致，无SQLite/outbox/echo控制面新增音效字段，执行隔离测试与`cargo test --workspace`（V13/V14）。

## 4. 播放后端与生命周期集成

- [ ] 4.1 基于Gate确定的API在MpvBackend实现命名链与运行时参数更新，固定浮点精度/preamp/空间/limiter配置，复用现有FFI隔离；验证：原生工具输出与第1组一致，压力/响应/空间折叠通过，运行`cargo test -p echo-desktop`；文档记录unsafe前提（若有）及精确链语法（V1/V2/V6/V7）。
- [ ] 4.2 actor串行调度16ms合并/尾值、30ms平滑、保护先后序和revision/epoch屏障；验证：fake故障测试及原生拖动后关闭/重置/切类型捕获均以最新状态收敛，失败回滚或旁路，过期步骤不能重新作用，运行`cargo test -p echo-desktop`（V5/V10/V16）。
- [ ] 4.3 接入实际处理环境、loadfile/输出重建、暂停/继续/无音轨首样本流程，支持mono/stereo EQ、只双声道空间；验证：22.05/44.1/48/96kHz及既有其他采样率、临时文件、换设备、seek/EOF、歌词/计数/volume/mute回归，通过原生工具与`cargo test -p echo-desktop`（V10/V13/V17/V18）。
- [ ] 4.4 runtime统一装配、会话恢复与退出flush/后台关窗，不另建音效会话；验证：窗口隐藏后持续音效，显示后读取同快照、资料库切换和清队列保留曲线，运行`cargo test --workspace`，更新`docs/DESIGN.md`的桌面音效分层/依赖图（V11/V13/V14）。

## 5. 类型化 IPC 与 UI

- [ ] 5.1 集中声明音效commands/DTO和player快照可选投影，生成IPC类型，区分接受请求/持久化/实际生效；验证：`pnpm --dir apps/desktop generate:ipc`、`cargo test -p echo-desktop`、`pnpm --dir apps/desktop typecheck`，序列化/旧字段缺失/旧消费者及未知值失败测试通过，不手改生成物（V16）。
- [ ] 5.2 player store接入唯一请求/应用状态，两标签页和普通/沉浸播放栏共用，只有确认生效显示名称；验证：Pending/Failed/Unavailable/恢复/旧回执测试通过`pnpm --dir apps/desktop test`，暂停编辑不触发播放，空间预设无可保存曲线入口（V1/V4/V7/V17）。
- [ ] 5.3 构建共宽度音效/均衡器浮层、固定顶部开关、十段推子/真实响应/preamp请求与实际提示及CRUD对话框；验证：`pnpm --dir apps/desktop test`覆盖V2/V4/V5/V8，桌面/窄屏截图确认独立控件占位、无重叠、内部可滚动，不显示原声或P1操作入口；预设文案不宣称行业标准曲线、动态去齿音或高频信息恢复。
- [ ] 5.4 在既有浮层栈接入音效/队列互斥、重复点击/外部关闭、Escape单层、非模态Tab离开与对话框焦点；实现tab左右键、推子上下/Home/End、读屏语义/禁用原因及礼貌/合并播报，更新`docs/interface-terminology.md`；验证：`pnpm --dir apps/desktop test`与`pnpm --dir apps/desktop test:e2e`覆盖V15和shell delta，外部点击与Tab离开保留目标焦点，焦点不误锁/误拉回，内部操作不误关。
- [ ] 5.5 完成UI失败重试/未保存反馈和正常退出草稿回归，同步原型与实现差异说明；验证：写入失败输入/列表/草稿保留、快速输入尾值/改名/删除正确，执行`pnpm --dir apps/desktop format:check`、`lint`、`typecheck`、`test`、`build`（V8/V11/V16）。

## 6. 集成验收与交付（不替代前述就近验证）

- [ ] 6.1 在三平台候选安装包运行已登记原生与E2E命令，完成V1–V8/V10–V18逐场景证据和离线/临时/后台/故障矩阵；验证：复用native-gate.md捕获命令并记录包/机器/素材和结果，100次延迟、5分钟CPU/underrun与听感复核全达标；V9/V19不标P0完成。
- [ ] 6.2 执行Rust全量`cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace`及UI第5.5命令、`pnpm --dir apps/desktop test:e2e`、`pnpm verify:governance`；验证：登记后运行对应`pnpm verify:task -- <实际ID>`/`pnpm verify:scenario -- <实际ID>`，IPC/架构/场景覆盖均通过。
- [ ] 6.3 复核PRD决策、专业来源与参数证据分级及产物，更新验收证据及0.2.0发布文档；验证：所有P0任务已完成且`openspec validate introduce-audio-effects-equalizer --strict`通过，再同步delta并执行`openspec validate --specs --strict`，归档本change后才创建PR，不将规划状态等同功能交付。
