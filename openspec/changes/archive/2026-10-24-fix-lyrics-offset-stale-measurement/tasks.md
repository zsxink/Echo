## 1. 测量修正

- [x] 1.1 `useLyricsStartOffset.ts`：`apply()` 改为按信息块渲染高度求偏移（高度 + 20px），已生效偏移以 `rect.top − 内联 margin` 算术扣除；不再为测量清除/回写 margin；保留写入后几何判定、有界重驱与全部观测通道（RO / 双 rAF / `fonts.ready` / `resize` / 断点 `change`）
- [x] 1.2 更新模块头注释：写清「清除旧偏移后同步读到的仍是旧布局」这一根因，以及「信息块是本 hook 从不移动的盒子」这一取值依据
- [x] 1.3 `useLyricsStartOffset.test.tsx` 新增陈旧布局用例：几何 stub 增加 `staleMargin`（布局仍反映的 margin），断言旧偏移仍在布局中时新实现写 `137px`（信息块高 117 + 20）

## 2. 门禁硬化

- [x] 2.1 `e2e/a9-lyrics-offset.mjs`：删除 `offset === "0px"` 一类写入值断言，全部换成几何不变量 `lyricsTop ≥ infoBottom + 20 − 1`；新增有界等待 + 复查（`assertLyricsGapSettles`），失败信息带内联 margin 与信息块 `display`
- [x] 2.2 新增 `e2e/a9-lyrics-focus.mjs`：专注往返进入前等待规则真正绘制（信息块 `display: none`、底部预留 ≥ 100px），专注期间断言无内联 margin，Escape 先退专注后关表面，退出后再次断言间距
- [x] 2.3 `run-e2e.mjs` 接线（打开 / 切歌 / 高度增长 / 回落 / 退出专注）并瘦身到规模门禁内
- [x] 2.4 `node scripts/verify/check-scale.mjs` 通过（run-e2e.mjs 936 行）

## 3. 验证

- [x] 3.1 `pnpm --filter @echo/desktop test`（413 例）、`typecheck`、`lint` 通过；`prettier --check .` 仅两个未跟踪的临时探针报未格式化（不提交）
- [x] 3.2 `pnpm --filter @echo/desktop build && CHROME_PATH=<chrome> node e2e/run-e2e.mjs`：13.1 全部通过，含 A9「信息块增长」判别步（修复前写入 48px，修复后 137px）
- [x] 3.3 `node scripts/verify/checks/task-13.1.mjs` 通过
- [x] 3.4 判别力：把 `apply()` 临时还原成旧实现后，新增用例报 `expected '48px' to be '137px'`（与 A9 实测的 48px 一致）；恢复后 10/10 通过

## 4. 规格与可追溯性（归档前）

- [x] 4.1 delta：MODIFIED IL-R10（偏移由信息块渲染高度求得；扣除已生效偏移不得依赖重排已完成）并追加场景「旧偏移尚未重新布局时重算偏移」
- [x] 4.2 `docs/traceability.md` 追加 `IL-R10-S04`
- [x] 4.3 `scripts/verify/scenario-commands.mjs` 追加 `IL-R10-S04`（`REACT_T` 指向新增用例；真实几何由 task-13.1 的 A9 覆盖）
- [x] 4.4 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 manifest（461 场景），`reconcile-scenarios` 复查不漂移
- [x] 4.5 `docs/acceptance/PRD-matrix.md` 更新 A9（31 → 32）与 A13（222 → 223）摘要并登记 S04；`node scripts/verify/prd-matrix.mjs` 通过（14 条路径 / 461 场景 / 1117 条边）
- [x] 4.6 `node scripts/verify/reconcile-scenarios.mjs`（461 = 461 = 461）与 `pnpm verify:scenario IL-R10-S01 … IL-R10-S04` 全绿；`pnpm verify:governance` 的 JS 门禁与 `cargo test --workspace` 全绿，仅 coverage gate 在本机 Windows 报 86.51% < 90%（与本 change 无关，见执行记录）

## 5. 归档与 Pull Request

- [x] 5.1 `openspec validate fix-lyrics-offset-stale-measurement --strict` 通过
- [x] 5.2 归档 change（delta 合入主规格，IL-R10-S04 落位）
- [ ] 5.3 创建关联 issue #90 的 Pull Request

### 执行记录

- **根因实测**：A9「信息块增长」步的探针在一次测量内读到 `info.bottom = 217`、`lyrics.top = 189`（= 共享区域顶 80 + 旧偏移 109），旧实现据此写 `48px`；正确值是 `137px`（信息块高 117 + 20）。清除内联 margin 后同步 `getBoundingClientRect` 仍返回清除前的布局（WebView2 与 `Emulation.setDeviceMetricsOverride` 下为一到两帧滞后），所以「先移除旧偏移再读取几何」在实测中读到的是偏移自己。
- **判别力验证**：把 `apply()` 临时还原为「清除后重读」，新增单测报 `expected '48px' to be '137px'`；恢复后 `useLyricsStartOffset.test.tsx` 10/10，全量 413/413。
- **门禁改造**：`a9-lyrics-offset.mjs` 只断言 `lyricsTop ≥ infoBottom + 20 − 1`（有界等待 3s + 300ms 复查），诊断期临时放宽的 8s 已收回；专注往返抽到 `a9-lyrics-focus.mjs` 并要求规则真正绘制（底部预留 ≥ 100px）；`run-e2e.mjs` 1003 → 936 行，规模门禁通过。
- **e2e**：`pnpm --filter @echo/desktop build && node e2e/run-e2e.mjs` 13.1 全绿（打开 / 回到当前行 / 专注进出 / 切歌 / 信息块增长 / 回落）；`scripts/verify/checks/task-13.1.mjs` 退出码 0。
- **本机环境的两处既有失败（与本 change 无关，均只在 Windows 本地出现；HEAD 即 main，未改 Rust 与注入探针）**：
  - coverage gate：`cargo llvm-cov -p echo-core --all-features --ignore-filename-regex application/testing --fail-under-lines 90` 在本机报 86.51% 行覆盖（`cargo llvm-cov clean --workspace` 后复测一致）。CI（ubuntu）上同一 main 提交的 governance job 为绿。
  - `injection-suite`：`guardPath` 以 `${ROOT}/`（正斜杠）作前缀比较，而 Windows 的 `resolve()` 产出反斜杠路径，导致 23 个需要改文件的注入证明被判 `refusing to mutate outside the repository`。
- **未做**：不修上述两处环境问题（需各自立 issue）；不改待定 change `align-immersive-player-and-playlist-directory`。

