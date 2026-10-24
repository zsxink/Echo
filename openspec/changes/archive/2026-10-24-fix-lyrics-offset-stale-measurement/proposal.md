# Proposal

## Why

issue #75 的持续观测修复合并后，用户仍反馈「切歌时歌词偶尔画进歌曲名区域」。复核发现测量本身还有一个独立的漏洞（issue #90），且现有门禁看不见它。

**测量用被偏移过的位置测量自身。** `apply()` 按规格「先移除旧偏移再读取几何」执行 `lyrics.style.marginTop = ""`，紧接着同步读 `getBoundingClientRect()`。在 WebView2（以及任何经 `Emulation.setDeviceMetricsOverride` 驱动的 Chromium）上，被清除的内联 margin 在那一读时尚未重新布局：返回的仍是旧偏移所在的位置。于是这一趟算出的偏移是

`infoBottom − (areaTop + 上一趟偏移) + 20px`

即「信息块高度 + 间距 − 旧偏移」。A9 注入高度增长的那一步实测到 `info.bottom = 217`、同一次测量里 `lyrics.top = 189`（= areaTop 80 + 旧偏移 109），写入 `48px`——正确值是 `137px`（信息块高 117 + 20）。歌词因此压回歌曲名区域，正是用户截图的现象；三帧重驱每帧重复同一个错误值，最后停在 48px。

**门禁断的是写入值，不是几何。** A9 原先断言 `margin-top === "0px"`。任何塌缩只要落在别的数字上（48px、20px）就被判为通过——恰恰塌缩后的值不是 0。另一面，门禁只读一次几何：而写入到布局生效之间有一到两帧的中间态，规格明确允许该中间态存在，只要不长期留存。专注阅读的检查同样只读一次，把「类名已加、规则未绘制」当成失败（底部内边距读到 48px，实际是 152px）。

## What Changes

- **测量改用信息块高度，不再清除后重读**：新偏移 = 信息块渲染高度 + 20px；歌词列当前已生效的偏移以算术方式扣除（`rect.top − 内联 margin`），得到未偏移位置，仅用于判断「两盒是否共享同一区域」。信息块是本 hook 从不移动的盒子，其高度不会对 hook 自己的写入过期。
- **停止为测量而清除/回写 margin**：一趟测量不再改动被测对象，`apply()` 幂等；失败趟保持上一趟的合法值，有界重驱与全部观测通道（RO / 双 rAF / `fonts.ready` / `resize` / 断点 `change`）不变。
- **门禁改为几何不变量**：所有 A9 检查断言 `lyricsTop ≥ infoBottom + 20 − 1`，并配合有界等待 + 复查（`assertLyricsGapSettles`）：中间态不判红，塌缩态任何时限内都无法达标。失败信息带内联值与信息块 `display`，区分「没写」和「还没生效」。
- **专注往返单独成模块**（`e2e/a9-lyrics-focus.mjs`）：进入前等待规则真正绘制（信息块 `display: none` 且底部预留 ≥ 100px），退出后再次断言间距——退出路径正是「信息块从无盒到有盒」的塌缩高发点。同时把 run-e2e.mjs 从 1003 行压回 936 行，守住 1000 行规模门禁。
- **规格**：`immersive-lyrics` 的 IL-R10 测量条款改写——要求「扣除当前已生效偏移」，并明确该扣除不得依赖移除后的重排已经发生；新增场景「旧偏移尚未重新布局时重算偏移」（IL-R10-S04）。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `immersive-lyrics`: 修改 IL-R10 的测量条款（偏移必须由信息块渲染高度求出，当前已生效偏移的扣除不得依赖旧偏移重新布局已经发生），新增一个验收场景覆盖陈旧布局下的重算。

## Impact

- **桌面前端（Presentation）**：`apps/desktop/src/features/player/useLyricsStartOffset.ts`（`apply()` 的取值方式）、`useLyricsStartOffset.test.tsx`（新增陈旧布局用例）。
- **e2e（Presentation 门禁）**：`apps/desktop/e2e/a9-lyrics-offset.mjs`（不变量、有界等待 + 复查、诊断信息）、新增 `apps/desktop/e2e/a9-lyrics-focus.mjs`、`apps/desktop/e2e/run-e2e.mjs`（接线与瘦身）。
- **规格与可追溯性**：`immersive-lyrics` delta（MODIFIED IL-R10 + 新场景 → IL-R10-S04）；`docs/traceability.md`、`scripts/verify/scenario-commands.mjs`、`docs/acceptance/PRD-matrix.md`、manifest 重生成。
- 不触及 Rust Core、IPC 契约、生成的 `player.css`/`responsive.css`、窄屏布局与专注阅读行为；不修改待定 change `align-immersive-player-and-playlist-directory` 的 delta。
