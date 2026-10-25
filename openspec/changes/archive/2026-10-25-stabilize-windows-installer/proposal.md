# stabilize-windows-installer

## Why

用户反馈在 Windows 10 上无法安装 v0.3.0 的 `Echo_0.3.0_x64-setup.exe`（issue #64）。对发布产物的静态分析证实根因在打包配置完全走 Tauri 默认值：WebView2 缺失时安装器用仅支持明文 HTTP 的 `NSISdl` 插件在线下载 bootstrapper（HTTPS 端点、明文回退链在受限网络下失败即中止安装）；安装器只有英文文案，中文用户看不懂失败原因，只能笼统反馈「装不上」；同时没有任何最低系统版本声明与提示，`docs/ROADMAP.md` 9.8 的 Win10 22H2 基线只存在于文档。需要让安装器在无 WebView2 的 Windows 10 上确定可装、失败可读，并把系统要求显式化。

## What Changes

- **WebView2 兜底内嵌化**：`bundle.windows.webviewInstallMode` 由默认 `downloadBootstrapper` 改为 `embedBootstrapper{silent:true}`，安装包体积 +约 2 MB，去掉安装期对 `go.microsoft.com` 的在线下载跳转；bootstrapper 仍按需下载完整运行时（联网环境行为不变，离线/受限网络的失败面显著收窄）。
- **WebView2 版本基线**：新增 `bundle.windows.minimumWebview2Version`，已装但过旧的运行时在安装期触发升级（升级失败允许忽略继续，与 Tauri 模板语义一致），而不是留到启动期白屏。
- **安装器中英双语**：`bundle.windows.nsis.languages = ["SimpChinese", "English"]`，自动跟随系统语言、兜底中文；Tauri 自带安装文案（含 WebView2 下载/安装失败提示）使用官方 `SimpChinese.nsh` 翻译，无需自定义语言文件。
- **最低系统版本门槛**：通过 `bundle.windows.nsis.installerHooks` 的 PREINSTALL 钩子检查 Windows 10 起步（WinVer.nsh `${AtLeastWin10}`），低于 Win10 时以中英双语 MessageBox 明确终止安装；README（中英）与 Release 说明声明 Windows 最低支持与实测基线（Win10 22H2 / 11 x64，与 ROADMAP 9.8 对齐）。
- **配置回归门禁**：verify 体系新增静态检查，断言 `tauri.conf.json` 的 Windows bundle 配置（embedBootstrapper、双语 languages、minimumWebview2Version、installerHooks）存在且未被回归为默认值。

非目标：代码签名/公证（独立成本决策）、`offlineInstaller` 全离线安装包（+127 MB，可后续按需评估）、应用 UI 自身的 i18n、Windows ARM64/x86 产物。

## Capabilities

### New Capabilities

- `windows-installer`: Windows NSIS 安装器的交付契约——WebView2 运行时兜底与版本基线、安装器双语、最低系统版本提示，以及上述配置的回归门禁。

### Modified Capabilities

（无。`ci-release-pipeline` 的「三平台产物矩阵」对 Windows 产出的既有要求——NSIS `.exe`、命名含应用名与版本——不变；`platform-libmpv-supply-chain` 涉及的 Windows 播放库装载契约不受影响。）

## Impact

- `apps/desktop/src-tauri/tauri.conf.json`：新增 `bundle.windows` 配置节（`webviewInstallMode` / `minimumWebview2Version` / `nsis.languages` / `nsis.installerHooks`）。
- `apps/desktop/src-tauri/`：新增 NSIS installer hooks `.nsh` 文件（Win10 门槛 + 中英双语提示，自引入 `WinVer.nsh`）。
- `README.md` / `README.en.md`：新增系统要求说明（最低 Windows 10、实测基线 22H2/11 x64、x64-only、安装包未签名的 SmartScreen 放行指引）。
- `scripts/verify/checks/`：新增配置门禁检查脚本；`scripts/verify/manifest.json` 注册新任务（17.x）。
- Release 产物：Windows `setup.exe` 体积 +约 2 MB；安装期网络依赖从「NSISdl 明文下载 bootstrapper + bootstrapper 再下载运行时」收敛为「内嵌 bootstrapper + 仅按需下载运行时」。
- 关联 issue：直接修复 #64；#54（Windows 播放界面异常）不在本变更范围。
