## Context

`macOS platform Gate` 在 PR 上稳定 12 分钟，其中 10m55s 在 task 1.9 的 release 构建上。诊断（PR #179，run 45590876151）确认：

- `macos-desktop` 缓存键的写者是 main-only 的 `rust-desktop` job（`cargo test --workspace --all-targets --all-features`），内容是 `target/debug/`。
- Gate 需要 `target/aarch64-apple-darwin/release/`，profile 与目录都不同 ⇒ 恢复的 766 MB 缓存对 release 构建零复用（CI 日志里 `Cache hit ... full match: true`，构建仍从零开始）。
- release 产物从未被任何 job 保存：Gate `save-if: false`，`release.yml` 无 Rust 缓存。

`Swatinem/rust-cache@v2` 的键结构（源码 `src/config.ts`）：

```
cacheKey   = v0-rust[-<shared-key>]-<os.type>-<os.arch>-<envHash8>-<lockHash8>
restoreKey = v0-rust[-<shared-key>]-<os.type>-<os.arch>-<envHash8>
```

`shared-key` 一旦设置，`key` 与 `GITHUB_JOB` 都不再进键 ⇒ 同一 `shared-key` 的 job 共享条目。

## Goals / Non-Goals

**Goals**

- 让 release 形态的编译产物可被复用，把 Gate 从 ~12 分钟压到 ~5 分钟量级。
- 保持门禁语义不变：缓存只影响编译时间，不影响检查是否执行。
- 让缓存配额有界，不再依赖平台自动淘汰。

**Non-Goals**

- 不改 `[profile.release]`（`lto` / `codegen-units` / `strip`）。
- 不改发布流程（`release.yml` 的 DMG 校验链）。
- 不给 `release.yml` 加缓存（本次只解决 PR 关键路径；发布流水线的缓存是独立议题）。

## Decisions

### D1：就地改写 Gate 的缓存步骤，不新增第二个步骤

把 `macos-platform-gate` 的「Cache Rust build artifacts」从 `shared-key: macos-desktop` / `save-if: false` 改为 `shared-key: macos-desktop-release` / `save-if: ${{ github.ref == 'refs/heads/main' }}`。步骤名、位置、`if:` 全部不变 ⇒ `ci-macos-packaging-structure.mjs` 里被钉住的步骤顺序与条件仍然成立。

**为什么不加第二个缓存步骤**：`Swatinem/rust-cache` 在 post step 里打包整个 `target/` 目录。同一个 job 里两个步骤 ⇒ 同一份字节被存两遍，配额翻倍；且 runner 的 post step 是 LIFO，后跑的 prune 会污染先跑的归档内容。已有的 `macos-desktop`（debug）缓存对 Gate 本来就是无用负担（profile 与目录都不匹配），直接换掉即可。

**为什么 Gate 不需要 debug 缓存**：Gate 只跑 1.9 / 1.10 / 9.7，全是 release 形态；debug 产物对它没有任何用途。registry 部分由新键自己的归档覆盖。

### D2：release 键由 Gate 自己写（main-only）

Gate 在 `main` 上无条件执行（`Classify macOS packaging paths` 的 `if` 只在 `pull_request` 生效），所以 `main` 上必然跑完 release 构建、必然有东西可存。PR 上的同一个 job 只恢复、不保存。

GitHub 的作用域规则：PR run 可以恢复 default branch 上创建的缓存；PR 自己创建的缓存只对 `refs/pull/<n>/merge` 可见。因此「main 写、PR 读」是唯一能让 PR 受益的组合，也天然满足「缓存写入 MUST 只发生在 main」。

### D3：release 键的构建形态固定

键的形状 = release profile + `aarch64-apple-darwin` target，由 task 1.9 的 `tauri build --bundles app --target aarch64-apple-darwin` 决定。这与 debug 形态的 `macos-desktop` 是两棵互不相干的树，不能互相顶替。结构门禁因此新增四条断言（见 D5）。

### D4：前端新鲜度不因缓存而失效

`tauri-build` 的 codegen 对 `build.frontendDist` 发出 `cargo:rerun-if-changed=<dist>`（`tauri-build-2.6.3/src/codegen/context.rs:91`），cargo 对目录递归取最新 mtime。CI 每次都重新 `pnpm --dir apps/desktop build`，dist 的 mtime 必然新于缓存里记录的指纹 ⇒ app crate 每次都失效重编 ⇒ 嵌入的永远是本轮 dist。

同时 rust-cache 在保存前会裁剪 profile 目录：只留 `build` / `.fingerprint` / `deps`，且默认只保留**依赖 crate** 的产物（workspace 成员被剔除）。所以缓存里既没有最终二进制也没有 `.app` bundle，task 1.9 的 `tauri build` 照常产出本轮的 bundle。

代价是 app crate 与 ThinLTO 链接每次都重做——这正是本地实测的 2m00s 热重建时间，也是缓存能拿到的下限。

### D5：结构门禁反向锁定

`scripts/verify/checks/task-1.5.mjs` 新增：

1. 每个 job 至多一个「Cache Rust build artifacts」步骤（原先的 `.find()` 会直接漏掉第二个步骤）。
2. 写者集合恰好是 `[governance-static, linux-workspace, macos-desktop, macos-desktop-release, windows-desktop]`。
3. `macos-desktop-release` 只被 `macos-platform-gate` 使用。
4. `macos-platform-gate` 不得恢复 debug 键 `macos-desktop`。

`scripts/verify/ci-macos-packaging-structure.mjs` 把 Gate 缓存步骤的 `uses` / `shared-key` / `save-if` 钉死。

四条断言各有对应的注入证明（`ci-cache/*`），证明门禁在违规时确实失败。

### D6：配额靠清理不可达条目控制

rust-cache 的 restore key 含环境哈希 ⇒ 环境哈希过期的条目**不可达**，只是占位。当前仓库有 3 个这样的条目（`ade52a28` × 2、`1d7dafcc` × 1），合计 3.47 GB；另有 1 个实验分支的键（`experiment-echo-165-45135283044`）不对应任何现有 workflow。

平台超配额时按全仓库最久未访问优先淘汰，可能连带删掉仍被读取的条目（正是 2026-11-17 计划文档里记录的三平台 Rust job 全部 miss 的成因）。所以配额管理 MUST 是主动清理，不是等平台淘汰。

保留的是「环境哈希仍当前、只是 lock 哈希旧」的条目：它们通过 restore key 前缀仍可达，是 Cargo.lock 变动时的 fallback。

## Risks / Trade-offs

| 风险 | 处置 |
| --- | --- |
| 引入该变更的 PR 本身仍是 12 分钟（release 缓存还不存在） | 收益从下一个 PR 起出现；写进 proposal 避免被读成「缓存没生效」 |
| Cargo.lock 每次变动产生新条目，配额缓慢增长 | 键的形态固定，靠清理不可达条目控制；每次 PR 的缓存体积实测 ~196 MiB（裁剪后的 release 树，zstd） |
| 命中缓存后二进制陈旧 | `rerun-if-changed` 使 app crate 每次重编；缓存里也不含最终二进制与 bundle |
| 误以为缓存能替代门禁 | 1.9 / 1.10 / 9.7 与嵌入前端校验每次照跑，结构门禁钉住 |
| 未来有人给 Gate 加第二个缓存步骤 | 断言 1 直接失败，并给出配额翻倍的原因 |

## Migration Plan

1. 合并后 `main` 上跑一次 CI → Gate 在 `main` 写入 `macos-desktop-release` 条目。
2. 之后的 PR 恢复该条目，编译时间从 ~11 分钟降到 ~2–3 分钟。
3. 每次 Cargo.lock 变动产生新条目；观察配额，必要时清理不可达条目。

回滚：把 Gate 的缓存步骤改回 `shared-key: macos-desktop` / `save-if: false` 即可，无代码侧残留。

## Open Questions

无。发布流水线（`release.yml`）是否也缓存 release 产物，留作独立议题。
