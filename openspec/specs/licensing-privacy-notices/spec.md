# licensing-privacy-notices Specification

## Purpose

定义 Echo 项目许可声明、用户媒体权利告知与隐私边界的交付契约：许可定性在许可文件、SPDX 字段、第三方清单与双语 README 间保持一致且内部自洽；用户媒体内容责任与局域网同步范围有明确告知；隐私边界记录在案并约束未来数据外发功能的发布次序；法律复核是正式发布前的显式门槛。

## Requirements

### Requirement: 项目许可声明一致性

仓库 MUST 以单一事实（根目录 `LICENSE`）界定 Echo 自身原创部分的许可，且许可定性 MUST 在所有公开表述处一致：Rust workspace 的 SPDX `license` 字段、`LICENSES.md`、中英文 README MUST 表述同一许可定性，MUST NOT 在任何位置把该许可标注为标准 `BUSL-1.1`（本许可的授权范围与 BUSL-1.1 基础授权不可互换）。许可文本 MUST 内部自洽：授权条款 MUST 只列举明示授予的权利并声明未列举的权利（含再分发）不予授予，MUST NOT 同时存在「授予再分发」与「禁止再分发」的对立条款。许可文本 MUST 厘清授权主体、再分发例外、第三方组件权利保留、适用法与争议管辖。

#### Scenario: SPDX 与许可文件一致

- **WHEN** 检查根 `Cargo.toml` 的 workspace `license` 字段与根 `LICENSE`
- **THEN** 两者指向同一许可（自定义许可以 `LicenseRef-` SPDX 标识表达），且任何位置不以 `BUSL-1.1` 作为本项目许可标识

#### Scenario: 第三方清单与 README 不与许可定性冲突

- **WHEN** 阅读 `LICENSES.md` 与中英文 README 的许可章节
- **THEN** 对 Echo 自身许可的描述与 `LICENSE` 一致，第三方组件另按其上游许可单独表述

#### Scenario: 第三方组件权利不被自身许可减损

- **WHEN** 按 libmpv/FFmpeg 等组件的上游许可行使权利（如获取与再分发对应源码）
- **THEN** Echo 自身许可的限制条款不适用于这些组件，组件权利完全由其上游许可决定

### Requirement: 用户媒体权利与同步范围说明

项目文档（双语 README 及用户文档）MUST 说明：导入与局域网同步会复制用户的音频与歌词文件，用户 MUST 自行确保拥有在其设备间复制/同步这些内容的权利；Echo 不提供、不下载音频内容，MUST NOT 在任何表述中暗示 Echo 已取得音乐、歌词或封面的版权授权；局域网同步的对象与范围（配对设备间、经 HTTPS/TLS 的局域网传输、同步音频文件与同名 `.lrc` 歌词及曲库元数据、不同步 SQLite 数据库文件、无云端中继）MUST 有明确说明。

#### Scenario: 用户了解媒体内容责任

- **WHEN** 用户阅读 README 或用户文档中的媒体权利说明
- **THEN** 能明确获知复制与同步的媒体内容由用户自负其权利责任，Echo 未取得也未转授内容授权

#### Scenario: 同步范围可查证

- **WHEN** 用户想了解局域网同步会传输哪些数据、经什么通道
- **THEN** 文档明确列出同步对象（音频、同名 `.lrc`、曲库元数据）、通道（配对设备间 HTTPS/TLS，无云端中继）与不同步项（SQLite 数据库文件）

### Requirement: 隐私边界记录与未来功能告知义务

项目文档 MUST 记录当前的隐私边界：无账号体系、无云端中继、无遥测或崩溃上报、无联网歌词/封面/元数据抓取、诊断日志仅保存在本地设备。若未来版本引入账号、云端备份/中继、遥测、崩溃上报或远程同步等任何数据离开本地设备或局域网的功能，发布前 MUST 先在用户文档补充相应的隐私告知（收集哪些数据、用途、留存与用户权利）。

#### Scenario: 当前隐私边界可查证

- **WHEN** 用户阅读隐私边界文档
- **THEN** 能获知当前版本无账号、无遥测、无云端中继、无联网内容抓取，日志只保留在本地

#### Scenario: 新增数据外发功能前先补告知

- **WHEN** 后续变更引入遥测、云备份、账号或远程同步等功能
- **THEN** 该变更须同时更新隐私告知文档，说明数据种类、处理规则与用户权利，未更新不得发布

### Requirement: 发布前法律复核门槛

项目文档 MUST 保留发布前检查单，其中 MUST 包含「由熟悉软件许可/版权的律师复核项目许可证与依赖分发方案」的门槛项；该复核 MUST 在面向公众的正式发布前完成。

#### Scenario: 发布检查单包含法律复核

- **WHEN** 维护者按发布前检查单逐项核对
- **THEN** 法律复核作为显式条目存在，未完成不得发布

### Requirement: 第三方依赖清单完整性

仓库 MUST 以锁文件为唯一事实来源维护第三方依赖清单：`LICENSES.md` MUST 覆盖 `cargo metadata` 输出的完整 crate 图（可为含 dev / target-specific 依赖的保守超集，并在文件中声明）与 `apps/desktop` 的全部 production npm 依赖；每个条目 MUST 给出名称、版本、SPDX 许可表达式与来源。清单 MUST 由生成脚本产出，且 MUST NOT 出现与依赖图不符的概括性断言（如「全部依赖均为宽松许可」）。静态门禁 MUST 在锁文件变化而清单未重新生成时失败。

#### Scenario: 清单由锁文件生成且不漂移

- **WHEN** 依赖新增、升级或移除后运行许可门禁
- **THEN** 生成脚本的 `--check` 比对磁盘清单与当前 `cargo metadata` / production dependencies，不一致时退出非零并列出差异

#### Scenario: 许可表达式按 SPDX 原样记录

- **WHEN** 依赖声明 `OR` / `AND` / `WITH` 等复合 SPDX 表达式
- **THEN** 清单按原样保留完整表达式，不简化为单个许可标识，也不丢失 `LicenseRef-` 自定义标识

### Requirement: 随包许可材料覆盖

三个平台的安装包 MUST 同时携带合并许可全文文件（`THIRD-PARTY-LICENSES.txt`）与完整第三方依赖清单文件（`THIRD-PARTY-NOTICES.txt`）。合并许可全文 MUST 为依赖清单及各平台 vendor manifest 中出现的每一个 SPDX 许可族提供对应的 `== SPDX: <id>` 分节与官方全文正文；分节缺失或正文被截断时静态门禁 MUST 失败。随包依赖清单 MUST 显式声明其为保守超集、并说明各组件版权声明以其上游源码包为准。

#### Scenario: 新增许可族的依赖被发现

- **WHEN** 依赖图出现当前合并许可全文未覆盖的新 SPDX 许可
- **THEN** task-18.1 报出缺失的分节映射，要求补齐官方全文后方可通过

#### Scenario: 三平台安装包均可获取两份材料

- **WHEN** 检查 macOS / Windows / Linux 的打包配置
- **THEN** 三者均映射 `THIRD-PARTY-LICENSES.txt` 与 `THIRD-PARTY-NOTICES.txt` 到包内路径

### Requirement: 发布前发行材料核验

发布前检查单 MUST 在律师复核门槛之外，额外包含三项显式核验：本次发布产物确实包含随包许可全文与依赖清单；三平台 NOTICE 与本次构建的许可记录一致（防止仓库已修而发布资产仍旧）；各平台 NOTICE 指向的对应源码链接在发布时可达。对应源码链接不可访问时，平台 NOTICE MUST 如实记录该事实，并把「提供完整对应源码或 GPL-2.0 §3(b) 三年书面要约」列为发布前待决项；项目 MUST NOT 以不可访问的链接冒充已履行对应源码义务。

#### Scenario: 对应源码链接失效

- **WHEN** 发布前核验发现某平台 NOTICE 指向的固定来源链接返回不可访问
- **THEN** 该事实被写入 NOTICE 与检查单待决项，发布在此项处置完成前不得继续

#### Scenario: 发布资产与仓库修复不一致

- **WHEN** 仓库许可材料已更新但本次 Release 资产仍缺少随包许可文件
- **THEN** 检查单的发布资产核验项阻止发布，直到重新生成并核验资产
