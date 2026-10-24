# 3.1 生命周期实施中交接（用户要求停止）

用户在本轮明确要求“停止任务，做一下交接”。主线程已中断实现代理，只执行
进程回收、状态核对与文档交接，不再新增实现或启动验证。所有子代理均串行。

## 分支与进度

- 分支 `codex/issue-72-library-first-durability`，改动未提交、未推送、未归档、未创建 PR。
- OpenSpec 仍为 **17/67**。2.3 与 3.1 整项未勾选；2.3 已验证子步骤见
  [契约清单](contracts-20261024.md)，其中 v3 正向往返仍未完成。
- 保留原未跟踪文件 `性能优化交接文档-2026-10-24.md`。
- 主线程停止后用 `ps` 核对未发现 Cargo、rustc 或 Echo/Core/Desktop 测试进程；
  代理最后 session `58397` 已结束并收取，没有待继续的后台验证。

## 已落盘的 3.1 实现（尚未最终验收）

- Core 新增 `application/local_activity.rs`：实例级 Mutex/Condvar 屏障，同线程
  同屏障嵌套租约、不可跨线程移动的 RAII lease、关闭/排空/永久封闭、自持 lease
  时拒绝关闭；现有 process-global RwLock 使用点迁移到显式装配的共享实例。
- `ScanDeps` 共享 gate；Core scan/recover/trash/watch/remote_delete/root_switch
  接线；candidate prepare 整段持租约；activation 关闭准入、取消、排空后重新读取
  candidate/blockers/active root/epoch，再提交事务，成功/失败均 RAII 恢复准入。
- scan 注册 token 后检查关闭，最终 reconcile 后、missing 前重新检查取消。
- Desktop 写守卫、recorder、recovery 三个可能写目录的入口接 gate；trash worker
  在租约内解析活动根；系统 import picker 不再跨活动租约。
- `AppServices::shutdown` 关闭准入、取消、显式停止外部 producer、排空并永久封闭；
  stop 失败恢复准入。
- native 新增 `shutdown.rs`，bootstrap/main 装配 ExitRequested 暂缓退出、后台单次
  shutdown、成功后按原 code 最终退出；provisioner 的独占入口迁移到同一实例。
- RootControlPlane 新增实例级 `cfg(test/testkit)` commit hook，未增加端口方法数。
  新增真实 SQLite/RootControlPlane 时序、长 Unicode 根、跨进程 permit/独立根测试。
- `design.md` D3.1 已记录上述决策。这不代表 4.2 全部生产写入口已统一收口。

## 最后改动：generation 与扫描 RED

主线程预审发现：等待 enter/exclusive 的线程可能错过一次很短的 close→reopen
脉冲，切库后继续执行关闭前排队的旧操作。代理已加 `generation`，关闭时推进代次，
等待者发现代次变化即拒绝，既有嵌套操作仍可完成。

确定性回归 `queued_exclusive_cannot_miss_a_complete_temporary_closure` 已写，
**Core 该单测尚未编译/运行**。generation 生产代码经随后 Desktop 专项编译。

最后为确认扫描缺陷的有效 RED，代理脚本暂时撤掉最终取消检查，真实扫描测试失败于
`cancellation during final commit was lost`。脚本 `finally` **已恢复修复版**；代理
及主线程只读核实 `scan/mod.rs` 仍有最终 token 重查。**恢复后未再次运行 GREEN。**
不得把最后一次 RED 误报为当前修复仍失败，也不得当作恢复后的最终通过。

## 实际验证及时间边界

| 检查 | 真实结果与限制 |
| --- | --- |
| activation 旧生产行为 RED | 1 failed：prepared 写入暂停时 activation 提前返回；`/tmp/echo-lifecycle-red.log` |
| shutdown 旧 stop-only 行为 RED | 1 failed：prepared 写入暂停时提前返回；`/tmp/echo-shutdown-red.log` |
| Desktop 第一组 lifecycle GREEN | 3 passed，activation/shutdown 在 prepared 与 committed 两阶段等待、stop 失败恢复；`/tmp/echo-lifecycle-green.log` |
| `cargo check --workspace --all-targets` | 当时通过，含 native/Core 单测目标；**早于最后 generation/扫描回归**；`/tmp/echo-check.log` |
| `cargo test -p echo-desktop --lib lifecycle -- --nocapture` | **7 passed**，1.36 秒；`/tmp/echo-lifecycle-all.log`；早于最后扫描 RED/恢复 |
| 最后扫描旧逻辑 RED | 1 failed；`/tmp/echo-scan-red.log`；结束后恢复修复但尚未复跑 |
| scale | 中途通过、allowlist 未增加；早于最后新增测试，不当作当前最终结果 |
| 主线程停止后 `git diff --check` | 通过；不是 fmt/编译或功能验证 |

7 项 GREEN 中含 6 项新增真实测试和 1 项已有 status-menu 测试：activation 等待、
shutdown 等待且拒绝新写、stop 失败恢复、排空后重读 blockers 并保留旧库、长 Unicode
根真实 commit/reopen/recover/retry、最终目录提交期间取消不标记 missing。

代理 Cargo 验证均串行，通过 `/tmp/echo-cargo-run.py` 设置 120/180 秒硬超时并回收
进程组，未发生超时。最初一次 Core `--exact` 过滤成 0 tests，明确弃用，不计通过。
独立 rustc 对新模型占位的 1 failed 只作 API 语义 TDD，不替代真实生产 RED。

**契约组的 Core 803/0/1、Desktop 423/0 与 workspace Clippy 是生命周期改动前结果**，
详见 `contracts-20261024.md`；它们不能作为当前工作树的全量验证。

## 下次必须先做

1. 读取本文件与 checkpoint 顶部，CodeGraph 核对 WIP；先把树整理为可编译和格式化。
   不回退已验证的 confirmed/publication/legacy tombstone/portable 矩阵改动。
2. 先执行非零测试数的 Core LocalActivity 单测（含 generation）、新跨进程 permit/
   独立根测试、native shutdown 两项单测，以及恢复后的 Desktop lifecycle GREEN。
   跨进程 permit 目前仅在此前 check 中编译过，尚未实际执行。
3. 串行 Sol 独立审查生命周期装配、嵌套租约、排队请求代次、actor/FS 锁顺序、
   activation 错误回退及 ExitRequested 时序。producer closure 当前停止 sync/trash；
   recorder 仅由共享 gate 阻止后续写入，没有新增 stop/join 句柄，需判断保证是否充分。
4. 完成 fmt、scale、workspace check/Clippy、Core/Desktop 全量及受影响 native 短组。
   exclusive-create、安全路径、未知格式既有回归本组还未重跑；原生 UI event loop/
   picker/ExitRequested 尚未实机验收。
5. 全部 3.1 行为与规定验证闭环后才勾整项。4.2 仍提交原规划 SQL 事务，v3、资源
   publication 恢复、完整入口及其他任务依旧未完成。继续严格串行子代理/单组 Cargo。

原样 513、workspace/all-features 测试、governance 与三平台原生验收本轮没有完成，
不要重启多个并发长测，也不要用过滤后的 native 短组替代完整验收。
