# Tasks

## 1. 依赖与模块拆分

- [x] 1.1 `crates/echo-desktop/Cargo.toml` 新增 `chrono = { version = "0.4", default-features = false, features = ["clock"] }`；`crates/echo-desktop/src/platform/diagnostics.rs` 拆分为 `diagnostics/mod.rs`（facade + installer + `write_crash`）、`diagnostics/rolling.rs`（`RollingLog`）、`diagnostics/retention.rs`（命名/解析、清理、累计裁剪、`LocalTimer`），`platform` 模块 re-export 保持 `Diagnostics` / `RollingLog` 入口不变。
  - 验证：`cargo build -p echo-desktop`（通过；echo-app 需 worktree 内补 gitignored 的 `apps/desktop/dist` 占位后一并编译）

## 2. 按日切分与本地时间戳（retention / rolling）

- [x] 2.1 `retention.rs`：日期化命名与严格解析助手（`echo-YYYY-MM-DD.log[.N]` / `crash-YYYY-MM-DD.log[.N]`、旧命名白名单识别）、`cleanup_logs_dir`（7 天窗口 + 旧命名删除 + 各前缀累计裁剪、保护当日活跃文件）、`LocalTimer`（RFC 3339 本地时间 + 偏移）；附单测（窗口边界 today−7 保留 / today−8 删除、旧命名删除、非日志文件不动、裁剪顺序、`LocalTimer` 可解析且偏移等于本机偏移）。
  - 验证：`cargo test -p echo-desktop diagnostics::retention`（7 用例通过）
- [x] 2.2 `rolling.rs`：`RollingLogInner` 改持 `dir + prefix + retention_days + clock`，`write` 内判定日期切换（关旧句柄 → 更新日期 → `cleanup_logs_dir` → 开新文件），日内大小轮转改为日期化 `.N` 并复用累计裁剪；`new` 用真实本地时钟、`with_clock` 供测试注入；改写既有轮转/截断/累计单测并新增跨零点切换、切换时清理两用例。
  - 验证：`cargo test -p echo-desktop diagnostics::rolling`（6 用例通过）
- [x] 2.3 `mod.rs`：`Diagnostics` 增加 `retention_days`（默认 7）与常量 `DEFAULT_RETENTION_DAYS`；`log_path` / `crash_path` 返回日期化文件名；`install_logger` / `install_panic_hook` 安装时执行启动清理；`write_crash` 按当日日期命名、日内轮转、追加前执行清理；subscriber 挂 `.with_timer(LocalTimer)`；更新路径断言单测。
  - 验证：`cargo test -p echo-desktop platform::diagnostics`（5 用例通过，含启动清理删除旧命名/超窗文件的断言）

## 3. 集成测试与调用点

- [x] 3.1 `crates/echo-desktop/tests/privacy.rs`：`RollingLog` 构造与读取改走新签名 / 活跃路径；`crash.log` 文件名断言改为日期化命名（本地日期）；模块头注释同步。
  - 验证：`cargo test -p echo-desktop --test privacy`（3 用例通过）
- [x] 3.2 `apps/desktop/src-tauri/src/bootstrap.rs` 安装点注释措辞同步（按日切分 + 启动清理），签名不动。
  - 验证：`cargo clippy --workspace --all-targets --all-features -- -D warnings`（全绿，含 echo-app 编译）

## 4. 文档与规格

- [x] 4.1 `docs/LOGGING.md` §3：目录内容改为「按本地日期切分（`echo-YYYY-MM-DD.log` / `crash-YYYY-MM-DD.log`）、保留最近 7 天（按文件名日期）、单文件 ≤ 10 MiB 与累计 ≤ 50 MiB 第二道闸、启动与跨日切换时清理（含旧命名残留）」，并补充 §2.3 时间戳本地时区 + 偏移约定。
  - 验证：人工对照 delta spec 一致性（逐条对应 `diagnostic-logging` 五条 requirement）

## 5. 整体验证与归档

- [x] 5.1 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test -p echo-desktop -p echo-app --all-features` 全绿。
  - 验证：fmt 通过；clippy 全绿；测试 550 通过 / 0 失败（echo-desktop 429+，echo-app 107，privacy 3 含于其中；cargo-deny 本机未装，chrono 许可证 MIT/Apache-2.0 且已是传递依赖，CI 门覆盖）
- [x] 5.2 `openspec validate daily-diagnostic-log-rotation --strict` 通过 → `openspec archive daily-diagnostic-log-rotation` → 主规格全量 strict 校验通过（本机 CLI 1.6.0 无 `--archived` 选项，等价执行 `openspec validate --specs --strict`：20/20 通过，`diagnostic-logging` 5 条 requirement 已入主规格）。
