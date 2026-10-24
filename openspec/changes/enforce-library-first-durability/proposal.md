# Proposal

## Why

[Issue #72](https://github.com/zsxink/Echo/issues/72) 表明：SQLite 中可见的歌曲、喜欢和歌单，未必已完整写进音乐库目录；应用重装或数据库损坏后，普通扫描无法找回原 UUID 和用户关系。落实用户的最高优先级：**本地音乐库是唯一持久事实来源；导入、扫描、局域网同步及后续编辑都先可靠保存到音乐库，SQLite 只作可重建投影。**

## What Changes

- 为所有用户资料建立同一个“准备 → 本地提交 → 投影 → 完成”的可恢复写入边界；多文件提交不能用数次 rename 或一个 SQLite 事务冒充原子操作。
- 音乐库持有媒体、稳定歌曲身份与元数据、喜欢的 true/false、歌单定义、成员 UUID 与追加顺序、覆盖层及用户资源、逐设备播放统计、类型化墓碑、必要别名和待确认本地候选；本地权威写入失败不能报告保存成功。
- 导入先校验并安全发布媒体，再持久提交对象；扫描发现、改名重关联和标签更新同样写歌曲对象资料；数据库提交后的 portable 写入副作用退出正式写路径。
- LAN 首次快照、增量、来源设备 ACK、主设备确认都通过 Core 的同一持久化用例；对象接收、数据库投影、媒体可用和整库已同步有独立且可验证的完成边界。
- SQLite 丢失或损坏时，仅依据新目录提交协议已保存的资料重建投影；不回填、救援或修复既有旧音乐库中的历史数据、身份或关系。
- 兼容读取支持的旧格式及历史缺省字段，不因读取而改写旧记录、补造时间或自动修复关系；未知未来格式保持只读。
- 4.2 的投影追赶以正确顺序、幂等性和可恢复性为首要验收；允许采用简单且较慢的实现。多线程、缓存、队列等仅在有证据时评估，大库性能优化和大规模基准可后续独立处理，CI 使用小规模代表性验证。
- **BREAKING（本地格式写入兼容）**：新增目录内提交协议，升级 manifest 格式以阻止旧程序绕过恢复流程写库；支持读取 v1/v2 格式及其既有字段，不要求对历史库执行数据修复或回填；不变更 LAN 冲突胜负规则。

## Capabilities

### New Capabilities

- `library-durability`：资料库权威提交、数据库投影顺序追赶、新协议资料恢复及故障注入契约。

### Modified Capabilities

- `portable-library-layout`：扩展持久恢复布局与对象资料完整性；保留旧格式兼容读取，不要求历史数据修复。
- `local-library`：扫描/监听持久化顺序、只读边界与 SQLite 可丢弃承诺。
- `safe-file-ingestion`：导入媒体、本地对象和数据库的提交顺序及逐文件失败语义。
- `playlist-management`：歌单/成员持久成功的定义与已保存但待投影的反馈。
- `library-experience`：收藏与批量收藏的反馈以音乐库持久结果为准，区分未保存、已保存待更新与已完成。
- `sync-foundation`：目录内持久候选与稳定事件捕获，数据库 outbox 可重建且远端应用不回环。
- `lan-library-sync`：所有入站及主确认先本地持久，游标/ACK/媒体/已同步的发布门槛和重装后的会话恢复边界。

## Impact

- Core `application/import`、`scan`、`continuation`、`root_switch`、收藏/歌单/删除/编辑用例，以及 `domain/library/portable.rs`、filesystem control plane/`portable_migration.rs`、SQLite repository/sync adapters；新增持久化 ports 和目录事务适配器，Application 协调事务，Domain 不依赖文件系统/SQLite。
- 验收登记：新增 capability `library-durability` 需注册 area 前缀并写入 `docs/traceability.md`，全部新增场景需在 `scripts/verify/scenario-commands.mjs` 登记并重新生成 `scripts/verify/manifest.json` 与 `tests/scenarios/*`；三方对账（spec/trace/manifest）、场景命令有效性与重复度棘轮必须随同步一起通过，不能只跑 `openspec validate`。
- `echo-desktop` 的 favorites/playlists/portable materialization、播放统计和编辑服务改为调用 Core 用例；`src-tauri/sync_provisioner` 与 `echo-sync` 只装配/调用 ports，不直接承担权威提交规则。
- SQLite 顺序迁移增加投影检查点/幂等标识；Core API 和类型化 IPC 增加持久失败、已保存待投影及新协议恢复报告；同步网络格式原则上复用现有契约，目录协议版本与网络版本分开。
- 实施时同步 `docs/PRODUCT.md`、`docs/DESIGN.md`、`docs/ROADMAP.md`、恢复文档及界面术语；清理文档中旧“空目录首次加入”口径，以现行 `lan-library-sync` 的本地优先合并为准，不借本次变更倒退或重新定义加入策略。
- 遵守 `openspec/CODE_STANDARDS.md` §2–§6、§8–§9；保留同步默认关闭、单主/epoch、冲突裁决、10 秒撤销、文件无覆盖和跨平台路径约束。已有 UI change `align-immersive-player-and-playlist-directory` 保持独立；本 change 仅提供后续歌单封面字段所需的持久化边界，不包含该 UI change 的字段或界面工作。
- 非目标：修复、回填、救援或身份/关系对账任何现有旧音乐库；从音频推测从未保存的歌单/喜欢；恢复系统钥匙串、配对凭据、主题/EQ、播放会话；添加云盘、流媒体或自动选主；直接备份/传输 SQLite；承诺任意进行中文件拷贝都是一致备份。仅复制 `media/` 能恢复音频，完整资料恢复必须保留整个音乐库的 `media/` 与 `echo/`。

本 change 只规划；详细证据、方案取舍、数据兼容、执行任务与验收矩阵见其余产物。

实现与交付流程：本 change 的全部代码与产物改动在 `codex/issue-72-library-first-durability` 分支上进行，经关联 issue #72 的 PR 合入 `main`；规划阶段不勾选任务、不归档、不声称修复已生效。
