## 1. 工作流

- [x] 1.1 把 `macos-platform-gate` 的「Cache Rust build artifacts」就地改为 `shared-key: macos-desktop-release` + `save-if: ${{ github.ref == 'refs/heads/main' }}`，步骤名、位置、`if:` 不变。
- [x] 1.2 确认不新增第二个 Rust 缓存步骤（同一 job 两个步骤会把同一份 `target/` 存两遍，配额翻倍）。

## 2. 结构门禁

- [x] 2.1 `task-1.5.mjs`：写者集合加入 `macos-desktop-release`；每个 job 至多一个缓存步骤；release 键独占；Gate 不得恢复 debug 键。
- [x] 2.2 `ci-macos-packaging-structure.mjs`：钉死 Gate 缓存步骤的 `uses` / `shared-key` / `save-if`。
- [x] 2.3 注入证明 `ci-cache/*` 四条：证明每条新断言在违规时确实失败（`ECHO_INJECTION_REQUIRE_ALL=1 node scripts/verify/injection-suite.mjs --only ci-cache`）。

## 3. 配额

- [x] 3.1 核对仓库缓存条目与占用（8,589,248,074 B / 10 GB），按 rust-cache 的键结构判定哪些条目不可达。
- [x] 3.2 清理 3 个环境哈希过期的不可达条目与 1 个实验分支键，释放约 3.47 GB。
- [x] 3.3 核对新条目体积：裁剪后的 release 树 zstd 后 ~196 MiB，加 registry 仍在清理后的余量内。

## 4. 验证及交付

- [x] 4.1 `node scripts/verify/checks/task-1.5.mjs`（含 macOS 打包结构契约）通过。
- [x] 4.2 `ci-cache/*` 注入证明全部通过。
- [x] 4.3 本地实测热重建下界：`cargo build --release --target aarch64-apple-darwin -p echo-app` 在依赖产物就绪时 2m00s。
- [x] 4.4 同步 delta spec 到主规格、验证同步结果后归档本 change，再提 Pull Request。
