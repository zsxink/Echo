# Design

## Context

动机与范围见 proposal；行为契约见三项 audio capability 及 desktop-app-shell delta。2026-10-02 用户确认：完整 P0，按 0.2.0 规划，P1 独立立项；不以 P1 未完成阻塞 P0。发布日期不在本规划承诺。

当前事实（CodeGraph 与仓库核对）：`player/port.rs` 提供 PlayerCommand/PlayerSnapshot/PlayerPort；`player/actor.rs` 的 BackendProperty 只包含 Seek/Volume/Mute/Pause，没有音效控制。队列由 coordinator 协调，`runtime/player/mod.rs` 装配播放与快照；React playerStore 消费 `player://snapshot`。`platform/local_state.rs` 的 DesktopStateStore 支持 app-private `desktop-state.json` 原子读改写和有效/未知字段保留，尚无音效字段。源音轨 DTO 中的采样率/声道不是滤镜实际处理环境。

基准为 `docs/DESIGN.md` 的 Core/播放隔离、`docs/PRODUCT.md` 的本地优先、`docs/ROADMAP.md` 的一期后功能及 `docs/interface-terminology.md`/原型的布局。原型连线响应和 localStorage 不是本次 DSP 或持久化实现。

## Goals / Non-Goals

**Goals:** 将音效状态机、数值模型、基础设施和 UI 分开；以可复现计算和实际包验证交付 P0；请求、落盘与应用回执分别表达成功边界。

**Non-Goals:** 不改 Core 业务 API、SQLite 或同步协议；不创建自研 PCM 播放引擎，不实施任意滤镜文本执行、P1 解析器或预设覆盖写入；三平台音频实验单独记录证据，不以软件测试替代。

## Decisions

### 1. 分层、端口与模式

依赖为 React 展示 → 集中 Tauri IPC → desktop 音效用例/纯规则 → 注入的 EffectsBackend 与 EffectsPreferencePort；具体 MpvBackend、DesktopStateStore 实现端口。音效纯规则留在 `echo-desktop` 的独立 effects 模块，不进入 echo-core。沿现有 PlayerPort/actor 加能力，不把命名校验、存储与 DSP 拼入 UI/coordinator；新增端口每个最多 6 方法，生产与故障 fake 提供真实可测边界，runtime 是唯一装配位置。

采用 State Machine 处理选择/草稿/确认和恢复，Command/Event 区分请求与事实，Adapter 隔离具名滤镜及本机文件，类型化 enum 隔离 EQ/Spatial。不引入万能 manager、隐藏单例或没有变化点的包装层。现有 FFI 隔离继续使用，若必须新增 unsafe 则单独记录生命周期与安全前提。CODE_STANDARDS §2.1、§3.1–3.3、§4、§6、§8–9 适用；文件规模和端口门禁不得新增豁免。

### 2. 类型、状态机与请求排序

```text
PresetId = builtin:<stable-id> | user:<UUID>
Payload = EqCurve { gainsDb[10], preampMode, requestedPreampDb }
        | Spatial { width:1.25, mix:1 }
EffectsDocument { schemaVersion:1, registryVersion:1, userPresets[],
  selection:None|PresetId|Draft, retainedPayload, draft?, requestedEnabled }
EffectsRuntime { revision:u64, playbackEpoch:u64, persistenceStatus,
  applied:Bypassed|Pending|Applied|Failed|Unavailable,
  effectivePreampDb?, activeBands[], processingRate?, channelLayout?, reason? }
```

P0 只接受上述载荷，非有限数、非法步进/范围/频段数整次拒绝；未来 Loudness/Imported 不作为已实现变体或可操作入口。retainedPayload 专门表达关闭已保存项后保留参数而没有可重启目标的状态；恢复时不得把它误当草稿。草稿为独立快照，与来源 ID 无所有权关系。requestedPreampDb 始终保存合法手动请求，Auto 时忽略该请求而不清除；切回 Manual 恢复请求值，首次默认为 0。重置同时恢复请求值 0。

| 操作 | 选择及参数 | 请求/实际状态 |
|---|---|---|
| 选可用预设 | 整体替换载荷、清草稿 | 请求启用，待当前环境确认 |
| EQ/preamp 编辑 | 新建或更新独立草稿、清选择；空间改为纯 EQ | 请求启用，合并中间更新 |
| 关已保存项 | 选择 None，载荷保留，无草稿 | 请求关闭，开关禁用 |
| 关草稿 | 草稿保留 | 请求关闭，开关可重新启用 |
| 保存草稿 | 原子新增后选择新 ID、清草稿 | 保持启用/关闭，不重置曲线 |
| 重置 | 清选择/草稿，全零/Auto | 请求关闭、最终旁路，实际 0 dB |
| 删除当前项 | 原子管理流程，成功后清选择 | 旁路成功后报告删除 |

actor 是唯一后端命令写入者。有效操作递增 revision，换曲/链重建递增 playbackEpoch；应用确认携两者，旧环境/旧 revision 不发布也不得重新写入。用户操作在 actor 下一轮即处理，不能再加 debounce；连续拖动由 mailbox 的 latest-intent 覆盖中间值，尾值必达。关闭/重置取消未发送编辑并优先执行，保护更新也有优先级。每个后端步骤前检查最新 revision，过期部分应用立即由最新目标收敛，不能只屏蔽 UI 回执。应用失败仅回滚同 epoch 上一次确认配置且回滚仍服从最新目标；无法确认则旁路并标记失败。关闭/重置的安全请求不得被故障回滚重新开启。

对外快照另携 `snapshotSequence`：服务在串行锁内捕获快照时分配单调序号，涵盖用户列表、选择、持久化及原生确认的投影，防止相同 revision/epoch 的旧事件或查询覆盖刚完成的 CRUD。该序号不落盘，不驱动 DSP，不替代音频 revision/epoch；IPC 为可选加法，缺字段的旧快照沿用原有音频排序规则。受保护恢复状态通过独立可选 `recoveryReason` 投影，不被默认播放回执覆盖；仅显式修复成功后清除，UI 显示通用恢复提示而不直接展示可能含本机路径的原因。

暂停/无音轨可落盘，但实际显示 Pending；播放加载由已有链路负责，音效不调用 play/loadfile。恢复/继续播放先配置并确认，再放行首个样本；缺少实际采样率时不能盲目标 Applied。观察实际协商的处理采样率、声道布局与输出重建，不能以源元数据替代；具体观察/capture 路径由首项原生实验验证并记录。

### 3. 固定 EQ 与响应算法

选择十个 PRD 中心频率、二阶 RBJ peaking、**Q=√2**。这是固定 Q 的工程选择：RaneNote 170 的带通关系给出一倍频程对应 Q≈1.414；RBJ peaking 的带宽定义与带通 -3 dB 定义不同，数字 BLT 还存在带宽压缩。故不得把固定 Q 宣传成数字域每段严格一倍频程带宽或 ISO/IEC 合规（来源边界见下表）。频段有效条件为 `fc <= 0.45*Fs`；超限系数为 identity，UI 禁用但持久值保留。EQ 支持 mono/stereo，其他布局标不可用而不自行下混。实际 Fs 未知采用明确标记的 48 kHz 参考图，不能作为实际应用证明。

系数依据 [W3C Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/)，计算如下，全部系数除以 a0：

```text
A=10^(g/40), w=2πfc/Fs, alpha=sin(w)/(2√2)
b0=1+alpha*A, b1=-2cos(w), b2=1-alpha*A
a0=1+alpha/A, a1=-2cos(w), a2=1-alpha/A
H_i(z)=(b0+b1*z^-1+b2*z^-2)/(1+a1*z^-1+a2*z^-2)
responseDb(f)=effectivePreampDb+20log10(abs(product(H_i(exp(j*2πf/Fs)))))
```

使用 f64、非反向 IIR、固定系数规则；候选 FFmpeg `equalizer` 指定 `t=q,w=√2,mix=1,normalize=false,precision=f64,block_size=0`，通过具名 lavfi 的 gain 命令改参。避免六个 biquad 系数逐条提交导致不一致。响应图复用后端计算结果，不在 UI 复制保护规则；在 20 Hz 到 min(20 kHz,0.45Fs) 的对数频点绘制合成 EQ 与实际 preamp，不包含空间/动态 limiter。共享数学测试误差≤0.1 dB，原生低电平扫频/脉冲响应与计算最大误差≤0.25 dB；保护不会触发的低电平样本用于此项。

九个初始曲线（2026-10-02 专业资料复核后的试听候选；顺序为 31.25/62.5/125/250/500/1000/2000/4000/8000/16000 Hz，单位 dB；均 Auto/请求 0）：

| ID | gainsDb |
|---|---|
| pop | 1, 2, 1, -1, -0.5, 0, 1, 2, 1, 0 |
| rock | 3, 4, 2, 0, -1.5, -2, -0.5, 2, 3, 2 |
| classical | 0, 0, 0, 0, 0, 0, 0, 0.5, 0.5, 0.5 |
| jazz | 1, 1.5, 2, 1.5, 0.5, 0, 0.5, 1, 1.5, 1 |
| electronic | 2, 3, 2, -1, -1.5, -1, 0, 1.5, 2, 1 |
| vocal | -1.5, -1.5, -1, -0.5, 0, 1, 2, 1, -1, 0 |
| bass | 1.5, 4, 3, 0.5, 0, -0.5, -0.5, -0.5, -1, -1.5 |
| warm | 1, 1.5, 1, 0.5, 0, 0, -0.5, -1, -2, -2.5 |
| retro | 0, 1, 2.5, 3, 2, 0.5, -0.5, -2, -3.5, -5 |

这些精确 dB 数值全部是 Echo 的主观调音候选，专业资料支持滤波器原理、频段知识及校准方法，不直接提供九条通用音乐类型配方。保留 pop/rock/jazz/warm/retro 初值，收敛 bass/electronic 的相邻低频大幅提升，降低 vocal 的存在感增强与高频削减，令 classical 更接近中性；这不是宣称新值已经更好听。

vocal 是针对完整歌曲的音色/存在感调节，iZotope 的独立人声轨教学只作频段参考；静态 8 kHz 衰减同时影响乐器，不提供动态去齿音、人声分离或内容识别。classical 的高频 peaking 仅作局部提亮，不等于 shelf、恢复高频延伸或改善源文件保真度；retro 的高频衰减也不称滤波器滚降。空间 width=1.25 同属待试听工程初值。

T1/T3 与 V3 以等响度 A/B 判定，不达标更新初值及证据，保持九项和稳定 ID，不扩大功能。内置调音修改不得重写已保存用户曲线/草稿快照；后续已发布注册表变更需版本记录，本轮未发布规划修订不改变偏好 schemaVersion/registryVersion。

#### 初值调整的数学检查（非音频实测）

使用本节 f64/RBJ 公式，Fs=48000 Hz，DC 至 Nyquist 共32769个等距频率点，未加 preamp/limiter 时计算串联合成峰值；自动 preamp 依第4节公式计算。数值为近似采样结果，不是瞬态上界、三平台滤镜测量或听感证据，实际环境必须重算，不能固定写死该 preamp。

为复算对比保留本轮替换前的参数快照（同十段顺序）：

```text
classical: 0,0,0,0,0,0,0.5,0.5,1,1.5
electronic: 4,5,3,-1,-2,-1.5,0,2,3,2
vocal: -2,-2,-1.5,-0.5,0,1.5,3,1.5,-1.5,-1
bass: 3,6,5,1,0,-0.5,-1,-1,-1.5,-2
```

| ID | 旧曲线合成峰值 dB | 新曲线合成峰值 dB | 新曲线保守 Auto preamp dB |
|---|---:|---:|---:|
| classical | +1.57 | +0.61 | -0.11 |
| electronic | +6.24 | +3.68 | -3.18 |
| vocal | +3.48 | +2.32 | -1.82 |
| bass | +7.52 | +4.84 | -4.34 |

该列按 `safe+0.5 dB` 重算，区别于此前 `safe=-max(0,G+1 dB)` 公式下的历史数值。降低叠加可减少为预留 headroom 所需的整体衰减，但不保证响度、音质或可辨识性；实测须同时记录限幅介入/增益衰减、低频瞬态和非目标乐器变化。

#### 专业资料与参数证据分级（2026-10-02 复核）

| 来源 | 支持内容 | 不支持的推论 |
|---|---|---|
| [W3C Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/) | RBJ peaking 系数、Q/BW 定义与数字带宽修正 | 不推荐某音乐类型的精确曲线，也不证明随包滤镜与计算一致 |
| [RaneNote 170](https://www.ranecommercial.com/legacy/note170.html) | 带通滤波器 BW/Q 的关系，1 octave对应Q≈1.414 | 不直接证明数字 peaking 所有频段严格一倍频程或ISO/IEC合规 |
| [iZotope 母带 EQ 指南](https://www.izotope.com/community/blog/how-to-eq-your-master) | 按素材克制调节、匹配增益A/B减少响度偏差 | 不提供 Echo 九条预设或通用播放器genre配方 |
| [iZotope 人声 EQ 指南](https://www.izotope.com/community/blog/how-to-eq-vocals) | 存在感约1.5–5kHz、齿音常见5–8kHz，静态削减可能暗淡 | 独立人声轨教学不能等同整曲人声增强或动态去齿音 |
| [iZotope M/S 指南](https://www.izotope.com/community/blog/what-is-midside-processing) | 拓宽的取舍、中央元素及mono兼容性检查 | 不为width=1.25背书，不保证所有歌曲拓宽后更好 |
| [ITU-R BS.1770-5](https://www.itu.int/rec/R-REC-BS.1770/en) | 节目响度及true-peak测量算法 | 不规定此应用的genre曲线、sample limiter时间常数或输出目标 |
| [FabFilter true-peak手册](https://www.fabfilter.com/help/pro-l/using/truepeaklimiting) | sample/true-peak区别，过采样与重建峰值风险 | 该产品的true-peak保证不能移植给FFmpeg alimiter |

RBJ公式属于算法依据；Q/采样率边界、0.5 dB步进、1 dB余量、limiter时间常数、width及误差/性能预算是 Echo 工程选择；九条增益是主观初值；原生输出/试听/三平台结果属于尚待补齐的实测证据。不得混写为“专业标准参数”。

### 4. 空间、整链增益及峰值保护

空间采用 `M=(L+R)/2,S=(L-R)/2,L'=M+1.25S,R'=M-1.25S`，候选 `extrastereo=m=1.25:c=false`，不暴露空间编辑。width 是滤镜安装期参数：随包 FFmpeg 的 `extrastereo` 没有 `process_command`，mpv 的 `af-command spatial:m` 虽返回成功但不改变参数；实现不得发送该命令。线性段单声道折叠保留 M，最坏逐样本放大界为 1.25（约1.938 dB）；限幅可能改变整体幅度，仍须人声折叠试听。

链序：浮点格式协商 → 独立 preamp → 十段 EQ **或**空间 → 不可关闭的末端 limiter → 既有输出链。新增链不写 mpv volume/mute，也不为衰减补偿主音量。关闭平滑过渡后移除全部新增处理，恢复既有旁路；过渡尾部也受保护。

EQ 的风险 `G` 为有效整链最大幅频增益：DC/Nyquist、中心频点及 32769 个等距频点，并细化局部极大值；空间用1.938 dB。峰值保护上限 `safe=-max(0,G)`，中性全零 EQ 特例 safe=0；自动初始策略采用 `safe+0.5 dB`（中性仍为0），由不可关闭的末端 limiter 承接不超过0.5 dB的合成稳态超限并保护真实瞬态；手动采用 `min(request,safe)`。安全值可低于 -12。该0.5 dB补偿是 review 提出的保守调音初值，不是响度匹配，须由候选包 PCM 压力捕获确认sample-peak门槛。过渡期间使用旧/新/经过参数的最保守界；提高风险先收紧前置衰减，降低风险先降效果后放宽衰减。频响估计不能证明任意瞬态安全，末端 limiter 必不可少。

限幅候选固定为 sample-peak，`limit=0.891250938`（-1 dBFS）、attack=5 ms、release=50 ms、level_in/out=1、asc=false、level=false、latency=true；不宣称 true-peak；-1 dBFS 的样本上限不等于 -1 dBTP，无法据此保证 DAC 重建或后置处理的 intersample peak。明确关闭空间内部 clipping 和 limiter 自动补增益，保持浮点中间余量。实际输出捕获全为有限值且 sample peak≤0 dBFS；若后置重采样等使保护失效，属于首项验证失败，不能放宽门槛。该阈值保留样本幅度余量，5/50 ms 是候选实现初值（也恰为FFmpeg该滤镜默认时间值），不是 ITU推荐或已证明最透明的设置；须在瞬态、持续低频和反相素材中审查失真/抽吸感。不承诺修复源失真或用户主音量额外放大。true-peak仅作为测量诊断记录，若日后增加true-peak处理或合规承诺需独立修订方案与预算，不在本轮扩大P0。

30 ms 连续过渡为目标：用户操作在 actor 下一轮立即开始斜坡，不叠加 debounce；稳态 gain/preamp 原地更新，类型切换/开关/采样率变化允许重新配置但不能断流。新滤镜链必须等 libmpv 报告重配置后再发送具名参数命令，避免命令早于滤镜实例就绪。具名命令存在不代表后端能平滑更新；须实际捕获验证斜坡、类型切换和首样本屏障。限幅延迟必须由播放链补偿，验证 seek、EOF 尾部、时长和歌词/进度差相对旁路不新增可测偏移（捕获时间容限 10 ms）。不把每个 input 重建链、静默断流或未评估双链引擎作为默认补救。

### 5. 曲线管理和偏好事务

名称复用 `echo-core` 既有公开文本规范算法（桌面已依赖 Core，不产生反向依赖），NFKC+空白规整+默认 full case-fold，按规整显示名计1–40 grapheme；不用 locale lower-case 替代。不变更歌单规则。内置/用户命名空间分离，检查内置显示名与全部用户名称，额度50。字符串仅文本展示。

偏好新字段 effects 封装 schemaVersion=1/registryVersion=1 与用户项/请求状态；写入经同一个 DesktopStateStore 原子读改写，保留主题、窗口、播放会话和未知字段。编辑音频请求与落盘独立：250 ms debounce 持久化，正常退出 flush，失败状态显式标未保存且保留内存草稿。CRUD 使用串行用例与存储端口，快照校验、ID生成、原子落盘后发布列表，不乐观报告成功。

删除当前项需要跨后端与文件的补偿流程：先确认旁路，再原子删除；写入失败列表/选择仍是原值，尝试恢复旧确认处理并显示失败，恢复无法确认则旁路。旁路失败不落盘删除，不能把非原子过程伪装文件事务。期间其他操作串行排队；提交成功再清选择，revision 和最新用户意图仍优先。重命名只更新元数据，不触碰音频链；删除草稿来源只改列表。

旧文档无 effects：补默认关闭但保留其他字段；未知 schema、非法载荷、悬空选择 ID 时不启用且禁止自动覆盖原字段，独立保留原文档副本/有效项供恢复。仅在显式重试/修复成功后替换。runtime Applied/安全值不落盘，版本相同的文档可在三平台人工迁移，不提供 P0 导入 UI 或自动同步。

### 6. IPC 与界面

在集中 DTO 定义 additions：查询音效快照、选择/编辑/启停/重置、保存/重命名/删除；返回 revision、持久化结果和应用状态，命令接受与 Applied 不是同一成功。沿现有 `player://snapshot` 加可选兼容的音效投影（旧消费者忽略；字段缺失前端默认为关闭）；新操作用单独类型化命令，不改变既有 PlayerCommand 参数。通过 `pnpm --dir apps/desktop generate:ipc` 更新生成物，禁止手改。

React 共用 player store，组件拆为触发入口、预设列表、EQ编辑/图、命名对话框，不直接读文件、调用 mpv 或用 localStorage 作音效真相源。音效与队列沿现有浮层栈互斥；在菜单/排序/队列层加入音效，外部点击/Tab 离开音效保留目标焦点且不陷焦点，模态命名/确认仍优先。共宽度两标签、固定顶部开关、内容内滚动，输入互不重叠，使用主题与语义共同表达选中。正常/沉浸视图共用状态，不创建另一音效会话。选择/保存/错误用礼貌播报，连续拖动合并播报，不逐帧播出。

官方接口依据：[FFmpeg equalizer](https://ffmpeg.org/ffmpeg-filters.html#equalizer)、[extrastereo](https://ffmpeg.org/ffmpeg-filters.html#extrastereo)、[alimiter](https://ffmpeg.org/ffmpeg-filters.html#alimiter) 与 [mpv Audio Filters / af-command](https://mpv.io/manual/stable/)。这些来源只支持候选接口与选项存在，包能力/平滑/输出时序必须由下面Gate证明。

### 7. 实施前 Gate 与验收证据

**状态：尚未完成三平台实测。2026-10-02 用户授权先推进功能实现；三平台 Gate 继续阻塞发布，授权不替代测量证据。** 先以小型原生实验验证 T1/T2/T3/T5/T6，T4仅属P1。每个平台使用实际分发库，锁定包校验和、libmpv/FFmpeg版本、滤镜选项、具名 target/命令、实际处理环境观察、loadfile 存活/重下发、volume/mute独立、限幅延迟与无爆音平滑。记录至少4核/8GB/SSD固定参考机与输出捕获工具；三平台具体机器资产号写入 gate 报告，不能用开发机单平台通过替代。

素材：可离线复现的生成正弦（含997 Hz）、对数扫频、固定种子宽带噪声、脉冲、单侧满幅与双声道反相，峰值设 -0.1 dBFS；响应测量使用 -60 dBFS 素材避免限幅。听感使用有授权的固定人声/低频/打击乐/古典素材，清单记录来源、许可、hash、时长和采样率。

听感对照使用同一片段，按 ITU-R BS.1770-5 测量积分响度，制作离线匹配副本（差≤0.5 LU；低于门限/片段过短时记录替代度量和原因）。至少两名审查者比较中性与相近预设，记录 A/B 意图、差异、失真及空间人声折叠结论；这不向产品新增响度归一化功能。

压力覆盖单段/相邻/全部±12、手动0、空间及切换，拒绝非有限值和新增硬削波。同时记录输出 true-peak（dBTP）、测量工具/版本/算法及限幅介入；它用于诊断 sample-peak 保护边界，不把 -1 dBTP 当作现有P0验收保证。

延迟用带时间标记的参数提交与输出捕获特征检测，不使用 command/UI 回执；开关、EQ→EQ、EQ↔空间、拖动每场景≥100次，p95≤150 ms。稳态/面板切换分开记录 underrun，CPU预热30秒后采样5分钟，以最重P0相对旁路增加≤10个百分点判定。V1–V8、V10–V18全部为P0；V9/V19及T4不纳入完成勾选。Gate 任一失败必须停止、记录并修订 design 和受影响 spec/PRD 后重新评审，不自动缩减 P0。

### 8. 验证命令与追踪

实现过程分别运行 `cargo test -p echo-desktop`、`cargo test --workspace`（含架构）、`cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`；UI `pnpm --dir apps/desktop format:check`、`lint`、`typecheck`、`test`、`build` 和 `test:e2e`；IPC生成及 `pnpm verify:governance`。将新需求/场景/任务登记进仓库现有验证 manifest，再运行 `pnpm verify:task -- <已登记ID>`/`pnpm verify:scenario -- <已登记ID>`，不杜撰尚不存在的 ID 或脚本为已通过。原生测试工具在Gate任务中建成并记录三平台可执行命令与证据，单元 fake 不能替代原生测量。

| 验收 | 主要能力/任务组 |
|---|---|
| V1/V3/V7/V12、T1/T2/T3/T5/T6 | audio-effects，Gate/后端/集成 |
| V2/V5/V6/V17/V18 | audio-equalizer，数值/后端/UI |
| V4/V8 | audio-effects-presets，规则/偏好/UI |
| V10/V13/V14 | audio-effects，播放集成/隔离 |
| V11/V16 | audio-effects及presets，状态/持久化/故障 |
| V15 | desktop-app-shell及equalizer，浮层/无障碍 |

## Risks / Trade-offs

- [API 可用但原地更新不平滑] → 首项输出捕获Gate，失败先修订方案，不宣称官方文档等于包验证。
- [限幅和处理延迟影响听感/时序] → 禁自动补偿增益、明确sample-peak、客观捕获与响度匹配试听；补偿/EOF专项验证。
- [系统输出设备暂不可用] → 请求保留、Pending/Unavailable、不假生效，支持恢复重新确认。
- [偏好损坏或跨后端/文件删除失败] → 原文档保留、原子写与补偿、typed错误与失败注入。
- [三平台验证环境未齐] → 分别补证据并阻塞Gate，不将其写为可延后P0验收。

## Migration Plan

1. 按本轮授权先开发并同步 PRODUCT/ROADMAP/术语，增加桌面内部模型与版本化偏好；老用户默认关闭，无DB迁移。
2. 加法IPC及生成类型与新UI同包交付；可选新字段使旧消费者继续处理既有播放快照。
3. 实施与验收分阶段提交，0.2.0发布前完成全部P0证据。回滚旧应用保留未知effects字段，新版本重装恢复合法请求但不自动发声。
4. 创建PR前按仓库流程同步delta到主规格、校验同步结果再归档；本次规划不执行同步/归档或版本号代码改动。
