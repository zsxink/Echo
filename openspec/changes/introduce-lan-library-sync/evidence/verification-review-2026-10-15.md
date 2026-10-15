# 局域网同步实现与验证记录（2026-10-15）

结论：核心实现已在本地工作树中，自动验证覆盖了 Rust、网络 crate、桌面端及前端；OpenSpec change 尚未达到完整验收条件，未归档。任务清单仍保留未完成项，不以模拟结果替代设备和发行包验收。工作环境为 macOS arm64。开始时已有大量未提交改动，本次保留并在其上继续工作。

## 本轮实现修正

- 修复主确认时播放统计 payload 被缩减，导致合法设备桶更新遭拒；现在保留完整共享记录，仅按作者授权合并逐设备最大值。
- 修复同步资源测试使用不安全临时目录及不符合库相对路径的 fixture。
- 使文件系统符号链接检测跨平台一致，更新相关用例的拒绝类别断言。
- 修正 LAN 设置角色选择用例，并补充 Effects Tab 键盘交互场景登记，使场景对账达到 428/428。
- 拆分超过治理规模限制的 Rust/TypeScript 源文件；修正拆分后的显式 lint 例外，严格 Clippy 通过。
- 修复新选空资料库无法加入的阻塞：Echo 初始化的 manifest/staging 所有权标记不再被误判为用户内容；仍拒绝额外文件、非空 `media/` 和符号链接。新增 Windows 路径分隔符无关的检查。

## 后续实现进展

- 补上 playlist 并发追加的主确认合并：主按确认顺序保留首个稳定成员身份、重排共享追加顺序，并将重复成员身份写入 typed alias、commit 日志和 SQLite。远端 apply 在同一事务恢复 alias；已删除规范成员不会阻挡新 UUID 重加，历史 alias 及其迟到删除不会指向新成员。
- 新增领域裁决、主端账本、从端回放及删除/重加覆盖；新增 `concurrent_duplicate_playlist_append_persists_alias_in_log_and_snapshot_state` 集成测试。OpenSpec 3.3 已完成复核并勾选。2.4/3.4 仍未勾选，因为歌曲 alias 与其它成员生命周期边界尚未闭环。
- 复核确认 3.2 的高水位、全部现有本地共享写入口事务捕获、删除意图与最终删除事件原子提交均已实现；对应覆盖包括 outbox 清理后 revision 单调、播放统计捕获、PendingDelete 撤销不产生同步墓碑及最终删除只生成一次，现标为完成。
- 资源清单与资源传输确认 4.1/4.2 已实现；4.3 后续也已完成：覆盖外部改动/旧哈希冲突、只读与存储满故障、大小写/Unicode 路径碰撞、符号链接父级/操作目录逃逸及暂存发布恢复。详见 `task-4.3-resource-path-safety-2026-10-15.md`。
- 5.1 HTTP 契约测试现已补齐并勾选：真实 Router 测试覆盖鉴权、实时撤销、提交重试、错误 anchor/日志链拒绝、payload 深度、限流、库隔离及有界 Range；真实 TLS 客户端测试覆盖重定向拒绝和丢 ACK 后续传。当前任务进度为 16/44。
- 后续复核命令 `cargo test --workspace --locked` 通过（Core 560 passed、1 ignored；桌面与 echo-sync 测试通过），`cargo fmt --all --check`、workspace strict Clippy 和 `cargo check --workspace --all-targets --all-features --locked` 通过。

## 自动验证

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | 通过 |
| `cargo check --workspace --all-targets --all-features` | 通过（本机当前工具链） |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 通过 |
| `cargo test --workspace --locked` | 通过，所有单元、集成、架构、协议和文档测试目标均无失败；Core 新增配置、资源发布、重新配对和空库注册覆盖 |
| `cargo build --workspace` | 通过 |
| `cargo llvm-cov -p echo-core --all-features --ignore-filename-regex application/testing --fail-under-lines 90` | 通过：Core 行覆盖率 90.01%，达到 90% 门槛 |
| `cargo deny check` | 通过：advisories、bans、licenses、sources 均通过。锁文件已升级 quick-xml 0.41.0 / plist 1.10.0 / time 0.3.47，workspace MSRV 明确提升到 Rust 1.88 |
| `pnpm --filter @echo/desktop format:check` | 通过 |
| `pnpm --filter @echo/desktop lint` | 通过，0 error、9 warnings |
| `pnpm --filter @echo/desktop typecheck` | 通过 |
| `pnpm --filter @echo/desktop test` | 通过：37 files、341 tests |
| `pnpm --filter @echo/desktop build` | 通过 |
| `pnpm verify:self-test` | 通过 |
| `pnpm verify:governance` | 通过：场景对账 428/428、验证清单 95 项、31/31 注入证明、Core 覆盖率 90.01%、scale/架构/IPC 漂移门禁通过。另修复音效归档后检查脚本仍引用旧 OpenSpec 路径的问题 |
| `openspec validate introduce-lan-library-sync --strict` | 通过 |
| `openspec validate --all --strict` | 通过：20 项 |
| 两台电脑验证准备 | Release run [38739308043](https://github.com/zsxink/Echo/actions/runs/38739308043) 从提交 `41a63a9d5ef268e36d67cffd681e846aae99dee6` 为 0.3.0 构建成功；macOS/Windows/Linux jobs 均通过，tag 留空没有发布 Release。Windows 安装器 `Echo_0.3.0_x64-setup.exe` SHA256 `8fb7e4579e66316e975d1a992d2a04cf78eec6208b33d9546f9394c42ad2ce73` 与 Actions 清单一致，并已下载到本机 `/tmp/echo-windows-0.3.0/nsis/`。Mac DMG `Echo_0.3.0_aarch64.dmg` SHA256 `7b73d235197ee1f8a73b929b3fc5c883bc210b447a47d0b5852ee68d84e003b0`；镜像校验通过且挂载确认含 `Echo.app` 与 Applications 链接。双机步骤见 `two-machine-smoke-test.md`；设备间网络验证仍待执行 |

## 尚未完成的验收

1. 主设备加两从设备真实网络矩阵未在本机执行；包括双向变更、断网编辑、并发、删除撤销/到期、账本分叉、强制退出和恢复。
2. 安装包尚未签名；局域网权限/防火墙、睡眠/网卡切换、磁盘满、占用文件替换及大库性能未完成跨平台验证。
3. 多个 OpenSpec 功能与场景任务仍未完成；需要继续实现并作独立审查后，才可同步 delta specs 和归档 change。

因此，本记录不把本地自动测试通过等同于发布门槛完成；tasks.md 中未满足的任务保持未勾选。
