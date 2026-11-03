# Design

## Context

见 `proposal.md` 的 Why 与「事实校核」——动机与偏差报告在那里，这里只列影响方案选择的约束与现状。

- **现状链路**：`PlaylistNameDialog.tsx` 读文件 → `Array.from(bytes)` 摊平成 `number[]` → `useState` → `bridge.call("set_playlist_cover", { id, bytes, mime })` → Tauri JSON 命名参数通道 → `commands.rs` 的 `bytes: Option<Vec<u8>>` → `AppServices::set_playlist_cover(id, Option<&[u8]>, Option<&str>)` → `echo_core::application::playlist::cover::set_playlist_cover` → `artwork::save`（校验 + `durable.write_asset`）。
- **既有契约已经是字节导向的**：`echo-desktop` 的服务层与 `echo-core` 全程只收 `Option<&[u8]>` / `&[u8]`，编码不是它们的概念。
- **既有校验有两层**：前端 `file.size > 5 * 1024 * 1024` 与 `<input accept>`；后端 `artwork::validate` 的空字节、5 MiB 上限与 JPEG/PNG/WebP 白名单。
- **规模约束**：`apps/desktop/src-tauri/src/commands.rs` 已 986 行，而 `scripts/verify/check-scale.mjs` 对源文件设 1000 行硬上限——新的解码逻辑不能再堆进该文件。
- **依赖现状**：`Cargo.lock` 已含 `base64 0.22.1`（现有传递依赖），`deny.toml` 的许可证允许列表已覆盖 MIT/Apache-2.0。
- **规格现状**：`DAS-R20` 已把「桌面壳 ↔ WebView 的封面字节边界」归属在 `desktop-app-shell`；`playlist-management` 的 `PM-R14` 编号已被进行中的 change 硬编码引用。
- **生成契约现状**：`crates/echo-desktop/src/ipc/generate.rs` 只产出返回类型映射；这两个命令的参数类型是手写的 `apps/desktop/src/bridge/index.ts` 的 `BridgeCommandArguments`。

## Goals / Non-Goals

**Goals:**

- 封面字节不再以逐字节数字数组形态存在于 WebView 侧的任何环节（state、IPC 载荷、序列化文本）。
- 选图产生的 blob URL 有确定的回收时机。
- 编码变更只落在**边界层**：`echo-core` 与 `crates/echo-desktop` 的封面写入路径零改动。
- 既有封面语义（设置/替换/恢复自动/不修改已保存封面）与两层校验零回归。

**Non-Goals:**

- 不引入新的图片内容真实性校验（当前 `artwork::validate` 只查体积与 MIME 白名单，非图片内容也能落盘——这是既有缺口，本 change 不扩大也不修补，另立 issue）。
- 不改封面缓存的磁盘布局、内容寻址与 GC 语义。
- 不改交互层（仍用 `<input type="file">`，不换 Rust 侧原生对话框）。
- 不消除 blob 预览对原图的解码开销（见「残留成本」）。

## Decisions

### D1. 载荷编码选 base64 字符串

| 方案 | WebView 侧峰值 | 交互层 | 改动面 | 结论 |
|---|---|---|---|---|
| 现状 `number[]` | 60–80 MB | 不变 | — | 淘汰 |
| raw payload（`Uint8Array`） | ≈10 MB | 不变 | 大 | 淘汰，见下 |
| Rust 侧读路径 | ≈0 | **变更** | 大 | 淘汰，见下 |
| **base64 字符串** | **≈14 MB** | 不变 | 小 | **采纳** |

**为什么是 base64。** 它是唯一同时满足「不动交互层」「不改 Core 与 echo-desktop」「峰值降一个数量级」的方案。5 MiB 原始字节的编码长度为 `4 × ⌈5 242 880 / 3⌉ = 6 990 508` 字符；作为 Latin-1 字符串在 V8 中约占 7.0 MB，Tauri 序列化时再复制一份 ≈7.0 MB，WebView 侧峰值 ≈14 MB。现状是 `number[]` 的 `5 242 880 × 8 B ≈ 41.9 MB` 加 JSON 数组文本 ≈20.9 MB，峰值 ≈62.8 MB。

**为什么不用 raw payload。** Tauri 的 raw body 通道（`tauri::ipc::Request` + `InvokeBody::Raw`）一旦启用，**命名参数通道同时失效**——`id` 与 `mime` 无处安放。要么自造二进制信封（前缀 + id + mime + 载荷），要么拆成 `upload_cover(raw) → handle` 两步并引入会话态缓冲。前者违反 `CODE_STANDARDS §6`「Tauri commands 使用集中定义的类型化接口」；后者让 `bridge.call` 出现第二种载荷形态（其余 80 个命令全部是对象载荷），把一次编码修正升级成 IPC 形态重构。

**为什么不用 Rust 侧读路径。** 它是 issue 正文给出的首选，但落地代价被低估了三处：(1) `<input type="file">` 在 WKWebView 拿不到真实路径，必须把选图交互换成 Rust 侧原生对话框——这是 UX 变更，不是一个 issue 里的一句「实现提示」；(2) 前端要预览就必须拿到可渲染的东西，于是「待保存的选图」需要一个落盘位置，最自然的是封面缓存，这会把缓存从「纯可丢弃层」扩展成「承载未提交选择」，与 issue 非目标「不改动封面缓存的语义」擦边；(3) `set_artist_cover` 同理需要第二个句柄通道。三处叠加后的改动面与回归面显著大于 base64，而 base64 已能把峰值降到「与文件大小同量级」——正是 issue 目标的表述。

**同时被否决的中间形态**：`JSON.stringify(new Uint8Array(...))` 会序列化成 `{"0":1,"1":2,…}`，比 `number[]` 更糟，不能作为「Uint8Array 直传」的替代。

### D2. 编码只在 Tauri command 边界解码

解码放在 `apps/desktop/src-tauri/src/cover_payload.rs`（新增模块，`main.rs` 声明 `mod cover_payload;`），由 `commands.rs` 调用：

```
前端 base64 字符串
  → commands.rs: bytes: Option<String>
  → cover_payload::decode_cover_bytes(Option<&str>) -> Result<Option<Vec<u8>>, IpcErrorDto>
  → services.set_playlist_cover(id, Option<&[u8]>, Option<&str>)   ← 既有签名，不改
```

**理由**：base64 是**线上格式**，属于拥有该格式的那一层。`crates/echo-desktop` 按 `CODE_STANDARDS §3` 不依赖 Tauri，把它放进 `echo-desktop` 会让一个 Tauri-free 的 crate 承担 wire format 概念；放进 `echo-core` 更进一步违反「Core 与 UI/框架无关」。反过来，解码放在 `echo-app` 之外会给只服务单一调用点的代码增加跨 crate 公共 API，违反 `§3.2`「不要只为包裹一次函数调用而增加一层接口」。既有服务签名 `Option<&[u8]>` 已经就位，说明 Core 层从一开始就是字节导向的——本次不需要动它。

**为什么单独成文件**：`commands.rs` 已 986 行，硬上限 1000 行；解码函数与其单元测试必须另立模块。

**错误映射**：解码失败与超限都用 `echo_core::error::Error::validation(ValidationSubject::Other, "cover", …)` 构造（注意 `ValidationSubject`，不是 privacy 模块的 `Subject`），经既有的 `IpcErrorDto::from_mutation` 转成 `code` / `message_key` / `field` 三元组，前端沿用既有的错误展示路径，不新增错误种类。

### D3. 前端用 `FileReader.readAsDataURL` 取载荷

`readAsDataURL` 直接产出 `data:<mime>;base64,<payload>`，前端因此**全程不持有 `Uint8Array`**：既没有 5 MB 的中间数组，也没有 `Array.from` 的 40 MB 摊平数组。

- 载荷取第一个 `,` 之后的子串（base64 字母表 `A–Za–z0–9+/=` 不含 `,`，切分安全）；mime 仍取 `file.type`（与现状一致，不依赖 data URL 的媒体类型推断）。
- 不用 `arrayBuffer()` + 手写 base64：`String.fromCharCode(...bytes)` 在 5 MB 上会爆栈，必须分块，实现面更大且更易错。
- 现有 `reader.onerror` 处理保留。

### D4. 状态分层：意图进 state，字节进 ref，预览句柄单独持有

现状的三态 `coverChange: number[] | null | undefined` 同时承担了两件事——「用户想做什么」（驱动渲染）与「字节是什么」（驱动提交）。拆开：

| 现状 | 改后 | 位置 |
|---|---|---|
| `coverChange === undefined` | `coverIntent === "untouched"` | `useState` |
| `coverChange === null` | `coverIntent === "reset"` | `useState` |
| `coverChange === number[]` | `coverIntent === "replace"` | `useState` |
| `coverChange` 里的字节 | `{ base64, mime }` | `useRef` |
| `coverPreview`（URL 字符串） | 不变 | `useState` |
| —（原本不存在） | 当前存活的 blob URL | `useRef` |

`useRef` **不降低内存量级**（issue 目标 2 的修正，见 proposal 的事实校核）。它的收益是：字节载荷不进入 React 状态快照，不触发与字节无关的重渲染，不被 devtools 的状态面板长期持有。真正消除内存的是 D1 的编码变更。

blob URL 的回收时机定为三处，任一路径都必须走到：

1. 重新选图 —— 先 revoke 上一个 blob，再创建新的；
2. 点击「自动封面」（回到 `reset`）—— revoke 当前 blob，预览切回自动封面；
3. 对话框卸载 —— `useEffect` 的 cleanup 中 revoke。

回收句柄放 `useRef` 而不是从 state 里反查：state 里存的是 `coverPreview`，它可能是封面资产 key（`assetUrl` 路径）或自动封面 key，只有 blob URL 需要 revoke，混在一个字段里容易漏判。

### D5. 参数名保持 `bytes`，只收窄类型

**不**改名。Rust 侧仍是 `bytes: Option<String>`（Tauri 参数名不变），只有类型从 `Option<Vec<u8>>` 收窄为 `Option<String>`。

**为什么不改名（这条在评审中修正过）**：Tauri 的命名参数解包对 `Option<T>` 的缺失键走 `visitor.visit_none()`（`tauri-2.11.6` 的 `src/ipc/command.rs:134-145`：`v.get(self.key)` 为 `None` ⇒ `visit_none()`），即「键不存在」与「键为 null」在命令参数里**不可区分**，两者都是 `None`。而 `None` 在本命令的服务层语义是**清除封面**（`AppServices::set_playlist_cover(id, None, None)` 会写入「恢复自动封面」，现有测试 `playlist_cover` 即断言 `has_custom_cover == false`）。若把参数改名为 `bytesBase64`，旧客户端发 `{ id, bytes: [...] }` 时 `bytesBase64` 键缺失 ⇒ `None` ⇒ **静默清空已保存封面**——这正是本 change 声称不会发生的事。

保留 `bytes` 名、只收窄类型则行为正确：旧载荷 `bytes: [1,2,3]` 在参数反序列化阶段就失败（`invalid type: sequence, expected a string`），错误发生在进入服务层之前，不可能落到清除路径。`null` 与「键缺失」仍然都是 `None`（清除），这与现状语义一致，不是回归。

**参数名不变 ⇒ 前端 bridge 类型也不用改名**，只把 `bytes: number[] | null` 收窄为 `bytes: string | null`，改动面更小、与后端键名一一对应（见 `tauri-ipc-param-camelcase` 的既有教训）。

### D6. 解码前按编码长度预检上限

base64 的解码长度上界是 `⌊len / 4⌋ × 3 − padding`，因此编码长度超过 `4 × ⌈5 242 880 / 3⌉ = 6 990 508` 的载荷**必然**解码出超过 5 MiB 的字节。先按长度拒绝，再解码，可以避免为损坏或恶意的超长载荷分配**解码缓冲**。措辞限定：base64 字符串本身在参数反序列化时已由 serde 分配（这 7 MB 省不掉），本条预检省的是「再一份 5 MiB+ 的字节缓冲」，以及避免把注定被 `artwork::validate` 拒绝的载荷解码一遍。现状的数字数组载荷是在反序列化阶段就已经分配完毕，这条预检是净增益。

### D7. 规格归属与场景编号

新增要求落在 `desktop-app-shell` 末尾（`DAS-R22`、`DAS-R23`），理由见 proposal。追加在末尾意味着**不移动任何既有 Requirement 与场景 ID**，`docs/traceability.md` 与 `scripts/verify/manifest.json` 的既有 550 个场景 ID 全部不变，只需追加 9 行（DAS-R22-S01..S06、DAS-R23-S01..S03）。

与进行中的 `align-immersive-player-and-playlist-directory` 的关系：两者都会改 `PlaylistNameDialog.tsx`。本 change 不向 `playlist-management` 追加要求，因此不与该 change 的 `PM-R13`/`PM-R14` 编号冲突；但**代码层面必须串行**——一方落地后另一方 rebase，合并时封面状态字段名需要人工对齐。

### D8. 生成契约按「零漂移」处理

`ipc/generate.rs` 只产出返回类型映射，本 change 不改任何命令的返回类型，因此 `apps/desktop/src/ipc/ipc-types.generated.ts` 应当零变化。任务里**不安排任何对该生成物的编辑**，只安排「运行生成器 + 运行漂移测试 + 断言 `git diff` 为空」，用证据证明没有漂移，而不是靠改生成物来「让门禁过」。

## 残留成本

**blob 预览仍需解码原图。** `URL.createObjectURL(file)` 指向原始尺寸的图片，渲染进程必须解码整张图才能绘制，RGBA 占用与像素数成正比（3155×3155 约 40 MB，8000×6000 约 190 MB），可能大于本 change 消除掉的数字数组。这条成本**不在本 change 的消除范围内**：#113 的目标是消除「摊平」与「泄漏」，不是消除「预览解码」。

处置：任务里安排一次真机量化（同一张图，改前/改后各测一次渲染进程 footprint），把数字写进证据文件。若量化结果显示预览解码是新的主导项，则另立 issue（把待保存选图交给 Rust 生成 512 px 缩略图再预览），不在本 change 里扩范围。

## Risks / Trade-offs

- **[新错误路径：载荷无法解码]** → 前端只有一条载荷来源（自家 `readAsDataURL`），非法 base64 实际不可达；但解码层仍必须把它映射为可识别错误而不是 panic 或静默成功，由 Rust 单元测试固定该行为。
- **[BREAKING：IPC 参数类型变更]** → 桌面端前后端同仓同版本发布，不存在版本 skew；`set_artist_cover` 无 UI 调用方，无外部消费者。回滚方式是整体回退本次提交（见「兼容与迁移」）。
- **[状态字段重命名导致与进行中 change 的合并冲突]** → 明确要求两者串行实施，合并时人工对齐字段名；本 change 的任务里包含一次与该 change 的对照检查。
- **[base64 相比原始字节多 33% 体积]** → 对 5 MiB 上限而言是 1.67 MB 的额外传输与一次编解码（毫秒级），换来的是消除 40 MB 的摊平数组与 14 MB 的 JSON 数组文本，净收益显著。
- **[引入新的第三方依赖 `base64`]** → `Cargo.lock` 已有 `base64 0.22.1`，不新增传递依赖；`deny.toml` 无需改；`LICENSES.md` 的 crate 集合不变（该文件当前已滞后于锁文件，**不做整体再生成**，避免把无关 churn 混入本 change）。
- **[`readAsDataURL` 的内存表现依赖实现]** → 任务里用真机量化数据固定结论，不以推理代替测量。

## 兼容与迁移

**变更面**：`set_playlist_cover` 与 `set_artist_cover` 的封面字节参数的**类型**（`Option<Vec<u8>>` → `Option<String>`）。参数名、返回类型、错误结构、命令名、封面落盘语义、内容寻址、缓存布局全部不变。

**兼容影响**：无跨版本消费者。桌面端前端与后端随同一构建发布，`apps/desktop` 的构建产物与 Rust 二进制同包。`set_artist_cover` 目前只有契约登记、没有调用方。

**迁移步骤**：

1. 前端把载荷换成 base64 字符串（此时旧后端 `Option<Vec<u8>>` 会以 `invalid type: string, expected a sequence` 拒绝——两端必须同批上线）；
2. 后端把参数类型收窄为 `Option<String>` 并接入解码；
3. 两端同批进入同一构建，不存在中间态发布。

**旧载荷的行为**：若客户端仍发 `number[]`，参数反序列化直接失败（`invalid type: sequence, expected a string`），命令不会执行——**不会**被当作空封面写入，也不会静默成功。这正是保留 `bytes` 名而不改名的目的：改名会让旧键缺失变成 `None`（清除），比拒绝更糟。

**回滚**：整体回退本 change 的提交即可恢复 `number[]` 载荷；因为不涉及持久化格式、schema 迁移或远端协议，回滚不需要数据修复。

## Open Questions

无。载荷编码、规格归属、错误映射、回收时机、依赖选择均已定；剩下的只有实现细节与实测数据，不改变规格、方案与任务拆分。
