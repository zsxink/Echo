# Proposal

## Why

issue [#113](https://github.com/zsxink/Echo/issues/113) 定位的选封面内存问题在当前 HEAD 仍然成立，且未被 #110/#111/#112 覆盖：`PlaylistNameDialog.tsx` 把封面字节 `Array.from()` 摊平成 `number[]` 存进 React state，并经 Tauri 的 JSON 命名参数通道传输——5 MB 上限下单次选图在 WebView 侧产生约 40 MB 的稠密数字数组加 15–20 MB 的 JSON 序列化字符串（峰值约 60–80 MB）；同时 `URL.createObjectURL` 全仓只创建不回收，每次选图在渲染进程泄漏一份 blob。

#111 治的是**日常渲染路径**的图像解码占用，#112 治的是**周期性** churn，两者都不触及选图路径的一次性峰值与永久泄漏。本 change 收敛选图路径本身，不动封面缓存的磁盘布局、内容寻址与 GC 语义，也不动 5 MB 上限与既有封面校验语义。

### 事实校核（与 issue 的偏差，先报告）

1. **「改载荷编码必须重新生成 IPC 契约」在本次改动下不成立。** IPC 生成器 `crates/echo-desktop/src/ipc/generate.rs` 只产出**返回类型**映射（`readonly set_playlist_cover: void;`），不产出这两个命令的**参数类型**；参数类型是手写的 `apps/desktop/src/bridge/index.ts` 里的 `BridgeCommandArguments`。本 change 不改任何命令的返回类型，因此 `apps/desktop/src/ipc/ipc-types.generated.ts` **应当零变化**——但仍必须运行生成器与漂移测试证明无漂移，而不是凭「生成物应当变化」去改它。
2. **issue 正文写的生成物路径 `bridge/` 不存在。** 实际路径是 `apps/desktop/src/ipc/ipc-types.generated.ts`。
3. **issue 目标 2（`coverChange` 不进 React state）不改变内存量级。** React state 持有的是**引用**，渲染不会复制该数组；state 与 ref 的差别只在无关重渲染与意外快照保留。它保留为顺带卫生项，**不作为本 change 的验收依据**。
4. **issue 未计入 blob 预览的解码开销。** `URL.createObjectURL(file)` 指向**原始尺寸**图片，渲染进程仍需解码整张图（RGBA 占用与像素数成正比，可能大于被消除的数字数组）。这条成本本 change **不消除**，只做显式回收；实测与后续处置见 design 的「残留成本」与 tasks 第 7 节的量化任务。
5. **`set_artist_cover` 没有 UI 调用方。** 它与 `set_playlist_cover` 是同一条封面写入链上的孪生命令（同一 `artwork::save` 落盘路径），本 change 一并改编码，避免二次返工；其验证由 Rust 侧命令与解码路径测试承担。

## What Changes

1. **封面载荷改为 base64 编码**（**BREAKING**：IPC 参数类型变更）。`set_playlist_cover` 与 `set_artist_cover` 的封面字节参数从 JSON 数字数组（`number[]`）改为 base64 字符串；Rust 侧在 Tauri command 边界解码为原始字节后，再进入既有的校验与持久化路径。
2. **前端不再构造逐字节数组。** 选图改用 `FileReader.readAsDataURL` 直接取得 base64 载荷，前端全程不持有 `Uint8Array`/`number[]` 形式的封面字节。
3. **封面字节移出 React state。** 「未改动 / 待替换 / 恢复自动」三态语义保留在 state 中驱动渲染，字节载荷改由 `useRef` 持有。
4. **blob URL 显式回收。** 选图产生的 blob URL 在被替换与对话框卸载时 `URL.revokeObjectURL`。
5. **两侧上限与类型白名单保留。** 前端 5 MB 选图上限与后端 5 MiB + JPEG/PNG/WebP 白名单校验都在；base64 解码前按编码长度预检上限，避免为超限载荷分配内存。
6. **载荷损坏可识别。** 非法 base64 载荷返回可识别的输入错误，不覆盖已保存封面。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `desktop-app-shell`：新增「封面字节必须以受控载荷传递」「封面预览的临时资源必须被回收」两条要求。

**归属理由**：

- `desktop-app-shell` 已有的 `DAS-R20`「封面资产协议必须按显示档位提供受控图像」确立了「桌面壳 ↔ WebView 之间的封面字节边界」这一归属；本 change 是同一条边界上的载荷编码约束，落在同一域最连贯。
- 不放在 `playlist-management`：进行中的 `align-immersive-player-and-playlist-directory` 已在其 tasks/design 中**硬编码** `PM-R13`/`PM-R14`（`PM-R14` 即「编辑歌单封面必须提供预览与可恢复状态」）。两个 change 同时向 `playlist-management` 追加要求，会让 Requirement 编号取决于归档顺序，破坏 `PM-R14` 的既有引用。
- 追加在 `desktop-app-shell` **末尾**（新编号 `DAS-R22`、`DAS-R23`），不移动任何既有 Requirement 与场景 ID。

## Impact

- **IPC 契约变化（BREAKING）**：`set_playlist_cover` 与 `set_artist_cover` 的封面字节参数由「JSON 数字数组」改为「base64 字符串」。**参数名保持 `bytes` 不变**：改名会让旧客户端缺失的键被 Tauri 的 `deserialize_option` 映射成 `None`，而 `None` 在本命令的语义是「清除封面」——等于把旧载荷静默当成清空封面。保留 `bytes` 而只把类型收窄为 `Option<String>`，旧 `number[]` 载荷在参数反序列化阶段就失败（`invalid type: sequence, expected a string`），不会走到解码层，更不会走到清除路径。对外语义（设置/替换封面、恢复自动封面、不修改已保存封面）不变。详见 design 的「兼容与迁移」。
- **依赖变化**：`apps/desktop/src-tauri`（`echo-app`）新增 `base64` 直接依赖。`Cargo.lock` 已含 `base64 0.22.1`（现有传递依赖），**不引入新的传递依赖**；`deny.toml` 的许可证允许列表已覆盖 MIT/Apache-2.0，无需新增例外。
- **受影响代码**：`apps/desktop/src/features/playlists/PlaylistNameDialog.tsx`、`apps/desktop/src/bridge/index.ts`、`apps/desktop/src-tauri/src/commands.rs`（新增一个解码模块，不往已达 986 行的 `commands.rs` 继续堆）、`apps/desktop/src/features/playlists/PlaylistNameDialog.test.tsx`、`apps/desktop/e2e/mock-bridge-handlers-library.ts`、`crates/echo-desktop/src/runtime/services/tests/playlist_cover.rs`（只补回归测试）。`crates/echo-core` 与 `crates/echo-desktop` 的封面写入路径**不动**——编码只在 Tauri command 边界解码，Core 继续只见 `&[u8]`。
- **与进行中 change 的关系**：`align-immersive-player-and-playlist-directory` 也会改 `PlaylistNameDialog.tsx`（为 `PM-R14` 补前端断言）。两者**不应并行实施**：本 change 先落地、对方在其之上 rebase，或反之；同一文件的封面状态字段名在合并时需要人工对齐。
- **场景登记**：新增 9 个场景（`DAS-R22-S01..S06`、`DAS-R23-S01..S03`）。登记（`docs/traceability.md`、`scripts/verify/scenario-commands.mjs`、`scripts/verify/manifest.json`、`docs/acceptance/PRD-matrix.md`）在**实现完成、验证通过、归档时**统一进行，不得提前登记未落地的场景。
- **非目标**：不改封面缓存的磁盘布局、内容寻址与 GC 语义；不改 5 MB 上限本身；不把选图交互换成 Rust 侧原生对话框（评估见 design 的「被否决的替代方案」）；不改 `artwork::validate` 的既有校验语义（空字节、超限、MIME 白名单）；不新增图片内容真实性校验（见 tasks 的后续 issue 任务）。
