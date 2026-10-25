## 1. 打包配置

- [x] 1.1 在 `apps/desktop/src-tauri/tauri.windows.conf.json` 的 `bundle` 下新增 `windows` 节：`webviewInstallMode: { "type": "embedBootstrapper", "silent": true }`、`minimumWebview2Version: "109.0.1518.78"`、`nsis: { "languages": ["SimpChinese", "English"], "installerHooks": "windows-installer-hooks.nsh" }`（hooks 路径相对 src-tauri 解析；主配置 `tauri.conf.json` 保持平台中立不动）
- [x] 1.2 新增 `apps/desktop/src-tauri/windows-installer-hooks.nsh`：自引入 `WinVer.nsh`，定义 `NSIS_HOOK_PREINSTALL` 宏——`${IfNot} ${AtLeastWin10}` 时中英双语 `MessageBox MB_OK|MB_ICONSTOP` 提示后 `Abort`（见 design D3 的宏体）
- [x] 1.3 验证配置合法：`node --input-type=module` 读取并 `JSON.parse` 两份 Tauri 配置确认可解析、`bundle.windows` 节合并语义正确（Windows 平台覆盖仅追加键）

## 2. 文档声明

- [x] 2.1 `README.md` 新增系统要求：Windows 最低 Windows 10、实测基线 Windows 10 22H2 / 11 x64、产物仅 x64、未签名安装包的 SmartScreen「仍要运行」放行指引
- [x] 2.2 `README.en.md` 补充与中文版口径一致的英文说明
- [x] 2.3 验证：两份 README 的最低版本与 x64-only 关键词由 17.1 门禁断言（见 3.1）

## 3. 回归门禁

- [x] 3.1 新增 `scripts/verify/checks/task-17.1.mjs`：静态断言合并平台覆盖后的安装器契约——`webviewInstallMode.type === "embedBootstrapper"` 且 `silent === true`、`nsis.languages` 恰为 `["SimpChinese","English"]`（含顺序）、`minimumWebview2Version` 非空、`nsis.installerHooks` 指向的文件存在且同时包含 `NSIS_HOOK_PREINSTALL` 与 `AtLeastWin10`、`README.md` 与 `README.en.md` 均含最低版本与 x64 声明；任一断言失败时输出具体缺失项并以非零码退出
- [x] 3.2 `scripts/verify/manifest.json` 注册 `17.1`（新 17.x 段，标题概括契约），`.github/workflows/ci.yml` 的 Windows 平台 job `verify:task` 列表加入 `17.1`（与 `9.7 9.8` 同跑）
- [x] 3.3 门禁自证可失败：临时移除 `webviewInstallMode`（回归默认值）确认 `node scripts/verify/checks/task-17.1.mjs` 以非零码失败并指明该项，再改动 `languages` 顺序复验一次，恢复后门禁通过
- [x] 3.4 验证：`node scripts/verify/checks/task-17.1.mjs` 与 `pnpm verify:task -- 17.1` 均通过；`pnpm verify:governance -- --without-injection` 不因 manifest 注册问题失败

## 4. 产物冒烟与收尾

- [x] 4.1 本地 Windows 构建 NSIS 冒烟：`pnpm --dir apps/desktop tauri build --bundles nsis` 成功产出 `target/release/bundle/nsis/*.exe`（注意不能写 `pnpm echo release -- --bundles nsis`：字面 `--` 会被透传给 cargo 报「unexpected argument」）；用 7-Zip 列表断言（a）产物内嵌 `MicrosoftEdgeWebview2Setup.exe`（`$PLUGINSDIR`，约 2 MB），（b）产物不再包含 `NSISdl.dll`（`downloadBootstrapper` 分支未编译的负向证据），（c）`Echo.exe`、`libmpv-2.dll`、`vulkan-1.dll`、NOTICE 与 v0.3.0 一致仍在
- [x] 4.2 手动双击安装一次确认：中文 Windows 上安装向导为简体中文、安装/卸载流程完整（本机有 WebView2，运行时段应直接跳过）
- [x] 4.3 在 issue #64 回帖补充根因分析结论（NSISdl 明文 HTTP、默认仅英文、无版本门槛）与本变更修复口径，说明修复随下个 tag 发布
- [x] 4.4 收尾验证：`pnpm format` / `pnpm lint` 通过；变更目录通过 `openspec validate --strict`（如 CLI 支持），准备归档
