# 平台 Gate 交接清单（0.1.0 人工/跨平台部分）

本文由 `docs/acceptance/PRD-matrix.md` 引用：PRD A1–A14 的 macOS 可自动执行部分由场景命令直接证明，
下列场景是真实的多平台 OS 交互（关机行为、托盘、单实例、文件打开唤醒、watcher 权限撤销等），
需要在目标平台上按 `docs/native-attestation-playbook.md` §5 的人工步骤执行，并按 §6 记录
操作者、平台与证据路径到 `artifacts/native-attestations/<ID>.log`。

运行方式：
1. 在目标平台构建并启动应用（`pnpm echo dev` 或打包产物）。
2. 打开 `docs/native-attestation-playbook.md` §5，找到该场景 ID 的人工步骤逐项执行。
3. 按 §6 的 7 个必备字段写 `artifacts/native-attestations/<ID>.log`（骨架见 `tests/native/_TEMPLATE.md`）。
4. 自检：`node scripts/verify/checks/check-native-attestation.mjs <ID>`。

> 表中「验收方式」由**命令本身**决定，不来自任何生成物：命令调用
> `check-native-attestation.mjs` 的才是人工举证行，其余是自动行。

| 场景 ID | 场景 | 平台 | 验收方式 |
|---|---|---|---|
| `DAS-R04-S01` | 选择退出应用 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R04-S01` |
| `DAS-R04-S02` | 选择后台运行 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R04-S02` |
| `DAS-R04-S03` | 关闭期间存在未完成初始化 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R04-S03` |
| `DAS-R05-S01` | 从平台入口显示主窗口 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R05-S01` |
| `DAS-R05-S02` | 从平台入口控制播放 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R05-S02` |
| `DAS-R05-S03` | 托盘或菜单栏初始化失败 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R05-S03` |
| `DAS-R06-S01` | 重复启动应用 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R06-S01` |
| `DAS-R06-S02` | 系统文件关联触发打开 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R06-S02` |
| `DAS-R06-S03` | 首实例尚未就绪 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R06-S03` |
| `DAS-R09-S01` | 离线启动 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R09-S01` |
| `DAS-R09-S02` | 跨平台验证 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DAS-R09-S02` |
| `DP-R01-S01` | 播放资料库歌曲 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DP-R01-S01` |
| `DP-R01-S02` | 文件不可用 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DP-R01-S02` |
| `DP-R05-S01` | 打开资料库外文件 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DP-R05-S01` |
| `DP-R05-S02` | 临时项执行持久化操作 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DP-R05-S02` |
| `DP-R08-S01` | 使用媒体键 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DP-R08-S01` |
| `DP-R08-S02` | 使用应用快捷键 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DP-R08-S02` |
| `DP-R09-S01` | 关闭窗口退出 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DP-R09-S01` |
| `DP-R09-S02` | 关闭窗口后台运行 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- DP-R09-S02` |
| `IL-R09-S02` | 跨平台离线歌词播放 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- IL-R09-S02` |
| `LE-R05-S01` | 打开歌曲操作菜单 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LE-R05-S01` |
| `LE-R05-S02` | 查看歌曲详情 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LE-R05-S02` |
| `LE-R05-S03` | 删除当前播放歌曲 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LE-R05-S03` |
| `LE-R05-S04` | 打开本地目录 | macOS / Windows / Ubuntu | ⚠️ 不在现行 spec 中（历史遗留行，待复核） |
| `LE-R05-S05` | 删除歌曲 | macOS / Windows / Ubuntu | ⚠️ 不在现行 spec 中（历史遗留行，待复核） |
| `LE-R05-S06` | 回收站结果无法证明 | macOS / Windows / Ubuntu | ⚠️ 不在现行 spec 中（历史遗留行，待复核） |
| `LL-R03-S01` | 解析内嵌数据 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LL-R03-S01` |
| `LL-R03-S02` | 解析同名 LRC 侧车 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LL-R03-S02` |
| `LL-R03-S03` | 文件损坏或标签异常 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LL-R03-S03` |
| `LL-R07-S01` | 根目录不可用 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LL-R07-S01` |
| `LL-R07-S02` | 删除歌曲后关联可见 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LL-R07-S02` |
| `LL-R07-S03` | 文件恢复 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LL-R07-S03` |
| `LL-R08-S01` | Unicode 和平台路径 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LL-R08-S01` |
| `LL-R08-S02` | 扫描期间继续使用界面 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LL-R08-S02` |
| `LL-R08-S03` | 不泄露本机路径 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- LL-R08-S03` |
| `SFI-R06-S01` | 外部文件直接打开 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- SFI-R06-S01` |
| `SFI-R06-S02` | 活动资料库内文件直接打开 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- SFI-R06-S02` |
| `SFI-R06-S03` | 非活动旧资料库文件直接打开 | macOS / Windows / Ubuntu | ⚠️ 不在现行 spec 中（历史遗留行，待复核） |
| `SFI-R06-S04` | 源文件保持不变 | macOS / Windows / Ubuntu | ⚠️ 不在现行 spec 中（历史遗留行，待复核） |
| `SFI-R06-S05` | 暂存目录名称与用户内容冲突 | macOS / Windows / Ubuntu | ⚠️ 不在现行 spec 中（历史遗留行，待复核） |
| `SFI-R07-S01` | 已运行实例接收文件关联 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- SFI-R07-S01` |
| `SFI-R07-S02` | 冷启动文件关联 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- SFI-R07-S02` |
| `SFI-R08-S01` | 跨平台安全命名 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- SFI-R08-S01` |
| `SFI-R08-S02` | 重试导入幂等 | macOS / Windows / Ubuntu | 自动：`pnpm verify:scenario -- SFI-R08-S02` |
