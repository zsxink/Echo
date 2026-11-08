# Spec Delta

## ADDED Requirements

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
