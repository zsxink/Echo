# Design

## Context

`crates/echo-desktop/src/platform/diagnostics.rs`（626 行）是桌面 runtime 的诊断通道 owner：解析 `echo/logs` 目录、安装全局 `tracing` subscriber（JSON 行写入滚动文件）与 panic hook（追加 `crash.log`）。当前切分策略是纯按大小：`RollingLog::write` 持锁写入、超单文件上限即 `rotate`（`.N` 平移、超份数删除、累计字节裁剪），崩溃日志走同构的 3 份轮转。

两个硬约束决定设计形态：

1. **全局 subscriber 一个进程只能装一次**（`install_logger` 靠 `try_init().is_ok()` 幂等）。跨零点切换不能靠"启动时起后台定时器换文件"——定时器无法更换已注入 subscriber 的 writer。所幸 `RollingLog::write` 本来就每次持锁，切日判定放在写入路径内即可。
2. **保留判定必须按文件名日期，不依赖 mtime**（issue #38 明确要求，避免文件复制/同步导致 mtime 漂移把新文件当旧文件删掉）。

时间能力现状：workspace 无任何日期/时间 crate 的直接依赖；`chrono 0.4.45` 与 `time 0.3.47` 都只是传递依赖。既有教训（`local-library` 的时区呈现要求、"len 类问题"的跨平台测试教训）要求时区处理显式且可测。

## Goals / Non-Goals

**Goals:**

- 运行日志与崩溃日志文件名携带机器本地日期，跨零点写入自动切换到新日期文件，无需重启。
- 日志行内 `timestamp` 为机器本地时间并带 UTC 偏移，可对照墙钟直读。
- 保留窗口 = 最近 7 天（按文件名日期），启动与每日切分时清理，静默机器也会回收；升级残留的旧命名文件一并删除。
- 单文件 / 累计字节上限继续生效，作为日期维度之外的第二道闸；两条日志路径同受约束。
- 日期相关逻辑可注入"今天"，跨零点、7 天边界、字节裁剪均有确定性单测，不依赖真实时间推进。

**Non-Goals:**

- 不做日志查看 UI、诊断包导出、保留天数/上限的用户设置项（后续能力另行立 change）。
- 不改变日志内容与脱敏规则（`docs/LOGGING.md` §1/§2 与 `local-library` 既有要求原样生效）。
- 不引入后台线程/定时器做任何清理；所有清理都在启动安装或写入/崩溃路径内同步完成。
- 不迁移（重命名）旧命名文件的历史内容，只删除（见 Decisions D4）。

## Decisions

### D1 时间库选 `chrono`（而非 `time` / `jiff`）

本地日期与本地时区偏移是本变更的核心需求。`time` 的 `UtcOffset::current_local_offset()` 在多线程 Unix 程序中直接返回错误（soundness 限制，桌面 app 必然多线程），Windows 虽可用但跨平台行为不一致，不可选。`jiff` 能力完备但是全新增依赖（tzdb 体积）。`chrono` 已是 Cargo.lock 传递依赖（0.4.45），`Local::now()` 在三大平台多线程下均可用，`default-features = false, features = ["clock"]` 即可覆盖需求，锁文件几乎零增量。依赖只加在 `echo-desktop`（平台层），`echo-core` 不感知时间库，分层不变。

### D2 切日判定放写入路径，writer 持有「目录 + 前缀」而非固定路径

`RollingLogInner` 改为持有 `dir` + `prefix`（`"echo"` / `"crash"`）+ 可注入时钟 `clock: Arc<dyn Fn() -> NaiveDate + Send + Sync>`（生产为 `Local::now().date_naive()`）。`write` 每次持锁后先取"今天"：与 `inner.today` 不同则关旧句柄、更新日期、跑一次目录清理（含 7 天窗口、旧命名、累计裁剪），再打开新日期文件。这样无定时器、无额外线程，且首写发生在构造之后跨零点的场景也正确（构造时即取一次今天，不依赖文件是否已打开）。

日期化文件名：`echo-YYYY-MM-DD.log`，日内大小轮转产物为 `echo-YYYY-MM-DD.log.N`（复用现有 `rotation_path` 拼接）。

### D3 保留语义：`date < today - 7 days` 删除，其余保留

`cutoff = today − 7 天`；文件名日期 **早于 cutoff** 的删除，即保留 `today-7 … today` 共至多 8 个日期的文件。与验收口径一致："8 天前的被删除、7 天内的保留"（恰好 7 天前的文件属"7 天内"，保留）。判定只看文件名里的日期，不看 mtime；解析失败的名字不删（见 D5 的白名单例外）。

### D4 旧命名残留直接删除，不做迁移重命名

升级首启时磁盘上可能有 `echo.log` / `echo.log.1..4` / `crash.log.*`。选项 A：按 mtime 推断日期重命名成新格式保留历史；选项 B：启动清理中直接删除。选 B：旧文件没有可靠的日期来源（mtime 恰是本设计明确不信任的信号，重命名会把错误日期写进按日期判定的保留体系，制造"永不过期"或"立即过期"的错位文件），且旧内容最多只覆盖最近几小时——损失可控，复杂度归零。清理规则按精确白名单匹配：`echo.log`、`crash.log` 及其 `.数字` 后缀，其余文件（如 `paths.json`）一概不动。

### D5 清理与累计裁剪统一为一个目录扫描，按「日期升序、同日序号降序」删除

`cleanup_logs_dir(dir, retention_days, today)` 一次 `read_dir`：

1. 识别 `echo-DATE.log[.N]` / `crash-DATE.log[.N]`（严格 10 字符 ISO 日期解析），`date < cutoff` 删除；
2. 识别旧命名白名单，删除；
3. 对每个前缀独立做累计字节裁剪：收集该前缀全部文件 `(date, index, len)`，总量超 `max_total_bytes` 时按 `date` 升序、同日 `index` 降序（序号越大内容越旧）逐个删除直至达标；**当日 index 0（活跃文件）永不删除**。

旧实现按 `max_rotations` 窗口数 `.N` 平移裁剪，新实现改为目录扫描式，因为日期维度加入后"份数窗口"不再能表达"跨日期取最旧"。运行日志 `rotate`（日内触顶）复用同一裁剪函数。清理在三个时机同步执行：`install_logger` / `install_panic_hook` 安装时（启动静默回收）与 `RollingLog` 每日切换时；`write_crash` 追加前也执行（崩溃低频、单次目录扫描成本可忽略，保证崩溃路径同样受 7 天窗口约束）。

### D6 时间戳计时器：自定义 `FormatTime`，本地时间 + 显式偏移

`tracing_subscriber::fmt::layer()` 增加 `.with_timer(LocalTimer)`：`LocalTimer` 用 `chrono::Local::now().to_rfc3339_opts(SecondsFormat::Millis, false)` 输出 `2026-10-24T21:05:03.123+08:00`。JSON 行的 `timestamp` 字段因此是本地墙钟时间且带偏移，既满足"以机器本地时间为准"，又因偏移显式而无歧义（UTC 机器上输出 `+00:00`，不伪装成别的时区）。

### D7 模块拆分：单一 `diagnostics.rs` → `diagnostics/` 目录

现文件 626 行，加入日期化/清理/计时器逻辑与测试后必然突破规范红线（>1000 行必须拆分）。按职责拆三块，公共入口稳定：

- `diagnostics/mod.rs`：模块 doc、`Diagnostics` facade（目录解析、`log_path` / `crash_path`、两个 installer）、`write_crash` 及其测试；
- `diagnostics/rolling.rs`：`RollingLog`（切日 + 日内轮转 + 写入）及其测试；
- `diagnostics/retention.rs`：命名/解析助手、`cleanup_logs_dir`、累计裁剪、`LocalTimer` 及其测试。

对外仍由 `platform` 模块 re-export `Diagnostics` / `RollingLog`，`bootstrap.rs` 调用点签名不变。

### D8 API 兼容策略：小步破坏，随 change 一次到位

`log_path()` / `crash_path()` 返回值变为日期化文件名（调用方仅测试与日志）；`RollingLog::new` 入参改为 `(dir, prefix, max_file_bytes, max_rotations, max_total_bytes, retention_days)`，另提供 `with_clock` 注入时钟（供测试与隐私集成测试复用）。这些都是 crate 内部/测试边界，无跨 crate 消费者，直接改签名并同步 `privacy.rs`，不做弃用过渡。

## Risks / Trade-offs

- [本地时区在测试机上不可控（CI 多为 UTC），"本地日期"相关断言可能机器相关] → 时钟注入：所有日期判定单测用固定/可变假时钟驱动，不读真实时间；真实时钟仅在生产构造函数与 `LocalTimer` 中出现，`LocalTimer` 断言只验证"可解析 + 偏移等于本机偏移"，不硬编码时区。
- [清理误删非日志文件（如 `paths.json`、用户放入的文件）] → 严格白名单：只识别 `echo-`/`crash-` 前缀 + 严格 ISO 日期 + `.log[.N]` 后缀，以及旧命名精确匹配；解析失败一律不动。
- [升级首启删除旧命名文件丢失最近历史] → 接受（D4）：旧文件无可靠日期来源，重命名反而制造日期错位；损失上限为旧命名文件覆盖的时段。
- [写入路径内做目录扫描带来延迟尖刺] → 清理仅在启动安装、每日切换（每天至多一次）与崩溃写入（低频）时发生；常规写入路径只有一次日期比较。单目录内文件数有 7 天 × 日内轮转上限约束，扫描成本可忽略。
- [时钟回拨（NTP 校时、手动改时）导致日期倒退] → 行为退化为"切回那个日期的文件继续追加"，不删除、不崩溃；窗口判定以写入时的 `today` 为准，最坏情况是该文件保留至下一次正常清理。可接受。
- [`chrono` 为平台层新增直接依赖] → 已在锁文件中（传递依赖），`clock` 最小 feature；`echo-core` 不引入，架构守卫（Core 无平台依赖）不受影响。

## Migration Plan

1. 实现与测试合入后，首次启动即完成迁移：启动清理删除旧命名文件，新写入走日期化文件名；无需数据迁移步骤。
2. 回滚：还原提交即可——旧代码会重新创建 `echo.log` 并忽略日期化文件；日期化文件在旧清理逻辑下不会被删除（旧逻辑只动 `echo.log.N`），仅占盘直至人工清理，无破坏性后果。

## Open Questions

（无——保留窗口、命名、清理时机均由 issue #38 期望行为确定；`chrono` 选型依据已固化于 D1。）
