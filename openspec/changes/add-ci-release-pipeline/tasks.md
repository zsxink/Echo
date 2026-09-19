## 1. 打包目标调整

- [x] 1.1 将 `apps/desktop/src-tauri/tauri.conf.json` 的 `bundle.targets` 由 `["app"]` 改为 `["app", "dmg", "nsis", "appimage", "deb"]`;验证 `tauri build` 的 schema 校验通过(`pnpm --dir apps/desktop tauri build --help` 不报配置错误,且 `openspec validate --change add-ci-release-pipeline` 通过)
- [ ] 1.2 在本地(任一平台)验证 `pnpm echo release` 能在不改变原有 `.app` 产出的前提下,按当前平台追加生成新的 target 安装包;验证 `target/<triple>/release/bundle/` 目录下产物符合预期形态(如 macOS 下同时有 `Echo.app` 与 `.dmg`)

## 2. 发布流水线

- [x] 2.1 新增 `.github/workflows/release.yml`:声明 `on.workflow_dispatch` 与 `on.push.tags`(`v*`);声明 `permissions: contents: write`;按 macOS/Windows/Linux 三平台矩阵定义构建 job,安装各平台编译依赖(Linux 沿用 ci.yml 的 apt 行),并稳定使用与 ci.yml 相同的 Rust 1.96.0 / Node / pnpm 版本;验证 workflow 语法被解析(`yamllint` 或 GitHub 解析器无错误)

- [x] 2.2 在每个平台 job 开头加入版本一致性校验:从 `GITHUB_REF_NAME`(tag)提取 `v<major>.<minor>.<patch>` 版本号,读取 `apps/desktop/src-tauri/tauri.conf.json` 的 `version`,不一致即 `exit 1`,错误信息同时给出 tag 版本与配置版本;验证 node 校验脚本在一致/不一致/非法 tag 三种输入下行为正确(`node scripts/release/verify-version.mjs v0.1.0` 等,若脚本落地为文件则直接跑文件)

- [ ] 2.3 在每个平台构建后、上传前生成 `SHA256SUMS`(Linux/macOS 用 `sha256sum`,`Get-FileHash` 用于 Windows),并断言预期产物存在(macOS `.dmg`、Windows `.exe`、Linux `.deb` 与 `.AppImage`),缺失即失败;验证 `action-gh-release` 上传前的「存在即非空」检查在缺产物时以非零退出

- [ ] 2.4 使用 `actions/upload-artifact@v4` 上传各平台安装包与 `SHA256SUMS`,固定路径附带 libmpv `NOTICE.md`(`apps/desktop/src-tauri/vendor/libmpv/macos/NOTICE.md`);验证 artifact 清单包含三类资产

- [ ] 2.5 新增汇总 job:依赖三个平台 job 成功,用 `actions/download-artifact@v4` 取回全部产物,统一调用 `softprops/action-gh-release@v2`(tag 触发自动推演、`workflow_dispatch` 显式传 `tag_name`)上传全部安装包/校验清单/NOTICE;验证同 tag 重复触发不会新建重复 Release(第二次在既有 Release 上追加)

## 3. 工程治理登记与一致性

- [ ] 3.1 将新 capability `ci-release-pipeline` 的 5 项需求(scenario 对应)登记进 `docs/traceability.md` 的追溯表与 `scripts/verify/manifest.json` 的场景清单,使"发布流水线领域"纳入规格/追溯/场景三方对账;验证 `node scripts/verify/reconcile-scenarios.mjs --report` 与 `pnpm verify:governance` 通过(注意 manifest.json 为 1 空格缩进、`],` 前需空行,文本化编辑而非 json.dump)

- [ ] 3.2 验证既有发布门禁不回归:确认 `ci.yml` 的 `macos-platform-gate`(`pnpm verify:task -- 1.9 1.10`,检查 `aarch64-apple-darwin` 下 `Echo.app` 的 Frameworks checksum)在 `bundle.targets` 变更后仍通过;验证 `pnpm verify:scenario` 与 `openspec validate --archived` 全绿

- [x] 3.3 更新 README/文档中与发布构建相关的说明,指明发布入口为 tag `v0.1.0` 触发、产物为未签名(notarization/证书为后续项);验证文档中引用的命令与路径真实存在(`README.md` 中 `pnpm echo release` 一栏可执行,`NOTICE.md` 路径存在)

## 4. 端到端验收

- [ ] 4.1 以 `workflow_dispatch` 人工触发一次完整 release 流水线,核实三平台 job 全绿、GitHub Release `v0.1.0` 资产齐全(三平台安装包、三份 `SHA256SUMS`、NOTICE),并在本机上抽查至少一个安装包可用;若因外部权限(GitHub 组织策略、缺 runner)受阻,记录阻塞原因并暂停,不自行绕过

- [ ] 4.2 对 spec 的每个 Requirement/Scenario 逐项核对实现覆盖,确保 `ci-release-pipeline` 的 5 项需求均有可验证证据;验证 `openspec validate --change add-ci-release-pipeline` 通过、任务全部勾选后方可提交