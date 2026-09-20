## ADDED Requirements

### Requirement: 加载尝试必须重置进度事实

每次开始一次加载时——无论来源是库内歌曲、暂停态库内歌曲还是临时播放项——actor MUST 先把快照中的 `position` 与 `duration` 重置为空，再进入加载流程。

被拒绝或失败的加载 MUST NOT 让界面继续展示**上一次成功加载文件**的时长或进度：失败快照的 `position`/`duration` 必须为空。这条约束存在的理由是诊断噪音——残留的上一次时长会让人误以为该文件已经被加载过（实测中「无法播放」时界面仍显示上一个文件的 `5:18`）。

本条不改变失败原因的呈现方式，也不改变队列推进语义（后者由 `播放错误处理` 定义）。

#### Scenario: 被拒绝的加载不残留上次时长

- **WHEN** 一次加载因路径形态不合法（含 `://`）被 actor 拒绝并置为 `Failed`
- **THEN** 该失败快照的 `duration` 与 `position` 均为空，界面不展示上一次成功加载文件的数值，并向用户反馈失败原因

#### Scenario: 库内歌曲解析失败同样清空

- **WHEN** 库内歌曲的路径解析器无法解析出路径，加载直接置为失败
- **THEN** 该失败快照的 `duration` 与 `position` 均为空，与路径形态被拒绝的情形语义一致

#### Scenario: 进入 Loading 时不携带旧进度

- **WHEN** 新的加载开始并进入 `Loading`
- **THEN** 该快照的 `duration` 与 `position` 为空，直到 mpv 为新文件上报 `duration`/`time-pos` 才出现数值

#### Scenario: 加载成功后的进度仍连续

- **WHEN** 新文件加载成功并开始播放
- **THEN** 快照的 `duration`/`position` 由新文件的上报值驱动，进度、歌词高亮与播放统计阈值判定的既有行为不变
