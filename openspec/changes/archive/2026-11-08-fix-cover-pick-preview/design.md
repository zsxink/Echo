# Design

## Context

动机、issue #133 的实测结论与偏差报告见 `proposal.md` 的 Why 与「事实校核」，这里只列影响方案选择的现状与约束。

- **现状链路**：`apps/desktop/src/features/playlists/PlaylistNameDialog.tsx` 选图 → `FileReader.readAsDataURL` 得到 `data:<mime>;base64,<payload>` → base64 载荷进 `coverPayload` ref（提交时 `set_playlist_cover`）、`URL.createObjectURL(file)` 得到的 blob URL 进 `coverPreview` state（预览 `<img>`）→ `previewBlobRef` 在重新选图、自动封面、卸载三处 `revokeObjectURL`。`coverIntent` 三态（`untouched` / `replace` / `reset`）驱动渲染，原图字节全程不进 React state（DAS-R22）。
- **CSP 现状**：`apps/desktop/src-tauri/tauri.conf.json` 的 `img-src` 为 `'self' cover: http://cover.localhost data:`，不含 `blob:`；打包 WebView 的选图预览因此只显示 alt 文本（PR #135「已知既有问题」）。`data:` 与 `cover:` 已放行。
- **档位现状**：`docs/DESIGN.md` §2.4 与 DAS-R20 固定 `s=detail` 为 512 px 档，仅用于歌单编辑预览；`crates/echo-core/src/infrastructure/metadata/cover.rs` 的 `scale_to(bytes, max_px)` 是 `pub(crate)`，服务于「已落盘、已校验」的缓存原图，走 `load_from_memory` → Lanczos3 `resize` → `to_rgb8` → JPEG，无像素分配上限，错误语义为 `CorruptMedia` 且带原子落盘前提。
- **LD-12 边界**：`crates/echo-core/src/application/artwork.rs` 的 `validate_content` 只读取容器头，注释明确以「整幅解码会把 5 MiB 放大成数百 MiB 像素（与 #133 同类的开销）」为由拒绝像素解码。本 change 的整幅解码发生在桌面壳、服务于一次性内存预览，不参与写入校验，不改变该边界。
- **桌面壳现状**：`echo-app` 是 thin Tauri shell，已有 `cover_payload.rs`（base64 解码、`6 990 508` 编码长度预检、错误映射）与 `cover_protocol.rs`（档位解析）；`main.rs` 声明模块，`bootstrap.rs` 的 `generate_handler!` 注册命令。`commands.rs` 接近 `CODE_STANDARDS §2.1` 的 1000 行硬上限，新逻辑必须另立模块。
- **生成契约现状**：`crates/echo-desktop/src/ipc/generate.rs` 手写返回类型清单，产出 `apps/desktop/src/ipc/ipc-types.generated.ts` 的 `IpcCommandResultMap`；`apps/desktop/src/bridge/index.ts` 用 `AssertNever` 双向断言参数与返回类型集合相等。#113 是「零漂移」，本 change 因新增命令而**有真漂移**，必须重新生成并提交生成物。
- **测试与登记现状**：`apps/desktop/src/test/setup.ts` 为 jsdom 25 缺失的 `createObjectURL` / `revokeObjectURL` 提供 stub；`DAS-R22-S01..S06`、`DAS-R23-S01..S03`、`DAS-R24-S01..S02` 已登记，`spec = trace = manifest = 572`。jsdom 不解码图片，UI 测试需要命令替身。
- **并行工作**：`align-immersive-player-and-playlist-directory`（0/28）会在 `PlaylistNameDialog.tsx` / `PlaylistNameDialog.test.tsx` 补 PM-R14 断言；`PM-R14` 的 delta 已硬编码在该 change 中。

## Goals / Non-Goals

**Goals:**

- WebView 只为选图预览解码最长边 ≤512 px 的 JPEG 缩略图，不为预览解码原图像素；预览在打包 WebView 中真正可见且不放宽 CSP。
- 预览生成不落盘、不进封面缓存、不改内容寻址与 GC、不改 Core 公共 API、不改 `cover://` 协议。
- 预览失败与超大像素输入都有明确、可恢复的界面状态；`DAS-R22` 的提交载荷与两层校验零回归。
- 用真机量化证明「预览占用不随原图像素量线性增长」，并留下可复核的数字或阻塞说明。

**Non-Goals:**

- 不改 `tauri.conf.json` 的 CSP、不引入 `blob:` 到 `img-src`、不换成 Rust 侧原生选图对话框。
- 不改 LD-12 的「容器头校验」语义，不把写入校验升级为像素探测；不改 `set_playlist_cover` / `set_artist_cover` 的参数、编码与语义。
- 不做前端 `createImageBitmap` / `OffscreenCanvas` 缩略图，不把「未提交选图」写进封面缓存。
- 不实现艺人封面编辑 UI（无调用方）、预览历史 / 撤销栈，不新增第三方 crate 版本。
- 不实现 PM-R13 / PM-R14 的其余要求（由活跃 change 负责），不实现封面提交失败后的可恢复语义变更。

## Decisions

### D1. 方向选择：桌面壳内存缩略图

| 方案 | WebView 像素峰值 | 预览可见性 | 缓存 / 安全姿态 | 跨平台风险 | 结论 |
|---|---|---|---|---|---|
| A. 放宽 CSP 加 `blob:`，继续预览原图 | 与像素量成正比 | 修好 | **放宽 CSP** | 无 | 淘汰 |
| B. 前端 `createImageBitmap` + `OffscreenCanvas` / `canvas.toBlob` 生成缩略图 | 仍要解码原图一次，只缩短存活期 | 需换 `data:` 或加 `blob:` | 不变 | WKWebView / WebKitGTK 对 `OffscreenCanvas` 与 resize 选项支持不一致 | 淘汰 |
| C. 原图写临时缓存后按 `?s=detail` 取 | 无（512 px） | 修好 | **未提交选图进入内容寻址缓存**，改 GC 语义，多一次磁盘往返 | 无 | 淘汰（issue 方向 2） |
| **D. 桌面壳在内存中生成 512 px 缩略图并返回** | **只解码 512 px JPEG** | `data:` 已放行，修好 | **不落盘、不放宽 CSP** | 解码在 Rust 侧，行为三端一致 | **采纳** |

D 是 issue 方向 2 去掉磁盘往返的变体：把「按展示档位提供受控图像」（DAS-R20 既有能力）用在尚未提交的用户输入上，且只在内存里做一次。B 之所以不采纳，是因为 issue 描述的是**峰值**而不是存活期：浏览器必须先解码原图才能缩放，峰值不会消失；而且项目要在 macOS WKWebView、Windows WebView2、Linux WebKitGTK 三端保持同一行为，前端 Canvas API 的差异无法用一条 vitest 断言覆盖。

### D2. 分层与模块归属

```
React UI（PlaylistNameDialog）
  → Tauri command（preview_cover，base64 原图载荷）
  → echo-app 壳：cover_payload::长度预检 → cover_preview::{容器头校验 → 限额解码 → 512px 缩放 → JPEG → base64}
  ← 返回 base64 JPEG → <img src="data:image/jpeg;base64,…">
```

- 预览缩略图生成放在 **Presentation / Platform 层的 `echo-app`**，新模块 `apps/desktop/src-tauri/src/cover_preview.rs`，与同层的 `cover_protocol.rs`（档位解析）、`cover_payload.rs`（边界解码）内聚；依赖方向仍是 WebView → Tauri command → 平台壳，平台壳不反向依赖 UI，Core 完全不参与（§3.1、`AGENT.md` 核心约束「Core 与 UI/播放器无关」）。
- **不提升 `scale_to` 为 Core 公共 API**：预览 512 px 是桌面展示档位概念，属于 `cover://` 档位协议所在层；把它放进 `echo-core::application` 会让 Core 承担「不可信用户输入的内存限额 + 一次性预览」，并触发 §9 对 Core 公共 API 变更的兼容与契约测试要求。`scale_to` 本身也是 `echo-core` infrastructure 的实现细节，让平台壳跨 crate 调用它会形成「上层依赖下层具体实现」的反向依赖（§3.1/§3.2）。
- **接受约 20 行相似流水线**：`scale_to` 的输入是已落盘、已校验、可重试的缓存原图，错误语义是 `CorruptMedia` + 原子落盘；`cover_preview` 的输入是不可信、一次性、无磁盘副作用的载荷，错误语义是 `validation`。两者只有 `load → resize → to_rgb8 → JPEG` 四步相似，输入信任级别与错误契约不同，不构成稳定复用点；为单一次调用新增 trait / Port 违反 §3.2「不要只为包裹一次函数调用而增加一层接口」。漂移风险由 D6 的档位一致性约束测试兜住，若将来出现第三个调用方再提取共享纯函数。
- 采用 §3.3 的 **Command 模式**：预览是「请求 → 一次性响应」，不是状态变化，因此用 Tauri command 而不是 event；不新增 Service / Manager 抽象。
- CPU 与内存密集的解码 MUST 经受控阻塞任务执行，不在 tokio 异步线程上直接跑（§4.3）；一次性预览不缓存，因此不需要 DAS-R21 的单飞闸门。实现为 `async fn preview_cover` + `tokio::task::spawn_blocking`，任务失败也映射成可识别的验证错误。
- **命令函数本身也放在 `cover_preview.rs`**（而非 `commands.rs`）：1.5 实测 `commands.rs` 已 990 行，新增命令会越过 §2.1 的 1000 行硬上限。`bootstrap.rs` 以 `cover_preview::preview_cover` 注册，`main.rs` 声明模块，`commands.rs` 零改动；这与「接近上限时先拆分再扩展」一致，也避免把一次新增命令升级成 `commands.rs` 重构。

### D3. 新命令与载荷复用

- `preview_cover(bytes: String, mime: String) -> String`（返回 base64 JPEG）。前端已由 `readAsDataURL` 持有同一份 base64 原图载荷：预览与提交**复用这份字符串**，不新增文件读取、不构造 `Uint8Array` / `number[]`（DAS-R22）。
- 参数**不使用 `Option<String>`**：#113 的 D5 已经证明 Tauri 对 `Option<T>` 的缺键会走 `visit_none()`；`decode_cover_bytes(None)` 在封面命令里表示「清除封面」，把该语义带进预览命令会让缺键变成「生成空预览」。预览命令的 `bytes` / `mime` 必填，缺键或空串在参数反序列化 / 解码阶段就报验证错误。
- 复用 `cover_payload` 暴露的编码长度上界 `6 990 508` 与 `Error::validation(...)` → `IpcErrorDto::from_mutation` 错误链，**不复制常量、不新增错误码种类**；新增的是「必填载荷」变体，与「可缺省 = 清除」变体共用一个解码实现。
- 返回单个 `String` 而不是 DTO：输出格式固定 JPEG（与 `cover://` 缩略图一致，DAS-R20），无需携带 MIME；生成契约、`BridgeCommandArguments` 与 `IpcCommandResultMap` 的增量最小。若将来支持多格式输出，再升级为带 `mime` 的 DTO 并另立 change。
- `set_playlist_cover` / `set_artist_cover` 的参数名、类型、返回类型与服务层 `Option<&[u8]>` 签名全部不变。

### D4. 受控解码：先拒绝，再解码

```
base64 长度预检（≤ 6 990 508）
  → 解码为 Vec<u8>（≤ 5 MiB）
  → MIME 白名单 + 容器头格式探测（只读签名与尺寸，不分配像素缓冲）
  → ImageReader 带 Limits::max_alloc 上限解码
  → Lanczos3 resize（最长边 ≤ 512，不放大）
  → to_rgb8 → JPEG → base64
```

- **上限以「解码器内存分配额」为单位**，而不是像素数：直接约束目标峰值，与 #113 D6「解码前按编码长度预检、不为超限载荷分配内存」同风格。1.3 核对的 `image 0.25.6` API：`Limits` 为 `#[non_exhaustive]`，只能用 `Limits::default()` 后修改 `max_alloc` 字段构造；`ImageReader::limits(&mut self, Limits)` 设置限额，`decode()` 在分配像素缓冲前调用 `Limits::reserve(decoder.total_bytes())` 拒绝超额。固定为 `MAX_PREVIEW_PIXELS = 20_000_000`（20 MP，允许 4000×4000 = 16 MP 通过、拒绝 20000×20000 IHDR）与 `MAX_DECODE_ALLOC = 96 MiB`；不使用 `no_limits()`。
- **预算要覆盖整条流水线**：`decode` 的 RGBA、`resize` 的中间缓冲与 `to_rgb8` 的 RGB 缓冲是独立分配，`max_alloc` 只约束解码器；实现任务 MUST 用一张大图实测总峰值后固定上限，并在代码注释里写明预算构成，不以推理代替测量。
- 容器头探测与解码分两步（两次 `ImageReader`）：先 `into_dimensions()` 读取声明尺寸、校验实际格式与声明 MIME 一致（思路与 LD-12 一致但**不调用、不修改** `artwork::validate`），再带 `limits()` 解码；容器头声明超限或截断时在分配像素缓冲之前返回验证错误。
- 尺寸常量与 `s=detail` 档位同源（512 px），不引入第二套展示尺寸；小图不放大，透明原图的 alpha 按既有缩略图行为丢弃（输出 JPEG）。
- 预览生成不写任何文件，不调用 `CoverCache::put` / `thumbnail`，不改变 `cover://` 的按需生成、单飞合并、原子落盘与内容寻址语义。

### D5. 预览资源形态与状态

| 现状 | 改后 |
|---|---|
| `URL.createObjectURL(file)` → `coverPreview` state | 壳层返回的 base64 JPEG → `data:image/jpeg;base64,…` 字符串进 `coverPreview` state |
| `previewBlobRef` + 三处 `revokeObjectURL` | 删除；重新选图覆盖 state、自动封面 / 取消 / 卸载清空 state 并回到已保存 / 自动封面 key |
| `coverPayload` ref（原图 base64，提交用） | 不变 |
| `coverIntent` 三态与提交载荷 | 不变 |
| 已保存封面预览 `bridge.assetUrl(key, "detail")` | 不变 |

- `data:` 已在 CSP 放行，因此**不改 `tauri.conf.json`**；打包 WebView 的 `<img>` 能真正渲染，PM-R14 的「提交前预览」不再是 alt 文本。
- 缩略图字符串通常几十 KB，远小于 5 MiB 原图；React state 只保留**当前一张**，不保存历史，也不把原图 data URL 放进 state（DAS-R23 修改后的「临时资源释放」由状态替换与卸载时的状态销毁承担，不再需要句柄回收）。
- **预览失败即该次选图不生效**：不写入 `coverIntent = "replace"`，不把用户看不见的图提交上去；界面给出「图片过大或无法读取封面图片，请换一张图片」的中文提示，保留名称输入、已保存封面与先前一次成功的选图，重新选择与取消入口可用。这不新增提交阶段的拒绝条件——提交阶段仍按既有两层校验运行，只是没有新的封面载荷（见 D7）。
- **异步响应不会串台**：`previewRequestRef` 单调递增，只有最新一次选图的 `preview_cover` 响应可以写 state；「自动封面」同样使其失效。卸载后 state 随组件销毁，不再持有任何预览资源（`CODE_STANDARDS §6` 的过期响应处理）。
- 取消、恢复自动封面与关闭对话框 MUST NOT 修改已保存封面；PM-R14-S02 的断言由活跃 change 负责，本 change 保持行为并补本域自己的回归断言。

### D6. 档位一致性与错误映射

- 用同一合法 PNG fixture（`tests/cover_fixtures.rs` 已有）分别生成预览缩略图与 `cover://` detail 档缩略图，断言输出都是 JPEG、最长边 ≤512、缩放后尺寸一致；这条约束测试专门防止 D2 中两套流水线漂移。
- 所有拒绝（长度超限、MIME 不符、格式错配、容器头无效、像素分配超限、解码失败）统一走 `Error::validation(ValidationSubject::Other, "cover", …)` → `IpcErrorDto::from_mutation` → 前端既有 `code` / `message_key` / `field` 展示路径，不 panic、不静默成功、不新增错误码族。
- `preview_cover` 调用本身失败（替身 / 旧后端 / 传输错误）时，前端按 D5 的失败态处理：明确提示、保留名称与已保存封面、不阻断其余表单操作。

### D7. 规格归属、场景编号与登记

- 与 `proposal.md` 的 Capabilities 一致：`desktop-app-shell` 的 `DAS-R23` 用 MODIFIED 整体替换（**标题、三条场景标题与 ID 全部不变**，只更新需求文本与证据映射），末尾 ADDED `DAS-R25`（5 条场景）。`DAS-R25` 是第 25 条 Requirement，不移动任何既有 ID；`147 → 148` Requirement、`572 → 577` Scenario。
- 不向 `playlist-management` 追加要求，避免与活跃 change 硬编码的 `PM-R13` / `PM-R14` 编号冲突；代码层面仍与活跃 change **串行实施**（一方落地后另一方 rebase，合并时人工对齐封面状态字段与测试名）。
- early-sync：实现完成、验证通过后把 delta 逐字同步进 `openspec/specs/desktop-app-shell/spec.md`（与 #113 的 7.1 一致，归档按幂等处理），再更新 `docs/traceability.md`、`scripts/verify/scenario-commands.mjs`（`DAS-R23-S01..S03` 的 evidence 改指向新测试名）、`scripts/verify/manifest.json`（由生成器重生成）、`docs/acceptance/PRD-matrix.md`。**不提前登记未落地的场景**。

### D8. 量化验收

- 同一张 ≤5 MiB、像素量远超展示尺寸（例如 4000×4000）的 PNG，在打包应用里用 `footprint -p <Echo Web Content pid>` 记录选图前 / 选图后峰值，写入 `/tmp/echo-cover-preview-after.txt`。
- 对照组：在本地临时测量分支把预览 `<img>` 指向原图 data URL（**不提交该改动**），同法记录 `/tmp/echo-cover-preview-before.txt`。两者差值用来证明「若预览原图会出现与像素量成正比的峰值，而修复后不会」。
- 验收断言：修复后预览可见（不显示 alt 文本）、选图增量不随像素量线性增长、主进程峰值落在 D4 的预算内。

### D8 实测结果（PR #143 review follow-up）

原 6.1 的「GUI 环境阻塞」出口已由真实测量取代。两轮都重新构建了 `Echo.app`，在同一 480 首资料库、同一歌单、同一张 `/tmp/echo-cover-4000.png`（4000×4000 / 16 MP / 56,311 B）下走完全相同的流程（选一次图 → 「自动封面」清除 → 稳定 6 s → 记录基线 → 再选同一张 → 稳定 5 s → 记录），对话框全程保持打开；命令为 `footprint -p <Echo Web Content pid> -f bytes --noCategories`。

| 构建 | 稳定选图前 | 选图后 | 增量 | `phys_footprint_peak` |
|---|---|---|---|---|
| 修复后（512 px JPEG 预览，pid 20280） | 94,487,560 B | 95,585,312 B | **+1,097,752 B（+1.05 MiB）** | 164,856,768 B（前后不变） |
| 对照（WebView 直接渲染原图 data URL，pid 21607） | 110,806,096 B | 131,351,632 B | **+20,545,536 B（+19.59 MiB）** | 178,799,696 B（前后不变） |

差值 19,447,784 B（18.54 MiB，约 18.7 倍）。对照改动从未提交，测后 `git checkout --` 恢复并重建正确 release 包与 dist。打包 WebView 截图确认预览显示 512 px 缩略图。绝对基线在不同应用会话间不可比（缓存与封面加载时机不同），因此只比较**同一会话内的增量**；这是单次真机测量，不是统计平均。原始命令、流程与口径说明见 `/tmp/echo-cover-preview-{after,before}.txt`。

## Risks / Trade-offs

- **[整幅解码从 WebView 搬到 Rust 主进程，峰值只是换了进程]** → D4 的分配上限 + 一次性无缓存 + 受控阻塞任务；D8 同时记录渲染进程与主进程数字，不把「WebView 下降」冒充「总量下降」。
- **[与 `scale_to` 的相似流水线可能漂移]** → D6 的档位一致性约束测试；出现第三个调用方时再提取共享纯函数，本 change 不为此新增抽象。
- **[新增 IPC command 与生成契约漂移]** → 生成器、生成物、漂移测试同批提交；旧前端不会调用新命令（无影响），新前端配旧后端时按 D6 降级为明确提示，不静默空白、不阻断表单。
- **[预览失败即选图不生效可能让用户困惑]** → 中文提示说明「图片过大 / 无法读取，请换一张」，保留名称与已保存封面、重新选择入口（D5）；不新增提交阶段拒绝条件。
- **[与活跃 change 同文件冲突]** → 明确串行实施、rebase、人工对齐字段名与测试名（#113 D7 的既有教训）。
- **[data URL 字符串进 React state]** → 只有 512 px JPEG、只保留当前一张、不保存原图 data URL；量化任务实测其量级。
- **[`image 0.25` 的 `Limits` API 与分配语义]** → 1.3 已核对 `image 0.25.6` 的 `Limits` / `limits()` / `reserve()` 语义并固定 20 MP + 96 MiB 两级上限（D4）；`no_limits()` 在本路径未使用（那正是 LD-12 为避免像素峰值而采用的反向设置）。D8 实测确认两轮选图都未推高生命周期 peak；若需扩展图片格式或调整预算另立变更。
- **[jsdom 25 的 blob stub 仍可能被其它测试依赖]** → 1.6 已用 `rg` 复核：消费者只有 `apps/desktop/src/test/setup.ts`、`PlaylistNameDialog.tsx` 与 `PlaylistNameDialog.test.tsx` 三处；本 change 改完后三处都不再使用 blob URL，因此**移除 setup 级 stub**，并由新 UI 断言证明不再调用 `createObjectURL`（见 tasks 4.3 / 4.4）。
- **[预览可见后 `artworkPalette` 等像素读取路径]** → 选图预览不做主色提取；沉浸式播放页仍按 DAS-R20 使用 `s=original`，不受影响。

## 兼容与迁移

**变更面**：新增 Tauri command `preview_cover`（参数 `{ bytes, mime }`，返回 base64 JPEG 字符串）与其在 `IpcCommandResultMap` 中的生成类型；`echo-app` 新增 workspace `image` 直接依赖；前端预览资源形态由 blob URL 改为 data URL；`DAS-R23` / `DAS-R25` 规格文本与 5 条场景登记。

**不涉及的兼容面**：Core 公共 API、SQLite schema / 迁移、远端协议、UUID / 哈希 / 导入恢复语义、封面缓存磁盘布局与内容寻址、`cover://` 档位协议、CSP 配置、5 MiB 上限与两层校验、`set_playlist_cover` / `set_artist_cover` 的参数与语义。因此**没有数据迁移与回填**。

**发布与版本 skew**：桌面前端与 Rust 二进制同仓同版本发布，不存在跨版本消费者。新增命令是纯增量：旧前端不调用它；新前端遇旧后端（命令不存在）按 D6 降级为「预览不可用」提示且不阻断表单，不出现静默成功或空白封面。

**回滚**：整体回退本次提交即可恢复 blob 预览路径；生成物随同一提交回退。不涉及持久化格式修复。若已归档，需按 OpenSpec 流程同步回退主规格与场景登记，不允许只回退代码而留下未实现的场景（§9）。

**归档顺序**：实现完成后先把 delta 逐字同步进主规格并验证 → `openspec validate --strict` → `openspec archive` → 中文 PR 关联 #133；未归档不得提 PR（`AGENT.md`）。

## Open Questions

无。方向、分层、命令形态、上限策略、资源形态、错误映射、规格归属与验收方法均已定。D4 的**具体分配上限数值**由实现任务在核对 `image 0.25` API 与大图实测后固定，它不改变规格约束、方案选择或任务拆分，属于可延后的实现细节。

## 复核收尾（PR #143 review follow-up）

PR 合并前的一次独立复核列出 F1–F5；以下修订全部落在**同一交付**（分支 `fix/issue-133-cover-pick-preview`、issue #133、PR #143），不产生新的 spec delta：规格要求没有变化，F2 是让实现满足既有规格而不是降低规格，主规格与场景集合仍为 577。

- **F1（核心修复缺真实 WebView 实证）**：`apps/desktop/e2e/run-e2e.mjs` 新增 #133 场景——Node 侧读取 `tauri.conf.json` 断言 `img-src` 放行 `data:` 且不放宽 `blob:`；浏览器侧打开编辑歌单对话框、用 `DataTransfer` 选图，断言 `<img>` 的 src 是 `data:image/jpeg;base64,…` 且 `naturalWidth > 0`（真实 Chromium 解码），并断言 `state.coverPreviewPayloads[0].bytes` 是字符串而非数组（该字段此前只写不读）。D8 的 `footprint` 真机量化随后也已完成（见「D8 实测结果」），「预览可见」既有 Chromium e2e 证据，也有打包 WebView 截图与选图流程实证。
- **F2（失败原因不区分）**：`IpcErrorDto` 的 `code` 恒为 `validation`、`field` 恒为 `cover`、结构不变；`cover_preview.rs` 只把 `message_key` 细分为 `previewTooLarge` / `previewUnsupported` / `previewDecodeFailed` / `previewUnavailable`，前端 `previewErrorMessage` 按 key 显示四条不同中文提示（旧后端无该命令时走「预览暂时不可用」兜底）。`keeps the saved cover and name when preview generation fails` 参数化覆盖四类提示，后端另有 `preview_errors_carry_distinguishable_message_keys` 固定分类。满足 `DAS-R25-S03` 的「明确说明图片过大」与 `S04` 的「说明失败原因」，不新增错误码族、不改生成契约。
- **F3（白名单复制）**：`echo-core::application` 新增 `pub const SUPPORTED_COVER_MIMES` 并 re-export；`artwork::validate`（LD-12）与 `cover_preview::declared_format` 共用同一常量，UI 的 `<input accept>` 由新增前端断言锁定。`preview_whitelist_matches_core_validation` 固定接受集与拒绝集。**兼容说明（§9）**：只新增一个编译期公共常量，`validate` 行为逐字不变；无 Core API 语义变化、无 schema / 迁移 / 远端协议 / 持久化格式变化；桌面壳仅引用常量，不调用 Core 逻辑，分层与依赖方向不变。
- **F4（边界与 ignored 用例）**：新增 `pixel_budget_accepts_the_boundary_and_rejects_one_pixel_more`（4472² 通过、4473² 拒绝）、`decode_limits_reject_oversized_allocation`（跳过头预检时 96 MiB 二级上限仍拒绝）、`preview_whitelist_matches_core_validation`、`preview_errors_carry_distinguishable_message_keys`；常规大图用例提升到 2.4 MP。16 MP 用例保留 `#[ignore]`，注释与任务清单明确其为**人工峰值证据、不进 CI**（debug 用例约 9 s、进程 RSS 远高于 release）。
- **F5（风格与记录）**：`tokio::task::spawn_blocking` 改为同层通用的 `tauri::async_runtime::spawn_blocking`；D4 预算注释改为「仅解码路径量级估算，不含构建/测试 harness 开销」，不再与 6.1 的 debug 进程 RSS 口径混淆。补记取舍：选图与提交各传输一次约 7 MB base64（预览 + 保存），换取 WebView 完全不解码原图像素。复核中提到的「预览失败保留先前选图」经逐条对照 `DAS-R25-S04` 与 `DAS-R23` 的 **THEN** 确认不构成偏差：规格要求的是不替换已保存封面、保留名称与重选/取消入口，实现全部满足，且保留用户已选的图更不容易丢失输入，故不改行为、不改规格。
