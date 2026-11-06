# Proposal

## Why

macOS vendored libmpv/FFmpeg 目前来自仓库外的手工构建（原 `/tmp/echo-macos-audio-build` 已不存在），且 FFmpeg 只在默认全量组件上追加了 `--enable-filter`，没有 `--disable-everything`。2026-11-05 用运行时组件探针核实：当前 vendor 的 `libavfilter` 注册 437 个 filter、`libavcodec` 662 个 codec 条目、`libavformat` 343 个 demuxer；h264 / hevc / prores、scale / overlay、matroska / mpegts 等视频组件全部进入发布包，而 Echo 的保证格式只有 mp3 / flac / ape / m4a / ogg / opus / wav。构建不可复现，且为纯音频产品携带了 100% 冗余的视频与网络协议代码。

issue #138 记录了完整复核结论；#137 已完成 strip 与未引用 mbedTLS 清理。本变更在 #137 的交付状态上重编译最小音频栈，并继续保持 strip、arm64-only 与供应链记录一致的约束。

## What Changes

- **新增可复现的 macOS vendor 构建脚本**：`scripts/release/build-macos-libmpv.mjs`（含跨平台纯函数单测）。脚本固定 FFmpeg 6.0 与 mpv 0.36.0 源码及 SHA-256，应用 `scripts/release/patches/` 下现有两个 mpv 补丁，按 arm64-only 构建，并把完整 configure / meson 参数写入 manifest。辅助库（libass / freetype / fribidi / harfbuzz 等）沿用 Homebrew，版本记录进 manifest / NOTICE。构建完成后复用 `thin-macos-libmpv.mjs --write` 完成 strip、ad-hoc 重签、SHA-256 与 `fileArchitectures` 重算。
- **FFmpeg 最小化**：使用 `--disable-everything`，只启用 7 种保证格式所需的 demuxer / decoder / parser（含 `ape`），只启用音效链所需的 `aformat,alimiter,equalizer,extrastereo,volume` 五个音频滤镜与 mpv 格式协商必需的 `aresample`；关闭 encoders / muxers / programs / avdevice / network / autodetect；仅保留 `file` 协议；不启用 audiotoolbox / videotoolbox；保留 shared build 与 `--enable-small`。FFmpeg 的 `print_enabled_components` 会无条件注册 `buffer`/`buffersink`/`abuffer`/`abuffersink` 四个图端点（`buffersink.o`/`buffersrc.o` 也无条件编译），因此 Gate 的音频组件白名单包含这四个端点，其余视频滤镜一律拒绝。
- **mpv 最小化**：关闭 cocoa / gl / videotoolbox-gl / vulkan / libplacebo 及不用的子系统；保留 `-Dcplayer=false`、`-Dlibmpv=true`、`-Dcoreaudio=enabled`；保留 libass 与 libswscale——二者是 mpv 0.36 的硬依赖，本变更不修改 mpv 源码。mpv 0.36.0 的 `meson_options.txt` 没有 `videotoolbox-pl` 选项（只有 `videotoolbox-gl`），脚本启动时必须用该文件校验选项名，拒绝未知选项，避免照搬新版参数导致 meson 配置失败。
- **同步供应链记录**：重算 manifest 的 `files` / `fileArchitectures` / `ffmpegBuild` / `mpvBuild`，更新 `NOTICE.md` 的构建说明与参数；`tauri.conf.json` 的 `bundle.macOS.files` 按实际 vendor 集合同步。本变更不删除 libass / 字体栈；实测文件集合由 17 个增至 19 个 dylib（新增 `libbrotlicommon.1.dylib` / `libbrotlidec.1.dylib`），所有 dylib 维持 arm64-only。
- **Gate 增加构建纯度断言**：在 macOS-only 的 `task-1.10.mjs` 中运行运行时组件探针（`av_filter_iterate` / `av_codec_iterate` / `av_demuxer_iterate` / `avio_enum_protocols`），拒绝视频 decoder / 视频处理 filter / 视频容器 demuxer、网络协议与 TLS 后端，断言 7 种格式组件、5 个音频滤镜、`aresample` 及四个无条件图端点存在；同时断言内嵌 `avcodec_configuration()` 含 `--disable-everything` 且不含 `--enable-videotoolbox`，并拒绝 `libmpv` 直接链接 OpenGL / AppKit / VideoToolbox / CoreVideo。探针的纯解析与判定函数抽到 `scripts/release/ffmpeg-component-probe.mjs`，注入式自证单测注册进 9.7 的任务组（跨平台运行）。
- **体积验收**：记录重编译前后 `Echo.app` 与 DMG 的实测体积；实施前预估 vendor 从约 25 MiB 降到 15–17 MiB（约省 9–12 MB 未压缩），以重编译产物为准。实测 vendor 为 7.6 MiB（7,934,176 bytes），明显低于预估区间：`--disable-everything` 后视频/网络组件全部消失、`-Wl,-dead_strip_dylibs` 又去掉未被引用的框架加载命令，strip 后的实际降幅大于按旧全量构建外推的估计；原因详见 `delivery-notes.md`「重建后」说明。

**BREAKING**：无。播放格式、音效链、文件关联、IPC 契约与用户可见行为不变；变化只在发布产物的二进制构成、构建来源与供应链记录。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `platform-libmpv-supply-chain`：新增「macOS 播放后端可复现最小音频构建」要求——macOS vendor 后端 MUST 由仓库内固定源码 pin 的构建脚本产出，只包含 7 种保证格式所需的音频组件与 5 个音效滤镜，MUST NOT 包含视频组件、网络协议或 TLS 后端；mpv 的硬依赖（libass、libswscale）MAY 保留且 MUST 在 manifest / NOTICE 说明理由。新增「构建组件集回归门禁」要求——Gate MUST 用运行时组件探针或等价可执行检查验证上述集合，并带注入式自证。

## Impact

- **产品 / 路线图基准**：`docs/DESIGN.md` 的「桌面播放由桌面适配层管理、不进入共享 Core」分层不变；`docs/ROADMAP.md` 的 0.3.0 桌面发布与三平台 libmpv 基座不受影响。本变更只作用于 macOS 发布产物的字节构成、构建来源与供应链记录，不改变格式覆盖与用户可见功能。
- **构建 / 打包**：新增 `scripts/release/build-macos-libmpv.mjs` 及其测试；`apps/desktop/src-tauri/vendor/libmpv/macos/` 下的 FFmpeg / libmpv dylib 重新生成，`manifest.json` 的 SHA-256、`fileArchitectures` 与构建说明更新，`NOTICE.md` 同步；实测文件集合由 17 个增至 19 个 dylib（新增 `libbrotlicommon.1.dylib` / `libbrotlidec.1.dylib`），`tauri.conf.json` 的 `bundle.macOS.files` 因此新增 2 行，共 21 项 = 19 个 Frameworks + NOTICE + THIRD-PARTY-LICENSES.txt。`codesign`、`signingIdentity: "-"`、`hardenedRuntime: false` 流程不变。
- **验证 Gate**：`scripts/verify/checks/task-1.10.mjs` 增加组件纯度、内嵌配置与 mpv 视频后端链接断言；`scripts/release/ffmpeg-component-probe.mjs` 的注入式自证单测与 `scripts/release/build-macos-libmpv.test.mjs` 注册进 `scripts/verify/manifest.json` 的 9.7 任务组。`pnpm verify:task -- 1.9 1.10 9.7` 须按顺序独占运行（9.7 依赖 1.9 的重签产物，且治理门有墙钟断言）。
- **供应链 / 许可**：`licensing-privacy-notices` 的既有义务不变；libass / 字体栈仍在 vendor 中，相关许可条目保留。若后续单独实施「最小字体栈」再删除组件，才需要同步删减许可条目（本变更明确不做）。
- **代码规范**：适用 `openspec/CODE_STANDARDS.md` 的单一职责、纯函数可测、注释解释约束、不在业务路径静默失败；`engineering-governance` 要求新增 Gate 断言带「注入违规即失败」证明，且规格 / 追溯 / 场景三方一致。
- **交付流程**：关联 issue #138，在独立分支实施，通过关联该 issue 的 PR 合并到 `main`；实现与验证完成后先同步 delta spec、归档 change，再创建 PR。
