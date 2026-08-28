# Echo 统一错误日志与本地诊断目录约定

> 状态：已批准（2026-08-28，task 1.8）
> 适用范围：Rust Core（`echo-core`）、桌面适配层（`echo-desktop`）、React/TypeScript 渲染层（`apps/desktop`），后续移动端遵循同一语义。

本文档是 Echo 错误日志与本地诊断的**唯一事实来源**。实现落地于：

- `crates/echo-core/src/logging.rs`（`redact_path`、`redact_sensitive`、`DiagnosticMode`、`init_test_logger`）
- `crates/echo-core/src/error.rs`（`Error::to_log`）
- `apps/desktop/src/logging/log.ts`（渲染层日志助手）

## 1. 隐私默认值（核心约束）

Echo 的默认日志**绝不包含**以下内容：

1. **完整绝对路径**：例如 `/Users/someone/Music/song.mp3`、`C:\Users\someone\Music\song.mp3`。
2. **歌词全文**：内嵌歌词或 `.lrc` 侧车内容。
3. **标签字符串**：艺人/专辑/标题之外的解析细节，如编码器字符串、码率字符串、章节、封面 hash 原文。
4. **文件内容**：音频容器字节、ID3 帧数据、封面图片数据或任何媒体二进制内容。

可以出现的是**不透明标识**：文件**名称**、稳定的**短 hash**（非加密，仅用于日志去重/关联）、`song_id`/`library_root_id`/`operation_id` 等 ID、**相对路径**（相对于资料库根，不泄露本机目录布局）。

> 例外只有一个：**显式诊断模式**（`DiagnosticMode::On`）。该模式由桌面 runtime 依据明确的设置项打开，允许附加一份**脱敏路径映射**（desensitized path map）。除此之外任何代码路径都不得绕过该开关直接输出原始路径。开启方式与字段见 §4。

## 2. Rust 侧结构化日志

### 2.1 字段约定

所有 `tracing` 事件使用结构化字段，字段名固定为：

| 字段 | 含义 | 隐私 |
|---|---|---|
| `operation_id` | 一次用例/命令的调用 ID（UUID） | 安全 |
| `song_id` | 歌曲逻辑 ID（UUID） | 安全 |
| `library_root_id` | 资料库根逻辑 ID | 安全 |
| `error.code` | 错误分类（`io`、`unsupported_media`、`validation`…） | 安全 |
| `operation` | 操作描述字符串 | 需经 `Error::to_log` 净化 |
| `location` | 由 `redact_path` 生成的 `文件.ext (短hash)` | 安全（默认） |
| `path` | 原始绝对路径 | **仅诊断模式**可附加 |
| `error.message` | 人类可读错误消息 | 需经净化 |

业务代码**不得**自行构造包含原始路径、歌词、标签或文件内容的日志字符串；必须构造 `Error`（或调用 `redact_path` / `redact_sensitive`）后交给 `Error::to_log`。

### 2.2 `Error::to_log`

`crates/echo-core/src/error.rs` 提供：

```rust
pub fn to_log(&self, diagnostic: DiagnosticMode) -> String
```

- 默认（`DiagnosticMode::Off`）输出形如 `error.code=io operation="metadata_read" location="song.mp3 (a3b1c2d4)"` 的一行，路径不出现。
- 诊断模式下额外附加 `path="<原始绝对路径>"`；这是返回完整路径的唯一出口。
- 该函数从不接收也不输出歌词、标签或文件内容。

## 3. 本地诊断目录约定

- Rust 层保持**平台无关**：不得自行解析 app-data 目录。`echo-core` 只提供目录名常量 `echolog::diagnostics_dir_name()` → `"echo/logs"`。
- **桌面 runtime（`echo-desktop`）拥有解析权限**：把 `echo/logs` 解析到平台 app-data 目录下的实际位置（macOS `~/Library/Application Support/Echo/logs`、Windows `%APPDATA%\echo\logs`、Linux `$XDG_DATA_HOME/echo/logs`），由 task 7.x runtime 落地。
- 目录内容：
  - 应用结构化日志（`tracing`，带轮转与大小上限，例如单文件 ≤ 10 MiB、累计 ≤ 50 MiB、保留最近若干份）；
  - **panic hook 与崩溃转储**（崩溃时写入该目录，便于离线诊断）。
- 目录写入使用与数据库/偏好相同的安全写约定：临时文件 + fsync + 原子替换；写入失败不得影响业务路径。
- 该目录内可以出现**完整路径映射**（诊断模式开启时生成的 desensitized path map），这是全仓库唯一允许存放此类信息的位置。

## 4. 诊断模式（唯一例外）

- 开关：`DiagnosticMode::On/Off`，默认 `Off`。由桌面 runtime 根据明确设置项打开；业务代码**不读取**该开关。
- 开启后仅允许：
  1. `Error::to_log(DiagnosticMode::On)` 的 `path` 字段附带原始绝对路径；
  2. 诊断目录内生成脱敏路径映射文件 `diagnostics/paths.json`（`原始路径 → hash`）。
- 仍然**禁止**：歌词全文、标签字符串、文件内容，即使诊断模式开启。这些内容只导出为 `redact_sensitive` 的短 hash。

## 5. 前端（React/TypeScript）日志

- 渲染层使用 `apps/desktop/src/logging/log.ts` 提供的小助手（`log.info/warn/error`），结构化输出 `level=… operation=… msg=…`，不可打印原生 `console.log` 散落调用。
- 前端**同样不得**记录 IPC 载荷内容（command 参数/返回值、字节块、base64）、完整绝对路径、歌词或标签文本。
- `log.ts` 中的 `redactMessage` 会把类绝对路径的 token 替换为 `文件.ext (短hash)`；结构化字段中也不要塞入原始路径，交给调用方先做 `hash` 化。
- 后续任务（7.2/7.4）再接 Tauri event，把前端事件转发为 Rust 侧结构化日志；当前约定保持不变。

## 6. 测试必须断言的隐私属性

默认测试日志设置必须满足：

1. 不出现**完整绝对路径**（含父目录片段）；
2. 不出现**歌词文本**（含片段）；
3. 不出现**标签字符串**（含码率/编码器等片段）；
4. 不出现**文件内容**子串；
5. 出现**安全字段**：文件名、短 hash、`operation_id`/错误码。

现有断言实现：

- `crates/echo-core/tests/logging.rs`：直接断言 `Error::to_log` 输出，并通过 `echo_core::logging::init_test_logger` 捕获真实 `tracing` JSON 行再断言。
- `apps/desktop/src/logging/privacy.test.ts`：mock `console`，断言渲染层助手不输出路径/载荷。

新增日志改造必须同步扩展对应断言，保持本文件的验收可执行。