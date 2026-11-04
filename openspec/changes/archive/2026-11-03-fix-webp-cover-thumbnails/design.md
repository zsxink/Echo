# Design

## Context

封面链路当前分为三段：扫描适配器把内嵌封面字节写入 `DiskCoverCache` 并记录 `cover_assets.mime_type`；`cover://` 协议按 `s=list|detail|original` 选择读取路径；`DiskCoverCache` 在首次访问缩略图档位时调用 `scale_to`，后者用 `image::load_from_memory` 解码原图、以 Lanczos3 缩放到 128/512px 并输出 JPEG。协议把任何缩略图错误统一映射为 404，前端 `onError` 将行标记为 `artworkUnavailable`。

`image` 0.25.6 在 workspace 中关闭默认特性，只启用 `png`/`jpeg`。`load_from_memory` 依据字节猜测容器，WebP 因解码特性缺失直接返回 `Unsupported("The image format WebP is not supported")`，被包装成 `corrupt_media`。MIME 不参与解码路径；它由标签适配器推导后落库，供协议原图档位的字节嗅探与诊断使用。当前 `mime_of` 会把 lofty 的未知字符串原样透传，因此它可能声明一个没有任何解码器的图片类型。

约束：Core 不依赖 UI/播放器；封面缓存的磁盘布局、内容寻址与 `gc` 语义不因本 change 改变；缩略图始终是 JPEG，不引入格式转换或“回退原图”。

## Goals / Non-Goals

**Goals:**

- 让所有会被 `mime_of` 声明的图片格式（PNG/JPEG/GIF/BMP/TIFF/WebP）都能生成 `list`/`detail` 缩略图。
- 把“声明图片 MIME”与“已启用解码器”绑定为可测试的不变量，防止未来再次漂移。
- 保持既有失败语义：不可解码的原图不产生半成品缩略图，协议仍返回 404，原图档位仍可读取。

**Non-Goals:**

- 不引入 AVIF/HEIC/EXR 等重型或未声明格式；不启用 `image` 默认特性。
- 不在扫描阶段拒绝封面或新增 warning 类型：所有标签可声明的格式都被支持，剩余未知声明按不透明字节保存。
- 不改前端回退逻辑、协议 404 语义、缩略图尺寸/质量或缓存布局。

## Decisions

### D1：启用 `webp`/`gif`/`bmp`/`tiff` 四项解码，而不是只加 `webp`

`mime_of` 的已知 lofty 变体已经包含 GIF/BMP/TIFF，只修 WebP 会让同一类 404 换一种格式继续存在。四个特性全部是纯 Rust 实现（`image-webp`、`gif`、`tiff`），不引入系统库或平台分支，符合 Core 跨平台约束。继续 `default-features = false`，只显式列出模板需要的格式，避免 `avif`（`ravif`/`dav1d`）等重型依赖。

被否决的替代方案：仅启用 `webp` 并把其余格式在扫描阶段拒绝。这会让用户已有的 GIF/BMP/TIFF 封面从可显示退化为不可显示，且需要新增 warning 语义与扫描迁移，成本更高。

### D2：`mime_of` 输出规范化，未知声明降级为 `application/octet-stream`

可解码集合固定为六个图片 MIME。lofty 已知变体直接映射；未知字符串先 `trim` + `to_ascii_lowercase`，命中集合才保留，否则返回 `application/octet-stream`。这样“图片 MIME 声明”本身就是可解码承诺，约束测试可以断言：输出只要不是 `application/octet-stream`，对应 `image::ImageFormat` 的 `reading_enabled()` 必须为真。

被否决的替代方案：保留未知字符串原样。这样 `image/avif`、`image/heic` 等声明会持续存在，约束测试无法形成封闭集合，也会让 UI/日志误以为这些格式可缩略。降级不丢数据：原图字节仍写入缓存，`s=original` 仍返回嗅探后的媒体类型。

### D3：解码以字节为准，MIME 只作声明

`scale_to` 继续使用 `load_from_memory` 按容器字节解码，不按 MIME 选择解码器。MIME 规范化只影响声明与诊断，不参与缩略图生成；真正的修复是 D1 的编译特性。这样即使标签声明的 MIME 与字节不一致，可解码的字节仍能生成缩略图。

### D4：用真实 lossy WebP fixture 覆盖缓存入口

在 `crates/echo-core/src/infrastructure/metadata/fixtures/cover.webp` 放置一张由 `cwebp` 生成的 600×400 lossy WebP，通过 `include_bytes!` 在 `cover.rs` 测试中使用。测试从 `DiskCoverCache::put(..., "image/webp")` 进入，分别请求 `list`/`detail`，断言 JPEG magic、最长边上限与两档独立文件，从而同时覆盖 `scale_to` 与 `thumbnail`。不构造 lossless WebP 生成器，因为真实内嵌封面通常是 lossy VP8，且 `image-webp` 对两者的解码路径不同。

### D5：不新增扫描 warning，不清理缓存

所有 lofty 可声明的图片格式都已支持；未知声明已降级为不透明类型，不再冒充图片。失败路径本就不写缩略图，重新编译后首次访问按需生成，因此不需要缓存迁移或清理。用户重启应用即可使用新解码器。

### D6：分层与依赖方向

`mime_of` 与 `scale_to` 都位于 `echo-core` 的基础设施（标签/封面适配器），`image` 已是 echo-core 的直接依赖；本 change 只改变其编译特性，不新增跨层依赖，也不把图片格式知识引入 Domain 或 Presentation。Domain 仅更新 `EmbeddedCover::mime` 的文档，说明“声明且可解码”的语义。

## Risks / Trade-offs

- **二进制体积与编译时间增加** → 只启用四个轻量纯 Rust 特性；通过 `cargo tree -p echo-core -e features -i image` 记录启用集合，若后续需要裁剪，可再按格式评估。
- **WebP 解码行为差异**（动画 WebP 只取首帧、内存上限与 libwebp 不同）→ 缩略图仅需要静态首帧，与 GIF 现有行为一致；JPEG 输出与尺寸上限保证下游不变。
- **未知 MIME 降级改变数据库声明** → 现有行不会自动改写，只有重扫才会更新；同步事实的 digest 可能随声明变化，但仅影响原本就无法缩略的极端封面。设计中接受该变化，因为旧声明本身不可兑现。
- **解码资源消耗**（大尺寸/压缩炸弹）→ 输入已有 20 MiB 标签限制，缩略图输出受尺寸上限约束；这是 PNG/JPEG 已存在的风险，本 change 不扩大输入上限。若需要更强的解码内存上限，另立 issue 评估 `image::Limits`。
- **测试 fixture 与实现同仓库** → fixture 是固定字节，测试不依赖外部工具；`cwebp` 只在生成 fixture 时使用，CI 不需要安装。

## Migration Plan

1. 合并后重新构建应用；首次访问旧缓存条目时按需生成缩略图，无需清理缓存。
2. 已有 `image/webp` 等声明保持不变；未知声明在下次扫描时降级为 `application/octet-stream`，原图字节不变。
3. 回滚只需恢复 `image` 特性与 `mime_of` 实现；已生成的 JPEG 缩略图仍可被旧版本读取，无数据迁移。
