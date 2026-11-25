# 验证记录

- 对应 GitHub Issue：[#176](https://github.com/zsxink/Echo/issues/176)。
- 基线 `4735cbbe861061925b7e6d1f9472c655c6c3fd89`：通过同一仓库入口 `pnpm verify:task -- 1.7` 重现失败（退出 1），RustSec 报告 RUSTSEC-2026-0592 和 RUSTSEC-2026-0594。
- 使用 Cargo 1.98.1 的精准版本更新只改变 `url 2.5.9`、`cookie 0.18.3` 和必要的 `form_urlencoded 1.2.3`。正式候选复用该解析结果；`cargo metadata --locked --format-version 1` 通过，这三项均来自 crates.io，许可仍为 `MIT OR Apache-2.0`，与 `LICENSES.md` 一致。
- 正式候选使用 Cargo 1.98.1、cargo-deny 0.22.1、cargo-audit 0.23.0、Node 22.23.5、pnpm 11.18.0。先执行 `pnpm install --frozen-lockfile`，再执行未开启离线模式的 `pnpm verify:task -- 1.7`：退出 0，日志未报告在线检查跳过。复核使用的 RustSec 数据库快照为 `10db6998b5d74ec438bcf0763125bd425a068a8f`。
- 直接运行 cargo-audit 时仍有既有非阻断告警 `glib 0.18.5`（RUSTSEC-2024-0429）；没有新增忽略项，没有发现其他阻断项。随后在 [GitHub 托管 runner](https://github.com/zsxink/Echo/actions/runs/45542301540/job/138963483336) 上再次通过任务 1.7，未跳过在线检查。
- `git diff --check` 和 OpenSpec 严格校验通过。没有规格差量，无需同步主规格。
- #177 后续：锁文件更新后使用 `node scripts/release/generate-third-party-notices.mjs` 同时同步仓库许可清单与安装包内 `THIRD-PARTY-NOTICES.txt`。再运行生成器 `--check` 和 `pnpm verify:task -- 18.1`，均通过；`Cargo.lock`、`LICENSES.md` 及门禁脚本没有额外变化。
