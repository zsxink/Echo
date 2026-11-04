# Tasks

## 1. 基线记录与维护脚本 strip 能力

- [x] 1.1 在 macOS 上记录 strip 前基线：`node scripts/release/thin-macos-libmpv.mjs`（dry-run）、`du -sh` 现有 `Echo.app` 与 DMG 体积、`nm -a` 对 `Echo` 主程序与各 vendor dylib 的本地符号计数。验证：基线数值写入 change 交付说明（或 PR 描述），`git status --short` 无 vendor 变化
- [x] 1.2 在 `scripts/release/thin-macos-libmpv.mjs` 的 `prepareThinnedCopy` 中，于 `ensureSignature()` 之前插入 `strip -x` 步骤（保持 `lipo -thin` → `chmod` → `otool -D` → `strip` → `ensureSignature` 顺序），命令组装抽成可单测纯函数。验证：`rg -n 'strip|ensureSignature' scripts/release/thin-macos-libmpv.mjs` 显示 strip 位于重签之前；dry-run 无副作用
- [x] 1.3 在 `scripts/release/thin-macos-libmpv.test.mjs` 增加用例：strip 命令顺序、已 strip 库幂等（no-op）、`--write` 对全部 `manifest.files` 重算 SHA-256 与 `fileArchitectures`。验证：`node scripts/release/thin-macos-libmpv.test.mjs` 全部通过（跨平台，不依赖 macOS 工具链）

## 2. 应用 vendor strip 与根 profile 全局 strip

- [x] 2.1 在 macOS 执行 `node scripts/release/thin-macos-libmpv.mjs --write`，确认 17 个仍带本地符号的 dylib 被剥离、其余已 strip 库字节不变。验证：再次 dry-run 报告零差异；`git diff --stat -- apps/desktop/src-tauri/vendor/libmpv/macos` 只含 dylib 与 `manifest.json`；`codesign --verify --strict` 对全部 dylib 通过
- [x] 2.2 在根 `Cargo.toml` 的 `[profile.release]` 增加 `strip = "symbols"`（保留 `lto` / `codegen-units`）。验证：`rg -n '^strip' Cargo.toml` 命中；`cargo build --release` 成功后 `nm -a` 对主程序无成规模 `t` / `s` 本地符号；dev/test 构建不受影响
- [x] 2.3 复核 strip 未改变依赖闭包：比较 strip 前后 `otool -L`（以 `libmpv.dylib` 为根）的仓库内引用集合。验证：前后集合一致，运行时库解析不缺失

## 3. 五处同步删除未引用的 mbedTLS 库

- [x] 3.1 从 `apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json` 移除三个库的 `files` 与 `fileArchitectures` 条目、`licenses` 中的 mbed TLS 组件；同步删除磁盘上的 `libmbedcrypto.dylib` / `libmbedtls.dylib` / `libmbedx509.dylib`。验证：`manifest.json` 的 `files` / `fileArchitectures` 为 17 条且三者相等；`ls` 确认三个文件不存在；`node scripts/release/thin-macos-libmpv.mjs` dry-run 哈希全部通过
- [x] 3.2 从 `apps/desktop/src-tauri/tauri.conf.json` 的 `bundle.macOS.files` 移除三条 `Frameworks/libmbed*.dylib` 映射（22 → 19 条），确认 `signingIdentity` / `hardenedRuntime` / NOTICE 映射未被误改。验证：`node scripts/verify/checks/task-9.7.mjs` 的 wiring 段通过，不出现 missing mapping
- [x] 3.3 更新许可说明：`vendor/libmpv/macos/NOTICE.md` 删除 mbed TLS 表格行；`vendor/libmpv/THIRD-PARTY-LICENSES.txt` 删除 `:25` 的 `libmbed*.dylib` 清单行，并把 `:958` 的 Apache-2.0 `Covers` 段只去掉「mbed TLS（双许可任选）」措辞、保留 Windows Vulkan Loader 与 macOS libgraphite2。验证：`rg -n 'mbedtls|mbedcrypto|mbedx509|mbed TLS|Mbed-TLS' apps/desktop/src-tauri` 为空；Apache-2.0 段仍覆盖另两个组件；`manifest.licenses` 剩余组件许可类型均被合并许可文本覆盖

## 4. Gate 增加符号纯度与依赖闭包断言（含注入式自证）

- [x] 4.1 在 `scripts/verify/checks/task-9.7.mjs` 增加 macOS-only 的符号纯度断言：对 `manifest.files` 每个文件用 `nm -a` 统计成规模本地符号，超阈值即 `fail` 并打印文件名与计数（允许必要的外部/未定义符号）。验证：`node scripts/verify/checks/task-9.7.mjs` 在 strip 后的树上通过；用含本地符号的 fixture 临时替换任一 dylib 时该检查失败并打印文件名
- [x] 4.2 在 `task-9.7.mjs` 增加依赖闭包与记录一致性断言：以 `libmpv.dylib` 为根递归 `otool -L` 求闭包，断言 `manifest.files` == 闭包 == `bundle.macOS.files` 的 `Frameworks/*` 集合，多余/缺失都 `fail`；不改 `task-1.9.mjs` / `task-1.10.mjs` 的动态遍历清单与 ci.yml 的硬断言字符串。验证：临时在 manifest 加回一条已删库、或在 tauri.conf 漏删一条映射时检查失败；正常树上通过
- [x] 4.3 为 4.1 / 4.2 增加注入式自证单测（构造含本地符号的 `nm -a` 输出样本与多/少条目的 manifest 快照，证明守卫能拒绝违规输入），注册进 `scripts/verify/manifest.json` 的 9.7 任务组；按 `engineering-governance` 同步 `scripts/verify/scenario-commands.mjs` 的注入证明登记（若适用）。验证：`pnpm verify:task -- 9.7` 执行日志包含该测试且通过；故意移除注入条件时测试失败（守卫未被空跑）
- [x] 4.4 更新 `scripts/verify/manifest.json` 中 9.7 的 `title`/`desc`，写明「符号纯度 + 依赖闭包精确匹配」。验证：`pnpm verify:task -- 9.7` 的描述输出与检查项一致

## 5. 集成验证与体积对比

- [x] 5.1 独占运行完整平台 Gate：`pnpm verify:task -- 1.9 1.10 9.7`（1.9 会重新构建并签名 app bundle；不与其它 cargo/node 扫描并发）。验证：三项全部通过，`codesign --verify --deep --strict` 通过，报告生成
- [x] 5.2 校验收尾口径：精确匹配 `rg -n 'mbedtls|mbedcrypto|mbedx509|mbed TLS|Mbed-TLS'` 在活跃代码/配置/许可文件中为空（显式排除 `openspec/changes/archive/**` 与 `embedded` 假阳性）；`nm -a` 对 bundle 内 `Echo` 与 17 个 dylib 无成规模 `t` / `s` 符号。验证：两条命令输出符合预期，归档证据保持不变
- [x] 5.3 记录 strip 前后 `Echo.app` 与 DMG 的实测体积对比（区分未压缩与压缩后收益），写入 change 交付说明或 PR 描述。验证：对比数值与 1.1 基线可对照，DMG 收益如实标注小于未压缩数值
- [x] 5.4 运行 Rust 质量门与 OpenSpec 校验：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`openspec validate strip-symbols-and-drop-mbedtls --strict`。验证：全部通过

## 6. 规格同步与归档

- [x] 6.1 把 delta spec 同步到主规格 `openspec/specs/platform-libmpv-supply-chain/spec.md`，并核对规格、追溯记录、场景清单三方一致（按 `engineering-governance` 要求）。验证：同步后 `openspec validate ... --strict` 通过，追溯/场景对账无漂移
- [x] 6.2 勾选本 change 全部任务、归档 change，然后在独立分支创建关联 issue #137 的 PR。验证：`openspec archive strip-symbols-and-drop-mbedtls` 成功；PR 标题/正文关联 #137，分支非 `main`，归档先于 PR 创建
