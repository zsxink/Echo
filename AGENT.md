# Echo 项目指引

## 项目定位

Echo 是本地优先的跨平台音乐播放器。项目以 OpenSpec 驱动需求、设计与实现；先明确整体方案，再进入具体开发。

## 总体原则

- 从产品、架构和路线图的整体一致性出发，不只处理局部问题。
- `docs/PRODUCT.md`、`docs/DESIGN.md`、`docs/ROADMAP.md` 是项目级事实来源；界面以 `docs/interface-terminology.md` 和 `docs/prototype/` 为准。
- 产品功能、技术方案、边界条件和验收标准等细节写入 OpenSpec，不在本文件重复维护。
- 规划使用 OpenSpec 的 propose/update 流程；实现只在 apply 阶段进行；完成后同步并归档。
- 修改前读取相关文档和当前 change，发现冲突时先修正规格，保持文档、规格与实现一致。

## 核心约束

- 本地音乐库是产品中心，不提供账号体系或流媒体曲库。
- Core 保持跨平台、与 UI 和播放器无关；播放能力由各端负责。
- 当前阶段、范围和验收标准以 `docs/ROADMAP.md` 及对应 OpenSpec change 为准。

## OpenSpec

具体上下文和产物规则见 `openspec/config.yaml`，代码规范见 `openspec/CODE_STANDARDS.md`。根据任务使用 propose、update、apply、sync、archive 或 explore；规划与实现不要在同一阶段混做。
