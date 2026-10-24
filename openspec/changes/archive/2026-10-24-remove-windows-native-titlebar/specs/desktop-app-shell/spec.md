## ADDED Requirements

### Requirement: Windows 无边框窗口与自绘窗口控件

Windows 上主窗口 SHALL 不绘制原生系统标题栏（`decorations: false`），窗口内容 SHALL 自窗口顶边开始布局，与 macOS overlay 标题栏的视觉一致。应用 SHALL 在窗口右上角渲染仅 Windows 可见的自绘窗口控件：最小化、最大化/还原、关闭。最大化/还原图标 SHALL 随窗口实际最大化状态切换。关闭按钮 SHALL 与原生关闭同路径（`CloseRequested → CloseBehavior`），不新增关闭行为特例。窗口拖拽与双击最大化/还原 SHALL 继续由顶部拖拽条（`data-tauri-drag-region`）承担。窗口命令 SHALL 经 bridge 统一封装调用，WebView capability SHALL 仅为此新增 `core:window:allow-minimize`、`core:window:allow-toggle-maximize`、`core:window:allow-close` 三个显式权限，不得引入特权权限族。macOS 与 Linux 的窗口外观与交互 SHALL 不受影响（Linux 保留原生装饰）。Windows 平台配置中的主窗口定义 SHALL 与基础配置保持一致，除 `decorations` 外不得漂移。

#### Scenario: Windows 无原生标题栏且内容从顶边开始
- **WHEN** 用户在 Windows 启动 Echo
- **THEN** 窗口顶部不出现带应用名与系统按钮的原生标题栏，工作区与侧边栏自窗口顶边的安全区以下开始布局，窗口仍可经顶部拖拽条移动、经系统边缘缩放、拖拽贴边与 Aero Snap

#### Scenario: 自绘窗口控件三键可用且图标随状态切换
- **WHEN** Windows 用户依次点击自绘控件的最小化、最大化/还原、关闭
- **THEN** 最小化将窗口收进任务栏；最大化/还原每次切换窗口状态且图标在最大化与还原图形间切换；关闭经与原生标题栏相同的 CloseRequested → CloseBehavior 路径隐藏到托盘或退出

#### Scenario: 双击拖拽区最大化与还原
- **WHEN** Windows 用户双击顶部拖拽条
- **THEN** 窗口在最大化与还原之间切换，行为与原生标题栏双击一致

#### Scenario: 非 Windows 不渲染自绘控件
- **WHEN** 应用运行在 macOS、Linux 或浏览器预览（无 Tauri WebView）
- **THEN** 界面不渲染自绘窗口控件，macOS 保留 overlay 标题栏拖拽条与红绿灯按钮，Linux 保留原生标题栏

#### Scenario: 窗口命令权限最小且配置不漂移
- **WHEN** 在任一平台运行权限与配置一致性测试
- **THEN** 主窗口 capability 恰为基础集合加三个窗口命令权限且无特权族；Windows 平台配置的主窗口定义与基础配置逐字段一致且 `decorations` 为 `false`
