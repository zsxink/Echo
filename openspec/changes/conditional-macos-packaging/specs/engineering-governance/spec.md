## ADDED Requirements

### Requirement: 保守分层 macOS 完整打包且严格检查真实发布包

macOS Platform job MUST 保持稳定命名。只有完整 PR 差异能证明所有变更均为已审核的非打包输入路径上的既有普通文件内容改动才 MAY 跳过完整打包，并 MUST 明示未运行，而非判为检查通过。main、审计不充分、未知或混合改动 MUST 执行。macOS Native libmpv smoke MUST 独立执行。执行全包时 MUST 保留 1.9、1.10、9.7 和严格嵌入前端检查。

发布流程 MUST 在发布前检查最终要上传的实际 DMG，并将其中 App 于只读环境下用于签名、文件关联、运行交接、原生依赖和匹配 dist 的嵌入前端检查。验证过程 MUST NOT 重建、重签或验证另一个 App 代替实际产物；不存在、无法启动、签名失败、缺少本轮 dist、非嵌入或旧前端 MUST 失败。测试 launch MUST 隔离 App 数据。

#### Scenario: 仅修改确定无合包影响的文件

- **WHEN** 完整 PR 差异只修改已经审核的既有普通非打包构成文件
- **THEN** 全包步骤及其报告上传不执行，job 报告 skip，独立 native smoke 保留

#### Scenario: 无法证明可以跳过完整打包

- **WHEN** main、未知或混合路径、打包输入变化、异常/无效 Git 数据、重命名、增加或删除出现
- **THEN** 全包准备、构建、原生与前端校验、报告仍然执行

#### Scenario: 真实发布包验收失败

- **WHEN** 实际发布 DMG 或其中 Echo.app 缺失、签名无效、严格嵌入校验失败、文件交接失败或 native gate 失败
- **THEN** 阻断发布，不用重新构建、重签或邻近 staged App 兜底
