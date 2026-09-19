## Context

见 proposal.md 的 Why 与 specs/ci-release-pipeline/spec.md。

现状:
- 已有 `.github/workflows/ci.yml`(push/PR 验证流水线)覆盖 Rust/前端/governance 三平台,Linux webkit 依赖、pnpm 11.18.0、Rust 1.96.0(pinned)已就位,可直接复用的模式。
- 本地发布入口已存在:`pnpm echo release` = `generate:ipc` → `build` → `tauri build`(`scripts/echo.mjs`)。
- `apps/desktop/src-tauri/tauri.conf.json`:`bundle.targets` 为 `["app"]`;`version` 为 `0.1.0`;`bundle.macOS.files` 将 vendor libmpv dylib 打进 `Echo.app/Contents/Frameworks`。
- macOS libmpv 为 universal(lipo x86_64+arm64);`task-9.7.mjs` 检查 `target/aarch64-apple-darwin/release/bundle/macos/Echo.app` 内 Frameworks 的 checksum(硬编码 aarch64 路径)。

## Goals / Non-Goals

**Goals:**
- 一条 tag 触发、三平台各自产出安装包、汇总为一个 GitHub Release 且附 checksum 与许可证的发布流水线。
- 通过 tag↔tauri.conf 版本一致性校验,杜绝错误版本发布。
- 复用既有 `pnpm echo release` 与 `ci.yml` 的平台依赖模式,不在 workflow 里重复实现构建编排。
- 保持既有 `ci.yml` / `macos-platform-gate`(task-9.7)不回归。

**Non-Goals:**
- macOS 代码签名与 notarization(需要 Apple Developer ID / App Store Connect 密钥,本期未就绪);产物为 unsigned,CI 上以 `APPLE_SIGNING_IDENTITY` 等 secret 缺省跳过。
- macOS universal(Intel+Apple Silicon)构建;本期仅 arm64。
- Windows 代码签名(digicert 等)与公证。
- 创建一键安装器之外的其他分发形态(如 auto-updater 签名发布管道、checksum 签名)。
- 修改既有 `ci.yml` 的 push/PR 验证路径。

## Decisions

### D1. 新增独立 `release.yml`,而非改 `ci.yml`

两者职责分离:ci.yml 负责提交/PR 的质量门;release.yml 负责 tag 发布。共享的模式(platform 依赖、toolchain、pnpm)在 release.yml 中按需重复,保持 workflow 文件可独立理解 —— 不使用可复用的 composite action,因为当前只有两个调用方、工作量不值得抽象,且 YAML 组合在 GitHub Actions 里调试成本高。后续若出现第三个使用方再抽取。

### D2. 构建复用 `pnpm echo release`

每个平台 job 执行 `pnpm echo release`,它串起 `generate:ipc`(生成 IPC DTO)→ `pnpm build`(Vite 前端)→ `tauri build`。Tauri 使用 `tauri.conf.json` 的 `bundle` 配置产出平台对应 target。`bundle.targets` 使用联合数组 `["app", "dmg", "nsis", "appimage", "deb"]`:Tauri bundler 按当前平台自动过滤不可用 target —— macOS 取 `app`+`dmg`,Windows 取 `nsis`,Linux 取 `appimage`+`deb`。native binary 名与 `productName`("Echo")保持一致,故产物名以 `Echo_*` 开头。

**备选**:按平台条件设置 targets(如 `tauri build --bundles dmg`),但 Tauri CLI 的 `--bundles` 参数与部分平台组合会报错,联合数组让单个 `tauri.build` 命令在各平台都正确,无需在每个 job 里加分叉。

### D3. macOS 仅 arm64

macos-latest runner 为 Apple Silicon;`cargo build --release` 默认本机架构,即 arm64,产物为 `target/aarch64-apple-darwin/release/bundle/macos/`。这也是 task-9.7 硬编码路径对应的架构。若直接用 `--target universal-apple-darwin` 需额外安装 x86_64 target、合并双架构,并改变 bundler 输出目录,反而与 9.7 的路径断言冲突 —— 本期不做,universal 留后续 change(见 Open Questions)。

### D4. 版本一致性校验放在 workflow 内,exit 非零即失败

首个构建 job(或独立 gate job)先从 tag 推导版本(`v0.1.0` → `0.1.0`),读 `apps/desktop/src-tauri/tauri.conf.json` 的 `version`,两者不一致则 `exit 1`。用 node(仓库已有 node/pnpm 环境)解析 JSON,避免引入 jq 依赖;同时校验 tag 符合 `v<semver>` 形状。放置于每个平台 job 的开头(而不是单独 job)理由:三个平台都要构建,校验失败就尽早停在各自平台,不浪费矩阵;且共享同样的 node 环境。

### D5. GitHub Release 上传用官方 `softprops/action-gh-release@v2`

对 tag 触发,`action-gh-release` 自带基于 `GITHUB_TOKEN` 的默认权限与 `VERSION` 推导;对 `workflow_dispatch`(无 tag),需显式传 `tag_name`。上传三类资产:
- 各平台安装包(glob:macOS `.dmg`,Windows `*.exe`,Linux `.deb` + `.AppImage`)
- 各平台 `SHA256SUMS`(构建 job 内 `sha256sum`/`Get-FileHash` 生成)
- libmpv `NOTICE.md`(固定路径 `apps/desktop/src-tauri/vendor/libmpv/macos/NOTICE.md`)

该 action 对已存在同 tag 的 Release 采用追加资产而非新建,满足 spec 中"重复触发不产生重复 Release"。这是除自写 REST 上传 API 外维护成本最低的选项。

### D6. 每个平台 job 独立生成并上传自身资产,统一汇总

构建 job 通过 `actions/upload-artifact@v4` 上传产物到平台级 artifact(便于失败归因与人工取用);一个汇总 job(需要三个构建 job 成功结束)用 `actions/download-artifact@v4` 取回全部产物,再调 `action-gh-release` 一次性上传。这样 Release 资产在三平台齐全后才发布,避免半成品 Release。

### D7. Linux 平台依赖在 workflow 内显式安装

Linux WebKitGTK/AppImage/deb 的构建需要 `libwebkit2gtk-4.1-dev`、`libayatana-appindicator3-dev`、`librsvg2-dev`、`patchelf`,以及生成 deb 所需的 `dpkg`(ubuntu runner 自带)与生成 AppImage 所需工具。这些与本仓库 `ci.yml` 已安装的 Linux 依赖一致 —— 直接沿用同样的 apt 行,保证先在 GitHub 上验证过的组合再次生效。

### D8. 产物校验

构建后、上传前,每个平台 job 断言预期产物存在(glob 匹配到非空):
- macOS:至少一个 `.dmg`
- Windows:至少一个 `.exe`
- Linux:至少一个 `.deb` 与一个 `.AppImage`
缺失即 fail,满足 spec"缺失产物导致失败"。用 `test -n "$(ls ...)"` 或 PowerShell 等价,不额外引入 node 脚本。

## Risks / Trade-offs

- **unsigned macOS 产物不可直接双击安装** → 本期明确 non-goal,在 Release body 与 README/公告中说明"未签名,可在隐私与安全性设置中放行";签名/公证列为 Open Question 的后续 change。
- **`bundle.targets` 变化可能影响本地开发/CI 构建时长** → `tauri build` 现在会在各平台多产出 dmg/nsis/deb/AppImage。`ci.yml` 的 frontend job 只 `pnpm build`(不 `tauri build`),不受影响;`macos-platform-gate`(task-9.7)走 `pnpm verify:task` 且检查的仍是 `aarch64-apple-darwin` 下的 `Echo.app` Frameworks,产物路径不变 —— 已在 proposal 中标注,apply 阶段以实测 `pnpm verify:scenario`/`verify` 门禁闭环。
- **`action-gh-release` 权限** → 需要 `GITHUB_TOKEN` 具备 `contents: write`;仓库默认关闭自定义 token 时该 token 即具备,apply 阶段在 workflow 头部显式声明 `permissions: contents: write`。
- **artifact 下载/上传 IO 成本** → macOS `.dmg` 与 `.AppImage` 较大;平台级 artifact + 汇总上传是标准做法,成本可接受。
- **Windows 上 NSIS 打包** → Tauri 2 的 nsis 需要额外资源的下载(CLI 按需拉取);离线/代理受限的 runner 可能失败 → 通常 GitHub hosted runner 可访问,apply 阶段用 workflow_dispatch 实测一次。

## Migration Plan

1. 新增 `.github/workflows/release.yml`。
2. 修改 `apps/desktop/src-tauri/tauri.conf.json` 的 `bundle.targets`(schema 校验通过后)。
3. 手工用 `workflow_dispatch` 在各平台 job 触发一次,核对产物与 Release。
4. 确认既有 `ci.yml` 与 `macos-platform-gate` 全绿,确认 `task-9.7` 未回归。
5. 按工程治理约定,将新 capability 登记进规格/追溯/场景清单(`docs/traceability.md` + `scripts/verify/manifest.json`),使发布流水线领域纳入门禁覆盖。

回滚:revert release.yml 与 tauri.conf 的 targets 改动即可恢复 `["app"]` 行为;Release 资产若已发布,保留或按需删除,不影响构建。

## Open Questions

- **macOS universal 构建**(Intel+Apple Silicon):libmpv 已 universal,但 Rust 双架构编译与 bundler 目录变化会与 task-9.7 的 aarch64 路径断言交互;待三平台基础管道稳定后,作为独立 change 评估。
- **代码签名/notarization 接入**:所需 secret(Apple Developer ID、App Store Connect 密钥、`APPLE_SIGNING_IDENTITY` 等)就绪后,在 release workflow 内以条件步骤启用;本期跳过。
- **Windows 代码签名**(Authenticode):无证书;后续接。
- **发布到 GitHub Release 之外的分发渠道**(如 Homebrew cask、Scoop、Linux 包仓库):不回本期。