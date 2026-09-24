## Context

参见 `proposal.md` 与两个 delta spec。桌面端已用 `useStoredSongSort` 按视图 key 持久化全部歌曲等排序；菜单由 `SongSortControl` 共用。歌手/专辑目录通过 Core `CatalogQuery::collections` 读取聚合条目。Core 排序字段和分页 SQL 当前包括歌曲名称、艺人、播放次数与添加时间。导航计数经 `library_counts` IPC 返回。

项目约束：遵守 `openspec/CODE_STANDARDS.md` §3、§4、§6、§8、§9。领域规则放在 Core；UI 不访问 SQLite；IPC DTO 显式转换。此 change 不改 SQLite schema、便携资料格式或同步协议。

## Goals / Non-Goals

**Goals:** 增加歌曲专辑排序和聚合目录排序；分别记住三类排序偏好；让导航计数与后端目录口径一致；保持菜单交互一致。

**Non-Goals:** 按专辑/歌手重定义详情歌曲顺序；排序设置跨设备同步；修改聚合身份、标签缺省值或封面选取规则。

## Decisions

1. **扩展既有歌曲排序类型与查询路径。** 为 Core `SongSortField` 增加 Album；SQLite 排序按生效专辑值排序并提供稳定 tie-break，使分页游标和 UI 顺序一致。UI 标签使用“歌手”，沿用已持久化的 `artist` 字段值，避免丢弃既有偏好。
2. **复用歌曲排序菜单的外观与语义。** 将字段/方向选择抽为可复用的紧凑排序控件；歌曲菜单及目录菜单使用同一组件样式。目录选项由目录类型提供：名称、歌曲数量，加分隔线和升降序。
3. **目录在现有聚合结果上排序。** Core 继续返回当前目录聚合，不把 UI 排序状态带入 Core API；界面用规范化名称/数量及稳定身份作确定性排序。当前返回的是完整目录集合，无分页。
4. **复用 `library_counts` 传输扩展计数。** Core 在已知可用歌曲聚合语义下提供 artist/album 总数，扩展既有 DTO 和 external store；刷新沿用现有资料库失效事件。避免侧边栏自行遍历分页或触发两次目录请求。歌手/专辑数量口径必须复用聚合定义。
5. **本机偏好按 scope 独立持久化。** `all` 歌曲排序继续使用现有 key 及 schema；新增 `artist-directory` 和 `album-directory` 两个不同 key。目录排序默认名称升序。损坏或未知偏好回退默认值。

替代方案：把目录排序放进 Core API 会增加请求/DTO 复杂度，但目录目前由单次完整聚合返回，排序状态只是展示偏好，因此 UI 排序层更贴合边界。直接复用全部歌曲的排序类型会混淆字段集合，因此共享交互组件、保持不同领域排序类型。

## Risks / Trade-offs

- `[Risk]` 生效专辑字段可能来自覆盖元数据，直接排序原始列将与 UI 不一致 → SQLite 查询复用当前有效元数据表达式，并补覆盖层与空值场景验证。
- `[Risk]` 扩展 Rust/IPC 计数结构可能漏更新测试替身或 command DTO → 更新所有构造位置、序列化边界和调用者测试。
- `[Risk]` 排序菜单抽象过度 → 只抽取相同控件结构和样式，输入限于明确字段列表及 sort 值，不新增通用菜单框架。

## Migration Plan

不需要数据迁移。旧的 `artist` 字段值保持原样并改显示名；未知字段（例如此前从未支持的 `album`）安全回退默认。新增目录偏好以独立本机 key 保存。回滚时可移除新 key，旧歌曲排序 key 不变。

## Validation

- `cargo fmt --all --check`
- `cargo test -p echo-core`
- `pnpm --filter @echo/desktop typecheck`
- `pnpm --filter @echo/desktop test`
- `openspec validate add-artist-album-sorting --strict`

以上覆盖格式、Core 分页排序与聚合计数、桌面类型/持久化/交互回归和规格闭环。
