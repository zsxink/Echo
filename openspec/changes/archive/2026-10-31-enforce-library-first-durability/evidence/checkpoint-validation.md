# 2026-10-24 检查点失败修复与 Core 复验

本轮只处理交接要求的 write permit 生命周期复查、`sync_cursor` 装配失败和对应
验证，不将这些子步视为任务 3.1 或整个 change 完成；未修改 `tasks.md`。

## 根因与实现

- `AccessGuard::drop` 已有显式 `fs2::FileExt::unlock`，能避免重复句柄或进程生成时
  暂存的继承句柄延长 OS 锁生命周期。但是 `prepare_write` 先获取 OS 锁，之后
  才在 migration recovery 和 manifest 校验成功时创建 guard；这些中间步骤失败
  仍只靠文件句柄析构。现在取得锁后立即创建 guard，再执行所有可失败准备步骤。
- guard 的字段顺序改为 file 在 mutex guard 前；显式 unlock 在线程 mutex 仍持有
  时执行，即使 unlock 失败，兜底文件 close 也先于线程 mutex 释放。Drop 仍不能
  回传 cleanup 错误，未改变格式、权限或生产写入准入规则。
- 新回归分别注入坏 manifest 与坏 v1→v2 migration journal，保留重复 file handle，
  确认持有 permit 时另一 adapter 被拒绝；准备返回错误后，在重复 handle 仍存活时
  两个 adapter 均可依次重新取得 permit。原成功路径重复句柄回归保留。
- `sync_cursor` 原 fixture 的 host/replica 只有裸 SQLite，集成编译没有内部
  `cfg(test)` 的孤立 repository fallback，因而缺目录提交能力。fixture 现在
  分别创建实际目录、注册 `RootRegistry`、写入合法 manifest 并注入真实
  `RootControlPlane`。本地目录 library ID 与 LAN binding ID 仍分别建模，未降低
  生产 directory-first 要求；原叉链、空响应、回滚游标和精确重试断言保持。

## 实际验证（macOS）

| 命令 | 结果 |
| --- | --- |
| `cargo test -p echo-core writer_permit --lib` | 1 passed，0 failed |
| `cargo test -p echo-core failed_write_preparation --lib` | 1 passed，0 failed；包含两个错误阶段 |
| `cargo test -p echo-core typed_tombstones_with_the_same_uuid_coexist_and_delete_independently --lib` | 1 passed，0 failed，0.08 秒 |
| `cargo test -p echo-core --test sync_cursor` | 1 passed，0 failed，0.44 秒 |
| `cargo test -p echo-core --lib` | **749 passed，0 failed，1 ignored**，47.10 秒；包含跨进程锁、只读浏览、typed tombstone、fence 和 scoped-state 回归 |
| `cargo clippy -p echo-core --all-targets --all-features -- -D warnings` | 通过；新增锁生命周期测试对 intentional guard scope 作局部、带原因的 lint 说明 |
| `cargo fmt --all --check`、`git diff --check` | 通过 |
| `node scripts/verify/check-scale.mjs` | 通过，白名单未增加 |

Core 完整输出：`/tmp/echo-library-first-20261024-checkpoint-core.log`；Clippy 输出：
`/tmp/echo-library-first-20261024-checkpoint-clippy.log`。完整 Core 测试期间另有先前
启动的 governance/513 原生慢用例占用机器；47.10 秒是实际该次耗时，不是独立性能基准。
未重跑完整 workspace、governance、513 或三平台验收，不将其记为通过。

独立 reviewer 复查了锁取得后的所有早退路径、字段析构顺序和真实目录 fixture，
未发现本轮可操作缺陷；并独立复跑成功/失败路径及 `sync_cursor`，全部通过。
主 Agent 另外执行 `cargo test -p echo-core --test sync_cursor`（1/1，0.47 秒）及
`cargo test -p echo-core failed_write_preparation --lib`（1/1，0.01 秒），均通过。
本轮修复已验收，但下表缺项仍不允许勾选整个任务 3.1。

## 任务 3.1 自动化事实盘点

| 验收点 | 已有自动化证据与边界 |
| --- | --- |
| 同库两个写入者 | `library_durability_cross_process_writer_exclusion_and_reader_access` 启动真实子进程，验证有锁时可读、不可提交；本轮成功/错误路径重复句柄测试补充独立 adapter 互斥与释放。尚无此处的高并发持续压测。 |
| 切库、取消、退出 | 既有 `switch_cancels_active_scan_and_advances_epoch`、扫描取消和 root-switch 状态机测试提供一般生命周期覆盖，但尚未证明持有**真实目录 permit**、处于提交/恢复边界时取消、切库或正常退出会等待安全边界。任务此部分仍缺专项。 |
| 只读浏览 | `library_durability_read_paths_do_not_create_or_acquire_write_lock` 检查锁缺失时不创建锁、外部持锁时仍可读；Unix 的 `library_durability_readonly_volume_and_unmaterialized_commit_can_be_browsed` 在 `echo/` chmod 0555、缺锁且未物化提交时浏览成功。本轮完整 Core 均通过；不是原生只读挂载三平台认证。 |
| 符号链接、越界 | `library_durability_symlink_unknown_format_and_invalid_resource_fail_before_marker`、`record_symlink_escape_fails_before_a_backup_or_fence`、`library_durability_domain_rejects_unsafe_resources_and_duplicate_typed_targets` 等覆盖控制面 symlink、资源越界与安全路径；已有缓存热读也重新拒绝 symlink。本轮完整 Core 通过；Unix symlink 分支不代表 Windows reparse point 原生验证。 |
| 长路径 | `media_path_respects_component_byte_limit` 拒绝 >4096 字节组件；Unicode 路径已有值类型及实际文件往返测试。尚无完整原生深层长路径 journal/锁/恢复矩阵，不能当作这部分完成。 |
| 未知格式 | `library_durability_commit_checks_future_format_before_recovery`、migration 的 `future_format_and_uuid_paths_are_rejected_before_mutation` 等拒绝未来格式并保留证据；本轮完整 Core 通过。 |
| 跨库隔离 | 已有 distinct directory/sync IDs、foreign batch/chain/checkpoint、root context 隔离测试；尚缺将真实 permit 与并发切库/取消组合的完整场景。 |

本轮没有改变目录提交协议或公共 API。仍需补齐上述生命周期及原生平台矩阵，
并继续统一提交协调器/可信 sync-state projector 等后续工作。
