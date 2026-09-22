# Proposal: add-song-quality-badges

关联 issue: https://github.com/zsxink/Echo/issues/2

## Why

用户在歌曲列表中无法直观判断文件音质档位,需要逐首打开详情才能确认。在歌曲名后追加 SQ/HQ 徽标,让用户在浏览列表时一眼区分无损与高音质条目。

## What Changes

- 歌曲列表行在歌曲名称后显示音质徽标:**SQ**(黄色)、**HQ**(蓝色),字号比歌曲名小一号;无对应档位的歌曲不显示徽标。
- 新增音质档位判定规则(读时派生,不入库、不迁移):
  - **SQ**(无损/高解析):`AudioFormat::Flac` / `Wav`;或位深 ≥ 24bit;或采样率 ≥ 96kHz
  - **HQ**(高音质):MP3 ≥ 320 kbps;或 Mp4/AAC ≥ 256 kbps;或 Opus/Ogg ≥ 256 kbps
  - 无徽标:其余(如 128kbps MP3)、参数缺失、`UnknownDamaged`
  - 两者同时满足时优先显示 SQ(从高到低:SQ > HQ > 无)
- IPC `SongView` DTO 新增派生字段 `quality?: "sq" | "hq"`,由 `format` + 音频参数(`bitrate_bps / sample_rate_hz / bits_per_sample`)计算。
- 重新生成 `ipc-types.generated.ts`;前端 `SongRow` 渲染徽标样式。

## Capabilities

### New Capabilities

(无)

### Modified Capabilities

- `library-experience`: 在歌曲列表行展示上新增「音质徽标」需求——徽标档位判定、显示条件、视觉区分(SQ 黄 / HQ 蓝、小一号字)以及不干扰行既有操作。

## Impact

- **echo-desktop IPC**:`crates/echo-desktop/src/ipc/dto.rs` — `SongView` 增加 `quality` 派生字段(纯读时计算,无 DB 变更)。
- **生成类型**:`apps/desktop/src/ipc/ipc-types.generated.ts` — 经 `pnpm generate:ipc` 重新生成。
- **前端渲染**:`apps/desktop/src/features/library/SongRow.tsx` — 歌曲名后追加徽标 span 及样式。
- **测试**:DTO 派生规则 Rust 单测;`SongRow.test.tsx` 覆盖 SQ / HQ / 无徽标。
- **无迁移、无重扫**:判定仅依赖 migration 0004 已落库的技术参数,读时派生对旧歌曲即时生效。
- 涉及界面术语与视觉:色值需与主题契约(coral/cobalt/turquoise 色板)协调,黄/蓝在三主题下保持可辨识。

## 非目标

- 不在导入/扫描时计算或持久化音质档位。
- 不新增数据库列或迁移。
- 不改动歌曲详情弹窗、播放栏等列表以外的展示(仅歌曲列表行)。
