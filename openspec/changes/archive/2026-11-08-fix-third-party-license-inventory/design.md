# fix-third-party-license-inventory — 设计

## Context

本变更只处理「文档 / 元数据 / 打包资源 / 静态门禁」，与 PR #99 同属许可合规材料层，不触碰 Rust / React 运行时代码。事实核对见 `openspec/changes/archive/2026-10-25-clarify-licensing-and-privacy-notices/design.md` 与 issue #144。

工具链事实：`cargo metadata --format-version 1` 输出 `packages[]` 含 `license` / `repository` / `manifest_path`；`cargo deny check licenses` 当前 `licenses ok`（仅一条 `license-not-encountered` warning）；`pnpm list --prod --depth 0 --json` 在 `apps/desktop` 给出 4 个生产依赖；SPDX 官方全文可从 `raw.githubusercontent.com/spdx/license-list-data/main/text/<id>.txt` 获取（已实测 200）。`THIRD-PARTY-LICENSES.txt` 以 `== SPDX: <id>` 分节，`task-18.1` 按分节标题与最小行数校验。

## Goals / Non-Goals

**Goals:** `LICENSES.md` 与锁文件一致且不再有误导性表述；三平台安装包内可获取完整依赖清单与所有出现许可的全文；清单漂移由静态门禁拦截；发布检查单能防止「仓库已修、发布产物仍旧」复发；Windows 对应来源不可访问的事实被如实记录而非掩盖。

**Non-Goals:** 不产出法律意见、不改 `LICENSE`、不作 GPL 书面要约、不发版、不判定 GPL「完整对应源码」的法律 adequacy、不为每个 crate 复制上游 LICENSE 文件全文。

## Decisions

**D1：以 `cargo metadata` 全图作为保守超集，而非精确生产图。** `cargo tree --edges normal --target <triple>` 需按三个平台分别解析且受 feature 解析影响，输出格式脆弱；全图包含 dev / target-specific 依赖，多列不会造成合规不足，只会冗余。清单与随包 NOTICE 均在头部显式声明「为保守超集，含开发与特定平台依赖」。npm 侧只列 `apps/desktop/package.json` 的 `dependencies`（生产依赖），并注明 devDependencies 不随包分发。

**D2：许可全文固化在仓库，不由生成脚本联网下载。** 法律文本必须精确且可离线复核；生成脚本只产出清单，缺失的 SPDX 全文作为一次性审计输入写入 `THIRD-PARTY-LICENSES.txt`，每节标注官方来源 URL 与 SHA-256。这样 CI 门禁无需网络即可验证分节存在与最小行数。

**D3：生成脚本双模式。** 默认模式写 `LICENSES.md` 与 `THIRD-PARTY-NOTICES.txt`；`--check` 模式在内存中重算并与磁盘比对，不一致退出 1。`task-18.1` 调用 `--check`，使「锁文件变了但清单没重生」在 CI 中直接失败。`cargo metadata` 已是项目既有工具链依赖，不引入新工具。

**D4：扩展 task-18.1，不新增 task id。** 新增门禁编号会牵动 `scripts/verify/manifest.json` 的场景/可追溯对账；18.1 的语义正是「许可与隐私告知契约」，纳入依赖清单覆盖属于同一契约。新增的 `--check` 作为 18.1 的第二条 command 注册。

**D5：随包 NOTICE 不逐 crate 复制 LICENSE 全文。** 447+ 个包的 LICENSE 文件在 registry 中并非都齐备（实测 symphonia 无 LICENSE 文件），逐份复制既易失准又使安装包膨胀。采用「清单（名称/版本/SPDX/来源）+ 合并许可全文（按 SPDX 分节）+ 指向分节的映射」的标准做法；文件头部明确说明各组件版权声明以其上游源码包为准，属律师复核可确认的边界，不伪装成已逐项收录。

**D6：Windows 来源问题只做事实层修复。** 404 的固定 release 链接是历史 pin 记录，保留其 SHA-256 与资产名（供应链门禁 `task-9.7` / `task-1.10` 依赖 `sourceUrl` 格式与校验和），但 NOTICE 增加「当前不可访问」说明、补可核实的上游引用，并把「完整对应源码归档 / 三年书面要约」列为发布前待决项。删除或改写为不存在的替代 URL 属于伪造来源，绝不采用。

**D7：发布检查单补防复发项。** v0.3.0 暴露的是「仓库修复未进入发布产物」，因此检查单新增：本次 tag 的 Release 资产 MUST 含 `THIRD-PARTY-LICENSES.txt` 与 `THIRD-PARTY-NOTICES.txt` 的随包证据、三平台 NOTICE 与当前构建记录一致、Windows 对应源码链接发布时可达。这些是人工核验项，由静态门禁校验其文本存在。

**D8：逐平台 manifest 随 Release 归档。** `vendor/libmpv/linux/manifest.json` 由发布流水线在容器内生成，仓库内不存在，律师复核时无法核对 Linux 的校验和、固定源码提交与许可表。`release.yml` 的既有「Stage platform NOTICE for release」步骤已按平台重命名合并产物，同一步骤追加 `manifest-libmpv-<platform>.json` 并加入发布 glob，`task-crp` 的 R04 扩展断言防止该留档被删除；macOS / Windows 的仓库内 manifest 也由此获得与发布 tag 绑定的快照。

## Risks / Trade-offs

- [保守超集含未分发依赖] → 头部显式声明；不影响合规充分性，律师复核时可据此缩小范围。
- [新增 SPDX 全文可能与上游文本版本漂移] → 每节记录来源 URL 与摘要；`task-18.1` 的最小行数断言防「只留标题」；全文逐字比对仍属律师复核范围（沿用 PR #99 的既有边界）。
- [`--check` 依赖 `cargo metadata`] → CI 已具备 Rust 工具链；脚本在 `cargo` 不可用时明确报错而非静默跳过。
- [Windows 对应源码仍不可获取] → 本变更不能替代完整对应源码或书面要约；已在检查单与 NOTICE 标为发布阻断待决项，需项目方/律师决策。

## Migration Plan

纯文本 / 配置 / 脚本变更，无数据迁移与运行时影响。执行顺序：先补许可全文分节 → 写生成脚本并生成两个清单 → 改打包映射 → 扩展门禁 → 改文档与检查单 → 验证 → 同步 delta spec 并归档。回滚 = revert。
