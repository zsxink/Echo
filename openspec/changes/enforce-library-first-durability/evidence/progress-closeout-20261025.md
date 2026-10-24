# 开发进度复核（2026-10-25）

本次只整理进度及证据，不修改实现、不提交、不归档。核对对象为
`codex/issue-72-library-first-durability` 当前工作树，包含未提交实现；这些结果不代表
`main` 已发布能力。任务清单由 17/61 更新为 **19/61**，新增完成 **2.3、4.5**。

## 验证环境与限制

原工作区 `application/import/tests.rs:33` 声明 `mod durability;`，对应文件缺失，
导致 `cargo test -p echo-core library_durability --lib` 和受影响 crate 的
`cargo clippy --all-targets -- -D warnings` 报 E0583，未完成检查。
`pnpm verify:governance` 的 spec/trace/manifest 对账为 461/461/461；后续无法列出
Core 测试，scenario command validity 失败。该结果不证明那些过滤器实际匹配零测试。
因此 **9.5、11.2 继续未勾选**。

为独立验证 2.3、4.5，将 `git ls-files --cached --others --exclude-standard` 所列
现有文件复制到临时工作区，仅在副本移除这条缺失模块声明。逐一比较 Rust 文件后，
确认唯一差异为该声明；生产实现、其余现有测试及共享工作区保持原样。临时副本为
`/var/folders/rx/cwp1dm6536789l30qt9hf9rm0000gn/T/echo-progress-audit-w58xhvv4`。
副本检查不包含缺失模块将来应有的导入测试，也不替代原工作区门禁。

副本实际执行 `CARGO_TARGET_DIR=/Users/xian/Project/music/Echo/target cargo test -p echo-core --lib`：
**865 passed、0 failed、1 ignored**，测试耗时 67.75 秒，进程 exit 0。
日志为 `/tmp/echo-progress-20261025-snapshot-core.log`，包含下述各项回归。
这不是原工作区完整 Core 通过，也未运行完整 workspace 或桌面全部测试。

原工作区 `cargo fmt --all --check`、`git diff --check` 和
`openspec validate enforce-library-first-durability --strict` 通过。

## 新增完成项

### 2.3：新协议 portable 字段与独立版本

- `domain/library/mod.rs` 当前 manifest 为 v3，对象 envelope 独立保持 v2。
- 完整字段往返覆盖歌单创建时间/封面原件引用、成员三方 UUID/顺序/加入时间、
  favorite false、覆盖层文本、逐设备桶及资源声明。
- confirmed v2、pending/receipt、publication v2 分开保存；原确认正文、alias、base、
  前驱及实际受管 owner/声明版本位置有严格校验，不把旧确认复制给新用户值。
- 副本中的 `manifest_v3_explicitly_reads_legacy_but_refuses_invalid_and_future_versions`、
  `library_durability_new_manifest_fences_legacy_writers_without_changing_record_envelopes`、
  `library_durability_portable_populated_v1_v2_commit_round_trip_preserves_all_user_fields`、
  `library_durability_protocol_keeps_unknown_historical_times_and_confirmation_absent`、
  `portable_v*` 及 confirmed/publication 字段回归实际执行通过。

这些结果补上 [10/24 契约证据](contracts-20261024.md) 当时缺少的 v3 正向往返与围栏。
不包含旧库回填，也不完成 3.4 的资源替换恢复或 7.8 的完整兼容矩阵/文档交付。
`docs/library-durability-protocol.md` 中“当前写 2”等版本描述仍需在对应文档任务中更新。

### 4.5：稳定 typed recovery key、读回与依赖重试

- `sqlite/durability_projection/retained.rs` 使用 serde 编码的 `ObjectKey` /
  `ProjectionTarget`，scoped state 保留完整 typed key；不以 `Debug` 输出作存储键。
- `read_recovery_records` 读回 records/state/resources；投影自动重试未解析关系。
- 副本中的 `library_durability_projection_reads_private_evidence_with_stable_typed_keys`
  验证 confirmed/pending 原文、稳定键、重开和重复投影保留证据。
- `library_durability_projection_retains_orphans_and_links_when_evidence_arrives_without_echo`
  验证缺父关系保留、追加父对象后自动关联且无上传回声。
- `library_durability_trusted_sync_retry_keeps_missing_parent_and_retries_real_directory_append`
  验证真实目录追加后的可信同步依赖重试；无凭据时不据此授权联网。

全部上述测试在副本实际执行通过。冻结 collector、全入口统一协调器、显式 adoption
分别仍属于 4.2/7.2/7.7，不作为本项追加完成条件。

## 其余已接线但尚未整项验收的进度

- 4.2：喜欢/歌单/统计已有规划回滚、目录提交及顺序追赶；导入/扫描和部分同步仍有
  per-operation 投影，未形成所有入口的唯一协调器。
- 4.3/4.4：目录到空 SQLite 的投影已有测试，临时库校验替换、坏库隔离和完整故障
  验收尚未闭合。
- 5.x：导入、扫描和 watcher 已有目录优先及身份复用；导入两段操作的自识别互引用、
  完整批量恢复/并发组合和 IO 记录仍缺。
- 6.1/6.2/6.3：喜欢/歌单/成员、封面原件和生产 recorder 已接 Core；成员完整 wipe
  比对、既有覆盖资源整体接入、ACK 幂等及桌面专项验收尚未齐全。
- 6.4/6.5/6.6：trash 仍有 DB-first 后写记录入口；写回旧字节保护与残留 builder
  清理未完成，不能宣称已无绕过路径。
- 3.5/7.x：checkpoint 私有历史/资源仍会累积，完整引用图 GC、remote publication
  当下的目录提交、冻结正文全入口捕获及 adoption 尚未完成。
- 8.1/8.2、9.x–11.x：完整 IPC/UI、综合验收、治理登记与交付流程继续按清单推进。
  没有新增三平台原生、完整 workspace 或原样 513 通过证据。
