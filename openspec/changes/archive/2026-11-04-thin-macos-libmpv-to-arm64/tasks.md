# Tasks

## 1. 维护脚本与单元测试

- [x] 1.1 新增 `scripts/release/thin-macos-libmpv.mjs`：导出纯函数（解析 `lipo -archs`、比较实际/声明架构集合、计算新 manifest），CLI 默认 dry-run、`--write` 执行原子裁剪；dry-run 同时校验 `manifest.files` 的 SHA-256 与磁盘文件一致。验证：在 macOS 运行 `node scripts/release/thin-macos-libmpv.mjs`，输出 10 个待裁剪文件与约 8 MB 预期收益，且 `git status --short` 不出现 vendor 变化
- [x] 1.2 新增 `scripts/release/thin-macos-libmpv.test.mjs`，覆盖 thin/fat 两种 `lipo -archs` 输出、精确相等/超集/缺架构三种判定、manifest 哈希与 `fileArchitectures` 重写、dry-run 无副作用。验证：`node scripts/release/thin-macos-libmpv.test.mjs` 全部通过
- [x] 1.3 在 `scripts/verify/manifest.json` 的 9.7 任务组注册 1.2 的单测命令（与 Linux 构建脚本测试同层级）。验证：`pnpm verify:task -- 9.7` 的执行日志包含该测试且通过

## 2. 应用 arm64-only 裁剪并同步记录

- [x] 2.1 在 macOS 运行 `node scripts/release/thin-macos-libmpv.mjs --write`，确认 10 个 fat dylib 变为 thin arm64、`files` 的 SHA-256 与 `fileArchitectures` 已更新。验证：再次 dry-run 报告零差异；`git diff --stat -- apps/desktop/src-tauri/vendor/libmpv/macos` 只包含 10 个 dylib 与 `manifest.json`
- [x] 2.2 更新 `apps/desktop/src-tauri/vendor/libmpv/macos/NOTICE.md`：删除 x86_64 slice / Intel 运行时段落，改为说明播放后端全部为 arm64-only；同步修改 `manifest.json` 的 `buildProcedureStatus`，记录重建须 `ARCHS=arm64` / `ONLY_ACTIVE_ARCH=YES`。验证：`rg -n "x86_64|Intel" apps/desktop/src-tauri/vendor/libmpv/macos/NOTICE.md` 不再出现旧架构声明，manifest 文案与二进制一致
- [x] 2.3 校验瘦身后的 manifest 与二进制一致。验证：`node scripts/release/thin-macos-libmpv.mjs` 报告所有文件架构精确匹配、全部 SHA-256 通过

## 3. Gate 收紧为架构精确匹配

- [x] 3.1 修改 `scripts/verify/checks/task-9.7.mjs`：新增 darwin-only 的 vendor 架构检查（不依赖 app bundle 是否存在），并把 bundle 逐文件循环改为「实际架构集合 == 发布目标 `architectures`，且 == `fileArchitectures[filename] ?? architectures`」；解析逻辑复用 1.1 导出的纯函数。验证：`node scripts/verify/checks/task-9.7.mjs` 在瘦身后的树上通过；临时用 fat 文件替换任一 dylib 时该检查失败并打印期望/实际集合
- [x] 3.2 修改 `scripts/verify/checks/task-1.10.mjs` 的逐文件循环，同样改为「== `architectures` 且 == `fileArchitectures`」的精确相等，保持其报告输出对全部文件成立。验证：`node scripts/verify/checks/task-1.10.mjs` 在正式 bundle 上通过（需先完成 4.1 的构建）
- [x] 3.3 更新 `scripts/verify/manifest.json` 中 9.7 的 `title`/`desc`，写明「vendor 与 bundle 架构精确匹配、拒绝 fat」。验证：`pnpm verify:task -- 9.7` 的描述输出与检查项一致
- [x] 3.4 修正 `scripts/verify/checks/task-1.9.mjs` 的 FFmpeg 依赖断言：原 `@rpath/libavcodec.dylib` 断言实际读取的是 fat 文件的 x86_64 slice，arm64 slice 用 `@loader_path`；改为接受 `@rpath/` 或 `@loader_path/` 的扁平打包内引用并拒绝逃逸路径，判定复用 1.1 导出函数。验证：`node scripts/release/thin-macos-libmpv.test.mjs` 覆盖引用判定，完整 Gate 1.9 通过

## 4. 集成验证

- [x] 4.1 在 macOS 运行完整平台 Gate：`pnpm verify:task -- 1.9 1.10 9.7`（1.9 会重新构建并签名 app bundle）。验证：三项全部通过，`target/aarch64-apple-darwin/release/bundle/macos/echo-libmpv-report.md` 生成
- [x] 4.2 检查正式 bundle 的架构与体积。验证：对 `Echo.app/Contents/Frameworks/*.dylib` 逐个运行 `lipo -archs`，结果全部为 `arm64`；app bundle 体积相比变更前减少约 8 MB
- [x] 4.3 校验 OpenSpec 变更本身。验证：`openspec validate thin-macos-libmpv-to-arm64 --strict` 通过
