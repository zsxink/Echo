## 1. 迁移运行入口

- [x] 1.1 仅从 PR / main 默认 CI 移除 task 12.5 和 JSON 上传，保留筛选、其他性能、覆盖、审计、平台与前端虚拟列表门禁。
- [x] 1.2 加入只接受手动触发的 Linux 工作流，并保留本地入口与同名产物、缺文件错误。固定与现有项目相同工具链，不创建工具缓存。

## 2. 验证执行语义

- [x] 2.1 结构守卫验证默认不运行、手动真实运行和上传顺序；更新 benchmark 场景的 carrier 检查、失败注入及防默认回归证明。
- [x] 2.2 默认 release gate 递延输出不得伪装为执行通过；核对准确到 ID 的独立计数和缺漏检测，本地完整 / 直接场景回归仍运行。
- [x] 2.3 运行 `node scripts/verify/self-test.mjs`、`pnpm verify:governance -- --sections=registry,static`、相关 `injection-suite`、`git diff --check` 和静态生成漂移检查。

## 3. 实际测量与交付

- [x] 3.1 以本地 task-12.5 及手动 workflow 的同一命令跑真实 50k，验证 JSON 字段、预算和近邻测试；运行默认 Release gate 的同一命令，验证明确递延而非通过。
- [x] 3.2 同步工程治理 delta，运行 `openspec validate move-50k-benchmark-to-manual --strict`，通过后归档再提 Draft PR；验证记录区分本地和 hosted；默认分支接纳入口后才能实际手动调度。
