## 1. 夹具

- [x] 1.1 修正 walker 测试夹具的 Windows 链接种类；验证成功创建的链接本身和目标种类，保留现有扫描断言。

## 2. 验证

- [x] 2.1 运行 `cargo fmt --all -- --check`、`cargo test --locked -p echo-core --all-targets --all-features infrastructure::filesystem::walker::tests` 和 `git diff --check`。
- [x] 2.2 在具备创建符号链接权限的真实 Windows runner 通过同一 walker 测试入口，确认未触发夹具跳过；检查 Windows CI main 平台矩阵入口。
- [x] 2.3 验证并归档 OpenSpec，确认既有 local-library 产品规格无需同步。

## 当前验证证据

- Linux / Rust 1.98.1：walker 7/7 通过，未跳过；格式与 diff 检查通过。
- Windows 主分支矩阵启用 Developer Mode，并通过 Cargo workspace 入口运行 walker 测试。首轮验证发现 shell 对三处嵌套路径的错误解析，改用原生 API 后，[Windows hosted 验证](https://github.com/zsxink/Echo/actions/runs/45544136647/job/138969525978) 通过：`cargo test --locked -p echo-core --lib --all-features infrastructure::filesystem::walker::tests -- --nocapture`，7/7 通过且无任何夹具跳过，包含根 `media/` 和嵌套目录的符号链接测试。
- Hosted 验证提交 `dd9d5158fe67aa12b2423433f4d3def498d79ba0` 相对于集成候选 `0af57c4b97b4ab8811be9e43635a0643bd51c6b1` 只增加了临时验证工作流；两者与本变更的 walker 源文件字节相同。临时工作流不进入交付分支。
- 本次为测试夹具修复，不改变既有 `local-library` 行为规格，也没有 delta spec 需要同步。
