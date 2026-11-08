# Proposal

## Why

issue [#133](https://github.com/zsxink/Echo/issues/133) 记录了一个尚未封口的边界：`PlaylistNameDialog.tsx` 选图后的预览仍把**原始尺寸**图片交给 WebView 渲染，一旦预览真正渲染，解码占用的 RGBA 与像素量成正比，而不是与文件大小成正比。change `fix-cover-bytes-memory-churn`（PR #135）消除了 `number[]` 摊平与 blob 句柄泄漏，但其 design 的「残留成本」明确把这条留给后续处置。本 change 收口它：让选图预览使用与 `s=detail` 一致的受控尺寸缩略图，并在同一条路径上修复「打包 WebView 里选图预览根本不显示」的既有缺陷，使预览恢复可见时不会把像素级内存峰值一起带回来。

### 事实校核（与 issue 的偏差，先报告）

1. **PR #135 的任务 7.7 已给出实测结论，当前 HEAD 上并不存在一个仍占主导的预览解码峰值。** 同一张 5,128,363 B / 1350×1350 PNG、同一 480 首资料库，选图增量从 +364 MB 降到 +17 MB，选图后峰值从 438 MB 降到 ≤155 MB（未超过「打开编辑对话框」阶段的 155 MB）。issue 约定「若可忽略，则在 PR 中记录结论并将本 issue 标记为暂不需要」：PR 正文记录了结论，但 issue 至今 OPEN、无标签或评论说明结论。本 change 不声称修复了一个已量化的大峰值，而是把「预览不得解码原图像素」固定成可验证的规格约束。
2. **峰值缺席的原因是另一个缺陷：预览当前不可见。** PR #135 的「已知既有问题」记录了打包 WebView 中 `<img src="blob:...">` 只显示 alt 文本「当前歌单封面」；`apps/desktop/src-tauri/tauri.conf.json` 的 CSP `img-src` 为 `'self' cover: http://cover.localhost data:`，不含 `blob:`。HEAD 的旧实现同样使用 `URL.createObjectURL(file)`，因此该现象早于 #113。一旦放宽 CSP 或改用 data URL 让预览可见，issue 描述的像素级峰值立即回归；**修复可见性与限制解码尺寸必须同批落地**，单独上线任一半都会制造回归。
3. **issue 的归因仍然成立。** `crates/echo-core/src/application/artwork.rs` 的 `validate_content` 已以「整幅解码会把 5 MiB 放大成数百 MiB 像素（与 #133 同类的开销）」为由，明确拒绝在写入校验阶段解码像素（LD-12）。本 change 的整幅解码只发生在桌面壳、只服务于一次性内存预览，MUST 带内存分配上限，不触碰 LD-12 的「只读取容器头」语义。
4. **issue 的两个建议方向都需要修正。** 前端 `createImageBitmap` + `OffscreenCanvas` 仍要先在 WebView 内解码原图（只缩短存活期，不消除峰值），且 WKWebView / WebKitGTK 对 `OffscreenCanvas` 与 `createImageBitmap` 的 resize 选项支持不一致；「先把原图写入临时缓存再按 `?s=detail` 取」会把「未提交选图」引入内容寻址缓存，改变 `fix-cover-bytes-memory-churn` 与 #111 共同列为非目标的缓存语义，还多一次磁盘往返。采纳的是第二条的变体：桌面壳在内存中生成缩略图并返回，不落盘。
5. **`URL.createObjectURL` / `revokeObjectURL` 当前由 `DAS-R23` 的三条场景固定**（`apps/desktop/src/test/setup.ts` 为其提供 stub）。本 change 移除句柄后，这三条场景的**标题与 ID 保持不变**，只更新需求文本与证据映射；既有 572 个场景 ID 不重编号。

## What Changes

1. **选图预览改为受控尺寸缩略图。** 用户选图后，编辑对话框只接收并展示最长边不超过 512 px 的缩略图（与 `cover://` 的 `s=detail` 展示档位一致）；WebView MUST NOT 为预览解码原始尺寸的封面像素，缩略图 MUST NOT 放大原图。
2. **新增桌面壳的预览生成边界**（新 Tauri command）。前端把既有的 base64 原图载荷交给壳层，壳层在受控内存边界内校验体积与格式、按预设分配上限拒绝超大像素图、缩放并编码为 JPEG 后返回缩略图数据。该边界 MUST NOT 写封面缓存、资料库或任何磁盘文件，MUST NOT 改动 `cover://` 协议与内容寻址。
3. **预览资源改为 CSP 已放行的只读展示图像数据**（`data:image/jpeg;...`），移除 `URL.createObjectURL` 与三处 `revokeObjectURL`；**不放宽 CSP、不引入 `blob:`**。这样打包 WebView 的选图预览真正可见（PM-R14 的「提交前预览」不再只剩 alt 文本），且无需修改安全姿态。
4. **失败路径可恢复。** 载荷超限、格式与声明不符、容器头无效或像素分配超限时 MUST 明确说明原因，MUST NOT 为超限载荷分配像素缓冲，并保留名称输入、已保存封面与重新选择入口；预览失败不得改变「设置 / 替换 / 恢复自动 / 取消不修改」的对外语义。
5. **规格**：`desktop-app-shell` 的 `DAS-R23` 以 MODIFIED 更新（临时资源从 blob 句柄改为展示档位图像数据，三条场景 ID 与标题不变）；末尾 ADDED `DAS-R25`（选图预览必须使用受控尺寸缩略图）。
6. **量化验收**：用一张像素量远超展示尺寸的大图，在打包应用里记录 `Echo Web Content` 的 footprint，并与「预览原图」的本地对照比较，把前后数字写入证据文件；GUI 无法运行时记录具体阻塞原因，不编造数字。
7. **场景登记与归档**：新增 5 条场景（572 → 577）、更新 DAS-R23 三条证据映射、同步主规格与三方登记、验证通过后归档；归档与 PR 关联 #133 都在 apply 阶段完成。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `desktop-app-shell`：更新「封面预览的临时资源必须被回收」（`DAS-R23`），新增「封面选图预览必须使用受控尺寸的缩略图」（`DAS-R25`）。

**归属理由**：`DAS-R20` 已把「桌面壳 ↔ WebView 之间的封面字节与显示档位边界」确立在该域，`DAS-R22`/`DAS-R23` 是同一条边界上的载荷与预览资源约束；本 change 是这条边界的延续。进行中的 `align-immersive-player-and-playlist-directory` 在其 delta/tasks 中硬编码了 `PM-R13`/`PM-R14`（`PM-R14` 即「编辑歌单封面必须提供预览与可恢复状态」），本 change **不向 `playlist-management` 追加要求**，避免 Requirement 编号依赖归档顺序、也避免两个 change 同时改同一份 delta。新要求追加在 `desktop-app-shell` 末尾（第 25 条），不移动任何既有 Requirement 与场景 ID。

## Impact

- **接口变更（新增，非 BREAKING）**：新增 Tauri command `preview_cover`（参数 `{ bytes, mime }`，返回 base64 JPEG 字符串）。`CODE_STANDARDS §6` 把 command 名称/参数/返回类型变化视为接口变更：`crates/echo-desktop/src/ipc/generate.rs` 的返回类型清单与 `apps/desktop/src/ipc/ipc-types.generated.ts` **必须重新生成并提交**（与 #113 的「零漂移」不同，这次是真漂移）；`apps/desktop/src/bridge/index.ts` 的 `BridgeCommandArguments` / `IpcCommandResultMap` 断言要求两侧同步，`apps/desktop/e2e/mock-bridge-commands.ts` 与 handler 也要补替身。
- **依赖**：`echo-app` 新增 workspace `image` 直接依赖（`image 0.25` 已在 `Cargo.lock`，不新增 crate 版本与传递依赖，`deny.toml` / `LICENSES.md` 不需整体再生成）。
- **受影响代码**：`apps/desktop/src-tauri/src/cover_preview.rs`（新模块）、`cover_payload.rs`（复用编码长度预检，不复制常量）、`commands.rs`、`bootstrap.rs`（注册）、`main.rs`（模块声明）；前端 `apps/desktop/src/features/playlists/PlaylistNameDialog.tsx`、`apps/desktop/src/bridge/index.ts` 及其测试、`apps/desktop/src/test/setup.ts`（移除 blob stub 的必要性需复核）、e2e 替身；`docs/DESIGN.md` §2.4 补一句。
- **保持不变**：Core 公共 API 与 `artwork::validate` 的 LD-12 语义；`cover://` 档位协议、封面缓存的磁盘布局 / 内容寻址 / GC；5 MB 上限与前后两层校验；`set_playlist_cover` / `set_artist_cover` 的参数、编码与对外语义；CSP 配置；沉浸式播放页 `s=original` 的 origin-clean 像素读取；歌单 / 艺人封面提交链路。
- **与进行中 change 的关系**：`align-immersive-player-and-playlist-directory`（0/28 tasks）也会改 `PlaylistNameDialog.tsx` 与 `PlaylistNameDialog.test.tsx`（PM-R14 的前端断言）。两者**必须串行实施**：一方落地后另一方 rebase，合并时人工对齐封面状态字段名与测试名；不得并行改同一文件。
- **代码规范与事实来源**：`openspec/CODE_STANDARDS.md` §2.1（文件规模）、§3.1/§3.2（分层、依赖倒置、抽象边界）、§4（Rust 门禁）、§6（类型化 IPC 与失败态）、§8/§9（交付门槛、规格复核与迁移说明）；`docs/PRODUCT.md`（本地优先）、`docs/DESIGN.md` §2.4（封面档位协议）、`docs/ROADMAP.md`（一期桌面版已发布，本 change 属质量与验收补强，不改变阶段范围）、`docs/interface-terminology.md` 与 `docs/prototype/echo-desktop-player.html`（编辑对话框的封面预览位）。
- **场景登记**：追加 `DAS-R25-S01..S05` 5 行，`docs/traceability.md` 头部基线改为 148 Requirement / 577 Scenario；`scripts/verify/scenario-commands.mjs`、`scripts/verify/manifest.json`（由生成器重生成）、`docs/acceptance/PRD-matrix.md` 同步；`DAS-R23-S01..S03` 的 evidence 改指向新测试名，ID 集合不变。登记在实现完成、验证通过、归档时统一进行，不提前登记未落地的场景。
- **非目标**：不改 CSP、不引入 `blob:` 到 `img-src`、不改为 Rust 侧原生选图对话框；不把「未提交选图」写进封面缓存；不做前端 `OffscreenCanvas` 缩略图；不新增第三方 crate 版本；不实现艺人封面编辑 UI（无调用方）；不实现预览历史 / 撤销栈；不改封面提交失败后的可恢复语义（PM-R14 由活跃 change 负责）；不在本 change 内升级 LD-12 的写入校验为像素探测。
- **交付**：本阶段只产出规划产物，不修改生产代码。apply 阶段在独立分支实施，中文 commit 与中文 PR（关联 #133），先同步 delta spec、完成归档，再创建 PR；未归档不得提 PR。
