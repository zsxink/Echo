# Design

## Context

见 `proposal.md` 的 Why。当前实现的事实基础（已逐处核对）：

- `crates/echo-core/src/infrastructure/metadata/tags/mod.rs` 用 `lofty = 0.22.2` 读标签，`ParsedMetadata` 只有 title/artist/album/...、`AudioParameters`、封面、歌词与告警；`Song` 上的 `audio_parameters` 是「扫描派生、随歌曲行提交」的既有先例（`0004_song_audio_parameters.sql`）。
- lofty 0.22.2 的 `ItemKey` 已把 `REPLAYGAIN_{TRACK,ALBUM}_{GAIN,PEAK}` 映射到 APE / ID3v2 / VorbisComments / MP4 iTunes freeform；`R128_TRACK_GAIN` / `R128_ALBUM_GAIN` **没有**映射，只能作为未识别键读取；`iTunNORM` 与 OpusHead output gain 无序/未公开换算语义（lofty 把 output gain 读成 `_output_gain` 后丢弃）。
- 桌面音效链在 `crates/echo-desktop/src/player/native_effects.rs`：`aformat → @echo_preamp(volume) → 10×equalizer 或 extrastereo → alimiter(-1 dBFS, level=false)`；`preamp()` 通过 `af-command` 改 `volume`，`TRANSITION = 30 ms` 的斜坡驱动 `advance()`。该链**只在音效启用时安装**；旁路会 ramp 到中性并 `remove()` 整条链。
- `crates/echo-desktop/src/player/actor_effects.rs` 的 `Engine::tick` 以 `(target.revision, playback_epoch)` 缓存「已应用」，`begin_load` 会递增 epoch 并在启用时先暂停，等待配置确认才放行首个样本。`Backend::apply_effects(payload, environment, analysis)` 是唯一的原生入口。
- `crates/echo-desktop/src/effects/model.rs` 的 `Payload` 只有 `Eq` / `Spatial`，P0 规格要求「同一时刻只启用一个预设」；归档 PRD 明确「音量均衡为 P1 独立类型，不与 EQ 隐式叠加，目标响度/峰值上限/已有标签/无标签策略由后续 design 明确」——本 design 即承担这次明确。
- `EffectsDocument` 是严格版本化、`deny_unknown_fields` 的本机偏好；`crates/echo-desktop/src/ipc/generate.rs` 是集中式 TypeScript 生成物来源。
- `song_payload` JSON 与 `sync_projection` 均显式列出 songs 列；`SONG_SELECT` 也是显式列清单，`0025_ignore_derived_song_audio_conflicts.sql` 对派生的音频参数做冲突收敛。

## Goals / Non-Goals

**Goals:**

- 在 Core 以纯数据形式得到逐曲响度标签，桌面以纯函数算出逐曲增益，播放器只负责「把已算好的 dB 值挂到既有 volume 节点」。
- 与 P0 音效显式叠加且互不破坏：归一化关闭时 P0 行为逐位不变；开启时 EQ 保护预算、limiter、状态机、快照语义仍成立。
- 增益随切歌平滑变化，无硬切；不新增滤镜种类、不新增第三方依赖、不改写文件、不扩大同步协议。
- 把 PRD 留给「后续 design」的四项策略（目标响度、峰值上限、已有标签、无标签曲目）一次性定死，并写入可验证规格。

**Non-Goals:**

- 解码音频做 BS.1770 / EBU R128 积分响度测量、true-peak 测量、动态压缩或限幅算法替换。
- 新增 `Payload` 变体、改 `AfCommandSyntax`、按曲目重建 `af` 图、或提前实施 #107 的「单一共享 DSP 模块」。
- 解析 `iTunNORM` / OpusHead output gain、写回标签、编辑标签、覆盖层语义调整。
- 临时播放项归一化、混响、设备绑定、AutoEq（#108）、P0 预设数值（#109）、暂停/seek 淡变（#102）。
- 改动 LAN 同步载荷、portable records、outbox 或 `docs/traceability.md` / `scripts/verify/manifest.json`。

## 分层、依赖方向与设计模式

| 层 | 本 change 的落点 | 依赖方向 |
|---|---|---|
| Domain（Core） | `LoudnessTags` 值对象（来源、track/album gain、peak），挂在 `ParsedMetadata` / `Song`；纯值，无 serde/IPC 类型 | 不依赖任何外层 |

实现口径补充：`LoudnessTags` 用定点整数（gain 为 milli-dB、peak 为百万分之一满幅）而非 `f64`，因为 `ParsedMetadata` 与 `Song` 都派生 `Eq`；`f64` 字段会破坏实体相等性与投影幂等性。dB/振幅换算由纯模块的 `LoudnessTags::gain_db` / `peak` 在使用点完成，不改变任何对外行为。
| Application（Core） | `MetadataReader` port 不变（返回结构扩展）；`GetSongDetail` DTO 增加只读投影 | 依赖 Domain + Port |
| Infrastructure（Core） | `LoftyMetadataReader` 解析；`0031` 迁移 + `statements.rs` / `conversion.rs` / `query.rs::SONG_SELECT` 读写新列 | 通过 Port 接入，不反向定义规则 |
| Application / Platform（desktop） | 新增 `loudness` 纯规则模块；`LoudnessLookup` port；`EffectsService` 侧的独立偏好文档与用例；集中 IPC DTO | 依赖 Core 的 DTO，不依赖 UI；播放器通过聚焦端口注入 |
| Presentation | 音效浮层独立区域；只经 Tauri command/event 访问 | 不接触 SQLite、文件路径或滤镜字符串 |

复用既有模式：**Ports and Adapters**（`LoudnessLookup` 只暴露「按 SongId 取参数」，测试用 fake 实现）、**Repository**（新列只经 `SongRepository` 投影读写，SQL 不越层）、**State Machine**（沿用 `Engine` 的 revision/epoch 确认机制，不新增并行的「归一化已生效」状态机）、**Command / Event**（偏好与状态经既有类型化 IPC）。新增的 `LoudnessLookup` 是唯一新增跨层抽象，理由：播放器 actor 需要按 `SongId` 读取扫描派生事实，而 `SongResolver` 已经承担「UUID → 绝对路径」；把两类能力塞进同一个闭包会造成「播放路径解析」与「元数据查询」职责混杂，且现有 `SongResolver` 在 tests/fake 中被大量复用。

`echo-core` 不新增滤镜、效果会话、偏好或播放器依赖（`docs/DESIGN.md` 铁律）；`echo-desktop` 不反向把 mpv/lavfi 字符串交给 UI。

## Decisions

### D1. 归一化是独立增益层，不是 `Payload` 变体

**决定**：`Payload` 保持 `Eq` / `Spatial` 两变体不变。归一化作为「逐曲、由独立开关控制、显式叠加」的增益层，进入 `Backend::apply_effects` 的附加输入，而不是第三个预设类型。

**理由**：归档 PRD 说「P1 独立类型，不与 EQ 隐式叠加」，P0 规格说「同一时刻只启用一个预设，EQ 与空间不得叠加」。若做成第三个 `Payload`，归一化会与 EQ/空间互斥（开 EQ 就不能统一音量，违背用户目标），且每次切歌都要 `af add/remove` 重建图（放大 #107 指出的脆弱性）。做成独立层则把「不隐式叠加」实现为「用户显式开启 + 界面明确说明 + 预算合并」，既满足 PRD 意图也满足可验证性。

**备选**：(a) 第三个 `Payload` 变体 —— 拒绝，互斥且触发图重建；(b) 直接改 mpv 全局 `volume` 属性 —— 拒绝，与用户音量/静音/会话恢复共用同一状态，无法精确表达 dB 且会污染播放快照；(c) 每曲解码做响度测量 —— 拒绝，超出「读取已有标签」范围且无法宣称 true-peak。

### D2. 两类数据的所有权：标签进 Core 投影，偏好进本机状态

**决定**：逐曲 gain/peak/来源是**扫描派生的资料库元数据**，与 `AudioParameters` 同构，落在 `songs` 表（新迁移），经 `SongRepository` 与 `GetSongDetail` 只读暴露；开关/模式/preamp 是**本机偏好**，放在 `DesktopStateStore` 的新独立版本化槽位。

**理由**：`audio-effects` 规格的「仅存本机偏好、不进 SQLite」约束针对的是用户可编辑的音色偏好，不是媒体元数据。混在一处会要么把逐曲数据塞进桌面偏好（几万首曲目的偏好文件，且资料库切换即失效），要么把开关写进 SQLite（同步载荷语义被破坏）。

**兼容**：不扩展 `statements.rs` 的 `song_payload` JSON 字段，也不扩展 `sync_projection` / `sync_song_aliases` 的显式列清单；新列是纯本机派生事实，各设备扫描同一媒体字节自行得出，因此 LAN 载荷与冲突裁决形状不变，`0025` 的派生字段收敛规则无需为新列改写。`SONG_SELECT` 与 `conversion.rs::song_from_row` 需要追加列并更新索引断言（这是内部投影读取，不是载荷）。

### D3. 标签解析策略与优先级

**决定**：
- ReplayGain 2.0：用 lofty 已映射的四个 `ItemKey`，从所有 tag 中取第一组有效值；gain 文本按 `+/-N.NN dB` 解析为 f64，peak 按振幅 `0..1` 解析。
- R128：lofty 无 `ItemKey`，改为读取未识别键 `R128_TRACK_GAIN` / `R128_ALBUM_GAIN`；整数为 Q7.8 定点，**除以 256 得 dB**；该类标签无 peak。
- 优先级：ReplayGain 2.0 > R128；同一来源下按用户 track/album 模式取值，缺失时回退到另一种模式，仍缺失则「无标签」。
- `iTunNORM`、OpusHead output gain：不解析，按无标签处理并给出诊断。理由：前者是 Apple 私有编码、没有可验证公开换算；后者被 lofty 丢弃且需解析容器头，超出「零新增依赖 + 标签读取」范围。MP4/iTunes 的验收通过 lofty 已映射的 `----:com.apple.iTunes:replaygain_*` freeform 键满足。
- 非法/超范围值进 `ParseWarning`，绝不阻断歌曲记录；沿用 `InputLimits` 的 4 KiB 字段上限。

**备选**：(a) 升级 lofty —— 拒绝，0.22.2 已能读到原始键，升级会带来全仓标签测试回归；(b) 自写容器解析 —— 拒绝，破坏「零新增依赖」且把基础设施规则扩散到播放器层；(c) 猜 iTunNORM 公式 —— 拒绝，属于「无依据的增益」，违反诚实口径。

### D4. 增益公式与保护预算（纯函数，常量集中定义）

**决定**：`crates/echo-desktop/src/effects/loudness/` 新增纯模块（沿用 `math.rs` 的范式），输入为 `LoudnessTags`、偏好（enabled/mode/preamp_db）与 `EffectsAnalysis`，输出 `NormalizationGain`：

```
source_offset_db = match source { ReplayGain2 => 0.0, R128 => +5.0 }   // -23 LUFS → 统一参考 -18 LUFS
raw_gain_db      = selected_tag_gain_db + source_offset_db + preamp_db
peak_limit_db    = peak.map(|p| -20*log10(max(p, EPS)) - 1.0)
                       .unwrap_or(MAX_BOOST_DB)                        // MAX_BOOST_DB = +6.0
gain_db          = clamp(raw_gain_db, MIN_GAIN_DB, min(peak_limit_db, MAX_BOOST_DB))  // MIN_GAIN_DB = -24.0
combined_preamp  = clamp(analysis.effective_preamp_db + gain_db,
                         MIN_COMBINED_DB, MAX_COMBINED_DB)   // -60.0 .. +6.0 dB
```

- `preamp_db` 合法域 `-12..+12`、步进 0.5 dB，非法值整次拒绝（沿用 `EqCurve::validate` 的口径）。
- 无标签 / 开关关闭 / 查询失败 → `gain_db = 0.0`，`combined_preamp = analysis.effective_preamp_db`（P0 行为逐位不变）。
- 所有输出必须有限；`-18 LUFS` 只用于把两类标签对齐到同一参考，**不是**对解码节目的测量结论。
- 合并采用「相加」而非「覆盖」：不允许把归一化值写成 EQ 的前置增益，也不把 limiter 的 -1 dBFS 阈值当成稳态增益的硬上限（已知 peak 由钳制负责、无 peak 由 +6 dB 上限与 limiter 负责）。末端 `alimiter` 保持末位且 `level=false`，不宣称 true-peak 合规。
- 音效旁路但归一化启用时，`analysis` 取中性 EQ 曲线的分析结果（中性曲线不走 32768 点峰值扫描），因此不会引入每帧扫频开销。

**理由**：Issue #101 的 `gain = 10^((gain+preamp)/20)` 只适用于 ReplayGain dB 值；R128 是定点值，直接套用会产生 256 倍错误。把「来源换算」「钳制」「预算合并」放在纯模块，才能满足「算法留在纯应用层、引擎只挂图」以及可单测。

**备选**：(a) 按标签各自参考不做对齐 —— 拒绝，RG2 与 R128 曲目会相差约 5 dB，达不到「统一」；(b) 不给无 peak 标签提升 —— 拒绝，R128 大多数曲目会完全失去归一化；(c) 依赖 limiter 兜住任意提升 —— 拒绝，limiter 是最后保护，不能当常规增益预算。

### D5. 增益阶段的生命周期与斜坡

**决定**：`Engine` 持有 `normalization_gain_db: f64`（每次 `LoadLibrarySong*` 前由 `LoudnessLookup` 更新；临时播放项固定为 0）；`Backend::apply_effects` 增加该标量参数。`requested()` / `begin_load` 屏障的判定改为 `effects_enabled || normalization_enabled`：`tick` 在「音效未启用但归一化启用」时不得走 `bypass_effects()`，而是保留一个 `enabled=false` 的中性 mailbox target 并调用「仅增益」的配置路径；归一化关闭且音效关闭时才沿用现有旁路。这样首个可听样本仍由暂停屏障守住。

- 音效启用：目标 `Configuration.preamp = combined_preamp`，其余不变。
- 音效旁路但归一化启用：安装**最小链** `aformat → @echo_preamp(volume) → alimiter`（不建 10 个 equalizer / extrastereo 节点），`gains` 全零；`Payload` 取中性 EQ 仅作内部配置表达，**不**向 UI 报告任何预设已生效。
- 两者都关闭：沿用现有 `start_bypass` + `remove()`，不遗留节点。仅归一化启用时，音效运行时状态 MUST 保持旁路/未应用，不把最小链伪装成已生效预设。
- `NativeEffects::apply` 的 `changed` 判定必须把 `preamp` 纳入比较（当前只比较 payload/environment）；这样切歌导致的增益变化会进入 30 ms 斜坡而不是被缓存成「无变化」。斜坡期间继续沿用「先收紧衰减、后放宽」的保守界，避免过渡态越过预算。
- 不新增滤镜种类、不改 `AfCommandSyntax`；图只在「启用/关闭状态」变化时增删，**不按曲目重建**。

**理由**：当前 `preamp()` 是瞬时 `af-command`，切歌硬跳会爆音（#102 已指出）；且音效关闭时根本不存在 volume 节点，Issue #101 的「复用 preamp 节点」必须补上「谁负责在旁路时安装它」。

**备选**：(a) 常驻整条 EQ 链（即使关闭）—— 拒绝，改变 P0 旁路语义与 CPU 基线；(b) 每曲 `af add/remove` —— 拒绝，正是 #107 要消除的竞态；(c) 新增独立 `af` gain 节点 —— 拒绝，违反「不引入新滤镜节点」且与既有 ramp 重复。

### D6. 逐曲参数的传递与临时播放边界

**决定**：新增 `LoudnessLookup = Arc<dyn Fn(SongId) -> Result<Option<LoudnessTags>, Error> + Send + Sync>`，在 `runtime/player` 装配层复用 `ScanDeps.songs` 构造，与 `SongResolver` 并列注入 `PlayerActor`；actor 在 `LoadLibrarySong` / `LoadLibrarySongPaused` 分支更新 `Engine` 的当前增益。`PlayerCommand` 不携带增益值（避免 UI 参与音频规则），失败/未知一律降级 0 dB。

资料库外临时播放项不查库、固定 0 dB、按无标签显示；这与 `safe-file-ingestion` 的「临时项不落库」一致，后续 change 若纳入，需要新增「在平台边界现场解析标签」的端口，本 change 不预先开洞。

**备选**：(a) 扩展 `SongResolver` 返回 `{path, loudness}` —— 拒绝，混合两类职责并牵动所有 fake/test；(b) 让 UI 把标签塞进 `LoadLibrarySong` —— 拒绝，音频规则下放到表现层且 10 Hz 快照会被污染；(c) 播放时重新读文件标签 —— 拒绝，actor 线程不应做阻塞 IO（`CODE_STANDARDS` §4.3）。

### D7. 偏好、IPC 与界面

**决定**：新增独立的 `NormalizationDocument { schema_version, enabled, mode, preamp_db }`，默认 `{1, false, Track, 0.0}`，复用 `DesktopStateStore` 的原子读改写与逐字段容错；**不**改 `EffectsDocument`（避免 `SCHEMA_VERSION` 提升波及 P0 曲线、草稿与快照契约）。IPC 新增 `NormalizationSnapshotDto`（偏好 + 当前曲目来源/生效增益/降级原因）与两个 command（读取、设置），在 `ipc/generate.rs` 注册并由 `pnpm generate:ipc` 重新生成 `ipc-types.generated.ts`。

UI 在 `EffectsPanel` 增加独立区域（不是第三个预设标签）：开关、track/album、preamp 与当前曲目状态；所有窗口经同一 service 状态渲染。文案只说「读取文件内已有的响度标签」「sample-peak 保护」，不出现「提升音质 / 动态归一化 / true-peak 合规」。

### D8. 默认产品口径（本 change 采用的取值）

- 默认**关闭**：P0 已交付的听感与响度行为不因升级改变，旧偏好和缺失槽位一律关闭。
- 默认 `Track` 模式、`preamp_db = 0.0`、按 peak 钳制始终开启（不提供单独关闭项）。
- 统一参考 `-18 LUFS`，R128 来源对齐 `+5 dB`；无标签、iTunNORM、OpusHead-only 一律 0 dB 并明确提示。
- 归一化增益边界 `-24..+6 dB`、preamp `-12..+12 dB`（0.5 dB 步进）。

这些取值同时写进 `specs/loudness-normalization/spec.md`，如果产品要在 apply 前调整，应通过 `$openspec-update-change` 同时修订规格、design 与 tasks，不能只改代码。

### D9. 规格、文档与治理同步

**决定**：`audio-effects` 的两条 Requirement 用 `## MODIFIED Requirements` 全文重写（不是新增局部说明），避免归档时丢失 P0 细节；新 capability 用 `## ADDED Requirements` 并带 `## Purpose`。`audio-effects` 与 `loudness-normalization` 都不在 `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX` 注册表中，因此不产生新的场景 ID，不改 `docs/traceability.md` 与 `scripts/verify/manifest.json`；仍须在归档后运行三方对账确认计数不变。

实施阶段同步 `docs/PRODUCT.md`（P1 条目改为已交付能力及诚实口径）、`docs/ROADMAP.md`（P1 排期状态）、`docs/DESIGN.md`（§桌面音效分层补一句「归一化是独立增益层，标签元数据与本机偏好分离」）、`docs/interface-terminology.md`（新增「音量归一化」术语），保持文档、规格、实现一致。

### D10. 验证策略

- 纯规则：单测覆盖来源换算（含 Q7.8）、track/album、peak 钳制、无 peak 上限、预算合并、非法输入、确定性、关闭时 P0 逐位不变。
- Core：标签 fixtures（沿用 `crates/echo-core/src/infrastructure/metadata/tags/tests.rs` 的构造方式）、`0031` 升级/旧行/重扫/同步载荷不变测试、`SONG_SELECT` 列索引断言。
- 桌面：`native_effects_tests.rs` 增加「仅归一化」「归一化+EQ」「切歌斜坡」「关闭回落」用例；`actor_effects_tests.rs` 增加 epoch/降级用例；`FakePlayer` 与 test backend 提供 `LoudnessLookup` fake。
- UI：vitest 覆盖默认关闭、开启/调整、损坏偏好回落、无标签/钳制提示。
- 原生证据：**本 change 只做 macOS**（用户范围决策）。沿用 `scripts/audio-effects/` 工具并新增 `normalization_probe.py` / `test_normalization_probe.py`，用打包的 macOS libmpv 做持续 PCM 捕获：增益前后输出、最小链结构、limiter 压力峰值；全新 fixture 与全新进程的独立重跑 + 数值重算记录在 `evidence/macos-independent-review-*.md`。Windows / Linux 真机验证延后，`native-gate.md` 对这两个平台保持发布阻断；命令返回值与 UI 回执不作为证据。

## Risks / Trade-offs

- [R128 无 peak，提升可能触发 limiter] → 统一 +6 dB 提升上限 + 末端 sample-peak limiter + 文案明确不宣称 true-peak；压力素材捕获验证不超过 0 dBFS。
- [来源对齐常量（+5 dB / -18 LUFS）是约定而非测量] → 在规格、界面与发布说明中明确口径；不写「合规」「测得」，只写「标签对齐」。
- [音效旁路时新增最小链会引入一次 `af add/remove`] → 只在开关切换时发生、不随曲目发生；沿用 `AUDIO_RECONFIG` 确认与暂停屏障；#107 的共享 DSP 重构是后续收敛点，不在本 change 提前实施。
- [切歌增益斜坡与 30 ms 过渡在极短曲目/快速切歌时可能未完成] → `begin_load` 的暂停屏障保证首个样本使用目标值；斜坡只负责「旧→新」的连续性，未完成即被新请求取代时按最新意图收敛。
- [`SONG_SELECT` 追加列会牵动 `conversion.rs` 索引与同步 durability 分页] → 追加在列尾、集中更新索引断言，并在任务组中运行 `--all-features` 的 Core/桌面测试与同步基线测试。
- [偏好新增独立槽位可能与其他本机偏好写者竞争] → 复用 `DesktopStateStore` 的锁内 read-modify-write 与 durable replace，不新增写者；损坏时逐字段回落。
- [iTunNORM / OpusHead 被排除，部分 iTunes/Opus 曲目仍不统一] → 明确「不支持的响度标签」诊断与「无标签不归一化」文案，避免用户误以为功能失效；后续 change 单独立项。
- [与 #107/#108/#109/#102 范围相邻] → 本 change 不引入 `Payload` 变体、不改预设数值、不做 seek/暂停淡变；设计上预留「增益层」边界，避免后续重构返工。

## Migration Plan

1. **SQLite**：新增 `0031_song_loudness_tags.sql`（编号以实施时的最新迁移为准），向 `songs` 追加可空列 `loudness_source TEXT`、`loudness_track_gain_mdb INTEGER`、`loudness_album_gain_mdb INTEGER`、`loudness_track_peak_ppm INTEGER`、`loudness_album_peak_ppm INTEGER`（定点整数，见上方「实现口径补充」）；在 `connection.rs::migrations` 注册。不改写已发布迁移，不建索引（按主键查询）。`SONG_SELECT` 追加列尾并同步 `conversion.rs` / `statements.rs` 的 upsert 与测试；`song_payload` JSON、`sync_projection`、`sync_song_aliases` 保持原字段清单不变。
2. **升级兼容**：旧行全为 `NULL` → 读模型返回「无标签」→ 归一化增益 0 dB；不触发全库重扫、不改 UUID/路径/收藏/歌单/统计。迁移失败时按既有失败恢复语义保留旧库并报告，不降级为「无迁移继续写」。
3. **本机偏好**：旧 `desktop-state.json` 缺少新槽位 → 独立文档取默认值（关闭），其他字段按既有逐字段容错保留；写入沿用 temp → fsync → rename → 目录 fsync。
4. **回滚**：功能回滚只需把开关置为关闭（增益回 0 dB、链恢复 P0 形态）；列是可空加列，保留不影响旧版读写？**否**——旧版二进制不认识新列但 `INSERT` 显式列名，仍可正常读写；若需彻底回退，按既有迁移策略不做 down migration，只保留可空列。发布回滚不要求删列。
5. **验收顺序**：先 Core 解析与迁移测试 → 纯规则单测 → 播放链与 P0 回归 → IPC/UI → macOS 原生证据与独立复核（Windows / Linux 延后） → 规格/文档同步 → `openspec validate --strict` / `pnpm verify:governance` → delta 同步与归档 → 关联 Issue #101 的 PR（`AGENT.md`：未归档不得提 PR）。
