# LAN library sync recovery

This guide covers LAN host recovery. For user-data recovery after reinstall, preserve the **complete library root**, including both `media/` and `echo/`; SQLite is intended to be a rebuildable projection under `enforce-library-first-durability`, but that change is not yet implemented and verified for every entrypoint. Keeping only `media/` preserves audio bytes, not song UUIDs, favorites, playlists, or membership. LAN trust and host authority are separate from those user materials: directory data alone must not restore credentials, authorize a host, or resume sync.

## Before restoring

1. Stop sync on every device and quit Echo on the host.
2. Preserve a copy of the complete library root, including `echo/` and `media/`. Keep any separate Echo application-data backup too; it may contain host ledger and device credentials. The library copy is user-data evidence; app data is needed only for session/trust recovery.
3. Restore the library and application data from the same backup point whenever possible. Do not delete `echo/tmp/`, `echo/records/`, `echo/manifest.json`, transactions, migration evidence, or the sync ledger by hand. A supported format upgrade must preserve original records and stop safely on unknown formats or read-only control planes; until that protocol ships, do not use manual edits as a repair method.
4. Open the restored library on the host and inspect its integrity status. A checkpoint ahead of the local ledger, an unreadable checkpoint, or a same-sequence digest mismatch fences the host. Do not reconnect peers until the recovery action completes.

## Start a new host epoch

In **Settings → 局域网同步 → 从备份恢复主机**, select the intended restored library and confirm the action. It stops an active host session, archives pending and conflicting local mutations, and creates a new authority epoch with an independent genesis checkpoint. The archive receipt contains counts and identifiers, not credentials or filesystem paths. Keep the receipt with the backup record.

Then start hosting again and pair each peer with a fresh six-digit code. Peers must accept the new authority identity; they must not reuse credentials from the old epoch. Review preserved conflicts and pending edits as ordinary new changes. Tombstones, inbox receipts, and conflict evidence are retained; recovery does not garbage-collect them.

If Echo cannot read the checkpoint or portable authority evidence, leave sync disabled and preserve the full backup for diagnosis. Do not treat `echo/` alone as a complete media backup, or SQLite alone as a consistent host/user-data restore. If records are missing, damaged, unknown-version, or have unresolved references, preserve them and report incomplete recovery; never invent historical favorites or playlist relations from audio tags.

## Interrupted media publication

Remote media is staged and verified before it is atomically published. If the destination file is locked (for example, by a player on Windows), metadata may be present while that media remains unavailable; the status must not claim that all media is ready. Close the player or release the file handle, then resume sync. A restart resumes from the durable publication journal. Cleanup only removes redundant staging data whose journal and content hash/length prove that it belongs to a completed publication; staging with unknown provenance is left untouched for diagnosis.

## 附属机写回内嵌信息后同步失败

修改歌曲文件中的封面、歌词或专辑会改变全文件哈希。旧实现可能在上传前向主机下载这个仅存在于附属机的新版本，日志返回 `resource_not_found`，界面却显示检查主机状态和配对关系。修复后的版本会先完整校验并复用本机文件，再上传并等待主机确认；资源缺失单独显示媒体资源不可用。

在两端更新 Echo 后保留原资料库与配对，重试同步。不要清空待同步队列、删除歌曲或重新导入来绕过这个错误。如果仍提示资源不可用，确认两端媒体目录可访问，并保留两端同步日志用于定位缺失资源。

## 主机新导入的歌曲未同步到附属机

现象：主机导入新歌后本机可正常播放，附属机始终收不到；此前已同步的歌曲不受影响。2026-10-19 复现记录：媒体文件完好（BLAKE3 与导入记录一致），曲目记录和索引均已写入，但同步资源行停留在 `local_state='missing'`，且没有生成对端发布记录，主机因此不会把内容通告给附属机；健康歌曲该值均为 `available`。

相关联的两个已知问题：一是删除歌曲后媒体路径可能落进保留目录 `echo/media/song/`，发布索引每隔约 10 秒重试并报 `media path must not use the reserved 'echo' directory`，发布管线会持续被该条目阻塞；二是重新绑定同步（开关局域网同步、从备份恢复主机等）会重建 library/authority 并清空 `sync_peers`，附属机随即失去配对。

临时处置：重新绑定后先在附属机重新配对，再删除并重新导入卡住的歌曲（或触发一次媒体库扫描），让资源重新登记为可用。仅重启不会修复该状态。修复方向：导入完成后按库内路径复核资源并落发布记录；删除流程不得把媒体路径留在 `echo/` 保留目录内。

## What cannot be detected

If the entire machine state, library root, and independently stored checkpoint are all rolled back together to the same older point, local evidence cannot prove that newer commits once existed. Use a backup inventory or another peer's observed checkpoint to identify that case. When in doubt, recover into a new host epoch and re-pair; never resume the old epoch from a potentially stale backup.

## Network checks

Sync is opt-in and stays disabled until a user starts hosting or pairs a peer. It uses HTTPS on the local network, port `50826`, with device identity pinned after pairing. There is no account sign-in or public relay. If discovery fails, verify both devices are on a reachable LAN, allow Echo through the operating-system local-network/firewall prompt, and enter the host's current LAN IP address with the pairing code. An IP change requires reconnecting to the pinned host identity; it does not authorize a different host. Revoke a lost device from the host before pairing a replacement.

## 本地目录提交的恢复边界

开发与诊断所需的版本、字段、typed 墓碑/别名和历史缺省规则见
[本地资料库提交协议](library-durability-protocol.md)。manifest、envelope、目录标记
及 LAN wire 分别版本化；当前目录提交实现不等于完整 v3 升级已经完成。

本地目录协议先刷盘 prepared 正文，再发布带序列/前驱摘要的 committed 标记，最后材料化对象与本机 state。只有 prepared 的操作仍是未保存；标记已经发布后的故障必须保留原 operation ID，并以“已保存、待恢复”继续同一操作。读取不创建或获取 `write.lock`，也不在只读浏览时执行格式迁移；已提交但尚未材料化的对象通过经过校验的提交载荷读取。

当前实现每 32 个活跃提交发布一次完整覆盖快照及真实链头，再回收已覆盖事务文件。原操作正文保留在按 ID 分片的 `echo/state/operations/`，用于同 ID 幂等重试；私有历史 state 和资源暂时保守保留，完整历史归档及资源回收尚未完成。备份必须包含整个 `echo/`，不能只复制 records 或媒体。协议字段、失败测试、平台限制和未完成部分见 [目录协议实施证据](../openspec/changes/enforce-library-first-durability/evidence/directory-protocol.md)。当前原生验证限于 macOS，不能据此承诺 Windows/Linux 或硬件断电下的相同行为。

### 文件系统适配与失败语义

目录提交适配器要求同一文件系统内的临时文件发布：先以 exclusive-create 创建临时文件，写全并 `sync_all`，prepared/marker 通过 hard link 独占发布，材料化记录和检查点通过 rename 替换。随后逐级同步父目录直到注册资料库根；任何必需同步失败均返回错误。此处只描述 Core 的目录提交适配器，其他历史媒体暂存或应用配置 helper 的能力不能替代它。

| 平台 | 目录同步与替换实现 | 已有证据与限制 |
| --- | --- | --- |
| macOS / Linux（POSIX） | 打开目录句柄并要求 `sync_all` 成功；同文件系统 hard link / rename；不先删除已提交目标 | macOS 临时目录和实际文件边界故障注入已有测试；Linux 尚未原生执行。挂载方式、文件系统或介质不支持时返回 IO 错误，不降级为普通保存成功 |
| Windows | 以 `FILE_FLAG_BACKUP_SEMANTICS` 打开目录并要求 `sync_all` 成功；使用相同 hard link / rename 发布规则 | 尚待原生验证。目录句柄权限、文件系统 hard link 支持、目标文件占用或替换限制均可能导致明确失败；不能仅凭文件已可见就跳过父目录同步 |

提交标记之前失败不会授权 SQLite 保存新的用户事实。提交标记已发布后，若目录同步或材料化失败，操作可能已经保存，必须保留原身份并查询/恢复，返回“已保存，待恢复”；若目录恢复完成而 SQLite 失败，返回“已保存，等待更新”。不要再次创建相同歌曲/歌单，也不要删除事务以消除提示。无法读取或证明原提交时保留证据并停止相关写入。

恢复顺序是验证当前 manifest 和库身份、覆盖快照及提交链，再从持久材料化水位按序补齐对象和私有 state；SQLite 的投影水位不能授权回收目录事务。坏摘要、断链、跨库资料或已提交文件缺失会停止恢复。受约束读路径检查相对路径、符号链接与实际内容；只读浏览不会创建写锁或执行迁移。未来格式拒绝写入，旧格式升级仍受专门迁移任务约束。

开发者可用 `cargo test -p echo-core library_durability --lib` 重复自动化验证，测试覆盖文件同步、发布、各级目录同步、材料化、覆盖快照和 GC 的强制失败。该测试不能代替磁盘满、真实拔盘、硬件断电和三平台原生验收。完整 v3 升级、所有入口的统一顺序投影与历史资源回收仍在开发；本节不表示整个 library-first change 已交付。
