# 3.7 性能续作

用户原性能交接文档保持原样。3.1 已完成，change 为 18/67；3.7 与 3.5 保留未勾选。
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
- [ ] 专项 GREEN、完整 fences/durability/SQLite sync 组、独立审查与主线程验证。
- [ ] 最新原样 513 单进程有界验证，分别记录构建与测试耗时。
- [ ] 后续有界扫描/导入（reservation 与 song）、10k 实际 IO 测量、单次编辑链扫描数。

本阶段不缩减 513，不与其他 Cargo 长测并行，不把局部优化或超时当作通过。

代理阶段验证：完整 FS durability 组 **53 passed / 0 failed**，26.00 秒，
`/tmp/echo-performance-fences-green.log`；全 Core durability 组 **138 passed / 0 failed**，
29.42 秒，`/tmp/echo-performance-durability-green.log`。Core all-targets/all-features
Clippy（7.11 秒）、fmt/scale/diff 通过。代理已归还 Cargo，无遗留验证进程；
主线程已启动完整 Core 复核，尚未完成。
