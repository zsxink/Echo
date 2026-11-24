## Why

[#173](https://github.com/zsxink/Echo/issues/173)：macOS 完整打包在 #172 取样耗时 8:46。只改动经过审计且不能影响产物的文件时，不需要重新生成整个应用。发版必须检查用户最终下载的已签名 DMG，而不是旁边新构建或重签的 App。

## What Changes

- PR 保持稳定 macOS Platform job；只有完整差异能明确证明所有修改均为非打包输入时才跳过重步骤。main、混合、添加/删除/重命名、未知、失败全跑。独立的 macOS libmpv 格式 smoke 保留。
- 拆分 1.9 构建/临时签名和只读的现有 App 检查。发版从最终 DMG 只读挂载，验证签名、文件关联、架构/依赖、嵌入前端和单实例交接，并在同一挂载 App 上跑 1.10/9.7。
- 只有独立验证无 dist 时 hook 自建后，才移除 macOS 打包前的显式前端 build。仅约束此路径，不改直接 Cargo 或其他平台。
- 保留默认 main 包构建、50k 策略、审计和其他性能场景原状。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `engineering-governance`：已审核 PR 的打包分层、现有包和发布产物证明。

## Impact

只改 CI、发布验证适配层和 OpenSpec，不改产品协议、应用包内容、签名流程或运行时行为。
