# Design

## Context

动机与问题清单见 `proposal.md` 与 issue #162；需求见本 change 的 delta specs。这里只记录实现这些需求时必须先定的技术选择。

当前状态（HEAD `762e4f55` 实测）：

| 事实 | 位置 |
|---|---|
| pending 事件以 `relative.identity_key()` 为键，flush 时 `RelativeMediaPath::new(&key)` 反解 | `crates/echo-core/src/infrastructure/filesystem/watcher.rs:116/131/142/179/194`、`:232` |
| `RelativeMediaPath` 的 `Eq`/`Ord`/`Hash` 只取 `identity`，`Display` 取 `normalized` | `crates/echo-core/src/domain/ids.rs:524-548` |
| `identity_key()` 是 NFD + case fold | `crates/echo-core/src/domain/text.rs:125-129` |
| walker 用 `canonical_root(&media_base)` 作包含性基准，`has_media_dir` 用 `is_dir()`（跟随链接） | `crates/echo-core/src/infrastructure/filesystem/walker.rs:50-52`、`:74-77` |
| `walk_quicktime_udta` 对 `udta` 子 atom 无深度参数地递归 | `crates/echo-core/src/infrastructure/metadata/tags/mod.rs:276-288` |
| LRC `timestamp_ms + offset_ms` 直接相加 | `crates/echo-core/src/infrastructure/metadata/lrc.rs:69` |
| 探测 `time.seconds * 1_000_000_000 + frac_nanos` 直接相加 | `crates/echo-core/src/infrastructure/metadata/probe.rs:115` |
| `restore` 用 `all_in_root(root).unwrap_or_default()` | `crates/echo-core/src/application/restore.rs:103` |
| `repos_catalog::delete` / `remove_member` 多语句自动提交；同文件 `add_member`/`upsert_member` 已用 `connection.transaction()` | `crates/echo-core/src/infrastructure/sqlite/repos_catalog.rs:262-300`、`:359-421`、`:343-357` |
| 根目录 epoch 解析失败 `unwrap_or(0)`，自增 `current_epoch + 1` | `crates/echo-core/src/application/root_switch.rs:366-385` |
| `derive_lrc_path` 按字节长度截断并 `expect` | `crates/echo-core/src/application/delete.rs:545-551` |
| 配对 bootstrap `danger_accept_invalid_certs(true)`，配对码只做回显比对 | `crates/echo-sync/src/client_pairing.rs:7-29`、`:53` |
| 服务端 `let changed_at_ms = mutation.changed_at_ms;` 原样入裁决 | `crates/echo-sync/src/server.rs:607` |
| `PROTOCOL_VERSION: u16 = 1`，`MutationDto.protocol_version` 在提交摘要覆盖范围内 | `crates/echo-sync/src/protocol.rs:15`、`:417-423` |
| `addTarget:action:` 绑定为 `Retained<AnyObject>` | `apps/desktop/src-tauri/src/macos_now_playing.rs:328-330` |
| 重装屏幕观察者只替换 `OBSERVER` 槽位；`unregister_screen_observer` 才 `removeObserver` | `apps/desktop/src-tauri/src/notch/window.rs:296-329` |
| actor 初始化 `Instant::now().checked_sub(1s).unwrap()` | `crates/echo-desktop/src/player/actor/mod.rs:324-326` |
| `play_context` / `play_context_paused` 对 `ctx.songs[selected]` 取下标 | `crates/echo-desktop/src/player/coordinator.rs:187-226` |
| 四个领域错误枚举只有 `Clone/Copy/Debug/Eq/PartialEq` | `crates/echo-core/src/domain/sync.rs:242-245`、`:375-383`、`:706-710`；`domain/sync_members.rs:23-27` |

约束：

- `echo-core` 不得依赖 Tauri/mpv/平台 crate；PAKE 只能落在 `echo-sync`（`echo-sync` 已有 `rustls`/`reqwest` 依赖，属于传输适配器）。
- workspace `rust-version = "1.88"`，`unsafe_code = "deny"`，`clippy::all/pedantic/nursery = warn` 且 CI 用 `-D warnings`。
- `deny.toml` 只允许白名单 SPDX 表达式，`unknown-registry = "deny"`。
- 场景 ID 由 spec 中的**位置**派生：新增场景必须追加到所属 Requirement 的末尾。

## Goals / Non-Goals

**Goals:**

- 13 项 P0 全部有可复现的失败证据与修复后的回归测试，且证据能映射到 delta spec 的场景。
- 修复保持分层：Core 的领域/应用/基础设施边界不变，平台修复只出现在 `echo-sync` 与 `apps/desktop/src-tauri`。
- 每项修复都不引入新的静默失败——原本被吞掉的错误必须变成可诊断的失败，原本的 panic 必须变成降级语义或类型化错误。

**Non-Goals:**

- 不追求把 `identity_key` 从 `RelativeMediaPath` 中移除；比较语义（大小写折叠）本身是正确的，错的是把它当路径用。
- 不重写配对协议的整体生命周期（邀请/凭据/撤销），只在既有一次性 bootstrap 上加 PAKE 握手并推进版本。
- 不改 SQLite schema、不新增迁移；`repos_catalog` 只是把既有语句收进事务。
- 不引入 `clippy::arithmetic_side_effects` 等全局 lint 强制（issue #162 的 P2 建议另行评估）。
- 不处理 issue #162 的 P1/P2。

## Decisions

### D1. watcher 的 pending 键改为真实路径，比较键只用于相等性

**选择：** `pending: HashMap<RelativeMediaPath, PendingEvent>`，`flush_ready` 直接使用键本身的 `normalized()` 采样，不再 `RelativeMediaPath::new(&key)`。

**理由：** `HashMap` 的键相等性仍走 `identity`（`Eq`/`Hash` 只取 `identity`），所以同一文件的大小写变体仍正确合并为一条 pending；而 `normalized` 保留原始大小写，采样路径因此正确。这样既保住了「两个拼写折叠成一首歌」的既有语义，又不再把折叠值当路径。

**备选：** 给 `RelativeMediaPath` 增加 `identity_key()` 的逆运算——不可能，折叠是有损的；或在 `PendingEvent` 里额外存一份 `PathBuf` 而键仍用字符串——会引入两处真值来源。

**注意：** `RESCAN_SENTINEL` 是 `String`，不是路径。`pending` 改为以 `RelativeMediaPath` 为键后，哨兵不能再混入同一张表。做法是拆成两个字段：`pending: HashMap<RelativeMediaPath, PendingEvent>` 加一个独立的 `rescan_at: Option<Instant>`；`flush_ready` 在 `rescan_at` 到期时先发一条 `RescanNeeded`。

### D2. `media/` 的符号链接检查放在 walker 入口，用 `symlink_metadata`

**选择：** `enumerate_files` 在 `has_media_dir` 之后、`canonical_root` 之前检查 `std::fs::symlink_metadata(&media_base)`；`file_type().is_symlink()` 时返回 `Error::unavailable("library", "library media/ must not be a symlink")`。

**理由：** 用 `unavailable` 而非 `InvariantViolation`——这是用户目录的形态问题，不是程序 bug，且扫描入口对这类失败已有「保留既有记录、报告可重试」的既有语义。`symlink_metadata` 不跟随链接，是唯一能看到「`media` 本身是链接」的系统调用。

**备选：** 在 `subscribe`（watcher）里也加同样的检查——`subscribe` 已经 canonicalize 根目录（`watcher.rs:356-362`），行为一致即可；两处都检查会重复，只在 walker 做并让 watcher 依赖扫描结果。

**未选：** 允许 `media/` 为指向库内的符号链接。这会要求逐条目校验链接目标仍在库内，复杂度高而收益为零——Echo 自己从不创建这种布局。

### D3. QuickTime `udta` 深度上限用固定常量，超限即停止递归

**选择：** `walk_quicktime_udta(..., depth: u8, ...)`，入口传 `0`，子 atom 传 `depth + 1`；`depth >= MAX_UDTA_DEPTH`（取 `8`）时直接返回，不报错、不计数。

**理由：** 合法 m4a 的 `udta` 嵌套通常只有 1–3 层（`udta` → `meta` → `ilst`），8 层留足余量。不报错是因为这类文件本就无法解析出更多标签，符合既有「无法解析的内容不中断整次扫描」语义。

**备选：** 用 `InputLimits` 承载该上限——会把「字节上限」与「结构深度」混进同一个配置类型，且现有调用方全部构造 `InputLimits::defaults()`，加字段会波及测试。取常量更小、更不可误配。

### D4. 时间轴与时长算术用 `checked_*`，溢出走既有降级路径

**选择：**
- LRC：`timestamp_ms.checked_add(offset_ms)` 为 `None` 时 `out_of_range += 1; continue;`——与既有的 `shifted < 0` 分支完全同构，不新增计数维度。
- probe：`time.seconds.checked_mul(1_000_000_000).and_then(|n| n.checked_add(frac_nanos))` 为 `None` 时该音轨不提供时长（`duration: None`），而不是整次探测失败。

**理由：** 两个降级路径都已经存在（LRC 的越界丢弃、probe 的 `duration: Option`），溢出只是它们的另一个触发条件。这样修复不改变任何外部可见的语义类别。

### D5. `restore` 传播读取错误

**选择：** `let snapshot = self.deps.songs.all_in_root(root)?;`。

**理由：** `run` 已经返回 `Result<RestoreOutcome, Error>` 且文档写明「propagates infra failures while reading the control surface or `media/`」——当前实现违反了它自己的文档。`?` 之后错误沿既有路径上抛，不需要新的错误变体。

### D6. `repos_catalog` 的多语句写入收进单个事务

**选择：** `delete` / `remove_member` 内所有语句改在 `let transaction = connection.transaction().map_err(storage)?;` 上执行，末尾 `transaction.commit().map_err(storage)?`——与同文件 `add_member`（343-351）逐字同构。

**理由：** 这是「同文件已有正确写法、只有两个方法遗漏」的情形，一致性本身就是理由。不改 schema，`rusqlite` 的 `Transaction` 在 drop 时自动回滚，天然满足「任一步失败整体不生效」。

**注意：** 事务内的语句必须全部改用 `transaction` 句柄而非 `connection`，否则会与已持有的事务争用同一连接并死锁。实现时必须逐句替换，不能只包一层。

### D7. 配对码用 SPAKE2 绑定到 TLS 通道（**BREAKING**）

**选择：** 引入 `pakery-crypto`（`default-features = false`，`features = ["std", "spake2", "p256"]`），使用其 `Spake2P256` 套件（RFC 9382 合规，P-256 + SHA-256，`NH = 32`）。握手在既有的一次性 bootstrap 请求上做两轮：

1. 设备以配对码派生口令标量 `w = P256Group::scalar_from_wide_bytes(blake3(DOMAIN || "password" || code || device_id))`，`PartyA::start(w, device_id, HOST_IDENTITY, aad, rng)` 得到首轮消息 `pa`，随 `PairingCodeRequest` 一起发出。
2. 主机用同一 `w` 做 `PartyB::start(...)` 得到 `pb`，`finish(pa)` 得到会话密钥与确认 MAC；把 `pb` 与自己的确认 MAC 放进 `PairingBootstrapResponse`。
3. 设备 `finish(pb)`，`verify_peer_confirmation(host_mac)` 常量时间校验通过后，才校验响应里的证书并固定 SPKI；随后把设备侧确认 MAC 用于让主机确认设备侧也持有配对码。

**修正（实现 7.2 时发现 D7 原写法不可行）：** 原文的 `blake3(code || invitation_id || binding)` 要求设备在发首轮消息前就知道 `invitation_id` 与 `binding`，但设备此时只有用户输入的六位码——邀请信息只存在于主机侧与二维码/剪贴板里，`PairingCodeRequest` 里也没有这两个字段。同时 SPAKE2 的 aad 在 `PartyA::start` 时即固定，设备那时也还没拿到主机证书。因此改为：

- 口令标量提交 `code` 与 `device_id`（两者在首轮消息发出时双方都已知），使一次捕获的首轮消息无法被另一台设备重放；
- `invitation_id`/`binding` 改由主机在 PAKE 已认证的响应中下发，中继无法在不完成确认的前提下伪造；
- 证书绑定改为 PAKE 之后的**显式证书证明**：主机用会话密钥对证书 DER 做 `blake3` keyed hash（`blake3::derive_key("echo-lan-pairing-v2 certificate proof", session_key)` 派生 key），设备在 `verify_peer_confirmation` 通过后、固定 SPKI 之前校验该证明。中继能转发 SPAKE2 消息，但没有配对码就派生不出会话密钥，因此无法为自己的证书生成证明。

这样「固定的公钥 MUST 是本次握手中被认证的那一个」仍然成立：认证点从 aad 移到会话密钥下的证书证明。

**配对码不得上线（实现 7.4 的约束）：** 现有 `PairingCodeRequest` 明文携带六位码。若继续携带，中继在 bootstrap TLS 上直接读到码，就能分别与两端完成 PAKE，前面的绑定全部失效。因此请求 DTO 必须去掉 `code` 字段；主机改用自己保存的邀请码（`Invitation` 在生命周期内保留明文码，最长 10 分钟）派生 `w`。设备端确认 MAC 回传后主机才发放 grant。

**理由（为什么是 SPAKE2 而不是「HMAC(code, SPKI)」）：** issue #162 的初版建议 `HMAC(code, server_cert_spki)` 只能证明「发请求的一方知道 code」，不能证明「对端在 TLS 握手中知道 code」——MITM 可以自己算这个 HMAC 再转发。要真正把 code 绑到通道上，必须是不对称的密钥交换：只有持有 code 的一方才可能算出会话密钥，任何中继都无法同时冒充两端。SPAKE2 是 RFC 9382 的平衡 PAKE，正是这个形状。

**理由（为什么是这个 crate）：** 已实测 `pakery-crypto 0.6.2`：`rust-version = 1.88`（与 workspace 完全一致）、`MIT OR Apache-2.0`（`deny.toml` 白名单内）、`#![forbid(unsafe_code)]`、基于 `zeroize`；`Spake2P256` 是唯一 RFC 合规套件（`Spake2Ristretto255` 的文档明写「非 RFC 套件、没有合规对端」，不能用）。`spake2-conflux` 要求 Rust 1.89，超出 MSRV，已排除。相对当前 `Cargo.lock` 净增 23 个 crate，全部许可证在 allow 列表内；`sha2` 0.11 与既有 0.10 并存（`multiple-versions = "warn"`）。`getrandom 0.4` 的 `r-efi 6` 声明 `LGPL-2.1-or-later`，但它只对 UEFI target 生效，而 `deny.toml` 的 `targets` 只列三个桌面平台，因此不参与许可证判定——实现时须在 `cargo deny check licenses` 的输出里确认这一点。

**端到端已验证：** 在 `/tmp` 里用最小 crate 跑通了 `PartyA`/`PartyB` 双方 `finish` + `verify_peer_confirmation`，两侧 `into_session_key()` 相等，`P256Group::scalar_from_wide_bytes` 是配对码 → 口令标量的正确入口。

**随机源：** `echo-sync` 目前没有 `rand_core` 依赖，现有随机性来自 `Uuid::new_v4()`。PAKE 需要 `rand_core::CryptoRng`，而该 trait 的定义是 `Rng + TryCryptoRng<Error = Infallible>`——**它不允许可失败的实现**，因此不能简单地「失败时返回类型化错误」。做法分两步：

1. 握手开始前先用 `getrandom::fill(&mut probe)` 做一次预检，失败时返回既有的 `PairingError::Unavailable`（OS 熵源不可用是环境问题，不是密码学失败）。
2. 预检通过后使用 `getrandom 0.4` 的 `sys_rng` 特性配合 `rand_core::UnwrapErr(SysRng)` 适配，并把适配包在一个本地 newtype 里，注释写明「预检已排除可恢复失败，此后的失败是进程级不可恢复状态」。

`echo-sync` 因此需要直接依赖 `getrandom 0.4`（`features = ["sys_rng"]`）与 `rand_core 0.10`。

**协议版本：** `PROTOCOL_VERSION` 1 → 2。`MutationDto.protocol_version` 已在提交摘要覆盖范围内（`protocol.rs:417-423`），既有校验点（223、410）自动拒绝旧版本；配对响应新增字段由 serde 缺省拒绝（无 `#[serde(default)]`），旧客户端会得到明确的协议失败而不是静默降级。

**`danger_accept_invalid_certs` 的边界：** 保留，但注释必须写明它只覆盖这一次 bootstrap；pin 的 SPKI 必须来自「本次握手已认证」的那份 DER，`validate_pairing_bootstrap` 在 PAKE 确认之前不得放行。

### D8. 服务端钳制对端提供的 `changed_at_ms`

**选择：** `server.rs` 在构造 `MutationCandidate` 前把 `mutation.changed_at_ms` 钳到 `now_ms() + MAX_CLOCK_SKEW_MS`（取 5 分钟）。`None` 保持不变（既有语义）。`echo-sync` 需要一个可注入的时钟以便测试，但当前 crate 里没有——实现时把 `now_ms` 作为 `HostApiState` 的一个 `fn() -> u64` 字段（默认 `SystemTime::now`），测试可替换。

**理由：** 钳制而非拒绝——拒绝会让合法但时钟偏快的设备完全无法同步；钳制只剥夺它「永远获胜」的能力，符合「本地优先、冲突可解释」的产品取向。5 分钟远大于正常的 NTP 偏差。

**不改写：** 钳制只影响本次裁决的输入值，不修改已持久化的 mutation 正文，也不影响来源设备自己冻结的时间戳（spec 已明确这一点）。

### D9. Objective-C 返回值绑定

**选择：** `let _: () = unsafe { msg_send![&*command, addTarget: target, action: action] };`

**理由：** `MPRemoteCommand.h:57` 声明返回 `void`。objc2 的 `ConvertReturn<NoneFamily> for Retained<T>` 会执行 `retain_autoreleased` 并检查非 NULL，而 `void` 的返回寄存器是垃圾值——debug 下 `msg_send_check` 的编码比对会 panic，release 下直接对野指针 retain。绑定为 `()` 后 objc2 不做任何转换。

### D10. 屏幕观察者重装先注销

**选择：** `install_screen_observer` 在登记新观察者之前，先 `if let Some(previous) = OBSERVER.with(|slot| slot.borrow_mut().take()) { unsafe { NSNotificationCenter::defaultCenter().removeObserver(&previous); } }`，然后才 `addObserver` 并写回槽位。

**理由：** `addObserver:selector:name:object:` 内部保存的是**不持有**的引用，所以旧的 `Retained` 一旦被 drop，通知中心就持有悬垂指针。`unregister_screen_observer`（320-329）已经证明正确做法存在，重装路径只是没走它。抽出私有 `fn remove(slot_value)` 让两条路径共用同一段 `unsafe`，避免出现第二份 `removeObserver` 调用。

### D11. actor 的计时器降级

**选择：** `Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(Instant::now)`。

**理由：** 该字段只用于「首次属性事件总是发布」的回溯判据，取 `now` 会让首帧仍发布（因为 `last_property_publish` 与 `now` 相等，比较用的是严格小于）。降级后语义不变，且不再有 panic 路径。

### D12. `derive_lrc_path` 用后缀剥离

**选择：**

```rust
let stem = match audio.extension() {
    Some(extension) => normalized
        .strip_suffix(extension)
        .and_then(|s| s.strip_suffix('.'))
        .unwrap_or(&normalized),
    None => &normalized,
};
RelativeMediaPath::new(&format!("{stem}.lrc")).map_err(|_| Error::invariant("lrc sibling path"))
```

**理由：** `extension()` 已返回小写扩展名，而 `normalized` 保留原始大小写——`strip_suffix` 在大小写不一致时返回 `None`，此时 `unwrap_or(&normalized)` 退化为「原路径 + `.lrc`」，这是安全的（宁可找不到侧车，也不要指向错误文件）。`Error::InvariantViolation { why }` 已存在但**没有 builder**（`crates/echo-core/src/error.rs` 只有 `validation`/`io`/`unavailable` 三个构造器），实现时需补一个 `Error::invariant(why)`——否则这条修复会变成「从 `expect` 换成手写结构体字面量」，仍是一处 `InvariantViolation` 的直接构造点。注意：路径推导失败是输入形态问题而非程序 bug，更贴切的类别可能是 `Error::validation`；实现时以「该路径来自已校验的 `RelativeMediaPath`，失败只可能来自内部推导逻辑」为由选 `invariant`，并在注释里说明。

**备选：** 用 `Path::with_extension("lrc")`——它会在无扩展名时正确追加，但返回 `PathBuf`，仍需转回 `RelativeMediaPath` 并处理 Windows 分隔符，收益不大。

### D13. 领域错误补 `thiserror::Error`

**选择：** 四个枚举加 `thiserror::Error` 派生与逐变体 `#[error("...")]`，保留既有 `Clone/Copy/Debug/Eq/PartialEq` 派生。文本只描述类别，不带参数。

**理由：** 这四类错误不带数据，`Display` 无需插值，因此不涉及敏感信息。保留 `Copy` 是关键——既有调用方可能依赖它，去掉会成为无声的破坏性变更。

### D14. 场景登记范围

`scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX` 只登记 12 个区域。本 change 修改的 7 个 capability 中：

- 在登记范围内：`local-library`（LL）、`library-durability`（LD）、`desktop-playback`（DP）——新增 9 个场景，基线 621 → 630。
- 不在登记范围内：`lan-library-sync`、`macos-now-playing-menu`、`macos-notch-player`、`engineering-governance`——其 delta 不进入 `traceability.md` / `manifest.json`，不改变基线。

**理由：** 给这四个区域临时加 `AREA_PREFIX` 会一次性引入它们的全部既有场景，把本 change 的场景计数改动从 9 条放大到上百条，且会与「只登记已正式发布的区域」的既有注释冲突。登记这四个区域是独立事项。

**代价：** 这四个 delta 场景没有场景级验收证据，只能靠 crate 级测试命令证明（在 `tasks.md` 中逐条写明）。这一取舍必须在 `tasks.md` 中显式记录，不能让读者以为它们被漏掉了。

## Risks / Trade-offs

- **PAKE 依赖体积** → `echo-sync` 净增 23 个传递依赖（含 `p256`、`elliptic-curve`、`crypto-bigint`）。缓解：全部为纯 Rust、无 `unsafe`、许可证在既有白名单；`cargo deny check licenses` 与 `cargo deny check advisories` 必须在新依赖落地后立即跑一遍，任一失败则回到选型。备选方案是自实现 SPAKE2（不可接受：自研密码学）。
- **协议版本推进导致已配对设备全部失效** → 这是有意的安全修复，但必须让用户在升级后得到明确提示而非「同步静默不工作」。缓解：`tasks.md` 要求把版本不匹配映射到既有的可诊断配对失败类别，并补一条面向用户的失败分类测试。
- **大小写敏感文件系统的测试盲区** → macOS/Windows 默认不区分大小写，现有 CI runner 也以这两者为主，D1 的修复在本地可能测不出来。缓解：测试必须直接构造 `RelativeMediaPath` 与假的 pending 状态，断言「`Song.MP3` 的 flush 采样路径仍是 `Song.MP3`」，不依赖真实文件系统的大小写行为；并在 Linux runner 上跑同一条用例。
- **`transaction` 误用导致死锁** → D6 若漏改某句仍用 `connection`，会在同一连接上二次借用并 panic 或死锁。缓解：改完后逐行 `rg -n 'connection\.' ` 检查该函数体内无残留，且回归测试必须覆盖「事务中途失败后行与墓碑都在」。
- **钳制时钟可能与真实产品语义冲突** → 若将来支持跨时区/长时间离线设备，5 分钟上界可能过于严格。缓解：把上界做成 `echo-sync` 的公开常量并写进 spec 的「允许偏差」措辞（当前 spec 只要求「固定允许偏差」，不锁具体数值）。
- **`AREA_PREFIX` 外的四个 delta 无场景证据** → 见 D14，接受该缺口并在 `tasks.md` 中显式记录。

## Migration Plan

1. **顺序：** 先做纯 Core 修复（D1–D6、D11、D12、D13，无协议/依赖影响），再单独做 D7（PAKE + 协议版本），最后做 D8–D10（传输与平台）。D7 单独成组是为了让「依赖与协议变更」在 diff 里可辨认，出问题时可单独回滚。
2. **回滚：** D1–D6、D11–D13 各自独立，逐项 revert 即可。D7 的回滚是「`PROTOCOL_VERSION` 退回 1 + 移除 PAKE 握手 + 移除依赖」——由于版本不兼容是单向的（升级后的库被旧客户端拒绝），回滚只对尚未升级的资料库安全；已升级的资料库需要保持新版本或按既有恢复流程处理。这一点必须在 PR 描述中写明。
3. **兼容：** 无 SQLite schema 变化，无迁移；旧库、既有投影、恢复流程不变。已配对设备在 D7 落地后需重新配对一次。
4. **发布：** 建议 D7 与其它 P0 一起进同一版本，避免出现「同一版本内部分设备能配对、部分不能」的中间态。
