# 内嵌标签写回同步修复验证

## 根因与现场证据

- 本机日志反复记录 `resource_not_found`，原桌面适配将其映射为 `sync.hostRejected`，错误提示指向配对关系。
- 只读检查《骆驼》的资料库记录：歌曲 UUID 未变，新音频哈希已入本机 pending，后续变更仍排队；当前资源声明是新版本，已发布资源位置仍关联旧版本。
- `pending_remote_resource_sets` 会看到本地更新后的资源声明；`merge_peer_library` 在上传前发布这些资源，因而请求主机尚未收到的新哈希并得到 404。
- 新端到端测试先执行主机→附属机首次同步，再改写相同中文路径的文件及资源声明；修复前稳定失败为 `SyncResourceUnavailable`。测试使用临时目录和固定字节，不修改用户音乐文件。

## 实现与边界

- 下载前在受控本地候选中完整校验长度和 BLAKE3；匹配时通过独立临时快照导入缓存，继续正常 journal 发布。
- 本地快照失败不污染网络已确认分块；完整校验后才发布不可变 blob 和完成检查点。清理完成检查点时也清理其遗留 part，过期临时快照只按严格模块命名处理。
- 音频/歌词逻辑路径已经完整匹配新目标时采用原路径，防止重扫产生重复歌曲；旧关联校验仅允许旧版本或完整新目标版本，第三方改动仍被拒绝。
- 本地文件复用不终结 pending、不标记已上传；主机拒绝后本地新文件和待发链保持，主机 ACK 才结束上传。
- `resource_not_found` 独立映射为 `sync.resourceUnavailable`；认证、绑定及内容不匹配仍保持各自分类。
- 无数据库迁移、远端 JSON 或配对变更。额外修正两处既有 Clippy 告警（局部类型声明顺序、相同兜底 match arm 合并），行为不变。

## 验证

- `cargo test --workspace --all-features`：1255 通过、5 个既有 ignored；包括歌曲修改往返、主机拒绝、本地资源缺失、发布幂等和本地快照失败重试。
- `cargo test -p echo-core sync_resource_publication`：14 通过，包含精确目标采用与第三方文件保护。
- 前端 typecheck、lint、format:check、build 通过；40 个测试文件、367 个测试通过（既有 React act 警告仍存在）。
- 最终增量验证：`cargo test -p echo-app embedded_resources -- --nocapture` 2 通过；`cargo test -p echo-sync --lib local_import -- --nocapture` 4 通过（包含崩溃残留清理）；`cargo test -p echo-core sync_resource_publication` 14 通过。往返测试使用生产扫描相同的资源声明／歌曲事务写入，确认保持逻辑路径且重扫不新增 mutation。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo build --workspace --all-features` 最终均通过。独立代码复审无未解决的可操作问题。
- `openspec validate introduce-lan-library-sync --strict` 通过；`git diff --check` 通过。
- `pnpm verify:governance`：场景对账 428/428、场景命令有效性和 manifest 检查通过，随后在既有文件/trait 规模门禁停止，不能宣称完整治理门禁通过。

未完成真实 macOS／Windows 双机安装包复测；未替换用户正在运行的应用、未修改真实资料库或配对。原 change 的其他真机验收和归档任务保持未完成。
