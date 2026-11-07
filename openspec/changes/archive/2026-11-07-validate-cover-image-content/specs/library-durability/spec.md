# Spec Delta

## ADDED Requirements

### Requirement: LD-12 自定义封面原件写入前必须确认内容真实性

系统 MUST 在把用户选择的封面字节提交为资料库内可校验原件之前确认：字节的实际图片容器格式与调用方声明的 MIME 一致，且该容器的头部可被解码器解析出有效尺寸。声明与实际内容不一致、无法识别为图片、或容器头损坏/截断的字节 MUST 在**保存阶段**以可识别的校验错误拒绝；MUST NOT 写入目录原文件、写入封面缓存、替换已保存封面、提交引用该资源的对象记录，也不得覆盖用户已输入的其它编辑内容。

声明类型 MUST 仍是 `image/jpeg`、`image/png`、`image/webp` 三者之一，且 MUST 与实际探测结果一一对应；体积非空且不超过 5 MiB、MIME 白名单、内容寻址、封面缓存磁盘布局、`cover://` 档位与缩略图生成语义 MUST 保持不变。校验 MUST NOT 因整幅像素解码而在保存路径引入与图像尺寸成正比的内存分配，也 MUST NOT 因解码器默认尺寸上限而拒绝原本可保存的合法封面；校验边界是容器头可解析，不得被表述为「整幅图像已验证可解码」。

该校验 MUST 位于共享 Core 的封面写入边界；歌单封面、艺人封面以及从目录读取已保存原件的路径 MUST 共用同一校验，UI 与平台壳 MUST NOT 拥有第二套写入规则。从目录读取时，已存在的无效原件 MUST 以明确的校验错误报告，MUST NOT 作为可展示封面写入缓存；不迁移、不删除、不回填该原件或其对象记录（与 `LD-06` 一致）。仅有历史缓存 key 而没有目录原件的封面 MUST 沿用既有恢复语义，不受本校验约束。

生产实现 MUST 遵守 CODE_STANDARDS §2、§3.1–§3.2、§4.1–§4.2、§8、§9：属于 Core Application 的输入校验，依赖方向仍为 Platform → Application → Domain（Infrastructure 通过既有 Port 接入），不新增跨层依赖、公开接口或外部依赖；错误继续使用既有可匹配的类型化校验错误。实现前 MUST 以当前 HEAD 复核 `artwork.rs` 的调用点与 `image` 特性；可执行验收证据 MUST 包含 `cargo test -p echo-core --all-features artwork::tests`、`cargo test -p echo-core --all-features --test arch`、`cargo test -p echo-desktop --all-features playlist_cover`、`cargo fmt --all -- --check`、`cargo clippy -p echo-core --all-targets --all-features -- -D warnings`、`pnpm verify:governance` 与 `openspec validate --all --strict`。本 change 不替换既有实现、导出、测试替身或登记资产，故无待清理残留；若实施中出现不再被解析的替代实现，MUST 按 §9 以引用检查与静态检查证明清理完成。

#### Scenario: 伪图片在保存阶段被拒绝

- **WHEN** 用户为歌单或艺人选择一段文本、压缩包等任意非图片字节并声明 `image/png` 后保存
- **THEN** 系统返回可识别的校验错误并说明原因，不写目录原件与封面缓存，不替换已保存封面与其它编辑内容，也不提交新的资源引用

#### Scenario: 声明类型与实际格式不一致被拒绝

- **WHEN** 提交声明为 `image/png` 但实际是 JPEG、WebP、GIF 或其它容器的字节，或声明为 `image/jpeg` / `image/webp` 而实际是另一种白名单格式
- **THEN** 系统在保存阶段拒绝并说明「声明类型与实际内容不一致」，不写入任何封面资源

#### Scenario: 容器头损坏被拒绝

- **WHEN** 提交仅带合法 JPEG/PNG/WebP 签名、但容器头截断、结构非法或无法解析出有效尺寸的字节
- **THEN** 系统在保存阶段以校验错误拒绝，不把失败推迟到 `cover://` 展示阶段，也不写目录原件与缓存

#### Scenario: 三种合法格式均可保存并展示

- **WHEN** 用户分别提交声明一致且容器头可解析出有效尺寸的 JPEG、PNG、WebP 封面
- **THEN** 三者都保存成功，目录原件的字节、长度、哈希与 MIME 与提交内容一致，并可经既有 `cover://` 路径读取

#### Scenario: 歌单与艺人封面共用同一校验

- **WHEN** 同样的伪造封面字节分别经歌单封面与艺人封面写入路径提交
- **THEN** 两条路径都在写入目录原件之前以同一类校验错误拒绝，不存在只校验其中一条的绕行入口

#### Scenario: 校验只读取容器头

- **WHEN** 提交声明一致、容器头可解析出有效尺寸、但不含完整像素数据的超大尺寸容器（例如仅含合法 IHDR 的 20000×20000 PNG 头）
- **THEN** 校验按「容器头可解析」的既定边界通过，不分配与像素数成正比的内存，也不因解码器默认尺寸上限驳回；该路径 MUST NOT 被声称已验证整幅图像可解码

#### Scenario: 既有体积与白名单语义不变

- **WHEN** 提交空字节、超过 5 MiB 的字节，或声明 `image/gif` 等白名单外的 MIME
- **THEN** 系统仍按既有体积/白名单规则拒绝，拒绝原因不被新的内容校验掩盖，且不产生 panic、静默成功或新的资源写入

#### Scenario: 既有无效原件与历史缓存-only 封面的兼容

- **WHEN** 资料库中已有指向非图片字节的目录原件，或仅有历史 cache key 而无目录原件的封面记录被读取
- **THEN** 前一种情况返回明确的校验错误，不写入缓存、不删除或改写原件与对象记录；后一种情况沿用既有 cache-only 恢复语义，不因新校验失效
