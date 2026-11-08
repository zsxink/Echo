# Tasks

## 1. 许可全文分节补齐

- [x] 1.1 从 SPDX `license-list-data` 官方文本取得生产依赖图出现但 `THIRD-PARTY-LICENSES.txt` 缺失的许可全文（MPL-2.0、Unicode-3.0、CC0-1.0、Unlicense、BSD-2-Clause、0BSD、BSL-1.0、MIT-0、LLVM-exception），以 `== SPDX: <id>` 分节追加，每节标注来源 URL 与 SHA-256
- [x] 1.2 验证：`rg '^== SPDX:'` 列出 18 个分节；`task-18.1` 的分节最小行数断言覆盖全部新增节

## 2. 生成脚本与清单产物

- [x] 2.1 新增 `scripts/release/generate-third-party-notices.mjs`：以 `cargo metadata` 全图 + `apps/desktop/package.json` production dependencies / `pnpm-lock.yaml` 为输入，生成完整 `LICENSES.md` 与 `vendor/libmpv/THIRD-PARTY-NOTICES.txt`；头部声明保守超集与版权声明边界；workspace 成员表由 metadata 生成，保留 `LicenseRef-Echo-Personal-Study-1.0` 与逐平台 libmpv 说明
- [x] 2.2 实现 `--check` 模式：重算并与磁盘产物比对，不一致退出 1；`cargo` 不可用或新增 npm 生产依赖未登记时明确报错
- [x] 2.3 运行生成脚本，提交 `LICENSES.md`（594 个 Rust 包 + 4 个 npm 生产依赖）与 `THIRD-PARTY-NOTICES.txt`；验证表述不再自称 permissive-only、不再声称由 cargo deny 维护

## 3. 随包分发与门禁

- [x] 3.1 `tauri.conf.json`（macOS files、linux deb/appimage files）与 `tauri.windows.conf.json`（resources）带入 `THIRD-PARTY-NOTICES.txt`
- [x] 3.2 扩展 `task-18.1.mjs`：SPDX 别名映射覆盖依赖图全部许可族（复合 `OR` / `AND` / `WITH` / 斜杠表达式拆词）、新增分节最小行数、调用生成脚本 `--check`、校验三平台随包清单映射、校验 `LICENSES.md` 与 NOTICE 的关键表述、校验 Windows NOTICE 的失效链接记录与发布前待决项
- [x] 3.3 `scripts/verify/manifest.json` 的 18.1 增加 `generate-third-party-notices.mjs --check` 命令并更新标题；`deny.toml` 清理未匹配的 allow 项（`cargo deny check licenses` 由 1 warning 变为干净通过）；`injection-governance.mjs` 新增 `licensing/stale-dependency-inventory` 注入失败证明
- [x] 3.4 `.github/workflows/release.yml` 把逐平台 `manifest.json` 以 `manifest-libmpv-<platform>.json` stage/publish（Linux manifest 为发布期生成物，仓库内不可核验），`task-crp.mjs` 的 R04 增加防回归断言

## 4. 文档事实层修复

- [x] 4.1 `docs/media-rights-and-privacy.md` 发布前检查单扩为 6 项：新增随包依赖清单核验、发布资产核验（含逐平台 manifest）、对应源码链接可达性核验与法律复核待决项，保留律师复核门槛与未来功能告知义务
- [x] 4.2 `README.md` / `README.en.md`：修正清单维护方式表述，补随包 `THIRD-PARTY-NOTICES.txt` 说明
- [x] 4.3 `vendor/libmpv/windows/{NOTICE.md,BUILD_INFO.md}`：如实记录固定来源链接当前 404，补可核实上游引用与「完整对应源码 / 三年书面要约」发布前待决项，保留 `## Licenses` 与 `Corresponding source` 章节

## 5. 验证与归档

- [x] 5.1 实跑通过：`node scripts/verify/checks/task-18.1.mjs`、`pnpm verify:task -- 18.1`、`node scripts/release/generate-third-party-notices.mjs --check`、`cargo deny check licenses`、`node scripts/verify/checks/task-1.7.mjs`、`task-9.7.mjs`、`task-17.1.mjs`、`task-crp.mjs`、`node scripts/verify/reconcile-scenarios.mjs`（577=577=577）、`node scripts/verify/ci-governance.mjs`（injection-suite 45/45）、`openspec validate --all --strict`（25 passed / 0 failed）
- [x] 5.2 注入失败证明：在 `LICENSES.md` 注入一行后 `--check` 与 18.1 均以 `STALE` 失败（exit 1），还原后恢复通过；该场景已登记为 `licensing/stale-dependency-inventory` 并由 injection-suite 实际执行
- [x] 5.3 同步 delta spec 到 `openspec/specs/licensing-privacy-notices/spec.md`、归档 change、提 PR 关联 issue #144
