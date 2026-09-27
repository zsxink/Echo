## Context

见 `proposal.md` 的问题说明和 `specs/playlist-management/spec.md` 的新增焦点场景。现有交互在同一 React 子树中并置歌单选择器与命名对话框：`PlaylistNameDialog` 负责命名输入，`useOverlay` 负责打开时的初始焦点及卸载后的恢复，`useFocusTrap` 限制 Tab 导航。集成测试复现了失焦反馈：`AddToPlaylistDialog` 在歌单列表异步加载完成时重渲染，并为命名弹窗重新创建内联 `onClose` 回调；`useOverlay` 将回调身份变化作为 effect 依赖，因而先执行清理、恢复焦点并触发输入框的 `onBlur` 校验，再重新注册。空名称由此显示错误，焦点短暂离开输入框。另一个恢复问题来自 React `autoFocus` 在 overlay effect 记录打开前焦点前先聚焦输入框，使 hook 错把输入框记录为触发元素。

本变更属于桌面 UI presentation 层，仅协调已有弹窗的焦点所有权。Core、Tauri command、IPC、数据库和跨平台业务边界均不受影响。实现遵守 `docs/DESIGN.md` 的 UI 编排边界和 `openspec/CODE_STANDARDS.md` §6、§8、§9。

## Goals / Non-Goals

**Goals:**

- 用从选择器打开命名弹窗的集成测试验证名称输入框可点击、保持焦点并接收文本及输入法组合输入。
- 验证关闭子弹窗后焦点回到“新建歌单”入口，选择器已有状态保留。
- 根据可复现的失败路径，最小化修正焦点 trap/恢复时序，并覆盖回归。

**Non-Goals:**

- 重写全局浮层栈或改变其他浮层的关闭优先级。
- 改变创建/校验/添加歌曲业务逻辑、样式或无障碍语义。
- 修改 IPC、Core、持久化格式或添加依赖。

## Decisions

1. **以真实嵌套交互为验证边界。** 测试通过渲染 `AddToPlaylistDialog`、在子弹窗打开期间完成父组件列表加载，再点击名称输入框、输入及取消；单测 `PlaylistNameDialog` 本身不足以捕捉父组件重渲染。检查焦点和错误状态，并验证输入法 composition 事件开始与结束间文字可被接收。
2. **使 overlay 注册独立于关闭回调身份。** `useOverlay` 将最新 `onClose` 保存在 ref 中，由稳定注册读取；effect 不因内联回调变化而清理/恢复焦点。这样每个 overlay 生命周期只记录一次真正的触发元素，父组件刷新数据时不会短暂把子弹窗焦点还给选择器。备选方案是要求每个调用方 memoize `onClose`，但 27 个调用点容易遗漏，修正应在负责生命周期的 hook 内完成。
3. **用 overlay 统一初始聚焦与恢复。** 去掉命名输入框上重复的 React `autoFocus`，由 `useOverlay` 在记录当前焦点后聚焦首个可聚焦控件；关闭时即可将焦点还给新建触发按钮。命名弹窗打开时父选择器继续注册但暂停自身 Tab trap，子级 `BlockingDialog` 保持 Escape 优先级。不在歌单组件中另建事件监听器或通用弹窗系统。

兼容性：无外部 API、数据格式或平台兼容变化。设计使用既有 overlay 栈，维持 BlockingDialog 高于 Picker 的 Escape 规则。

## Risks / Trade-offs

- [模拟 DOM 不完整反映原生输入法焦点行为] → 测试明确覆盖 `compositionstart` / 输入 / `compositionend` 和焦点状态；若当前测试环境无法可信断言，增加项目已有的桌面端浏览器/E2E 场景，不用未验证的事件模拟替代真实行为。
- [共享焦点管理修改影响其他弹窗] → 限定变更范围，并运行 overlay 单测及 playlist 集成测试。

## Migration Plan

无数据或接口迁移。随桌面应用正常发布；若修复引入其他浮层回归，回滚对应 UI 代码即可。
