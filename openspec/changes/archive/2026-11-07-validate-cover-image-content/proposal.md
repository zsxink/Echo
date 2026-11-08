# Proposal

## Why

issue [#132](https://github.com/zsxink/Echo/issues/132) 指出：`crates/echo-core/src/application/artwork.rs` 的 `validate` 只检查字节体积与调用方声明的 MIME 白名单，不检查字节是否真是该类型的图片。任意文本、压缩包或构造字节只要声明 `image/png` / `image/jpeg` / `image/webp`，就会经 `artwork::save` 写入资料库内的可校验原件（`write_asset`），并在 `artwork::resolve` 时写进封面缓存；解码失败被推迟到 `cover://` 展示阶段，用户看到的不是保存阶段的明确拒绝，而是破损图或 404。

现在修的理由：#113（已归档 `fix-cover-bytes-memory-churn`）在 tasks 6.2 把这个缺口明确留给了后续 issue；封面写入链现已稳定（base64 载荷、`DAS-R22`），改动可收敛在 Core 的单一校验入口，不需要动 IPC、内容寻址、磁盘布局或前端。

### 事实校核（与 issue 正文的偏差，先报告）

1. **「只检查三件事」实际是两条判断。** 当前 `validate` 只有 `bytes.is_empty() || bytes.len() > 5 * 1024 * 1024` 与 `matches!(mime, "image/jpeg" | "image/png" | "image/webp")`；「声明类型与实际内容一致未校验」是缺口描述，不是第三条检查。
2. **`validate` 不是只在写入时调用。** `save` 与 `resolve` 共用它；`resolve` 在 BLAKE3/长度核对与 `cache.put` 之前调用。因此加固写入的同时也会改变**已保存原件**的读取行为，兼容性影响必须显式登记（见 design「迁移与兼容」）。
3. **桌面壳已有 magic bytes 嗅探，但不是写入校验。** `apps/desktop/src-tauri/src/cover_protocol.rs::cover_media_type` 只用于给 `cover://` 响应挑 `Content-Type`，无法识别时仍以 `application/octet-stream` 返回原字节；它属于平台壳，不承担资料库写入校验。
4. **最小头部探测不需要新增依赖。** `image 0.25.6` 已是 `echo-core` 依赖并启用 `png/jpeg/gif/bmp/tiff/webp` 解码（#134 已落地）；`ImageReader::with_guessed_format()`、`no_limits()`、`into_dimensions()` 在锁定的 0.25.6 中均可用。
5. **内嵌封面扫描链路不走 `artwork::validate`。** `DiskCoverCache::put` 面向扫描解析出的封面，`infrastructure/metadata/cover.rs` 的既有测试甚至会把非图片字节写入缓存并断言原图仍可原样读回。本 change 只约束**用户选择的封面**，不改变内嵌封面与 `mime_of` 降级策略。

## What Changes

- 在 `artwork::validate` 的既有体积与 MIME 白名单校验之后，补「内容真实性 + 声明一致」一层：从字节探测实际容器格式，MUST 与声明的 `image/jpeg` / `image/png` / `image/webp` 一一对应，且容器头 MUST 可解析出有效尺寸；否则以既有 `Validation` 错误在保存阶段明确拒绝。
- 用解码库的**最小头部探测**而非全图 `decode()`：不分配整幅像素缓冲、不受解码 `Limits` 的尺寸上限影响，同时能挡下「只有 magic、头部截断」的伪图片。全图像素级损坏仍属展示阶段问题，作为已知残留明确登记。
- 歌单封面（`set_playlist_cover`）、艺人封面（`set_artist_cover`）两条写入路径与 `resolve` 读取路径继续共用同一个 `validate`；不新增公开 API、Port、IPC 命令、命令参数或返回类型。
- 拒绝 MUST 发生在 `write_asset` 之前：不写目录原文件、不写封面缓存、不替换已保存封面、不提交引用该资源的对象记录，也不覆盖用户已输入的名称。
- 既有无效原件的读取改为「明确的校验错误」而非破损图，但不做迁移、清理、回填或历史修复；`resolve_legacy` 的 cache-only 语义保持不变。
- 保持 5 MiB 上限、MIME 白名单、内容寻址、`cover://` 档位、缩略图生成和缓存 GC 语义不变。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `library-durability`：在末尾新增一条要求，规定用户自定义封面原件写入资料库前必须验证声明类型与实际内容一致、容器头可解析，并规定保存阶段拒绝、既有无效原件读取与历史缓存-only 封面的兼容语义；新增 8 个场景。

**归属理由**：`LD-08 资源原件与实际位置可恢复` 已把用户自定义封面定义为「以可校验原件及对象引用保存到资料库」，本次正是补齐该「可校验」边界；`LD-10` 要求持久化规则留在共享 Core，校验落点也在 Core Application。桌面壳 `DAS-R22` 只要求「后端保留等价的体积上限与类型白名单校验」，本 change 不改变该事实，无需改写其要求；进行中 change `align-immersive-player-and-playlist-directory` 已硬编码引用 `PM-R13`/`PM-R14`，故不向 `playlist-management` 追加要求。新要求追加为第 12 条（`LD-R12`），不移动或重编号任何既有 Requirement / Scenario。

## Impact

- **代码**：`crates/echo-core/src/application/artwork.rs`（校验与就近单元测试）；`crates/echo-desktop/src/runtime/services/playlists.rs` 的错误文档注释；`crates/echo-desktop/src/runtime/services/tests/playlist_cover.rs`（回归测试）；`docs/DESIGN.md` §2.4 补一句写入校验说明。前端、Tauri command、IPC 生成物、SQLite/schema、封面缓存磁盘布局均不改。
- **依赖**：无新增。`image` 已是 `echo-core` 依赖，`Cargo.lock` / `deny.toml` / `LICENSES.md` 应零变化；不启用新解码格式。
- **错误契约**：继续复用 `Error::validation(Subject::Other, "cover", ...)` 与既有 `IpcErrorDto` 映射，仅新增可区分的 reason 文案；错误码、字段与命令签名不变，不属于 CODE_STANDARDS §9 所列的接口/迁移变更。
- **兼容性**：`resolve` 现在会拒绝历史上已写入的无效原件，用户看到明确的「封面原件内容无效」而不是破损图；不删文件、不改对象记录、不自动修复（与 `LD-06` 一致）。仅有 historical cache key、没有目录原件的封面不受影响。
- **验证与登记**：新增 `LD-R12-S01..S08` 共 8 个场景。实现完成、验证通过后按项目惯例同步主规格、登记 `docs/traceability.md`、`scripts/verify/scenario-commands.mjs`、`scripts/verify/manifest.json`、`docs/acceptance/PRD-matrix.md`，再归档。基线 146 Requirement / 564 Scenario → 147 / 572（以 `node scripts/verify/spec-scenarios.mjs` 实际输出为准）。
- **非目标**：不改 5 MiB 上限与白名单语义；不做全图像素解码、不引入 AVIF/HEIC；不改内嵌封面扫描与 `mime_of`；不改 `cover_protocol::cover_media_type` 与 `cover://` 档位；不清理或迁移既有无效原件；不处理 #126（封面缓存 GC 容量）与 #133（预览解码内存）；不新增 UI 组件（沿用既有错误反馈路径）。
- **交付**：按 `AGENT.md` 在独立分支实施，以中文 PR 关联并 `Closes #132`；PR 前必须先同步 delta spec、完成 `openspec archive`。本 change 只做规划，不在 propose 阶段改代码。
