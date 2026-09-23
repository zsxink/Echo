## Why

歌手与专辑目录已有聚合内容，但缺少数量提示与用户可控排序；曲库增大后难以快速定位条目。全部歌曲排序也缺少按专辑排序，且界面术语应统一使用“歌手”。

## What Changes

- 在侧边栏显示歌手数与专辑数，计数与对应活动资料库聚合目录条目数一致。
- 歌手、专辑目录分别提供名称/歌曲数和升序/降序，并持久化各自的排序选择。
- 目录排序菜单沿用全部歌曲排序菜单的视觉和交互样式。
- 全部歌曲排序菜单将“艺人”改为“歌手”，增加“专辑”排序；既有歌曲排序偏好继续跨重启保存。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `artist-album-browsing`: 聚合目录支持独立排序偏好，并要求侧边栏计数与目录条目一致。
- `library-experience`: 全部歌曲支持按专辑排序、显示“歌手”术语，排序偏好跨重启持久化，并扩展导航计数范围。

## Impact

- 影响 Rust Core 聚合/歌曲排序模型、资料库计数 IPC DTO 与桌面端 command 转换。
- 影响 React 排序菜单、歌手/专辑目录、导航计数和偏好存储。
- 不增加依赖，不改变 SQLite 持久化或媒体资料格式；遵循 `docs/PRODUCT.md`、`docs/DESIGN.md`、`docs/ROADMAP.md`、`docs/interface-terminology.md` 与 `openspec/CODE_STANDARDS.md`。
