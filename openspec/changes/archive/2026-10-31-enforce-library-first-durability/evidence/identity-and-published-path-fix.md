# 2026-10-23 身份与已发布媒体路径修复

本记录是历史专项修复证据；当前分支与整体进度以[当前交接](handoff-20261025.md)和 tasks.md 为准。

## 修复边界

- 本地 `domain::library::LibraryId` 与 `domain::sync::LibraryId` 独立生成。移除 envelope binding ID 与目录 ID 相等检查，以及通用 state JSON 内任意 `library` / `library_id` 字段的递归相等检查；不改 ID 生成、manifest 历史或序列化格式。
- 保留目录 manifest/batch/checkpoint/链、操作原正文及 SQLite watermark 的本地归属校验；实时当前完整 binding 的网络验证保持原有实现。历史 state 只保留为 `RequiresStateRecovery`，不安装 live binding、凭据或上传候选。
- 新增真实临时目录双 ID 提交、重试、不同新旧 binding state、压缩恢复与独立全新 SQLite 测试；真正跨本地库的 batch/链/checkpoint、水位与链拒绝测试，其中 SQL 拒绝路径直接断言 `Connection::total_changes()` 不变。
- 修复已完成 publication 对应的 `echo/media/` 实际版本路径进入逻辑 `SongRecord.media_path` 的错误：scan 提交与 SQLite typed 导出都从既有 completed publication 的当前歌曲关联取逻辑路径。物理资源校验与本机 SQLite 位置继续使用原实际路径；未声明控制面路径仍拒绝。
- full scan 对已发布 hash/length 未变的字节只刷新派生索引，使用原行保留共享元数据，不覆盖 envelope、不产生新候选。原生 HTTP/host-loop 测试故意使用不同于嵌入标签的共享 title，重写相同字节强制重解析并比较原 path、title、完整 envelope 与完整 pending 集合；之后继续真实外部改字节、watcher、host 再发布与 peer 再次更新全过程。

任务 2.1 的完整 typed state 与 7.7 adoption 未完成。现有 `(StateKind, id)` 覆盖命名空间可能覆盖不同 binding 的历史事实，必须在后续 typed 契约与历史归档中解决。本轮只修错误身份域比较，不将这些缺口标为完成。

## 实际验证

| 命令 | 结果 |
| --- | --- |
| `cargo test -p echo-core library_durability --lib` | 修复后 37 passed / 0 failed，21.10 秒 |
| `cargo test -p echo-core application::scan --lib` | 28 passed / 0 failed，1.80 秒 |
| `cargo test -p echo-app --bin Echo sync_runtime::tests::host_resources -- --nocapture` | 2 passed / 0 failed，10.55 秒；包含 HTTP、真实发布、完整重扫与 watcher |
| `cargo fmt --all --check` | 通过 |
| `git diff --check` | 通过 |
| `cargo clippy -p echo-core --all-targets --all-features -- -D warnings` | 未通过：lib 31 个、lib test 总计 42 个错误；包含当前 change 已存在的 missing_errors_doc、redundant_pub_crate、large_stack_arrays、items_after_test_module 等。没有将该命令报告为通过，也未在本轮无关扩展修复。 |

`cargo test -p echo-app --bin Echo` 已实际启动（108 tests），但未完成，退出 143。原全套使用首轮 identity 修复后的旧 binary，在 host_resources 后续全扫处发现 reserved echo media 路径错误；该路径现由上述最新定向测试覆盖并通过。全套最后仍运行的 `native_pull_drains_more_than_one_commit_page_without_losing_local_work` 被主 Agent 中止，不能据定向结果宣称全套通过。

## 性能缺口实测

`tests/merge_cycle.rs:189` 的准备循环为 `sequence in 2..=513`，逐个创建 512 条 host confirmation，随后才开始 pull。完整 suite 的 cargo PID 18145 / test PID 18298 运行 8 分 47 秒时，通过只读 SQLite 查询仍只有 host `songs=77`、`sync_commits=77`，replica `songs=1`、`sync_commits=0`，尚未进入 pull。该进程此前 CPU 约 98%，观察时约 21%；已明确看到有限准备循环中的严重退化，不是已验证的网络收敛失败或成功。

取证临时根为 `/private/var/folders/rx/cwp1dm6536789l30qt9hf9rm0000gn/T/.tmpM0c7ng`。主 Agent 随后 TERM 该 cargo/test 进程，runner 返回退出 143。全量 typed 导出与目录重材料化/链操作的增长需继续独立分析；此证据不完成任务 3.7 / 10.4 的性能验收。没有缩小 513 条场景、放宽生产 directory-first 要求或将慢测设为忽略。

全 workspace / all-features、governance、跨平台与独立审查由后续阶段继续；本轮未验证。
