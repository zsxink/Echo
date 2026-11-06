## Purpose

定义 Echo 三平台桌面应用的第三方播放后端（libmpv 及关联 FFmpeg 库）供应链契约：每个平台固定来源与版本、校验完整性、随安装包打包、运行时加载匹配，以及规范的第三方许可证说明，使 macOS、Windows、Linux 都以可审计、可复核的方式接入同一播放后端。

## Requirements

### Requirement: 三平台 libmpv 供应链固定来源

系统 MUST 为 macOS、Windows、Linux 各自固定一组声明了确切来源与版本的 libmpv 及关联 FFmpeg 库，作为该平台打包进安装包的播放后端基线，并有单一 manifest 记录三平台的来源、版本与校验值。Windows 基线取自 shinchiro `mpv-dev-*.7z` 的选定发行（固定版本号与下载地址，不追最新）；Linux 基线的首个发行由仓库 CI 依据官方 mpv 源码按 `--minimal` 配置构建，并记录最低 glibc 依赖要求；macOS 维持现有 media-kit 构建产物约定。任一平台库的版本、来源或校验值发生变化 MUST 重新生成 manifest 并经平台校验后才可进入发布。

#### Scenario: 校验清单可审计

- **WHEN** 维护者检查任一平台的 vendor 记录
- **THEN** 能获得该平台所用库的固定来源、版本与下载地址，且与单一 manifest 记录一致

#### Scenario: 来源变更需重新校验

- **WHEN** 任一平台库的版本、来源或校验值发生变化
- **THEN** 系统要求重新生成 manifest 并完成对该平台的校验后，新基线才可进入发布

### Requirement: 库完整性校验与 ABI 契约

系统 MUST 在打包与加载两个环节校验各平台 libmpv 库的完整性：打包产物内的库文件 MUST 与 manifest 声明的 SHA-256 一致，manifest 亦记录该版本库的 libmpv 客户端 API 版本；运行时加载 MUST 校验动态库的 ABI 与客户端一致，加载失败或 ABI 不符 MAYST 以明确错误终止播放后端初始化，不得让用户在缺少或错配播放后端的安装包上遭遇不可解释的启动闪退。

#### Scenario: 清单校验通过

- **WHEN** 构建时校验某平台安装包内库文件的 SHA-256 与 manifest 一致
- **THEN** 打包流程继续，产物进入发布候选

#### Scenario: ABI 不符拒绝加载

- **WHEN** 运行的平台加载到 ABI 与客户端不一致的 libmpv
- **THEN** 系统记录明确错误并终止播放后端初始化，不以未定义状态或闪退呈现给用户

#### Scenario: 校验不一致导致构建失败

- **WHEN** 某平台安装包内库文件与 manifest 校验值不一致或缺失
- **THEN** 该平台构建失败，不得静默产出缺库或错库的安装包

### Requirement: 三平台打包与加载契约

系统 MUST 将各平台的 libmpv 及关联库随对应安装包分发（macOS 应用包内、Windows NSIS 安装目录、Linux 的 AppImage/deb 安装目录），并保证运行时桌面应用能从安装包内解析本平台所需的库，无需用户另行安装。某平台的库缺失或解析失败 MUST 使该平台的启动路径返回明确可读的错误，不得以静默闪退呈现。

每个平台的安装包 MUST 仅携带该平台运行所需的播放后端二进制，MUST NOT 携带其它平台的播放后端二进制或其它平台专属的库依赖文件。该约束 MUST 对全部三个平台的正式构建产物成立，包括在本平台配置中以平台无关方式声明资源的场景。

#### Scenario: 库随安装包提供

- **WHEN** 安装包在对应平台完成安装
- **THEN** 桌面应用包含该平台所需 libmpv 及关联库，未要求用户额外安装任何组件

#### Scenario: 缺失库给出明确错误

- **WHEN** 某平台安装包未包含该平台运行所需的库
- **THEN** 应用启动时给出明确可读的错误信息（可追溯到日志），而非无提示闪退

#### Scenario: 产物不含其它平台的播放后端

- **WHEN** 检查任一平台的正式构建产物所携带的播放后端二进制
- **THEN** 只存在该平台运行所需的库，不含其它平台的播放后端二进制，产物体积不因其它平台的库而增大

#### Scenario: 平台专属资源不外溢到其它平台

- **WHEN** 某平台的库以平台专属方式声明为打包资源
- **THEN** 其它平台的构建不把该平台的资源纳入自身产物，该平台的构建仍完整获得其所需资源

### Requirement: macOS 播放后端架构纯度

系统 MUST 保证 macOS 端 vendored 的 libmpv 及其关联库只包含当前发布目标架构（现为 arm64），MUST NOT 随发布产物分发包含 x86_64 slice 的 universal/fat 二进制。每个文件的 Mach-O 架构集合 MUST 与发布目标架构集合（`architectures`）精确相等，manifest 中记录的 `fileArchitectures` MUST 与该文件实际 Mach-O 架构精确相等；manifest 的 SHA-256 MUST 覆盖实际分发的二进制内容；NOTICE 与实际架构构成 MUST 一致，不得保留已移除架构的说明。

重新构建或更新 macOS 播放后端基线时 MUST 以目标架构-only 产出，不得依赖事后裁剪来掩盖构建配置；无论采用何种方式，最终进入 vendor 的二进制都 MUST 满足上述架构纯度约束。

#### Scenario: 架构集合精确匹配

- **WHEN** Gate 检查 macOS vendor 目录或正式 app bundle 中的每个播放后端库
- **THEN** 该文件的实际架构集合既与发布目标 `architectures` 精确相等，也与 `fileArchitectures` 记录精确相等；任何多余架构、缺少声明架构或两者不一致都使检查失败

#### Scenario: fat 产物被拒绝

- **WHEN** 任一 vendored 或打包后的 dylib 同时包含 arm64 与 x86_64
- **THEN** 校验失败并明确指出该文件，产物不得进入发布

#### Scenario: 哈希与二进制同步

- **WHEN** 播放后端二进制被替换或裁剪
- **THEN** manifest 中对应文件的 SHA-256 同步更新，且 vendor 校验与打包校验均通过

#### Scenario: 说明文件与架构构成一致

- **WHEN** 维护者或用户检查 manifest、NOTICE 与实际二进制
- **THEN** 三者对架构的描述一致，NOTICE 不再声明保留 x86_64 slice 或未通过 Gate 的 Intel 运行时

### Requirement: 第三方许可证说明

系统 MUST 为各平台 vendor 的 libmpv 及其关联库（含 FFmpeg 与其它依赖）提供统一格式的许可证说明文件，随对应平台安装包带入应用；Windows/Linux 引入的库来源新增的许可信息 MUST 合并进与既有 macOS 结构一致的 NOTICE 说明，单一 manifest 与 NOTICE 覆盖三平台。

manifest 与 NOTICE 中记录的每个组件许可 MUST 与该平台实际构建配置一致：构建启用了 GPL 选项或 GPL 组件（如 FFmpeg `--enable-gpl`、链接 x264/x265）时，对应组件 MUST 记录为 GPL-2.0-or-later，MUST NOT 记录为 LGPL。

各平台安装包 MUST 携带打包二进制所涉许可的完整文本（合并许可文本文件），且覆盖 manifest `licenses` 清单中出现的全部许可类型；每个平台的 NOTICE MUST 说明对应源码的获取方式（来源仓库与固定版本/提交）。

#### Scenario: 三平台均随包携带许可说明

- **WHEN** 任一平台的安装包被安装
- **THEN** 应用内包含该平台所 vendor 库的许可证说明，且结构与既有 macOS NOTICE 一致

#### Scenario: 新增来源纳入统一 NOTICE

- **WHEN** 新增某平台库来源或版本（如 Linux 自建产物引入的依赖）
- **THEN** 该来源的许可证信息并入统一 NOTICE，安装包随之更新

#### Scenario: 许可记录与构建配置一致

- **WHEN** 某平台构建配置使组件按 GPL 授权（如保留 `--enable-gpl` 或链接 GPL 库）
- **THEN** 该组件在 manifest 与 NOTICE 中记录为 GPL-2.0-or-later，与二进制的实际许可义务一致

#### Scenario: 许可全文随包可得

- **WHEN** 用户安装任一平台安装包并查找打包库的许可条款
- **THEN** 安装包内的合并许可文本包含该平台 manifest 所列全部许可类型的完整文本

#### Scenario: 对应源码可追溯

- **WHEN** 用户或审计者需要获取打包二进制的对应源码
- **THEN** 该平台 NOTICE 指明来源仓库与固定版本/提交，据此可重建或下载对应源码

### Requirement: macOS 播放后端符号与依赖最小集

系统 MUST 保证进入 macOS 发布产物的播放后端二进制不含成规模的本地符号：vendored 的每个 Mach-O 库在剥离后 MUST 仍通过代码签名校验与 Gate 的架构、校验和、ABI 检查，且 `nm -a` 中 MUST NOT 出现成批的本地符号（`t` / `s` 等小写类型）。主程序与 vendored 库的剥离 MUST NOT 改变其运行时加载关系或依赖关系。

系统 MUST 只分发被该平台播放后端依赖闭包实际引用的库。manifest 的 `files`、`fileArchitectures`、`licenses`、打包配置中的库映射、NOTICE 与合并许可文本 MUST 与磁盘上实际分发且被依赖闭包引用的组件集合一一对应：某个组件从分发中移除后，上述所有供应链记录与许可说明 MUST 同步移除其条目，MUST NOT 保留无对应二进制的库声明或许可条目。删除组件 MUST NOT 破坏其余组件的许可说明（与其它组件共用的许可段落 MUST 保留其余组件的覆盖范围）。

供应链校验 MUST 能拒绝回归：任一 vendored 库重新出现成规模本地符号，或供应链记录与磁盘/依赖闭包不一致时，校验 MUST 失败并明确指出该文件，产物不得进入发布。

#### Scenario: 本地符号被剥离且校验仍通过

- **WHEN** Gate 检查 strip 后的 macOS vendor 目录与正式 app bundle 中的播放后端二进制
- **THEN** `nm -a` 中不存在成规模的 `t` / `s` 本地符号，且 `codesign --verify --deep --strict`、架构集合精确匹配、manifest SHA-256 与 ABI 检查全部通过

#### Scenario: 本地符号回归被拒绝

- **WHEN** 任一 vendored dylib 被替换为含成规模本地符号的版本（或重新构建后未执行剥离）
- **THEN** 校验失败并明确指出该文件，产物不得进入发布

#### Scenario: 未引用组件被移除后记录一致

- **WHEN** 维护者检查已删除未引用库的 macOS vendor 目录、manifest、打包配置、NOTICE 与合并许可文本
- **THEN** 五处均不含该组件的条目，manifest 的库集合与磁盘文件、依赖闭包、打包映射一致，其余组件的许可说明完整保留

#### Scenario: 供应链记录漂移被拒绝

- **WHEN** manifest / 打包配置 / 许可说明仍声明某个已从磁盘移除的库，或仍为已剥离的二进制记录旧 SHA-256
- **THEN** vendor 校验或打包校验失败，明确指出不一致的条目，产物不得进入发布

#### Scenario: 剥离不改变依赖闭包

- **WHEN** 比较 strip 前后 `libmpv.dylib` 及其依赖库的 `otool -L` 引用
- **THEN** 依赖闭包内的库引用保持不变，运行时仍能解析全部所需库，被删除的组件原本不在闭包内且无任何二进制引用它

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
