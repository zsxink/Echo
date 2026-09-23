## ADDED Requirements

### Requirement: 应用禁用 WebView 默认右键菜单
桌面应用 SHALL 在应用工作区中阻止 WebView 的默认上下文菜单，避免浏览器菜单干扰 Echo 的桌面交互；歌曲列表的行内菜单行为由 `library-experience` 定义。

#### Scenario: 在应用工作区右键
- **WHEN** 用户在 Echo 工作区任意位置触发右键
- **THEN** 系统不显示 WebView 默认上下文菜单
