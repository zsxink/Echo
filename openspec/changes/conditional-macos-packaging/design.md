## Context

CI 1.9 创建 arm64 App 后作临时签名，继续 1.10/9.7 和内嵌资源检查。Release 先构建最终 DMG，目前只验证临近 staged App 的 libmpv。独立 task 8.12 不检查文件关联、内嵌资源或文件交接。见 #173。

## Goals / Non-Goals

PR 安全且保守地节约完整打包；CI 及 release 不把缺证据视为已通过。保持任务、预算、默认 main、Windows/Linux、发布签名和公开产物流不变。不声称直接启动检查等同 Gatekeeper、安装器或公证验证。

## Decisions

1. 和现有性能分类一样基于完整 Git 三点差异，仅接受逐一列出的既有普通文件内容改动。包专用白名单不得含 CSS、源码、资源、原生库、构建配置或门禁输入；初版仅审计 docs 中的静态计划和历史验证报告。假阴性成本仅为重跑；任何异常、路径未知、二义 merge-base 或其他事件都运行完整路径。
2. CI 的 macOS Platform job 保持并明确打印 skip 理由。完整路径仍以同一个 App 执行 1.9 / 1.10 / 9.7、严格前端新鲜度和报告。独立 macOS Native smoke 不受分类影响。移除前置 frontend build 只在 clean-dist 真实 runner 验证通过后实施；否则先保留。
3. 包检查是独立验证模块，接受明确现有 App 和对应 dist。CI 中正常 build 仍做既有 ad-hoc signing，发布检查入口禁止 build、签名或覆写已有包；唯一预期 DMG 必须正好是后续上传的路径。只读挂载后以其中 App 的绝对路径执行全部检查，要求签名有效、不能接受 dev/no-binary skip，测试启动数据隔离；卸载在失败路径也执行。
4. 静态守卫和 failure mutations 检查流向、条件、真正上传的文件、证据必需性；真实 macOS runner 对照安全 PR、打包输入 PR、现有包干净 dist 与负样本。输入、packaging、release 检查职责分层，遵守 `openspec/CODE_STANDARDS.md`，不新增运行时依赖。

## Risks / Trade-offs

手工更新包专用白名单前需审计间接资源依赖。测试 launch 与本地包体只读检查有 GUI runner 依赖；直接 exec 不覆盖 Finder/Gatekeeper。macOS runner 无法验证不得宣称完整宿主通过。
