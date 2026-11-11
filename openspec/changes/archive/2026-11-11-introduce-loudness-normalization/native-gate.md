# 响度归一化原生 Gate 记录

状态：**macOS 已完成并独立复核；三平台发布 Gate 仍未通过**。按本次范围决策，Windows / Linux 真机验证延后：不得从 macOS 结论继承，也不得以命令返回值、单元测试或 UI 回执替代原生音频证据（与 `introduce-audio-effects-equalizer/native-gate.md` 的既有口径一致）。报告一律保持 `gate_passed=false`。

## macOS 实测（2026-11-11）

- 运行库：`apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib`，SHA-256 `536cc29610c2cb105671fa92a5dfac00a483f534899c5f5884b1cd55ca4d855d`
- 分发清单：`apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json`；素材由 `scripts/audio-effects/generate_fixtures.py` 生成（CC0-1.0，48 kHz 双声道 997 Hz 正弦，-60 dBFS 与 -0.1 dBFS 两档）
- 工具：`scripts/audio-effects/normalization_probe.py`（复用 `probe.py` / `mpv_client.py` 的原生客户端与 PCM 窗口分析），单元测试 `test_normalization_probe.py`；`python3 -B -m unittest discover -s scripts/audio-effects -p 'test_*.py' -v` → 28 项通过
- 证据：`evidence/macos-normalization-2026-11-11.json`（首次）、`evidence/macos-normalization-independent-rerun-2026-11-11.json`（全新 fixture / 全新进程）、`evidence/macos-audio-effects-runtime-baseline-2026-11-11.json`（P0 `volume` 参数路径对照）

| 检查 | 首次实测 | 独立重跑 | 判定 |
|---|---:|---:|---|
| 最小链结构 | 3 节点、无 equalizer/extrastereo、limiter 末位 | 同 | CONFIRMED |
| 静态 +6 dB 参考（harness 自检） | 5.996256 dB（误差 -0.003744 dB） | 5.996256 dB | CONFIRMED |
| 运行时 `0 → -6 dB` | -6.038842 dB（误差 -0.038842 dB） | -6.044049 dB | CONFIRMED |
| 运行时 `0 → +6 dB` | +5.994225 dB（误差 -0.005775 dB） | +5.989077 dB | CONFIRMED |
| 关闭回落 `+6 → 0 dB` | -5.982563 dB（误差 +0.017437 dB） | -5.989066 dB | CONFIRMED |
| 压力素材 0 dB / +6 dB 峰值 | 0.891265869 = -0.999854 dBFS | 同 | CONFIRMED（≤ 0 dBFS） |

三次运行时命令均在 EOF 之前发出、`command_result ≥ 0`、`runtime_confirmed=true`、`eof_before_command=false`，最大误差 0.044 dB（小于 0.25 dB 判据）。压力素材在 +6 dB 提升下被末端 limiter 保持在 -1 dBFS 阈值，整数 PCM16 峰值未超过 0 dBFS。

**独立复核**：`evidence/macos-independent-review-2026-11-11.md`。复核者从磁盘重算哈希、`10^(±dB/20)`、每个 `error_db` 与 `peak_dbfs`，并逐项核对最小链结构、`0031` 列顺序、`SONG_SELECT` 索引、同步载荷形状、增益常量、默认偏好、UI 文案与主规格同步；共 **76 项 CONFIRMED / 0 REFUTED / 0 CHANGED**（43 数值/哈希复核、21 规格与实现一致性、12 条 DSP 公式独立重算）。

## 自动化覆盖（不构成原生证据）

- 纯规则：来源换算（ReplayGain 2.0 dB / R128 Q7.8）、track/album 取值、peak 钳制、无 peak 上限、与 EQ 保护预算合并、非法偏好拒绝、确定性、关闭时 P0 前置增益逐位不变。
- Core：FLAC / MP3 / MP4 iTunes freeform / Opus 容器标签解析、损坏标签非致命告警、`0031` 迁移旧行全 NULL、重扫更新、同步载荷不含响度列。
- 播放链：归一化仅启用时安装最小链（`aformat → volume → limiter`）、切歌增益走 30 ms 斜坡、增益变化不重建图、关闭完全旁路、查询/应用失败降级 0 dB、暂停屏障覆盖仅归一化场景。
- 集成：`cargo test --workspace --all-features`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`pnpm --dir apps/desktop typecheck/lint/format:check`、`pnpm --filter @echo/desktop test -- --run`、`pnpm verify:governance` 全部通过；IPC 生成物无漂移；场景三方对账 `609 = 609 = 609`。

## 明确未由本次捕获证明

1. **30 ms 切歌增益斜坡平滑度**：`ao=pcm` 非实时，无法输出可判定的过渡时间轴；Rust 单测 `track_gain_change_ramp_reaches_the_new_target_without_a_hard_cut` 只证明命令序列存在中间值。
2. 实时更新延迟、underrun / 断流、输出设备重建与 CoreAudio 实际播放。
3. 中间浮点链非有限值诊断与 true-peak（PCM AO 只写整数 PCM16）。
4. **Windows / Linux 分发运行库**：本次按范围决策延后，缺证平台 MUST 标记待验证，不得用于发布宣称。
5. BS.1770 节目响度测量、匹配响度听感 A/B 与 CPU 长时测量。

## 复现

```sh
python3 -B scripts/audio-effects/generate_fixtures.py --output /tmp/echo-normalization-fixtures --duration 4
python3 -B scripts/audio-effects/normalization_probe.py \
  --libmpv apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib \
  --input /tmp/echo-normalization-fixtures/tone997-48000-2ch-60db.wav \
  --pressure-input /tmp/echo-normalization-fixtures/tone997-48000-2ch-0.1db.wav \
  --fixture-manifest /tmp/echo-normalization-fixtures/manifest.json \
  --manifest apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json \
  --output /tmp/echo-normalization-macos.json --step-timeout 15 --capture-seconds 2
python3 -B -m unittest discover -s scripts/audio-effects -p 'test_*.py' -v
```

Windows / Linux 后续补齐时，须各自使用安装包内运行库与对应分发清单，独立产出 `evidence/<platform>-normalization-*.json`，逐平台记录 CPU、系统、运行库哈希、素材与采样率；三平台齐全且经复核后才可把本文件状态改为「通过」。
