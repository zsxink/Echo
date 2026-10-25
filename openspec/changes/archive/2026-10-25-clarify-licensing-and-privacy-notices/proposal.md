# clarify-licensing-and-privacy-notices

## Why

发布前法律风险排查（issue #66）发现四类工程层面的许可与告知缺口：

1. **项目许可证自相矛盾**：根目录 `LICENSE` 标题使用 Business Source License 1.1 的名称与结构，正文第 1 条授予「再分发」权，附加使用限制第 3 条又全面禁止再分发，并声明「附加限制优先」。标准 BUSL-1.1 的基础授权本身包含再分发权，且 Additional Use Grant 参数只能增加权利、不能削减（MariaDB BSL 1.1 Licensor Covenants），因此「禁止再分发」无法用标准 BUSL-1.1 表达；`Cargo.toml` 与 `LICENSES.md` 又把项目标为 `BUSL-1.1` SPDX，与实际文本不符。
2. **libmpv 许可记录与事实不符**：`LICENSES.md` 把三平台打包的 libmpv 概括为「LGPL-2.1+ / ISC」。逐平台核对构建配置证实：Windows 的 shinchiro 构建 FFmpeg 带 `--enable-gpl --enable-libx264 --enable-libx265`（静态链入 libmpv-2.dll），实际许可是 **GPL-2.0-or-later** 而非 manifest 记录的 LGPL-2.1-or-later；Linux 自建 FFmpeg 保留 mpv-build 默认的 `--enable-gpl`，也是 GPL-2.0-or-later 而非脚本 LICENSES 表记录的 LGPL。GPL 分发义务（许可文本随包、对应源码获取方式）在三个平台的安装包内均未完整提供——macOS 仅随包 NOTICE，Linux 安装包内连 NOTICE 都没有（仅作为 Release 附件上传）。
3. **用户媒体权利未说明**：导入会复制用户音频，LAN 同步会在配对设备间复制音频与歌词文件，但 README/文档未说明用户须自行拥有这些内容的权利，也未声明 Echo 不取得任何音乐/歌词/封面授权。
4. **隐私边界未记录**：当前无账号、无云端中继、无遥测、无联网歌词/封面抓取的事实（已逐项核对源码确认）没有落成文档，未来若新增相关功能缺少「先补告知」的约束。

## What Changes

- **许可证定性为独立定制许可并消除内部矛盾**：`LICENSE` 重写为「Echo 个人学习使用许可 1.0」（Echo Personal Study License 1.0）——保留既有授权意图（仅限个人学习研究、禁止商业用途、禁止销售/转卖、禁止再分发、禁止 AI 转写、Change Date 2099-12-31 后按 GPL-2.0-or-later 提供），但：授权条款改为「仅授予明示列举的权利」的有限授权结构，消除「正文授予再分发、附加限制再收回」的矛盾；补充授权主体（Echo 项目作者，GitHub: zsxink）、再分发例外（个人备份、Change Date 后、第三方组件上游许可）、适用法（中华人民共和国法律）与争议管辖；新增第三方组件条款——第三方组件不受本许可限制、按其上游许可享有全部权利（GPL/LGPL 分发合规的前提）。
- **SPDX/元数据统一**：`Cargo.toml` 的 `license` 由 `BUSL-1.1` 改为 `LicenseRef-Echo-Personal-Study-1.0`（自定义许可的标准 SPDX LicenseRef 写法；workspace 成员本就 `publish = false` 且 cargo-deny `[licenses.private]` 豁免）；`deny.toml`、`LICENSES.md` 的注释与表述同步更新，`LICENSES.md` 的 libmpv 说明改为逐平台准确表述。
- **libmpv 许可记录纠错**：Windows `vendor/libmpv/windows/manifest.json` 的 libmpv 与 FFmpeg 条目 LGPL→GPL-2.0-or-later（依据 shinchiro 构建配置核实）；`scripts/release/build-linux-libmpv.mjs` 的 LICENSES 表 FFmpeg 条目 LGPL→GPL-2.0-or-later（`--enable-gpl` 在打补丁后的 OPTIONS 中保留）；Windows NOTICE 补「Licenses」与「对应源码」章节，macOS NOTICE 补对应源码与许可文本指引，Linux NOTICE 模板引用统一许可文本。
- **许可文本随安装包分发**：新增 `apps/desktop/src-tauri/vendor/libmpv/THIRD-PARTY-LICENSES.txt`（GPL-2.0、LGPL-2.1、ISC、MIT、Apache-2.0、FTL、Zlib、BSD-3-Clause、libpng 全文的合并文本，覆盖三平台 manifest 所列全部组件），macOS/Windows/Linux 三平台 tauri 配置均映射进安装包（Linux 同时把 NOTICE 一并带入 deb/AppImage，补齐「随安装包带入应用」的既有 spec 缺口）。
- **用户媒体权利与隐私边界文档**：新增 `docs/media-rights-and-privacy.md`（中英双语）：用户媒体责任（须有权在自己设备间复制/同步，Echo 不取得也不转授音乐/歌词/封面版权）、LAN 同步范围（配对设备间、HTTPS/TLS、同步音频与同名 `.lrc` 及曲库元数据、不同步 SQLite 文件、无云端中继）、隐私边界（无账号/遥测/崩溃上报/云备份/联网歌词封面抓取，日志仅本地保留；未来新增相关功能前须先补充隐私告知）、发布前法律复核清单（律师复核为发布门槛）。中英文 README 新增「用户媒体权利与隐私」章节并更新许可证章节表述。
- **回归门禁**：verify 体系新增 task 18.1 静态检查——断言 Cargo SPDX 与 LICENSE 定性一致（不得回归 BUSL-1.1 标注）、LICENSES.md 逐平台许可表述、三平台 tauri 配置均携带 THIRD-PARTY-LICENSES.txt、Windows manifest 许可为 GPL、README 双语包含媒体权利与隐私章节。

非目标：不改变授权的实际宽严（既不放宽为标准 BUSL-1.1，也不移除既有使用限制）；不改构建脚本的构建行为（仅纠正其许可记录）；法律意见本身（律师复核）不在工程变更内，仅落为发布清单条目；macOS Intel/universal 打包与签名/公证维持现状。

## Capabilities

### New Capabilities

- `licensing-privacy-notices`：项目许可声明一致性（LICENSE/SPDX/LICENSES.md/README 不得互相矛盾）、用户媒体权利与 LAN 同步范围说明、隐私边界与未来功能告知义务、发布前法律复核清单。

### Modified Capabilities

- `platform-libmpv-supply-chain`：既有「第三方许可证说明」要求强化——许可记录 MUST 与实际构建配置一致（GPL 开启时不得记为 LGPL）；完整许可文本 MUST 随三平台安装包带入应用（不仅是 Release 附件）；各平台 NOTICE MUST 说明对应源码获取方式。

## Impact

- `LICENSE`：整体重写（独立定制许可，消除内部矛盾）。
- `Cargo.toml`：workspace `license` 改为 `LicenseRef-Echo-Personal-Study-1.0`；`deny.toml` 注释同步。
- `LICENSES.md`：许可定性、workspace 行、libmpv 逐平台说明改写。
- `apps/desktop/src-tauri/vendor/libmpv/`：新增 `THIRD-PARTY-LICENSES.txt`；`windows/manifest.json` 许可条目纠正；`windows/NOTICE.md`、`macos/NOTICE.md` 补章节。
- `scripts/release/build-linux-libmpv.mjs`：LICENSES 表 FFmpeg 条目纠正 GPL-2.0-or-later。
- `apps/desktop/src-tauri/tauri.conf.json`、`tauri.windows.conf.json`：三平台许可文本（及 Linux NOTICE）打包映射。
- `docs/media-rights-and-privacy.md`：新增；`README.md` / `README.en.md`：许可证章节改写 + 媒体权利与隐私章节。
- `scripts/verify/checks/task-18.1.mjs`：新增门禁；`scripts/verify/manifest.json` 注册。
- 关联 issue：直接完成 #66 的四项待办；验收标准中的「律师复核」落为发布清单门槛项，属 issue 遗留的人工动作。
