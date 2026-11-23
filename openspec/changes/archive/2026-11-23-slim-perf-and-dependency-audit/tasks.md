## 1. 精简并验证

- [x] 1.1 在保持 job、50k benchmark 和原生测试不变的前提下实现 PR 性能路径筛选；验证修改、混合改动、删除/重命名、空或未知 diff、main。
- [x] 1.2 合并 cargo-deny 政策命令，拆出在线 advisory 命令；用替身验证调用次数、阻断与离线回退语义。
- [x] 1.3 更新 task 1.5 结构门禁和注入证明；运行新增 Node 测试、结构与治理验证、`git diff --check`。
- [x] 1.4 运行 OpenSpec 严格校验，同步 delta 后归档；记录本地验证范围，不把替身验证当成完整线上审计。

验证记录：`pnpm verify:governance -- --sections=registry,static`（含 16 个回归）、`node scripts/verify/injection-suite.mjs --only ci-structure/perf-`（3 个反向证明）、OpenSpec 严格校验与 `git diff --check`。依赖审计回归使用命令替身；原生 Rust 验证和实际审计留给原有 CI。
