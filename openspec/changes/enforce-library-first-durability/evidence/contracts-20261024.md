# 2.3 confirmed 与 publication 契约补充

本轮子代理严格逐个派发：Luna 只读定位 → Sol publication 分析 → Sol 独立审查 →
Luna portable 字段核对 → Sol 最终审查。生产代码与测试由主线程编辑。

## 已完成并逐项验证的子步骤

- [x] 真实 host 带音频/歌词声明的确认导出为 confirmed.v2；修复 LAN `content_hash`
  与 Domain `digest` JSON 形状混用。原始 LAN 字符串及摘要不改，保留 base/前驱。
  回归先失败于实际 exporter 仍输出 v1，修复后通过。
- [x] active/archived 两个完整 binding 的同 seq=1 不串确认；缺 ledger 或相同 seq
  但对象不同的历史事实保持 v1，重复 enrichment 的序列化字节不变。
- [x] confirmed 的 typed aliases、必需原字段及畸形 mutation 拒绝；正向保留原始
  pretty JSON 与 alias/base/前驱。独立审查发现的非对象 IndexMut panic 先复现再修复。
- [x] 真实 host dedup 生成的 typed song alias 随完整确认保留。
- [x] resource-publication.v2 的原正文、root/operation/completed、owner、资源声明
  版本、实际/旧/staging 位置、长度/摘要/MIME 和字段完整性严格校验；Domain 不引入
  Application 依赖。缺字段与嵌套身份冲突先观察失败再修复。
- [x] 真实 SQL prepare/save/finish 的 exporter 输出完整 publication.v2，保持
  exact original_body，未完成与完成状态分别往返。旧 v1 和未知 schema 仍保留证据。
- [x] v1 显式 null 墓碑通过独立可信 ledger 验证后恢复 SQL NULL，连续两次重试一致；
  回归先失败于 InvalidStateFact，再修复。v2 marker 的既有墓碑回归继续保留。
- [x] populated portable v1/v2/目录 commit 矩阵保持歌单时间/封面引用、成员三方 UUID/
  顺序/加入时间、喜欢 false、覆盖层文本和逐设备桶；旧 absent 回归继续保留。

## 任务边界

2.3 整项保持未勾选：目标 manifest v3 的正向往返与完整升级围栏仍待实现，不能把
当前拒绝未知版本 3 的测试替代它。3.4/4.2/4.5/7.4 及其他入口任务没有因此完成。
publication 中的 declaration digest 是资源集合版本，不能冒充未捕获的主机
object_version。恢复这些凭据不授予联网、文件可用或完成资源发布的权限。

## 主线程实际验证

| 命令 | 结果 |
| --- | --- |
| `cargo test -p echo-core --lib` | 803 passed / 0 failed / 1 ignored，45.29 秒；日志 `/tmp/echo-library-contracts-core-20261024.log` |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 通过，1 分 41 秒 |
| `cargo test -p echo-desktop --lib` | 423 passed / 0 failed，2.41 秒；日志 `/tmp/echo-library-contracts-desktop-20261024.log` |
| `cargo fmt --all --check` | 通过 |
| `node scripts/verify/check-scale.mjs` | 通过，白名单未增长 |
| `openspec validate enforce-library-first-durability --strict` | 通过 |
| `git diff --check` | 通过 |

全量 Core 前的 `library_durability --lib` 跑次为 125/125，之后三个新增专项
（真实 host alias、legacy null 墓碑、populated 字段矩阵）分别通过，并全部纳入
上表最终完整 Core 跑次。最终 Sol 独立审查无新增可操作问题。

本轮发生过一次短暂测试调度失误：125 条跑次尚未结束时启动了单项 alias 跑次，
两者均完成通过。此后已确认前一进程结束才启动下一组；没有并行 workspace 长测。
原样 513、workspace/all-features 测试、governance 和三平台原生验收仍未完成。

## 3.1 下一步的真实生命周期缺口

后续串行 Sol 只读分析核实：`ActivateLibrary::activate` 在 cancel_all 后直接推进
activation；`ScanSupervisor` 只置取消标记，没有等待机制；桌面正常退出路径停止
LAN runtime、flush effects，但没有排空本地扫描/目录提交。现有 `LOCAL_ACTIVITY`
共享租约覆盖部分本地工作，切库/退出没有取得独占屏障。不能据此认为取消已到达
持久安全边界，也不能用测试线程自行 join 代替生产等待。

3.1 继续保持未勾选。完整修复须明确停止新工作准入、取消、排空到安全边界，再推进
activation/exit，并在失败时恢复旧库准入；需要核对嵌套共享租约与 SQLite actor 的
锁顺序，避免直接增加独占锁造成重入死锁。本轮没有修改这部分生产代码。FS 深层
Unicode/长路径和真实持 permit 的确定性边界测试也仍欠缺。
