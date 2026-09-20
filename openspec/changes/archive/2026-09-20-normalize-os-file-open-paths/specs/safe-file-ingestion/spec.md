## ADDED Requirements

### Requirement: 操作系统文件打开载荷必须先归一化为本地路径

系统 SHALL 在**最早**的壳边界把操作系统下发的文件打开载荷归一化为文件系统路径，并且转换责任 MUST 有唯一的归属点（macOS 的 `RunEvent::Opened` 分支），MUST NOT 由下游层各自兜底。

具体约束：

- macOS 投递的 `file://` URL MUST 经 `url::Url::to_file_path()` 处理——它同时完成 scheme 校验与百分号解码——其输出才可进入启动 FIFO、`FILE_OPEN_REQUEST` 事件与播放链路。
- 非 `file` scheme（`http://`、`smb://`、`mms://`…）MUST 被丢弃并记录告警，MUST NOT 以任何形态进入播放链路或事件载荷。MUST NOT 依赖下游的 `is_local_media_path` 来承担这一层判断。
- 壳（`main.rs`）与运行时（`StartupSupervisor`）之间的文件打开契约 MUST 由**类型**承载（路径类型），使「把 URL 原文当路径下发」无法编译通过，而不是只在注释里声明载荷语义。
- 前端 MUST NOT 对已归一化的载荷做二次解码。
- 归一化 MUST 对两条入口产生一致语义：macOS 的 `RunEvent::Opened` 与单实例 argv 支路归一化后落在同一形态上。

#### Scenario: 带百分号编码的 file URL 被归一化

- **WHEN** macOS 通过文件关联下发 `file:///Users/…/We%20Will%20Rock%20You%20-%20Queen.flac`
- **THEN** 壳把载荷归一化为 `/Users/…/We Will Rock You - Queen.flac`（无 scheme 前缀、百分号已解码），该路径进入启动 FIFO 与事件载荷，界面标题显示解码后的文件名，且文件作为临时播放项（或命中活动资料库时以其既有 UUID）正常出声

#### Scenario: 非文件 scheme 被丢弃

- **WHEN** 打开事件携带 `http://…` 或其他非 `file` scheme
- **THEN** 系统丢弃该载荷并记录告警，不创建播放项、不发出 `FILE_OPEN_REQUEST` 事件、不向 mpv 投递任何加载，且不改变当前播放状态

#### Scenario: 归一化契约由类型强制

- **WHEN** 检查壳侧文件打开链路的签名（`deliver_file_open`、`StartupSupervisor::receive_file_open`、待处理打开队列）
- **THEN** 每个从 OS 边界接收入的参数都是文件系统路径类型；以字符串（URL 原文）转递的实现无法通过编译

#### Scenario: 前端不得二次解码

- **WHEN** 事件载荷已是解码后的路径，且其文件名**合法地**包含 `%20` 字面量
- **THEN** 前端不做 URL 解码，显示名直接取路径末段（`%20` 原样呈现是该文件名的正确显示）

#### Scenario: 纵深防御不得被放宽

- **WHEN** 修复本次的边界缺陷
- **THEN** actor 对含 `://` 的路径的拒绝行为与 `protocol-whitelist=file` 保持不变；修复发生在该拒绝的**上游**，MUST NOT 通过放宽这一层来让文件「能播放」
