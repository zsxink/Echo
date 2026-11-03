# Proposal

## Why

issue [#134](https://github.com/zsxink/Echo/issues/134) 报告内嵌 WebP 封面的歌曲在列表、播放条、队列等 `list` 档位只显示占位色块，`detail` 档位同样无法生成缩略图。根因不在缓存或协议：扫描已把封面字节写入内容寻址缓存，`cover_assets.mime_type` 也正确落库为 `image/webp`；失败发生在 `scale_to` 的 `image::load_from_memory` —— workspace 的 `image` 只启用了 `png`/`jpeg` 解码，WebP 原图被包装成 `corrupt_media`，协议统一映射为 404，前端将该行标记为不可用。

这不是 WebP 单点问题：`mime_of` 还会产出 `image/tiff`、`image/bmp`、`image/gif` 以及任意 `MimeType::Unknown(value)`，这些封面同样会在缩略图阶段 404。现在修复，是因为 #111 已把按需缩略图与档位协议稳定下来，改动可以收敛在“解码能力 + MIME 声明策略 + 回归测试”，不动缓存布局、协议语义或前端。

## What Changes

- 在 workspace 的 `image` 依赖上启用 `webp`、`gif`、`bmp`、`tiff` 解码，继续关闭默认特性与其它重型格式；`png`/`jpeg` 行为不变。
- 收紧 `mime_of` 的输出策略：已知的 lofty 变体映射为规范 MIME；`MimeType::Unknown(value)` 先去空白并小写化，只有落在缩略图管线可解码集合（PNG/JPEG/GIF/BMP/TIFF/WebP）内才保留，否则声明为 `application/octet-stream`。由此 `mime_of` 不再对外声明一个没有解码器的图片格式。
- 更新 `EmbeddedCover::mime` 的文档语义：它是“声明且可解码的封面 MIME；无法解码的声明降级为不透明字节”。
- 新增回归测试：用真实的 lossy WebP fixture 覆盖 `DiskCoverCache::thumbnail` 的 `list`/`detail` 两档；新增约束测试，枚举 `mime_of` 可能产出的图片 MIME，断言每个都能被已启用的 `image` 解码器读取。
- 不清理缓存、不改磁盘布局：失败路径本来就不写缩略图，重新编译后按需生成；用户重启应用即可。

**事实校核**：issue 引用的 `Cargo.toml:81`、`cover.rs:334 scale_to`、`tags/mod.rs` 的 `mime_of` 与当前 HEAD 一致；`image 0.25.6` 的 `webp` 特性由纯 Rust 的 `image-webp` 提供，可解码 lossy/lossless WebP，不需要系统 libwebp。`MimeType::Unknown` 是字符串透传，因此 WebP 依赖这一分支保留 `image/webp`。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `desktop-app-shell`：新增「封面缩略图必须解码已声明的图片格式」要求，锁定 WebP 等格式在两档缩略图中的行为。
- `local-library`：新增「内嵌封面 MIME 声明必须与解码能力一致」要求，锁定 `mime_of` 的规范化与降级策略。

**归属理由**：缩略图生成与 `cover://` 档位属于桌面壳既有能力 `DAS-R20`；`mime_of` 与扫描入库属于本地资料库的元数据解析范围。两者分别落在既有能力末尾，不移动或重编号任何已有 Requirement/Scenario。

## Impact

- **依赖**：`Cargo.toml` 的 `image` 特性新增 `webp`/`gif`/`bmp`/`tiff`；`Cargo.lock` 新增 `image-webp`（以及 `gif`/`tiff` 的纯 Rust 传递依赖）。不启用 `avif`/`dds`/`exr`/`hdr`/`ico` 等未声明格式，避免重型依赖。
- **代码**：`Cargo.toml`、`crates/echo-core/src/infrastructure/metadata/tags/mod.rs`（`mime_of`）、`crates/echo-core/src/domain/media.rs`（文档）、`crates/echo-core/src/infrastructure/metadata/cover.rs`（测试与 fixture）。协议与前端无需改动。
- **数据**：已入库的 `image/webp` 等声明保持不变；不受支持的 `Unknown` 声明在新扫描中降级为 `application/octet-stream`，原图字节仍写入缓存并可通过 `s=original` 读取。
- **不在范围**：不引入 AVIF/HEIC 解码；不以“缩略图失败回退原图”规避问题；不改缩略图 JPEG 输出、Lanczos3、alpha 丢弃、5 MiB 上传白名单或缓存容量语义；不新增扫描 warning，因为声明格式已全部被支持，剩余未知格式按不透明字节处理。
