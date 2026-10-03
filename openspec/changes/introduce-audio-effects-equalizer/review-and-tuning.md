# 音效复核与听感优化（2026-10-03）

状态：**复核记录及待测项目清单**。本文记录对 `introduce-audio-effects-equalizer` 已实现部分的复核结论、修复方案与优化提案。所有离线曲线数值均为 f64 计算（RBJ peaking，Fs=48000，Q=√2），**不构成 Gate 通过**；实际取得的 PCM 单点响应另见 `native-gate.md` 与 `evidence/macos-pcm-response-probe.json`，不得外推为完整曲线或听感结论。

## 1. 复核结论摘要

实现链路是通的。三处需要处理，其中一处是假成功、一处是潜在死锁、一处是听感设计问题。

| # | 类别 | 结论 | 严重度 |
|---|---|---|---|
| R1 | 假成功 | **运行时改参整体无效**：EQ 增益 / preamp / width 的 `af-command` 均 rc=0 但 PCM 实测输出无变化。原归因「`extrastereo` 无 `process_command`」已证伪 | 高（证据误判 + 设计前提失效） |
| R2 | 潜在死锁 | `install_pending` 仅由 `reconfirm()` 清除，依赖 AUDIO_RECONFIG 必然到达 | 中 |
| R3 | 听感 | auto preamp 按合成峰值扣 1.6–5.9 dB，峰值落在 ~62 Hz，响度差盖过音色变化 | 高（用户可感知） |
| R4 | 听感 | 九条曲线幅度过小（多数 ±0.5–2 dB），弱于可闻阈值 | 中 |
| R5 | 已确认正确 | `g=0` 段数学恒等；`alimiter level=false` 正确；`latency=true` 确实补偿延迟（实测 239 样本） | 无需修改 |
| R6 | 环境 | 随包 vendor 已更新，但 `/Applications/Echo.app` 仍是旧库 | 阻断真机验证（已在修复轮解除） |

## 2. 复核方法与证据

### 2.1 随包库能力（已变更）

`apps/desktop/src-tauri/vendor/libmpv/macos`（2026-10-02 自建，libavfilter SHA-256 `2591332d9a3ccc0f6a7313b98fc2566ef4193dd40271a6908dec7243085df6bc`）滤镜白名单为 `aformat,alimiter,equalizer,extrastereo,volume`，见 `scripts/release/build-linux-libmpv.mjs` 的 `--enable-filter=`。

⚠️ **`/Applications/Echo.app/Contents/Frameworks/libavfilter.dylib`（SHA-256 `0bde7975d6407edbedf93d6bb5d43e361fd1ed8c4fef3f864d0b346245466afb`，2026-09-30）仍是旧库**，`aformat`/`volume`/`alimiter`/`extrastereo` 命中数均为 0。任何真机验证前必须重装 app，否则测的是旧库。

### 2.2 运行时改参数能力（本次新增判据）

判据是读 `AVFilter` 结构体末尾的 `process_command` 指针。

> ⚠️ **2026-10-03 更正**：本节原以 `extrastereo` 作为「无回调」的对照组，据此得出 R1。该对照**不成立**——脚本当时按错误的字段顺序读取，错位读出 `nb_inputs=65536` 等荒谬值；而只比对 `filt.name`（offset 0）的 sanity check **任何布局都能通过**，故无鉴别力。

**FFmpeg 6.0 的真实字段顺序**（`inputs`/`outputs` 在 `priv_class` **之前**，pad 计数是 `uint8`，`priv_size` 在 **offset 80**）：

```c
const char *name; const char *description;
const AVFilterPad *inputs; const AVFilterPad *outputs;
const AVClass *priv_class; int flags;
uint8_t nb_inputs; uint8_t nb_outputs; uint8_t formats_state;
int (*preinit)(...); int (*init)(...); void (*uninit)(...);
union { ... } formats;
int priv_size; int flags_internal;
int (*process_command)(...);
int (*activate)(...);
```

修正布局并用导出的 `avfilter_filter_pad_count()` 交叉校验后，对随包库的实测：

| 滤镜 | `priv_size` | `process_command` | 结构层能否改参 |
|---|---:|---|---|
| `equalizer` | 272 | 有 | ✅ |
| `volume` | 200 | 有 | ✅ |
| `alimiter` | 184 | 有 | ✅ |
| `extrastereo` | 16 | 有 | ✅（**与原结论相反**） |
| `aformat` | 56 | 无 | ❌ |

**结构层结论不等于行为结论**：PCM 判别性实测（见 R1）显示三者运行时改参**均不改变音频**。故结构检查只能用作「存在性」，行为判据必须用 PCM 前后测量。

上游依据：`af_biquads.c` 的 `process_command` 会 `av_opt_set` 后 `config_filter` 重算系数；`af_extrastereo.c` 赋 `.process_command = ff_filter_process_command`。mpv 侧补丁经 `otool` 反汇编确认存在。**故障点未定位。**

### 2.3 已确认正确的实现（不要动）

- **`g=0` 的 peaking 段是精确恒等**。RBJ 解析式在 `A=10^(0/40)=1` 时 `a0=1+alpha`，`b0=(1+alpha)/a0=1`，`b1=a1`，`b2=a2` ⇒ 分子分母相同 ⇒ `H(z)≡1`，零幅度零相移。`math.rs` 的 `Biquad::IDENTITY` 特判与该性质一致。
- **`alimiter ... level=false` 是对的**。FFmpeg `alimiter` 的 `level`（auto level）**默认开启**，会把限幅后的输出归一化回 0 dB；显式关闭才能让 `limit` 成为真实样本上限。`level_in`/`level_out` 是线性增益（默认 1.0），写 1 即直通，与 auto-level 无关。
- **`latency=true` 是对的**（2026-10-03 实测更正）。它表示 alimiter **自行补偿**前瞻延迟：FFmpeg 置 `in_trim = out_pad = attack*Fs - 1`，实测在 48 kHz/attack=5 ms 下抵消 **239 样本**（与 `5ms×48000−1=239` 精确一致）；`latency=false` 则残留全部 attack 延迟。此前「开启后多出延迟」的判断有误。
- **Q=√2 的选择本身可接受，但「≈1 倍频程」只在 +6 dB 附近成立**。peaking 的 −3 dB 带宽**随增益变化**（Fs=48000 实测全宽）：+6 dB 时 1 kHz→0.999 octave；+12 dB 时 31.25 Hz→0.54、1 kHz→0.54、8 kHz→0.43、16 kHz→**0.21** octave。RaneNote 的「1 octave↔Q≈1.414」是**带通**关系，对 peaking 不适用。故固定 Q 的十段图示 EQ 中心频率按倍频程排布是合理的，但**不得宣传各段带宽恒定或严格等于一倍频程**。

## 3. 待修问题与方案

### R1 `extrastereo` 宽度调整是假成功

> **⚠️ 2026-10-03 修复轮结论更正（详见 `native-gate.md`）**：本条原结论「`extrastereo` 没有 `process_command`」**已被证伪**——按 FFmpeg 6.0 真实字段布局复核，随包库的 `extrastereo` **确有**该回调（此前 `inspect-filter-runtime.py` 的字段序错误导致假 False）。但 PCM 判别性实测显示：**本随包库上任何 `af-command` 运行时改参都不改变音频**（EQ 增益 / preamp / width 三者一致，rc 均为 0 而输出不变），而 mpv 因 `f_lavfi.c` 只判 `result >= 0` 报假成功。故「width 只能安装期生效」的**结论成立，但理由是运行时改参整体失效**，而非 ENOSYS 掩盖。方案 A 仍适用，任务 4.5 的负向断言需按此重新定性。

**现象**：`native_effects.rs` 的空间链用 `af-command echo_spatial spatial:m <v>` 调宽度，返回 rc=0。PCM 实测输出无变化。

**证据（2026-10-03 PCM 判别性测量）**：固定 12 s 素材 + 固定 4 s 捕获窗（同链重复 5 次极差 0.0000 dB）：

| 链 | 命令 | 实测 | 理论 |
|---|---|---:|---:|
| 安装期 `g=6` | 无 | **+5.9959 dB** | +6（逐位复现既有证据） |
| 安装期 `g=0` | `eq5:gain 6`，rc=0 | **+0.0076 dB** | +6 |
| 安装期 `m=1.25` | `spatial:m 1.4`，rc=0 | **+0.0000 dB** | +0.9844 |
| 安装期 `volume=1` | `preamp:volume 0.5012`，rc=0 | **+0.0076 dB** | −6 |

**影响**：空间感 `width` 事实上固定为安装时的 `1.25`，拖动无响应。**且此结论对 EQ 增益与 preamp 同样成立** ⇒ 依赖原地改参的 30 ms 斜坡当前**不可依赖**，`Spatial::default()` 也是 1.25，因此当前**未暴露给用户的差异**；一旦 UI 开放宽度编辑即暴露。

**方案**：
- **A（采用）**：承认 width 不可运行时调整，`Payload::Spatial` 保留 `width` 但仅在安装时生效；UI 不暴露宽度滑块。
- **B**：宽度变更走 `af set`/`install` 重建整条链。因运行时改参已实测无效，**重建链是当前唯一被实测可改变音频的机制**，但需 native-gate 补充「重建期间无 underrun / 无爆音」的捕获证据。

**任务落点**：任务 4.5 的负向断言保留（仍不应发送 `spatial:m`），但其**理由文本**须改为引用 PCM 实测，而非结构体检查。

### R2 `install_pending` 缺少自愈

**现象**：`install()` 置 `install_pending = true`；清除点只有 `reconfirm()`（由 `BackendEvent::AudioReconfigured` 驱动，`actor.rs:1078`）与 `remove()`。`apply()` 与 `bypass()` 的首行都是 `if self.install_pending { return Ok(false) }`。

**风险路径**：`af add` 之后若 mpv 未发出 AUDIO_RECONFIG（链已存在、格式未变、或事件在 actor 轮询间隙被合并），`install_pending` 永久为 true，此后所有 `apply`/`bypass` 静默早退，表现为「改了没反应」且 runtime 停在 `Pending`。现有单测 `reconfiguration_reconfirms_once_and_does_not_restart_the_ramp_each_tick` 显式手工调 `reconfirm()`，**因此掩盖了这条路径**。

**方案**：`install()` 记录 `installed_at: Instant`；`apply()`/`bypass()` 的 `install_pending` 早退改为「超过一个 `TRANSITION` 窗口（30 ms）后放行并强制 `reconfirm`」，既不无限等待也不静默卡死。需补单测：不调 `reconfirm()`、仅推进时间，断言第二次 `apply` 会真正下发 `af-command`。

**任务落点**：新增任务 4.7。

### R6 真机验证阻断

**方案**：在任务 1.1 增加一条硬前置——比对 `/Applications/Echo.app/Contents/Frameworks/libavfilter.dylib` 的 SHA-256 与 `vendor/libmpv/macos/manifest.json` 声明，不一致则先重装再测。判据用 `shasum -a 256`，不用 `git status`（vendor 路径可能被 ignore）。

## 4. 听感优化提案

### 4.1 根因：auto preamp 用「合成峰值」而非响度

`math.rs::analyze` 的 `safe_preamp_db = -(peak_gain_db + 1)`，`peak_gain` 取全频段合成响应的最大值。实测该最大值的位置：

| 预设 | 合成峰值 | 峰值频点 | 该点 A 计权 | 现状 preamp | EQ 自身响度变化(pink) |
|---|---:|---:|---:|---:|---:|
| rock | +4.90 | 61.8 Hz | −26.5 dB | −5.90 | +2.56 |
| bass | +4.85 | 63.2 Hz | −26.2 dB | −5.85 | +2.35 |
| electronic | +3.68 | 62.5 Hz | −26.4 dB | −4.68 | +1.77 |
| pop | +2.33 | 62.2 Hz | −26.4 dB | −3.33 | +1.05 |
| warm | +1.88 | 62.5 Hz | −26.4 dB | −2.88 | +0.91 |
| jazz | +2.60 | 124.7 Hz | −16.2 dB | −3.60 | +1.47 |

**峰值集中在 62–125 Hz，而该频段 A 计权为 −16 至 −26 dB，对响度的贡献极小。** 结果是 preamp 扣 3–6 dB，实际只需扣 1–2.5 dB，差额 2.5–3.5 dB 纯属「为最坏情况（纯音恰好落在峰值频点）买保险」。主观上先听见的是「变小声」，音色变化被掩盖。

三份证据来源支持这一判断：
- iZotope 母带 EQ 指南要求 A/B 试听**匹配增益**，以免响度偏差被误读成音色变化；
- ITU-R BS.1770-5 的节目响度测量即以频率加权积分能量为准，而非峰值；
- ProSoundWeb 指出图示 EQ 的「面板曲线 ≠ 实际曲线」，配合 preamp 后前面板的绝对刻度进一步失真。

### 4.2 方案对比（以 rock 为例）

| 策略 | preamp | 净响度Δ | 音色RMS | 留给 limiter |
|---|---:|---:|---:|---:|
| 现状 `-(peak+1)` | −5.90 | **−3.34** | −10.17 | −1.00（limiter 不动作） |
| `-(peak+3)` | −7.90 | −5.34 | −10.17 | −3.00 |
| 响度匹配（pink） | −2.56 | −0.00 | −10.17 | +2.34 |
| 响度匹配（A 计权） | −0.70 | +1.86 | −10.17 | +4.20 |

**响度匹配把净响度差归零，音色 RMS 完全保留**（RMS 只取决于曲线形状，与 preamp 无关）。

**但代价是峰值风险全部转移给末端 `alimiter`**：现状 limiter 承接 −1.00 dB（完全不动作），改后需承接 +2.34 dB。当前 `alimiter` 参数是 `attack=5ms, release=50ms`，**5 ms attack 偏慢**，真实素材的鼓点/瞬态会产生可闻的泵感（gain pumping）。且 FFmpeg 官方明确建议「应用该滤镜前先 2x/4x 过采样 `aresample`」。

**因此不建议一步到位全额匹配。分三档推进：**

| 档 | 做法 | limiter 承接 | 风险 |
|---|---|---:|---|
| **保守（建议先做）** | 保留 peak 匹配，但把余量从 1 dB 降到 0 dB，并加 0.5 dB 输出补偿 | −0.5 dB | 极低；净响度差改善约 1 dB |
| **中间** | 响度匹配，但限制「让位给 limiter 的量 ≤ 3 dB」 | ≤3 dB | 中；需要泵感盲听 |
| **激进** | 全额响度匹配 | 4–5.4 dB | 高；需先补 `aresample` 过采样与更快 attack |

各预设在「A 计权响度匹配」下需 limiter 承接：classical 0.36、pop 1.31、vocal 1.31、jazz 1.69、warm 2.63、electronic 3.18、rock 4.20、retro 4.31、bass 5.36 dB。**classical/pop/vocal 落在保守档内，bass/retro 必须走中间或激进档。**

**策略语义**：本轮先落地保守档，沿用 `PreampMode::Auto | Manual`；Auto 按 `safe=-max(0,G)+0.5 dB` 起步，最终 sample-peak 由 limiter 保护，单元数学验证不代替 PCM 压力验收。若以后选择中间/激进响度匹配档，才需要新增显式模式以区分响度匹配和峰值保护，并同步重订 UI 与规格；那是语义变更，不能当作参数微调。

**任务落点**：4.8 实现保守公式与数学压力边界；1.4 记录 limiter 压力 PCM 捕获和分档试听。候选包压力素材必须确认 `sample-peak ≤ 0 dBFS` 且全部有限。只有评审决定进入中间/激进档时，才另立模式变更并要求 BS.1770-5 响度匹配误差≤0.5 LU 的 A/B 记录。

### 4.3 预设幅度偏小

九条曲线的增益多数落在 ±0.5–2 dB，`classical` 仅 ±0.5 dB。离线计算下 `classical` 的音色 RMS 最大（−21.8 dB，形状最"干净"），但绝对幅度仍在可闻阈值附近或以下。`design.md` 已自述「这不是宣称新值已经更好听」。

**方案**：把幅度提到可闻区间，并按 iZotope 的「按素材克制调节」原则设上限（单段 ≤±6 dB，避免相邻同号大幅提升叠加）。**不直接给新数值** —— 曲线是主观候选，必须走任务 1.4 的 BS.1770-5 响度匹配盲听迭代定稿，盲听未完成前不写入 `presets.rs`。

首尾两段（31.25 Hz / 16 kHz）可考虑 shelf 化：peaking 在 `fc±1 个倍频程` 外迅速衰减，而 31 Hz 的 shelf 能整体改变低频体感。**但这会突破「十段固定 Q peaking」的现有 spec 边界**，属于 P0 范围变更，需先修订 spec。

### 4.4 首尾加宽 Q（可选，低风险）

保持 Q=√2 但**首尾两段用更小的 Q**（如 0.7，接近 2 个倍频程宽），可让低频/高频段的能量更集中地落在可闻区。离线计算显示对 `[0,3,0,…,0,−4]` 这类曲线，合成峰值不变（峰值由中心频点决定），但**同等滑块值产生的低频增益更宽**，主观低频体感更明显。

**这是工程选择，不是行业标准**，且会改变 `math.rs` 的响应计算与 `native_effects.rs` 的安装语法（每段独立 `w=`）。列为 P1 候选，不进 P0。

## 5. 执行顺序

```
R6（重装 app，解除真机阻断）
  └─ R2（install_pending 自愈，纯逻辑，可立即做）
  └─ R1（extrastereo 语义澄清，需产品决策 A/B）
  └─ 4.2 保守档（preamp 余量 1→0 dB + 0.5 dB 补偿）
  └─ 1.4 分档盲听 → 定 4.2 最终档位
  └─ 4.3 预设幅度迭代（依赖盲听）
  └─ 4.4 首尾 Q 加宽（P1）
```

**Gate 依赖**：4.2 中间档与激进档、4.3 全部、4.4 全部依赖候选包实际 PCM 捕获。当前 `ao=pcm --capture-only` 已能捕获固定 EQ 的真实输出，不需要 Python 另行组装 `buffersrc`/`buffersink` 图；但跨采样率响应、limiter 压力矩阵、转换平滑/时延、underrun、CPU 与合许可素材仍待补测。故当前可实现并计算保守档，尚不能据此定稿其听感，也不能开始预设盲听定稿。

### 实施回填（2026-10-03）

- R1 采用方案 A：Rust 原生链把空间 width 写入安装期滤镜配置，并删除无效的 `spatial:m` 运行时调用；随包滤镜结构检查、候选 CoreAudio 探针和负向单测已留证。
- R2 已增加一个 `TRANSITION` 后的自愈路径，不再永久等待 `AUDIO_RECONFIG`；单测不调用 `reconfirm()`，推进安装时间后验证后续 `apply` 发送原地命令。
- PCM harness 已建立：libmpv `ao=pcm` 能捕获候选 bundle 的实际输出。WAVE_FORMAT_EXTENSIBLE integer PCM 解析已补齐；1 kHz +6 dB EQ 对 997 Hz/-60 dBFS 输入测得 +5.9959 dB，误差 -0.0041 dB；997 Hz/-0.1 dBFS、1 kHz +12 dB EQ、Auto preamp -11.5 dB 压力点测得 sample-peak -0.99985 dBFS。两者都是单点，不能替代滤镜、采样率与信号矩阵。
- R6 随后完成：将签名验证通过的 macOS 候选 bundle 安装到 `/Applications/Echo.app`，旧包保留在 `/Applications/Echo.app.review-backup-20261003`；安装后 libavfilter 哈希与 manifest 一致，滤镜结构检查命中预期五种滤镜，app 内 libmpv 探针通过初始化、十段链安装和 loadfile 后链存活检查（`evidence/macos-installed-app-probe.json`）。
- R3 的保守 preamp 数学与单音压力点已过，但没有 BS.1770-5 匹配素材与盲听记录；因此不切换中间/激进档。R4 曲线改值仍不做：没有盲听定稿。任务 1.2/1.3/1.4 的完整响应矩阵、实时转换/设备捕获、CPU、underrun 和听感门槛仍待完成。

## 6. 验证命令

```bash
# 结构体判据（复核 R1）
python3 scripts/audio-effects/inspect-filter-runtime.py   # 待新增

# 数学复算（复核 R3/R4）
cargo test -p echo-desktop effects::math
cargo test -p echo-desktop effects::presets

# 随包库一致性（R6）
shasum -a 256 apps/desktop/src-tauri/vendor/libmpv/macos/libavfilter.dylib
shasum -a 256 /Applications/Echo.app/Contents/Frameworks/libavfilter.dylib
```

## 7. 不在本文范围

- 三平台 PCM 捕获 Gate（任务 1.1–1.5）、Windows/Linux 候选包（任务 7.x）；
- P1 的 Loudness / Imported 载荷、动态去齿音、人声分离；
- 任何 `presets.rs` 新数值的定稿（须盲听证据）。
