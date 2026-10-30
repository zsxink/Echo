# stabilize-windows-installer — 设计

## Context

v0.3.0 的 Windows 产物 `Echo_0.3.0_x64-setup.exe`（Tauri 2.11 / NSIS）在「无 WebView2 运行时」的 Windows 10 上安装失败（issue #64）。对产物与上游模板的静态分析结论：

- `tauri.conf.json` 无 `bundle.windows` 节，全部走 bundler 默认：`webviewInstallMode = downloadBootstrapper{silent:true}`、`nsis.languages = ["English"]`、`installMode = currentUser`。
- WebView2 缺失时，安装器模板用 `NSISdl` 插件在线下载 bootstrapper。`NSISdl` 只支持明文 HTTP（NSIS 官方 ReadMe 明言；插件导入表仅 `WSOCK32`，无 TLS 能力），对 `https://go.microsoft.com/fwlink/?LinkId=2124703` 实际走 80 端口明文 + 301 重定向链。该链路在过滤明文 HTTP/强制 HTTPS 代理/离线的网络下失败，安装器 `Abort`，且报错为英文。
- 即便 bootstrapper 下载成功，它以 `/silent /install` 运行时仍需再下载完整运行时（100 MB+），失败同样中止。
- NSIS 模板无任何 Windows 版本门槛；`docs/ROADMAP.md` 9.8 的 Win10 22H2 基线未在安装包层面落实。
- 仓库已有平台覆盖文件 `apps/desktop/src-tauri/tauri.windows.conf.json`（现承载 `bundle.resources`），Windows 专属 bundle 配置应放这里而非主配置。

本变更不触碰 Core/desktop 的 Rust 代码与前端，影响面集中在打包配置、NSIS hooks、文档与 verify 门禁。

## Goals / Non-Goals

**Goals:**

- 无 WebView2 的 Windows 10 上安装成功率与失败可读性显著提升：安装期不再依赖明文 HTTP 下载 bootstrapper；失败提示对中英文用户可读。
- 已装但过旧的 WebView2 在安装期触发升级，而不是留到启动期白屏。
- 低于 Windows 10 的系统得到明确的双语终止提示，而非模糊失败或静默装完无法启动。
- Windows 最低支持口径（Win10 22H2 / 11 x64 实测基线）在 README（中英）显式化。
- 上述安装器契约有静态回归门禁，配置被改回默认值时 CI 失败。

**Non-Goals:**

- 代码签名 / 公证（SmartScreen 与杀软误报的根治手段，属独立成本决策，不随本变更引入）。
- `offlineInstaller`（+127 MB）与 `FixedRuntime`（+180 MB、失去 Evergreen 更新）两种全离线/锁版本方案；`embedBootstrapper` 落地后若仍有离线场景诉求再评估。
- 应用 UI 自身的 i18n。
- Windows ARM64 / x86 产物。
- 运行时（应用启动期）的 WebView2 缺失引导——安装器兜底到位后，剩余场景（用户卸载 WebView2）由 WebView2 Loader 自身报错，暂不扩展。

## Decisions

### D1: WebView2 兜底选 `embedBootstrapper`，配置放 `tauri.windows.conf.json`

`bundle.windows.webviewInstallMode` 设为 `{ "type": "embedBootstrapper", "silent": true }`，写入平台覆盖文件（与现有 `bundle.resources` 同层新增 `windows` 节），主配置保持平台中立。

- 对比 `downloadBootstrapper`（现状默认）：消除安装期第一跳网络依赖（NSISdl 明文下载），安装包 +约 1.8 MB；
- 对比 `offlineInstaller`：+127 MB 使 41 MB 的 setup 翻倍以上，代价不成比例；bootstrapper 内嵌后离线失败面已收窄到「无 WebView2 且完全无网」，该场景留给 Release 说明引导手工安装；
- 对比 `FixedRuntime`：锁定版本失去 Evergreen 安全更新，仅适用于企业锁版场景。

内嵌的 bootstrapper 运行时仍需联网下载完整运行时——这是微软官方组件自身的行为（含重试与错误码），Tauri 模板对它的非零退出以双语文案 Abort，失败可读。

### D2: NSIS 双语 `["SimpChinese", "English"]`，不弹语言选择框

NSIS 按 OS 语言自动匹配，未命中时回退列表首语言。`SimpChinese` 放首位：未知/小语种系统兜底中文，与产品主要受众一致；英文系统自动英文。`displayLanguageSelector` 保持 `false`（默认），避免多一步对话框。Tauri bundler 内置官方 `SimpChinese.nsh` 翻译（覆盖 `webview2Downloading`/`webview2AbortError` 等自定义文案，已核对 2.11.4 `get_lang_data` 映射），无需 `customLanguageFiles`。

### D3: 最低系统门槛用 `installerHooks` 的 PREINSTALL 钩子，而非自定义模板

`bundle.windows.nsis.installerHooks` 指向新增 `apps/desktop/src-tauri/windows-installer-hooks.nsh`，内容：

```nsis
!include "WinVer.nsh"   ; 主模板未引入，hooks 自行引入（NSIS 头有防重入保护）

!macro NSIS_HOOK_PREINSTALL
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "Echo 需要 Windows 10 或更高版本。$\r$\nEcho requires Windows 10 or later."
    Abort
  ${EndIf}
!macroend
```

- 对比自定义 `NsisConfig.template`：整包替换模板会冻结上游演进（模板近年多次修复，如 WebView2 下载返回值处理），维护成本高；hooks 只注入宏，Tauri 升级兼容面最小。
- 门槛语义：只拦 Win10 以下（Win7/8/8.1——WebView2 官方已停止支持的系统）。**不**按 build 号拦 Win10 早期版本（1607–21H2）：22H2 是「实测基线」而非能力下限，过早拦截会误伤可正常运行的用户。README 口径据此表述。
- 已知边界：PREINSTALL 在向导页之后、复制文件之前执行，低版本用户会先走完向导再被拦；文案双语、含版本要求，满足「明确提示而非模糊失败」。静默安装（`/S`）下 `Abort` 直接以非零退出码结束，脚本化安装可感知。

### D4: `minimumWebview2Version` 取 `109.0.1518.78`

WebView2 109 是微软支持 Win7/Win8 的最后一个主线版本，也是普遍公认的最低安全线。低于它的安装（典型：长期断更/企业冻结的机器）在安装期经 EdgeUpdate 触发一次升级；按 Tauri 模板语义，升级失败弹「中止/重试/忽略」，用户可忽略继续安装（此时回落到「已装旧版仍可尝试运行」）。取值原则：宁低勿高——设得过高会让大量正常 Evergreen 用户在安装期被迫下载 100 MB+ 升级。

### D5: 回归门禁注册为 verify 任务 17.1，挂 Windows 平台 CI job

新增 `scripts/verify/checks/task-17.1.mjs`：静态断言合并平台覆盖后的 `bundle.windows` 契约——`webviewInstallMode.type === "embedBootstrapper"`、`nsis.languages` 恰为 `["SimpChinese","English"]`（顺序也断言，首语言即兜底语言）、`minimumWebview2Version` 存在、`installerHooks` 路径存在且含 `AtLeastWin10` 与 `NSIS_HOOK_PREINSTALL`。检查必须「可失败」：对缺节/回归默认值/文件缺失各给出明确错误退出。注册进 `scripts/verify/manifest.json`（17.x 新段），并加入 `.github/workflows/ci.yml` Windows 平台 job 的 `verify:task` 列表（与 9.7/9.8 同跑）。遵守治理契约（14.5/14.8：注册的 gate 观察真实事实且真的会失败）。

## Risks / Trade-offs

- [安装包 +约 2 MB] → 41 MB 量级下可接受；SHA256SUMS 随发布更新，用户侧无感知差异。
- [bootstrapper 下载完整运行时仍需联网，「无 WebView2 且完全离线」依然装不完整] → Release 说明与 README 引导手工下载 WebView2 Runtime（固定版本链接），属于可执行兜底；不在安装包内再叠加 127 MB。
- [hooks 方案依赖上游保留 `NSIS_HOOK_PREINSTALL` 宏名] → 宏属 Tauri 公开配置契约（`NsisConfig.installerHooks` 文档化行为），升级 Tauri 时由 17.1 门禁在 CI 提前暴露断裂。
- [Win10 早期版本（1607–21H2）不拦截，可能存在未实测路径] → README 声明实测基线 22H2；早期版本失败按普通 issue 处理，避免把门槛做成误伤面。
- [`minimumWebview2Version` 触发的升级在企业代理下可能失败] → 模板语义允许「忽略」继续安装，不阻断。
- [双语文案中的 Tauri 自定义部分依赖上游 `SimpChinese.nsh` 质量] → 仅覆盖安装器流程文案，风险低；确有翻译问题时可用 `customLanguageFiles` 单点覆盖，不动其它决策。

## Migration Plan

1. 配置与 hooks 一次合入（无存量数据迁移）；下一个 tag 的 Release 产物即带内嵌 bootstrapper 与双语安装器。
2. 回滚 = revert 配置节与 hooks 文件；17.1 门禁随配置一同回滚，不留悬挂检查。
3. Release 说明模板（`docs/` 或发布正文）补充 Windows 最低系统要求与 SmartScreen 放行指引，随下个版本生效。

## Open Questions

（无——`minimumWebview2Version` 具体取值如需调整，在实现阶段以注释形式记录理由即可，不改变契约。）
