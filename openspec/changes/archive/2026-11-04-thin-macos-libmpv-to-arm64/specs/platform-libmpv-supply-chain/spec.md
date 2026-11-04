# Spec Delta

## ADDED Requirements

### Requirement: macOS 播放后端架构纯度

系统 MUST 保证 macOS 端 vendored 的 libmpv 及其关联库只包含当前发布目标架构（现为 arm64），MUST NOT 随发布产物分发包含 x86_64 slice 的 universal/fat 二进制。每个文件的 Mach-O 架构集合 MUST 与发布目标架构集合（`architectures`）精确相等，manifest 中记录的 `fileArchitectures` MUST 与该文件实际 Mach-O 架构精确相等；manifest 的 SHA-256 MUST 覆盖实际分发的二进制内容；NOTICE 与实际架构构成 MUST 一致，不得保留已移除架构的说明。

重新构建或更新 macOS 播放后端基线时 MUST 以目标架构-only 产出，不得依赖事后裁剪来掩盖构建配置；无论采用何种方式，最终进入 vendor 的二进制都 MUST 满足上述架构纯度约束。

#### Scenario: 架构集合精确匹配

- **WHEN** Gate 检查 macOS vendor 目录或正式 app bundle 中的每个播放后端库
- **THEN** 该文件的实际架构集合既与发布目标 `architectures` 精确相等，也与 `fileArchitectures` 记录精确相等；任何多余架构、缺少声明架构或两者不一致都使检查失败

#### Scenario: fat 产物被拒绝

- **WHEN** 任一 vendored 或打包后的 dylib 同时包含 arm64 与 x86_64
- **THEN** 校验失败并明确指出该文件，产物不得进入发布

#### Scenario: 哈希与二进制同步

- **WHEN** 播放后端二进制被替换或裁剪
- **THEN** manifest 中对应文件的 SHA-256 同步更新，且 vendor 校验与打包校验均通过

#### Scenario: 说明文件与架构构成一致

- **WHEN** 维护者或用户检查 manifest、NOTICE 与实际二进制
- **THEN** 三者对架构的描述一致，NOTICE 不再声明保留 x86_64 slice 或未通过 Gate 的 Intel 运行时
