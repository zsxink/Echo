## Context

现有 `task-gates` core 矩阵调用 `1.1 1.3 12.5 13.3`，并在性能路径判断前上传 `benchmark-50k`。`release-gate` 使用 `--delegated=…benchmark…`，场景 LE-R07-S01 因此不会重复执行；直接删除 task 或 upload 会触发结构检查，而用空壳保留会丢失证据。主分支 #45339130187 的 task 步骤合计 2m29s（不等于 50k 单项），上传约 1s，另一个独立 `pnpm verify:perf` 为 5m44s，三条场景引用同一个构建并执行的 release-profile 测试批次。

## Goals / Non-Goals

保留原基准、SQLite 真查询、200/500 ms p95 门槛、50,000 条规模和 JSON 脚本；换入口，不降低独立前端虚拟化、其他默认性能和覆盖门禁。不增写缓存、不动审计优化，不将 50k 误报为扫描内存实测。

## Decisions

1. 独立 Linux `workflow_dispatch` 工作流使用固定 Rust 工具链及 Node，调用现有 task-12.5 检查脚本并上传原 JSON（缺文件失败）。没有 push、pull_request、定时或 reusable 调用入口，不新增缓存写入。本地入口不依赖 GitHub；本地执行生成同格式文件而不上传。
2. 将现有 50k 场景结构证据由默认 job 换成确实执行命令的手动 workflow。默认 release gate 明确使用手动递延模式，输出具体场景 ID 和 `not run`，独立计数，不能计入 `ok: scenario`。其他默认 CI delegation 不变；本地 full / 显式场景命令不传递延选项。
3. 结构验证同时约束两个方向：默认 CI 不含运行 benchmark / upload 的矩阵、独立流程或其他步骤；手动流程只允许指定触发、真实脚本、缺文件失败上传和正确顺序。失败注入覆盖新增默认触发、删除命令、条件跳过、调包产物、默认重新引入及递延报告不一致。
4. 遵守代码规范的单一职责、漂移守卫和不静默失败。工作流装配依赖治理适配层和原性能脚本，不引入产品层反向依赖。现有注入登记、manifest 生成来源和 release gate 校验保持一致。

## Risks / Trade-offs

默认 CI 不再直接发现 50k SQLite 延迟回归；相关结构验证、轻量测试不等于自动测量。只有用户手动触发或在本地执行才有当次 50k 证据。比较时间时需拆分编译、执行与其他步骤，不能将原 task 组合耗时全记给 50k。
