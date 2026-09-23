## 1. Core 歌曲排序和目录计数

- [x] 1.1 扩展歌曲排序领域字段与 SQLite 查询，使歌曲可按生效专辑稳定排序；保留现有 `artist` 编码兼容，并验证分页次序与 tie-break。
- [x] 1.2 扩展目录导航计数，确保歌手、专辑计数采用与 `collections` 一致的活动根、可用歌曲与缺省元数据聚合口径；覆盖新增/删除/扫描后读取。
- [x] 1.3 更新 Tauri command/IPC DTO、React bridge 类型及所有测试替身。

## 2. 桌面排序控件与偏好

- [x] 2.1 全部歌曲排序菜单显示“歌手”并增加“专辑”；确认原 `artist` 本机偏好仍有效，新专辑偏好能跨重启保存。
- [x] 2.2 提取可配置字段的共用排序菜单外观，在歌手、专辑目录提供名称/歌曲数量与升降序选项。
- [x] 2.3 分别持久化歌手和专辑目录排序字段与方向；按字段稳定排序当前聚合条目，默认名称升序。
- [x] 2.4 在侧边栏显示歌手与专辑总数，并沿用资料库计数刷新/保留已知值行为。

## 3. 验证与收尾

- [x] 3.1 补齐排序方向、稳定 tie-break、独立偏好、重启恢复、数量刷新和旧偏好兼容的桌面/Core 场景验证。
- [x] 3.2 运行 `cargo fmt --all --check`、`cargo test -p echo-core`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop test` 与 `openspec validate add-artist-album-sorting --strict`。
- [x] 3.3 复核变更与工作区既有未提交样式改动的边界，提交本次功能并 push 当前分支。
