## Context

#164 与 #166 的 Linux 缓存键分别包含 `ade52a28` 和 `fcc63f3e`，依赖摘要相同。缓存 action 会枚举 Rustup 中所有已安装的工具链，包含 runner 的预装版本。

## Decisions

- CI `rust`、`task-gates`、`governance-static` 和独立 `dependency-audit` 在安装 Rust 前，统一将 `RUSTUP_HOME` 配置为 runner 用户目录内的 Echo 专用路径。用 repo-local Bash 脚本写入 `GITHUB_ENV`，不卸载或改写 runner 的默认 Rustup home。只隔离工具链目录，不改变 `CARGO_HOME`、缓存实现、锁文件或任何 Cargo 命令。
- 两个共享缓存组的读写双方一起迁移。保留现有键边界、actions 缓存环境哈希和 main-only 写入约束。macOS 和 Windows 不受影响。
- 静态门禁验证四处引用、顺序和共享缓存组；用临时目录验证脚本输出，使用现有注入套件证明绕过或漏接时失败。实际 GitHub CI 验证该工具链可正常安装并运行现有检查。新键初次冷启动，不把首轮当作缓存命中效果。

## Non-Goals

不修改测试矩阵、编译 profile、锁文件、原生门禁和发布产物，不人为触发 main workflow 或扩大 PR 权限。
