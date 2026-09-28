# Design

## Context

动机见 [proposal.md](proposal.md) 的 Why；本节只记录做决定所必需的现状与约束。

### 现状

`apps/desktop/src/app/overlays.ts` 维护单一浮层栈：模块加载时在 `window` 上装一个 Escape 处理器（`overlays.ts:95`）和一个 capture 阶段的 `pointerdown` 处理器（`overlays.ts:114-126`）。后者取当前最高优先级层，判定「内部区域」的方式只有一条：

```ts
if (target && registration.containerRef.current?.contains(target)) return;
```

`Registration` 只持有 `containerRef`，`useOverlay` 的 `OverlayOptions` 也只接受 `containerRef`（`overlays.ts:128-138`）。`dismissOnInteractOutside` 默认为 `tier === OverlayTier.Menu`（`overlays.ts:150`）。

缺陷由此产生：某浮层能否被自己的触发控件再次点击关闭，取决于触发控件是否恰好落在 `containerRef` 指向的 DOM 元素内。仓库里三种 `Menu` 层浮层的实测结果：

| 浮层 | 触发控件 | 在 `containerRef` 内？ | 能否二次点击关闭 |
|---|---|---|---|
| `SortMenu` | `.sort-wrap` 内的按钮 | 是（`SortMenu.tsx:67` 同一 `div`） | 是 |
| `QueuePanel` | `<footer class="playerbar">` 内的按钮 | 否 | **否** |
| `SongMenu` | 表格行内 `.song-more` 按钮 | 否 | **否** |
| `BatchSongMenu` | 表格行内 `.song-more` 按钮 | 否 | **否** |

`ImmersivePlayer` 不受影响，因为它注册为 `OverlayTier.Immersive`，`dismissOnInteractOutside` 默认为 `false`，`pointerdown` 直接跳过。

`SongMenu` 还额外挂了一个自己的 `document` 级 `pointerdown` 监听（`SongMenu.tsx:111-120`），判定逻辑与浮层栈的监听重复，同样只认 `menuRef`。

### 触发控件的接线深度

- 播放队列：`App.tsx:347`（`<QueuePanel />`）与 `App.tsx:349`（`<PlayerBar />`）是平级兄弟，触发按钮在 `PlayerBar` 内部。需要 `App` 持有一个 ref 并同时传给两者。
- 歌曲操作菜单：触发控件在 `SongRow` 内的 `.song-more` 按钮（`SongRow.tsx:274-282`）。`SongRow` ← `SongList` ← `LibraryWorkspace` / `PlaylistsView` / `CollectionDirectory` 三条入口各持有一份菜单状态，因此 ref 要穿三层 props。

### 关键约束：不能靠重新挂载 DOM 解决

`.queue-popover` 是 `position: fixed; right: var(--space-6); bottom: 88px`（`styles/player.css:64`），即相对视口定位，与 DOM 位置无关。但把面板移入 `<footer class="playerbar">` 会改变它的包含块：

- `player.css:144`：`body.player-mode-open .playerbar` 用了 `backdrop-filter: blur(18px) saturate(135%)`；
- `styles/responsive.css:14`：窄屏侧边栏展开时 `.playerbar` 用了 `transform: translateX(45vw)`。

`backdrop-filter` 与 `transform` 都会让元素成为 `position: fixed` 后代的包含块，面板会从视口定位变为相对播放栏定位，弹出位置错乱。这排除了 issue 提出的「套一层共同容器」方案（`SortMenu` 能用是因为它自己的面板也用 `usePlacement` 走绝对定位，不受影响）。

`SongMenu` / `BatchSongMenu` 的触发控件在虚拟滚动表格的行内，同样没有共同容器可套。

## Goals / Non-Goals

**Goals**

- 让「再次点击触发控件」对所有 `Menu` 层浮层成为可靠行为，且不依赖触发控件与浮层的 DOM 关系。
- 判定规则收敛到浮层栈一处，不在 `SongMenu` 保留第二份重复逻辑。
- 不改变浮层栈的任何既有对外契约：Escape 优先级、焦点进入/恢复、Tab 陷阱、点击外部关闭。

**Non-Goals**

- 不改 `.queue-popover` 的定位方式或任何 CSS。
- 不改右键菜单的打开方式、菜单项集合、批量操作语义与队列投影规则。
- 不为 `useOverlay` 引入新的层级或注册通道。

## Decisions

### D1：用可选 `triggerRef` 显式声明触发控件，而不是推断

`OverlayOptions` 增加一个可选 `triggerRef?: RefObject<HTMLElement | null>`，`Registration` 同步持有它，判定改为两者取并：

```ts
const inside =
  (target && registration.containerRef.current?.contains(target)) ||
  (target && registration.triggerRef?.current?.contains(target));
if (target && inside) return;
```

**为什么不是「自动捕获上一次 pointerdown 目标」**：capture 监听本来就能读到 `event.target`，把它记成浮层的触发控件可以做到零穿参、自动覆盖所有浮层。但 `useOverlay` 注册发生在浮层**打开**时，而注册那一刻最近一次 pointerdown 往往不是本次的触发控件（键盘打开、程序化打开、或两次不同浮层先后打开时都会读到过期目标）。那是隐式状态，出错时难以定位；显式 ref 把意图写在调用处。

**为什么不是「套共同容器」**：见 Context 关键约束。且它对虚拟滚动的表格行根本不成立。

**为什么不用 callback 而不是 ref 对象**：调用方需要 ref 挂到真实 DOM 节点上才能做 `contains`，callback ref 无法回答「这个节点包含目标吗」。调用方本来就在为 focus trap 之类的用途持 ref，沿用同一种形态即可。

**为什么是可选**：已有调用点（含 `SortMenu` 与 12 个对话框/选择器）不传时行为完全不变，`contains` 判定仍是唯一的内部区域规则。

### D2：触发控件计入内部区域只作用于 `pointerdown`，不碰其它路径

Escape 处理器（`overlays.ts:95`）与 `useFocusTrap` 都不读 `triggerRef`，保持原样。

**取舍**：这带来一个可接受的不一致——Escape 关闭浮层时，`useOverlay` 的清理函数（`overlays.ts:179-187`）会把焦点还给「打开瞬间的 `document.activeElement`」。经触发控件打开时那正是触发控件，焦点自然回到它；经右键打开时是页面上的其它元素，焦点回到那里。这是既有行为，本次不改。spec 中「焦点回到该按钮」的场景限定在播放队列这条由按钮打开的路径上。

### D3：`SongMenu` 的重复 `pointerdown` 监听改为复用同一判定

`SongMenu.tsx:111-120` 自己的监听和浮层栈的监听做同一件事，判定条件将改为调用从 `overlays.ts` 导出的同一个内部判定函数，并把 `triggerRef` 纳入其中。监听本身保留：它带 `dismissable` 开关（菜单内部弹出确认框/详情时暂停），这是浮层栈不知道的信息。

`BatchSongMenu` 没有这层重复监听，不需要改动判定，只需传入 `triggerRef`。

### D4：右键/键盘打开的菜单，触发控件取行元素而非按钮

`SongMenu` 可由三条路径打开：`.song-more` 按钮点击（`SongRow.tsx:274-282`）、行右键（`SongRow.tsx:166-170`）、键盘菜单键（`SongRow.tsx:152-160`）。后两条没有按钮参与，但行元素本身有 `data-song-id` 且可作为 `contains` 的接收端。

因此 `SongRow` 把**行元素**（`event.currentTarget`，已存在于各 handler 中）作为 `triggerRef` 节点回传，而不是 `.song-more` 按钮。

**取舍**：行包含 `.song-more`、收藏、加入队列等按钮，于是「菜单打开时点击该行内的其它行内按钮」会被判定为内部交互，不触发菜单关闭。这与这些按钮本来的行为不冲突——它们各自有 handler，且 `fromRowControl`（`SongRow.tsx:91-93`）已让行自身的点击处理器在行内控件上让位。真正的代价是：菜单打开时点击同一行的收藏心形，不再顺带关掉菜单。相较于「再次点击触发控件完全无效」，这是更小的取舍；spec 中「仍可点击真正外部区域关闭」的场景正是守住其余区域的行为。

**同时不改变**：再次右键**其它**行仍走 `onContextMenu` 打开新菜单（既有行为），Escape 与点击面板外部的路径不变。

### D5：ref 穿三层 props，不引入 Context

`SongRow` ← `SongList` ← 三个视图组件。备选是新建一个 `SongMenuTriggerContext`，让 `SongRow` 就地把 ref 写进视图持有的可变对象。

**为什么穿 props**：`SongList` 已在用 props 传 `onOpenMenu` 等同类回调，新增一个 `triggerRef` 与既有风格一致；`SongList` 与 `SongRow` 都有公开测试，新增 prop 只需给默认值。Context 会在虚拟滚动的每一行都订阅一次，收益不抵成本。

**为什么值得穿三层**：只修播放队列的话，`SongMenu` / `BatchSongMenu` 会继续以完全相同的方式失效——同一个抽象缺陷留下两个已知的、带同样症状的残留点。

### D6：可访问名称随状态切换

`PlayerBar.tsx:327` 的 `aria-label="显示播放队列"` 改为随 `ui.queueOpen` 在「显示播放队列」/「隐藏播放队列」间切换，遵循 `docs/interface-terminology.md` 中「播放队列面板」的既有称谓，不引入新术语。`aria-expanded` 已有，保持不变。

## 跨模块影响

```
App.tsx ──queueTriggerRef──> PlayerBar (挂在 queue-trigger 按钮)
       └──────────────────> QueuePanel (useOverlay triggerRef)
                              ↑ overlays.ts: Registration.triggerRef

LibraryWorkspace / PlaylistsView / CollectionDirectory
  ──menuTriggerRef──> SongList ──> SongRow (挂在 <tr>)
  ──batchTriggerRef──> SongList ──> SongRow
SongMenu / BatchSongMenu (useOverlay triggerRef + 共用判定)
```

分层：全部落在 React 展示层（`CODE_STANDARDS` §3.1 的 Presentation / Platform），不触碰 Core、`echo-desktop`、SQLite 或 IPC，故不触发 §9.1 的兼容与迁移条款。`useOverlay` 的签名是前端内部接口，新增的是可选字段，无破坏性。

## Risks / Trade-offs

- **[`SongRow` 内部点击不再顺带关闭菜单（D4）]** → 已在 spec 的场景中显式化；点击菜单外部的行、行间空白或其它控件仍照常关闭，用户关闭菜单的路径不受影响。
- **[`SongMenu` 合并两处判定后行为漂移]** → 合并后 `SongMenu` 的关闭时机由「面板内/触发控件内不关」与浮层栈保持一致；其 `dismissable` 开关在删除确认框与详情浮层打开时仍暂停关闭，两层语义不冲突。回归用例覆盖该场景。
- **[`triggerRef` 未挂载或已卸载时判定退化]** → `contains` 对 `null` 返回 `false`，退化为「面板外即关闭」，即修复前的行为，安全方向正确。
- **[虚拟滚动下行节点被回收]** → `SongRow` 是虚拟化的，行会因滚动离开 DOM 而卸载，`triggerRef.current` 随之变为 `null`，判定退回原行为；菜单本身不因此失去关闭能力。

## Migration Plan

无。纯前端交互修复，无数据格式、IPC、数据库或配置变更。回滚即还原本次改动。

## Open Questions

无。均已在 D1–D6 内定案。
