## 1. 持续观测测量

- [x] 1.1 重写 `useLyricsStartOffset.ts`：保留 layout-effect 同步初测与「先摘属性再测量」；新增 `ResizeObserver` 观察 `.now-playing-info`、双 rAF 补测、`document.fonts.ready` 补测；保留 `window.resize`；全部通道共用幂等 `apply()`；清理路径取消 rAF、断开 RO、移除监听并以 disposed 标志拦 `fonts.ready` 迟到回调；`ResizeObserver`/`fonts` 缺失时逐项降级（jsdom 语义不变）
- [x] 1.2 更新模块头注释：测量契约从「签名触发」改为「几何观测」，说明 Windows 字体时序根因
- [x] 1.3 `ImmersivePlayer.test.tsx` 既有用例全绿（对外签名不变，预期零回归）

## 2. 单元与浏览器门禁

- [x] 2.1 新增 `src/features/player/useLyricsStartOffset.test.tsx`：stub `matchMedia`（宽屏命中）与 `ResizeObserver`（捕获实例与回调），元素 `getBoundingClientRect` 可变；断言初测写属性、RO 回调按新高度重算、`resize` 重算、签名变化重建观测、≤980px 不写属性
- [x] 2.2 `e2e/mock-bridge.ts`：`player_control next/prev` 改为真实推进队列（循环、position 归零、保持播放态并重发快照）；`snapshotOf` 补 `currentArtist`/`currentAlbum`
- [x] 2.3 `e2e/run-e2e.mjs` A9 扩展：点击「下一首」→ 等待重挂载与两帧 → 再断言 `lyricsTop ≥ infoBottom` 且偏移非零；注入与签名无关的信息块高度变化（padding）→ 等待重测 → 断言仍让开，移除注入后断言回落（判别步：旧实现必红）
- [x] 2.4 判别力验证：临时还原 `useLyricsStartOffset.ts` 后 e2e A9 判别步与单测 RO 用例必须失败；恢复后全绿
- [x] 2.5 运行 `pnpm --filter @echo/desktop test -- --run src/features/player/`（单测）与 `pnpm --filter @echo/desktop build && pnpm --filter @echo/desktop test:e2e`（浏览器门禁）全部通过

## 3. 可追溯性与规格同步

- [x] 3.1 手工向 `docs/traceability.md` 追加 `IL-R10-S01`（切歌后歌词起点重新对齐）/`IL-R10-S02`（信息块高度事后变化时重算偏移）/`IL-R10-S03`（窗口缩放后重算偏移），任务列写本 change 任务号，测试层 `React`，manifest 列指向 `tests/scenarios/<ID>.yaml`
- [x] 3.2 手工向 `scripts/verify/scenario-commands.mjs` 追加三个 ID 的验收命令（`REACT_T` 过滤到 2.1 新增用例名；e2e 断言由 task-13.1 门禁继续覆盖，不进入场景命令）
- [x] 3.3 归档后运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 manifest 与 yaml（场景 ID 由主规格标题顺序派生，需 delta 合入后才能生成），无 `--write` 模式确认不再漂移
- [x] 3.4 手工更新 `docs/acceptance/PRD-matrix.md`：IL-R10-S01–S03 归入 A9（歌词与沉浸模式）相应小节并同步摘要计数；运行 `node scripts/verify/prd-matrix.mjs` 通过
- [x] 3.5 运行 `node scripts/verify/reconcile-scenarios.mjs`（三方集合相等，基线 443 → 446）、`pnpm verify:scenario IL-R10-S01 IL-R10-S02 IL-R10-S03` 逐项通过

## 4. 最终复核与归档

- [x] 4.1 `pnpm --filter @echo/desktop lint`、`typecheck`、`prettier --check .`、全量 `test` 通过；`cargo fmt --all --check` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings` 无变化基线
- [x] 4.2 `openspec validate fix-lyrics-start-offset-remeasure --strict` 通过；运行 `node scripts/verify/checks/task-13.1.mjs`（重建 + 浏览器门禁）通过
- [x] 4.3 归档（delta 合入主规格，IL-R10 编号落位），创建关联 issue #75 的 Pull Request

### 执行记录

- **RO 观察盒子**：首轮实现用默认 content-box 观察，e2a 判别步（padding 注入）不触发回调——padding 不改内容盒。改为 `observe(info, { box: "border-box" })`，与 `getBoundingClientRect` 测量的盒子一致；真实失效（字体/折行/明细改变行数）本就同时改变内容盒与边框盒，两种盒子都覆盖。
- **A9 块的位置**：切歌断言最初放在 A9 首次几何断言之后，推进了当前曲目，破坏了后续「专注阅读显示当前歌曲歌词」断言（其采样于切歌前）。已移到 A9 末尾（Escape 退出专注后、pass 前）。
- **规模门禁**：新增断言使 run-e2e.mjs 达 1044 行（上限 1000，豁免表冻结不可增）。抽出 `e2e/a9-lyrics-offset.mjs`（80 行模块），主文件回到 986 行。
- **判别力验证**：还原旧 `useLyricsStartOffset.ts` 后，单测 2 例（RO 重算/重建观测）与 e2e 判别步均失败；恢复后 380/380 单测、task-13.1 全绿。
- **门禁**：`verify:scenario IL-R10-S01–S03` 通过；`reconcile-scenarios` 443 = 443 = 443（本分支基于 main，基线 440 + 3；与 #47 的 PR 各自 +3，后合并方在 traceability/commands/manifest 有相邻行小冲突，语义无重叠）；`prd-matrix` 14 路径 443 场景 1052 边；`verify:governance` 全绿（首轮两次红均为 prettier 未过，格式化后通过）；`task-13.1`（重建 + Chromium 全量）通过。
