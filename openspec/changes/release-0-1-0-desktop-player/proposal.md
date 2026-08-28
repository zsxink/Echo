## Why

Echo 目前只有已批准的产品/架构文档和可交互桌面原型，还没有可运行的应用实现。0.1.0 需要一次性建立桌面本地音乐体验的完整地基与日常闭环，使 macOS、Windows、Linux 用户无需账号或网络即可安全管理、检索和播放自己的音乐资料库。

## What Changes

- 建立 Rust workspace、Tauri 2 桌面壳和 React + TypeScript + Vite UI，形成 `echo-core`、`echo-desktop`、`apps/desktop` 的分层工程。
- 建立 SQLite 迁移、Repository、FTS5 索引和稳定 UUID 数据模型，保存资料库、歌曲、收藏、播放统计、歌单、覆盖层预留和可恢复操作日志。
- 支持选择一个本地资料库根目录、全量/增量扫描、文件监听、手动重扫、元数据/封面/歌词解析、BLAKE3 去重与移动重关联，以及资料库不可用状态。
- 支持安全导入：复制外部音频和同名 `.lrc`，按默认目录规则组织，绝不覆盖已有文件，并通过暂存、校验、原子移动和操作日志实现崩溃恢复。
- 支持系统文件关联；资料库内文件按已有 UUID 播放，资料库外文件作为不持久化、不计播放统计的临时播放项。
- 提供全部歌曲、最近添加、喜欢的音乐和歌单视图，以及搜索、四种排序、歌曲详情、打开所在目录、收藏和可恢复删除。
- 提供歌单创建、查看、重命名、删除和歌曲成员管理；歌单内歌曲按追加顺序，不实现手动排序。
- 通过 mpv/libmpv 提供桌面播放、队列、进度、音量、播放模式、媒体键/快捷键、播放统计和本机播放会话恢复。
- 实现常驻播放栏、沉浸式黑胶播放器、同步歌词、歌词专注阅读、无歌词状态、三套主题、窄屏适配、键盘可达性和本机偏好。
- 实现 macOS 菜单栏状态项、Windows/Linux 系统托盘和“关闭主窗口时退出或后台运行”的平台偏好。
- 明确 0.1.0 不提供账号、网络依赖、S3/WebDAV 同步、可操作同步入口、全选批量操作、歌单手动排序、歌曲元数据写回/移动、自定义导入模板、目录树视图或移动端客户端。
- 原型中与路线图冲突的模拟同步、全选批量和歌单排序交互不进入 0.1.0；`docs/PRODUCT.md`、`docs/ROADMAP.md` 与本变更规格优先于原型演示脚本。

## Capabilities

### New Capabilities

- `desktop-app-shell`: 跨平台桌面工程、应用生命周期、托盘/菜单栏、窗口行为、本机偏好、主题和响应式可访问壳层。
- `local-library`: 资料库根目录、SQLite 模型、扫描/监听、元数据提取、稳定身份、搜索索引、不可用与删除恢复行为。
- `safe-file-ingestion`: 外部文件安全导入、去重/重名、同名歌词侧车、操作日志恢复和系统文件关联临时播放。
- `library-experience`: 曲库视图、搜索、排序、收藏、歌曲详情、空态/错误态和高效大曲库浏览。
- `playlist-management`: 歌单 CRUD、成员增删、追加顺序、重复成员规则和歌单失效歌曲处理。
- `desktop-playback`: mpv 播放适配、播放队列、传输控制、进度/音量/播放模式、快捷键、播放统计和会话恢复。
- `immersive-lyrics`: 常驻播放栏、封面、沉浸式黑胶播放器、时间同步歌词、专注阅读、无歌词状态和浮层交互。

### Modified Capabilities

无。项目当前没有已发布的 OpenSpec 主规格。

## Impact

- **代码与模块**：新增 `Cargo.toml` workspace、`crates/echo-core`、`crates/echo-desktop`、`apps/desktop`、跨平台打包配置、迁移和测试基建。Core 不依赖 Tauri、mpv、React 或平台 UI。
- **边界接口**：新增类型化 Tauri commands/events、播放器 Adapter、资料库/文件/标签/操作日志 Ports 和 UI DTO；歌曲和歌单跨层引用统一使用 UUID。
- **本地数据**：新增 `echo.db` 及顺序迁移；用户音频和 `.lrc` 仍保留在所选资料库，绝不上传 SQLite。0.1.0 尚无已发布数据，因此无向后兼容迁移，但迁移框架必须从首版建立。
- **依赖**：Rust 侧使用 Tauri 2、rusqlite/SQLite FTS5、lofty、blake3、uuid、notify、tokio、serde、thiserror 等；桌面播放使用 mpv/libmpv；前端使用 React、TypeScript、Vite 和相应测试工具。新增依赖须锁定并通过许可证/三平台构建检查。
- **系统集成**：需要目录/文件选择、文件监听、系统回收站、打开所在目录、文件关联、媒体键、托盘/菜单栏和单实例唤醒权限；各平台使用 Adapter 隔离。
- **事实来源**：产品范围遵循 `docs/PRODUCT.md` 与 `docs/ROADMAP.md`，架构遵循 `docs/DESIGN.md`，界面遵循 `docs/interface-terminology.md` 与 `docs/prototype/`，实现门槛遵循 `openspec/CODE_STANDARDS.md`。
