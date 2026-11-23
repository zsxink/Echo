## 1. 隔离 Linux 缓存工具链

- [x] 1.1 在四个 Linux Rust 缓存参与 job 安装工具链前接入共享 Rustup home 设置。
- [x] 1.2 验证共享配置、顺序、缓存与原有门禁；执行脚本测试、task 1.5 与相关注入验证。
- [x] 1.3 跑 registry/static 治理检查、OpenSpec 严格校验和 `git diff --check`，同步规格并归档，再更新 Draft #166。

验证：干净的独立 Rustup home 实装并确认只列出 Rust 1.98.1；本地 registry/static 治理（18 项回归）、3 个 Linux Rustup 注入证明、OpenSpec 严格校验、`git diff --check` 通过。GitHub 热缓存命中须在正常 main 运行写入新键后观察。
