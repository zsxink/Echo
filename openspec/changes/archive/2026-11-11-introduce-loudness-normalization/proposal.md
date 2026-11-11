# Proposal

## Why

GitHub Issue #101「本地曲库音量归一化：读取 ReplayGain/R128 标签并接入效果链」指出：不同专辑/来源的本地曲目响度参差，而 Echo 目前完全不读取 ReplayGain / R128 标签。`docs/PRODUCT.md`、`docs/ROADMAP.md` 已把「响度归一化」列为 P1 独立立项，归档的 `introduce-audio-effects-equalizer` PRD 也把目标响度、峰值上限、已有标签及无标签曲目策略留给「后续 design」明确。当前 `lofty = 0.22.2` 已在 workspace 中并原生映射 ReplayGain 键，因此这是零新增依赖的能力缺口，适合作为独立 P1 change 落地并补齐规格决策。

## What Changes

- **Core 侧（跨平台、与播放器无关）**：`MetadataReader` 在扫描/导入时解析 ReplayGain 2.0（track/album gain + peak）与 Opus/Vorbis R128 标签，形成纯值对象 `LoudnessTags` 挂到 `ParsedMetadata` / `Song`；新增一条顺序迁移把逐曲标签写入 SQLite 只读投影，旧行保持空值；只读歌曲详情 DTO 暴露标签来源与数值。不写回音频文件、不新增依赖、不改覆盖层语义。
- **桌面规则层**：新增纯函数模块计算逐曲归一化增益（标签来源对齐、用户 preamp、按 peak 钳制、增益上限、与 EQ 保护预算合并），不带播放器状态、可独立单测。
- **桌面播放层**：通过聚焦端口按 `SongId` 读取逐曲标签，把归一化增益并入既有 `@echo_preamp` 的 `volume` 节点；增益随切歌走既有平滑过渡，音效关闭时仍可工作；不新增滤镜种类、不改 `AfCommandSyntax`、不重建 P0 预设注册表。
- **偏好与界面**：新增独立、版本化的本机偏好（默认关闭、track/album 模式、preamp），在音效浮层提供独立开关与当前曲目状态；旧偏好或非法数据一律回落为关闭且不覆盖原数据。偏好经集中 IPC 生成物下发，UI 不接触 SQLite 或滤镜字符串。
- **规格与项目文档**：新增 capability `loudness-normalization`；修改 `audio-effects` 的 P0 范围条款与本机持久化隔离条款，明确「归一化不是预设、显式叠加」「逐曲标签是资料库元数据而非音效偏好」；实施阶段同步 `docs/PRODUCT.md`、`docs/ROADMAP.md`、`docs/DESIGN.md`、`docs/interface-terminology.md`。
- 无 **BREAKING** 变更：迁移为加列、偏好为新增独立键、P0 音效行为在归一化关闭时保持不变。

非目标：运行时节目响度测量（BS.1770/EBU R128 扫描）、无标签曲目的动态归一化、`iTunNORM` 与 OpusHead output gain 解析、true-peak 合规宣称、写回或编辑音频标签、资料库外临时播放项归一化、混响/设备绑定/AutoEq（#108）、单一共享 DSP 模块重构（#107）、P0 预设数值调整（#109）、暂停/seek 淡变统一（#102）、LAN 同步载荷格式变更。

## Capabilities

### New Capabilities

- `loudness-normalization`: 逐曲标签提取与落库、归一化增益纯规则、增益应用与平滑过渡、偏好与界面、失败降级及诚实验收口径。

### Modified Capabilities

- `audio-effects`: 「统一预设与一期范围」需改为允许 P1 归一化能力在独立开关下与 EQ/空间显式叠加，并移除「无 P1 可操作入口」的旧断言；「音效本机持久化及恢复隔离」需区分本机音效偏好与扫描派生的逐曲资料库元数据。

## Impact

- 事实来源：`docs/PRODUCT.md` §桌面音效（P1 边界）、`docs/ROADMAP.md` §0.2.0（P1 独立 change）、`docs/DESIGN.md` §4 曲库与扫描 / §桌面音效分层与本机恢复（Core 铁律、覆盖层不动文件、投影可重建）、`docs/interface-terminology.md` §音效与均衡器（新增术语）、`openspec/CODE_STANDARDS.md` §3 分层、§4 Rust、§5 SQLite 迁移、§6 类型化 IPC、§8–9 交付门槛。
- Core：`crates/echo-core/src/domain/media.rs`、`domain/entities/song.rs`、`infrastructure/metadata/tags/mod.rs` 及测试、`infrastructure/sqlite/migrations/0031_song_loudness_tags.sql`（编号以实施时最新迁移为准）、`connection.rs`、`statements.rs`、`conversion.rs`、`application/detail.rs`。
- 桌面 Rust：`crates/echo-desktop/src/effects/`（新增纯规则模块）、`player/actor_effects.rs`、`player/actor/backend.rs`、`player/actor/mpv_backend.rs`、`player/native_effects.rs`、`player/port.rs`、`runtime/player/mod.rs`（新增按歌曲查找标签的端口）、`platform/local_state/`（新增版本化偏好槽）、`ipc/`（新增 DTO 与 `generate.rs` 注册）、`effects_service.rs` 及对应测试。
- 桌面壳层与 UI：`apps/desktop/src-tauri/src/main.rs`、新增/扩展的 Tauri command 模块、`apps/desktop/src/features/player/EffectsPanel.tsx` 及其测试、`apps/desktop/src/ipc/ipc-types.generated.ts`（由 `pnpm generate:ipc` 重新生成，不手工编辑）。
- 规格与治理：`openspec/changes/introduce-loudness-normalization/` 的四份产物；归档时同步到 `openspec/specs/loudness-normalization/spec.md` 与 `openspec/specs/audio-effects/spec.md`。两个 capability 均不在 `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX` 注册表中，因此不改动 `docs/traceability.md` 与 `scripts/verify/manifest.json`；仍需 `node scripts/verify/reconcile-scenarios.mjs` 确认三方计数不变。
- 依赖：无新增第三方依赖；复用 lofty 0.22.2、既有 DesktopStateStore 原子读改写、既有 mpv lavfi 图与 limiter。
- 关联但未纳入本 change：#102（增益变化应平滑，本 change 只处理切歌增益）、#107、#108、#109。
- 原生验证范围（本次修订）：只做 macOS 实际分发运行库的持续 PCM 捕获与独立复核；Windows / Linux 真机验证延后，发布 Gate 对这两个平台继续保持阻断，不从 macOS 继承结论。
