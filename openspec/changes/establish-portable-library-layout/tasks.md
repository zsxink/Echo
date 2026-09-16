## 1. 领域模型与数据库演进

- [x] 1.1 在 `echo-core` 定义资料库布局、受限相对路径、manifest、可携带对象、设备 ID 和 HLC 的领域类型与校验规则；为绝对路径、路径逃逸、控制目录路径、Unicode 路径和版本不兼容添加单元测试并运行 `cargo test -p echo-core`。
- [x] 1.2 新增顺序 SQLite 迁移和 Repository 映射，为可同步对象写入设备/HLC、歌单成员稳定 UUID及其墓碑提供持久化；验证全新库与已迁移库均通过迁移集成测试，且不修改已发布迁移。
- [x] 1.3 扩展同步对象/Outbox 领域端口，使歌曲、收藏、歌单、成员、覆盖层和删除使用完整对象载荷与单调 revision；为同一对象连续写入及并发顺序提交添加测试并运行 `cargo test -p echo-core sync`。

## 2. 可携带控制面适配器与恢复

- [x] 2.1 在 Infrastructure 实现 `echo/manifest.json` 与分片对象资料的原子读写、格式校验和 `echo/tmp/` 临时文件忽略策略；为半写 JSON、未知格式、分片路径和无敏感字段添加文件系统测试。
- [x] 2.2 实现新资料库初始化 manifest/对象资料及由对象资料投影新 SQLite 的用例；验证重复执行不重建歌曲、收藏、歌单或成员 UUID。
- [x] 2.3 实现“对象资料先行、媒体后到”的恢复协调：按 library ID 校验、投影对象、扫描 `media/`、哈希关联并保留 missing 状态；增加跨设备恢复端到端夹具并运行对应 native/Core 测试。

## 3. 导入、扫描与操作恢复

- [x] 3.1 修改导入路径规划，将所有新媒体和同名 LRC 发布到 `media/<artist>/<artist> - <title>`，同时保留安全命名、BLAKE3 去重和编号冲突策略；更新/新增导入测试并运行 `cargo test -p echo-core import`。
- [x] 3.2 将导入暂存限定到专属 `echo/tmp/<operation-id>`，并在操作日志恢复中协调媒体发布、SQLite 写入、对象资料物化与失败回滚；为每个崩溃窗口添加可重复恢复测试。
- [x] 3.3 修改扫描与 watcher，仅发现 `media/` 并忽略 `echo/`（包括 `echo/tmp/`）及其他根目录内容；验证旧布局被明确拒绝、`media/` 导入可发现且控制面不出现在歌曲列表。

## 4. 歌单、收藏与跨层装配

- [x] 4.1 更新歌单成员创建/删除用例和 SQLite 适配器，使用稳定成员 UUID、追加顺序键与成员墓碑；验证跨歌单添加产生不同成员 UUID、重复添加保持原位置、删除可被恢复流程消费。
- [x] 4.2 更新收藏和其他可同步变更的事务边界，保证正表、outbox 与可携带对象资料在可恢复流程中一致；验证任何载荷均不包含本机绝对路径、数据库路径、凭据或播放状态。
- [ ] 4.3 在 desktop composition root 装配资料库控制面与恢复服务，但不装配 RemoteConnector、不新增网络依赖或同步 UI；运行 `cargo test -p echo-desktop` 和桌面 IPC 相关测试。

## 5. 文档与全量验证

- [x] 5.1 更新 `docs/DESIGN.md`、`docs/ROADMAP.md` 和相关示例，将新导入布局改为 `media/` 并说明 `echo/`、`echo/tmp/` 与 SQLite 的边界；人工核对文档不承诺一期同步入口。
- [x] 5.2 更新测试夹具和原生验收材料，覆盖新资料库、旧布局拒绝、控制面不可写、新设备全量恢复与媒体缺失恢复；运行 `pnpm --filter @echo/desktop test`（或项目实际等效测试命令）。
- [ ] 5.3 执行交付检查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace`、`pnpm lint`、`pnpm typecheck`、`pnpm test` 和 `openspec validate establish-portable-library-layout --strict`，修复本 change 引入的失败后记录结果。
