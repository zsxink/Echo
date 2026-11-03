# Spec Delta

## ADDED Requirements

### Requirement: 封面缩略图必须解码已声明的图片格式

封面缩略图生成 SHALL 解码内嵌封面可能声明的全部图片格式：PNG、JPEG、GIF、BMP、TIFF 与 WebP。WebP 内嵌封面 MUST 与 JPEG/PNG 封面一致，按需生成 `list`（最长边不超过 128px）与 `detail`（最长边不超过 512px）两档 JPEG 缩略图，并继续复用既有的按需生成、单飞合并、原子落盘与不可变缓存语义。缩略图管线 MUST NOT 因原图为 WebP 而返回不可解码错误或 404。

构建配置 MUST 实际启用上述格式的解码能力；约束测试 MUST 保证每一个会被声明的图片 MIME 都有启用的解码器，新增声明格式而未启用对应解码器时 MUST 使测试失败。无法解码的极端格式 MUST 维持既有失败语义：不生成可读的半成品缩略图，原图仍可通过原图档位读取。

#### Scenario: WebP 内嵌封面生成列表缩略图

- **WHEN** 歌曲原图的封面是 WebP，列表类视图首次请求 `s=list`
- **THEN** 协议返回最长边不超过 128px 的 JPEG 缩略图，缓存条目下存在 `thumb-128.bin`，后续请求复用同一结果

#### Scenario: WebP 内嵌封面生成详情缩略图

- **WHEN** 歌曲原图的封面是 WebP，详情类视图首次请求 `s=detail`
- **THEN** 协议返回最长边不超过 512px 的 JPEG 缩略图，缓存条目下存在 `thumb-512.bin`，后续请求复用同一结果
