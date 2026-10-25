## 1. 许可证定性统一（issue #66 待办一）

- [x] 1.1 重写根 `LICENSE` 为「Echo 个人学习使用许可 1.0」：有限授权结构消除「授予再分发/禁止再分发」矛盾；厘清授权主体（Echo 项目作者，GitHub: zsxink）、再分发例外、第三方组件权利保留、适用法（中华人民共和国法律）与争议管辖；保留既有使用限制意图与 Change Date（2099-12-31 → GPL-2.0-or-later）机制
- [x] 1.2 `Cargo.toml` workspace `license` 改为 `LicenseRef-Echo-Personal-Study-1.0` 并更新注释；`deny.toml` 注释同步（不改变 private 豁免行为）
- [x] 1.3 `LICENSES.md`：许可定性改写、workspace 成员行同步、libmpv 说明改为逐平台准确表述（macOS LGPL/ISC、Windows GPL-2.0-or-later + Apache-2.0、Linux GPL-2.0-or-later 自建）

## 2. libmpv 许可纠错与打包义务（issue #66 待办二）

- [x] 2.1 Windows `vendor/libmpv/windows/manifest.json`：libmpv 与 FFmpeg 许可 LGPL-2.1-or-later → GPL-2.0-or-later（shinchiro `--enable-gpl --enable-libx264 --enable-libx265` 核实）；`NOTICE.md` 补 Licenses 与对应源码章节
- [x] 2.2 `scripts/release/build-linux-libmpv.mjs`：LICENSES 表 FFmpeg 条目 → GPL-2.0-or-later（OPTIONS 保留 `--enable-gpl`），NOTICE 模板 Licenses 章节同步
- [x] 2.3 macOS `vendor/libmpv/macos/NOTICE.md` 补对应源码与许可文本指引
- [x] 2.4 新增 `vendor/libmpv/THIRD-PARTY-LICENSES.txt`：GPL-2.0、LGPL-2.1、ISC、MIT、Apache-2.0、FTL、Zlib、BSD-3-Clause、libpng 官方全文合并，头部为组件→许可映射与对应源码说明
- [x] 2.5 三平台打包映射：macOS `tauri.conf.json` `bundle.macOS.files` → `Resources/third-party/libmpv/`；Windows `tauri.windows.conf.json` `bundle.resources`；Linux `bundle.linux.{deb,appimage}.files` → `usr/share/doc/echo/`（含 NOTICE 一并带入安装包）

## 3. 用户媒体权利与隐私边界文档（issue #66 待办三、四）

- [x] 3.1 新增 `docs/media-rights-and-privacy.md`（中英双语）：用户媒体责任、LAN 同步对象与范围、隐私边界（无账号/遥测/云端中继/联网抓取，日志仅本地）、未来新增数据外发功能前先补告知、发布前检查单（含律师复核门槛）
- [x] 3.2 `README.md` / `README.en.md`：许可证章节改为独立定制许可表述（不得再称 BUSL-1.1）；新增「用户媒体权利与隐私」章节并链接新文档

## 4. 门禁与验证

- [x] 4.1 新增 `scripts/verify/checks/task-18.1.mjs` 静态门禁（SPDX 不回归 BUSL、manifest 许可、三平台许可文本映射、README 章节、LICENSES.md 表述），注册进 `scripts/verify/manifest.json`
- [x] 4.2 验证：`cargo metadata`（LicenseRef 可解析）、`cargo deny check licenses`（private 豁免不变）、`node scripts/verify/checks/task-18.1.mjs`、task-1.7 / task-17.1 不回归、`openspec validate clarify-licensing-and-privacy-notices`
- [x] 4.3 归档 change、同步 delta spec 到主规格、提 PR 关联 issue #66
