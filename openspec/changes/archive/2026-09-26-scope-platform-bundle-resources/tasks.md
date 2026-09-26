## 1. 打包配置按平台隔离

- [x] 1.1 在 `apps/desktop/src-tauri/tauri.windows.conf.json` 新增平台专属配置，把 `libmpv-2.dll`、`vulkan-1.dll`、`NOTICE.md`、`VulkanRT-LICENSE.txt` 四条 `bundle.resources` 映射原样迁入（源路径与目标名不变）；验证文件为合法 JSON，且四条映射的源/目标与迁移前逐条一致
- [x] 1.2 从 `apps/desktop/src-tauri/tauri.conf.json` 删除顶层 `bundle.resources`；验证主配置中不再有 `resources` 键，且 `bundle.macOS.files`（9 条 dylib + NOTICE）与 `bundle.linux.deb/appimage.files` 未被改动（`git diff` 仅显示 `resources` 块删除）
- [x] 1.3 用 macOS 正式构建验证产物纯净：`pnpm echo release` 后检查 `target/*/release/bundle/macos/Echo.app/Contents/Resources/` 不含 `.dll` 与 Linux `.so`，`Contents/Frameworks/` 仍含全部 macOS dylib；同时确认 `target/release/` 不再出现 `libmpv-2.dll`/`vulkan-1.dll`（编译期拷贝症状一并消失）；验证 dmg 体积较修复前（约 66.7 MB）显著缩小
      - 实测：`Echo.app` 146 MB → 30 MB，dmg 66.7 MB → 15.7 MB；`Resources/` 仅余 `icon.icns` + `third-party/`；全包 `find` 无 `.dll`/`.so`；删除旧 `target/release/*.dll` 后 `cargo build --release -p echo-app` 未再生成

## 2. Gate 同步与回归断言

- [x] 2.1 改 `scripts/verify/checks/task-9.7.mjs` 的 Windows 分支：资源映射断言改读 `tauri.windows.conf.json`（并对主配置断言顶层 `resources` 不存在，防止回归）；验证文件为合法 JS 可被 `node --check` 通过
- [x] 2.2 在 `task-9.7.mjs` 的 macOS 分支新增产物纯净性断言：正式构建产物的 `Contents/Resources/` 不得含非本平台播放后端二进制（按扩展名 `.dll` 与 Linux `.so` 判定）；验证断言在产物干净时通过
      - 追加：把 `9.7` 加入 `.github/workflows/ci.yml` macOS Gate 步骤（原为 `1.9 1.10`，macOS 从不跑 9.7，断言在 CI 中是死代码）；`9.7` 对 CI 步骤的字符串断言是子串匹配，追加任务号后仍成立
- [x] 2.3 验证 2.2 的断言确实能失败：临时把一条 Windows 资源映射写回主 `tauri.conf.json` 顶层并重跑，`pnpm verify:task -- 9.7` 必须红（不得静默跳过），随后还原
      - 实测：注回四条映射并**重新执行真实构建**（而非只改配置），`Resources/` 复现 `libmpv-2.dll` 114.2 MB / `vulkan-1.dll` 1.5 MB；9.7 以 exit 1 报 `carries non-macOS playback binaries: libmpv-2.dll, vulkan-1.dll`；随后还原配置并重建，`Resources/` 恢复仅 `icon.icns` + `third-party/`
- [x] 2.4 在 macOS 上跑通 Gate：`pnpm verify:task -- 9.7 9.8` 全绿；`node scripts/verify/checks/task-9.7.mjs` 在无正式产物时按既有形态报告 pending 而非失败
      - 实测：`pnpm verify:task -- 9.7 9.8` 全绿；与 CI 完全一致的 `pnpm verify:task -- 1.9 1.10 9.7` 亦 exit 0；移走产物后单跑 9.7 输出 `skip: no formal build artifact found...` 且 exit 0
      - 依赖顺序：9.7 的 codesign/签名段读取的产物须先经 1.9 的 universal 合并 + `codesign --force --deep --sign -`（`task-1.9.mjs:56`）；裸 `tauri build` 的未重签产物会让 codesign 段失败。故 9.7 必须在 macOS CI 作业中排在 1.9 之后（已如此接线）
- [ ] 2.5 在 Windows 上跑 `pnpm verify:task -- 9.7 9.8` 确认 DLL 映射与真实加载仍绿（Windows 专属，需在 windows-latest 或本地 Windows 执行；本地无法执行时在任务勾选中显式标注待 CI 验证，不得默认视为通过）
      - **待 CI 验证**：本机为 macOS，Windows 分支（含本次改写的 6c）未执行。CI 的 `windows-platform-gate` 步骤已跑 `9.7 9.8`，但该作业需本次改动推送后才会以新配置运行

## 3. 规格与未归档 change 对齐

- [x] 3.1 把「每个平台的安装包 MUST NOT 携带其它平台的播放后端二进制」及两个可验证场景补入 `openspec/changes/introduce-windows-linux-libmpv/specs/platform-libmpv-supply-chain/spec.md` 的「三平台打包与加载契约」requirement（该能力尚未进入 `openspec/specs/`，在其源 change 内就地补入可避免 `MODIFIED` delta 在归档时被拒，并消除两 change 的归档顺序依赖）；同时更新其 task 3.4 措辞为平台专属配置表述；验证 `openspec validate introduce-windows-linux-libmpv` 通过且该 requirement 含新增约束与场景
      - 实测：`openspec validate introduce-windows-linux-libmpv` → valid；新增 1 条约束段 + 2 个场景（产物不含其它平台的播放后端 / 平台专属资源不外溢到其它平台）
- [x] 3.2 确认本 change 声明 `skip_specs: true` 且 `specs/` 目录不存在（零 delta 由父 change 内的规格修正承载）；验证 `openspec validate scope-platform-bundle-resources` 通过且无 archive 拒绝 INFO
      - 实测：valid，仅剩 `skip_specs is set` 的 INFO，无 archive 拒绝项
- [x] 3.3 归档本 change 时同步 `scripts/verify/manifest.json` 的 9.7 条目描述（新增产物纯净性断言），并按既有流程重生成 traceability / prd-matrix 基线；验证 `pnpm verify:governance` 通过
      - manifest.json 9.7 条目描述已更新（保持 1 空格缩进，`node -e JSON.parse` 校验合法）
      - prd-matrix **未重生成**：在剔除本次全部改动的干净树上执行 `node scripts/verify/prd-matrix.mjs --write` 仍产生 281+/214- 漂移，且 A1/A6/A7/A12/A13/A14 六行数值与本次改动导致的完全一致 —— 属**既有基线滞后**（其它工作未同步），与本次仅触及 `platform-libmpv-supply-chain` 的规格修改无关。吸收它会把 495 行无关变更混入本 change，故保留已提交状态，另行同步
      - `pnpm verify:governance` 通过（exit 0，`ok governance: CI core gate passed`；注入套件 27/27 全部按声明失败）
