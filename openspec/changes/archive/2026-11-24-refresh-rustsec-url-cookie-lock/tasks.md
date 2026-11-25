## 1. 精准刷新 Rust 依赖
- [x] 1.1 在已有对应 Issue 后，运行 `cargo update -p url --precise 2.5.9` 和 `cargo update -p cookie --precise 0.18.3`；检查只有上述二者和必要的 `form_urlencoded 1.2.3` 发生变更。
- [x] 1.2 用 `cargo metadata --locked --format-version 1` 核对依赖版本、仓库及许可，将 `LICENSES.md` 与 `Cargo.lock` 对齐。

## 2. 验证并归档
- [x] 2.1 以未开启离线标记的 `pnpm verify:task -- 1.7` 验证，必要时直接比对同一数据库的基线和候选 Rust 审计结果；记录真实额外发现或网络限制。
- [x] 2.2 执行 `git diff --check` 和 `openspec validate refresh-rustsec-url-cookie-lock --strict`。通过后归档；无 delta spec 同步。
