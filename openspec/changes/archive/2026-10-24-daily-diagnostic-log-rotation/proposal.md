# Proposal

## Why

桌面 runtime 的结构化日志与 panic 崩溃日志目前按大小轮转：文件名恒为 `echo.log` / `crash.log`（轮转产物 `.1`…`.4`），不带日期、保留窗口由写入量决定而非天数，清理只发生在轮转路径上。用户报障时无法说明日志对应哪一天，跨天问题（"昨天启动就闪退""只在夜间播放时出错"）几乎无法定位；静默机器上的旧文件也永远不会回收（issue #38）。同时，日志行内的 `timestamp` 字段由 `tracing-subscriber` 默认计时器生成，是无时区标注的 UTC 值，用户与开发者对照本机墙钟时间时存在 8 小时级别的错位，需要以机器本地时间为准。

## What Changes

- 诊断日志与崩溃日志**按本地日期切分命名**：`echo-YYYY-MM-DD.log` / `crash-YYYY-MM-DD.log`（日期为写入时刻的机器本地日期）。跨零点写入在写入路径内自动切换到新日期文件，无需重启、不依赖后台定时器。
- 日志行内 `timestamp` 改为**机器本地时间并携带 UTC 偏移**（如 `2026-10-24T21:05:03.123+08:00`），不再输出无标注的 UTC。
- **保留最近 7 天**：启动时与每日切分时删除文件名日期早于「今天 − 7 天」的日志文件（按文件名日期判定，不依赖 mtime）；当天没有产生任何日志时，启动清理同样回收超过 7 天的残留文件。
- **清理旧命名残留**：启动清理识别并删除历史 `echo.log` / `echo.log.N` / `crash.log` / `crash.log.N`，避免两套命名长期共存。
- **字节上限保留为第二道闸**：单文件上限（10 MiB）触顶时在同一日期内继续按 `.N` 轮转；累计字节上限（50 MiB）按「日期最旧优先、同日内序号最大优先」删除，两条路径（运行日志、崩溃日志）均受约束；清理永不删除当日活跃文件。
- `crates/echo-desktop/src/platform/diagnostics.rs` 按职责拆分为模块目录（facade / rolling writer / retention），并引入 `chrono`（`clock` feature）作为本地日期与时区的直接依赖。

## Capabilities

### New Capabilities

- `diagnostic-logging`: 本地诊断日志的切分、时间戳时区、保留窗口与字节上限契约——按本地日期切分文件、日志时间戳用本地时区、保留最近 7 天（含静默回收与旧命名清理）、字节上限作为第二道闸。

### Modified Capabilities

（无。`local-library` 仅约束日志脱敏隐私，`phase-one-acceptance` 仅要求诊断日志安装无已知失败，两者行为均不变。）

## Impact

- 代码：`crates/echo-desktop/src/platform/diagnostics.rs`（拆分为 `diagnostics/mod.rs`、`diagnostics/rolling.rs`、`diagnostics/retention.rs`），公开 API `Diagnostics::log_path` / `crash_path` 返回值变为日期化文件名，`RollingLog::new` 入参由「完整文件路径」改为「目录 + 文件前缀」；`crates/echo-desktop/tests/privacy.rs` 同步新命名断言。
- 依赖：`echo-desktop` 新增 `chrono`（`default-features = false, features = ["clock"]`；已是 Cargo.lock 传递依赖，`time` crate 的本地偏移在多线程 Unix 上不可用，故选 chrono）。不触碰 `echo-core`（Core 保持平台无关，日志通道归桌面 runtime 所有）。
- 文档：`docs/LOGGING.md` §3 的「保留最近若干份」改为明确的按日切分 + 最近 7 天 + 字节上限兜底，并补充时间戳本地时区约定。
- 调用点：`apps/desktop/src-tauri/src/bootstrap.rs` 的安装调用（`Diagnostics::new` / `install_logger` / `install_panic_hook`）签名不变，仅注释措辞更新。
- 非目标：不做「设置里查看/打开日志目录」「导出诊断包」；不改变日志脱敏与隐私规则（`local-library` 既有要求不变）；不改变崩溃日志的本地不上传语义；不为保留天数/字节上限增加用户设置项。
