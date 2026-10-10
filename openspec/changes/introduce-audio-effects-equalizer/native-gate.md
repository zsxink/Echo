# 原生音效 Gate 记录

状态：**未通过 / 发布阻断**。用户已授权先实现功能，并同意补齐分发库能力；这不等于平台测量通过。这里记录可复现实验和已取得证据，缺失平台或硬件测量不能用 mock、静态版本号或 `ao=null` 替代。

## 2026-10-09 DSP 专业复核（只读）：Gate 阻断点重排

复核方式：以 FFmpeg **n6.0 上游源码**（`af_biquads.c` / `af_alimiter.c` / `af_extrastereo.c` / `af_volume.c` / `avfiltergraph.c`）+ `math.rs` 同式独立复算，不引用本仓库既有结论。HEAD `b8711fe`（含 10-08 未提交改动）。

**能力面判断不变且已加强**：macOS 候选库的参数路径由 10-08 持续 PCM 复测证明可用。AFX-11.1 的静态响应采集已经完成并通过独立证据复核；limiter/首尾内容、实时过渡、设备性能及跨平台 Gate 仍不完整，细项状态见 `tasks.md`。本节所有运行报告仍保持 `gate_passed=false`。

| 阻断点 | 现有证据为何不足 |
|---|---|
| ≤0.25 dB 响应验收 | 完成四率低电平 stepped-sine 共 52 份报告、7,774 个频点比较；最大原生与复合 RBJ 差 0.000006809483 dB，保守舍入界 0.000134970 dB。报告和哈希/聚合由主 Agent 独立核验；仅覆盖静态低电平链 |
| `fc<=0.45*Fs` 用源率还是图实际率 | 四档采样率矩阵中 `audio-params` 与 `audio-out-params` 恒等，结构性无法暴露差异 |
| `AUTO_HEADROOM_DB=0.5 dB` | `evidence/` 内无任何 BS.1770 / 响度 / LU 字段，纯推算 |
| 脉冲/limiter 压力验收 | 四档脉冲首样本与无 limiter 线性模型相差 <0.00044 dB；既有整数 PCM 捕获不足以诊断 limiter 前后浮点行为，不能以输出峰值下限验收 |
| `latency=true` 的首尾内容 | trim/pad 源码不足以推出节目首尾内容丢失；须捕获首尾内容、帧数、对齐和 seek/EOF 行为 |

**本轮已修正的 DSP 与证据口径**（不改代码行为，只改错误的陈述）：
1. `extrastereo` 的 `c` **是内部削波开关（默认开），不是 center coefficient**（`af_extrastereo.c:37`）。`c=false` 关闭削波是正确决策；此前把它当作中心系数是误读。
2. `latency=true` 以 trim/pad 补偿 look-ahead：trim 裁切延迟输出，EOF 补入等量输入零样本以排空缓冲（`af_alimiter.c:303-317` / `:346-353`）。这一机制本身不能证明节目开头丢失或尾部被静音替换。
3. 四率脉冲首样本峰值与无 limiter 线性模型差小于 0.00044 dB；低峰可由 `b0` 与 preamp 的线性衰减解释，不能据此断言 attack 不足。
4. 「30 ms 连续过渡」实为**分轮参数插值**，不是逐样本包络；且 `config_filter(reset=0)` 保留 biquad 状态但仍非交叉淡化。
5. 「`spatial:m` 更由结构体检查证实不受支持」与同文件 10-08 实测矛盾，已删除。
6. `safe+0.5 dB` 的「未验证」限定原只写在第 4 节，现已在第 3 节表格处重复标注，防止被单独摘出引用。

**本节初稿已更正**：RBJ 系数三处自洽；`G` 用 `product(H_i)` 正确覆盖级联。复算的相邻 1/2 kHz 两段 +12 峰值为 **+14.564616 dB @约1025.35 Hz**，全十段 +12 为 **+18.402800 dB @约499.90 Hz**；旧 +0.607 dB 是扫描范围错误。`level=false` 必需；`asc=false` 关闭 `auto_release` 自动释放控制，并不关闭 true-peak 检测；`attack=5`/`release=50` 确为 FFmpeg 默认；`-0.99985 dBFS` 与 `limit` 自洽；`extrastereo` 的 M/S 语义与实现一致且中央信号不下沉；`fc<=0.45*Fs` 退化清单（22050 → 仅 16 kHz）；R10/R11 已真正闭合。完整清单见 `tasks.md` 第 11、12 节。

## 2026-10-09 逐频响应捕获（静态低电平范围已完成）

证据汇总：[macos-stepped-sine-20261009/summary.json](evidence/macos-stepped-sine-20261009/summary.json)。独立脚本 `scripts/audio-effects/sine_sweep.py` 对 9 个内置曲线及 4 个压力曲线、四种处理率共生成 52 份报告和 7,774 个频点比较；每率频点数为 146/150/151/151。观测范围至 `0.499 Fs`（可表示时额外包含 20 kHz），0.45 Fs 仍是 EQ 中心频率有效性边界。最大原生与复合 RBJ 响应差为 0.000006809483 dB，最大保守舍入界为 0.000134970 dB。主 Agent 独立核对了报告数、工具哈希、比例与聚合；工具测试 23 项通过。

此结果支持 macOS 静态低电平响应的 ≤0.25 dB 判据，不验证 limiter 高压力/浮点链、运行时过渡、设备端输出、听感或 Windows/Linux 能力；52 份报告均保持 `gate_passed=false`，其余发布 Gate 继续阻塞。

## 2026-10-08 运行时改参复测：已确认 macOS 参数路径

证据：[macos-runtime-parameter-recheck-20261008.json](evidence/macos-runtime-parameter-recheck-20261008.json)。主 Agent 独立重跑修复后的探针，并补测空间同相和单侧输入；libmpv SHA-256 为 `164430192f73da459916e384a405685697387fa5e333c19ffb1be4297a4852f3`，libavfilter 为 `2591332d9a3ccc0f6a7313b98fc2566ef4193dd40271a6908dec7243085df6bc`。

**AFX-9.5 根因已定位到测量时序**：`ao=pcm` 非实时高速解码，旧短素材在 `wait_audio()` 的 50 ms pump 内已经到 EOF；命令发生在样本处理完之后。即使 label 正确，命令也不能改变已写出的 PCM。新探针先在磁盘离线重复素材至 600 秒，实际产生基线 PCM 后且 EOF 前发送命令；每项均为一次不中断播放，命令前后不 pause/seek/loadfile 或重建链。它记录文件字节位置、EOF 状态及前后样本窗口，拒绝 EOF 后命令或缺少后续窗口的测量。字节位置是已写样本上界，**不能用来声称实时更新延迟通过**。

| 命令或对照 | 独立复测变化 dB | 理论 dB | 结论 |
|---|---:|---:|---|
| `af-command echo_eq5 eq5:gain 6` | +5.990866 | 约 +6 | EQ 运行时生效 |
| `af-command echo_preamp preamp:volume 0.501187233627272` | −6.042937 | −6 | preamp 运行时生效 |
| `af-command echo_spatial spatial:m 1`，反相 | −1.947385 | −1.938200 | 实际宽度由 1.25 回到 1 |
| 同一宽度命令，同相 | +0.004099 | 0 | 中央信号不因宽度改变而放大 |
| 同一宽度命令，单侧输入的第一声道 | −1.026806 | −1.023050 | 符合 M/S 矩阵 |
| 安装期 +6 dB 静态参考 | +5.995869 | 约 +6 | harness 自检通过 |

这些低电平 PCM16 测量与理论差均小于 0.25 dB，误差包括量化和窗口投影；不是整条频响在全部频率的误差验收。旧「全部运行时命令无效」结论不成立，旧正确 label 的不变捕获也不能证明改参失效。固定用户 width=1.25 不变，内部启用/旁路可使用已实证的 `spatial:m` 过渡。命令 rc=0 仍不单独构成证据。

复现：

```sh
python3 scripts/audio-effects/generate_fixtures.py --output /tmp/echo-runtime-fix-fixtures --duration 10
python3 scripts/audio-effects/probe.py \
  --libmpv apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib \
  --input /tmp/echo-runtime-fix-fixtures/tone997-48000-2ch-60db.wav \
  --fixture-manifest /tmp/echo-runtime-fix-fixtures/manifest.json \
  --manifest apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json \
  --output /tmp/echo-runtime-recheck.json \
  --ao pcm --pcm-output /tmp/echo-runtime-static.wav --capture-only \
  --runtime-parameter-proof --runtime-capture-seconds 1 --step-timeout 10 --timeout 60
```

报告保持 `gate_passed=false`。本节仅解除 macOS 参数机制的阻塞；30 ms 过渡的真实瞬态、CoreAudio 设备捕获、完整扫频、100 次时延、性能、听感以及 Windows/Linux 实测仍需原 Gate。以下 2026-10-03/04 内容作为历史实验记录保留，当前结论以本节为准。

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

`inspect-filter-runtime.py` 直接读 `AVFilter.process_command`，并用导出的 `avfilter_filter_pad_count()` 交叉校验字段布局（pad 数不符或 `priv_size` 不合理时**拒绝输出逐滤镜结论**）。**该脚本只证明结构，不证明行为**：结构上有回调不等于命令会改变音频，唯一可接受的行为判据是 PCM 前后测量（见下）。

⚠️ **2026-10-03 布局更正**：脚本此前按 `name, description, priv_size, flags_internal, priv_class, inputs, nb_inputs, outputs, nb_outputs, process_command` 读取，而 FFmpeg 6.0 的真实顺序是 `name, description, inputs, outputs, priv_class, flags, nb_inputs(u8), nb_outputs(u8), formats_state, preinit, init, uninit, formats, priv_size, flags_internal, process_command, activate`（`priv_size` 在 offset 80，pad 计数为 `uint8`）。错位导致读出 `nb_inputs=65536`、荒谬 `priv_size`，并由此**错误地**把 `extrastereo` 判为无回调——该结论已作废。修正后：`equalizer`/`volume`/`alimiter`/`extrastereo` 均有 `process_command`，仅 `aformat` 无。

### 运行时改参的 PCM 判别性实测（2026-10-03）

固定 12 s 素材、固定 4 s 捕获窗（同一条链重复 5 次极差 0.0000 dB）：

| 链 | 命令 | 实测 | 结论 |
|---|---|---:|---|
| 安装期 `g=6`（静态） | 无 | **+5.9959 dB** | 逐位复现 `macos-pcm-response-probe.json` |
| 安装期 `g=0` | `af-command <label> eq5:gain 6`，**rc=0** | **+0.0076 dB**（= 基线） | **未生效** |
| 安装期 `m=1.25` | `af-command <label> spatial:m 1.4`，**rc=0** | **+0.0000 dB**（理论 +0.9844） | **未生效** |
| 安装期 `volume=1` | `af-command <label> preamp:volume 0.5012`，**rc=0** | **+0.0076 dB**（理论 −6） | **未生效** |

⇒ **2026-10-04 撤回本节「运行时改参一律无效」的结论**：下表实验使用了**错误的 mpv label**（把 `af` 属性显示的滤镜名当成 label，见下方「label 语义」），因此整条判别链无效，不能作为能力结论。**保留的有效事实只有两条**：① harness 正确（静态 g=0/+6 稳定测得 +0.0076 / +5.9959 dB）；② `af-command` 的 rc 不能作为生效证据，必须用 PCM 前后对比。

用**正确 label** 重做实验（2026-10-04）得到一个更精确、也更矛盾的观测：

| 命令形式 | rc | 含义 |
|---|---:|---|
| `eq5:gain` / `eq5:frequency` / `eq5:width` / `equalizer:gain` | **0** | 仅有 `AV_OPT_FLAG_RUNTIME_PARAM` 的选项返回 0 |
| `all:gain` / `eq5:mix` / `eq5:normalize` / `eq5:precision` / `eq5:bypass` | **-12** | 选项查找失败（`av_opt_find2` 找不到） |

该返回码分布与 `ff_filter_process_command` 的 `av_opt_find2(..., AV_OPT_FLAG_RUNTIME_PARAM | ...)` 选项查找路径一致，但它本身不能证明参数随后影响了正在输出的实例。当前这组正确 label 的 PCM 捕获与基线逐字节相同（sha256 一致），分段幅度在命令前后亦相同；`volume` 的捕获也未见输出变化。**根因仍未定位**（任务 AFX-9.5）：mpv `f_lavfi.c` 的 `lavfi_reset()`→`free_graph()` 是否在播放期间替换 `c->graph` 仍待运行时日志或实例身份证据验证。

⚠️ **mpv `af-command` 的 label 语义（2026-10-04 排障记录，本轮踩过的坑）**：
- `af add/set` 的 `@label:NAME` 中，`label` 是 `@` 与 `:` 之间那段，`NAME` 是**滤镜名**（`options/m_option.c:3177-3196`）。
- `af` 属性返回的 `name` 是**滤镜名而非 label**。因此 `@echo_x:lavfi=[…]` 在 `af` 里显示为 `lavfi` 属正常，**不能**据此推断 label 不可用。
- `af-command <label> …` 经 `find_by_label()` 在 user_filters 中匹配（`filters/f_output_chain.c:421-451`），找不到即 rc=-12。
- `target=all` 走广播分支（`f_lavfi.c:439`），**无条件返回 true**，是另一处假成功来源。

复现命令（新增判据，`runtime_parameter_pcm_proof` 检查项）：

```sh
python3 scripts/audio-effects/generate_fixtures.py --output /tmp/echo-fx-long --duration 12
python3 scripts/audio-effects/probe.py \
  --libmpv /Applications/Echo.app/Contents/Frameworks/libmpv.dylib \
  --input /tmp/echo-fx-long/tone997-48000-2ch-60db.wav \
  --fixture-manifest /tmp/echo-fx-long/manifest.json \
  --ao pcm --pcm-output /tmp/afx.wav --capture-only \
  --capture-gain-db 6 --runtime-parameter-proof \
  --runtime-capture-seconds 4 \
  --output /tmp/afx.json --timeout 120
```

通过阈值：`static_gain_reference_db`（安装期 g=6）≈ +6 dB 以验证 harness；`runtime_parameter_pcm_proof.runtime_m_confirmed` 为 `true` 才算运行时改参生效。当前实测 `static` +5.9959 dB（harness 正常）、`runtime_m_confirmed=false`（这次捕获未观察到改参效果；根因未明）。

⚠️ **上表实验的 label 有误，结论已撤回**（见下方「label 语义」与「2026-10-04 撤回」）。保留其 harness 自检价值：`static_gain_reference` 证明量具能测出静态 +5.9959 dB，因此「运行时无变化」不是量具问题——但由于 label 错误，该测量本身也不成立，须用正确 label 重做。

⚠️ **既有 +5.9959 dB 证据的作用域**：`macos-pcm-response-probe.json` 的链中 `eq5 g=6` 是**安装期写死**的静态链，该测量证明的是**静态链响应**（-54.0507 dBFS 峰值），**并未隔离出运行时命令的效果**。此前把它读作「运行时改参已验证」是过度解读。

⚠️ **`af` 属性无判别力**：mpv 的 `af` 只报告**已配置描述**，运行时改参不会改写它，故 graph 前后 diff 两种情况都表现为「不变」。另实测 `af set` 用 `@label:lavfi=[...]` 时 `af` 里 label 全为 `lavfi`，`af-command` 只接受真实存在的 label（其余 rc=-12）。

**测量方法学坑（会产出自信的错数）**：
① `pump` 到 EOF 会让分析窗依赖解码时序 ⇒ 同一条链两次跑出 +0.0076 / +1.6871 dB 的**矛盾**结果；必须用固定捕获窗（`--runtime-capture-seconds`）。
② 反相/低电平素材须**匹配输入采样率**且避开 limiter 饱和，否则两端同样削顶、看不出差别（首轮即因此得到假阴性）。
③ 要**逐位复现仓库既有证据**必须用 **2 s 素材 + `pump(duration+0.25)`**；换成长素材会得到 +17.6 dB 之类的假数。
④ 素材是 **32-bit** 而 `ao=pcm` 输出 **16-bit** ⇒ 必须按位深各自归一化，否则会把格式差读成约 **−96 dB** 的「响应误差」。
⑤ **harness 必须自检**：每份报告带 `static_gain_reference`（安装期已知增益），只有它 `harness_valid=true` 时，运行时改参的「无变化」结论才可信——否则「什么都没变」可能只是量具坏了。

### 扫频响应（design §3 的 0.25 dB 仍未验证）

新增 `probe.py --sweep-response` 测量安装期静态链的扫频响应，并**诚实标注其不足以判定容差**：

| 捕获窗 | 窗数 | 最大误差 | 说明 |
|---|---:|---:|---|
| 10 ms | 1198 | +10.2164 dB | 窗内跨越频率过宽 |
| 20 ms | 598 | +6.0990 dB | 仍受窗内频率斜率污染 |
| 40 ms | 298 | +5.9494 dB | 开始收敛 |
| 80 ms | 148 | **+5.9493 dB** | **收敛到安装增益 +6 dB** |

误差随窗长单调收敛到安装增益本身 ⇒ 残余起伏是**窗化伪影**而非滤波器偏差；但也说明**窗包络法无法分辨 0.25 dB**（1 kHz 附近单窗跨越数百 Hz，量的是窗内频率内容而非滤波器响应）。故该检查输出 `usable_for_tolerance_verdict: false`，**不得记为通过**。

要真正兑现 design §3 的 ≤0.25 dB，需**逐频正弦扫频**（每频点一次捕获，或 FFT 频率分辨率显著窄于滤波器带宽）。当前证据仍只有 997 Hz 单点。

## 已取得证据

| 平台/候选 | 版本与识别 | 结果 |
| --- | --- | --- |
| macOS 本机候选 vendor 包 | macOS 27.0 arm64；包内 libmpv SHA-256 记录于报告；mpv 0.36.0 + FFmpeg 6.0，FFmpeg 6.0 源码 SHA-256 `57be87c22d9b49c112b6d24bc67d42508660e6b718b3db89c44e47e289137082`；完整探针见 `evidence/macos-probe.json` | 五个所需 filters 均可用；十段 EQ、分段 EQ、双声道空间候选链建立成功；`eq5:gain`、`preamp:volume`、`spatial:m` 定向命令**被接受**。使用 `ao=coreaudio` 的音频输出为 48 kHz / 2ch stereo / s32。mpv 音量限制为 1%；没有 Loopback 捕获，故响应、峰值、真峰值和听感均未验证。⚠️ 该 JSON 的 `audio-device` 全程为 `auto`，原文「仅输出到 MacBook Pro Speakers」无证据支撑。Gate 未通过。 |
| macOS 重新打包的 `Echo.app` | 候选 bundle 位于 `target/release/bundle/macos/Echo.app`；包内 libmpv SHA-256 `164430192f73da459916e384a405685697387fa5e333c19ffb1be4297a4852f3`；旧 app-bundle 探针见 `evidence/macos-audio-app-bundle-probe.json` | 探针实际加载 app 内 dylib，识别 mpv 0.36.0 / FFmpeg 6.0，映射的 libmpv/FFmpeg dylib 均来自 bundle；CoreAudio 协商 48 kHz / 2ch，候选链加载及 loadfile 后配置链保留成功。⚠️ 原文「ad-hoc 签名通过 `codesign --verify --deep --strict`」在该 JSON 中**无任何字段支撑**（全文不含 `codesign`/`signature`），须补记录或删除。⚠️ 原文称 `spatial:m` 命令后「26 条日志」，实际为 **118** 条 events（内容支持「无错误」，计数错误）。旧报告的 `af-command` rc=0 曾被误写作实际生效；历史短输入探针曾得出「结构有回调但运行时改参实测不生效」，该结论已撤回：输入在命令前已 EOF，不能验证参数变化；以 AFX-9.5 的持续 PCM 对照为准。此报告没有 PCM 响应、时延、underrun、CPU 或听感，`gate_passed=false`。 |
| macOS 候选 bundle PCM | `target/release/bundle/macos/Echo.app`；probe report `evidence/macos-pcm-response-probe.json`；CC0 997 Hz / -60 dBFS / 48 kHz / stereo 输入 SHA-256 `81d179034a32a2bc4cfbd1c20e492d036828e1c34fb5c3d7ba51b772b567672a` | 通过候选 bundle `ao=pcm` 捕获 96,000 帧、48 kHz、2ch、s16 输出；**安装期即写死 `g=6` 的静态链**在 997 Hz 实测 `+5.9959 dB`，误差 `-0.0041 dB`；输出 sample-peak `-54.0507 dBFS`。⚠️ 该测量证明**静态链响应**，不证明运行时改参（后者实测无效，见上）。所有 bundle dylib 哈希匹配 vendor manifest，运行时映射均来自候选 bundle。此单点响应不代表整条频响曲线或压力素材门槛；true-peak 未测，`gate_passed=false`。 |
| macOS 候选 bundle limiter 压力点 | `evidence/macos-pcm-limiter-stress-probe.json`；CC0 997 Hz / -0.1 dBFS / 48 kHz / stereo 输入 | 单段 1 kHz/+12 dB、Auto preamp -11.5 dB（对应 `G=12 dB`、`safe+0.5 dB`）经过候选包实际滤镜输出；捕获 sample-peak `-0.99985 dBFS`，s16 integer PCM。此单音压力点不能替代扫频、宽带、脉冲、双声道反相及不同采样率矩阵；true-peak/听感未测，`gate_passed=false`。 |
| macOS 候选 bundle CoreAudio | probe report `evidence/macos-audio-candidate-coreaudio-probe.json`；同一个候选 bundle 与 -60 dBFS fixture | 真实 CoreAudio 协商 48 kHz / 2ch / s32；十段 EQ、空间链、loadfile 链保留均成功；运行时 `eq5:gain`、preamp 具名命令调用返回 rc=0。rc=0 只说明调用被接收，不证明音频参数产生可测变化；`spatial:m` 的 `process_command` **存在**（结构层，`inspect-filter-runtime.py` 按真实字段布局判定），其行为已由 2026-10-08 持续 PCM 复测证明（反相 m1.25→1 变化 −1.947 dB）。⚠️ 原文「`spatial:m` 更由结构体检查证实不受支持」与同日实测直接矛盾，已删除；结构层与行为层必须分别表述。测试音 mpv 音量限制为1%；`gate_passed=false`。 |
| Windows | 有固定 vendor DLL；未获得候选安装包、参考机或捕获设备 | 未测 |
| `/Applications/Echo.app` 已安装候选 | `evidence/macos-installed-app-probe.json`；app 内 libavfilter SHA-256 与 vendor manifest 一致 | app 内 libmpv 探针成功初始化、建立包含十段 EQ 和 limiter 的链、更新 EQ/preamp 命令并在 `loadfile` 后保持配置链；仅命令/快照证据，未证明目标参数的 PCM 响应。`gate_passed=false`。 |
| `/Applications/Echo.app` CoreAudio 与 mono | `evidence/macos-installed-app-coreaudio-probe.json`、`evidence/macos-installed-app-mono-probe.json` | CoreAudio 协商 48 kHz / stereo / s32；EQ 与 preamp 命令均返回 rc=0，十段链和 loadfile 后配置链存活。Mono 输入输出仍为 mono，探针跳过空间链。定向命令 rc=0 与 `process_command` 检查只证明可调用，不冒充参数 PCM 响应或设备听感。 |
| 2026-10-03 当前安装 app 复核 | `evidence/macos-installed-app-coreaudio-recheck-20261003.json` | 直接从 `/Applications/Echo.app/Contents/Frameworks/libmpv.dylib` 加载，CoreAudio 输出为 48 kHz / stereo / s32，mpv volume 为 1（mpv 百分制，即 1%）、mute=false；app 内 libmpv 与 vendor manifest SHA-256 一致。该探针仍是低音量合成音和配置/命令证据，不测输出响应或实际过渡时延。 |
| 2026-10-03 PCM 多采样率响应/压力矩阵 | `evidence/macos-pcm-matrix-20261003/summary.json` 与同目录 24 份原始 JSON；使用当前安装 app 的 libmpv、CC0 fixture manifest、`ao=pcm` | MacBook Pro (Mac17,2, Apple M5，10核，32 GB，内置 SSD，macOS 27.0 arm64)。22.05/44.1/48/96 kHz 下分别捕获 mono 与 stereo 的 997 Hz/-60 dBFS 输入经单个 1 kHz/+6 dB **静态** EQ 的输出；四个 stereo 响应误差为 -0.00422 至 -0.00411 dB（对 997 Hz 解析值 5.999529 而言真实误差约 -0.0037 dB；量化误差贡献未单独隔离）。每个采样率还覆盖 stereo 的 -0.1 dBFS 噪声、扫频、脉冲和反相素材，经单个 1 kHz/+12 dB **静态** EQ、-11.5 dB preamp 与 limiter；24/24 报告为 probe-only，输出均为有限 integer PCM，最大样本峰值 -0.99985 dBFS（与 limit 自洽：差 0.489 LSB 的取整残差，非超限）。⚠️ **覆盖的是四种「解码」采样率**：`ao=pcm` 不重采样，各档 `audio-params` 与 `audio-out-params` **完全相等**，故该矩阵**结构性无法暴露源 Fs ≠ 输出 Fs**。这直接削弱 `fc<=0.45*Fs` 边界的证据力：实现取 `audio-params/samplerate`（`actor_effects.rs:204`）并显式丢弃 `audio-out-params/samplerate`（:224），而 libmpv 的重采样可能发生在用户 `af` 链之前或之后——若滤镜图实际率等于输出率而判据用了解码率，22.05 kHz 源 + 48 kHz 设备这一组合会误弃 16 kHz 段。该项为**未验证的高风险**，须同时记录 `audio-params/samplerate`、`audio-out-params/samplerate` 与 `equalizer` 实例 link 的实际 `sample_rate`，模型 Fs 与 equalizer 实际 link Fs 相同才算通过；源率与 AO 率允许不同（AFX-11.2）。
（2026-10-03 历史矩阵补充）另 22.05 kHz 下仅 16 kHz 段退化（0.45Fs=9922.5），44.1/48/96 kHz 下十段全部有效。该历史整数 PCM 压力矩阵不含 limiter 前后浮点捕获，不能据其中脉冲低峰归因 attack。2026-10-09 复核发现四档脉冲首样本峰值与无 limiter 线性模型差均小于 0.00044 dB，低峰可由 `b0` 与 preamp 的线性衰减解释；不得把“脉冲低约 9.5 dB”作为当前机制结论。空间反相最坏 M/S 放大与 EQ 压力应作为各自独立场景评估，不将互斥模式串接。未覆盖全十段曲线、空间链、CoreAudio 环回、100 次实时过渡、underrun、进度偏移、CPU 或听感，`gate_passed=false`。
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
| `af-command <label> eq5:gain` | 0 | ⚠️ **rc 不可信** | 仅 `RUNTIME_PARAM` 选项返回 0（`mix`/`normalize` 等返回 -12），说明命令到达滤镜并通过选项查找；**但音频未变**，根因未定位（AFX-9.5） |
| `af-command <label> preamp:volume` | 0 | ⚠️ **rc 不可信** | 同上；`volume` 的 `process_command` 无重配置步骤仍无效 |
| `af-command <label> spatial:m` | 0 | ⚠️ **rc 不可信** | 同上 |

**结论修正（2026-10-04，替代 10-03 版本）**：
① 10-03 的表述「`extrastereo` 没有 `process_command`，故被 `AVERROR(ENOSYS)` 拒绝而 mpv 掩盖之」**错误**：回调**存在**（布局更正后已证实）。
② 10-03 的替代结论「运行时改参一律无效」**也已撤回**：其判别实验使用了错误的 mpv label。
③ **当前唯一确定的事实**：`af-command` 的 rc 不能证明参数改变（mpv 只判 `result >= 0`，且 `target=all` 无条件返回 true）；判断是否生效必须用 PCM 前后对比。**当时根因仍未定位；macOS 后由 AFX-9.5 确认为短素材在命令前到达 EOF。**

**历史结论（2026-10-03，已于 10-04 撤回，保留供追溯）**：当时表述「`extrastereo` 没有 `process_command`，故 `spatial:m` 被 `AVERROR(ENOSYS)` 拒绝而 mpv 掩盖之」**双重错误**：① 回调**存在**（布局更正后已证实）；② 当时据以判断的 PCM 实验使用了错误的 mpv label，故「命令不改变音频」也未成立。

`macos-audio-app-bundle-probe.json` 中 `spatial:m` 命令 rc=0、`error=null`（原文称其后 26 条日志无错误，实际为 118 条 events）。**无论根因为何，rc=0 都不构成生效证据。**

（历史实现状态，已由 AFX-10.3 替代）旧版曾将空间感 `width` 按安装期常量实现并不发送 `spatial:m`。当前用户 width 固定 1.25，内部生命周期已使用实测有效的 `spatial:m` 在 1 与 1.25 间过渡；设备空间/折叠听感仍须 Gate 评估。

⚠️ **AFX-4.5 历史断言已撤回**：旧版“不发送 `spatial:m`”测试与安装期常量实现均已由 AFX-10.3 实际 PCM 过渡取代。macOS 已有空间参数运行时变化实测；其他平台仍须独立测量。旧 2026-10-03/04 对运行时失效的记录仅作为历史保留，不代表当前能力。

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
