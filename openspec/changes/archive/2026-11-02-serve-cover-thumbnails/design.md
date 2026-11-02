# Design

## Context

本设计基于 `main` 的 `7839321`，分支 `codex/issue-111-cover-thumbnails`。当前事实（已在 HEAD 复核）：

- `CoverCache` 只有 `put` / `get` / `gc` 三个方法（`crates/echo-core/src/application/ports/media.rs:69-77`）；`DiskCoverCache` 的 `thumbnail(asset_key, detail: bool)` 已存在（`crates/echo-core/src/infrastructure/metadata/cover.rs:108-131`），但 trait 上没有它，生产调用为零。
- `cover://` 处理器只调用 `cache.get`（`apps/desktop/src-tauri/src/cover_protocol.rs:46`），成功响应只设置 `Content-Type`、`Vary: Origin` 和按 Origin 白名单的 `Access-Control-Allow-Origin`，没有 `Cache-Control`。
- 协议以同步方式注册（`apps/desktop/src-tauri/src/bootstrap.rs:26`）。
- `CoverProtocol::parse` 先剥离 query/fragment（`crates/echo-desktop/src/platform/security.rs:115-116`），再校验 key；当前没有任何档位概念。
- `thumbnail` 的文档写“未知 key → `Ok(None)`”（`cover.rs:106-107`），实现却返回 `Err(unavailable)`（`cover.rs:125-127`），测试也钉住了 `Err`（`cover.rs:353-374`）。
- `MemoryCoverCache` 只有 `put` / `get` / `gc`（`crates/echo-core/src/application/testing/small_fakes.rs:260-282`）。
- 前端唯一 URL 组装点是 `assetUrl(key)`（`apps/desktop/src/bridge/index.ts:227-242`），生产调用点 9 处；`convertFileSrc` 来自 `@tauri-apps/api` 2.11.1。
- `docs/DESIGN.md` 只有第 1–8 节，**没有 §16**。issue 引用的 §16 asset-protocol 契约实际在归档 change `openspec/changes/archive/2026-09-15-release-0-1-0-desktop-player/design.md:396`，并被 `security.rs:84-86` 引用。当前职责划分是“协议只校验 key，尺寸枚举归前端、不进入文件系统”，本 change 要显式取代它。

分层约束：`echo-core` 不依赖 Tauri/UI；协议解析在 `echo-desktop`（纯 Rust，可单测）；Tauri 注册与响应只在 `echo-app` 薄壳；前端只通过 `assetUrl` 组合 URL。`CODE_STANDARDS.md` §4.2 反对布尔参数堆栈，§4.3 禁止在异步任务里做长阻塞工作；公开 trait 方法数上限为 6。

## Goals / Non-Goals

**Goals:**

- 列表类视图拿 128px、歌单预览拿 512px、沉浸页显式拿原图；缺省或未知档位永远落在 128px。
- 缩略图首访不阻塞 WebView 主线程；同 key 同档位并发只解码一次。
- 不改变 key 白名单、长度上限、404 语义、CSP、CORS 像素读取白名单与磁盘布局。
- 成功响应可被 WebView 长期缓存，且旧缓存条目无需迁移。
- 用可复现的测试/测量证明上述行为。

**Non-Goals:**

- 不改 `gc()` 的容量语义（当前超容量只记日志）。缩略图会增大 `stored_bytes`，容量策略另行立项跟踪。
- 不改封面磁盘布局、content-addressing、`original.bin` / `mime.txt` 文件名与 MIME 存储；不引入新图片格式或新依赖。
- 不改 JPEG 质量、缩放算法（Lanczos3）或 alpha 丢弃行为；透明原图的缩略图仍是 JPEG RGB，这是既有行为。
- 不改 macOS Now Playing 的原生取图路径（`apps/desktop/src-tauri/src/main.rs:317` 仍用 `get` 原图）。它是系统集成，不是 WebView 渲染路径；“仅沉浸页取原图”限定在 WebView 封面渲染路径内。
- 不做 `put()` 时的预生成；理由见 D5。
- 不处理 #112 / #113 / #114。

## Decisions

### D1：trait 只放行既有 `thumbnail(asset_key, detail: bool)`，不引入 Core 枚举

`CoverCache` 新增：

```rust
fn thumbnail(&self, asset_key: &str, detail: bool) -> Result<Option<Vec<u8>>, Error>;
```

`get` 继续只返回原图；`detail == false` 对应 128px，`true` 对应 512px。方法数从 3 增至 4，低于 6 的上限。

备选：

- **在 Core 引入 `CoverTier { List, Detail, Original }`**：拒绝。用户明确要求不引入新枚举、不改双档语义；原图仍走 `get`，缓存层不需要第三态。
- **拆成 `list_thumbnail` / `detail_thumbnail`**：拒绝。两个方法重复实现与文档，且未来加档位会继续膨胀 trait。
- **`thumbnail` 返回原图作为 `original` 档**：拒绝。缓存层无法表达“原图”与“512px”的区别，且会让缺省档位有回落到原图的风险。

协议层确实需要三态 `list | detail | original`，因为沉浸页必须**显式**请求原图，不能靠缺省。该三态用 `echo-desktop` 内的小型 `CoverSize` 表达（D3），不进入 Core，也不改变 `detail` 语义。

### D2：`thumbnail` 的 None/Err 契约统一为文档语义

- 未知 key（key 形状合法、缓存条目不存在）→ `Ok(None)`。
- key 畸形（前缀/长度/十六进制不合法）→ `Err(Validation)`。
- 原图存在但不可解码 → `Err(CorruptMedia)`。
- I/O 失败 → `Err(Io)`。

理由：文档已经这样写，且协议对 `Ok(None)` 与 `Err(_)` 都返回 404，统一不会改变可观察行为；在把方法放上 trait 前修正，避免把矛盾契约扩散给未来的调用方。`cover_cache_thumbnail_missing_and_corrupt_originals` 中未知 key 的断言改为 `Ok(None)`，损坏原图仍断言 `corrupt_media`。生成失败不写文件，已有缩略图不被覆盖。

### D3：协议在剥离 query/fragment 之前解析档位，`s` 永不参与 key 校验

`CoverProtocol::parse` 返回 `CoverRequest { asset_key: String, size: CoverSize }`：

```rust
pub enum CoverSize { List, Detail, Original }
```

解析顺序：

1. 校验并剥离 `cover://` / `COVER://`。
2. 在 `#` 处截断 fragment；在 `?` 处切出 query，保留 query 字符串。
3. 从 query 的 `&` 分隔项中查找第一个 `s`；`list` / `detail` / `original` 精确匹配，缺省、空值、未知值、重复项（取第一个）一律 `List`。不做百分号解码：`%64etail` 不是档位，安全回落。
4. 用剩余的 path 按既有逻辑取 key 候选（`cover://asset/<key>`、`cover://<key>`、`cover://localhost/<key>`），再走 `is_valid_key`。

约束：

- `s` 只做三值 allowlist，**不参与** `is_valid_key`、长度计算或路径拼接。`?s=../../etc/passwd` 的 key 仍是 query 前的合法 key，档位回落 `List`；如果 key 本身畸形（`cover://asset/../../etc/passwd`），仍 `InvalidKey` → 404。
- 缺省和未知档位是 `List`，不是 `Original`；只有字面 `s=original` 才读原图。
- 既有 `KeyTooLong` 分支目前不可达（`is_valid_key` 对超长返回 false，`parse` 返回 `InvalidKey`）。本 change 保持既有 404 行为，不把超长 key 改成 400；该分支的清理另行处理。
- 备选：保留 `parse(String)`，另加 `parse_size(uri)`。拒绝：两次解析容易在剥离顺序上分叉，且调用方必须同时处理两个返回值。备选：引入 `url` crate 解析 query。拒绝：`cover://` 去掉 scheme 后不是标准 URL，且当前 query 语法只有 `s=` 一项；手写切分更小、更可控。

### D4：前端在 `convertFileSrc` 之后追加 `?s=`，且只对 cover 基址追加

`assetUrl` 改为：

```ts
export type CoverSize = "list" | "detail" | "original";

export function assetUrl(key: string, size: CoverSize = "list"): string {
  const base = convertFileSrc(key, COVER_SCHEME);
  if (!isCoverBase(base)) return base;
  return `${base}${base.includes("?") ? "&" : "?"}s=${size}`;
}
```

`isCoverBase` 识别 `cover://`、`http://cover.localhost/`、`https://cover.localhost/`。原因：

- `convertFileSrc` 会对**路径参数**做 percent-encoding。`convertFileSrc(key + "?s=list")` 会把 `?`/`=` 编成 `%3F`/`%3D`，协议看到的是一个畸形 key。追加必须发生在它之后。
- macOS/Linux 的基址是 `cover://localhost/<key>`，Windows/Android 是 `http://cover.localhost/<key>`。wry 在 Windows 上把后者还原为 `cover://localhost/<key>` 再交给自定义协议处理器，query 在还原中保留；解析器同时接受 `cover://localhost/<key>?s=detail` 与 `cover://asset/<key>?s=detail`。
- 测试替身可能返回 `data:` / `blob:` URL（`apps/desktop/e2e/mock-bridge.ts:980-981` 就是这样）。对非 cover 基址不追加，避免破坏 data URI。
- 缺省参数是 `list`，所以任何忘记传档位的调用点最坏得到 128px，不会回到原图。

### D5：异步 URI scheme + `spawn_blocking`，不做 `put()` 预生成

桌面壳把 `register_uri_scheme_protocol` 换成 `register_asynchronous_uri_scheme_protocol`。处理器：

1. 解析 `CoverRequest`；解析失败立即 404（既有语义），不启动工作线程。
2. 取 `Arc<dyn CoverCache>`；缺失立即 404。
3. 取 `Origin` 头为 `Option<String>`，克隆 cache、request、responder。
4. `tauri::async_runtime::spawn_blocking(move || { ... })`：按档位调用 `get` 或 `thumbnail`，构造响应，`responder.respond(...)`。

`UriSchemeResponder` 内部是 `Box<dyn FnOnce(...) + Send>`，可以在工作线程调用；`spawn_blocking` 使用 tokio 的阻塞池，符合 §4.3“异步任务里不做长阻塞工作”。

备选：

- **同步处理器 + 在 `put()` 预生成 128/512**：拒绝。它只能覆盖新写入的封面；现有缓存条目仍然首访解码，且扫描/导入会为可能永不展示的封面付出两次缩放成本。异步方案同时覆盖新旧条目。
- **自建线程池**：拒绝。Tauri 已提供阻塞池，新增池只增加生命周期与关闭复杂度。
- **在同步处理器里直接解码，但加一个全局互斥**：拒绝。只是把并发变成串行，仍然阻塞主线程。

### D6：`DiskCoverCache` 按 `(key, 档位)` 单飞，原子写保持半成品不可见

`DiskCoverCache` 增加：

```rust
inflight: Arc<Mutex<HashMap<(String, bool), Arc<Mutex<()>>>>>,
```

`thumbnail` 的流程：

1. 校验 key，算出 `hash` 与 `max_px`/`thumb_path`。
2. 先尝试读 `thumb_path`；命中直接返回。
3. 取得该 `(hash, detail)` 的闸门（map 锁只用于取/放 Arc，不跨解码持有）。
4. 拿闸门锁后**再查一次磁盘**；若已被前一个请求写好，直接返回。
5. 读原图；未知 key → `Ok(None)`。
6. `scale_to`，`write_atomically`，返回。
7. 释放闸门并从 map 移除；移除前用 `Arc::ptr_eq` 确认移除的是自己那一把，避免误删后来者的新闸门。

等待者被 `std::sync::Mutex` 阻塞，但它们本身运行在 `spawn_blocking` 线程里，不占用 WebView 主线程。锁中毒用 `unwrap_or_else(PoisonError::into_inner)` 恢复，保证一次 panic 不会永久毒化该 key。生成失败时闸门照常释放，后续请求可以重试。

单飞只对**成功生成**承诺“只解码一次”。失败路径不缓存失败（`Error` 不是 `Clone`，缓存失败还会掩盖后续重试），等待者会重试并同样失败；这只发生在损坏原图/写入失败的异常路径，代价是重复一次失败解码，换取错误分类不被篡改、后续修复后能自然恢复。

备选：

- **在 Tauri 壳里做单飞**：拒绝。Core 级单飞对所有调用方生效，且不把并发策略塞进薄壳。
- **`OnceCell` / `OnceLock` 每 key 一个**：拒绝。失败后不可重试，且 map 生命周期与清理更复杂。
- **异步单飞（tokio `Notify`）**：拒绝。`DiskCoverCache` 是同步、跨平台、与运行时无关的 Core 类型，不应依赖 tokio。

原子写沿用 `write_atomically`（`.tmp` + `rename`，`cover.rs:193-199`）。同 key 同档位由闸门串行化；不同档位写不同文件；`put` 写 `original.bin` / `mime.txt`，文件名不同，`.tmp` 不冲突。中断留下的 `.tmp` 不会被读取，`gc` 删除整个条目时一并清理。

### D7：成功响应加不可变缓存头，保留既有 CORS 语义

成功响应增加：

```text
Cache-Control: private, max-age=31536000, immutable
```

同时保留 `Content-Type`（按字节 magic 嗅探）、`Vary: Origin` 和按 Origin 白名单的 `Access-Control-Allow-Origin`。key 是内容寻址的，`?s=` 使三档成为不同 URL，因此“一年 + immutable”不会读到过期内容。`private` 是因为这是本机 WebView 的响应，不应进入共享代理缓存。404 不设该头。

备选：`public` —— 拒绝，语义上把本机资源暴露给共享缓存；`no-cache` / 不设头 —— 拒绝，WebView 会重复请求，抵消缩略图的收益。

### D8：前端档位归位表

| 调用点 | 语义 | 档位 |
| --- | --- | --- |
| `apps/desktop/src/features/player/ImmersivePlayer.tsx:193` | 沉浸页黑胶封面 + 主色提取 | `original` |
| `apps/desktop/src/features/player/PlayerBar.tsx:85` | 常驻播放条 | `list` |
| `apps/desktop/src/features/player/QueuePanel.tsx:37` | 播放队列 | `list` |
| `apps/desktop/src/features/library/SongRow.tsx:114` | 歌曲行 | `list` |
| `apps/desktop/src/features/library/SongMenu.tsx:293` | 歌曲菜单缩略图 | `list` |
| `apps/desktop/src/features/library/CollectionDirectory.tsx:578` | 目录卡片 | `list` |
| `apps/desktop/src/app/App.tsx:364` | 歌单卡 | `list` |
| `apps/desktop/src/features/playlists/AddToPlaylistDialog.tsx:244` | 添加到歌单对话框中的歌单卡 | `list` |
| `apps/desktop/src/features/playlists/PlaylistNameDialog.tsx:274` | 歌单封面预览（已保存封面分支） | `detail` |

`PlaylistNameDialog` 的 blob 预览分支继续用 `URL.createObjectURL`，不经过 `assetUrl`，不涉及档位。`ImmersivePlayer` 的 `artwork` URL 同时传给 `useArtworkTint`（`ImmersivePlayer.tsx:194`），所以主色提取自然继续基于原图；`crossOrigin="anonymous"` 与协议 CORS 不变。

### D9：职责划分的修订（取代旧 §16 契约）

新的分工：

- **前端**：按视图语义选择档位；缺省 `list`。
- **协议（`echo-desktop`）**：解析并 allowlist 档位，执行安全缺省，校验 key；档位不进入文件系统路径。
- **Core 缓存**：把 `detail` bool 映射到 128/512，按需生成与单飞。
- **安全边界**：key 白名单、长度上限、404、CSP、CORS 不变。

落地位置：

- `crates/echo-desktop/src/platform/security.rs` 的模块文档与 `CoverProtocol` 文档改为上述分工，删掉“size enum 永远不进入协议”的旧句子。
- `docs/DESIGN.md` 增加“封面资产协议”小节（第 2 节之下），写明 `s=list|detail|original`、默认 `list`、档位不参与 key 校验、缩略图按需生成与不可变缓存头。
- 归档 change 的 §16 保持历史原文不动；新文档明确它已被本 change 取代，避免再造一个“引用了不存在的 §16”的注释。
- `bridge/index.ts:227` 与 `task-12.7.mjs:5` 的 `design §16` 注释同步指向新小节/新要求。

### D10：测试替身与 e2e 的更新

- `MemoryCoverCache::thumbnail` 返回已存字节（两档同值）。它是测试替身，不模拟缩放；需要区分档位的测试用专门的记录型 fake。
- 前端模块 mock 的 `assetUrl` 接收第二个参数并写入 URL，例如 `(key, size = "list") => \`cover://${key}?s=${size}\``；受影响断言更新为带 query 的形态。
- `test/setup.ts` 的 `convertFileSrc` 仍返回 `cover://<key>`，`assetUrl` 会在其后追加 query；这正好覆盖真实组合顺序。
- e2e mock 返回 data URI，`assetUrl` 不追加 query，e2e 继续渲染；档位由单元测试与 Rust 协议测试覆盖。
- 增加一个 `assetUrl` 单元测试，mock `convertFileSrc` 模拟 Tauri 的 `encodeURIComponent`，断言最终 URL 是 `cover://localhost/cv1-abc?s=detail`，而不是被编码的 `%3F`。

### D11：证据与测量方式

- 缩略图存在与尺寸：在真实缓存目录执行 `find ... -name 'thumb-*.bin' | wc -l` 和 `file <path>`（`file` 按 magic 报告 JPEG 尺寸，不依赖扩展名）。
- 解码后尺寸：Rust 测试用 `image::load_from_memory` 断言最长边 ≤128 / ≤512；真实数据用 `file` 的尺寸输出复核。
- 沉浸页原图：断言请求 URL 带 `s=original`，并用同一 URL 的截图/像素探针确认与改动前一致。
- macOS 内存：在实现前先从当前 HEAD 构建并运行应用，打开含多封面的歌曲列表，记录 `Echo Web Content`（WKWebView 内容进程）的 `footprint -p <pid>` 基线；实现后再测同一场景。若环境无法启动 GUI 或找不到内容进程，记录具体命令与失败原因，不给数字。
- 首访/滚动：在真实应用中用 500+ 首歌、24 行可见列表滚动；观察是否掉帧，并记录协议日志中缩略图生成耗时（仅 debug 日志）。若无法量化，报告“为主观观察 + 协议线程证据”，不把它写成测量数字。
- 门禁：`pnpm verify:governance`、`cargo test -p echo-core --all-features`、`cargo test -p echo-desktop --all-features`、`cargo test -p echo-app`、`pnpm --dir apps/desktop test`、`pnpm --dir apps/desktop build`（具体脚本以 `apps/desktop/package.json` 为准）。

## Risks / Trade-offs

- **首访并发解码的 CPU 峰值**：24 个不同 key 会各占一个阻塞线程。单飞只能合并同 key；不同 key 仍并行。→ 先测量；若卡顿仍明显，再评估全局并发上限（不在本 change 预设，避免过度设计）。
- **阻塞池被等待者占满**：同 key 的等待者会各占一个阻塞线程。→ 单飞已把重复解码降为一次；等待者只是阻塞读，成本远低于解码。若真实场景出现池饥饿，再引入信号量。
- **Windows 的 query 保留依赖 wry 的 URL 还原**：已在 wry/Tauri 源码确认 `http://cover.localhost/<path>` 会还原为 `cover://localhost/<path>` 且保留 query。→ 解析器同时接受两种 host 形态，并加单测；Windows 真机检查列为任务。
- **`Cache-Control: immutable` 与未来格式变化**：key 是内容寻址的，档位是确定性的；如果未来改变缩略图编码/尺寸，必须换 key 前缀（如 `cv2-`）或换 URL 参数，否则一年缓存会读到旧图。→ 在 comments 与 design 中登记这个约束；本 change 不改编码。
- **透明原图的 alpha 丢失**：JPEG 缩略图不保留 alpha，透明区域可能变黑。→ 这是既有行为，登记并在测试中固定；若产品认为不可接受，另立 issue 改格式。
- **`gc()` 容量语义未处理**：缩略图会增大 `stored_bytes`，超容量仍只记日志。→ 明确 out of scope，另立 issue 跟踪；本 change 不顺手改。
- **超长 key 的 400 分支不可达**：保持现状（404），避免改变安全验收；清理另立 issue。→ 测试断言现状，不引入行为变化。
- **测试替身与真实缓存行为不一致**：`MemoryCoverCache` 不做缩放。→ 文档标注为 fake；协议层测试用记录型 fake 验证调用了正确的 `thumbnail`/`get`，尺寸由 `DiskCoverCache` 测试覆盖。

## Migration Plan

1. **基线（先于代码改动）**：从当前 HEAD 构建并运行，记录 macOS 内存基线与首访/滚动观察；若环境不允许，记录原因。
2. **Core**：trait 加 `thumbnail`、统一 `Ok(None)`、实现单飞与原子写；更新 `MemoryCoverCache` 与测试。此时旧协议仍走 `get`，行为不变。
3. **协议层**：`CoverRequest`/`CoverSize`、解析顺序与 allowlist、单测；更新 `security.rs` 文档。
4. **桌面壳**：异步注册 + `spawn_blocking` + `Cache-Control`；补响应构造的纯函数测试。
5. **前端**：`assetUrl` 档位 + 9 个调用点 + 单测/模块 mock/e2e mock。
6. **文档与治理**：`docs/DESIGN.md`、traceability、scenario 命令与 manifest、`task-12.7` 测试清单；运行完整门禁。
7. **回滚**：纯代码回滚。旧代码只读 `original.bin`，会忽略新增的 `thumb-*.bin`；`gc` 删除条目时一并删除缩略图，因此不需要数据迁移或清理脚本。

## Open Questions

无会改变规格、方案或任务拆解的未决问题。JPEG 质量是否提高、`gc` 容量策略、超长 key 的 400 清理都明确不在本 change 内，作为后续 issue 跟踪。
