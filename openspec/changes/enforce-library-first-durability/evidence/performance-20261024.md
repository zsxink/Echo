# 3.7 性能续作

本线程未修改/删除用户原性能交接文档；外部 Git 整理后该根目录文件已不在工作区，
原安全约束已固化到 design D2.1 与本清单。3.1 已完成，change 为 18/67；3.7 与
3.5 保留未勾选。
本阶段先验证 GC 证明/删除阶段的目录去重及 enrich 合并索引，所有必要文件 sync 保留，
不引入跨调用证明或导出缓存。安全边界已先写入 design D2.1。

## 证据与清单

- [x] 串行 Sol 只读设计核对当前源码与原动态采样；确认 scan 逐首提交、import 每首
  还另有 claim reservation，不能只合并最终 song 后宣称批量任务完成。
- [x] 当前源码重新构建原样 513：构建 6.20 秒，过滤器实际执行 1 项；
  `/tmp/echo-performance-513-before.log`；**120 秒上限内未完成，整组 SIGKILL 回收，
  exit -9**。包含构建，因此真实测试只知道运行约 113.8 秒后仍未结束，不能给完整耗时。
  本次 native 二进制 SHA-256：
  `ac05da771bfa1e3b4de0bdd1aa0691e711773404185832a06627f05d8fac3934`，
  大小 79,607,912 bytes、mtime Unix 1792837792；避免与原交接中的旧二进制混淆。
- [x] 运行中进行 2 秒原生 sample，约测试运行 46 秒时取样；
  `/tmp/echo-performance-513-before-sample.txt` 再次出现 commit_projection、
  cleanup_covered 与 sync_ancestors 热栈。命中计数不是 syscall 数或总耗时比例。
- [x] 目录去重及删除后统一 sync 的有效 RED：旧生产 0 passed / 3 failed，
  `/tmp/echo-performance-gc-red.log`。同一真实 FS 样例含 3 receipt、4 个必要文件，
  证明阶段目录 sync 18→6、删除后祖先 sync 9→3，文件 flush 仍为 4；最小实现后
  3 项 GREEN。此数量只属于该样例，不能换算整体运行加速倍数。
  最终同一样例直接观测日志：旧 `/tmp/echo-performance-gc-old-observed.log`
  为 4/18/9（预期 RED）；新 `/tmp/echo-performance-gc-final-green.log` 为 4/6/3、
  4 项 GREEN。临时旧 checkpoint 在脚本 finally 中已恢复，最后验证的是修复版。
- [x] GC 所有文件/唯一祖先在首次删除前完成；后续 receipt/目录失败全部 journal 保留。
- [x] 失败/重试重新证明；冷热缓存篡改保持，Windows 可写文件 flush 分支保留，
  不将本机测试表述为 Windows 原生认证。每个 proof 文件/唯一目录及删除后祖先
  失败恢复随完整 FS 组通过。
- [x] 目录集合收集后被替换为 symlink 的有效 RED：
  `/tmp/echo-performance-path-red.log`，0 passed / 1 failed；修复后每次实际目录 sync
  都重新经过受约束解析（根目录也重验），不沿原始 PathBuf 跟随越界链接；该回归
  随完整 FS 组确认 GREEN。
- [x] 状态合并索引保持原值替换、新值追加、first-match、scope、Vec 顺序和摘要：
  真实 exporter fixture 播种两个新 cursor，涵盖同 kind 异 id、异 kind 同 id、
  active/archived binding 及初始重复 first-match；与旧线性结果的 canonical digest 一致。
- [x] 首轮专项 GREEN、完整 fences/durability/SQLite sync 组、独立审查与主线程 Core 验证。
- [x] 主线程完整 Core 回归：**821 passed / 0 failed / 1 ignored**（41.66 秒），
  架构 8/8、其余集成 7+1+2+19 全过；`/tmp/echo-performance-core-final.log`。
- [x] 独立审查：首次因 account usage limit 失败未计通过；用户继续后重试完成，
  Sol 只读核对真实 4/18/9→4/6/3 日志、GC 屏障、路径约束及 merge first-match/
  顺序/摘要，未发现可操作问题。该结论不替代原样 513、3.7 或 3.5 的完整验收。
- [ ] 最新原样 513 单进程有界验证，分别记录构建与测试耗时。
- [x] 最新样本确认剩余热点：100s 业务线程 1592 命中中，chain inclusive 975、
  recover 1158、GC 357，export/enrich 各 18；45s 为 chain 571/1318、GC 466、
  enrich 13。是采样窗口内的 inclusive 命中，不能转为全程比例。
- [x] `mark_published` 精确单 ledger after-image，证明此前目录事实后才省略无关资料；
  旧/未播种路径完整捕获，重试/跨 binding/资源/压缩/wipe 回归。
- [ ] 4.2 host 纵切建立已验证水位共同基线，再推进通用 delta；不得只删全量导出。
- [x] 小规模扫描/导入（reservation 与 song）提交次数及资源声明观测，见下节。
- [ ] 10k 实际 IO 测量、单次编辑链扫描数；均超出 3.7 范围。

## 小规模扫描与导入（task 3.7，2026-10-26）

- `library_durability_small_scan_commits_one_record_and_resource_per_song` 用 3 首不同音频验证：首次扫描产生 3 个目录 commit，每首为 1 条 Song after-image + 1 项已校验资源；重复扫描保持 3 个 commit 和 3 个 UUID，不为 unchanged 文件再次写目录。
- `library_durability_small_import_records_reservation_and_song_commit_per_success` 用 2 个独立来源验证：导入成功产生 4 个目录 commit（每首先保存 identity reservation，再保存 Song after-image），总计 2 条 Song 记录和 2 项媒体资源；reservation gate 无违规。
- 故障追赶由 `library_durability_scan_saves_identity_before_projection_and_reuses_it_on_retry`、`recovery_commits_the_directory_record_before_the_sqlite_row` 与 `library_durability_import_recovery_rebuilds_reserved_identity_in_a_fresh_database` 覆盖。当前通过：scan 57/57、import 62/62（包含本轮小规模计数测试）。
- 以上是序列化目录提交和资源声明次数的协议级观察，不是 OS write/fsync syscall 计时；没有延后唯一权威提交，也没有执行 10k 规模测量。原样 513、单次编辑链扫描数和更大规模测量仍待后续独立性能工作。

本阶段不缩减 513，不与其他 Cargo 长测并行，不把局部优化或超时当作通过。

代理阶段验证：完整 FS durability 组 **53 passed / 0 failed**，26.00 秒，
`/tmp/echo-performance-fences-green.log`；全 Core durability 组 **138 passed / 0 failed**，
29.42 秒，`/tmp/echo-performance-durability-green.log`。Core all-targets/all-features
Clippy（7.11 秒）、fmt/scale/diff 通过。代理已归还 Cargo，无遗留验证进程；
主线程完整 Core 复核 821/0/1 与全部集成通过，详见上表。
外部合并后主线程完整 Desktop **430 passed / 0 failed**，2.44 秒，
`/tmp/echo-performance-desktop-final.log`；workspace all-targets/all-features Clippy
通过，22.23 秒，`/tmp/echo-performance-workspace-clippy.log`。
最新 native 短组 **130 passed / 0 failed / 1 ignored / 1 显式过滤 513**，13.95 秒，
`/tmp/echo-performance-native-short.log`；该结果早于单 ledger 新改动。
外部合并使 E2E mock 达 1003 行，封面 URL helper 原样拆至 `mock-cover.ts` 后，
scale、两文件 lint/format、typecheck、build 与 Chromium e2e 均通过；
构建与 E2E 日志为 `/tmp/echo-performance-frontend-{build,e2e}.log`。未扩大白名单。

## 首轮优化后的原样 513

`/tmp/echo-performance-513-after.log`：构建 20.02 秒，原样过滤器实际 1 test；
**180 秒上限内未完成、进程组回收、exit -9**。约 159.98 秒测试运行后仍未完成。
前后两次都是超时下界，不能据此计算整体加速或回退倍数，也不能计为验收通过。
下一步增加不改变规模/断言的阶段计时，并在最新源码有界采样，定位剩余成本。

阶段计时跑次 `/tmp/echo-performance-513-phases.log`：构建 3.93 秒，fixture
246.108ms；发布 host sequence 32/513 用时 24.635 秒，64/513 用时 95.157 秒；
180 秒整组上限内仍未到 96，尚未进入 pull。样本位于
`/tmp/echo-performance-513-latest-sample-{45s,100s}.txt`，每次 2 秒。
不能把本跑次表述为分页 pull 慢；下一步依据最新样本定位增长成本。

## 外部 Git 状态变化

本线程没有执行提交、合并或推送。共享工作区经外部操作将先前改动纳入 `6d4a8fc`，
并合并 main 至 `2ae81ca`。`6d4a8fc..HEAD` 无 Core 差异；Desktop 新增外部
`platform/security.rs` 差异（71/8），主线程会按当前源码补 Desktop 验证，保留这些变更。

## 单 ledger 发布子阶段

实现集中在 `sqlite/sync/durability/{commit,publication}.rs`，仅 `mark_published`
选择 focused capture；完整捕获和 focused 捕获共用目录提交/不确定操作查询/错误映射。
不修改 FS、ports、schema，也未把原规划事务提交改成 4.2 的唯一投影。

只有同 root 已保存、非 NotSaved 的 host-confirmation operation，且 manifest library ID、
来源、唯一完整 typed key、binding、seq、原 body/digest 与全部其余字段完全匹配时，
才提交 records 为空、仅一条完整 ledger 的 batch。缺失/legacy/不匹配继续全量捕获。
原 publication operation 重试继续使用原 batch，但先核对当前完整 ledger；不匹配
conflict 并回滚 SQL，避免旧 authority/epoch 的 operation 授权新 SQL 变化。

- [x] 真实第三对象 RED：`/tmp/echo-publication-delta-red.log`，23,158 bytes、
  records=3、state=11；focused GREEN 为 3,243 bytes、records=0、state=1。
  原文、digest、资源及其余字段逐值一致；该计数只属于该 fixture。
- [x] 同根旧 authority/epoch identity 冲突 RED：
  `/tmp/echo-publication-scope-red.log`，旧行为错误返回 Ok；修复后拒绝、SQL 仍 pending。
- [x] 最终专项 `/tmp/echo-publication-delta-refactor-focused.log`：9/0，7.92 秒。
  包括 legacy SQL-only 全量捕获/wipe、同 seq 跨 root/binding、元数据不匹配 fallback、
  wire resources/digest、跨 WINDOW 原 stage/publication 精确复读/重试与 folded state
  wipe 逐值比较，冻结 mutation/disposition、alias、receipt、cursor、publication、
  无关事实和资源均保留。marker 后 SavedPendingRecovery 与 SQLite deferred COMMIT
  失败后的 SavedPendingProjection 也已实际执行。
- [x] 完整 durability `/tmp/echo-publication-delta-durability-final.log`：147/0，34.67 秒。
  后续只有测试 fixture helper 抽取与冗余 clone 清理；上述专项在整理后重跑 9/0。
- [x] Core all-targets/all-features Clippy：
  `/tmp/echo-publication-delta-clippy-final.log`，通过，2.92 秒；fmt/scale/diff 通过。
- [x] 主线程完整 Core 复核：`/tmp/echo-publication-core-final.log`，单元 830/0/1 ignored，
  48.16 秒；架构 8/8、其余集成 7+1+2+19 全部通过。
- [x] 主线程最新 Desktop 430/0（2.42 秒）、native 短组 130/0/1 ignored/
  1 显式过滤 513（13.50 秒）；`/tmp/echo-publication-desktop-final.log`、
  `/tmp/echo-publication-native-short.log`。短组不替代原样 513。
- [x] 主线程 workspace all-targets/all-features Clippy 通过（21.16 秒），
  `/tmp/echo-publication-workspace-clippy.log`。
- [x] 串行 Sol 独立审查无有证据、可操作的新问题；只读核对源码与现有真实日志，
  未重跑 Cargo。合法 unseeded fullcapture 的静态重试路径一致，未发现错误拒绝反例；
  现有 legacy fixture 未直接再次调用 mark_published，不将未执行测试计为通过。
- [x] 新源码原样 513 已实际有界测量，但未通过：`/tmp/echo-publication-513.log`，
  构建 24.28 秒；fixture 236.240ms；host 32/64/96 分别为 19.117/66.583/145.383 秒。
  180 秒整组上限仍在 host 发布，未进入 pull；整组 SIGKILL 回收、exit -9，
  测试运行约 155.72 秒仍未结束。45s/100s 各 2 秒 sample 位于
  `/tmp/echo-publication-513-sample-{45s,100s}.txt`。不能将该次测量计为完整 513 通过。
  32/64 阶段相较前次 24.635/95.157 秒下降；两次均未跑完，不能计算整体加速比。

限制：旧 host-published identity 仍只有 library ID/seq，跨 epoch 碰撞现安全拒绝；
新的完整 scope 身份及跨 epoch 后续发布可用性仍需 7.3/7.7，未宣称完成。
通用 delta、4.2、有界 scan/import、10k 和完整 513 仍未完成，3.7 不勾选。
