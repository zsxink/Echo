# 原生音效 Gate 记录

状态：**未通过 / 发布阻断**。用户已授权先实现功能，并同意补齐分发库能力；这不等于平台测量通过。这里记录可复现实验和已取得证据，缺失平台或硬件测量不能用 mock、静态版本号或 `ao=null` 替代。

## 实验素材与命令

素材由 `scripts/audio-effects/generate_fixtures.py` 离线生成；仅含确定性合成音（997 Hz、扫频、固定种子噪声、脉冲及声道关系），CC0-1.0，不含第三方录音。生成的 `manifest.json` 为每段素材记录 PCM 布局、量化后 sample-peak、SHA-256 和生成器 SHA-256。重复性及素材矩阵由 `python3 -m unittest discover -s scripts/audio-effects -p 'test_*.py' -v` 检查。运行示例及滤镜候选、客户端命令和结果解释见 `scripts/audio-effects/README.md`。

```sh
python3 scripts/audio-effects/generate_fixtures.py --output /tmp/echo-audio-effects-fixtures --duration 2
python3 scripts/audio-effects/probe.py \
  --libmpv /Applications/Echo.app/Contents/Frameworks/libmpv.dylib \
  --input /tmp/echo-audio-effects-fixtures/tone997-48000-2ch-60db.wav \
  --fixture-manifest /tmp/echo-audio-effects-fixtures/manifest.json \
  --manifest apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json \
  --output /tmp/echo-macos-probe.json --timeout 30
```

探针只加载指定候选包中的绝对路径，macOS 使用 `dlopen(RTLD_LAZY | RTLD_LOCAL)`；没有系统库 fallback。`ao=null` 只核对运行库、命令、属性和日志。`ao=pcm --capture-only` 可将一个固定 EQ 链的实际输出写入 WAVE 并测量 sample-peak 与低电平正弦响应；它在 CoreAudio 之前，不能验收设备时延、underrun、CPU 或听感。

```sh
python3 scripts/audio-effects/probe.py \
  --libmpv target/release/bundle/macos/Echo.app/Contents/Frameworks/libmpv.dylib \
  --input /tmp/echo-audio-effects-fixtures/tone997-48000-2ch-60db.wav \
  --fixture-manifest /tmp/echo-audio-effects-fixtures/manifest.json \
  --manifest apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json \
  --output openspec/changes/introduce-audio-effects-equalizer/evidence/macos-pcm-response-probe.json \
  --timeout 30 --ao pcm --pcm-output /tmp/echo-macos-eq6.wav \
  --capture-only --capture-gain-db 6
```

## 库一致性硬前置（2026-10-03 复核新增）

**2026-10-03 更新：已将签名有效的候选包安装到 `/Applications/Echo.app`；旧包保留在 `/Applications/Echo.app.review-backup-20261003` 以便回滚。安装后再次核对哈希与滤镜结构。**

| 位置 | libavfilter SHA-256 | `aformat`/`volume`/`alimiter`/`extrastereo` 命中 |
|---|---|---|
| `apps/desktop/src-tauri/vendor/libmpv/macos` | `2591332d9a3ccc0f6a7313b98fc2566ef4193dd40271a6908dec7243085df6bc` | 各 1 |
| `/Applications/Echo.app/Contents/Frameworks`（更新后） | `2591332d9a3ccc0f6a7313b98fc2566ef4193dd40271a6908dec7243085df6bc` | 各 1 |

```sh
python3 scripts/audio-effects/inspect-filter-runtime.py
python3 scripts/audio-effects/inspect-filter-runtime.py \
  --library /Applications/Echo.app/Contents/Frameworks/libavfilter.dylib
shasum -a 256 apps/desktop/src-tauri/vendor/libmpv/macos/libavfilter.dylib
shasum -a 256 /Applications/Echo.app/Contents/Frameworks/libavfilter.dylib
```

`inspect-filter-runtime.py` 直接读 `AVFilter.process_command`。**它同时是「随包库是否支持运行时改参数」的判据**：某滤镜存在但 `process_command` 为空时，`af-command` 会被 `avfilter_graph_send_command` 以 `AVERROR(ENOSYS)` 拒绝，而 mpv 的 `f_lavfi.c` 只判 `return result >= 0`，因而**报成功但参数未变**。当前随包库的 `extrastereo` 即属此情况（见下）。

## 已取得证据

| 平台/候选 | 版本与识别 | 结果 |
| --- | --- | --- |
| macOS 本机候选 vendor 包 | macOS 27.0 arm64；包内 libmpv SHA-256 记录于报告；mpv 0.36.0 + FFmpeg 6.0，FFmpeg 6.0 源码 SHA-256 `57be87c22d9b49c112b6d24bc67d42508660e6b718b3db89c44e47e289137082`；完整探针见 `evidence/macos-probe.json` | 五个所需 filters 均可用；十段 EQ、分段 EQ、双声道空间候选链建立成功；`eq5:gain`、`preamp:volume`、`spatial:m` 定向命令返回成功。使用 `ao=coreaudio` 的音频输出为 48 kHz / 2ch stereo / s32。仅输出到 MacBook Pro Speakers，播放合成 -60 dBFS 测试音并将 mpv 音量限制为 1%；没有 Loopback 捕获，故响应、峰值、真峰值和听感均未验证。Gate 未通过。 |
| macOS 重新打包的 `Echo.app` | 候选 bundle 位于 `target/release/bundle/macos/Echo.app`；ad-hoc 签名通过 `codesign --verify --deep --strict`；包内 libmpv SHA-256 `164430192f73da459916e384a405685697387fa5e333c19ffb1be4297a4852f3`；旧 app-bundle 探针见 `evidence/macos-audio-app-bundle-probe.json` | 探针实际加载 app 内 dylib，识别 mpv 0.36.0 / FFmpeg 6.0，映射的 libmpv/FFmpeg dylib 均来自 bundle；CoreAudio 协商 48 kHz / 2ch，候选链加载及 loadfile 后配置链保留成功。旧报告的 `af-command` rc=0 曾被误写作实际生效；R1 已更正：`extrastereo` 无运行时命令回调。此报告没有 PCM 响应、时延、underrun、CPU 或听感，`gate_passed=false`。 |
| macOS 候选 bundle PCM | `target/release/bundle/macos/Echo.app`；probe report `evidence/macos-pcm-response-probe.json`；CC0 997 Hz / -60 dBFS / 48 kHz / stereo 输入 SHA-256 `81d179034a32a2bc4cfbd1c20e492d036828e1c34fb5c3d7ba51b772b567672a` | 通过候选 bundle `ao=pcm` 捕获 96,000 帧、48 kHz、2ch、s16 输出；1 kHz EQ +6 dB 在 997 Hz 实测 `+5.9959 dB`，误差 `-0.0041 dB`；输出 sample-peak `-54.0507 dBFS`。所有 bundle dylib 哈希匹配 vendor manifest，运行时映射均来自候选 bundle。此单点响应不代表整条频响曲线或压力素材门槛；true-peak 未测，`gate_passed=false`。 |
| macOS 候选 bundle limiter 压力点 | `evidence/macos-pcm-limiter-stress-probe.json`；CC0 997 Hz / -0.1 dBFS / 48 kHz / stereo 输入 | 单段 1 kHz/+12 dB、Auto preamp -11.5 dB（对应 `G=12 dB`、`safe+0.5 dB`）经过候选包实际滤镜输出；捕获 sample-peak `-0.99985 dBFS`，s16 integer PCM。此单音压力点不能替代扫频、宽带、脉冲、双声道反相及不同采样率矩阵；true-peak/听感未测，`gate_passed=false`。 |
| macOS 候选 bundle CoreAudio | probe report `evidence/macos-audio-candidate-coreaudio-probe.json`；同一个候选 bundle 与 -60 dBFS fixture | 真实 CoreAudio 协商 48 kHz / 2ch / s32；十段 EQ、空间链、loadfile 链保留均成功；运行时 `eq5:gain`、preamp 具名命令调用返回 rc=0。rc=0 只说明调用被接收，不证明音频参数产生可测变化；`spatial:m` 更由结构体检查证实不受支持。测试音 mpv 音量限制为1%；`gate_passed=false`。 |
| Windows | 有固定 vendor DLL；未获得候选安装包、参考机或捕获设备 | 未测 |
| `/Applications/Echo.app` 已安装候选 | `evidence/macos-installed-app-probe.json`；app 内 libavfilter SHA-256 与 vendor manifest 一致 | app 内 libmpv 探针成功初始化、建立包含十段 EQ 和 limiter 的链、更新 EQ/preamp 命令并在 `loadfile` 后保持配置链；仅命令/快照证据，未证明目标参数的 PCM 响应。`gate_passed=false`。 |
| `/Applications/Echo.app` CoreAudio 与 mono | `evidence/macos-installed-app-coreaudio-probe.json`、`evidence/macos-installed-app-mono-probe.json` | CoreAudio 协商 48 kHz / stereo / s32；EQ 与 preamp 命令均返回 rc=0，十段链和 loadfile 后配置链存活。Mono 输入输出仍为 mono，探针跳过空间链。定向命令 rc=0 与 `process_command` 检查只证明可调用，不冒充参数 PCM 响应或设备听感。 |
| 2026-10-03 当前安装 app 复核 | `evidence/macos-installed-app-coreaudio-recheck-20261003.json` | 直接从 `/Applications/Echo.app/Contents/Frameworks/libmpv.dylib` 加载，CoreAudio 输出为 48 kHz / stereo / s32，mpv volume 为 1（mpv 百分制，即 1%）、mute=false；app 内 libmpv 与 vendor manifest SHA-256 一致。该探针仍是低音量合成音和配置/命令证据，不测输出响应或实际过渡时延。 |
| 2026-10-03 PCM 多采样率响应/压力矩阵 | `evidence/macos-pcm-matrix-20261003/summary.json` 与同目录 24 份原始 JSON；使用当前安装 app 的 libmpv、CC0 fixture manifest、`ao=pcm` | MacBook Pro (Mac17,2, Apple M5，10核，32 GB，内置 SSD，macOS 27.0 arm64；满足参考机规格下限)。22.05/44.1/48/96 kHz 下分别捕获 mono 与 stereo 的 997 Hz/-60 dBFS 输入经单个 1 kHz/+6 dB EQ 的输出；四个 stereo 响应误差为 -0.00422 至 -0.00411 dB，mono 也保持输入布局。每个采样率还覆盖 stereo 的 -0.1 dBFS 噪声、扫频、脉冲和反相素材，经过单个 1 kHz/+12 dB EQ、-11.5 dB preamp 与 limiter；24/24 报告为 probe-only，输出均为有限 integer PCM，最大样本峰值 -0.99985 dBFS。噪声/脉冲峰值更低属于素材频谱及 limiter 时间响应的测量结果，不推断其响度。仅证明这些固定链的离线 PCM 点测；未覆盖全十段曲线、空间链、CoreAudio 环回、100 次实时过渡、underrun、进度偏移、CPU 或听感，`gate_passed=false`。 |
| Linux | 当前仓库无 `vendor/libmpv/linux` 候选目录/安装包或参考机 | 未测 |

## Windows/Linux 开发侧复核（2026-10-03）

Windows 的发行库已固定为 shinchiro `20260814` 的 x86-64 `libmpv-2.dll`
（mpv `20260814 git-7b8915bc1d`、ABI 2.5；SHA-256
`f709c7ca8b183bec76b8158bf0c45c53018c63366750729352612f228ff7bdea`，与
`vendor/libmpv/windows/manifest.json` 一致）。macOS 主机上的 PE 架构/清单核对、
`cargo check -p echo-desktop --target x86_64-pc-windows-gnu`、平台 Gate 检查
`node scripts/verify/checks/task-9.7.mjs` / `task-9.8.mjs` 均通过；9.8 在此
主机只执行 macOS 分支。Windows 平台 Gate 测试可交叉编译，不能在 macOS 上运行，
也不能证明 DLL 内的 FFmpeg 滤镜与定向运行时命令行为。

Linux 的供应链脚本、22.04 构建容器引导和 provenance 自测均通过：
`node scripts/release/build-linux-libmpv.test.mjs`、
`node scripts/release/build-linux-libmpv-container.test.mjs`。当前 checkout 未包含
Linux manifest/候选 ELF；主机既没有 Docker/Podman/Lima，也没有 Linux C 工具链/系统
sysroot。`cargo check -p echo-desktop --target x86_64-unknown-linux-gnu` 因缺少
`x86_64-linux-gnu-gcc` 失败，因此不能据此宣称 Linux 编译或原生 Gate 通过。

音频工具离线自测通过（10 项），macOS 本机 `cargo test -p echo-desktop` 通过
（368 个单元测试及集成测试）。2026-10-03 又直接探测当前 `/Applications/Echo.app`
并完成 24 项四采样率 PCM 响应/压力矩阵（明细见上表）；`ao=pcm` 不经过 CoreAudio，
不能替代设备连续性、延迟、CPU 与听感验收。以上是共享实现、macOS 候选包装载与离线
实验的证据，不是 Windows/Linux 运行时滤镜链、PCM 响应或硬件验收；tasks 7.1–7.3
仍待候选 Linux 运行库及各平台机器/捕获设备完成后更新。

CoreAudio 能力探针的输出协商与候选包哈希记录见其原始报告；PCM 报告另外证明了固定 +6 dB 1 kHz EQ 的一项真实处理响应，并在近满幅单音上测得 sample-peak `-0.99985 dBFS`。`/Applications/Echo.app` 已更新为 vendor manifest 对应的候选包，旧包保留在备份目录。报告的 `gate_passed` 有意保持 `false`：这些点测不等于完整滤镜/采样率/压力验收，也不证明运行时更新的实际响应、平滑、设备时延、CPU、underrun 或听感。

## 复核修正：「命令返回成功」不等于「参数已改变」（2026-10-03）

上表「`eq5:gain`、`preamp:volume`、`spatial:m` 定向命令返回成功」的表述**过强**，按 `review-and-tuning.md` 的 R1 修正如下。

| 命令 | rc | 实际能力 | 判据 |
|---|---|---|---|
| `af-command <label> eq5:gain` | 0 | ✅ 真的改增益 | `equalizer` 有 `process_command`（`af_biquads.c` 2017 起支持 `gain`/`frequency`/`width`/`width_type`/`mix`/`bypass`） |
| `af-command <label> preamp:volume` | 0 | ✅ 真的改增益 | `volume` 有 `process_command` |
| `af-command <label> spatial:m` | 0 | ❌ **静默无效** | `extrastereo`（`af_stereotools.c`）**没有** `process_command` |

`macos-audio-app-bundle-probe.json` 中 `spatial:m` 命令 rc=0、`error=null`，且其后 26 条日志无任何错误或滤镜重配置记录（对比 `af set` 换链时有完整的 `Setting option 'graph'` 与 `lavfi (echo_spatial)` 重初始化序列）。**该「成功」是 mpv 包装层掩盖了 `AVERROR(ENOSYS)`，不构成生效证据。**

因此：**空间感 `width` 目前只能在安装时生效，运行时调整无效。** 任务 4.5 采用方案 A：`native_effects.rs` 在安装参数中写入 width，不再发送 `spatial:m`，Rust 回归测试断言命令不会发出。设备空间/折叠听感仍须 Gate 评估。

PCM 输出无需在 Python 侧自行搭 FFmpeg graph：随包 libmpv 的 `ao=pcm` 已能从候选包播放路径捕获滤镜后 WAVE。早期探针无法分析 WAVE_FORMAT_EXTENSIBLE，现已支持其 integer PCM 子格式并有回归测试；`capture-only` 将一个已知静态增益链独立捕获，避免与前序探针阶段混合。该捕获仍不能代替 CoreAudio 环回、underrun/延迟/CPU 或人工 A/B。任务 1.1 的可复现 PCM probe 已建立；1.3/1.4 的输出环境门槛继续待测。

## macOS 分发候选排查

上游将 `audio-full` 描述为 LGPL / 可商用；GPL 的是单独的 `encodersgpl` flavor，因此许可边界本身可接受（见 [上游 build/variant 文档](https://github.com/media-kit/libmpv-darwin-build#commercial-use)）。v0.7.2 预编译 flavor 对完整 EQ 链缺少所需 filters，因此没有继续作为 P0 包。候选完整下载包 SHA-256 为 `f65e00d8e5f6ae6668b12d8c9c63fa726b0b3a28e3b07cd3e29e1db620d32fa9`，探针结果见 `evidence/macos-audio-full-candidate-probe.json`。这两个预编译 flavor 不适合作为 P0 替代包，仍需用上游 Nix build 配方增加所需 LGPL filters 后构建自己的固定产物；macOS arm64 本机已完成固定源码的候选构建并接入 vendor；原有 universal dylib 的 x86_64 slices 保留，未在 Intel 机器上验证。

macOS 探针的 fixture 是 48 kHz、双声道、2 秒 PCM32LE 低电平正弦，文件 SHA-256 `81d179034a32a2bc4cfbd1c20e492d036828e1c34fb5c3d7ba51b772b567672a`；它不是完整 80 文件矩阵上的音频验收。报告 `gate_passed=false`。

## 分发构建修订与剩余工作

macOS vendor 包包含 FFmpeg 6.0 / mpv 0.36.0 本机 arm64 构建，FFmpeg 只打开 LGPL filters，关闭 GPL 和 nonfree；完整文件哈希、各 dylib 架构、许可证及构建来源记录在 `apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json` 与 `NOTICE.md`。mpv 的 `af-command` 目标滤镜补丁位于 `scripts/release/patches/mpv-lavfi-target-command.patch`，Cocoa Objective-C 构建补丁一并固定。当前 Intel slices 沿用既有产物；macOS Gate 仍需在目标参考设备执行 PCM 环回捕获、filter loadfile/参数存活实测、100 次转换时延/underrun、CPU 和盲听记录。

Linux 的固定构建脚本已配置同一组最小 LGPL FFmpeg filters；当前 checkout 没有 Linux
候选 ELF，macOS 主机也没有 Docker/Podman/Lima 可运行 Ubuntu 22.04 构建容器。Windows
vendor DLL 已固定，但 Windows/Linux 原生探针与音效 Gate 仍需对应系统实际运行。

macOS 的任务 1.2–1.5 和发布验收在 PCM/实时捕获与全部门槛完成前保持未勾选。当前机器有 CoreAudio 输出和内建麦克风，但没有虚拟 Loopback 捕获设备。需要为输出接入受支持的捕获链路后，按 `tasks.md` 记录完整响应、峰值、平滑、进度偏移、CPU 与 BS.1770-5 听感数据。
运行时定向命令现在按 libmpv `command-list` 实际签名选择：3 参数使用 macOS
0.36 补丁格式 `<target>:<option>`，4 参数使用上游格式 `option value target`；
未知签名关闭原生效果请求，不尝试有误伤风险的盲目重试。Windows/Linux 平台 Gate
也断言候选库暴露独立 target 参数。mpv 在 v0.37 引入该上游参数，见 [mpv 命令参考](https://github.com/mpv-player/mpv/blob/master/DOCS/man/input.rst)。
更新后的探针已针对 macOS app bundle 实际运行：报告
`evidence/macos-af-command-compat-probe.json` 读出 3 参数签名，EQ/preamp
命令各返回 0；这验证了兼容路径及参数形状，仍不是实际增益变化的证据。
