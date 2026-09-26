## Context

见 proposal.md - Why。此处只记录解释方案所需的现状与约束。

现状要点（均经实测或源码核实）：

- `tauri.conf.json` 的顶层 `bundle.resources` 是 Tauri 2 的**平台无关**字段。`tauri-utils` 的 `MacConfig` / `LinuxConfig` 各自提供 `files` 子配置，唯独 Windows 没有对应的 `files`，只能依赖顶层 `resources`。
- 该字段的影响面不止打包产物。`tauri-build` 在编译期（`lib.rs` 中读取 `config.bundle.resources`）就无条件把映射的文件拷进 `target/<profile>/`，不判断目标平台。因此 macOS 上 `target/release/` 同样出现了 `libmpv-2.dll`、`vulkan-1.dll`、`NOTICE-libmpv-win.md`、`VulkanRT-LICENSE.txt` —— 这是同一处配置造成的第二个症状。
- Tauri 原生支持平台专属配置文件 `tauri.{macos,windows,linux}.conf.json`，按 RFC 7396 JSON Merge Patch 合并到主配置（`tauri-utils` `config/parse.rs`）。项目当前未使用任何平台专属配置。
- `scripts/verify/checks/task-9.7.mjs` 第 265 行从主 `tauri.conf.json` 读 `bundle.resources` 断言 Windows DLL 映射完整性，且该检查在 CI 的 macOS 与 Windows 平台 Gate 都会执行。

## Goals / Non-Goals

**Goals:**

- 让每个平台的正式产物只携带本平台的播放后端二进制，且该性质由 Gate 在 CI 内断言，而非依赖人工 review 配置。
- 保持 Windows 打包行为与 Gate 断言强度不变（不能因为把配置挪走而降低 Windows 侧覆盖）。
- 配置保持声明式、单一事实来源，不引入按平台改写配置的构建步骤。

**Non-Goals:**

- 不合并三个平台的打包配置为同一种机制（macOS 的 `Frameworks/` 布局与 rpath、Linux 的 `usr/bin/` 布局各有其加载约束，改动会牵连 `bundled_libmpv` 与 `build.rs` 的 rpath 逻辑）。
- 不改动 libmpv 的 vendoring、manifest 校验或 ABI 契约。
- 不处理与本次误打包无关的历史遗留问题。

## Decisions

### 决策 1：把 Windows 的 `resources` 移入 `tauri.windows.conf.json`

**选择**：新增平台专属配置文件承载 Windows 的资源映射，主 `tauri.conf.json` 移除顶层 `resources`。

**理由**：这是 Tauri 提供的、原生的按平台分发机制。合并发生在配置解析层，macOS/Linux 的构建根本不会看到这些条目 —— 既修好了打包产物，也顺带消除了 `target/<profile>/` 被污染的症状，且不需要任何自定义脚本。配置仍是纯声明式的单一事实来源。

**备选与否决理由**：

- *把 macOS 也改用顶层 `resources` 以求统一*：否决。`resources` 一旦写进顶层就是三平台全发，正是当前缺陷的机制本身；Tauri 没有条件表达式可用。统一反而要推翻 macOS 现有 `macOS.files` 布局，连带改动 rpath 与三处 Gate（`task-9.7` / `task-1.9` / `task-1.10`）的大量布局校验，风险与收益不成比例。
- *在构建前用脚本按 `process.platform` 改写配置*：否决。那会让配置文件成为脚本的产物而非事实来源，本地与 CI 可能出现不同结果，违反「配置是单一事实来源」。

### 决策 2：Gate 增加「产物纯净性」断言，而不只改断言的读取源

**选择**：除把 `task-9.7` 的 Windows 资源断言改读 `tauri.windows.conf.json` 外，在 macOS 侧新增一条对正式构建产物的断言：`Contents/Resources/` 不得含 `.dll` 与 Linux `.so` 等非本平台播放后端二进制。

**理由**：只改读取源属于「跟着实现走」—— 配置再写回顶层时 Gate 依然全绿，而那正是本次缺陷的形态。断言产物内容才能让回归在 CI 内可复现地失败。macOS 是唯一有正式产物可检查的平台（Gate 1.9/1.10 的 `BUNDLE` 常量已指向 `target/aarch64-apple-darwin/release/bundle/macos/Echo.app`），故断言落在 macOS 侧；Linux 产物在 CI 内不常驻构建，其纯净性由配置层保证。

**备选与否决理由**：*断言配置文件本身不含 Windows 路径*：否决，那只验证了配置写法，验证不了 Tauri 实际把什么放进了产物 —— 缺陷恰恰发生在「配置写法」与「实际产物」之间。

### 决策 3：约束随能力「首次落库」一并写入，而非落库后再打 `MODIFIED` 补丁

**选择**：本 change 声明 `skip_specs: true`、不产出 spec delta；「每个平台的安装包 MUST NOT 携带其它平台的播放后端二进制」这条约束由 tasks 3.1 直接补进**未归档**的 `introduce-windows-linux-libmpv` 的 `platform-libmpv-supply-chain` capability 内，使该能力在首次进入 `openspec/specs/` 时即带有该约束。

**理由**：`platform-libmpv-supply-chain` 仍是 pending capability（尚未进入 `openspec/specs/`）。对该能力写 `MODIFIED` delta 会被 `openspec validate` 判为 **archive 时拒绝**（已实测：`target spec does not exist; only ADDED requirements are allowed for new specs`）。而两个 change 同时 `MODIFIED` 同一 requirement 则存在归档顺序风险：后归档者会用旧文本覆盖先归档者的结论。既然该能力尚未落库，在其源 change 内就地补入约束是最简单的形态 —— 一次落库、结论唯一、无归档顺序依赖。同时 task 3.4 当前未勾选，其正文正是本 change 要修正的实现，不应先把「顶层 `bundle.resources`」当作已成立的结论勾掉。

**备选与否决理由**：*在本 change 写 `MODIFIED` delta、并在 `introduce-windows-linux-libmpv` 归档后再补一个 delta*：否决。既产生一次注定被拒的归档，又把正确性押在「两个 change 严格按序归档」这一人工约定上。

## Risks / Trade-offs

- **[Windows 产物丢失 DLL]** → 平台专属配置只在 Windows 目标构建时合并，Windows Gate（9.7/9.8）会实机校验资源映射与真实加载；改动后必须跑 Windows Gate 确认，而不能只在 macOS 上验证。
- **[`task-9.7` 在 macOS 上跳过 Windows 分支，配置错误无法本地暴露]** → macOS 侧新增的产物纯净性断言能在 macOS 上捕获回归的一半（资源外溢方向），其余依赖 Windows Gate，属既有分工。
- **[`deny.toml` / 许可证复核依赖产物内实际存在的文件]** → macOS 移除的是 Windows 的 NOTICE，macOS 自身的 `Resources/third-party/libmpv/NOTICE.md` 不受影响；`ci-release-pipeline` 要求的三份 NOTICE 来自 `release.yml` 上传 vendor 目录而非打包产物，不受影响。
- **[JSON Merge Patch 对 `resources` 映射的合并语义]** → 本次是「主配置中该字段完全不存在、由平台文件新增」，不涉及跨文件合并同一对象的键覆盖，属该机制最简单的情形；无需依赖覆盖行为。
- **[macOS dmg 体积变化使既有 Gate 的体积类断言失败]** → 现有 Gate 断言的是 Frameworks 内文件存在性与校验和，未对总体积设上限；若有则需随本次更新。

## Migration Plan

纯构建配置变更，无运行时行为、数据库或公共接口变更。回滚方式为还原 `tauri.conf.json` 的顶层 `resources` 并删除 `tauri.windows.conf.json`（同时 Gate 断言需一并回退），无需数据迁移。发布产物需重新构建，旧版本安装包不受影响。
