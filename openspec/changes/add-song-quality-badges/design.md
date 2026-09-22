# Design: add-song-quality-badges

## Context

动机见 proposal.md。现状要点:

- 列表查询 `SONG_SELECT` 一次性读回 `song` 全部列,`Song` 实体已挂有 `format` 与音频参数字段(`bitrate_bps / sample_rate_hz / channels / bits_per_sample`,migration 0004),见 `crates/echo-core/src/domain/entities.rs`。
- IPC 层 `SongView` 的 `From<&Song>` 位于 `crates/echo-desktop/src/ipc/dto.rs`,前端类型由 `pnpm generate:ipc` 生成 `apps/desktop/src/ipc/ipc-types.generated.ts`。
- 列表行组件为 `apps/desktop/src/features/library/SongRow.tsx`,歌曲名渲染于 `.track-title`。
- 主题契约:三主题 coral/cobalt/turquoise,无 light/dark 概念;徽标需在三主题下均可辨识。

## Goals / Non-Goals

**Goals:**

- 歌曲列表行按统一判定规则显示 SQ/HQ 徽标,读时派生、零迁移。
- 判定逻辑单一来源,DTO 单测可覆盖全部边界。

**Non-Goals:**

- 不在 Core 持久化或导出音质档位(见 proposal 非目标)。
- 不改歌曲详情弹窗、播放栏等列表外展示。

## Decisions

### D1: 读时派生,而非导入时入库

在 `SongView: From<&Song>` 转换处计算 `quality` 字段。

- 备选 A(弃):导入时算好加列入库 → 需要迁移、旧行需重扫/回填、参数更新时脏数据,收益为零(判定是纯比较,无 I/O)。
- 备选 B(弃):前端计算 → 判定规则会泄漏进 UI 层,且违背「跨边界 DTO 与领域模型分离」;规则应只在 Rust 侧维护一份。
- 选 B 的变体即当前方案:DTO 层派生,规则测试写在 Rust 侧,前端只做展示。

### D2: 判定规则实现位置

在 `echo-desktop` 的 dto 模块内实现纯函数 `derive_quality(format, bitrate_bps, sample_rate_hz, bits_per_sample) -> Option<QualityTier>`(具体命名以现状代码风格为准),不放进 `echo-core`:

- Core 不需要此概念(UI 展示关注点);放在 desktop IPC 层符合「UI 相关 DTO 由 desktop 负责」的边界。
- 若实现中发现 `AudioFormat` 判定更适合放 Core(如已有共享判定工具),再下沉,并保持 Core 无 UI 依赖。

规则优先级:先判 SQ(FLAC/WAV、≥24bit、≥96kHz),命中即返回;否则判 HQ 码率阈值;否则 `None`。码率统一以 `bitrate_bps` 比较(320 kbps = 320_000,256 kbps = 256_000),避免单位换算歧义。Opus/Ogg 归属以 `AudioFormat` 枚举现有变体为准。

### D3: DTO 字段与兼容

`SongView` 新增 `quality: Option<QualityTier>`(`"sq" | "hq"`),`None` 序列化为缺省(与现有 optional 字段惯例一致,如 `song_view_is_camel_case` 测试所锁定的风格)。旧前端收到多余字段无影响;新前端对旧 payload(无该字段)按无徽标渲染——同进程同版本部署,无跨版本窗口,风险可忽略。

### D4: 前端渲染与样式

`SongRow.tsx` 在 `.track-title` 文本后追加 `<span class="quality-badge q-sq|q-hq">SQ|HQ</span>`:

- 字号比歌曲名小一号(使用主题既有字号阶梯,不硬编码 px 之外的魔法值);SQ 黄、HQ 蓝——在三主题(coral/cobalt/turquoise)下取既有语义色 token 或新增专用 token,须在三主题下与列表背景对比可读。
- `pointer-events: none`(或不绑定任何交互),纯展示,不进入焦点序,不影响行点击/菜单/多选。
- 徽标为行内元素,不改变行高与列布局(列表列宽、序号、操作列不受影响)。

### D5: 生成代码纪律

`ipc-types.generated.ts` 只经 `pnpm generate:ipc` 更新,禁止手改。

## Risks / Trade-offs

- [码率单位误用(kbps vs bps)] → 判定函数以 bps 为唯一单位,单测覆盖阈值边界(319_999 / 320_000 等)。
- [格式枚举与规则不符(如 M4A 中的 ALAC)] → 按 issue 基线,非 FLAC/WAV 无损变体不入 SQ;若 `AudioFormat` 后续增加无损变体,规则测试会先行暴露,再走变更更新规格。
- [三主题下黄/蓝对比度不足] → 取色基于主题 token 并在三主题下目测验证;必要时用主题内已验证的近似色,规格只要求「黄色/蓝色可辨识」。
- [徽标挤压长歌曲名] → 歌曲名容器保持省略号截断,徽标随行尾 flow,不换行撑高行高。

## Migration Plan

无数据库迁移、无数据回滚需求。代码回滚即行为回滚(字段为 optional,旧代码忽略之)。

## Open Questions

无——判定规则与实现路线已按 issue 基线确认。
