# Design

## Context

现状与动机见 `proposal.md`；本变更的行为契约见 `specs/platform-libmpv-supply-chain/spec.md`。以下是已按当前 HEAD（`ae8e884`，#100 已合并）复核、且影响方案选择的约束：

- 根 `Cargo.toml:112-114` 的 `[profile.release]` 只有 `lto = "thin"` + `codegen-units = 1`，无 `strip`；Cargo 只采用工作区根 manifest 的 profile，四个 crate 的 `Cargo.toml` 均无 profile 段。`.cargo/config.toml` 无 rustflags。
- `scripts/release/thin-macos-libmpv.mjs`（#100 引入）已具备完整的「改字节 → 重算 SHA-256 + `fileArchitectures` → 原子替换」机制：`applyVendorState:95-106` 重写 manifest，`--write` 路径 `:313-350` 即使 `plan` 全为 `match`（纯 strip 场景）也会对全部文件重算哈希；`prepareThinnedCopy:186-203` 的顺序是 `lipo -thin` → `chmod` → `otool -D` install-name 检查 → `ensureSignature`（`:168-176`，ad-hoc 重签 + `codesign --verify --strict`）。**仓库内没有任何二进制 strip 调用**，重签/验证流水线已存在，只缺 strip 步骤。
- `manifest.json` 有 20 个库，`files` / `fileArchitectures` 各 20 条，`architectures: ["arm64"]`；三个 mbed 库同时出现在 `files`、`fileArchitectures`、`licenses`（`:131-135`）及 `tauri.conf.json` 的 `bundle.macOS.files`（`:62-64`，共 22 条）、`macos/NOTICE.md:19`、`vendor/libmpv/THIRD-PARTY-LICENSES.txt:25` 与 `:958`（后者是与 Windows Vulkan Loader、macOS libgraphite2 共用的 Apache-2.0 `Covers` 段）。共 **5 处**需同步，而非 issue 所写的 3 处。
- Gate `task-1.9.mjs`、`task-1.10.mjs`、`task-9.7.mjs` 全部以 `Object.keys/entries(manifest.files)` 动态遍历，无硬编码库清单；`task-9.7.mjs:190-195` 逐文件核对 manifest ↔ `bundle.macOS.files`，`:238-245` 核对 vendor 校验和，`:264-266` 做 `codesign --verify --deep --strict`，`:301-321` 做 bundle 架构精确匹配。`task-1.10.mjs:56` 与 `task-9.7.mjs:213` 对 `ci.yml` 中 `pnpm verify:task -- 1.9 1.10` 是字符串硬断言。
- `tauri.conf.json` 无 `frameworks` 字段，实际是 `bundle.macOS.files`；`:48-49` 已是 `signingIdentity: "-"` / `hardenedRuntime: false`，与 ad-hoc 重签配套，不是待补项。
- 精确匹配 `mbedtls|mbedcrypto|mbedx509|mbed TLS|Mbed-TLS` 的活跃命中为 4 文件 15 行；`-i mbed` 会命中全仓几十处 `embedded`，且 `openspec/changes/archive/**/evidence/*.json` 有近百份历史快照。验收口径必须收窄。
- 项目基准：`docs/DESIGN.md` 规定播放能力由桌面适配层管理、不进入共享 Core；`openspec/CODE_STANDARDS.md` 要求单一职责、纯函数可测、不静默失败、单文件一般 ≤500 行；`engineering-governance` 要求门禁有注入式自证用例且规格/追溯/场景三方一致。

## Goals / Non-Goals

**Goals:**

- 以一次可复现、可审计的操作完成主程序与各平台 release 产物、macOS vendor dylib 的本地符号剥离，并让 manifest 哈希与二进制原子一致。
- 从五处供应链/许可记录中彻底移除三个未引用库，且不让 Gate 因清单变化而失效。
- 让 Gate 能拒绝「本地符号回归」「记录与磁盘/依赖闭包漂移」，而不是只验证当前这一次的结果。
- 记录 strip 前后 `Echo.app` 与 DMG 的实测体积，形成 issue #137 的交付证据。

**Non-Goals:**

- 不引入新的 capability 或跨层依赖；不触碰播放、音效链、Core、UI、同步代码。
- 不新增 macOS vendor 构建脚本（仓库内仍无可复现的 macOS 编译流程，strip 属发布前二进制后处理）。
- 不改动 Windows/Linux 的供应链文件，不清理 git 历史与归档 change 的历史快照。
- 不追求 git 历史体积、DMG 压缩后与未压缩体积等同。

## Decisions

### 1. Rust 侧采用全局 strip，而非 macOS-only target 配置

在根 `Cargo.toml` 的 `[profile.release]` 增加 `strip = "symbols"`，一次生效于三平台 release 产物。

- **范围决策**：已与用户确认接受全局行为。崩溃回溯丢失本地符号名是已知、可接受的取舍；本变更不涉及遥测/崩溃上报（`licensing-privacy-notices` 的隐私边界不变），故无需额外告知。
- **替代方案 A（`.cargo/config.toml` 的 `[target.aarch64-apple-darwin] rustflags = ["-C", "strip=symbols"]`）**：能把影响收窄到 macOS，但 rustflags 与 Tauri 打包链路的复用关系需额外验证，且为单个平台增加一层构建配置；收益只是保留另两平台的符号，与 issue 验收不符，拒绝。
- **替代方案 B（不 strip 主程序，只做 vendor）**：收益从 ~6.17 MiB 降到 ~2.6 MiB，不符合 issue 目标，拒绝。
- **为什么是 `strip = "symbols"` 而非 `debuginfo`**：`symbols` 等价于 strip 掉符号表与调试信息（含本地符号），与 vendor 侧 `strip -x` 的目的一致；项目未配 `debug`，单独 `debuginfo` 不足以去掉本地符号。
- **兼容性**：仅影响 release 构建产物，dev/test 构建不变；不改任何公共 API、IPC 契约或数据结构。

### 2. vendor strip 复用现有维护脚本，不新增脚本

在 `thin-macos-libmpv.mjs` 的 `prepareThinnedCopy` 中，于 `ensureSignature()` **之前**插入 `strip -x <临时副本>`，并保持 `lipo -thin`（如有）→ `chmod` → `otool -D` → `strip` → `ensureSignature` 的顺序。

- **顺序是硬约束**：`strip` 会改写 Mach-O 内容并使已有 ad-hoc 签名失效，必须在重签之前执行；若放在重签之后，`codesign --verify --strict` 与 Gate 会失败。`otool -D` 的 install-name 漂移检查保留（strip 不改 install-name，但检查成本为零、能防回归）。
- **幂等与全量重算**：`strip -x` 对已 strip 的库是 no-op（字节不变）；`--write` 路径本来就会对全部 `manifest.files` 重新计算 SHA-256 与 `fileArchitectures`，因此 `libavcodec` 等已 strip 的库哈希不变、其余 17 个更新，无需手工改 20 个哈希。
- **为什么用 `strip -x` 而不是 `-S` / `-u -r`**：`-x` 只去本地符号，保留外部/未定义符号，dylib 的动态链接依赖与 `otool -L` 闭包不变；`-u -r` 会动更多符号类别，可能影响动态加载，风险与收益不匹配。此选择已在副本上验证 `strip -x` + ad-hoc 重签后 `codesign --verify --strict` 通过（issue #137 的仓库外实验）。
- **脚本职责与规范**：脚本当前 362 行，新增 strip 步骤后仍远低于 `CODE_STANDARDS.md` 的 500 行；strip 判定/命令组装抽成可单测的纯函数，`scripts/release/thin-macos-libmpv.test.mjs` 增加用例（跨平台，不依赖 macOS 工具链），真实 strip 的集成行为只在 macOS Gate 中执行。沿用 #100 已建立的「维护脚本 + Gate 复用纯函数」模式，不引入新抽象层。

### 3. Gate 断言落在 9.7，且必须是可注入自证的

在 `scripts/verify/checks/task-9.7.mjs` 的 vendor 架构纯度检查之后、bundle 校验之前，新增两段 macOS-only 断言：

- **符号纯度**：对每个 `manifest.files` 对应文件运行 `nm -a`，统计小写本地符号（`t` / `s` / `b` / `d` 等）数量；超过阈值（成规模，例如 > 0 或一个明确的绝对上限）即 `fail` 并打印文件名与计数。`nm` 对已 strip 的 dylib 仍会列出少量必要外部符号，断言须允许这些，不能写成「零符号」。
- **依赖闭包与记录一致性**：以 `libmpv.dylib` 为根用 `otool -L` 递归求闭包，断言 `manifest.files` 集合 == 闭包集合（排除系统库与 `@rpath`/`@loader_path` 解析后的仓库内文件），并断言 `bundle.macOS.files` 的 `Frameworks/*` 映射与之相等；多余/缺失条目都 `fail`。这样「只删磁盘不改 manifest」「改了 manifest 没改 tauri.conf」「未来重新打包回 mbed」都会被拒绝，而非依赖人工核对。
  - 备选方案是在 Gate 里硬编码「mbed 三库不得存在」。拒绝：清单会随基线变化，硬编码无法防其它未引用库，与现有「按 manifest 动态遍历」的设计相悖。
- **注入式自证**：按 `engineering-governance` 的要求，新增单测用一个临时构造的 fixture（含本地符号的 Mach-O 片段或 `nm -a` 输出样本、多一条/少一条的 manifest 快照）证明断言能失败；「从未拒绝过任何输入」的守卫不算有效。测试注册进 `scripts/verify/manifest.json` 的 9.7 任务组，并同步 `scenario-commands.mjs` 的注入证明登记（若该守卫属于自包含检查脚本）。
- `task-1.9.mjs` / `task-1.10.mjs` 不改清单逻辑；`ci.yml` 的 `pnpm verify:task -- 1.9 1.10` 字符串不得改动（两处硬断言）。

### 4. 删库按「五处 + manifest 内三处」同步，许可文本只删 mbed 措辞

删除顺序建议为：先改配置与文档（manifest 的 `files` / `fileArchitectures` / `licenses`、`tauri.conf.json` 的三条 `Frameworks/*` 映射、`NOTICE.md:19` 的表格行、`THIRD-PARTY-LICENSES.txt:25` 的清单行），再由脚本 `--write` 处理 strip 与哈希重算，最后删除磁盘上的三个 dylib。任何中间状态都会被 `task-9.7` 的 wiring / 校验和断言拒绝，故须在同一逻辑提交内完成。

- `THIRD-PARTY-LICENSES.txt:958` 的 Apache-2.0 `Covers` 行同时覆盖 Windows Vulkan Loader、macOS libgraphite2、mbed TLS；只删除「、mbed TLS（双许可任选）」措辞，保留另外两个组件。该合并许可文本是 `ci-release-pipeline` 要求随 Release 上传的资产，不能整段删除。
- `manifest.licenses` 的 mbed TLS 条目删除后，剩余 12 个组件的许可类型（含 Apache-2.0，因 libgraphite2 仍在）不变，合并许可文本仍覆盖全部类型。
- `manifest.buildProcedureStatus` 与 `NOTICE.md` 的架构说明（arm64-only、`ARCHS=arm64`）保持不变；`assetSha256`、`additionalSources` 是上游源码 pin，与 vendored 二进制构成无关，不变。
- 验收口径：`rg -n 'mbedtls|mbedcrypto|mbedx509|mbed TLS|Mbed-TLS'` 在活跃代码/配置/许可文件中为空；显式排除 `openspec/changes/archive/**` 与 `embedded` 假阳性。归档 change 是不可变历史证据，不回填修改。

### 5. 分层与依赖方向

本变更只作用于 **Presentation / Platform** 层的发布产物与 `scripts/` 维护工具，不涉及 Core 领域规则、Port、IPC/DTO 或数据库。无新增跨层依赖、无新增外部依赖、无接口/迁移影响。适用 `CODE_STANDARDS.md` 的「单一职责、纯函数可测、注释解释约束、错误保留上下文、不在业务路径静默失败」；`design` 与受影响 capability spec 已按 `engineering-governance` 记录代码规范、所属层级、兼容性与可执行验证命令（见 `tasks.md`）。

### 6. 变更原子性与体积证据

二进制替换、manifest 哈希、五处记录、Gate/脚本改动放在同一个逻辑提交中。体积对比用 `du -sh Echo.app` / `ls -l *.dmg` 在 strip 前后各取一次，记录到 change 的交付说明或 PR 描述（不新增长期维护的文档）；DMG 压缩后收益预计小于 6.17 MiB，需在证据中如实区分未压缩与压缩后数值。

## Risks / Trade-offs

- [三平台 release 崩溃回溯丢本地符号名] → 已与用户确认的全局取舍；本变更不涉及崩溃上报/遥测，不影响 `licensing-privacy-notices` 的隐私边界；如未来引入符号化崩溃分析，另行评估 debug 符号归档。
- [`strip -x` 破坏 ad-hoc 签名] → 插入点固定在 `ensureSignature` 之前，写回前用 `codesign --verify --strict` 校验；Gate 再跑一次 `codesign --verify --deep --strict`。
- [`strip -x` 改变动态依赖] → `-x` 只去本地符号；Gate 用 `otool -L` 闭包断言验证前后一致；`task-1.9` 的 FFmpeg 依赖探针与 ABI 检查继续覆盖运行路径。
- [删库遗漏某处记录] → 5 处清单化 + Gate 的 manifest↔闭包↔bundle 三方一致性断言；任一处遗漏即构建失败。
- [误删许可文本导致其它组件许可覆盖缺失] → `:958` 段只删 mbed 措辞，保留 Windows Vulkan Loader 与 libgraphite2；Gate 的许可清单完整性检查（task-1.10）覆盖。
- [`nm -a` 断言过严误伤已 strip 库] → 断言「成规模本地符号」而非「零符号」，阈值与样本由单测固定；允许必要的外部/未定义符号。
- [`strip -x` 在已 strip 库上改变字节导致哈希意外变化] → 实测为 no-op；`--write` 全量重算哈希，dry-run 会先报告差异。
- [归档 change 的历史快照仍含 mbed] → 明确排除在验收与清理范围外，避免 495 行级无关 churn（与既有 traceability 漂移治理一致）。
- [并行运行治理门导致 5 秒墙钟断言 flake] → 完整 Gate `1.9 1.10 9.7` 独占运行，不与其它 cargo/node 扫描并发；裸 `tauri build` 产物未重签会让 9.7 codesign 段失败，须按 `1.9 1.10 9.7` 顺序。

## Migration Plan

1. 在 macOS 上先跑 `node scripts/release/thin-macos-libmpv.mjs`（dry-run）记录 strip 前的体积与哈希基线，测 `du -sh` 的 `Echo.app` 与已有 DMG。
2. 应用脚本改动（插入 strip）与单测 → 运行单测通过 → 在 macOS 执行 `--write`，复核 `git diff --stat` 只包含 17 个 dylib 字节变化与 `manifest.json`。
3. 五处同步删除 mbed 条目（含许可文本的精确措辞修改），删除磁盘上三个 dylib。
4. 根 `Cargo.toml` 增加 `strip = "symbols"`；本地 `cargo build --release`（或由 1.9 触发）产出 strip 后的主程序。
5. 更新 `task-9.7.mjs` 的符号/闭包断言与注入式自证单测，注册进 `scripts/verify/manifest.json` 的 9.7 组。
6. 独占运行 `pnpm verify:task -- 1.9 1.10 9.7`；记录 strip 后 `Echo.app` 与 DMG 体积，形成对比证据。
7. `cargo fmt --check` / `cargo clippy --workspace --all-targets --all-features -- -D warnings`（如本次触及 Rust 构建配置）、`openspec validate ... --strict`；按 `engineering-governance` 核对规格/追溯/场景三方一致。
8. 同步 delta spec 到主规格、勾选全部任务、归档 change，再创建关联 issue #137 的 PR。
9. 回滚：`git revert` 该提交即可同时恢复二进制、manifest、配置与 Gate 改动；无数据结构或用户数据迁移。

## Open Questions

无。strip 的平台范围（全局）已与用户确认，其余决策均不依赖待定信息。
