# 3.1 生命周期续作验收

本清单接续 [停止时交接](lifecycle-handoff-20261024.md)，逐项记录当前工作树验证。
**3.1 已完成并勾选，change 进度 18/67。** 下述行为、独立复审及门禁闭环；
子代理严格串行，Cargo 验证也串行。未提交、未推送、未归档、未创建 PR。

## 验收清单

- [x] Core LocalActivity 既有 5 项专项通过（实例隔离、关闭拒绝新工作、嵌套排空、
  关闭唤醒等待独占、排队独占不能错过 close/reopen）；
  `/tmp/echo-resume-activity.log`：5 passed / 0 failed / 806 filtered。
- [x] 真实 native peer → begin_root_handoff → stop-only shutdown → abort 回退
  RED 有效：`failed activation restarted sync after shutdown`；
  `/tmp/echo-native-shutdown-fence-red.log`，1 failed。
- [x] native 退出专用永久启动围栏及失败策略，通过真实回退/恢复/启动专项。
  代理 `sync_runtime::handoff::tests`：14 passed；
  `/tmp/echo-native-shutdown-handoff.log`。包含普通 stop 可重启、退出失败保留围栏、
  启动/退出串行、自动 retry 与 stop/handoff 不死锁、公共 restore waiter 清理。
- [x] native request 所用后台 dispatcher 验证重复退出、停止生产者及后台完成；
  代理 dispatcher 专项 3 passed。此处为生产共用调度逻辑测试，非 UI 实机认证。
- [x] queued enter 的完整 close/reopen 脉冲专项：撤掉 reader generation 判断得到
  有效 RED（1 failed）；finally 恢复后 LocalActivity 6/6 GREEN。
  `/tmp/echo-queued-enter-red.log`、`/tmp/echo-resume-core-activity-green.log`。
- [x] 真实 SQLite 激活事务回滚保留旧根/epoch，恢复准入后实际目录写入成功。
  在 root flip 后的 epoch SQL 注入 `RAISE(ABORT)`，确认旧活动根及 epoch=7、候选
  inactive；移除 trigger 后，经恢复的 gate 和生产 `DurableLibraryEdits` 保存歌单，
  真实 RootControlPlane replay 中有同 UUID，另一库为空；再次 activation 成功到 epoch=8。
- [x] 跨进程 writer permit、独立根互不污染专项，确认子进程非零测试数。
  `/tmp/echo-resume-core-lifecycle.log`：7 passed / 0 failed；其中上述 SQL 专项 1 项，
  permit 父测试及 3 个 child 均实际执行。首次 SQL 测试误用旧 DB-only helper，目录
  断言失败；改用现行生产 durable 用例后通过，未改变生产逻辑规避失败。
- [x] 修复版 Desktop lifecycle 全组 GREEN，含最终目录提交中取消。
  `/tmp/echo-lifecycle-desktop-final.log`：完整 Desktop 429/429，2.40 秒。
- [x] 既有只读、危险路径、未知格式、exclusive-create 回归随完整 Core 全部通过；
  `/tmp/echo-lifecycle-core-final.log`：单元 814 passed / 0 failed / 1 ignored（46.52 秒），
  架构 8/8，其余集成 7+1+2+19 全过。该全量之后仅提取 SQL 测试 fixture helper，
  后续专项和 Clippy 会验证该等价整理。
- [x] 新增 exclusive-create 正常及竞争专项：目标只在完整临时文件 sync 后可见；
  目标预先存在、file sync 后被其他写者抢先创建两种情况均拒绝替换并保留原件，
  清理临时文件；`/tmp/echo-exclusive-publication.log`：2/2。
- [x] 串行 Sol 最终独立审查无未解决问题：退出 P1 及自动恢复 P2 均经独立复审；
  恢复/代际复核/结果发布共用 guard，普通锁竞争延期不耗预算，停止/退出 join 无反锁。
- [x] Core/Desktop 全量（结果及测试 fixture 整理的后续验证范围见上）。
- [x] workspace all-targets/all-features check/Clippy：
  `/tmp/echo-lifecycle-workspace-check.log`（18.90 秒）、
  `/tmp/echo-lifecycle-workspace-clippy.log`（最终 5.88 秒）。初次 Clippy 的 callback
  type alias、提前释放测试 guard、移动 child helper、测试路径和文档标记问题均已修复，
  没有新增 lint suppressions。整理后 lifecycle 7/7（含 3 个非零 child）和 gate 6/6
  再次通过：`/tmp/echo-core-lifecycle-cleanup-green.log`、
  `/tmp/echo-local-activity-cleanup-green.log`。
- [x] 最终受影响 native 短组：130 passed / 0 failed / 1 ignored / 1 显式过滤 513，
  14.09 秒；`/tmp/echo-native-retry-short-final.log`。native all-targets/all-features
  Clippy（含依赖）通过；`/tmp/echo-native-retry-clippy-final.log`。
- [x] fmt、scale、OpenSpec strict、diff 检查；白名单未增长。

## 退出围栏决策

独立审查发现退出与切库错误回退之间的竞态：停止现有同步生产者后，旧的
`abort_root_handoff` 仍可能恢复旧会话。退出需要受 sync transition 锁保护的
永久启动围栏，覆盖启动、恢复和 handoff 回退入口；普通用户停止同步仍须可重启。
退出清理失败时保留该围栏以允许再次清理，不能恢复生产者。专项通过，最终独立审查
和工作树全量验证尚在进行。

本清单不替代 4.2 的完整写入口收口、10.3 三平台原生验收或原样 513 长测。

## 最终审查补发现

串行 Sol 审查确认退出 P1 已修；另发现自动恢复在有效 generation 的普通 transition
锁竞争时会无声退出（P2）。正在按同一临界区执行恢复与结果发布，并使普通竞争不消耗
实际尝试预算；实例测试 hook 与暂停的真实 RecordingProvisioner 已得到确定性 RED：
有效代际普通竞争错误地把 attempts 从 0 增到 1，之后未报告 exhausted；
`/tmp/echo-native-retry-contention-red.log`，1 failed。尚须 GREEN、复审及最终
native/workspace 门禁。

P2 后续 GREEN：同一个 try_lock guard 覆盖 restore、generation 核验和状态发布；
WouldBlock 返回 Deferred，由 while 循环延期且不消耗实际 attempt。新竞争回归 1/1、
handoff 整组 15/15，`/tmp/echo-native-retry-handoff-green.log`。主文件缩至 876 行，
scale 通过且白名单未增加。随后独立复审无未解决问题，最新 native 130 项短组及
workspace 门禁通过，至此完成本项。UI 实机和三平台认证仍属于 10.3。
