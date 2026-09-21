## Context

See proposal.md for the user-visible problem. 当前播放栏通过 `fireAndForget` 调用临时导入，资料库工作区的 `useSongs` 只在自身收到显式 `reset()` 时刷新；Tauri command 也只执行 Core 导入用例，没有修改 coordinator 的当前 queue entry。导入结果 DTO 已包含 `Imported.songId` 和 `Duplicate.existingSongId`，可直接作为转换身份。

实现必须保持 `echo-core` 不依赖 UI 或播放器。播放队列属于 `echo-desktop`，资料库刷新属于 React 查询边界，Tauri command 只编排两者之间的类型化调用。仓库当前工作树包含另一个未提交的系统回收站变更，本 change 不修改其文件或行为。

## Goals / Non-Goals

**Goals:**

- 让播放栏导入拥有单次、可等待、可取消重复点击的 UI 状态。
- 在成功/重复结果后通过共享前端失效信号刷新已挂载歌曲查询和导航计数。
- 将临时当前项原地转换为库内 `SongId`，保留 queue entry 身份、播放状态和播放位置。
- 用单元测试和 React 测试覆盖导入结果、转换语义、失败保护和刷新广播。

**Non-Goals:**

- 不改变 `import_current_temporary_file` 的命令名称或 ImportResultDto wire shape。
- 不把临时项写入 Core 的会话持久化、同步或播放统计规则之外。
- 不为本 change 引入全局 Tauri library event、数据库迁移或新的第三方依赖。

## Decisions

### 1. 通过共享前端失效 store 广播导入完成

在 library feature 内增加最小的 `libraryInvalidation` 外部 store。`useSongs` 订阅它并触发自己的权威首屏刷新；`useLibraryCountSync` 复用同一失效信号重新拉取计数；播放栏在收到 `imported` 或 `duplicate` 后发布一次失效。选择共享 store 而不是让播放栏持有 `LibraryWorkspace` 回调，是因为两个组件是兄弟节点，且无需把 Tauri 事件扩展成新的跨窗口协议。`useSongs` 保留现有刷新期间数据，避免 reset 造成短暂空列表。

### 2. 后端在同一个 command 中完成导入和队列身份转换

command 先在短锁内读取当前临时项的路径和 entry ID，执行既有导入服务，再在 coordinator 锁内校验该 entry 仍是同一路径的当前临时项。只有 `Imported` 或 `Duplicate` 能得到 `SongId` 时才调用 coordinator 的原地转换方法；如果用户在导入期间已经切歌，旧临时 entry 不被误替换，导入结果仍正常返回。这样不会把绝对路径传入 WebView，也不会让异步导入覆盖新的用户播放意图。

### 3. coordinator 原地替换并恢复 transport

新增一个小的播放层方法：保留当前 `QueueEntryId`，把 `QueueItem::Temporary` 换成 `QueueItem::Library(song_id)`，读取切换前的 player snapshot，并按原状态发送库内加载命令。Playing 使用正常播放加载，Paused/其他非播放状态使用暂停加载；若已有有效位置，则在加载命令之后发送 seek，使复制后的同一音频从原位置继续。不会重建队列、改变模式或重置历史，因此队列中的其他 entry 不受影响。

### 4. 导入按钮状态归 UI 所有

`PlayerBar` 复用沉浸式导入的 async 状态模式，但成功/重复时额外广播资料库失效，失败/跳过时显示短结果提示。成功结果的当前项转换由 command 完成，UI 只等待返回值并负责可见的列表/计数失效；这保持播放状态的权威来源仍是 Rust snapshot。

## Risks / Trade-offs

- [导入期间用户切换了歌曲] → command 只在 entry ID、临时路径和当前项仍匹配时替换；否则不触碰新的播放上下文。
- [加载库内文件短暂产生 Loading snapshot] → 使用与播放状态对应的 load 命令并紧接恢复 seek；这是播放器切换相同音频所需的最小状态窗口，最终仍保持 Playing/Paused。
- [多个资料库视图同时挂载] → 失效广播不携带局部结果，每个查询自行从后端拉取，避免视图过滤和游标状态互相污染。
- [导入结果成功但列表查询失败] → 保留旧列表并沿用现有错误/重试路径；不把一次成功导入伪装成查询成功。

## Migration Plan

无需数据迁移。部署后新 command 行为兼容现有前端返回 DTO；回滚仅需恢复播放栏调用、coordinator 方法和前端失效 store，已有导入记录不受影响。
