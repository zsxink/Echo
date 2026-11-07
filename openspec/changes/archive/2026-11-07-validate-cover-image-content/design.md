# Design

## Context

见 `proposal.md` 的 Why；此处只记录影响实现方式的现状与约束。

- `crates/echo-core/src/application/artwork.rs` 的 `validate(bytes, mime)` 目前有两条判断（体积、MIME 白名单），被 `save` 与 `resolve` 共用：`save` 在 `ControlPlanePort::durability().write_asset()` 之前调用，`resolve` 在 `asset.validate()`、`read_asset()`、BLAKE3/长度核对与 `CoverCache::put()` 之前调用。
- 两个写入用例是 `application/playlist/cover.rs::set_playlist_cover` 与 `application/artist_cover.rs::set_artist_cover`；读取用例是 `resolve_playlist_cover_record` 与 `resolve_artist_cover`；历史缓存路径 `resolve_legacy` 只查 `CoverCache::get`，没有目录原件字节。
- `PortableAssetReference::validate()` 只校验内容哈希与 MIME token 形状，与字节内容无关，本次不动。
- `image 0.25.6`（`Cargo.lock` 锁定）已是 `echo-core` 依赖，workspace 特性为 `png/jpeg/gif/bmp/tiff/webp`，`ImageReader::with_guessed_format()`、`format()`、`no_limits()`、`into_dimensions()`、`ImageFormat::from_mime_type()`、`reading_enabled()` 均可用。`into_dimensions()` 只构造解码器并读取头部尺寸，不构建 `DynamicImage`。
- `crates/echo-core/tests/arch.rs` 用 `layering_violations("application", "fn f() { crate::infrastructure::X::go(); }")` 禁止 Application 直接引用 `crate::infrastructure`；`ControlPlanePort` 的方法数有 15 的基线断言，不应往该 trait 上堆方法。
- 平台壳 `apps/desktop/src-tauri/src/cover_protocol.rs::cover_media_type` 已有 magic bytes 嗅探，用途是给 `cover://` 响应挑 `Content-Type`，无法识别时按 `application/octet-stream` 返回原字节；它比 `image::guess_format` 宽松（例如 PNG 只比 4 字节），属于读取侧启发式。
- `docs/DESIGN.md` §2.4 目前只写「封面写入经 Tauri command 以 base64 传递、壳边界解码为 `&[u8]` 后交给 Core」，未写 Core 的内容校验。
- `pnpm verify:governance` 已包含 `cargo fmt --all -- --check`、workspace Clippy、workspace 全量测试、场景对账、规模/覆盖率/注入等门禁；场景登记以 `scripts/verify/spec-scenarios.mjs`（当前基线 146 Requirement / 564 Scenario）为准。

## Goals / Non-Goals

**Goals:**

- 在 Core 的单一校验入口把「声明类型与实际内容一致 + 容器头可解析」变成保存阶段的可识别拒绝，且拒绝发生在任何 durable 写入之前。
- 不引入像素级解码、新依赖、新公开接口、新 Port、新 IPC 契约或数据迁移。
- 让写入、读取、歌单、艺人四条路径共享同一规则；为既有无效原件给出明确且无损的失败语义。
- 产出可追溯的 Core 单测、服务层回归、架构门禁与规格/场景登记证据。

**Non-Goals:**

- 全图像素解码、解压炸弹治理、AVIF/HEIC 等新格式；容器头合法但像素数据损坏仍可能在展示阶段失败，这是明确登记的残留边界。
- 内嵌封面扫描链路、`mime_of` 降级策略、`DiskCoverCache::put`、缩略图生成、`cover://` 档位与 `cover_media_type`。
- 既有无效原件的清理、迁移、回填或历史修复；#126（缓存 GC 容量）、#133（预览解码内存）。
- 前端 UI 改动、错误提示组件、命令参数或返回类型改动。

## Decisions

### D1 校验仍在 `artwork::validate`，且放在既有判断之后

保持 `validate(bytes, mime) -> Result<(), Error>` 签名与调用点不变，按「体积 → MIME 白名单 → 内容真实性」顺序追加第三道 guard。`save` 因此天然在任何 `write_asset` 之前失败，`resolve` 也在 `cache.put` 之前失败；四条路径不会各写一份规则。错误继续用 `Error::validation(Subject::Other, "cover", reason)`，新增三条可区分 reason（无法识别为图片 / 声明与实际不一致 / 容器头无效）；错误码、字段、`IpcErrorDto` 映射与命令签名零变化。

**被否决**：把校验放在 Tauri command 边界（`cover_payload.rs`）——`set_artist_cover` 与未来移动端同样进入 Core，壳侧校验会留下绕行入口，违反 `LD-10`；另起 `validate_for_save` 供 `save` 专用——会让 `resolve` 继续把无效原件写进缓存，与 `LD-08` 冲突。

### D2 用 `image` 的最小头部探测，而不是只比 magic 或整幅解码

在 `validate` 内以 `Cursor<&[u8]>` 做零文件 I/O 的内存探测：

1. `ImageReader::new(...).with_guessed_format()` 得到实际 `ImageFormat`（内部只预读前 16 字节做格式嗅探）；
2. `ImageFormat::from_mime_type(mime)` 得到声明对应的格式，要求 `detected == expected` 且 `detected.reading_enabled()`；声明与内容不一致、嗅探不到格式、或该格式解码器未启用都判为校验错误；
3. `reader.no_limits()` 后再 `into_dimensions()`，要求解析出非零尺寸；头部截断/非法即拒绝。

`no_limits()` 是必需的：`ImageReader` 默认 `Limits::default()` 带 512 MiB 分配上限，`into_decoder()` 会调用 `decoder.set_limits()`；若不加清除，超大但合法尺寸的封面会被**误拒**，等于悄悄改变既有保存能力。清除限制后仍只读取头部，不分配像素缓冲。

**被否决**：

- 只比 magic bytes（issue 的第一个建议）：`\x89PNG` 后面接垃圾也能通过，无法覆盖 issue 明确点名的「恶意构造的字节」；且与平台壳已有的嗅探语义重复但不一致。
- `ImageReader::decode()` 全图解码：5 MiB 压缩数据可展开成 GB 级 RGBA，与 #133 记录的解码内存峰值同因，并引入解压炸弹面；`Limits` 又会误拒合法大图。
- 新增 `CoverFormatProbe` Port 并把实现放进 `infrastructure/metadata/`：该探测无 I/O、无第二种合理实现、无测试替身需求，§3.2 明确禁止「只为包裹一次函数调用而增加一层接口」；还会牵动 fakes 与装配层，且与 `ControlPlanePort` 15 方法基线冲突。
- 复用/统一 `cover_protocol::cover_media_type`：它是读取侧 `Content-Type` 启发式，且位于平台壳；让 Core 依赖壳会反转依赖方向，改变其「未知字节仍以 octet-stream 提供」的既有语义。

### D3 分层、依赖方向与规范条款（CODE_STANDARDS §9）

| 项 | 结论 |
|---|---|
| 所属分层 | Core Application，`crates/echo-core/src/application/artwork.rs` 的纯输入守卫；Domain 模型与 Infrastructure 适配器不改 |
| 依赖方向 | Platform（Tauri command）→ Application（两个封面用例）→ Domain（`PortableAssetReference`）；Infrastructure 仍只通过既有 `ControlPlanePort::durability()` 与 `CoverCache` port 接入，Application **不引用** `crate::infrastructure`，`crates/echo-core/tests/arch.rs` 保持通过 |
| 复用抽象 / 设计模式 | 复用既有 Ports and Adapters 边界；不引入新设计模式、trait、状态机或全局状态。校验是对 `&[u8]` 的纯函数式 guard clause，与同文件既有的 `blake3` 哈希同属无外部 I/O 的字节级判定 |
| `image` 的位置 | `image` 是 Core 清单中已有的纯 Rust 编解码库，不是数据库/文件系统/网络/UI 适配器；此处只用其内存中的格式与头部解析，像素解码仍留在 `infrastructure/metadata/cover.rs`。不新增跨 crate 或跨层依赖，`Cargo.toml` / `Cargo.lock` / `deny.toml` 零变化 |
| 适用规范 | §2（注释解释约束、不 panic / unwrap / 静默失败、结构化错误）、§3.1（向内依赖）、§3.2（最小抽象）、§4.1（fmt / Clippy / 无 `unsafe`）、§4.2（可匹配的类型化 `Error`）、§8（测试就近、固定样本、跨平台字节逻辑）、§9（规格复核、严格校验、残留清理） |
| 兼容性影响 | 无 schema / 目录格式 / IPC / 生成物变化；`resolve` 对历史上已落盘的无效原件由「破损图或 404」改为明确 `Validation` 错误，原件、哈希与对象记录一律保留；`resolve_legacy` 与扫描内嵌封面不受影响 |
| 待清理残留 | 无被替代实现、无无消费者导出、无失效测试替身、无失效登记资产；`infrastructure/metadata/cover.rs` 中「非图片字节写入缓存并按原样读回」的既有测试**必须保留**，它不是本 change 的残留 |

**实现前 MUST 基于当前 HEAD 重跑（不得把本 design 的快照当施工前提）**：

```bash
rg -n 'pub fn validate|pub fn save|pub fn resolve|write_asset|cache.put' crates/echo-core/src/application/artwork.rs
rg -n 'artwork::(save|resolve|validate)' crates/echo-core/src -g '*.rs'
cargo tree -p echo-core -e features -i image
rg -n -A2 '^name = "image"' Cargo.lock
rg -n 'fn cover_media_type' apps/desktop/src-tauri/src/cover_protocol.rs
node scripts/verify/spec-scenarios.mjs --check
```

若任一结果与本 design 或 `proposal.md` 的事实校核不符，先修正规格与 design 再编码。

### D4 拒绝边界与写入顺序

`save`：体积/白名单/内容校验全部通过后才取 `durability()` 并 `write_asset()`；任一失败都不会产生 `PortableAssetReference`，因此 `set_playlist_cover` / `set_artist_cover` 也不会提交引用该资源的对象记录，已保存封面与用户输入不变（现有服务层回归用例要覆盖这一点）。

`resolve`：`asset.validate()` 与 `read_asset()` 保持原顺序，`validate(&bytes, &asset.mime)` 失败则直接返回，不再做 BLAKE3/长度比较，也不调用 `cache.put()`；封面缓存因此保持「纯可丢弃层」，不再接收无法解码的原文件。`resolve_legacy` 无字节可校验，保持原样。

### D5 测试与证据形状

- Core 单测就近放在 `artwork.rs` 的 `#[cfg(test)] mod tests`：三种合法格式用 `image` 的编码器在测试内生成极小 PNG / JPEG / WebP 字节（不新增 fixture 文件、不依赖外部工具）；伪造样本用固定字节数组，包括纯文本、ZIP 头、签名+截断头、格式错配（JPEG 声明 PNG）、以及仅含合法 IHDR 的 20000×20000 PNG 头（证明 `no_limits()` 与「不解码像素」）。
- 约束测试枚举三种白名单 MIME，断言 `ImageFormat::from_mime_type(...)` 存在且 `reading_enabled()` 为真；workspace `image` 特性退化时该测试必须失败（与 #134 的同类约束一致）。
- 服务层回归放在 `crates/echo-desktop/src/runtime/services/tests/playlist_cover.rs`：先保存合法封面，再提交伪造封面，断言返回 `Validation` 且已保存封面未被替换；艺人路径至少有一条 Core 层用例证明共用校验。
- 架构证据：`cargo test -p echo-core --all-features --test arch`；生成物零漂移：`cargo run -p echo-desktop --bin echo-generate-ipc` 后 `git diff --exit-code -- apps/desktop/src/ipc/ipc-types.generated.ts`（本 change 不改命令，生成物应无变化）。
- 8 个场景 `LD-R12-S01..S08` 全部映射到上述真实测试名，按项目惯例在验证通过后登记，不得提前登记未落地证据。

## Risks / Trade-offs

- **[容器头合法但像素数据损坏仍可保存]** → 明确写进 `LD-12` 的边界：校验只声称「容器头可解析」，不得宣称整幅可解码；残留失败仍在展示阶段，由独立 issue/change 决定是否升级为带限额的像素探测。
- **[`image` 版本或特性变化导致白名单格式未被启用]** → 锁文件固定 0.25.6；约束测试枚举三种 MIME 的 `reading_enabled()`；Clippy/全量测试与治理门在 CI 中兜底。
- **[默认 `Limits` 误拒合法大图]** → 探测前 `no_limits()`，并保留「超大尺寸头通过、不分配像素缓冲」的回归测试；不得改用 `decode()`。
- **[既有无效原件的读取行为变化]** → 不改变任何磁盘事实，只把失败提前并给出类型化错误；与 `LD-06` 的「损坏原件保留并说明原因」一致；不提供自动清理，避免把用户资料删掉。
- **[Core 与平台壳各有一份 magic 判定]** → 两者用途不同（写入拒绝 vs 读取 Content-Type），不合并；在 design 与 `docs/DESIGN.md` 明确区分，避免后续误以为其中一份可以删除。
- **[`validate` 现在每次 resolve 都会做头部探测]** → 探测只读文件头、无像素分配，且 5 MiB 输入上限已存在；`cover://` 已有缓存与单飞机制，不额外引入新的 IO 或后台任务。

## Migration Plan

无数据迁移、无 schema 变更、无 IPC 或磁盘格式变更。上线即生效：新写入的无效封面会被拒绝；历史上已写入的无效原件在读取时转为明确错误，字节与记录保持原状。回滚方式是回退本次代码提交，磁盘数据不受影响，回滚后恢复原有「展示阶段失败」行为。交付顺序按 `AGENT.md`：独立分支 → 实现与验证 → 同步 delta 到 `openspec/specs/library-durability/spec.md` → `openspec validate --strict` → `openspec archive` → 中文 PR `Closes #132`；未归档不得提 PR。
