# Proposal

## Why

对 workspace 做了一次完整的 `rust-review` 只读复核（issue #162），范围覆盖 4 个 member、约 17.6 万行 Rust。工具基线全部通过——`cargo fmt --check` 干净、`cargo clippy --all-targets --all-features -- -W clippy::pedantic` 零告警、`cargo test --workspace --all-features` 1894 通过——但复核仍发现 **13 处必修问题**，其中 2 处是未定义行为、3 处可被外部输入（恶意媒体文件、局域网对端）触发崩溃或资源耗尽。

问题集中在两个方向，都不是 lint 能抓的形态：

1. **把已校验的类型降级成字符串再还原。** watcher 把构造期校验过的 `RelativeMediaPath` 折成 `identity_key()` 存进 pending map，flush 时再用 `RelativeMediaPath::new()` 反解——大小写敏感文件系统上 `Song.MP3` 变成 `song.mp3`，采样取不到文件，新建/修改事件被当作「文件已消失」静默丢弃。同类问题还包括路径包含性检查对 `media/` 自身是符号链接时失效，以及删除路径用字节长度截断推导 `.lrc` 兄弟路径。
2. **外部字节流进入解析器时缺少上限。** QuickTime `udta` 递归没有深度上限，约 1 MB 的嵌套 atom 就能栈溢出（`catch_unwind` 抓不到，进程直接死）；LRC `offset` 与 symphonia `calc_time` 的秒字段相乘没有溢出保护；配对引导接受任意证书且配对码与 TLS 会话没有任何密码学绑定，局域网内主动 MITM 可让受害端 pin 住攻击者的 SPKI。

还有三处属于「错误被静默降级」：`restore` 把存储错误变成空库（整次恢复被读作全部拒绝）、`repos_catalog` 的多语句写入没有事务（崩溃后留下「已发 outbox 但行还在」的中间态）、服务端原样采用对端给的 `changed_at_ms`（一个 peer 发 `u64::MAX` 就能永久赢得所有编辑）。

**为什么现在做**：这些问题不会被现有门禁发现，且随着用户量增长暴露面在扩大——恶意媒体文件来自用户的音乐收藏，配对攻击来自同一个局域网。同时 P0-8 需要改协议版本，越晚改代价越大。

## What Changes

- **watcher 保留真实相对路径**：pending 事件按 `RelativeMediaPath` 存储而非折叠后的 `identity_key()`，`flush_ready` 用原路径采样。大小写敏感文件系统上不再丢事件。
- **路径包含性检查覆盖起点**：`media/` 本身是符号链接时拒绝扫描，而不是把 `canonical_base` 指到库外后让逐条目检查恒真。
- **不可信容器解析加上限**：QuickTime `udta` 递归限制深度；LRC 时间轴偏移与探测时长改用 `checked_*`，溢出按既有「丢弃单行 / 无时长」语义处理而非 panic 或回绕。
- **恢复与投影不再吞错**：`restore` 读取失败向上传播；`repos_catalog::delete` / `remove_member` 的多语句写入收进单个事务；根目录 epoch 解析失败与自增溢出不再静默归零/回绕。
- **配对码绑定到 TLS 握手（**BREAKING**）**：引入 SPAKE2（RFC 9382）平衡 PAKE，让六位配对码参与握手认证，校验通过前不 pin 证书；`danger_accept_invalid_certs` 只覆盖那一次 bootstrap 请求，且 pin 的 SPKI 必须与 PAKE 通道绑定的证书一致。协议版本从 1 升到 2，旧客户端不得写入。
- **冲突裁决的时钟输入加上界**：服务端拒绝或钳制对端提供的 `changed_at_ms`，使其不能无界地压过本机后续编辑。
- **Objective-C 绑定与生命周期修正**：`addTarget:action:` 按 `void` 返回值绑定（当前 release 下对野指针 `retain`）；重装屏幕观察者时先 `removeObserver` 再替换。
- **删除路径的 `.lrc` 推导改用后缀剥离**：无扩展名音频不再多减一字节，且不再用 `expect` 兜底。
- **领域错误类型可进入错误链**：四个只有 `Debug` 的公开错误枚举补上 `Display` 与 `std::error::Error`。
- **可达 panic 归零**：播放器 actor 用「当前时刻回退一秒」做首帧发布判据，该计算在系统运行时长不足一秒时返回空值并 `unwrap` 崩溃；改为降级取值。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `local-library`: R01「单一资料库根目录与本地持久化」——根目录 epoch 读取失败与自增溢出不得静默归零或回绕；R02「扫描、监听与手动重扫」——监听事件必须按真实相对路径生效，不得因大小写折叠被当作文件消失；R03「一期格式与内容解析矩阵」——不可信容器必须有解析深度上限，时间轴与时长算术溢出必须降级而非崩溃；R05「SQLite 检索与曲库排序」——删除等跨多表的投影写入必须在单个事务内完成；R07「资料库不可用与删除恢复」——删除时推导的同名 `.lrc` 兄弟路径必须精确，无扩展名不得截断；R08「跨平台路径、隐私与性能约束」——`media/` 自身为符号链接时扫描必须拒绝，不得读取库外文件。
- `library-durability`: LD-06「不完整资料不得被静默破坏」——恢复读取存储失败时必须报告失败，不得以空快照继续并把全部记录读作「拒绝」。
- `lan-library-sync`: R02「显式启用与可信设备加入」——六位配对码必须参与握手认证，校验通过前不得固定对端证书；R05「可解释的冲突与删除裁决」——对端提供的变更时间戳必须受本机时钟约束。
- `macos-now-playing-menu`: R03「Echo 处理 macOS 系统远程传输命令」——系统远程命令的注册不得因错误的返回值绑定而使应用崩溃或引用无效对象。
- `macos-notch-player`: R01「刘海播放器只在带刘海的 macOS 屏幕上出现」——重新评估屏幕参数时必须先注销旧观察者，不得让通知中心持有已释放对象。
- `desktop-playback`: R01「桌面音频播放」——播放后端初始化不得因主机运行时长等环境状态崩溃；空播放上下文必须以明确冲突失败，不得索引越界。
- `engineering-governance`: 新增要求——从公开函数返回的错误类型必须实现 `Display` 与 `std::error::Error`，可进入调用方的错误链。

## Impact

- **代码**（按 crate）：
  - `crates/echo-core`：`infrastructure/filesystem/watcher.rs`、`infrastructure/filesystem/walker.rs`、`infrastructure/metadata/tags/mod.rs`、`infrastructure/metadata/lrc.rs`、`infrastructure/metadata/probe.rs`、`infrastructure/sqlite/repos_catalog.rs`、`application/restore.rs`、`application/root_switch.rs`、`application/delete.rs`、`domain/sync.rs`、`domain/sync_members.rs`
  - `crates/echo-sync`：`pairing.rs`、`client_pairing.rs`、`server.rs`、`protocol.rs`，新增 PAKE 握手模块
  - `crates/echo-desktop`：`player/actor/mod.rs`、`player/coordinator.rs`
  - `apps/desktop/src-tauri`：`macos_now_playing.rs`、`notch/window.rs`
- **依赖**：`echo-sync` 新增 `pakery-crypto`（`default-features = false`，启用 `std`、`spake2`、`p256`），以及 `rand_core 0.10` 与 `getrandom 0.4`（`sys_rng`）作为握手随机源。已核验：MSRV 1.88（与 workspace 一致）、MIT OR Apache-2.0（在 `deny.toml` 白名单内）、`#![forbid(unsafe_code)]`、基于 `zeroize`。相对当前 `Cargo.lock` 净增 23 个 crate（`p256`、`elliptic-curve`、`crypto-bigint`、`hkdf`、`hmac`、`sha2 0.11`、`der`、`sec1` 等），全部许可证在既有 allow 列表内；`sha2` 0.11 与既有 0.10 并存（`multiple-versions` 现为 `warn`）。`getrandom 0.4` 的 `r-efi 6` 仅对 UEFI target 生效，`deny.toml` 的 `targets` 只含三个桌面平台，该 crate 不参与许可证判定。
- **协议（**BREAKING**）**：`PROTOCOL_VERSION` 1 → 2，配对握手新增消息与证书绑定字段。已配对设备需重新配对；不兼容客户端不得写入已升级的资料库（既有 epoch/协议校验路径已覆盖拒绝逻辑）。
- **数据库**：无 schema 变化、无迁移；仅把既有语句序列收进事务。
- **场景基线**：`local-library`、`library-durability`、`desktop-playback` 三个已登记区域新增 9 个场景，基线由 621 变为 630，需同步 `docs/traceability.md`、`scripts/verify/scenario-commands.mjs` 与生成的 `manifest.json`。`lan-library-sync`、`macos-*`、`engineering-governance` 不在 `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX` 内，其 delta 不改变基线。
- **规范与分层**：全部修复落在既有分层内——`echo-core` 的领域/应用/基础设施边界不变，平台专属修复只触及 `echo-sync` 与 `apps/desktop/src-tauri`，Core 不新增对 Tauri/mpv 的依赖。适用 `openspec/CODE_STANDARDS.md` §2（不得有未处理 panic 与静默失败）、§3（依赖方向）、§4.1（`-D warnings`、`echo-core` 不使用 `unsafe`）、§4.2（`thiserror`、业务路径不 `unwrap`）、§5（SQLite 为可重建投影、关联数据在单个投影事务中提交）。
- **非目标**：不处理 issue #162 的 13 项 P1 与 11 项 P2（另立变更）；不改目录提交协议、恢复流程、封面管线、播放队列语义或界面交互；不引入新的 lint 强制项（P2 中的机械防护建议另行评估）。
