# DAS-R19-S01 — Windows 无原生标题栏且内容从顶边开始

> Requirement: Windows 无边框窗口与自绘窗口控件
> 生成：`scripts/verify/gen-scenario-manifests.mjs`（traceability.md 权威）

## 目标平台

| 平台 | 版本 | 桌面环境 |
|---|---|---|
| Windows | 10 22H2 / 11 | 主验收平台（macOS/Linux 不适用：本场景验证 Windows 专属外观） |

## 人工步骤

1. 在 Windows 上以 `pnpm --filter @echo/desktop tauri dev`（或安装包）启动 Echo。
2. 观察窗口顶部：不得出现带「Echo」文字与 — □ × 的原生标题栏；侧边栏与工作区自窗口顶边的安全区（36px）以下开始，右上角为自绘窗口控件。
3. 按住顶部拖拽条拖动窗口；将窗口拖至屏幕边缘验证贴边与 Aero Snap；拖拽窗口四边/四角缩放，验证最小尺寸 760×560 生效。
4. 检查任务栏缩略图与预览正常显示；托盘「显示 Echo」可唤回窗口。
5. Win10 22H2 上重复 1–4（无 DWM 圆角/阴影描边属已知平台差异，记录实际表现）。

## 预期结果

1. 无原生标题栏，内容从顶边开始，与 macOS overlay 标题栏视觉一致。
2. 拖拽、贴边、Aero Snap、边缘缩放、最小尺寸均正常。
3. 任务栏与托盘行为无回归。

## 证据与记录

- operator: (填写执行人)
- evidence-path: artifacts/native-attestations/DAS-R19-S01.log
- date: (YYYY-MM-DD)
- result: (pass / fail + 说明)
