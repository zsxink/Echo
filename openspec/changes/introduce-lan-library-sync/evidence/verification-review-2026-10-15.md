# 局域网同步实现与验证记录（2026-10-15）

结论：核心实现已在本地工作树中，自动验证覆盖了 Rust、网络 crate、桌面端及前端；OpenSpec change 尚未达到完整验收条件，未归档。任务清单仍保留未完成项，不以模拟结果替代设备和发行包验收。工作环境为 macOS arm64。开始时已有大量未提交改动，本次保留并在其上继续工作。

## 本轮实现修正

- 修复主确认时播放统计 payload 被缩减，导致合法设备桶更新遭拒；现在保留完整共享记录，仅按作者授权合并逐设备最大值。
- 修复同步资源测试使用不安全临时目录及不符合库相对路径的 fixture。
- 使文件系统符号链接检测跨平台一致，更新相关用例的拒绝类别断言。
- 修正 LAN 设置角色选择用例，并补充 Effects Tab 键盘交互场景登记，使场景对账达到 428/428。
- 拆分超过治理规模限制的 Rust/TypeScript 源文件；修正拆分后的显式 lint 例外，严格 Clippy 通过。
- 修复新选空资料库无法加入的阻塞：Echo 初始化的 manifest/staging 所有权标记不再被误判为用户内容；仍拒绝额外文件、非空 `media/` 和符号链接。新增 Windows 路径分隔符无关的检查。

## 自动验证

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | 通过 |
| `cargo check --workspace --all-targets --all-features` | 通过（本机当前工具链） |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 通过 |
| `cargo test --workspace --locked` | 通过，所有单元、集成、架构、协议和文档测试目标均无失败；Core 545 passed/1 ignored，Desktop 413 passed，Echo Sync 18 passed |
| `cargo build --workspace` | 通过 |
| `cargo llvm-cov -p echo-core --all-features --ignore-filename-regex application/testing --fail-under-lines 90` | **失败**：85.95% 行覆盖率，门槛 90%；`remote_resources.rs` 和 `sync_configuration.rs` 当前未被覆盖 |
| `cargo deny check` | 通过：advisories、bans、licenses、sources 均通过。锁文件已升级 quick-xml 0.41.0 / plist 1.10.0 / time 0.3.47，workspace MSRV 明确提升到 Rust 1.88 |
| `pnpm --filter @echo/desktop format:check` | 通过 |
| `pnpm --filter @echo/desktop lint` | 通过，0 error、9 warnings |
| `pnpm --filter @echo/desktop typecheck` | 通过 |
| `pnpm --filter @echo/desktop test` | 通过：37 files、341 tests |
| `pnpm --filter @echo/desktop build` | 通过 |
| `pnpm verify:self-test` | 通过 |
| `pnpm verify:governance` | **未通过 coverage gate**。场景映射、scale、架构、命令验证、构建纯度及其余先行检查通过；失败原因为上述 Core 90% 覆盖率门槛 |
| `openspec validate introduce-lan-library-sync --strict` | 通过 |
| `openspec validate --all --strict` | 通过：20 项 |
| 两台电脑验证准备 | GitHub Actions run [38677948966](https://github.com/zsxink/Echo/actions/runs/38677948966) 在提交 `1f968dee149a2dbd9bac02862020adcb34ccc32f` 上三平台构建成功；Windows/macOS/Linux artifacts 已上传，Release 发布 job 因 tag 留空而跳过。Windows 0.3.0 安装器 SHA256 `1be7f1222ad34e88474febe41df75a3a9545c25b49bd5203a059e46c4a4948fd` 与 Actions 清单一致；Mac 0.3.0 DMG 已下载并通过布局校验。设备间网络验证仍待执行 |

## 尚未完成的验收

1. Core 行覆盖率需达到 90%，当前为 85.95%；`remote_resources.rs`、`sync_configuration.rs` 仍无覆盖。
2. 主设备加两从设备真实网络矩阵未在本机执行；包括双向变更、断网编辑、并发、删除撤销/到期、账本分叉、强制退出和恢复。
3. 安装包尚未签名；局域网权限/防火墙、睡眠/网卡切换、磁盘满、占用文件替换及大库性能未完成跨平台验证。
4. 仍有多个 OpenSpec 功能与场景任务未完成，需继续实现并作独立审查后，才可同步 delta specs 和归档 change。

因此，本记录不把本地自动测试通过等同于发布门槛完成；tasks.md 中未满足的任务保持未勾选。
