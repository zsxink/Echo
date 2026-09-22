# Tasks: add-song-quality-badges

## 1. Rust IPC 层:派生规则与 DTO

- [x] 1.1 在 `crates/echo-desktop/src/ipc/dto.rs`(或随代码风格就近)实现纯函数 `derive_quality(format, bitrate_bps, sample_rate_hz, bits_per_sample) -> Option<QualityTier>`:先判 SQ(FLAC/WAV、位深 ≥ 24、采样率 ≥ 96 kHz),再判 HQ(MP3 ≥ 320_000 bps、Mp4 ≥ 256_000 bps、Opus/Ogg ≥ 256_000 bps),其余与 `UnknownDamaged`、参数缺失返回 `None`;并验证 `cargo test -p echo-desktop` 下新增边界单测通过(含阈值边界 319_999/320_000 等、SQ 优先于 HQ)
- [x] 1.2 定义 `QualityTier` 序列化为 `"sq"`/`"hq"`,`SongView` 增加 `quality: Option<QualityTier>` 字段并在 `From<&Song>` 中从 `song.format()` + `song.audio_parameters()` 派生;验证 `song_view_is_camel_case...` 测试族仍然通过,且新增用例确认字段序列化为 camelCase `quality`、`None` 时省略

## 2. 生成类型

- [x] 2.1 运行 `pnpm generate:ipc`(即 `cargo run -p echo-desktop --bin echo-generate-ipc`),确认 `apps/desktop/src/ipc/ipc-types.generated.ts` 的 `SongView` 出现可选 `readonly quality?: "sq" | "hq"`;验证 `git diff --exit-code` 输出仅含该生成变更、无手改痕迹

## 3. 前端展示

- [x] 3.1 `apps/desktop/src/features/library/SongRow.tsx` 在 `.track-title` 文本后追加徽标 span(如 `<span className="quality-badge q-sq|q-hq">SQ|HQ</span>`),仅当 `song.quality` 存在时渲染;验证 `pnpm --dir apps/desktop typecheck && pnpm --dir apps/desktop lint` 通过
- [x] 3.2 在主题样式(SongRow 相关 sheet/模块样式)为徽标添加样式:字号小于歌曲名、`q-sq` 黄色、`q-hq` 蓝色、`pointer-events: none`,并确认三主题 (coral/cobalt/turquoise)下与列表背景对比可读;验证 `pnpm --dir apps/desktop format:check` 通过

## 4. 测试

- [x] 4.1 `apps/desktop/src/features/library/SongRow.test.tsx` 新增用例覆盖 SQ 显示、HQ 显示、无徽标(不渲染)三行,确认徽标文本与 class 正确、不干扰行内既有点击/播放行为;验证 `pnpm --dir apps/desktop test -- SongRow`
- [x] 4.2 运行 `cargo test -p echo-desktop` 全量与 `pnpm --dir apps/desktop test`、`typecheck`、`lint`、`format:check`,确认全部绿

## 5. 验收与收尾

- [x] 5.1 逐条对照 specs/library-experience 的「歌曲音质徽标」场景验收,人工/浏览器检查列表三种呈现(SQ 黄、HQ 蓝、无徽标)、旧歌曲不重扫即时生效、行操作不受影响
- [x] 5.2 `pnpm verify:governance` 或仓库规定的验证命令通过;`git diff` 仅含本变更文件与生成产物,提交变更