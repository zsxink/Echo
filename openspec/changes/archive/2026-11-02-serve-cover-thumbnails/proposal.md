# Proposal

## Why

封面是目前唯一仍整张走原图的用户可见图片：`cover://` 协议只调用 `CoverCache::get`，而 `DiskCoverCache` 已经能生成 128 / 512 两档缩略图，却没有任何生产调用方（issue #111）。一屏 24 行歌曲会各自解码并上传原图，播放条、队列与歌曲行还会重复请求同一张封面；WebView 的内存与首屏解码成本随原图尺寸放大，缩略图能力则成为死代码。

现在处理，是因为封面写入、content-addressing、CSP 与协议边界都已稳定：可以只改读取路径与档位选择，不动写入、磁盘布局和既有安全边界。

## What Changes

- 在 `CoverCache` trait 上放行既有的 `thumbnail(asset_key, detail: bool)`；`get` 仍只返回原图。缓存 trait **不引入新枚举**，保持 `detail` 双档语义。未知 key 的契约统一为 `Ok(None)`（当前实现返回 `Err(unavailable)`），畸形 key 仍为校验错误，损坏原图仍为 `corrupt_media`。
- `cover://` 协议在剥离 query/fragment **之前**解析 `s=list|detail|original`；缺省或未知 `s` 回落到 `list`，绝不回落到原图。`s` 只选择缓存读取档位，不参与 key 白名单、长度上限或文件系统路径构造；未知/畸形 key 仍返回 404，CSP 不放宽。
- 桌面壳改用异步 URI scheme 注册，把缩略图的读取、解码、缩放与落盘放到 WebView 主线程之外；`DiskCoverCache` 按 `(key, 档位)` 做单飞合并，原子写保证半成品不可见。
- 成功响应增加基于内容寻址的不可变 `Cache-Control` 头；原图与两档缩略图都受益。
- 前端 `assetUrl(key, size = "list")` 表达档位，并在 `convertFileSrc` **之后**追加 query，避免被其路径编码吞掉。9 个生产调用点逐一归位：列表类 128、歌单预览 512、沉浸页显式原图。
- 更新 `MemoryCoverCache`、缓存/协议/前端测试和 e2e mock，并同步 `crates/echo-desktop/src/platform/security.rs` 与 `docs/DESIGN.md` 中“尺寸枚举只归前端、不进入协议”的旧职责划分。

**事实校核（与 issue 的偏差，先报告）**：当前 `docs/DESIGN.md` 只有第 1–8 节，**没有 §16**；issue 引用的 §16 asset-protocol 契约实际位于归档 change `openspec/changes/archive/2026-09-15-release-0-1-0-desktop-player/design.md:396`，并被 `crates/echo-desktop/src/platform/security.rs:84-86` 引用。本 change 不假装该节仍存在：它通过 `desktop-app-shell` 的新增要求、`docs/DESIGN.md` 的新小节和 `security.rs` 文档同步，显式取代旧的职责划分。

**协议枚举的理由**：缓存 trait 继续用 `bool`（list / detail），不新增 Core 枚举。协议层需要表达三态 `list | detail | original`，因为沉浸页必须**显式**请求原图而不能依赖缺省；这里使用一个只存在于 `echo-desktop` 的小型 `CoverSize`，它不进入 Core，也不改变 `detail` 语义。

## Non-Goals

- 不改 `gc()` 的容量/驱逐语义（当前超容量只记日志）；缩略图增大 `stored_bytes` 的问题另立 issue 跟踪。
- 不改封面磁盘布局、content-addressing、`original.bin` / `mime.txt` 的字节与文件名；不引入新图片格式或新依赖。
- 不改 JPEG 质量、Lanczos3 缩放算法或透明原图的 alpha 丢弃行为。
- 不改 macOS Now Playing 的原生取图路径（仍用 `CoverCache::get` 原图）；它不是 WebView 渲染路径。
- 不处理 #112 / #113 / #114；不顺手清理超长 key 的不可达 400 分支。

## Capabilities

### New Capabilities

- 无。

### Modified Capabilities

- `desktop-app-shell`: 新增“封面资产协议按显示档位提供受控图像”与“首访缩略图生成不得阻塞 WebView 主线程”两项要求；不删改既有 19 条要求。

## Impact

- 代码：`crates/echo-core/src/application/ports/media.rs`、`crates/echo-core/src/infrastructure/metadata/cover.rs`、`crates/echo-core/src/application/testing/small_fakes.rs`、`crates/echo-desktop/src/platform/security.rs`、`apps/desktop/src-tauri/src/{bootstrap.rs,cover_protocol.rs}`、`apps/desktop/src/bridge/index.ts` 及 9 个生产调用点、相关单元测试与 e2e mock。
- 接口/协议：`cover://<key>?s=list|detail|original`；`CoverCache` 新增第 4 个方法。无 IPC 命令、数据库 schema、磁盘布局或 content-addressing 变化。
- 依赖：无新增依赖；沿用 `image`、Tauri 2.11 与 `tauri::async_runtime::spawn_blocking`。
- 兼容：旧缓存条目首次请求时懒生成缩略图；未知/畸形 key 仍 404；CSP 不变；`gc()` 的容量语义（当前只记录日志）不在本 change 内，另行跟踪。
- 产品/设计基准：`docs/PRODUCT.md` 的本地优先与离线可用不变；`docs/DESIGN.md` 增加封面资产协议小节；`openspec/CODE_STANDARDS.md` 的分层（Core 不依赖 Tauri）、“异步任务不做长阻塞工作”和公开 trait 方法数上限均满足。
