## 1. 实现桌面定位过渡

- [x] 1.1 给现有 actor 增加简小、有界、非阻塞的定位过渡状态机；在原生后端接入定位与播放恢复事件及独立的瞬时增益逻辑。
- [x] 1.2 保留暂停、静音、加载中定位、已有位置接受/拒绝语义；处理定位合并、换曲和停止取消、结束、超时、写入失败恢复以及过渡中音量变化。

## 2. 回归与真实输出

- [x] 2.1 新增可控时钟/假后端测试，覆盖增益与 seek 排序、状态回滚、在途合并、忽略陈旧事件、静音/暂停、最新音量及取消清理。运行 `cargo test -p echo-desktop --lib player::actor`。
- [x] 2.2 使用与本仓库一致的 libmpv 和实时音频捕获，记录修前/修后连续音的相邻采样差、单次与快速定位最大跳变、淡变耗时；明确与听感验证的区别。覆盖暂停定位、静音、结束和换曲中途取消；如无法运行，保留未通过状态和原因，不用离线正对照代替。
- [x] 2.3 运行 `cargo fmt --all -- --check`、`cargo clippy -p echo-desktop --all-targets --all-features -- -D warnings`、`cargo test -p echo-desktop --all-features` 以及 `pnpm verify:governance -- --sections=registry,static`；记录实际执行结果。

## 3. 规格与交付

- [x] 3.1 验证通过后同步 desktop-playback delta，运行 OpenSpec 严格验证、`git diff --check` 并归档变更。未完成的验证保留为开放任务，不提交 PR。

### 当前验证记录

- 真实输出采集（CoreAudio 进程 tap，内建扬声器 48 kHz，25% 满量程 437 Hz 正弦；基线 `main` 94ac49cd 与修复分支同一 harness）：单次定位的相邻采样最大跳变从 0.066–0.223（峰值的 26%–84%）降到 0.022–0.024（9%–10%），接近同一段连续音的自然斜率 0.014–0.015（5.7%–6.0%）。快速连续定位 0.022 左右且只淡变一次；曲尾定位 0.024；暂停/静音中定位无输出；换曲后新曲不继承衰减；曲目结束状态为 `Ended`。详见 `docs/verification/desktop-seek-transition-2026-11-24.md`。
- 实测纠正了初始 12 ms 等功率淡变：`replaygain-fallback` 按渲染音频帧（约 10 ms）落地，12 ms 淡变在输出上退化为一次约 -49 dB 阶跃（跳变 0.101，比基线更差）。最终改为 120 ms 线性幅度淡变 + 50 ms 底噪保持，实测单步约 1/12 满幅；代价是单次定位提交前多约 170 ms 等待。
- 隔离 macOS libmpv 0.36.0 PCM 文件输出：相同源、公开音量 80、静音关闭时，运行时 0 / -20 / -40 / -60 dB 独立增益的峰值分别为 4192 / 416 / 32 / 0（最后一项为量化结果，不等同于绝对静音）。同一实例在播放中还原 0 dB，三次均回到 4192，公开音量与静音不变。
- Rust 1.98.1：actor 定向回归 117 项通过；echo-desktop all-features 测试、格式检查、严格 Clippy、治理 registry/static 与 OpenSpec 严格校验通过。数字采集不是听感验收。
