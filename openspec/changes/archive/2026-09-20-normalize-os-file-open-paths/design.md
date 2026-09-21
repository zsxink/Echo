## Context

Finder 双击音频后无声、进度不走、标题带 `%20`（issue #6）。现象是两条入口的分叉：单实例 argv 支路（`main.rs:657-659`）传的是平台原生路径，macOS 的 `RunEvent::Opened` 支路（`main.rs:869-874`）传的却是 `tauri::Url::to_string()`。下游没有任何一层负责把 URL 归一化成路径，最终被 actor 的纵深防御正确拦下（`actor.rs:1248-1256` 置 `Failed` 且不向 mpv 投递 load），于是**静默无声**。

**本设计的所有事实均在 HEAD `e48ae6e` 上复验**，可在同 HEAD 用文中命令重跑。（issue #6 的调查基准是 `6e4c20c`，其后有 9 个提交；行号有漂移，见「复审核正」表。）

### 实测证据

| # | 事实 | 实测 / 位置（HEAD `e48ae6e`） | 复验命令 |
|---|---|---|---|
| E1 | `RunEvent::Opened` 把 URL 当路径下发 | `main.rs:872` `deliver_file_open(app, url.to_string())`（issue 记 864，已漂移） | `sed -n '865,874p' apps/desktop/src-tauri/src/main.rs` |
| E2 | 接收端原样透传、不做校验 | `runtime/mod.rs:256` `receive_file_open(&self, path: String)`，`:262-267` 仅做「ready 即返回 / 否则入 FIFO」 | `sed -n '244,268p' crates/echo-desktop/src/runtime/mod.rs` |
| E3 | 前端从 URL 末段取显示名 | `App.tsx:89` `path.split(/[\\/]/).filter(Boolean).at(-1)`；`:90` `fireAndForget("play_temporary_file", { path, displayName })` | `sed -n '81,97p' apps/desktop/src/app/App.tsx` |
| E4 | 先按库内歌曲查（永远不命中），再当临时项 | `commands.rs:642-651` `active_song_for_path(Path::new(&path))`；`:652-657` `play_temporary(… on_active_root: false)` | `sed -n '631,660p' apps/desktop/src-tauri/src/commands.rs` |
| E5 | 纵深防御拒绝含 `://` 的路径 | `actor.rs:1270-1272` `fn is_local_media_path(path) -> !path.to_string_lossy().contains("://")` | `sed -n '1263,1272p' crates/echo-desktop/src/player/actor.rs` |
| E6 | 拒绝分支置 `Failed`、publish、**return**（不投递 load） | `actor.rs:1248-1256` | 同上 |
| E7 | 两条入口之一（argv）形态是好的 | `main.rs:657-659` `for path in args.into_iter().skip(1) { deliver_file_open(app, path) }` | `sed -n '651,660p' apps/desktop/src-tauri/src/main.rs` |
| E8 | 载荷语义只写在注释里 | `main.rs:461-465`「the payload is a single absolute path」；`runtime/mod.rs:254-255` 同义 | 上述两处 `sed` 可见 |
| E9 | `tauri::Url` 就是 `url::Url` | `tauri-2.11.5/src/lib.rs:83` `pub use url::Url;`（issue 记的位置**逐字一致**） | `grep -n 'pub use url::Url' ~/.cargo/registry/src/*/tauri-2.11.5/src/lib.rs` |
| E10 | `Opened` 载荷类型是 `Vec<url::Url>`，且仅 macOS/iOS/Android 编译 | `tauri-2.11.5/src/app.rs:263` `Opened { urls: Vec<url::Url> }`，`#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]` | `sed -n '258,266p' ~/.cargo/registry/src/*/tauri-2.11.5/src/app.rs` |
| E11 | `to_file_path()` 存在，同时做 scheme 校验与百分号解码，返回 `Result<PathBuf, ()>` | `url-2.5.8/src/lib.rs:2726` | `grep -n 'pub fn to_file_path' ~/.cargo/registry/src/*/url-2.5.8/src/lib.rs` |
| E12 | 依赖已在 `Cargo.lock`（无需新增） | `tauri 2.11.5`、`url 2.5.8` | `grep -A2 '^name = "\(tauri\|url\)"$' Cargo.lock` |
| E13 | `duration` 只在 mpv 上报时被写；`load_path` 拒绝分支既不清 `duration` 也不清 `position` | `actor.rs:966` `"duration" => self.duration = Some(value)`；字段见 `:730-731`；`build_snapshot` `:859-869` 原样发布 | `sed -n '1241,1260p' crates/echo-desktop/src/player/actor.rs` |
| E14 | 另有**两个**`Failed` 产生点同样不清进度 | `actor.rs:1042-1043`（`LoadLibrarySong` 解析失败）、`:1066-1067`（`LoadLibrarySongPaused` 解析失败） | `sed -n '1035,1077p' crates/echo-desktop/src/player/actor.rs` |
| E15 | 场景命令表是唯一权威源，末行有 `ATTEST` 兜底 | `scenario-commands.mjs` `const command = COMMANDS[id] \|\| ATTEST(id);` | `tail -25 scripts/verify/scenario-commands.mjs` |
| E16 | 5 条相关场景全部指向 `task-9.1.mjs` | `DAS-R06-S01/S02/S03`、`SFI-R07-S01/S02` | 见下方脚本 |
| E17 | `task-9.1.mjs` 的三段验收对载荷形态全不敏感 | `cargo check/clippy -p echo-app`；`cargo test -p echo-desktop --lib runtime::` 的 3 个 FIFO 用例；6 个 token `includes` 检查 + `app.emit(FILE_OPEN_REQUEST` 计数 ≤2 | `sed -n '38,86p' scripts/verify/checks/task-9.1.mjs` |
| E18 | `task-9.1.mjs` 无注入证明条目 | 套件共 25 条条目，无 `9.1`（它只出现在「delegating」的注释行里，不在 `ENTRIES` 中） | `node scripts/verify/injection-suite.mjs --list` |
| E19 | 豁免规则是**二分类**的：检查里只要出现 `spawnSync`/`execFileSync` 就**整体**豁免注入证明 | `injection-suite.mjs:910` `if (/spawnSync\(|execFileSync\(/.test(source)) delegating.push(path)`；`unproven` 只在两者皆无时收集（`:905-912`） | `sed -n '864,920p' scripts/verify/injection-suite.mjs` |
| E20 | 该豁免桶覆盖 87 个已登记检查中的 75 个，`UNPROVEN` 恒为 0 ⇒ 完整性规则当下**未约束任何检查**；`task-9.1.mjs` 的 `spawnSync` 计数为 1 ⇒ 它是 delegating | `registered: 87 \| proven: 12 \| delegating: 75 \| unproven: 0` | 脚本 A |
| E21 | `SFI-R06-S01/S02/S03` 被指向无关检查 | 命令 = `task-12.7.mjs`（安全加固），其自述 5 条验收项与三条场景的 THEN 子句无交集 | `grep -in 'temporary\|file-open\|file_open\|fileopen\|uuid\|Opened' scripts/verify/checks/task-12.7.mjs` → **0 命中** |
| E22 | 前端用例用的是普通路径，从未覆盖 `file://` 形态 | `App.test.tsx:138` `emit("app://file-open-request", ["/tmp/first.flac", "/tmp/second.flac"])` | `grep -n 'first.flac' -B3 -A3 apps/desktop/src/app/App.test.tsx` |
| E23 | `is_local_media_path` 的既有测试只证明「非本地路径必须被拒绝」 | `actor.rs:2575` `is_local_media_path_classifies_schemes` | `grep -n 'fn is_local_media_path_classifies_schemes' -A 6 crates/echo-desktop/src/player/actor.rs` |
| E24 | 人工举证侧同样为空 | `tests/native/` 50 个 `.md` **全部**是未填写模板；`artifacts/native-attestations/` 为空目录 | `grep -rl '填写该场景的具体可执行步骤' tests/native/ \| wc -l` → 50 |
| E25 | `on_active_root` 是只写死字段 | `commands.rs:656` 硬编码 `false`；`queue.rs:64` 定义；唯一非构造引用是 `coordinator.rs:263` 的字段搬运 | 脚本 B |
| E26 | `task-9.1.mjs` 内部断言与该检查的 `spawnSync` 同处一文件 ⇒ 被判为 delegating 后，**它的 token/count 断言从未需要过失败证明** | `task-9.1.mjs:83-86`（`app.emit(FILE_OPEN_REQUEST` 计数比较）＋ `:31` `spawnSync` | `sed -n '30,90p' scripts/verify/checks/task-9.1.mjs` |

**脚本 A**（复现 E18–E20 的完整性分类，只读）与 **脚本 B**（E16/E25 的映射与读取点盘点）：

```bash
# A：复现 injection-suite 的完整性分类
node - <<'EOF'
const { readFileSync } = require('node:fs'); const { resolve } = require('node:path');
const ROOT = process.cwd();
const manifest = JSON.parse(readFileSync(resolve(ROOT,'scripts/verify/manifest.json'), 'utf8'));
const found = new Map();
for (const t of manifest.tasks || []) for (const c of t.commands || []) {
  const m = (c.cmd || '').match(/(?:^|\s)node\s+(scripts\/verify\/(?:checks\/)?[A-Za-z0-9_.-]+\.mjs)/);
  if (m) found.set(m[1], t.id);
}
const proven = new Set(['injection-suite.mjs','injection-object-kinds.mjs']
  .flatMap((f) => [...readFileSync(resolve(ROOT,'scripts/verify',f),'utf8')
    .matchAll(/"(scripts\/verify\/[^"]*\.mjs)"/g)].map((m) => m[1])));
const delegating = [], unproven = [];
for (const [p, id] of found) {
  if (proven.has(p)) continue;
  const src = readFileSync(resolve(ROOT, p), 'utf8');
  (/spawnSync\(|execFileSync\(/.test(src) ? delegating : unproven).push(`${p} (task ${id})`);
}
console.log('registered:', found.size, '| proven:', [...found.keys()].filter((p) => proven.has(p)).length,
            '| delegating:', delegating.length, '| unproven:', unproven.length);
EOF

# B：相关场景的命令映射 + on_active_root 的引用点盘点
node -e "import('./scripts/verify/scenario-commands.mjs').then(m=>{const l=m.scenarioCommands();for(const s of l) if(/^(SFI-R06|SFI-R07|DAS-R06)/.test(s.id)) console.log(s.id.padEnd(13), s.title.padEnd(28), '=>', s.command);})"
grep -rn "on_active_root" crates/ apps/ -C2   # 只应看到定义/构造/搬运，无读取点
```

### 约束

- **`is_local_media_path` 的拒绝是正确行为，不得放宽。** 它是 task 8.3 的纵深防御，与 mpv 的 `protocol-whitelist=file` 呼应（`actor.rs:1244-1247` 注释）。本 change 的修复点在其上游。
- **`Opened` 分支只在 macOS 编译**（E10）⇒ 修复无法在 Linux CI 上通过真机验证；验收必须靠「纯函数单测 + 结构断言」在任意平台可跑，真机路径作为可选的手工复核。
- **IPC 边界不能传非 JSON 形状**：`play_temporary_file` 的入参与 `FILE_OPEN_REQUEST` 事件的载荷都要过 WebView 的序列化边界，只能是字符串。因此「契约由类型承载」只适用于 shell↔runtime 这一段。
- 无 HMR：前端改动要 `pnpm --dir apps/desktop build` 并重启 dev 窗口才可见。
- `scripts/verify/scenario-commands.mjs` 是场景命令的**唯一权威源**，直接改 `manifest.json.scenarios[]` 或 `tests/scenarios/*.yaml` 会被下一次 `--write` 静默抹掉。

## Goals / Non-Goals

**Goals:**

- 让 Finder 双击（`file://` URL + 百分号编码）在 macOS 上真的出声，且标题显示解码后的文件名。
- 让「OS 边界上的 URL→路径转换」成为有唯一归属点、由类型强制的契约，而不是注释里的约定。
- 让失败/加载中的快照不再携带上一次成功加载文件的 `duration`/`position`。
- 让 `DAS-R06-*` / `SFI-R06-*` / `SFI-R07-*` 的验收命令真的能证伪它们断言的行为——即修好本 bug 的那一层，恰好也是门禁唯一能看见这一层的地方。
- 顺带修正 `SFI-R06-S01/S02/S03` 的无关命令映射。

**Non-Goals:**

- 不做 issue 附节的 UI 调整（沉浸页徽标/按钮、播放条小号「导入」、库内文件不显示导入），也不接线 `on_active_root`。
- 不引入 `MediaPath` newtype。
- 不放宽 `is_local_media_path`，不改 `protocol-whitelist`。
- 不新增 IPC 命令、不改 `main.json` 权限集（因此无 `security.rs` / `gen/schemas/` 联动）。
- 不填 `tests/native/` 的 50 个空模板，不为 `COMMANDS[id] || ATTEST(id)` 兜底分支新增检查。
- 不在本 change 内实现被测代码（propose 阶段只规划）。

## Decisions

### D1：归一化点选在壳的 `RunEvent::Opened` 分支，且先抽成纯函数

新增一个**不依赖 `AppHandle`** 的纯函数，把「URL 列表 → 路径列表」的转换独立出来：

```rust
/// macOS file-association opens arrive as `file://` URLs. Only local files are
/// accepted; percent-encoding is decoded here, exactly once.
fn open_targets(urls: &[tauri::Url]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|url| match url.to_file_path() {
            Ok(path) => Some(path),
            Err(()) => {
                tracing::warn!(%url, "ignoring non-file open request");
                None
            }
        })
        .collect()
}
```

`RunEvent::Opened` 分支变成 `for path in open_targets(&urls) { deliver_file_open(app, path) }`。

理由：`Opened` 只在 macOS 编译（E10），把转换抽成不碰 `AppHandle`、不碰平台 API 的纯函数后，**这个转换语义可以在 Linux CI 上用单测覆盖**——这是让「平台专有分支不可测」不再成为借口的唯一办法。`to_file_path()` 的选择依据见 E11/E12：它在同一次调用里完成 scheme 校验与百分号解码，且 `url` 2.5.8 已是 `Cargo.lock` 中的传递依赖，无需新增依赖（也无需把 `url` 提为直接依赖——`tauri::Url` 就是 `url::Url`，E9）。

备选与放弃理由：

- **在前端用 `decodeURIComponent` 兜底**：`file://` 前缀仍在，actor 依旧拒绝（E5）；而且把 OS 边界的知识泄漏进 UI 层。放弃。
- **在 `runtime::receive_file_open` 里做归一化**：该函数在 `echo-desktop`（跨平台层），会为 macOS 的形态问题给全平台加一条 URL 解析分支；而且它是 `.on_ready()` 排空 FIFO 的同一份数据，两处形态不一致时排空路径仍未覆盖。放弃。

### D2：「契约由类型承载」的边界停在 shell↔runtime，不过 WebView

- **收窄**：`deliver_file_open(app: &tauri::AppHandle, path: PathBuf)`、`StartupSupervisor::receive_file_open(&self, path: PathBuf)`、`pending_opens` 队列元素 `PathBuf`。这样 `url.to_string()` 这一类写法**编译不过**，注释（E8）从"约定"升级为"类型"。
- **不收窄**：`FILE_OPEN_REQUEST` 事件的载荷（序列化为 `string[]`）与 IPC 命令 `play_temporary_file(path: String)`。二者都要过 WebView 的 JSON 边界（约束第 3 条），把 `PathBuf` 推过去只会得到一次无意义的字符串往返。**这是本 change 唯一一处「类型没盖住」的地方，因此必须由测试兜住**（见 D4 与 G3 的任务）。

放弃 `MediaPath` newtype 的理由：`TemporaryItem.path` 与 actor 的 `load_path(&Path)` 已经是 `PathBuf`，newtype 只会在 shell↔runtime 这一段多出一套与 `PathBuf` 互转的类型。收益（更强的语义名）不足以抵消转换噪音，留待独立 issue。

### D3：进度清零收敛为「开始一次加载」的单一入口，而不是在三个 `Failed` 点各清一次

E13/E14 表明原有 **3 个** `Failed` 产生点（形态被拒绝、两个解析失败）都不清进度。若在每处补两行，会留下第 4 个入口（未来新增的加载命令）再次漏掉的隐患。改为收敛：把 `load_path` 开头与两个解析失败分支统一到一个 `fn begin_load(&mut self) -> u64`，它负责 `generation += 1`、`pending_seek = None`、`position = None`、`duration = None` 并返回新的 generation；三个入口都先调它。

语义上的取舍：进入 `Loading` 时快照不再携带旧进度 ⇒ 界面在加载瞬间会看到进度条归零，而不是停留在上一首的位置。这是**期望行为**（旧值本来就是错的），且与 `播放错误处理`（失败要提示并跳过）不冲突——失败原因仍照常上报，只是快照里不再有假进度。

### D4：门禁通电走「扩已有检查 + 补注入条目」，不新写脚本

`task-9.1.mjs` 虽然对归一化全盲（E17），但它**已经**是这 5 条场景的验收命令，且它的主题（OS 文件打开 → 启动 FIFO）正是本 change 的主题。所以：

1. 在 `task-9.1.mjs` 里新增第 4 项验收：断言 `main.rs` 的 `Opened` 分支**不再出现** `url.to_string()`，且**出现** `to_file_path()` / `open_targets`；并以纯函数单测（`open_targets`）作为行为断言，在任意平台可跑。
2. 为 `task-9.1.mjs` 补一条注入证明条目。**注意这不只是「忘了登记」**：E19/E20/E26 表明套件按「文件里是否出现 `spawnSync`」二分类，`task-9.1.mjs` 因为有 1 处 `spawnSync` 被判为 delegating 而**整体**豁免——它的 6 个 token `assert` 与 `app.emit` 计数比较从未需要过失败证明。所以这一条要同时做两件事：(a) 补条目，注入机制用 `mutate`——把 `main.rs` 里的归一化改回 `url.to_string()` 形态，断言 `task-9.1` 退出非 0，然后按哈希恢复；(b) 把「混合形态检查不得整体豁免」写成新增的治理需求（`specs/engineering-governance/spec.md`），并让完整性问题可被机械校验——否则下一个同样形态的检查会再次被静默豁免。

不新写 `task-9.x.mjs` 的理由：新脚本若不同时登记进 `manifest.json` 与注入套件就是第二个死门禁；扩已有检查的成本更低、语义更准（它本来就该管这件事）。

### D5：`SFI-R06-S01/S02/S03` 的命令按 THEN 子句重新选，选不出来就诚实降级

这三条场景的 THEN 子句分别是「创建临时播放项并播放」「使用该歌曲已有 UUID 播放」「按临时项处理且不用旧根 UUID 绕过隔离」（见 `openspec/specs/safe-file-ingestion/spec.md:84-94`）。它们**不是**安全加固（E21 已证明当前映射无关），而是「路径 → 播放项」的分派语义。目标：

| 场景 | 建议命令 | 证伪什么 |
|---|---|---|
| `SFI-R06-S01` | `DAS-R06`/`SFI-R07` 同一份壳侧检查 + 新增的「路径 → 临时项」协调器定向测试 | 归一化后的路径未命中库内歌曲时必须创建临时项并播放 |
| `SFI-R06-S02` | 同上 + 「路径命中活动根歌曲时必须走 `play_context`，不创建临时项」定向测试 | 命中库内歌曲时不得退化为临时项 |
| `SFI-R06-S03` | 若「非活动旧根不得绕过隔离」无法在本机自动化，则**明确降级**为 `check-native-attestation.mjs`，并在 `docs/native-attestation-playbook.md` 写出可执行步骤 + 填实 `tests/native/SFI-R06-S03.md` | 人工举证，而不是错配一条与 THEN 无关的命令 |

具体选哪条实现由施工阶段按实际可测边界决定；本 change 的硬约束是：**每条命令必须能证伪其 THEN 子句，或明确降级为已填实的人工举证**（这正是新增治理需求要求的事）。

### D6：`record_gate_open` 的入参类型跟随 D2 收窄

`main.rs:449` 的 `record_gate_open(paths: &[String])` 服务于 macOS Gate（`task-1.9.mjs` 用 `ECHO_GATE_OPEN_LOG` 断言热启动把路径交给了已运行实例）。归一化后它收到的是 `PathBuf`，一并收窄并 `to_string_lossy()` 写日志。

顺带说明一个**本 change 不修但必须记录**的盲区：`task-1.9.mjs` 用 `spawn(EXECUTABLE, [FIXTURE])` 走的是 argv 支路，且只用 `basename(FIXTURE)` 做子串断言（`scripts/verify/checks/task-1.9.mjs:108-116`），而 fixture 名不含特殊字符 ⇒ 该 Gate 对 `file://` 形态**天然不敏感**。它不承担 `SFI-R06`/`DAS-R06` 的验收，所以不构成本 change 的门禁空洞；但如果将来要让 macOS Gate 真正覆盖文件关联，必须改成 `open -a Echo.app <file>` 触发 `RunEvent::Opened`，并把断言从 `includes(basename)` 改为**整行等于绝对路径**、且用一个含空格/中文的临时 fixture。这条记入 `tasks.md` 的观察项。

### D7：前端只改注释 + 补一条「不做」的用例

`App.tsx` 的逻辑不变（E3 的 `split` 取末段在输入已是路径时是正确的）。改动是：

1. 更新 `App.tsx:78-80` 的注释，说明载荷现在是**已解码的绝对路径**，而不是 URL。
2. `App.test.tsx` 补一条用例：载荷为含字面 `%20` 的路径（例如 `/tmp/My%20Mix.flac`）时，`displayName` 必须是 `My%20Mix.flac`（**不解码**），并断言「前端不调用 `decodeURIComponent`」这一契约。既有 `:138` 的普通路径用例保留为回归。

理由：归一化只在壳侧做一次。如果前端也「顺手」解码，含字面 `%20` 的合法文件名会被破坏——这是修复这类 bug 时最典型的二次伤害。

### D8：范围决策——不新增「场景命令不得靠兜底降级」的检查

E15 的 `COMMANDS[id] || ATTEST(id)` 兜底使未登记的场景静默降级为人工举证（这也让 `missing command for <ID>` 之类的防护永不触发）。这与本 bug 的根因**同源**（都是"看起来有覆盖，实际没有"），但它是仓库级的既有状态：35 条 fallback 场景 + 50 个空人工模板（E22），影响面远超本 bug。硬塞进本 change 会让验收从「双击能出声」漂移成「追溯体系重构」。**处置**：在 `tasks.md` 末尾登记为独立观察项，并作为下一个 change 的候选议题。

## 本轮方案自身的复核

**复核方式**：派一个独立复核者（新上下文，只给 change 目录与仓库路径，不给本文推理过程），要求它逐条重跑 E1–E26 的每个数值与每个签名，并指明 `CONFIRMED / REFUTED / CHANGED`；同时复核 `proposal.md` 的 G1–G6、`specs/` 三个 delta 的 `####` 层级，以及 `openspec validate --strict`。
**复核 HEAD**：`e48ae6e`（与本文一致）。**复核结论**：事实层 **0 处 REFUTED**，`validate --strict` 退出 0，判定可直接施工。

对 issue 原文的逐条核对结果：

| issue 原文 | 复验判定 | 事实（HEAD `e48ae6e`） | 若不纠正的后果 |
|---|---|---|---|
| `main.rs:864` 的 `RunEvent::Opened` 分支 | **CHANGED**（行号） | 实际在 `main.rs:869-874`，`url.to_string()` 在 `:872` | 施工者按 864 定位会落到 `focus_main_window` 的 `Reopen` 分支附近，改错地方 |
| `runtime/mod.rs:256` `receive_file_open(path: String)` | **CONFIRMED** | 行号与签名一致 | — |
| `App.tsx:84-90` | **CONFIRMED** | 订阅 `:84`、`displayName` `:89`、`fireAndForget` `:90` | — |
| `commands.rs:631-657` `play_temporary_file` | **CHANGED**（范围） | 实际 `:631-660`（`on_active_root: false` 在 `:656`） | 影响面清单漏掉函数尾部的 `Ok(())`，无实质影响 |
| `actor.rs:1248` → `:1270` `is_local_media_path` | **CONFIRMED** | `load_path` `:1241-1260`、拒绝分支 `:1248-1256`、`is_local_media_path` `:1270-1272` | — |
| `actor.rs:966` 是 `duration` 唯一写入点 | **CONFIRMED** | `:966` | — |
| `main.rs:444` `record_gate_open` / `:456-460` 注释 | **CHANGED**（行号） | `:449-459` / `:461-465` | 施工者按旧行号找不到 |
| `queue.rs:64` `on_active_root` 无读取点 | **CONFIRMED** | `:64` 定义；`coordinator.rs:656` 是 `TemporaryPlay` 的**同名字段**（issue 未提及的两个同名点），唯一非构造引用为 `coordinator.rs:263` 搬运 | 若把 `TemporaryPlay.on_active_root` 与 `TemporaryItem.on_active_root` 当成一处，接线时会漏一处 |
| `tauri-2.11.5/src/lib.rs:83` `pub use url::Url;` | **CONFIRMED**（逐字） | `:83` | — |
| `url-2.5.8` 的 `to_file_path` 返回 `Result<PathBuf, ()>` | **CONFIRMED** | `url-2.5.8/src/lib.rs:2726` | — |
| `App.test.tsx:138` 用普通路径 | **CONFIRMED** | `:138` | — |
| 「两个正确行为之间存在无人负责的缝隙」 | **CONFIRMED 且已补强** | 补上 issue 未覆盖的门禁层证据 G1–G6 / E16–E26 | 只修代码会让下一次同样的缝隙继续静默通过 |

**独立复核者自身的误判（已逐条复验，勿照抄）**：复核者在总体结论里附了一条「本文 E2/E3/E4/E7/E10/E13/E14 行号有 ±1~4 漂移」的提示。对其中每一条重跑取证后，**该提示本身不成立**——是复核者的读数错误，本文行号是对的。记录在此以免下一次会话据其"修正"出错误的行号：

| 复核者主张 | 实测（重跑命令） | 判定 |
|---|---|---|
| `App.tsx` split 在 `:90`、`fireAndForget` 在 `:92`（本文写 `:89`/`:90`） | `grep -n 'displayName = path.split\|fireAndForget("play_temporary_file' apps/desktop/src/app/App.tsx` → **:89 / :90** | 复核者误判；本文正确 |
| `actor.rs` 两个 `Failed` 在 `:1044`/`:1070`（本文写 `:1042`/`:1066`） | `grep -n '^                    self.state = PlaybackState::Failed' crates/echo-desktop/src/player/actor.rs` → **:1042 / :1066**（另 `:1253` 为拒绝分支） | 复核者误判；本文正确 |
| `main.rs` argv 循环在 `658-660`（本文写 `657-659`） | `sed -n '656,660p' apps/desktop/src-tauri/src/main.rs` → for 在 **657**、`deliver_file_open(app, path)` 在 **658** | 复核者误判；本文正确 |
| `runtime/mod.rs` 就绪分支在 `261-266`（本文写 `262-267`） | `sed -n '258,268p' crates/echo-desktop/src/runtime/mod.rs` → `if ready {` 在 **262**、`enqueue` 在 **265** | 复核者误判；本文正确 |
| `commands.rs` 命中分支是 `642-648`（本文写 `642-651`） | `sed -n '640,652p' apps/desktop/src-tauri/src/commands.rs` → `if let Some(song_id)` **642** … `return Ok(())` **650**、块尾 **651** | 复核者误判；本文正确 |

复核者唯一有实质贡献的两处：(a) 把 `task-9.1.mjs` 的 greps 从"4 项"订正为"三段验收"（已采纳，E17）；(b) 明确确认 `deliver_file_open` / `receive_file_open` / `record_gate_open` 都不是 `#[tauri::command]`，而 `play_temporary_file` 才是——因此「不改命令集与权限 ⇒ `gen/schemas/` 与 `ipc-types.generated.ts` 不动」成立（已并入 Impact 一节）。**元结论**：复核者的价值在敢说 REFUTED，但它同样会错；「事实错了」与「锚点/读数错了」必须分开判定。

issue 未覆盖、本轮新增的发现（**来源均为本次复验，非 issue 文本**）：5 条场景共用 `task-9.1.mjs` 而该检查对载荷形态不敏感（E16/E17）；注入证明的完整性规则因「出现 `spawnSync` 即整体豁免」而覆盖不到 `task-9.1.mjs` 的内部断言，且该豁免桶占 87 个已登记检查中的 75 个、`UNPROVEN` 恒为 0（E18–E20、E26）；`SFI-R06-S01/S02/S03` 被映射到无关的安全加固检查（E21）；`is_local_media_path` 之外还有两个 `Failed` 产生点同样残留进度（E14）；`TemporaryPlay` 与 `TemporaryItem` 各有一个 `on_active_root`（§复审核正表第 8 行）。

## 开放问题

1. **`SFI-R06-S03`（非活动旧根不得绕过隔离）能否在本机自动化？** 它需要「已保留但非活动的旧根 + 该根下的文件」这一状态。若不可行则按 D5 明确降级为已填实的人工举证——**这个选择不影响其余任务**，可在施工阶段决定。
2. **`open_targets` 的可见性**：作为 `main.rs` 内的私有 `fn` + `#[cfg(test)] mod`，还是提到一个可被集成测试引用的位置？倾向前者（`echo-app` 是 bin crate，集成测试无法引用其私有项；`main.rs` 内的单测足够覆盖这条纯函数）。
3. **`main.rs` 是否会因此越线（>1000 行 / clippy 复杂度）** 需要施工时确认；若越线则把 `open_targets` 与其单测放进同 crate 的新模块（如 `apps/desktop/src-tauri/src/open_targets.rs`）。

## 归档复核（2026-09-21）

- **任务 6.1（macOS Finder 双击复核）**：以复核结论关闭。该人工 GUI 步骤依赖 Finder，本会话无法执行（wrapup.md「阻塞 / 待操作者」已如实记录）。其断言的行为已由等价自动化覆盖：`open_targets` 单测断言 `file:///…/%20…` 解码为绝对路径、非 file scheme 丢弃、混合顺序保持；`App.test.tsx` 新增字面 `%20` 载荷用例断言 `displayName` 不解码、路径原样透传；`task-1.9` cold/hot 单实例已由本轮 CI 多平台验证通过。真正 Finder 双击的人工复核留待后续真机会话。
- **任务 6.5（回填归档素材）**：wrapup.md 已回填全部施工输出（open_targets/actor/runtime 测试计数、注入证明、e2e、场景实跑）；6.1 的 Finder 日志片段随 6.1 一并延期。本条随本批复核完成。
