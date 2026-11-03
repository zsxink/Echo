# Spec Delta

## ADDED Requirements

### Requirement: 内嵌封面 MIME 声明必须与解码能力一致

系统从内嵌封面标签推导 MIME 声明时 MUST 将输出限制在缩略图管线可解码的图片类型（`image/png`、`image/jpeg`、`image/gif`、`image/bmp`、`image/tiff`、`image/webp`）或明确的不透明类型 `application/octet-stream` 内。标签声明为已知图片类型时 MUST 映射为对应规范 MIME；标签声明无法识别时 MUST 去除首尾空白并转为小写，仅当结果属于上述可解码集合时才保留，否则 MUST 声明为 `application/octet-stream`。

系统 MUST NOT 对外声明一个没有启用解码器的图片 MIME；约束测试 MUST 枚举可能声明的图片类型并逐一验证解码器已启用。降级为 `application/octet-stream` MUST NOT 丢弃原图字节：原图仍写入内容寻址缓存并可通过原图档位读取，该封面不生成缩略图。既有封面缓存的磁盘布局、内容寻址与 JPEG 缩略图输出 MUST 保持不变。

#### Scenario: WebP 声明保留为图片 MIME

- **WHEN** 内嵌封面标签声明的 MIME 是 `image/webp`（或大小写/空白不同的等价写法）
- **THEN** 扫描结果将其声明为 `image/webp`，且该 MIME 对应的解码器已启用

#### Scenario: 不受支持的声明降级为不透明类型

- **WHEN** 内嵌封面标签声明的 MIME 是 `image/avif` 等没有启用解码器的图片类型，或任意非图片字符串
- **THEN** 扫描结果将其声明为 `application/octet-stream`，原图字节仍写入内容寻址缓存并可读取，缩略图请求按既有失败语义处理

#### Scenario: 已知图片变体全部落入可解码集合

- **WHEN** 标签声明 PNG、JPEG、GIF、BMP 或 TIFF 封面
- **THEN** 扫描结果输出对应的规范图片 MIME，且约束测试确认每个 MIME 都有启用的解码器
