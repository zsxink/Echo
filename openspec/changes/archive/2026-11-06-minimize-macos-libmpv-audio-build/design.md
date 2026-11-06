# Design

## Context

当前 macOS vendor 是 17 个 arm64-only dylib，`manifest.json` 已经固定了 FFmpeg 6.0 与 mpv 0.36.0 的下载地址、SHA-256 和两个 mpv 补丁，但仓库里没有生成它们的脚本；`buildProcedureStatus` 仍写着「clean-room rebuild remains pending」。2026-11-05 的运行时探针显示，这份二进制注册了 437 个 filter、662 个 codec 条目、343 个 demuxer，`libmpv.dylib` 直接链接 libass、libswscale、OpenGL、AppKit、CoreVideo，`libavcodec.dylib` 直接链接 VideoToolbox。也就是说，manifest 的供应链 pin 是完整的，缺的是把这些 pin 变成可执行、可校验的构建路径。

本设计只解决「怎么重建并证明重建结果更小、更纯」，动机与范围见 `proposal.md`，组件清单的规范性要求见 delta spec。约束来自三处：

- `platform-libmpv-supply-chain` 既有要求：arm64-only、SHA-256 与 `fileArchitectures` 精确匹配、依赖闭包 == manifest == bundle、strip 后无成规模本地符号、许可说明与二进制一致。
- Gate 顺序：`1.9` 先构建并重签 app bundle，`1.10` 读 bundle 做 provenance，`9.7` 再做闭包与跨平台接线；三者必须按顺序独占运行。
- `fixtures/audio` 只允许可再生成的原创内容，且系统 FFmpeg 没有 Monkey's Audio 编码器，因此仓库不可能用生成器造出 `.ape` fixture。

## Goals / Non-Goals

**Goals:**

- 用仓库内脚本从固定源码重建整个 macOS vendor 集合，构建参数、辅助库版本与工具链可审计。
- FFmpeg 只保留 7 种保证格式所需的音频组件、5 个音效滤镜与四个无条件图端点；mpv 只保留 libmpv + CoreAudio，不链接任何视频输出后端。
- 重建候选在替换 vendor 之前先通过同一个运行时组件探针；不合格的构建进不了仓库。
- manifest / NOTICE / Gate 三处记录与二进制一致，且新增断言带注入式自证。

**Non-Goals:**

- 不追求跨工具链的字节级可复现（不同 Xcode / Homebrew 版本会改变字节）；可复现指输入、参数与组件集可重放，最终哈希按次记录。
- 不修改 macOS deployment target（当前二进制已是 minos 27.0，调整它是产品决策）。
- 不删除 libass / 字体栈，不修改 mpv 源码，不升级 FFmpeg / mpv。
- 不修 Linux 构建脚本的 APE 遗漏，不做系统解码器复用。
- 不新增自动化解码 smoke 或 APE fixture；格式矩阵的机器保证是组件探针，实际播放用现有 fixture 加本地 APE 样本人工验收。

## Decisions

### D1. 独立构建脚本，而不是复用 mpv-build

新增 `scripts/release/build-macos-libmpv.mjs`，直接驱动 FFmpeg configure 与 mpv meson。mpv-build 的 `scripts/ffmpeg-config` 默认静态链接 FFmpeg，Linux 脚本必须运行时改写它；macOS 需要的是可控的 dylib 安装名、Homebrew 辅助库闭包和固定的 configure 前缀，绕开这层包装更直接，也少一处上游脚本漂移。

脚本以 `apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json` 为唯一 pin 来源：`sourceUrl` / `assetSha256` 是 FFmpeg，`additionalSources` 是 mpv 与两个补丁。流程：

1. 校验宿主机是 `darwin` + `arm64`，否则直接失败；不做事后裁剪来掩盖 x86_64 构建。
2. 下载 FFmpeg 与 mpv 归档到 `--work` 下的独立目录，逐个比对 manifest 的 SHA-256；校验失败即停。
3. 解包 mpv，按 manifest 顺序 `patch -p1 --batch --forward` 应用两个补丁；任何一个 hunk 被拒即停。
4. 构建并安装 FFmpeg 到 `--work/ffmpeg-dest`，再构建 mpv 到 `--work/mpv-dest`。
5. 从 `libmpv.dylib` 出发收集依赖闭包，把 FFmpeg 库与 Homebrew 辅助库复制到 `--out`，统一安装名，并对候选 dylib 做 ad-hoc 重签（`install_name_tool` 会作废 arm64 代码签名，未重签的库无法加载）。
6. 对 `--out` 运行组件探针；不合格就停在候选目录，不碰 vendor。
7. 只有显式传 `--write-vendor` 才把候选写回 `vendor/libmpv/macos/` 并写入 manifest / NOTICE 的结构化字段；随后调用现有的 `thin-macos-libmpv.mjs --write` 完成 strip、ad-hoc 重签与 SHA-256 / `fileArchitectures` 重算。`thin-macos-libmpv.mjs` 目前把 vendor 路径固定在仓库内，因此不能直接对 `--out` 运行；流程设计成「先复制、再 thin」，不为了复用而重构 Gate 关键脚本。默认（不带 `--write-vendor`）只留候选目录，避免一次失败构建污染 vendor。

备选方案：把所有命令写进文档由维护者手跑（不可执行、无法在 PR 里验证，否决）；复用 mpv-build（静态 FFmpeg 与 macOS dylib 身份不匹配，否决）。

### D2. 固定 configure 前缀 + DESTDIR，避免临时路径进入二进制

FFmpeg 的 `avcodec_configuration()` 会内嵌 `--prefix` 等 configure 参数。如果直接用 `--prefix=<work>/ffmpeg`，每次重建的 workdir 不同，内嵌字符串与 manifest 记录都会漂移。构建统一使用 `--prefix=/usr/local`，安装时用 `DESTDIR=<work>/ffmpeg-dest`。

mpv 的 meson 通过 pkg-config 找 FFmpeg。直接指向 DESTDIR 时 `.pc` 文件里的 `prefix=/usr/local` 会解析到宿主机 `/usr/local`，可能误用 Homebrew 的 FFmpeg 9.0。因此安装后对六个 `.pc` 文件做一次受控改写：把 `prefix=`、以及指向同一 configure 前缀的绝对 `libdir=` / `includedir=` 改到 `<work>/ffmpeg-dest/usr/local`，只影响这次构建的 pkg-config 搜索路径。FFmpeg 6.0 的 `.pc` 里 `libdir` / `includedir` 是绝对路径，只改 `prefix=` 仍会输出 `-I/usr/local/include`，从而误取宿主头文件。该改写抽成纯函数，单测覆盖。mpv 安装同样用 `--prefix=/usr/local` + `meson install --destdir`。

工具链其余部分不 pin，但 deployment target 显式固定为 `MACOSX_DEPLOYMENT_TARGET=27.0`：构建脚本把它注入 configure / make / meson / ninja 的 env，并用 `--extra-cflags=-mmacosx-version-min=27.0` 与 `--extra-ldflags=-Wl,-dead_strip_dylibs -mmacosx-version-min=27.0` 把它写进内嵌配置串（见 D3），不继承宿主默认值。Gate 实测：本脚本编译的 7 个 dylib 的 minos 精确为 27.0，vendor / bundle 全部 dylib 的 minos 都 ≤ 27.0；Homebrew 辅助库允许更低的 minos（如 `libgraphite2.3.dylib` / `libintl.8.dylib` 为 26.0），不算违规。记录的 `clang --version`、SDK 版本、宿主 macOS 版本与固定的 `MACOSX_DEPLOYMENT_TARGET` 写入 manifest 的 `builds.*.toolchain` / `buildHost`。

### D3. FFmpeg 组件集：`--disable-everything` 加显式音频白名单

`--disable-everything` 只 unset `AVCODEC/AVDEVICE/AVFILTER/AVFORMAT_COMPONENTS_LIST`（decoder、encoder、parser、bsf、hwaccel、filter、demuxer、muxer、protocol、indev、outdev），不会关闭 `libavcodec` / `libavformat` / `libavfilter` / `libswscale` / `libswresample` 这些库，因此 mpv 的硬依赖仍然可链接。`--disable-avdevice` 不在组件列表里，必须单独传。`--enable-filter` / `--enable-decoder` 的逗号列表由 configure 的 `filter()` 做后缀匹配，`pcm_*` 通配有效。

固定的 configure 参数（与 `FFMPEG_CONFIGURE_ARGS` 及 manifest `builds.ffmpeg.configure` 的 25 项逐 token 一致）：

```text
--prefix=/usr/local
--arch=arm64
--target-os=darwin
--enable-shared
--disable-static
--disable-everything
--disable-autodetect
--disable-programs
--disable-doc
--disable-debug
--disable-avdevice
--disable-network
--disable-securetransport
--disable-audiotoolbox
--disable-videotoolbox
--disable-zlib
--extra-cflags=-mmacosx-version-min=27.0
--extra-ldflags=-Wl,-dead_strip_dylibs -mmacosx-version-min=27.0
--enable-small
--enable-pthreads
--enable-protocol=file
--enable-demuxer=mp3,flac,ape,mov,ogg,wav
--enable-decoder=mp3,mp3float,flac,ape,aac,aac_latm,alac,vorbis,opus,pcm_*
--enable-parser=aac,flac,mpegaudio,vorbis,opus,ac3
--enable-filter=aformat,alimiter,equalizer,extrastereo,volume,aresample
```

注释：`--extra-cflags` / `--extra-ldflags` 的 `-mmacosx-version-min=27.0` 把 deployment target 显式写进内嵌配置串（见 D2 与任务 7.1）；`--enable-parser=...,ac3` 的 AC3 parser 是 ld64 链接闭包依赖，`--enable-filter=...,aresample` 的 aresample 是音效图格式协商依赖，两点详见下方两节。

几个容易漏掉的点：

- **四个图端点不受 `--disable-everything` 控制。** FFmpeg 6.0 的 `print_enabled_components` 在生成 `filter_list` 后无条件追加 `asrc_abuffer`、`vsrc_buffer`、`asink_abuffer`、`vsink_buffer` 四项，`libavfilter/Makefile` 也把 `buffersink.o` / `buffersrc.o` 列为无条件对象。所以最小构建仍会注册 `buffer`、`buffersink`、`abuffer`、`abuffersink`；mpv 0.36 的 `filters/f_lavfi.c` 在音频图上按名查找 `abuffer` / `abuffersink`，不需要额外 configure 开关。Gate 必须把这四个端点列为基础白名单；其中 `buffer` / `buffersink` 是视频图端点，不是视频处理滤镜，stock FFmpeg 无法在不打补丁的情况下移除。
- **没有 `--disable-video`，也不能 `--disable-swscale`。** 前者不是 FFmpeg 6.0 的选项；后者会破坏 mpv 0.36 的硬依赖。视频裁掉靠的是组件白名单，不是库开关。
- **CoreFoundation / CoreMedia / CoreVideo 用 `-dead_strip_dylibs` 剥离。** FFmpeg 6.0 的 macOS configure 无条件探测这三个 framework，且它们位于 `avutil_suggest`，`--disable-videotoolbox` 并不会阻止 `EXTRALIBS-avutil` 带上 `-framework CoreVideo`；configure 又不接受 `--disable-corevideo`（unknown option），framework 探测还会在选项解析后重新启用它们。三者对应的 `CONFIG_*` 符号在没有 videotoolbox 时没有代码引用，因此用 ld64 的 `-Wl,-dead_strip_dylibs` 去掉未使用的 framework 加载命令，不改变组件集。
- **`aresample` 是音效图的格式协商依赖。** mpv 通过 lavfi 桥把 `af` 节点接到音频链上；当源格式（如 `floatp`）与 `aformat=sample_fmts=dblp` 不一致时，libavfilter 自动插入 `aresample` 做采样格式转换。`--disable-everything` 会把它一并裁掉，结果是音效图每个节点都报 `'aresample' filter not present, cannot convert formats` 并失败。它不是音效滤镜，但属于运行时必需的基础设施，必须加入 `--enable-filter` 白名单并由探针断言存在。5.3 的包内播放探针（libmpv 客户端加载 fixture 后安装完整音效图）复现并确认了这一点。
- **AC3 parser 是 AAC parser 的链接闭包依赖。** FFmpeg 6.0 在启用 `aac` parser 时会编译 `aac_ac3_parser.o`，该对象无条件引用 `ff_ac3_find_syncword`，而符号本身只存在于 `CONFIG_AC3_PARSER` 分支。默认构建两个 parser 同时开启，所以上游没有暴露；ELF 允许共享库保留未定义符号，ld64 直接报错。最小 macOS 构建必须同时启用 `ac3` parser（纯音频，不引入保证格式之外的容器或视频组件），否则只能给 FFmpeg 增加第三个补丁。

不启用 encoder / muxer / bsf：Echo 只读本地文件，不需要转码或复用。configure 对未命中的组件只打印 `did not match anything` 警告而不失败，所以脚本必须扫描 configure 日志，把该警告当错误（同时保留探针作为第二道防线）。

### D4. mpv 选项集按 0.36.0 的 `meson_options.txt` 校验

固定选项：

```text
-Dcplayer=false -Dlibmpv=true -Dgpl=false -Dbuild-date=false -Dtests=false
-Dcoreaudio=enabled -Daudiounit=disabled
-Dcocoa=disabled -Dgl=disabled -Dplain-gl=disabled -Dgl-cocoa=disabled
-Dvideotoolbox-gl=disabled -Dvulkan=disabled
-Dlibplacebo=disabled -Dlibplacebo-next=disabled
-Dmacos-cocoa-cb=disabled -Dmacos-media-player=disabled -Dmacos-touchbar=disabled
-Dlua=disabled -Djavascript=disabled -Dlibarchive=disabled -Dlibavdevice=disabled
-Dzlib=disabled -Duchardet=disabled -Dlibbluray=disabled -Ddvdnav=disabled
-Dcdda=disabled -Dsdl2=disabled -Dsdl2-audio=disabled -Dsdl2-video=disabled
-Dsdl2-gamepad=disabled -Drubberband=disabled -Dlcms2=disabled -Dzimg=disabled
-Dvector=disabled -Djpeg=disabled -Dvapoursynth=disabled -Dcplugins=disabled
-Dswift-build=disabled -Dmanpage-build=disabled -Dhtml-build=disabled -Dpdf-build=disabled
```

`v0.36.0/meson_options.txt` 里没有 `videotoolbox-pl`（只有 `videotoolbox-gl`），照搬较新 mpv 的参数会让 meson 直接失败。脚本在配置前解析 pin 住的 mpv 源码里的 `meson_options.txt`，把选项名与声明集合比对，未知选项立即报错；不静默丢弃，也不依赖「meson 大概会忽略」。该检查抽成纯函数，用一段最小选项表做单测。

保留 libass / libswscale 是 mpv 的硬依赖，不是遗漏；manifest 与 NOTICE 必须把这一点写清楚。`-Dgl=disabled` 同时断掉 libmpv 对 OpenGL 的直接链接，`-Dvideotoolbox-gl=disabled` 断掉 VideoToolbox GL interop。

### D5. 辅助库使用 Homebrew 闭包，安装名统一成现有布局

辅助库（libass、freetype、fribidi、harfbuzz、glib、graphite2、libintl、pcre2、libpng、libunibreak）继续用 Homebrew 产物，版本由 `brew list --versions` 在构建时记录。脚本递归读取 `otool -L`：

- 路径在 Homebrew prefix 下的 dylib：复制进候选目录，并改写引用；
- `/usr/lib`、`/System/Library`、`/Library/Apple` 下的系统库与框架：保留绝对路径，不复制；
- 其余绝对路径：构建失败，指出该引用，避免把宿主机私有路径带进发布包。

安装名沿用当前 vendor 的约定：每个文件的 ID 设为 `@rpath/<basename>`，对候选集合内部的依赖改写为 `@loader_path/<basename>`。这正是 `1.9` / `1.10` / `9.7` 已经接受的形态，也避免依赖可执行文件的 rpath 顺序。改写计划抽成纯函数，输入 `otool -L` 文本与文件集合，输出 `install_name_tool` 调用列表，便于单测。

闭包可能随 Homebrew 版本变化（例如某个辅助库不再依赖 libpng）。脚本不硬编码 17 个文件；以闭包为准，写回 manifest 后由 9.7 的「manifest == 闭包 == bundle 映射」与 1.10 的「manifest.files / fileArchitectures / vendor / bundle 四集合相等」断言兜底。实测闭包由 17 个增至 19 个 dylib（新增 `libbrotlicommon.1.dylib` / `libbrotlidec.1.dylib`），`tauri.conf.json` 的 `bundle.macOS.files` 已按实际闭包新增 2 行，共 21 项 = 19 个 Frameworks + NOTICE + THIRD-PARTY-LICENSES.txt；许可记录同步核对，不能假设文件集合不变。

### D6. 运行时组件探针：构建期自检 + 1.10 Gate 共用同一套判定

新增两个文件：

- `scripts/release/ffmpeg-component-probe.c`：不依赖 FFmpeg 头文件（CI 不装 SDK 头），只镜像公开结构的前缀：`AVCodec.name/type`（`libavcodec/codec.h`）、`AVInputFormat.name`、`AVOutputFormat.name`（`libavformat/avformat.h`）与 `AVFilter.name/inputs/outputs`（`libavfilter/avfilter.h`）。filter pad 不再按内部 `AVFilterPad` 布局遍历——6.0 的 pad 数组不以 NULL 结尾，按私有结构读取会提前停止——改用公开的 `avfilter_filter_pad_count()` 与 `avfilter_pad_get_type()` 读取真实 pad 数量与类型。另声明 `av_filter_iterate` / `av_codec_iterate` / `av_demuxer_iterate` / `av_muxer_iterate` / `avio_enum_protocols` / `avcodec_configuration` / `av_version_info` / `av_codec_is_decoder` / `av_codec_is_encoder` / `mpv_client_api_version`。输出 TSV，包含版本、内嵌配置、每个 decoder / encoder 的类型、每个 filter 的输入/输出 pad 类型、demuxer / muxer 名、输入与输出 protocol、`dladdr` 报告的实际加载镜像路径，以及一行 counts。版本断言保证升级时不会静默错读。
- `scripts/release/ffmpeg-component-probe.mjs`：编译并运行探针，解析输出，导出纯判定函数。判定规则：
  - 版本等于 manifest 的 `builds.ffmpeg.version`；内嵌配置串经 shell token 化后必须与构建脚本的 `FFMPEG_CONFIGURE_ARGS` 逐 token 相等，且含 `--disable-everything`、`--disable-network`、`--disable-autodetect`，不含 `--enable-gpl` / `--enable-nonfree` / `--enable-videotoolbox` / `--enable-audiotoolbox` / 任一 TLS 后端。
  - 所有 decoder 的 `type` 必须是 AUDIO（1）；必需 decoder 集合存在；除必需集合与 `pcm_*` 外的任何 decoder 都失败；encoder 集合必须为空；ape / flac / mp3 / aac / alac / vorbis / opus / pcm_* 一个不能少。
  - filter 名必须属于 5 个音效滤镜 + 格式协商必需的 `aresample` + `buffer` / `buffersink` / `abuffer` / `abuffersink` 四个无条件端点；每个 filter 有明确的 pad 契约：6 个音频滤镜输入/输出各至少一个 AUDIO pad，`abuffer` 无输入且输出 AUDIO，`abuffersink` 输入 AUDIO 且无输出，`buffer` 无输入且输出 VIDEO，`buffersink` 输入 VIDEO 且无输出。空 pad 列表不再被当作「全部符合」，出现任何集合外 filter 即失败。
  - demuxer 名按逗号拆 token（`mov,mp4,m4a,3gp,3g2,mj2` 是一个注册项），每个 token 必须属于 `mp3/flac/ape/mov/mp4/m4a/3gp/3g2/mj2/ogg/wav`，并且六个主 demuxer 都必须出现；muxer 集合必须为空。
  - 输入与输出 protocol 集合都必须恰好是 `file`。
  - core 枚举中任何一个返回零项、配置串为空、探针退出非零或输出无法解析、counts 与实际解析行数不一致、`dladdr` 报告的加载路径不在被测目录内，都失败——不允许空输出或宿主库静默通过。
  - `otool -L` 的禁止链接检查覆盖 vendor 与 bundle 的每一个 dylib，而不只是 `libmpv.dylib` / `libavcodec.dylib`；出现 OpenGL、AppKit、VideoToolbox、CoreVideo、Metal、QuartzCore 或 openssl / gnutls / mbedtls / libtls 即失败。

构建脚本在第 6 步用同一模块跑候选目录，1.10 再对 vendor 与 bundle 跑一次。1.10 是 macOS-only，适合编译探针；`ffmpeg-component-probe.mjs` 的纯判定函数与 `build-macos-libmpv.mjs` 的参数/清单纯函数由 `ffmpeg-component-probe.test.mjs`、`build-macos-libmpv.test.mjs` 覆盖，注册进 9.7 任务组，在三个平台都运行。

为什么不用 `strings` / `nm`：二进制已 strip，`nm` 没有本地符号；`strings` 对 `scale`、`matroska` 这类短词会在错误消息、路径或无关字符串里误报。运行时枚举读的是实际注册表，才是「这个组件能不能被调用」的证据。

为什么不在 `9.7` 里执行探针：9.7 在 Windows / Linux 也跑，探针只能在 macOS 编译并加载 Mach-O dylib。把执行放 1.10、把纯判定放 9.7 的测试组，既覆盖三平台逻辑，又让 macOS 实际执行一次。能力 `platform-libmpv-supply-chain` 不在 `spec-scenarios.mjs` 的 12 个受追踪区域内，新增场景不会改变 traceability / scenario manifest 的 564 基线；仍要跑 `reconcile-scenarios.mjs` 证明没有漂移。

### D7. manifest / NOTICE 增加结构化构建记录

在现有字段上追加，不替换既有 pin：

```json
{
  "buildScript": "scripts/release/build-macos-libmpv.mjs",
  "builds": {
    "ffmpeg": {
      "version": "6.0",
      "configure": ["--prefix=/usr/local", "..."],
      "deploymentTarget": "27.0",
      "toolchain": "Apple clang ...; SDK ..."
    },
    "mpv": {
      "version": "0.36.0",
      "meson": ["-Dcplayer=false", "..."],
      "patches": ["scripts/release/patches/mpv-lavfi-target-command.patch", "..."],
      "hardDependencies": ["libass.9.dylib", "libswscale.dylib"]
    },
    "auxiliary": [
      { "component": "libass", "version": "0.17.x", "files": ["libass.9.dylib"] }
    ]
  }
}
```

- 顶层 `sourceUrl` / `assetSha256` / `additionalSources` 仍是源码 pin 的唯一权威；两个 mpv 补丁的 SHA-256 与归档 pin 同列在 `additionalSources`，构建前逐个校验；`builds.*.version` 提供给 Gate 做机器比对，必须与 pin 对应。
- `ffmpegBuild` / `mpvBuild` 摘要字符串保留，供 1.10 报告使用；内容从结构化字段派生，Gate 断言二者不矛盾。
- `buildProcedureStatus` 的「pending」改为指向构建脚本与本次已验证的结论。
- NOTICE 的辅助库版本表放进 `<!-- BEGIN GENERATED AUXILIARY VERSIONS -->` / `<!-- END ... -->` 哨兵块，由构建脚本按 `builds.auxiliary` 生成；块外的法律文本与来源说明仍手工维护。Gate 断言每个声明组件与版本都在块内，且块内没有未声明的组件。

`build-macos-libmpv.mjs` 必须导出 `FFMPEG_CONFIGURE_ARGS` 与 `MPV_MESON_OPTIONS` 两个常量（或返回它们的纯函数），1.10 导入后与 manifest 数组做深比较。这样「脚本参数」与「供应链记录」不可能各写一套。脚本入口用 `import.meta.url === pathToFileURL(process.argv[1]).href` 守卫，被导入时不执行构建。

### D8. 格式矩阵的验证边界

机器可证明的是「组件已注册」：探针断言 ape demuxer / decoder 存在，其余六种格式同理。`fixtures/audio` 已有 mp3 / flac / m4a / ogg / opus / wav 与视频 mp4 / 无音轨 mp4 / 损坏 mp3，足以人工播放验收；没有 APE fixture，且 FFmpeg 没有 APE 编码器，仓库的「全部 fixture 由脚本生成、无第三方媒体」政策不允许下载样本。

因此本变更的验收拆成两层：

1. 组件层（自动、每次 Gate）：7 种格式的 demuxer / decoder 必须注册。
2. 播放层（人工、一次）：用现有六个 fixture 加一个本地合法 APE 样本，逐个用打包后的 Echo 打开，确认能开始播放且音效链不报错；证据写进 PR 描述。若以后有可再分发的 APE 生成器或样本，再补 fixture 与自动 smoke。

不在本变更给 1.9 加解码 smoke：issue #138 把 Gate 的范围定为组件纯度；现有 1.9 只验证文件打开交接，不验证解码，扩展它属于另一个验收问题。

### D9. 体积与回滚

体积不是 Gate 断言（同一份源码在不同 Xcode / Homebrew 下会有差异），而是交付证据：记录重编译前后 `vendor/libmpv/macos`、`Echo.app`、DMG 的 `du` 与文件数，写进 PR；实施前预估 vendor 从约 25 MiB 降到 15–17 MiB。实测 vendor 为 7.6 MiB（7,934,176 bytes），低于预估区间——`--disable-everything` 去掉全部视频/网络组件、`-Wl,-dead_strip_dylibs` 去掉未引用的框架加载命令，strip 后实际降幅大于按旧全量构建外推的估计；前后对照表与原因见 `delivery-notes.md`。回滚就是 `git revert` 对应提交（vendor 二进制、manifest、NOTICE、脚本、Gate 一起回退），没有运行时数据迁移或用户可见行为变化。

### D10. release workflow 对实际发布 bundle 复跑 Gate

`.github/workflows/release.yml`（任务 7.4）在 `pnpm echo release` 构建发布产物之后，增加一个 `if: runner.os == 'macOS'` 的步骤，对即将发布的 app bundle 再跑一次 Gate 的 bundle 半程：`pnpm verify:task -- 1.10 9.7`。

- **不重跑 1.9**：1.9 会重新构建并重签 app bundle，发布流程里的构建已完成，重跑既浪费又可能改动即将签名的产物；这里只复用 1.10（来源 / 组件 / 链接 / minos / 四集合相等 / install name 断言）与 9.7（闭包与跨平台逻辑）对既有产物校验。
- **路径覆盖**：Gate 默认读 `target/aarch64-apple-darwin/release/bundle/macos/Echo.app/Contents` 与同目录报告，而 release build 用的是 `target/release`。步骤用 `ECHO_MACOS_BUNDLE: ${{ github.workspace }}/target/release/bundle/macos/Echo.app/Contents` 与 `ECHO_MACOS_REPORT: ${{ github.workspace }}/target/release/bundle/macos/echo-libmpv-report.md` 两个 env 覆盖默认值，让 1.10 读的正是实际发布的 bundle，而不是另一个 triple 目录下的残留产物。
- **接线自证**：1.10 读取 `release.yml` 文本，断言其中同时存在 `ECHO_MACOS_BUNDLE` 与 `pnpm verify:task -- 1.10 9.7`；删掉这一步会让 Gate 失败，避免发布流程静默绕过校验。CI 的 `pnpm verify:task -- 1.9 1.10 9.7` 仍按顺序独占运行，负责开发期重建与验收；release 这一步只是发布前的最后一道复验。

## Risks / Trade-offs

- **最小 filter 集漏掉 mpv 运行时依赖** → 依赖 FFmpeg 无条件注册的 `abuffer` / `abuffersink` 图端点，并用构建期探针确认四个端点都在；打包后人工验收音效链。若还缺别的 filter，运行时错误会指出名字，按需加进白名单并同步 spec / manifest，而不是在 Gate 里放宽。
- **configure 对未命中组件只警告** → 构建脚本扫描 `did not match anything` 并失败；探针二次确认必需组件存在。
- **mpv 选项名漂移** → 配置前解析 pin 源码的 `meson_options.txt`，未知选项立即失败；`videotoolbox-pl` 已确认在 0.36.0 不存在。
- **Homebrew 辅助库版本或闭包变化** → 版本写入 manifest / NOTICE 生成块；9.7 的闭包断言阻止 manifest 与磁盘漂移。升级 Homebrew 导致的闭包变化必须显式同步 `tauri.conf.json` 与许可记录，不能静默通过。
- **探针依赖 FFmpeg 6.0 的公开枚举 API** → pad 读取改用 `avfilter_filter_pad_count` / `avfilter_pad_get_type`，不再镜像私有 `AVFilterPad`；版本断言仍把探针和 6.0 绑定，升级 FFmpeg 时同步更新探针并重新验证，属于另一个 change。
- **APE 只有组件级自动验证** → 人工样本验收写进 PR；剩余风险明确记录，不用「7 种格式全部可播放」的口头断言掩盖。
- **deployment target 继承宿主** → 构建脚本显式设置 `MACOSX_DEPLOYMENT_TARGET=27.0` 与 `-mmacosx-version-min=27.0`，并断言本脚本编译的 dylib minos 精确等于 27.0、所有 dylib 不高于 27.0；宿主升级到 27.2 时不会再静默抬高发布产物的最低系统版本。降低到更低版本仍需单独评估 API 与 CI 镜像。
- **构建耗时长、不能在 CI 每次重跑** → 构建脚本是维护者工具，CI 只验证纯函数与已提交二进制；构建产物进 PR 时附探针输出与体积对比。

## Migration Plan

1. 先落地脚本、探针、纯函数与单测，不碰 vendor；此时 Gate 仍对旧二进制运行，新断言应失败（旧二进制有全量视频组件），这是新 Gate 的第一次注入式自证。
2. 在 arm64 macOS 上构建候选，跑探针、`thin-macos-libmpv.mjs` 与体积测量；候选不合格就停在候选目录。
3. `--write-vendor` 替换 17 个 dylib（或实际闭包），更新 manifest / NOTICE；按实际闭包同步 `tauri.conf.json`。
4. 独占运行 `pnpm verify:task -- 1.9 1.10 9.7`，跑 `reconcile-scenarios.mjs` 与 `gen-scenario-manifests.mjs` 漂移门，确认 564 基线不变。
5. 人工播放验收（六个 fixture + APE 样本），记录体积。
6. 同步 delta spec、归档 change，再创建关联 #138 的 PR。回滚按提交整体 revert。

## Open Questions

- 是否要在后续 change 中把 macOS deployment target 从已显式固定的 27.0 降到更低版本？本变更已用 `MACOSX_DEPLOYMENT_TARGET=27.0` 固定最低系统版本，发布产物 minos 精确为 27.0；再降低仍需单独评估 API 可用性与 CI 镜像（见 Risks / Trade-offs）。
- 是否值得从源码最小构建 libass / HarfBuzz 以去掉 glib / graphite2 / pcre2 / libintl / libpng / libunibreak（约 2.4 MB）？属于 proposal 列出的后续项。
- 是否能获得可再分发的 APE 样本或编码器，从而把格式矩阵纳入自动 smoke？在 fixture 政策允许之前保持人工验收。
