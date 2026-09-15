## Purpose

让 Echo 本地数据的**存储形状**从 0.1.0 起就可被二期同步引擎消费，同时严格保持本期"离线、无账号、无网络、无可操作同步入口"的生产边界。本 capability 只建立同步基础数据的 schema、对象 revision 与本地 outbox 预写，不提供远端连接、转换、冲突裁决或 UI 入口。

## ADDED Requirements

### Requirement: 同步基础数据形状（schema 骨架）

系统 MUST 为二期同步引擎预先固定数据形状：全部可同步候选对象（歌曲、歌单、覆盖层、资料库根目录）携带单调递增的整体 `revision`；本地删除保存于 `tombstones`；远程配置与同步游标保存于 `sync_state`；待上传的逻辑变更保存于 `sync_outbox`。上述表与字段 MUST 通过新增的 `0005` 顺序迁移 `0005_sync_foundation.sql` 落地，且不得改动已发布的 `0001` 迁移。

#### Scenario: 数据形状已就绪且离线边界不变
- **WHEN** 迁移 `0005_sync_foundation` 执行后，数据库 schema 包含 `tombstones`、`sync_state`、`sync_outbox` 三张表及全部同步候选对象的 `revision` 字段
- **THEN** 系统保持完全离线运行：不建立任何远端连接、不上传/下载、不引入网络客户端依赖，UI 不显示任何可操作的同步、上传、下载或同步状态入口

#### Scenario: 对象级 revision 单调递增
- **WHEN** 一首歌、一个歌单、根目录或覆盖层发生任何本地逻辑变更（导入、收藏、歌单成员、覆盖层写入、删除）
- **THEN** 该对象的 `revision` 单调递增（初始为 0，每次变更 +1），且本地高并发顺序提交不产生倒退或跳号

### Requirement: 本地变更预写 outbox

系统 MUST 在本期对全部本地逻辑变更（歌曲导入/编辑、收藏切换、歌单增删成员、覆盖层、删除）执行"预写 outbox"：把变更事件作为一条待上传记录写入 `sync_outbox`，使本地数据从首日起即为可同步形状，二期同步引擎无须回填历史。

#### Scenario: 导入与收藏变更是 outbox 条目
- **WHEN** 用户导入歌曲或将歌曲设为收藏，以及歌单增删成员、覆盖层写入或删除
- **THEN** 对应 `sync_outbox` 行被创建，携带对象类型、对象 UUID、递增 revision、操作类型和全量对象载荷；且本条变更同时已提交到对应正表并立即可被本地读取

#### Scenario: 预写不产生可操作同步
- **WHEN** 任何本地变更导致 `sync_outbox` 行增长
- **THEN** 系统依旧不初始化远程连接、不尝试上传、不显示同步进度或错误；outbox 仅是本地持久化的可同步队列，数据可被二期引擎续推

### Requirement: 墓碑

系统 MUST 持久化逻辑删除的墓碑，使删除这一操作本身就是可同步的本地事实（而非仅删除歌曲记录）。墓碑行 MUST 记录删除对象的类型、UUID、revision 与删除时间。

#### Scenario: 删除产生墓碑
- **WHEN** Echo 主动删除歌曲或歌单（非外部文件缺失）
- **THEN** 系统写入一条 `tombstones` 行，记录对象类型、UUID、revision 与删除时间；对象正表行保持或按删除语义收敛，二期可据此进行跨端传播删除

### Requirement: 可同步载荷不含本机路径

系统 MUST 沿袭 `local-library`「绝不把根目录绝对路径写入歌曲关联或用户可同步数据」的约束：当同步基础数据（`sync_outbox` 全量载荷、`tombstones`、对象 revision）成为可同步数据的载体时，其载荷只包含对象 UUID、相对路径与对象字段，绝不包含本机绝对路径、数据库文件路径、凭据或网络端点。

#### Scenario: 本机绝对路径不进入可同步载荷
- **WHEN** 任何同步基础数据（`sync_outbox` 载荷、`tombstones`、对象 revision）被序列化或导出
- **THEN** 载荷中不出现根目录绝对路径、数据库文件路径、账号/凭据或网络端点，只包含逻辑 ID、相对路径与对象字段