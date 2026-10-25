# clarify-licensing-and-privacy-notices — 设计

## Context

issue #66 的四项待办都是「文档/元数据与事实不符」类缺陷，不涉及运行时代码。事实核对结论：

- **BUSL-1.1 不可表达「禁止再分发」**：BUSL 1.1 基础授权明确包含 redistribute（copy, modify, create derivative works, redistribute, non-production use），Additional Use Grant 只能追加权利、不得施加额外限制（Licensor Covenants 第 (a) 条）。现 LICENSE 附加限制第 3 条「禁止再分发」与 BUSL 结构根本冲突，「附加限制优先」的兜底条款使许可文本整体不可按 BUSL 解释。
- **Windows 打包的 libmpv 是 GPL**：shinchiro/mpv-winbuild-cmake 的 `packages/ffmpeg.cmake` 带 `--enable-gpl --enable-libx264 --enable-libx265`（x264/x265 为 GPL 组件），FFmpeg 静态链入 `libmpv-2.dll`；vendor manifest 记录的 LGPL-2.1-or-later 与事实不符。
- **Linux 自建 FFmpeg 是 GPL**：`build-linux-libmpv.mjs` 打补丁只改 static/shared，保留 mpv-build 默认 OPTIONS 中的 `--enable-gpl`；FFmpeg configure 在 `--enable-gpl` 下将构建定性为 GPL。脚本 LICENSES 表记录 LGPL-2.1-or-later 与事实不符；mpv 侧未传 `-Dgpl=false`，GPL-2.0-or-later 记录正确。
- **macOS 是仓库内唯一可信的 LGPL 记录**：自建 FFmpeg「GPL and nonfree disabled」，manifest 记录了来源与校验和，维持 LGPL-2.1-or-later。
- **隐私边界事实成立**：源码中除 LAN 同步（HTTPS/TLS, 端口 50826）外无外部网络端点；无 updater/遥测/崩溃上报配置；`transfer_telemetry` 等命名均为本地传输进度计测，不上报远端。

## Goals / Non-Goals

**Goals:** 许可证文件、SPDX 字段、第三方清单、双语 README 四处对许可的表述一致且与事实相符；三平台安装包内可获取 GPL/LGPL 等许可全文与对应源码获取方式；用户媒体权利、LAN 同步范围与隐私边界有单一事实文档；上述一致性有静态门禁防回归。

**Non-Goals:** 不改授权宽严（不迁移到标准 BUSL-1.1，也不删除既有使用限制）；不改构建行为（构建脚本仅改许可记录表与 NOTICE 模板文案）；不出法律意见——律师复核落为发布清单条目；不做 macOS Intel/universal、不做签名/公证。

## Decisions

**D1：许可证定性为独立定制许可，而非标准 BUSL-1.1。** 两条路线中，标准 BUSL-1.1 会被迫删除「禁止再分发」「禁止 AI 转写」等作者明示的限制（BUSL 基础授权不可削减），实质改变授权范围——超出「澄清」的范畴。独立定制许可保留原意，代价是失去 `BUSL-1.1` 这个 SPDX 标识，改用 `LicenseRef-Echo-Personal-Study-1.0`（SPDX 规范对自定义许可的标准写法）。工程影响已核：workspace 成员 `publish = false`（crates.io 校验不适用），cargo-deny `[licenses.private] ignore = true` 使 LicenseRef 不参与 allow-list，`cargo metadata`/`cargo deny` 均接受 LicenseRef 语法。

**D2：有限授权结构替代「宽授权 + 限制优先」结构。** 现 LICENSE 的矛盾根源是先按 BUSL 授予再分发、再用附加限制收回。重写为：第 2 条「授权范围」只列举明示授予的权利（个人学习研究目的的复制/修改/编译/运行/衍生/备份），并声明「未明示授予的权利一概不保留，包括再分发」；限制条款从「收回已授予权利」变为「界定与解释授权边界」，不再需要「冲突时以附加限制为准」的兜底。Change Date/Change License 机制（2099-12-31 → GPL-2.0-or-later）作为独立条款保留，并显式说明本许可不是 BUSL-1.1、仅借鉴其 Change Date 结构。

**D3：第三方组件条款是 GPL/LGPL 分发合规的前提。** Echo 自身条款若按字面禁止一切再分发，会与「随包分发 GPL 的 libmpv/FFmpeg 并允许接收者按 GPL 获取/再分发其源码」冲突（GPL 第 6 条不得对下游施加进一步限制）。新 LICENSE 增加专门条款：第三方组件（含打包的 libmpv/FFmpeg 与 Cargo/npm 依赖）不受本许可约束、始终按其上游许可享有全部权利，本许可的任何限制不减损之。这同时满足 issue「确认 Echo 自身条款没有限制第三方库按其上游许可享有的权利」。

**D4：一份合并的 THIRD-PARTY-LICENSES.txt，而非按平台/按组件拆分。** 三平台 manifest 所列组件的许可集合为 {GPL-2.0, LGPL-2.1, ISC, MIT, Apache-2.0, FTL, Zlib, BSD-3-Clause, libpng}，合并为单文件约 90KB；拆分按平台/组件则需维护三份映射且易漏。文本不是二进制，不触发 task-9.7 的 Resources 纯净性检查（仅拦 `.dll`/`.so`）；三平台各自映射一次即可。文件放在 `vendor/libmpv/`（与三平台 vendor 目录平级、不属于任一平台 manifest 的 files 集），避免干扰 manifest 校验。

**D5：Linux 安装包补带 NOTICE + 许可文本，Windows/macOS 补带许可文本。** `platform-libmpv-supply-chain` 既有要求是「许可证说明随对应平台安装包带入应用」，当前 Linux 仅有 Release 附件（`release.yml` 的 NOTICE-staging），安装包内无任何许可材料——按 spec 本就是缺口。映射位置：macOS `Resources/third-party/libmpv/THIRD-PARTY-LICENSES.txt`；Windows 安装根目录（与 NOTICE-libmpv-win.md 同级）；Linux `usr/share/doc/echo/`（deb 惯例位置，AppImage 同构）。

**D6：门禁用静态断言（task 18.1），不改运行时代码。** 与 task 17.1 同模式：所有变更都是文本/配置，编译期不会失败，回归只能靠静态检查拦截——断言 SPDX 不回归 BUSL-1.1、manifest 许可条目、三平台打包映射、README 章节、LICENSES.md 表述。许可文本的完整性以「包含 GPL-2.0/LGPL-2.1 关键标题行」校验（全文比对留待律师复核）。

## Risks / Trade-offs

- [`LicenseRef-Echo-Personal-Study-1.0` 非 OSI/SPDX 标准标识] → 这是自定义许可的规范表达；`publish = false` + cargo-deny private 豁免使工具链无感。若未来要发 crates.io 需另行决策。
- [Windows manifest 许可改 GPL 影响对 LGPL 义务的理解] → GPL 分发义务更严（对应源码），NOTICE 已补「对应源码」章节说明 shinchiro 构建脚本仓库 + 上游源码的获取路径；最终以律师复核为准（发布清单已列）。
- [合并许可文本可能缺未来新增组件的许可] → task 18.1 断言 manifest 中出现的许可标识都在合并文本中出现，新增组件/许可漏配会直接失败。
- [律师复核未完成前发布] → 属发布流程门槛，落入 `docs/media-rights-and-privacy.md` 的发布前检查单与 ROADMAP 既有发布门禁表述，不由本变更替代。

## Migration Plan

纯文档/配置/许可文本变更，无数据迁移、无运行时行为变化。回滚 = revert。验证：`cargo metadata`（LicenseRef 可解析）、`cargo deny check licenses`（workspace 豁免路径不变）、task 1.7 / 9.7 / 17.1 不回归、新增 task 18.1 通过、`openspec validate`。

## Open Questions

（无）
