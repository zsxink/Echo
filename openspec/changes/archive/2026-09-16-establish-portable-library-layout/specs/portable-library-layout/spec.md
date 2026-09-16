## Purpose

为 Echo 定义可携带、可版本化的资料库布局，使媒体与逻辑资料在不同设备上可被独立恢复，同时不把本机数据库或敏感配置作为资料库内容传播。

## ADDED Requirements

### Requirement: 受管理资料库目录布局

系统 MUST 使用以下资料库目录布局：

```text
<library-root>/
├── media/
│   └── <artist>/<artist> - <title>.<extension>
└── echo/
    ├── manifest.json
    ├── records/
    │   ├── songs/<uuid-prefix>/<song-uuid>.json
    │   ├── playlists/<uuid-prefix>/<playlist-uuid>.json
    │   ├── playlist-items/<uuid-prefix>/<item-uuid>.json
    │   ├── favorites/<uuid-prefix>/<song-uuid>.json
    │   └── tombstones/<uuid-prefix>/<object-uuid>.json
    ├── snapshot.json
    └── tmp/<operation-id>/
```

系统 MUST 将 Echo 管理导入的媒体置于资料库根目录的 `media/` 下；音频及其同名 `.lrc` 侧车只能位于该树。`echo/` 是可见的资料库控制面目录，`echo/tmp/` 是仅本机导入恢复临时目录；扫描、媒体浏览与用户可见的歌曲列表均不得把 `echo/` 内的文件当作音乐文件。

#### Scenario: 新资料库导入一首歌曲
- **WHEN** 用户向一个可写的新资料库导入带有艺人和标题的音频
- **THEN** 音频被发布到 `media/<安全艺人>/<安全艺人> - <安全标题>.<扩展名>`，同名歌词若成功导入则位于相同目录和基础名，控制面与暂存内容不出现在曲库

#### Scenario: 控制面目录不可写
- **WHEN** 资料库媒体根可读但 `echo/` 无法安全创建或更新
- **THEN** 系统不得把该资料库作为可同步管理资料库启用导入或逻辑变更，并明确报告资料库控制面不可用；已有可读取媒体仍可按只读资料库语义浏览

### Requirement: 可携带对象资料

`echo/` MUST 含有版本化 manifest 和以稳定 UUID 分片存放的对象资料。对象资料 MUST 覆盖歌曲、收藏、歌单、歌单成员及墓碑；歌曲资料 MUST 包含资料库逻辑 ID、BLAKE3 内容哈希与以 `media/` 开头的规范化相对路径。可携带对象资料不得包含绝对路径、SQLite 文件、同步端点、凭据、设备缓存或播放会话状态。同步资料 MUST 包含 `echo/manifest.json` 和 `echo/records/` 的已完成文件，MUST 排除 `echo/tmp/`，且 `snapshot.json` 只能作为可再生加速文件。

#### Scenario: 检查资料库控制面
- **WHEN** 支持同步的设备读取 `echo/` 资料
- **THEN** 它可以识别资料库逻辑 ID、格式版本和每个对象的 UUID、版本及业务载荷，而无需读取另一台设备的 SQLite

#### Scenario: 路径数据被序列化
- **WHEN** 一首已导入歌曲的可携带资料被生成或更新
- **THEN** 该资料只包含类似 `media/周杰伦/周杰伦 - 晴天.flac` 的相对路径，不包含任一设备的盘符、主目录、数据库位置或认证信息

#### Scenario: 同步忽略临时目录
- **WHEN** 导入操作正在 `echo/tmp/<operation-id>/` 中暂存资源
- **THEN** 同步资料不包含该操作目录或任何暂存文件，恢复程序仅处理已完成的 manifest 与对象资料

### Requirement: 新设备资料库重建

当用户在设置中选择同一资料库并连接其同步资料后，系统 MUST 先验证 manifest 的资料库逻辑 ID 和格式兼容性，再将对象资料投影到本机 SQLite，并扫描 `media/` 关联文件。歌曲、收藏、歌单和歌单成员 MUST 按原 UUID 恢复；对象资料存在但对应媒体尚未就绪时，系统 MUST 保留该对象并将歌曲标记为不可用，而不得分配替代 UUID 或丢弃歌单成员。

#### Scenario: 全新设备恢复完整资料
- **WHEN** 新设备取得一个含有媒体和完整可携带对象资料的资料库
- **THEN** 设置完成后，该设备重建与原设备一致的歌曲 UUID、我的喜欢、歌单及歌单成员顺序，并可通过相对路径播放已校验媒体

#### Scenario: 同步资料先于媒体到达
- **WHEN** 新设备已获得歌曲、收藏和歌单对象资料，但 `media/` 中尚未存在某首歌曲
- **THEN** 系统显示该歌曲为不可用并保留所有 UUID 关联；媒体随后到达且哈希校验成功时恢复可用，不产生第二首歌曲
