## Why

用户从“添加到歌单”选择器打开“新建歌单”后，名称输入框无法稳定获得焦点，导致不能输入名称并中断添加歌曲流程。该常用入口需要与独立创建歌单入口一样支持键盘和中文输入法。

## What Changes

- 明确嵌套歌单选择器与命名对话框之间的焦点所有权：打开命名对话框后，名称输入框可点击、可输入并可完成中文输入法组合；关闭后焦点回到选择器。
- 为从选择器创建歌单的完整交互补充焦点回归覆盖，并修复触发抢焦或失焦的行为。
- 不改变歌单创建、命名校验、选择新歌单及后续添加歌曲的业务规则。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `playlist-management`：明确从歌单选择器打开新建对话框时输入框焦点、输入和返回焦点的行为。

## Impact

- 影响 `apps/desktop/src/features/playlists/AddToPlaylistDialog.tsx`、`PlaylistNameDialog.tsx` 及其 React 测试；若调查确认焦点栈存在通用缺陷，再最小范围调整 `apps/desktop/src/app/overlays.ts`。
- 不改变 IPC、Core、数据库或持久化格式，不新增依赖。
- 符合一期桌面播放器的歌单管理范围；界面术语遵循 `docs/interface-terminology.md`，焦点交互参考现有覆盖层栈约定。
- 关联用户反馈：[Issue #30](https://github.com/zsxink/Echo/issues/30)。
