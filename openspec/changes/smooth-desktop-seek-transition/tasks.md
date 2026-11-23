## 1. 实现桌面定位过渡

- [x] 1.1 给现有 actor 增加简小、有界、非阻塞的定位过渡状态机；在原生后端接入定位与播放恢复事件及独立的瞬时增益逻辑。
- [x] 1.2 保留暂停、静音、加载中定位、已有位置接受/拒绝语义；处理定位合并、换曲和停止取消、结束、超时、写入失败恢复以及过渡中音量变化。

## 2. 回归与真实输出

- [x] 2.1 新增可控时钟/假后端测试，覆盖增益与 seek 排序、状态回滚、在途合并、忽略陈旧事件、静音/暂停、最新音量及取消清理。运行 `cargo test -p echo-desktop --lib player::actor`。
- [ ] 2.2 使用与本仓库一致的 libmpv 和实时音频捕获，记录修前/修后连续音的相邻采样差、单次与快速定位最大跳变、淡变耗时；明确与听感验证的区别。覆盖暂停定位、静音、结束和换曲中途取消；如无法运行，保留未通过状态和原因，不用离线正对照代替。
- [x] 2.3 运行 `cargo fmt --all -- --check`、`cargo clippy -p echo-desktop --all-targets --all-features -- -D warnings`、`cargo test -p echo-desktop --all-features` 以及 `pnpm verify:governance -- --sections=registry,static`；记录实际执行结果。

## 3. 规格与交付

- [ ] 3.1 验证通过后同步 desktop-playback delta，运行 OpenSpec 严格验证、`git diff --check` 并归档变更。未完成的验证保留为开放任务，不提交 PR。

### 当前验证记录

- 隔离 macOS libmpv 0.36.0 PCM 文件输出：相同源、公开音量 80、静音关闭时，运行时 0 / -20 / -40 / -60 dB 独立增益的峰值分别为 4192 / 416 / 32 / 0（最后一项为量化结果，不等同于绝对静音）。同一实例在播放中还原 0 dB，三次均回到 4192，公开音量与静音不变。
- 以上验证了原生增益写入和恢复，不证明实时 fade/seek 时序。当前 macOS 执行会话未暴露 CoreAudio 输出且没有已验证的回环设备；文件 sink 不按真实媒体时间运行。2.2 仍待在可捕获实际输出的环境运行，3.1 因此保持开放。
- Rust 1.98.1：actor 定向回归 117 项通过（新增 10 项）；echo-desktop all-features 测试、格式检查、严格 Clippy、治理 registry/static 与 OpenSpec 严格校验通过。Linux 的无设备测试不是 macOS 听感验收。
