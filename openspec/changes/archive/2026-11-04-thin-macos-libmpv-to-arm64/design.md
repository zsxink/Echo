# Design

## Context

- 现状见 `proposal.md`。`vendor/libmpv/macos/manifest.json` 已有 `architectures: ["arm64"]` 与逐文件 `fileArchitectures`，但 10 个文件实际为 `arm64 + x86_64`。
- macOS vendor 二进制由仓库外构建后提交，仓库内没有 macOS 的 vendor 构建脚本（`scripts/release/` 只有 Linux 构建脚本），因此 issue #100 所说「构建步骤强制单架构」在仓库内只能落实为：**维护脚本负责可复现裁剪与 manifest 同步，Gate 负责阻止回归**。
- `task-1.10` 已在 bundle 侧读取 `fileArchitectures`，但只做「包含」断言；`task-9.7` 同样只检查「包含声明架构」。两者都无法拒绝 fat。
- 已实测：`lipo -thin arm64` 后 dylib 的 ad-hoc 签名仍通过 `codesign --verify --strict`，`otool -D` install-name 不变。打包阶段本来就会重新签名。

## Goals / Non-Goals

**Goals:**

- 以一次可复现、可审计的操作把 10 个 fat dylib 收敛为 arm64-only，并让 manifest 的哈希/架构记录与二进制原子一致。
- Gate 用精确集合比较拒绝任何多余架构，覆盖 vendor 目录与正式 app bundle。
- 未来重新 vendoring 时，同一条命令可以复核并修正架构构成。

**Non-Goals:**

- 不改变 Windows/Linux 供应链与 Gate。
- 不引入 universal 构建、Intel 支持或 Rosetta 路径。
- 不压缩 git 历史中已有的 fat blob（本次只减少工作树与发布产物体积）。
- 不触碰播放、音效链或 Core 代码。

## Decisions

### 1. Gate 采用架构集合精确相等，而不是字符串包含

在 `task-9.7` 中新增一段 macOS-only 的 vendor 架构检查，放在 manifest 结构校验之后、不依赖 app bundle 是否存在；在 `task-9.7` 的 bundle 校验与 `task-1.10` 的逐文件循环中，把「包含声明架构」改为两条精确断言：实际架构集合 == 发布目标 `architectures`，且 == `fileArchitectures[filename]`（缺省时回退到 `architectures`）。

- **为什么不能只信 `fileArchitectures`**：它是「上次构建实际产出了什么」的记录。只校验实际 == `fileArchitectures` 时，一次 fat 重建只要同步更新该字段就能通过；发布目标 `architectures: ["arm64"]` 才是必须满足的契约。
- **实现方式**：统一用 `lipo -archs <file>`（输出 `arm64` 或 `x86_64 arm64` 的空格分隔列表）解析为集合，再做排序后比较。相比解析 `lipo -info` 的自然语言输出（thin/fat 两种句式），`-archs` 更稳定且天然适合集合比较。
- **失败信息**：明确指出文件、期望集合与实际集合，便于定位重建时混入的架构。
- **替代方案**：继续用 `-info` + `includes` —— 已证明会放过 fat，拒绝。
- **1.9 的依赖断言需同步修正**：1.9 仍不承担架构断言，但它检查 bundled `libmpv.dylib` 是否通过打包内路径解析 FFmpeg。原断言只接受 `@rpath/libavcodec.dylib`，实际能通过是因为 `otool -L` 对 fat 文件默认读取 x86_64 slice（该 slice 用 `@rpath`）；arm64 slice 用 `@loader_path/libavcodec.dylib`。移除 x86_64 后断言会误报。改为接受 `@rpath/` 或 `@loader_path/` 的扁平打包内引用，并拒绝绝对路径、子目录和父级穿越；判定逻辑放入 1.1 的共享模块并单测覆盖。

### 2. 新增 `scripts/release/thin-macos-libmpv.mjs` 维护脚本

默认 dry-run：读取 manifest，逐个文件比对实际架构与声明架构，列出差异、可节省体积；`--write` 时执行裁剪并回写 manifest：

- 实际架构是声明架构的严格超集 → 对每个目标架构执行 `lipo -thin <arch> -output <tmp>`，原子替换原文件，校验 `codesign --verify --strict` 与 `otool -D` install-name；签名若失效则以 ad-hoc 重签（vendor 签名不是供应链锚点，manifest SHA-256 才是），然后重新计算该文件 SHA-256。
- 实际集合缺少声明架构 → 失败，要求重新获取/重建基线，绝不“补架构”。
- 实际集合与声明一致 → 无操作。
- 全部完成后把 `fileArchitectures` 重写为实际集合、`files` 中的 SHA-256 更新为最终字节，避免手改 20 个哈希出错。
- 仅 darwin 可运行；非 darwin 明确失败而不是静默跳过（`lipo` 不存在）。

dry-run 同时逐个校验 `manifest.files` 的 SHA-256 与磁盘内容，这样「架构已对但哈希过期」也会在写回前暴露。脚本拆出纯函数（解析 `lipo -archs` 输出、计算新 manifest、生成变更摘要），Gate 检查复用同一解析/比较函数；`scripts/release/thin-macos-libmpv.test.mjs` 做跨平台单测，真实裁剪的集成行为只在 macOS Gate 中执行。测试注册进 `scripts/verify/manifest.json` 的 9.7 任务组，与 Linux 构建脚本测试同一层级。

- **替代方案**：只做一次性手工 `lipo` —— 无法防止未来重建再次混入，也无法保证哈希/架构记录同步，拒绝。
- **替代方案**：用纯 JS 解析 Mach-O header —— 代码量与出错面更大；Gate 本身已要求 macOS 工具链，没有收益。

### 3. manifest 不升 schema，`architectures` 作为目标契约

保留 `schemaVersion: 1` 与现有字段；`architectures` 继续表示发布目标架构（arm64），`fileArchitectures` 表示每个文件实际应含的架构。本次瘦身后两者对所有文件一致。`buildProcedureStatus` 文案改为记录「arm64-only；重建须 `ARCHS=arm64` / `ONLY_ACTIVE_ARCH=YES`」，不新增字段，避免影响现有读取方。

### 4. NOTICE 与二进制保持一致

删除 NOTICE 中「x86_64 slice 保留旧 mpv/FFmpeg 运行时、Intel 未通过音频 Gate」的段落，改为说明播放后端全部为 arm64-only、与当前发布目标一致，并保留既有许可与对应源码说明（许可清单不变）。这是 `licensing-privacy-notices` 对「许可记录与构建配置一致」的直接要求。

### 5. 变更原子性

二进制替换、manifest 哈希/架构、NOTICE 文案、Gate/脚本改动放在同一个逻辑提交中。任何只提交其中一部分的状态都会被 9.7 的 vendor 校验或 1.10 的 bundle 校验拒绝。

## Risks / Trade-offs

- [Intel Mac 无法再运行 vendored 播放后端] → 发布目标本就不含 Intel；在 proposal/README 变更说明中显式标注 **BREAKING**，需要 Intel 调试时从 git 历史取回旧基线。
- [瘦身不缩小 git 历史] → 已知；本次目标是工作树与安装包体积，历史压缩另行处理。
- [未来仓库外重建再次产出 fat] → Gate 的精确相等断言是唯一可靠兜底；构建说明与维护脚本降低复发概率。
- [`lipo -thin` 破坏签名] → 已实测当前 dylib 不破坏；脚本在写回前校验，失效时 ad-hoc 重签再计算哈希。
- [脚本与 Gate 使用不同判据导致漂移] → 两者复用同一纯函数解析架构集合，单测固定 `lipo -archs` 的 thin/fat 两种输出格式。

## Migration Plan

1. 在 macOS 上运行维护脚本 dry-run，确认 10 个文件与约 8 MB 的预期收益。
2. `--write` 应用裁剪并生成新 manifest；人工复核 `git diff --stat` 与 manifest 差异。
3. 更新 NOTICE 与构建说明。
4. 运行 `node scripts/verify/checks/task-9.7.mjs`（vendor 部分无需 bundle 即可验证），再运行完整的 `pnpm verify:task -- 1.9 1.10 9.7` 验证正式 app bundle。
5. 回滚：`git revert` 该提交即可同时恢复二进制与 manifest；无数据结构或用户数据迁移。
