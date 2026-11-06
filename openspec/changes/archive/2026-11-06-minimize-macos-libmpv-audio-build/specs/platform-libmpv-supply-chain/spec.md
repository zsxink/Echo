# Spec Delta

## ADDED Requirements

### Requirement: macOS 播放后端可复现最小音频构建

macOS 端 vendored 的 libmpv 与 FFmpeg 库 MUST 由仓库内纳入版本控制的构建脚本产出；构建 MUST 固定上游源码版本与 SHA-256，记录完整 configure / meson 参数，并产出 arm64-only 二进制。构建 MUST 只包含 7 种保证格式（mp3、flac、ape、m4a、ogg、opus、wav）所需的 demuxer / decoder / parser，以及音效链所需的 `aformat`、`alimiter`、`equalizer`、`extrastereo`、`volume` 五个音频滤镜与 mpv 音效图格式协商必需的 `aresample`；FFmpeg 无条件注册的 `buffer` / `buffersink` / `abuffer` / `abuffersink` 四个图端点 MAY 保留，其中 `buffer` / `buffersink` 是图端点而非视频处理滤镜。构建 MUST NOT 包含其它视频 decoder、视频处理 filter、视频容器 demuxer、encoder、muxer、网络协议或 TLS 后端。mpv 的硬依赖 libass 与 libswscale MAY 保留，MUST 在 manifest 与 NOTICE 中说明其为 mpv 构建的硬依赖，不属于可裁剪的视频组件。FFmpeg 6.0 在 ld64 下启用 AAC parser 时还要求 AC3 parser 参与链接（`aac_ac3_parser.o` 无条件引用仅在 `CONFIG_AC3_PARSER` 下定义的 `ff_ac3_find_syncword`）；该 parser 为纯音频且不引入额外格式，MAY 保留。辅助库沿用外部包管理器时 MUST 记录其确切版本。

#### Scenario: 固定源码构建可复现

- **WHEN** 维护者在干净工作目录执行仓库内的 macOS 构建脚本
- **THEN** 产出的播放后端 dylib 全部为 arm64-only，实际注册组件集与声明的音频组件集一致，且 manifest 记录 FFmpeg / mpv 的源码 pin 与完整 configure / meson 参数

#### Scenario: 视频组件被拒绝

- **WHEN** 重编译后的 `libavcodec`、`libavfilter` 或 `libavformat` 注册了任一视频 decoder、视频处理 filter 或视频容器 demuxer（如 h264、hevc、prores、scale、overlay、matroska、mpegts），或 `libmpv` 直接链接 OpenGL、AppKit、VideoToolbox、CoreVideo
- **THEN** 构建或发布 Gate 失败并指出具体组件与所在文件；FFmpeg 无条件注册的 `buffer` / `buffersink` / `abuffer` / `abuffersink` 四个图端点不属于此列，产物不得进入发布

#### Scenario: 保证音频矩阵完整

- **WHEN** 对产出二进制枚举注册组件
- **THEN** mp3、flac、ape、aac、alac、vorbis、opus 与 PCM 家族 decoder，以及 mp3、flac、ape、mov、ogg、wav demuxer 均存在；`aformat`、`alimiter`、`equalizer`、`extrastereo`、`volume` 五个音效过滤器、格式协商必需的 `aresample` 与 `buffer` / `buffersink` / `abuffer` / `abuffersink` 四个图端点均存在

#### Scenario: 网络协议与 TLS 后端关闭

- **WHEN** 检查产出二进制的注册协议与动态依赖
- **THEN** 不存在 http、https、tcp、tls 等网络协议，也不存在 mbedTLS、OpenSSL、GnuTLS 等 TLS 后端依赖；本地播放只依赖 file 协议

#### Scenario: mpv 硬依赖的保留可审计

- **WHEN** 维护者检查 manifest、NOTICE 与 `libmpv.dylib` 的依赖闭包
- **THEN** libass 与 libswscale 的存在被明确记录为 mpv 0.36 的构建硬依赖；若它们从依赖闭包中消失，构建必须失败，除非另有规格变更显式调整 mpv 依赖策略

#### Scenario: 构建记录与实际二进制一致

- **WHEN** 检查 manifest 中记录的 FFmpeg / mpv 版本、configure / meson 参数与辅助库版本
- **THEN** 记录与实际 vendor 二进制内嵌的配置字符串及 `otool -L` 依赖一致；任何漂移使发布 Gate 失败

### Requirement: 构建组件集的回归门禁

macOS 发布 Gate MUST 通过可执行的运行时组件探针（或等价的组件注册枚举）读取实际二进制的 decoder / filter / demuxer / protocol 集合，MUST NOT 仅依赖 `strings` 或 `nm` 的字面抽查。Gate MUST 拒绝视频组件、网络协议与 TLS 后端，MUST 把 FFmpeg 无条件注册的 `buffer` / `buffersink` / `abuffer` / `abuffersink` 四个图端点列为基础设施白名单，MUST 断言 7 种保证格式、5 个音效滤镜与 `aresample` 存在，MUST 断言内嵌的 `avcodec_configuration()` 含 `--disable-everything`，并 MUST 拒绝 `libmpv` 直接链接 OpenGL、AppKit、VideoToolbox 或 CoreVideo。新增断言 MUST 带注入式自证：注入一个被禁止的组件、移除一个必需组件或让探针零匹配时，检查 MUST 失败并指出原因。

#### Scenario: 视频组件回归

- **WHEN** 未来的重编译重新启用 h264、scale、matroska 等任一视频组件
- **THEN** Gate 失败并指出被禁止的组件，而不是仅报告文件存在

#### Scenario: 必需音频组件缺失

- **WHEN** ape demuxer / decoder、任一音效滤镜或 `aresample` 未注册
- **THEN** Gate 失败并指出缺失的组件

#### Scenario: 探针未实际执行

- **WHEN** 组件探针返回零个组件、无法加载目标 dylib 或输出无法解析
- **THEN** Gate 失败，不得以空输出静默通过

#### Scenario: 注入式自证

- **WHEN** 注入证明套件构造一个包含被禁止组件的探针输出，或一个缺少必需组件的探针输出
- **THEN** 对应的 Gate 断言失败并给出预期错误，证明该检查能拒绝违规输入
