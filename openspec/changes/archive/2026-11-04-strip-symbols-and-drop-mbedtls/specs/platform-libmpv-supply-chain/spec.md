# Spec Delta

## ADDED Requirements

### Requirement: macOS 播放后端符号与依赖最小集

系统 MUST 保证进入 macOS 发布产物的播放后端二进制不含成规模的本地符号：vendored 的每个 Mach-O 库在剥离后 MUST 仍通过代码签名校验与 Gate 的架构、校验和、ABI 检查，且 `nm -a` 中 MUST NOT 出现成批的本地符号（`t` / `s` 等小写类型）。主程序与 vendored 库的剥离 MUST NOT 改变其运行时加载关系或依赖关系。

系统 MUST 只分发被该平台播放后端依赖闭包实际引用的库。manifest 的 `files`、`fileArchitectures`、`licenses`、打包配置中的库映射、NOTICE 与合并许可文本 MUST 与磁盘上实际分发且被依赖闭包引用的组件集合一一对应：某个组件从分发中移除后，上述所有供应链记录与许可说明 MUST 同步移除其条目，MUST NOT 保留无对应二进制的库声明或许可条目。删除组件 MUST NOT 破坏其余组件的许可说明（与其它组件共用的许可段落 MUST 保留其余组件的覆盖范围）。

供应链校验 MUST 能拒绝回归：任一 vendored 库重新出现成规模本地符号，或供应链记录与磁盘/依赖闭包不一致时，校验 MUST 失败并明确指出该文件，产物不得进入发布。

#### Scenario: 本地符号被剥离且校验仍通过

- **WHEN** Gate 检查 strip 后的 macOS vendor 目录与正式 app bundle 中的播放后端二进制
- **THEN** `nm -a` 中不存在成规模的 `t` / `s` 本地符号，且 `codesign --verify --deep --strict`、架构集合精确匹配、manifest SHA-256 与 ABI 检查全部通过

#### Scenario: 本地符号回归被拒绝

- **WHEN** 任一 vendored dylib 被替换为含成规模本地符号的版本（或重新构建后未执行剥离）
- **THEN** 校验失败并明确指出该文件，产物不得进入发布

#### Scenario: 未引用组件被移除后记录一致

- **WHEN** 维护者检查已删除未引用库的 macOS vendor 目录、manifest、打包配置、NOTICE 与合并许可文本
- **THEN** 五处均不含该组件的条目，manifest 的库集合与磁盘文件、依赖闭包、打包映射一致，其余组件的许可说明完整保留

#### Scenario: 供应链记录漂移被拒绝

- **WHEN** manifest / 打包配置 / 许可说明仍声明某个已从磁盘移除的库，或仍为已剥离的二进制记录旧 SHA-256
- **THEN** vendor 校验或打包校验失败，明确指出不一致的条目，产物不得进入发布

#### Scenario: 剥离不改变依赖闭包

- **WHEN** 比较 strip 前后 `libmpv.dylib` 及其依赖库的 `otool -L` 引用
- **THEN** 依赖闭包内的库引用保持不变，运行时仍能解析全部所需库，被删除的组件原本不在闭包内且无任何二进制引用它
