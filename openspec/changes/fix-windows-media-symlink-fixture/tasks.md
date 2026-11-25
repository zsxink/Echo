## 1. 夹具

- [x] 1.1 修正 walker 测试夹具的 Windows 链接种类；验证成功创建的链接本身和目标种类，保留现有扫描断言。

## 2. 验证

- [x] 2.1 运行 `cargo fmt --all -- --check`、`cargo test --locked -p echo-core --all-targets --all-features infrastructure::filesystem::walker::tests` 和 `git diff --check`。
- [ ] 2.2 在具备创建符号链接权限的真实 Windows runner 通过同一 walker 测试入口，确认未触发夹具跳过；检查 Windows CI main 平台矩阵入口。
- [ ] 2.3 验证并归档 OpenSpec，确认既有 local-library 产品规格无需同步。

## 当前验证证据

- Linux / Rust 1.98.1：walker 7/7 通过，未跳过；格式与 diff 检查通过。
- 已确认 Windows 主分支矩阵启用 Developer Mode，并通过 Cargo workspace 入口运行 walker 测试；首轮 Windows runner 确认根 media/ 拒绝通过，但三处 cmd 路径解析导致跳过，整套验证失败；改用原生 API 后待重跑。
