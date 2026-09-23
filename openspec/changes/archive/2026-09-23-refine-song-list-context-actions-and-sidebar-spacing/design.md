## Context

参见 `proposal.md` 的动机和 `specs/library-experience/spec.md`、`specs/desktop-app-shell/spec.md` 的行为约束。歌曲列表由共享的 `SongList`/`SongRow` 呈现，曲库和歌单工作区各自装配单曲菜单、批量菜单和播放命令；应用框架由 React 桌面壳装配。此次变更只在桌面 presentation 层完成。

## Goals / Non-Goals

**Goals:**

- 复用现有单曲下一首播放命令，令行内加号与菜单项行为一致。
- 在曲库和歌单歌曲列表中，用相同菜单组件响应三个点和行右键，并保留多选模式既有逻辑。
- 在应用壳层拦截 WebView 原生 `contextmenu` 默认行为。
- 小幅收紧侧边导航区域 padding。

**Non-Goals:**

- 不改变播放队列插入、去重、排序或错误反馈规则。
- 不改变菜单项目、批量选择语义、Tauri IPC 或 Core。
- 不调整表格密度、字体、导航项尺寸或窄屏断点。

## Decisions

1. **复用现有队列操作和单曲菜单。** 将共享行组件的加号回调由 enqueue 替换为 play-next，并沿 `SongList` props 传至曲库/歌单已有的 play-next 回调。右键在普通模式触发同一个 `menuFor` 状态和 `SongMenu` 渲染路径；多选模式继续使用现有 context-menu 分发至批量菜单。这样避免在行组件复制菜单操作或创建新的队列语义。替代方案是复制菜单项逻辑到行组件，会造成不同列表表面的操作和禁用条件漂移。
2. **在应用壳拦截默认菜单。** 在根应用生命周期注册并清理一个 `contextmenu` 监听器，统一调用 `preventDefault()`。行级 React 事件仍负责打开菜单；桌面组件继续通过既有 `MenuAnchor`、`usePlacement` 和 overlay stack 管理定位、焦点和关闭。该改动不跨越 UI 与 Core 边界，也不新增依赖。替代方案是在每个可交互容器分别拦截，覆盖不完整且重复。
3. **只收紧侧边导航自身的留白。** 调整现有侧边导航样式的水平/垂直 padding token 或规则，优先复用现有尺寸变量，不改 DOM 结构和导航项目命中区域；按截图减少明显空白并保留布局层级。

## Risks / Trade-offs

- [统一禁用原生菜单会影响工作区中的文本/WebView 原生右键动作] → 这是桌面播放器期望行为；用户明确要求全局禁用默认右键菜单，Echo 的歌曲操作由应用菜单提供。
- [共享行组件在不同列表中的队列回调容易漏改] → 更新 `SongList` 的类型与全部调用点，并覆盖曲库及歌单列表的操作路径。
- [压缩 padding 可能让窄屏导航显得拥挤] → 只调整已有 padding，不缩小图标、字体或交互目标，并检查窄屏布局。

## Migration Plan

纯前端行为与样式变更，无数据迁移或 API 迁移。回滚时还原桌面 UI 修改即可。

## Open Questions

无。
