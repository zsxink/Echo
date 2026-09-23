## 1. 歌曲列表操作语义

- [x] 1.1 更新共享歌曲行及 SongList 类型/回调，让加号调用现有 play-next 回调；接入曲库和歌单工作区并保留菜单的“加入播放队列”操作。通过检查各工作区将 play-next handler 传入行，并保留菜单 enqueue handler 确认语义区分。[library-experience: 歌曲行提供一致的下一首播放与操作菜单入口]
- [x] 1.2 将普通模式行右键接入与三个点相同的单曲菜单状态和组件；保留多选模式的批量菜单及选择行为。通过检查三个工作区将行右键接入 SongMenu 状态和多选菜单分支确认行为一致。[library-experience: 歌曲行提供一致的下一首播放与操作菜单入口]
- [x] 1.3 在应用壳统一阻止 WebView 默认 contextmenu，并确保事件监听生命周期被清理。检查全局监听注册、preventDefault 和卸载时移除逻辑。[desktop-app-shell: 应用禁用 WebView 默认右键菜单]

## 2. 导航视觉间距

- [x] 2.1 适度压缩侧边导航现有 padding，不缩小文字、图标和交互命中区域；检查 CSS 变更限定在侧边导航布局样式。

## 3. 验证与交付

- [x] 3.1 按代码规范运行桌面端格式、lint、类型检查和构建命令（不新增或运行测试）；格式检查、lint（0 错误/6 条既有 warning）、类型检查及生产构建均通过。
- [x] 3.2 运行 `openspec validate refine-song-list-context-actions-and-sidebar-spacing --strict` 并复核变更与 issue #20 验收范围一致。
