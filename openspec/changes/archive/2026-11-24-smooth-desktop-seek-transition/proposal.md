## Why

关联 [Issue #102](https://github.com/zsxink/Echo/issues/102)。桌面端播放中直接向 libmpv 发出绝对定位指令。使用仓库随附的 macOS libmpv 0.36.0 隔离抓取 PCM，五次定位均出现超出原信号范围的相邻采样跳变；这是潜在爆音的信号证据，不等于已经验证扬声器听感。先修复这条已复现路径。

## What Changes

- 桌面播放中定位采用短输出增益过渡：先压低旧位置的输出，再定位，待新位置恢复后抬升输出。
- 主播放栏、刘海播放器、歌词和快捷键等现有定位入口走同一后端保护。快速定位不堆积过渡；保留最后一次有效意图，保留已有的定位范围与失败回滚。
- 用户设定的音量、静音、音效及响度归一化不随内部过渡变化。暂停时定位不自动开始播放；换曲、停止和结束不会恢复旧曲的过渡。
- 不修改暂停/继续、换曲、停止本身的音频过渡，不引入自研 DSP，不改变 UI 手势或移动端行为。这些路径的听感仍需独立验证。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `desktop-playback`：新增播放中定位的增益过渡、连续定位及并发取消要求。

## Impact

变更限于 `echo-desktop` 的播放 actor、既有 Backend/原生 libmpv 适配边界、相关回归及证据。Rust Core、React/Tauri IPC、快照格式、数据库、播放器依赖和设置兼容性不变。依据 `docs/PRODUCT.md` 的本地播放原则、`docs/DESIGN.md` 的平台播放器边界、`docs/ROADMAP.md` 一期播放控制目标及 `docs/interface-terminology.md` 的定位术语。
