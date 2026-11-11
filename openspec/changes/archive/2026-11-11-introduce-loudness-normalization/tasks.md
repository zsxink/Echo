# Tasks

## 1. Core：领域值与标签解析

- [x] 1.1 在 `crates/echo-core/src/domain/media.rs` 定义 `LoudnessTags`（来源枚举、track/album gain dB、track/album peak 振幅），并作为可选字段挂到 `ParsedMetadata`；在 `domain/entities/song.rs` 增加对应字段与只读访问器，保持现有构造/默认值不变；用 `cargo test -p echo-core --all-features domain::` 验证新增与既有实体测试全部通过
- [x] 1.2 在 `infrastructure/metadata/tags/mod.rs` 用 lofty 已映射的四个 ReplayGain `ItemKey` 解析 gain/peak，覆盖 FLAC、MP3、APE、MP4 iTunes freeform、Vorbis Comment；在 `tags/tests.rs` 补固定样本单测，用 `cargo test -p echo-core --all-features tags::` 验证数值、来源与文件字节不变
- [x] 1.3 读取 lofty 未映射的 `R128_TRACK_GAIN` / `R128_ALBUM_GAIN`，按 Q7.8 除以 256 换算 dB 并标记「无 peak」；补 Opus/Vorbis 单测，用 `cargo test -p echo-core --all-features tags::` 验证换算与来源
- [x] 1.4 实现优先级（ReplayGain 2.0 > R128；同来源 track/album 回退）、非法/非有限/越界值告警丢弃、部分缺失保留可用项、iTunNORM/OpusHead-only 判为无标签；补异常路径单测，用 `cargo test -p echo-core --all-features tags::` 验证歌曲仍可入库且产生非致命 `ParseWarning`
- [x] 1.5 在 `application/detail.rs` 的 `SongDetail` 增加只读响度参数 DTO 与来源标识，补序列化/空值断言，用 `cargo test -p echo-core --all-features detail::` 验证旧行返回「无标签」
- [x] 1.6 运行 `cargo fmt --all --check` 与 `cargo clippy -p echo-core --all-targets --all-features -- -D warnings`，确认第 1 组无格式与静态检查告警

## 2. Core：投影迁移与兼容

- [x] 2.1 新增 `crates/echo-core/src/infrastructure/sqlite/migrations/0031_song_loudness_tags.sql`（编号以实施时最新迁移为准，五个可空列），在 `connection.rs::migrations` 按序注册；补迁移顺序与旧库升级测试，用 `cargo test -p echo-core --all-features sqlite::tests::` 验证升级后旧行全为 NULL、UUID/路径/收藏/歌单/统计不变
- [x] 2.2 在 `query.rs::SONG_SELECT` 列尾追加新列，更新 `conversion.rs::song_from_row` 索引、`statements.rs` 的 upsert/快扫语句与对应断言；用 `cargo test -p echo-core --all-features conversion::` 与 `sqlite::tests::songs` 验证读写一致
- [x] 2.3 确认 `song_payload` JSON、`sync_projection`、`sync_song_aliases` 的显式字段清单**不**包含新列，补一条「同步载荷形状不变」断言；用 `cargo test -p echo-core --all-features sqlite::tests::sync_payloads` 与 `sqlite::tests::sync_baseline` 验证无载荷增量、无新增冲突
- [x] 2.4 补「外部改标签后重扫更新、未重扫保持旧值」测试，并在 `docs/DESIGN.md` §4 补一句逐曲响度参数属于扫描派生元数据；用 `cargo test -p echo-core --all-features scan::` 与 `git diff -- docs/DESIGN.md` 验证语义与文档一致

## 3. 桌面：纯规则模块

- [x] 3.1 新增 `crates/echo-desktop/src/effects/loudness/`（类型与常量集中、`compute(tags, preference, analysis)` 纯函数），不引入任何播放器/UI/时间依赖；用 `cargo test -p echo-desktop --all-features effects::loudness` 验证模块可独立编译与调用
- [x] 3.2 实现 -18 LUFS 参考对齐（R128 +5 dB）、preamp 合法域（-12..+12、0.5 dB 步进）、peak 钳制、无 peak +6 dB 上限、-24..+6 dB 增益边界与有限值保证；补参数化单测覆盖正常与非法输入，用 `cargo test -p echo-desktop --all-features effects::loudness` 验证公式可复现、拒绝而非静默钳制
- [x] 3.3 实现与 `EffectsAnalysis` 的合并前置增益（`clamp(effective_preamp + gain, -60 dB, +6 dB)`，相加而不覆盖 EQ 保护衰减）与「关闭/无标签返回 0 dB 且 P0 前置增益逐位不变」；补回归单测，用 `cargo test -p echo-desktop --all-features effects::` 验证 P0 数值无变化
- [x] 3.4 运行 `cargo fmt --all --check` 与 `cargo clippy -p echo-desktop --all-targets --all-features -- -D warnings`，确认第 3 组无告警

## 4. 桌面：播放链、增益阶段与斜坡

- [x] 4.1 定义 `LoudnessLookup` 聚焦端口，在 `runtime/player` 装配层复用 `ScanDeps.songs` 构造，与 `SongResolver` 并列注入 `PlayerActor`；为 fake/test backend 提供实现，用 `cargo test -p echo-desktop --all-features runtime::player` 验证依赖注入与空实现降级为 0 dB
- [x] 4.2 让 `Engine` 持有当前逐曲增益，在 `LoadLibrarySong*` 前经 lookup 更新并使 `completed`/epoch 缓存失效；`begin_load` 的暂停屏障在「仅归一化启用」时同样生效；补 `actor_effects_tests.rs` 用例，用 `cargo test -p echo-desktop --all-features actor_effects` 验证首样本前完成计算、旧环境不假生效
- [x] 4.3 为 `Backend::apply_effects` 增加归一化增益标量参数，更新 `mpv_backend.rs` / `test_backend.rs` / `fake.rs` 与所有调用点；用 `cargo test -p echo-desktop --all-features player::` 验证接口变更未破坏既有命令与快照语义
- [x] 4.4 在 `native_effects.rs` 实现 D5：音效旁路但归一化启用时安装最小链、把 `preamp` 纳入 `changed` 判定、切歌走 30 ms 斜坡、过渡期保守界、两者都关闭时完全旁路；在 `native_effects_tests.rs` 补「仅归一化」「归一化+EQ」「切歌增益变化」「关闭回落」用例，用 `cargo test -p echo-desktop --all-features native_effects` 验证不新增滤镜种类、limiter 仍在末位
- [x] 4.5 实现查询失败、参数损坏、`AfCommandSyntax::Unavailable`、应用失败的 0 dB 安全降级与明确诊断，确认不改变音量/静音/播放计数/队列；补失败路径测试，用 `cargo test -p echo-desktop --all-features --test player_smoke` 验证基础播放不中断、不自动播放
- [x] 4.6 运行 `cargo test -p echo-desktop --all-features` 与 `cargo clippy -p echo-desktop --all-targets --all-features -- -D warnings`，确认第 4 组与既有 P0 音效回归全绿

## 5. 桌面：偏好、IPC 与界面

- [x] 5.1 新增独立版本化 `NormalizationDocument`（默认关闭 / Track / 0 dB），复用 `DesktopStateStore` 原子读改写与逐字段容错；补缺失、损坏、未来版本、非法 preamp 与并发写入测试，用 `cargo test -p echo-desktop --all-features local_state` 验证回落关闭且不覆盖其他偏好
- [x] 5.2 新增偏好的读取/设置用例与 Tauri commands，经唯一 service 状态向所有窗口提供 `NormalizationSnapshotDto`；补 command 层序列化与错误结构测试，用 `cargo test -p echo-app --all-features` 验证命令契约
- [x] 5.3 在 `ipc/` 新增 DTO 与 `typescript()` 片段并注册到 `generate.rs`，运行 `pnpm --dir apps/desktop generate:ipc` 重新生成 `ipc-types.generated.ts`；用 `git diff --exit-code -- apps/desktop/src/ipc/ipc-types.generated.ts` 与 `pnpm --dir apps/desktop typecheck` 验证生成物无漂移
- [x] 5.4 在 `EffectsPanel` 增加独立区域（开关、track/album、preamp、当前曲目来源/生效增益/钳制与无标签提示），不进入预设列表；在 `EffectsPanel.test.tsx` 补默认关闭、开启调整、无标签、钳制、损坏偏好回落用例，用 `pnpm --filter @echo/desktop test -- --run --reporter=verbose` 与 `pnpm --dir apps/desktop lint` 验证
- [x] 5.5 在 `docs/interface-terminology.md` 补「音量归一化」术语与文案边界，运行 `pnpm --dir apps/desktop format:check`，确认界面文案不含 true-peak / 动态归一化 / 提升音质等无依据表述

## 6. 规格、文档同步与归档

- [x] 6.1 同步 `docs/PRODUCT.md`（P1 条目与 sample-peak 口径）、`docs/ROADMAP.md`（P1 排期状态）、`docs/DESIGN.md` §桌面音效分层（独立增益层、标签元数据与本机偏好分离），用 `git diff -- docs/` 与 `pnpm verify:governance` 中的文档/追溯检查验证一致
- [x] 6.2 运行 `pnpm exec openspec validate introduce-loudness-normalization --strict` 与 `node scripts/verify/reconcile-scenarios.mjs`，确认 delta 合法、两个 capability 均不在 `AREA_PREFIX` 中且三方场景计数不变（无需改 `docs/traceability.md` / `scripts/verify/manifest.json`）
- [x] 6.3 在实现与验证全部通过后，使用 `$openspec-sync-specs` 把两份 delta 同步到 `openspec/specs/loudness-normalization/spec.md` 与 `openspec/specs/audio-effects/spec.md`，运行 `pnpm exec openspec validate --archived` 与 `node scripts/verify/reconcile-scenarios.mjs` 验证同步结果
- [x] 6.4 使用 `$openspec-archive-change` 归档本 change，确认任务清单全部勾选、主规格与 change 一致，并在关联 GitHub Issue #101 的 PR 描述中记录归档结果（`AGENT.md`：未归档不得提 PR）

## 7. 原生证据与集成门禁

- [x] 7.1 在 macOS 实际分发运行库上用 `scripts/audio-effects/normalization_probe.py` 做持续 PCM 捕获，记录增益前后输出、最小链结构与压力素材峰值，写入 `native-gate.md` 与 `evidence/`；用全新 fixture 的独立重跑、哈希与数值重算复核，确认报告 `gate_passed=false` 且 Windows / Linux 明确延后、不从 macOS 继承结论。已执行：`python3 -B scripts/audio-effects/normalization_probe.py ...`、`python3 -B -m unittest discover -s scripts/audio-effects -p 'test_*.py' -v`（28 项通过），证据见 `evidence/macos-normalization-2026-11-11.json` 与 `evidence/macos-independent-review-2026-11-11.md`
- [x] 7.2 运行 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`，确认全 workspace 静态检查与测试通过
- [x] 7.3 运行 `pnpm --dir apps/desktop typecheck`、`pnpm --dir apps/desktop lint`、`pnpm --filter @echo/desktop test -- --run` 与 `pnpm verify:governance`，确认 IPC 生成物、分层门禁、规模检查、场景三方对账与治理门禁全绿
- [x] 7.4 独立复核：以全新 fixture / 全新进程重跑 macOS 探针，并从磁盘重算哈希、dB 数、峰值与规格同步状态；76 项检查全部 CONFIRMED、0 REFUTED、0 CHANGED，结论见 `evidence/macos-independent-review-2026-11-11.md`
