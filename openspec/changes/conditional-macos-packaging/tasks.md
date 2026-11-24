## 1. CI 打包分层

- [x] 1.1 实现独立分类与明确 skip 日志，保持默认 main 和独立原生 smoke。
- [x] 1.2 正反对照安全既有内容、未知/CSS、混合、变更模式与 Git 失败；结构注入和默认 gating 验证。

## 2. 发布现有包严格验证

- [x] 2.1 拆分构建和 App 验证；严格嵌入前端校验与隔离交接。
- [x] 2.2 发布只读挂载实际上传的唯一 DMG，验签并对其中 App 跑全套检查；完整保留签名。
- [ ] 2.3 验证挂载、关闭、Fail-closed 边界；在 Mac 对照清洁 dist 真实打包，只有证实 hook 构建后移除前置步骤。

## 3. 验证及交付

- [x] 3.1 运行 Node tests、task 1.5、governance registry/static、目标 injection、OpenSpec 严格检查及 git diff check；区分未验证的真实发布 Gate。
- [ ] 3.2 完成任务后同步并归档，不把本地替身或确认失败的 Mac 发布路径记为通过。
