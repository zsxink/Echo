# 交付记录

## 基线（2026-11-05，重编译前）

| 项 | 路径 | 体积 |
|---|---|---:|
| vendor dylib 集合（17 个） | `apps/desktop/src-tauri/vendor/libmpv/macos` | 25 MiB（26,302,000 bytes；`du -sh` 25M） |
| Echo.app（未压缩） | `target/release/bundle/macos/Echo.app` | 46 MiB |
| DMG（压缩） | `target/aarch64-apple-darwin/release/bundle/dmg/Echo_0.3.1_aarch64.dmg` | 22,638,316 bytes（22 MiB） |

说明：最终 1.9 已在 `target/aarch64-apple-darwin/release/bundle/macos/` 重建 `Echo.app`（同目录另有 `echo-libmpv-report.md`）；但基线表中的未压缩 app 仍取 `target/release/bundle/macos/Echo.app`、DMG 取现存 `Echo_0.3.1_aarch64.dmg`，两者不是同一构建的严格配对，都是重建前产物，仅用于量级对照。

## 重建后（2026-11-06，独立复核修复后重跑）

| 项 | 路径 | 体积 | 对照基线 |
|---|---|---:|---:|
| vendor dylib 集合（19 个） | `apps/desktop/src-tauri/vendor/libmpv/macos` | 7.6 MiB（7,934,176 bytes；`du -sh` 7.6M） | −18,367,824 bytes（−69.8%）；文件数 17 → 19（新增 Brotli common/decoder） |
| Echo.app（未压缩） | 重建 DMG 内 `Echo.app`（挂载后 `du -sh`；文件字节和 29,479,815） | 28 MiB | 46 MiB |
| DMG（压缩） | `target/aarch64-apple-darwin/release/bundle/dmg/Echo_0.3.1_aarch64.dmg` | 13,164,067 bytes（12.6 MiB） | −9,474,249 bytes（−41.8%） |

说明：实测 vendor 低于提案预估的 15–17 MiB。原因是 `--disable-everything` 后 video/网络组件全部消失，`-Wl,-dead_strip_dylibs` 又去掉了未被引用的 CoreVideo 等框架加载命令，strip 后实际降幅大于按旧全量构建外推的估计。app 体积取最终重建 DMG 内 `Echo.app` 的 `du -sh` 与文件字节和；DMG 为压缩后字节数，两者不是同一层次，不能相加比较。最终 vendor 的全部 dylib minos 不高于 27.0，其中本脚本编译的 7 个 dylib 精确为 27.0，Homebrew 辅助库中 `libgraphite2.3.dylib` / `libintl.8.dylib` 为 26.0。

## 组件探针结果

- 运行时探针（`scripts/release/ffmpeg-component-probe.mjs`）：FFmpeg 6.0，**44 decoders / 10 filters / 6 demuxers / 0 muxers / 0 encoders，输入与输出 protocol 均仅 `file`**；`otool -L` 检查 vendor 与 bundle 共 38 个 dylib，0 条禁止链接（OpenGL / AppKit / VideoToolbox / CoreVideo / Metal / QuartzCore / TLS）。
- 10 个 filter = `aformat`、`alimiter`、`equalizer`、`extrastereo`、`volume`、`aresample` + 四个无条件图端点 `buffer` / `buffersink` / `abuffer` / `abuffersink`。
- `aresample` 为本次验收中发现并修复的回归：mpv 的 lavfi bridge 在效果图输入格式（floatp）与 `aformat=sample_fmts=dblp` 不一致时会自动插入 `aresample`；缺失时每个效果节点报 `'aresample' filter not present, cannot convert formats.` 并导致整条链配置失败。加入后打包应用与 libmpv 运行时探针均无该错误。
- Gate `pnpm verify:task -- 1.9 1.10 9.7` 在最终产物上独占运行全部通过；1.9 重新构建并重签 app，1.10 校验组件/来源/签名/Closure、deployment target、install name、manifest `files` / `fileArchitectures` / vendor / bundle 四者集合相等，9.7 含跨平台注入式自证。release workflow 在 `pnpm echo release` 后也会用 `ECHO_MACOS_BUNDLE` 对实际发布 bundle 运行 1.10 + 9.7。

## 人工播放验收

环境限制：本机 GUI 自动点击不可用（Orca 返回 `permission_denied`，`screencapture` 输出黑屏），因此验收走打包产物的真实播放路径而非模拟点击：启动 Gate 1.9 构建并重签的 `Echo.app/Contents/MacOS/echo`，设置 `ECHO_GATE_DATA_DIR` 到全新临时目录，用单实例文件打开把每个 fixture 交给运行中的实例（与 Finder 双击同一条 `app://file-open-request` → `play_temporary_file` 路径），并从 Rust 诊断日志读取 `mpv actor snapshot` 的播放状态。

- 音效 OFF：`tone-short.{mp3,flac,m4a,ogg,opus,wav}` + `example.ape` 全部交付并进入 `Playing`（7 条 Playing 快照），0 条 `loadfile failed` / `event error` / filter 错误。
- 音效 ON：预置 `desktop-state.json` 的 `builtin:pop` EQ 且 `requestedEnabled: true`，7 个文件全部交付并播放（7 条 Playing 快照），0 条 `audio effect request was not applied` / `'aresample' filter not present` / `failed to configure the filter graph` / `Failed to find filter` / decoder 错误。
- 以上 packaged-app 验收在 2026-11-06 最终重建后重新执行，两次都是 7/7 交付、7 条 Playing 快照、`problems` 为空。
- libmpv 运行时探针（`rpath` 指向最终 `vendor/libmpv/macos`，使用与 `native_effects` 相同的 `af` 图）对同一 7 个文件在 effects on/off 下均解码并推进播放位置（各文件 `position` 均 > 0.25 s，退出码 0），无 filter 缺失错误；`DYLD_PRINT_LIBRARIES=1` 确认实际加载的是 vendor 目录内的 `libmpv.dylib`。
- APE 样本：`https://example-files.online-convert.com/audio/ape/example.ape`（Monkey's Audio 3990，stereo 44.1 kHz，2,462,664 bytes），SHA-256 `11671a8798e710b85d209155d8cc810b8f6770c599991df059b87fc82bb4ac7c`；仅用于本地验收，未提交到 `fixtures/`。
- 音效链确认依据：Rust 侧成功应用不写日志，失败会写 `audio effect request was not applied`（本次修复前已在打包应用上观察到该 WARN）。开启预置后连续播放 7 个文件未出现该 WARN，且 `desktop-state.json` 保留 `requestedEnabled: true` 与完整 EQ 文档，说明文档被接受并提交给播放 actor。

## 回滚方式

回滚为 `git revert` 对应提交（vendor 二进制、manifest、NOTICE、构建脚本与 Gate 一起回退）；无运行时数据迁移或用户可见行为变化。
