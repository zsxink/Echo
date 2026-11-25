## Why

`macOS platform Gate` 在 PR 上稳定跑 12 分钟左右（历史 8m13s–16m55s），是整条 CI 的关键路径。诊断结论：job 内 10m55s 全在 task 1.9，而 task 1.9 是**从零开始的 release 构建**（`tauri build --target aarch64-apple-darwin`，[profile.release] `lto="thin"` + `codegen-units=1`）。

原因不是构建本身慢，而是**release 产物从未被缓存**：

- `macos-desktop` 缓存键的写者是 main-only 的 `rust-desktop` job，它最后一条 cargo 调用是 `cargo test --workspace --all-targets --all-features` → 只产出 `target/debug/`。
- Gate 恢复这份缓存后，要的是 `target/aarch64-apple-darwin/release/`——目录和 profile 都不同 ⇒ 零复用。
- Gate 自己的缓存步骤是 `save-if: false`，`release.yml` 完全没配 Rust 缓存 ⇒ release 产物从不落盘。

旁证：Linux job 命中同一份缓存时只重编 4 个 workspace crate（4m18s）；macOS native Gate 用同一份缓存跑 debug 形态任务只要 3m26s——只有 release 形态的 job 慢。

本地实测（Apple Silicon，同一条 `cargo build --release --target aarch64-apple-darwin -p echo-app`）：依赖产物已就绪、只有工作区 crate 过期时，**热重建耗时 2m00s**（34 个 crate + ThinLTO 链接）。缓存能把这个 10m43s 压到 2–3 分钟量级。

配额侧：仓库 Actions 缓存当前占用 **8,589,248,074 B ≈ 8.59 GB**（Free 档每仓库 10 GB 上限），只剩 ~1.41 GB。其中 3 个条目的缓存键环境哈希已不是当前环境（`ade52a28` / `1d7dafcc`），rust-cache 的 restore key 含环境哈希 ⇒ **这三个条目不可达**，合计 3.47 GB 纯占位。

## What Changes

- macOS 平台 Gate 的 Rust 缓存步骤从 `macos-desktop`（debug 形态）改为独立的 `macos-desktop-release`（release 形态），`save-if` 改为 main-only。PR 只读取 `main` 写入的条目，永不写入。
- 结构门禁反向锁定：缓存键写者集合、release 键的独占读写、同一 job 至多一个 Rust 缓存步骤、Gate 不得恢复 debug 键。
- 清理 3 个不可达条目 + 1 个实验分支键，释放约 3.47 GB，为 release 条目腾出空间。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `engineering-governance`：共享缓存的写者/读者集合、release 形态缓存的隔离与配额边界。

## Impact

只改 CI 工作流、CI 结构门禁与 OpenSpec，不改产品协议、应用包内容、签名流程或运行时行为。

不改动 `[profile.release]`：`lto` / `codegen-units` / `strip` 保持原值。task 9.7 校验正式产物的 rpath、签名、ABI 与 libmpv 布局，放宽 profile 等于让门禁验证一个和发布物不同的二进制。

风险与边界：

- **首次引入的 PR 仍是 12 分钟**——release 缓存条目要等 `main` 上跑过一次才存在。收益从下一个 PR 起出现。
- **缓存条目按 Cargo.lock 哈希分代**。每次改 `Cargo.lock` 产生一个新条目，旧条目靠 restore key 前缀保持可达并继续提供 fallback；配额因此会缓慢增长，必须靠清理不可达条目控制。
- **命中不等于跳过门禁**。1.9、1.10、9.7 与严格嵌入前端校验每次照跑；缓存只缩短编译时间。
- **前端新鲜度仍然成立**：`tauri-build` 对 `frontendDist` 发出 `cargo:rerun-if-changed`，cargo 对目录递归取最新 mtime ⇒ 每次重建前端都会让 app crate 失效重编，不会出现旧二进制配新 dist。
