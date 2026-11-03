## ADDED Requirements

### Requirement: 系统正在播放信息的发布不重复处理封面数据

系统 SHALL 在发布 macOS Now Playing 信息时，把封面处理与播放位置帧解耦。封面数据 MUST 按封面标识缓存：同一封面的数据 MUST 只读取一次、只解码一次，MUST NOT 因播放位置推进而重复读取或重复解码；封面标识变化时 MUST 重新读取并重新解码新封面的数据。系统 MUST 只为系统展示读取尺寸受控的封面档位，MUST NOT 为系统展示读取原始尺寸的封面文件。封面不可用时，系统 MUST 省略封面并保持其余展示信息有效，MUST NOT 发布陈旧封面。

#### Scenario: 位置推进不重复处理封面
- **WHEN** 播放位置持续推进，而当前歌曲的封面标识未发生变化
- **THEN** 系统 MUST NOT 再次读取该封面的数据
- **AND** MUST NOT 再次解码该封面

#### Scenario: 封面变化时重新处理
- **WHEN** 切换到封面标识不同的歌曲
- **THEN** 系统 SHALL 读取并解码新封面的数据
- **AND** 发布的封面 MUST 对应新歌曲

#### Scenario: 只读取尺寸受控的展示档位
- **WHEN** 系统为 Now Playing 准备封面数据
- **THEN** 系统 MUST 只读取尺寸受控的展示档位
- **AND** MUST NOT 读取原始尺寸的封面文件
