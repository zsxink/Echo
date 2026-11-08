# fix-third-party-license-inventory

## Why

对 issue #66 的工程交付（PR #99）做发布前合规事实复核时发现：许可定性统一与 libmpv 逐平台纠错在仓库层面已成立，但「随安装包分发的第三方许可材料」只覆盖平台播放库组件，未覆盖 Rust / npm 依赖；`LICENSES.md` 仍是旧的精简快照，且现有门禁无法发现清单漂移。具体事实（issue #144）：

- `LICENSES.md` Rust 表 43 条，而 `cargo metadata` 全图 594 个非 workspace 包、`cargo tree --edges normal --target aarch64-apple-darwin` 解析出 447 个去重包；npm 表只列 react / react-dom，漏了 `@material/material-color-utilities`（Apache-2.0）与 `lucide-react`（ISC）。文件自称 "Full lockfile graph … superset"、"All third-party crates below are permissive-licensed"，而生产图含 MPL-2.0、Unicode-3.0、CC0-1.0、Unlicense、BSD-2-Clause、0BSD、BSL-1.0、MIT-0、`Apache-2.0 WITH LLVM-exception`。
- `vendor/libmpv/THIRD-PARTY-LICENSES.txt` 的九种许可全文与三平台映射正确，但只映射平台播放库；MIT / ISC / BSD-3 段落是无具体权利人的模板；`LICENSES.md` 不随安装包分发。
- `task-1.7` 只校验 `LICENSES.md` 存在非空；`task-18.1` 只校验 libmpv manifest 许可族，两者都不校验依赖清单完整性。
- Windows NOTICE / manifest / BUILD_INFO 指向的 `shinchiro/mpv-winbuild-cmake/releases/tag/20260814` 与 asset 链接当前均 404，`gh api releases/tags/20260814` 亦为 404；仓库内无 GPL-2.0 §3(b) 书面要约，`docs/media-rights-and-privacy.md` 检查单第 3 项无法通过。
- 最新 Release `v0.3.0`（2026-10-21）早于 PR #99（2026-10-25），资产无 `THIRD-PARTY-LICENSES.txt`；其 `NOTICE-libmpv-linux.md` 仍把 FFmpeg 记为 LGPL-2.1-or-later，Windows NOTICE 无 Licenses / 对应源码章节。

## What Changes

- 新增 `scripts/release/generate-third-party-notices.mjs`：以 `cargo metadata` 全图（保守超集）与 `apps/desktop/package.json` 的 production dependencies 为唯一事实来源，重建完整 `LICENSES.md`，并生成随安装包分发的 `vendor/libmpv/THIRD-PARTY-NOTICES.txt`（依赖名 / 版本 / SPDX 表达式 / 来源 / 许可全文分节指引）。脚本提供 `--check` 模式，供门禁验证产物与锁文件不漂移。
- 按 SPDX `license-list-data` 官方文本，为 `THIRD-PARTY-LICENSES.txt` 补齐生产图出现而当前缺失的许可全文分节（MPL-2.0、Unicode-3.0、CC0-1.0、Unlicense、BSD-2-Clause、0BSD、BSL-1.0、MIT-0、LLVM-exception 等），新增分节带来源 URL 与校验摘要注记。
- 扩展 `scripts/verify/checks/task-18.1.mjs`：SPDX 分节映射覆盖依赖图全部许可族、校验 `LICENSES.md` / `THIRD-PARTY-NOTICES.txt` 与生成脚本一致、校验三平台打包映射带入随包依赖清单。
- 三平台打包配置（`tauri.conf.json`、`tauri.windows.conf.json`）带入 `THIRD-PARTY-NOTICES.txt`；修正双语 README 对清单维护方式的错误表述；`docs/media-rights-and-privacy.md` 发布前检查单补「随包清单核验」「发布资产核验」「对应源码链接可达性核验」三项，防止 v0.3.0 式复发。
- `.github/workflows/release.yml` 把逐平台 `manifest.json` 以平台限定名一并 stage/publish（Linux 的 manifest 只在发布期生成、仓库内不可核验），并由 `task-crp` 断言防回归。
- Windows `NOTICE.md` 如实记录固定来源链接当前不可访问，补可核实的上游引用（mpv commit / FFmpeg / shinchiro 构建仓库），并显式标注「完整对应源码归档或 GPL-2.0 §3(b) 三年书面要约」为发布前待决项，不擅自替项目方作出法律承诺。

## Capabilities

### Modified Capabilities

- `licensing-privacy-notices`：新增「第三方依赖清单完整性」「随包许可材料覆盖」「发布前发行材料核验」需求，把许可声明一致性的范围从 Echo 自身与平台播放库扩展到 Cargo / npm 依赖。

## Impact

- 新增：`scripts/release/generate-third-party-notices.mjs`、`apps/desktop/src-tauri/vendor/libmpv/THIRD-PARTY-NOTICES.txt`（生成物）。
- 重写：`LICENSES.md`；扩展 `apps/desktop/src-tauri/vendor/libmpv/THIRD-PARTY-LICENSES.txt`。
- 修改：`scripts/verify/checks/task-18.1.mjs`、`scripts/verify/checks/task-crp.mjs`、`scripts/verify/injection-governance.mjs`（新增注入证明）、`scripts/verify/manifest.json`、`tauri.conf.json`、`tauri.windows.conf.json`、`.github/workflows/release.yml`、`README.md`、`README.en.md`、`docs/media-rights-and-privacy.md`、`vendor/libmpv/windows/{NOTICE.md,BUILD_INFO.md}`、`deny.toml`（清理未匹配的 allow 项）。
- 非目标：不改根 `LICENSE` 条款（法律文件，待律师复核）；不完成 issue #66 验收标准第 4 条；不重新发布 v0.3.0 或处置历史分发；不代表项目方作出三年书面要约；不改 macOS NOTICE 中 #138 的 Gate 状态陈述。
- 无运行时代码、无数据迁移、无接口变化；回滚 = revert。
