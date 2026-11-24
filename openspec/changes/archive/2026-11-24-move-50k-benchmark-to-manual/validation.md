# #171 验证记录

## 本地实际结果

- Rust/Cargo 1.98.1；本地 `pnpm verify:task -- 12.5` 跑真实 SQLite 50,000 首基准，JSON 中搜索 p95 81.2 ms、首屏 p95 296.3 ms，预算仍为 200/500 ms。
- 手动工作流所用同一命令 `node scripts/verify/checks/task-12.5.mjs` 实际生成 JSON：95.9 ms、37.4 ms（单次本地结果，不是 GitHub 时钟或加速结论）。
- 直接 `node scripts/verify/run-scenario.mjs LE-R07-S01` 执行一个真实 Cargo 命令并通过。默认 CI 的 Release gate 同一命令 `node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf,workspace-test,frontend-test --defer-manual-benchmark` 通过：585 个自动化条目中 584 个按常规处理，LE-R07-S01 明确递延、未执行且未计入成功。19 条命令执行、528 项由同一默认 CI 其他 job 承载。
- `node scripts/verify/self-test.mjs`、`node scripts/verify/checks/task-1.5.mjs`、`node scripts/verify/injection-suite.mjs --only benchmark`（4/4 违规触发）均通过。门禁接线和故障验证由实际命令观察。
- `pnpm verify:governance -- --sections=registry,static` 通过，18 个现有工作流回归通过；本机 FFmpeg 与固定版本有差异，现有门禁按既定规则提示环境漂移。场景清单 645，manifest 漂移 0；PRD 14 条验收路径保持。
- OpenSpec 严格校验、同步后场景映射检查和 `git diff --check` 通过。

## 交付或上线后的验证

以上均为本地或结构验证，没有宣称 GitHub 手动 dispatch 或 hosted CI 已通过。新的 `workflow_dispatch` 需 workflow 首次出现在默认分支后才可实际触发；如另用临时 push-only workflow 验证实际 runner，其结果也要与正式手动触发分开记录。
