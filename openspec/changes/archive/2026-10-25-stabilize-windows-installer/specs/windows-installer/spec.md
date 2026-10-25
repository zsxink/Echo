# windows-installer 增量规格

## ADDED Requirements

### Requirement: WebView2 运行时兜底内嵌

Windows 安装器 MUST 内嵌 WebView2 bootstrapper（`embedBootstrapper`，静默模式）。目标机器缺失 WebView2 运行时时，安装器 MUST 使用内嵌的 bootstrapper 完成运行时安装，MUST NOT 在安装器自身层面发起在线下载 bootstrapper（即不得回归 `downloadBootstrapper`）。bootstrapper 或运行时安装失败时，安装器 MUST 以跟随系统语言的文案中止安装，不得静默继续。

#### Scenario: 无 WebView2 的机器安装

- **WHEN** 在未安装 WebView2 运行时的 Windows 10 x64 机器上运行安装器
- **THEN** 安装器使用内嵌 bootstrapper 安装运行时，全程不发起安装器自身的在线下载跳转

#### Scenario: 已安装 WebView2 的机器安装

- **WHEN** 在已有 WebView2 运行时的机器上运行安装器
- **THEN** 安装器跳过运行时安装段，按常规流程完成安装

#### Scenario: 运行时安装失败

- **WHEN** bootstrapper 安装运行时失败（如完全离线）
- **THEN** 安装器以系统语言对应的文案（中文或英文）中止安装并提示原因

### Requirement: WebView2 版本基线

安装器 MUST 声明最低 WebView2 运行时版本（初始基线 `109.0.1518.78`）。目标机器已装版本低于基线时，安装器 MUST 在安装期触发一次运行时升级；升级失败时 MUST 允许用户选择忽略并继续安装。

#### Scenario: 过旧运行时触发升级

- **WHEN** 机器上已装 WebView2 版本低于声明的基线
- **THEN** 安装器在安装期触发运行时升级

#### Scenario: 升级失败可忽略

- **WHEN** 运行时升级失败且用户选择「忽略」
- **THEN** 安装继续，不被强制中止

### Requirement: 安装器中英双语

Windows 安装器界面与全部提示（含 WebView2 相关失败文案）MUST 提供简体中文与英文两种语言，自动跟随系统语言；系统语言不在支持列表内时 MUST 回退简体中文。

#### Scenario: 中文系统

- **WHEN** 在系统语言为简体中文的机器上运行安装器
- **THEN** 安装器界面与提示为简体中文

#### Scenario: 英文系统

- **WHEN** 在系统语言为英文的机器上运行安装器
- **THEN** 安装器界面与提示为英文

#### Scenario: 其它语言系统回退

- **WHEN** 在系统语言不在支持列表内的机器上运行安装器
- **THEN** 安装器界面与提示为简体中文

### Requirement: 最低系统版本门槛

Windows 安装器 MUST 在复制文件前检查系统版本：低于 Windows 10 时 MUST 以中英双语提示明确终止安装；Windows 10 及以上 MUST 正常继续；静默安装模式下终止 MUST 体现为非零退出码。门槛不得按 build 号进一步拦截 Windows 10 早期版本（1607–21H2）。

#### Scenario: 低于 Windows 10 终止

- **WHEN** 在 Windows 8.1 及更早系统上运行安装器
- **THEN** 安装器弹出中英双语版本提示后终止，不复制任何文件

#### Scenario: Windows 10 正常安装

- **WHEN** 在 Windows 10（任意 build）及以上系统上运行安装器
- **THEN** 版本检查通过，安装流程正常继续

#### Scenario: 静默安装的版本门槛

- **WHEN** 在低于 Windows 10 的系统上以静默模式（`/S`）运行安装器
- **THEN** 安装器以非零退出码结束

### Requirement: Windows 系统要求公开声明

项目 README 的中文版与英文版 MUST 声明 Windows 最低支持为 Windows 10、实测基线为 Windows 10 22H2 / 11 x64、产物仅提供 x64，并包含未签名安装包的 SmartScreen 放行指引。

#### Scenario: 双语 README 声明一致

- **WHEN** 查看 `README.md` 与 `README.en.md`
- **THEN** 两者均包含 Windows 最低版本、x64-only 与 SmartScreen 指引的对应说明

### Requirement: 安装器配置回归门禁

verify 体系 MUST 包含针对 Windows 安装器契约的静态门禁：断言合并平台覆盖后的 `bundle.windows.webviewInstallMode` 为 `embedBootstrapper`、`nsis.languages` 恰为 `["SimpChinese","English"]`（含顺序，首语言即兜底语言）、`minimumWebview2Version` 已声明、`installerHooks` 指向的文件存在且包含 Win10 门槛宏。任一断言不满足时门禁 MUST 失败并指出具体缺失项，且门禁 MUST 在 CI 的 Windows 平台验证路径中执行。

#### Scenario: 契约齐全时通过

- **WHEN** `tauri.windows.conf.json` 与 hooks 文件满足全部断言
- **THEN** 门禁通过

#### Scenario: 回归默认值时失败

- **WHEN** `webviewInstallMode` 被移除（回落 `downloadBootstrapper` 默认值）或 `languages` 顺序/内容被改动或 hooks 文件缺失
- **THEN** 门禁失败，输出指明第一个不满足的契约项
