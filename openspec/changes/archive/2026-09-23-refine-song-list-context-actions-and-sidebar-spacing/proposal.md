## Why

歌曲行的「+」目前把歌曲追加到队列，与用户期待的“下一首播放”不同；右键歌曲行也未在普通模式下提供与三个点相同的菜单。全局原生右键菜单和侧边导航留白进一步打断了桌面曲库操作的一致性。

## What Changes

- 将歌曲列表行内「+」改为调用既有“下一首播放”语义。
- 全局拦截 WebView 默认右键菜单；歌曲行右键打开该歌曲的操作菜单，多选模式沿用现有批量菜单规则。
- 适度收紧侧边导航栏的内边距，同时保持导航项可识别和可操作。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `library-experience`：歌曲行的下一首播放入口与普通模式行右键菜单行为。
- `desktop-app-shell`：应用禁用 WebView 默认右键菜单。

## Impact

- 影响桌面端 React 歌曲列表、单曲菜单及应用壳事件处理。
- 影响桌面端侧边导航样式；不改变 Core、Tauri IPC、持久化格式或播放器接口。
- 产品界面基准参见 `docs/PRODUCT.md`、`docs/ROADMAP.md`、`docs/interface-terminology.md` 和 `docs/prototype/echo-desktop-player.html`；实现遵循 `docs/DESIGN.md` 与 `openspec/CODE_STANDARDS.md` 的桌面 presentation 分层约束。
