# 同步空闲 CPU 修复验证（2026-10-17）

关联 Issue #52；仅覆盖 task 6.6 的自唤醒缺陷，不代表整个 LAN change 已完成验收。

## 现场根因与修复

原 Host 周期无条件调用 `advance_rejected_local_mutations`。空队列也提交事务并返回 `Ok(0)`；SQLite writer 对所有成功请求发送同步通知，下一轮立即消费自己的通知，绕过 60 秒轮询间隔。现场 PID 51557 读到 148.5% CPU，停止同步后降至 2.9%～4.4%；两份开启时的采样反复出现 `confirm_local`、`advance_rejected_local_mutations`、数据库查询和序列化。

修复将普通写订阅与同步工作通知分开：有同步订阅时，在写请求前后比较主键索引端点 `MAX(outbox_id)`，仅新增本地 outbox 自动唤醒；主机首次发布远端 commit 显式唤醒。空事务、内部游标维护、快照创建、本机确认、重复发布及回滚不再触发下一轮。查询计划验证使用 `Last` 而非从头 `Rewind`，不全表扫描。未修改 schema、协议、确认顺序或失败退避；并发本地编辑通知仍保留。

独立 reviewer 检查通知时序、事务、发布幂等与并发后未发现阻塞问题。一个非阻塞边界：关闭同步后的断开 sender 要在下一次同步工作通知时才清除，因此此后普通写入可能仍执行两次索引端点查询；不会产生后台循环。

## macOS 原生进程对照

环境：macOS arm64，同一 debug 构建、同一进程 PID 64757，用户保持正常歌曲播放。以 `ps -p <pid> -o time=` 的累计 CPU 时间差除以单调时钟测量时长；单核满载为 100%，不使用活动监视器的瞬时读数作为均值。开启样本由后台 50826 监听与 `echo-lan-sync-loop` 线程共同确认，包含开发重载恢复保存主机状态；用户随后停止主机服务，确认监听消失，继续播放测量关闭基线。

| 后台状态 | 墙钟时长 | 进程 CPU 时间增量 | 平均 CPU |
| --- | ---: | ---: | ---: |
| 同步开启、无待处理传输 | 60.034 秒 | 5.03 秒 | 8.379% |
| 同步关闭、继续播放 | 60.037 秒 | 5.02 秒 | 8.361% |

额外平均占用差值为 **0.017 个百分点，约 0.02 个百分点**，低于本轮空闲同步额外 1% 预算。进程整体约 8.4% 包含播放器与 UI；本次对照不把它全部归因于同步。

修复后开启状态另取 3 秒 `sample`：同步线程的 1442 个采样均落在阻塞等待链，末端为 `semaphore_timedwait_trap`，没有原来的连续同步周期。原始现场采样与度量 JSON 保存在本机 `/tmp/echo-cpu-51557*.sample.txt`、`/tmp/echo-cpu-fixed-on.sample.txt`、`/tmp/echo-cpu-fixed-{on,off}.json`，不作为持久发布资产。

该结果是本机一次对照，CPU 时间读数精度与 UI/系统调度会带来波动，不表示所有平台、曲库规模和传输状态都具有固定占用。大文件哈希、首次同步和实际传输的 CPU 峰值需分别评估。

## 自动验证

- 新增 6 个回归全部通过：无同步订阅不读 outbox；空事务/无关写/回滚不唤醒且后续本地工作正常；索引端点查询；仅首次远端发布唤醒（含本机发布、失败、重复和快照负例）；Peer 空页游标维护不唤醒；真实调度器在维护后阻塞并保留执行期间并发编辑。
- `cargo test -p echo-app sync_runtime`：5 passed。扩大到 echo-app 全目标：41 单元测试、2 架构测试通过，1 个已有真实 LAN 压测忽略。
- `cargo test -p echo-core --lib infrastructure::sqlite:: -- --skip different_hash_same_path_gets_a_stable_safe_destination`：170 passed、1 个已有性能测试忽略。扩大到 Core 全部单元测试：597 passed、1 failed、1 ignored。
- 失败的 `different_hash_same_path_gets_a_stable_safe_destination` 在 `sync_payloads/song_aliases.rs:264`；使用 `git archive HEAD` 解出未修改源码并独立编译运行相同测试，仍在相同断言失败，确认不是本修复引入。
- `cargo test -p echo-sync -p echo-desktop --all-features`：全部实际执行的单元、集成、协议、架构和文档测试通过（echo-desktop 单元 413、echo-sync 单元 36）；3 个已有 mDNS 环境测试忽略。
- 本轮所有修改的 Rust 文件通过定向 `rustfmt --check`，`git diff --check` 与 `openspec validate introduce-lan-library-sync --strict` 通过。
- `cargo build --workspace` 通过，修复版已在本机原生开发进程运行并完成上述 CPU 对照。
- 全仓 `cargo fmt --all --check` 仍因未修改文件原有格式问题失败，包括 main.rs、sync_commands.rs、sync_provisioner、sync_runtime.rs、sync_song_aliases.rs 和 echo-sync/client.rs。
- 全仓严格 Clippy 仍有 7 处未修改代码的现有错误：scan.rs:154 锁释放；sync_ledger.rs:429 函数长度、:815 const；sync_projection.rs:326 和 sync_queue.rs:82 函数长度；sqlite/mod.rs:529 `use_self`；sync_snapshot_apply.rs:502 redundant clone。本轮新增 actor 警告已修复，复跑中不再出现。

整个同步 change 仍有其他未完成任务，未创建 PR、未归档。
