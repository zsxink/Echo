# 2026-10-24 协议字段与历史缺省契约（任务 2.4）

新增 [协议说明](../../../../docs/library-durability-protocol.md) 并从恢复文档链接，
对照实际 Domain/FS 实现说明独立版本、完整 batch/commit、检查点、typed
墓碑/别名、scoped state 和 frozen/disposition 字段。明确当前 manifest/envelope 2
与 marker/checkpoint 1，不把目标 manifest 3、全部 typed 语义、GC、可信 projector
和兼容矩阵写成已交付。历史 Song 的 0 回退与新增 Optional 时间省略分别记录。

新增固定历史 playlist/member 缺字段回归：v1 解码后封装为 v2，原未知创建/加入
时间、封面及确认信息继续缺省，再经完整 commit 序列化、反序列化和摘要校验。
既有旧 state 缺 key 的固定 canonical JSON/digest 回归未改写。

主 Agent 实际验证：

| 命令 | 结果 |
| --- | --- |
| `cargo test -p echo-core library_durability --lib` | 74 passed，0 failed，35.77 秒；本次新增缺省用例添加前的协议组 |
| `cargo test -p echo-core library_durability_protocol_keeps --lib` | 新增缺省回归 1 passed，0 failed |
| `cargo test -p echo-core portable_v --lib` | 4 passed，0 failed，含 typed 别名/墓碑及未知版本拒绝 |
| `cargo clippy -p echo-core --all-targets --all-features -- -D warnings` | 最终通过；新测试数字格式告警已修复 |
| `cargo fmt --all --check`、scale、`git diff --check` | 通过，白名单未增长 |
| `openspec validate enforce-library-first-durability --strict` | 通过 |

独立 reviewer 对照实际字段、常量、摘要输入与序列化 fixture，未发现可操作问题。
任务 2.4 已勾；2.1/7.8/8.2 等完整实现仍按各自标准验收。原生 513、最终 workspace
和三平台验收没有被本文件认定为通过。
