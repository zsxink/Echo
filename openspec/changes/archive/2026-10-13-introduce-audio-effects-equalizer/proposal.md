# Proposal

## Why

Echo 当前没有应用级音色补偿、预设试听及保存用户曲线的闭环。以 0.2.0 为目标，为一期后的桌面播放器提供离线听感增强，同时保持 Core 与播放实现隔离。

## What Changes

- 本次完整交付 PRD 的 P0：9 个内置 EQ 预设、`surround`「空间感增强」、统一类型化预设、10 段 EQ、实际响应图、自动/手动 preamp、不可关闭的削波保护、自定义曲线新增/重命名/删除及本机恢复。
- 专业资料复核：算法/测量有来源，九条曲线和空间宽度是 Echo 调音候选；收敛低音/电子初值，修正人声与高频描述，明确 sample-peak 保护边界，须经响度匹配 A/B 校准，不宣称通用专业标准曲线。
- EQ 与空间处理互斥；编辑转独立草稿。明确选择、请求启用和实际生效三个概念，覆盖关闭已保存项、关闭草稿、重置、异步失败与旧请求竞态。
- 常驻播放栏增加音效入口，非模态浮层共用「音效 / 均衡器」标签；与队列互斥，支持窄屏、键盘与读屏。
- 音效只保存于 desktop local state，不进入 SQLite、资料库控制面、outbox 或同步载荷；临时文件、后台播放和资料库切换共用全局状态。
- 2026-10-02 范围决策：用户确认保留完整 P0，三平台技术验证作为实施前门槛；按 **0.2.0** 规划，发布日期依实际验收确定。
- P1 单独立项，未实现不阻塞 P0：混响、响度归一化、导入导出（含 AutoEq 适配/保真直通）、按设备绑定；P0 不提供其可操作入口。P1 排期、格式及拟合误差阈值在后续 change 决定。
- 非目标：多效果叠加、覆盖已有曲线、单曲绑定、跨设备自动同步、网络/账号/服务端处理、音质恢复及多声道认证承诺。

## Capabilities

### New Capabilities

- `audio-effects`: 类型化预设与全局状态、9 个 EQ 和空间预设、确认/回滚、播放兼容、离线边界、持久化与三平台发布门槛。
- `audio-equalizer`: 固定 10 段编辑、响应图、preamp、整链削波保护、编辑/关闭/重置与采样率边界。
- `audio-effects-presets`: 用户曲线快照、命名与额度、原子管理及失败恢复。

### Modified Capabilities

- `desktop-app-shell`: 扩展浮层关闭及键盘优先级，增加音效与队列互斥和非模态 Tab 离开语义；保留原有场景。

## Impact

- 桌面播放抽象/actor/coordinator、runtime 组装及本机偏好适配器；集中 IPC DTO、生成类型与 React player store、播放栏/浮层。Core、SQLite、同步格式不引入音效职责。
- 后端先验证实际分发的 libmpv/FFmpeg 对 EQ、原地改参、空间拓宽、保护及曲目/输出重建的能力；失败必须修订 design/范围并复核，不能以禁用 P0 能力代替三平台验收。
- 遵守 `docs/DESIGN.md` 分层和 `openspec/CODE_STANDARDS.md` §2–4、§6、§8–9；新增 IPC 为向后兼容的加法，偏好新增版本化字段，详细迁移见 design。
- 实施时同步 `docs/PRODUCT.md` 的本机音效边界、`docs/ROADMAP.md` 的 0.2.0 后续版本范围及 `docs/interface-terminology.md`；现有 `docs/prototype/echo-desktop-player.html` 只作为布局参考，其模拟 DSP 不作为技术证据。
- 本轮仅补齐规划；不会修改生产代码或将未执行的实测标为通过。
